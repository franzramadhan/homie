//! Embedded guest consoles (RFC 0023). Opening one asks Proxmox for a console
//! ticket and keeps it here under a single-use id bound to the session; the
//! WebSocket on that id is relayed to Proxmox with Homie's token.

use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message as Down, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::{SinkExt, StreamExt};
use openidconnect::CsrfToken;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as Up;

use super::{Shared, guard};
use crate::integrations::{
    Caller,
    proxmox::{Console, ConsoleRequest, ConsoleStream},
};

/// How long an id from `open` waits for its WebSocket.
const ID_TTL: Duration = Duration::from_secs(30);
/// How often a running console checks that its session still exists.
const SESSION_CHECK: Duration = Duration::from_secs(30);

pub struct Pending {
    sid: Option<String>,
    pve: String,
    console: Console,
    created: Instant,
}

fn error(status: StatusCode, msg: impl ToString) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

fn user(caller: &Caller) -> &str {
    caller.email.as_deref().unwrap_or("local")
}

pub async fn open(
    State(state): State<Shared>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(name): Path<String>,
    Json(req): Json<ConsoleRequest>,
) -> Response {
    let Some(pve) = state.lab.proxmox.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such Proxmox");
    };
    let guest = format!("{name}/{}/{}", req.node, req.vmid);
    if !pve.console {
        tracing::info!(
            msg = "console",
            user = user(&caller),
            guest,
            result = "denied",
            reason = "off"
        );
        return error(StatusCode::FORBIDDEN, "consoles are off for this Proxmox");
    }
    let console = match pve.console(&req).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(msg = "console", user = user(&caller), guest, result = "error", error = %e);
            return error(StatusCode::BAD_REQUEST, e);
        }
    };
    let body = json!({
        "guest": console.guest, "mode": console.mode,
        "switchable": console.switchable, "password": console.password,
    });
    let id = CsrfToken::new_random().secret().clone();
    let mut pending = state.consoles.lock().unwrap();
    pending.retain(|_, p| p.created.elapsed() < ID_TTL);
    pending.insert(
        id.clone(),
        Pending {
            sid: caller.sid.clone(),
            pve: name,
            console,
            created: Instant::now(),
        },
    );
    let mut body = body;
    body["id"] = json!(id);
    Json(body).into_response()
}

/// Takes the pending console for this id: once, within `ID_TTL`, from the
/// session that opened it and on the same Proxmox.
fn take(state: &Shared, id: &str, sid: Option<&str>, pve: &str) -> Option<Pending> {
    let p = state.consoles.lock().unwrap().remove(id)?;
    (p.created.elapsed() < ID_TTL && p.sid.as_deref() == sid && p.pve == pve).then_some(p)
}

pub async fn connect(
    State(state): State<Shared>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    Path((name, id)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    // browsers always send Origin on a WebSocket; it must be this site
    if headers.get(axum::http::header::ORIGIN).is_none() || guard::cross_origin(&state, &headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request");
    }
    let Some(p) = take(&state, &id, caller.sid.as_deref(), &name) else {
        tracing::info!(
            msg = "console",
            user = user(&caller),
            pve = name,
            result = "denied",
            reason = "expired id"
        );
        return error(StatusCode::FORBIDDEN, "this console expired: open it again");
    };
    let Some(pve) = state.lab.proxmox.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such Proxmox");
    };
    let upstream = match pve.console_connect(&p.console).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(msg = "console", user = user(&caller), pve = name, guest = p.console.guest, result = "error", error = %e);
            return error(StatusCode::BAD_GATEWAY, e);
        }
    };
    tracing::info!(
        msg = "console",
        user = user(&caller),
        pve = name,
        guest = p.console.guest,
        mode = p.console.mode,
        result = "open"
    );
    let max = pve.console_max;
    ws.protocols(["binary"])
        .on_upgrade(move |socket| relay(state, caller, name, p, socket, upstream, max))
}

async fn relay(
    state: Shared,
    caller: Caller,
    pve: String,
    p: Pending,
    socket: WebSocket,
    upstream: ConsoleStream,
    max: Duration,
) {
    let (mut down_tx, mut down_rx) = socket.split();
    let (mut up_tx, mut up_rx) = upstream.split();
    let started = Instant::now();
    let deadline = tokio::time::sleep(max);
    tokio::pin!(deadline);
    let mut check = tokio::time::interval(SESSION_CHECK);
    let mut bytes: u64 = 0;
    let reason = loop {
        tokio::select! {
            m = down_rx.next() => {
                let m = match m {
                    Some(Ok(Down::Binary(b))) => Up::Binary(b),
                    Some(Ok(Down::Text(t))) => Up::text(t.as_str()),
                    Some(Ok(Down::Ping(_) | Down::Pong(_))) => continue,
                    _ => break "closed by the browser",
                };
                bytes += m.len() as u64;
                if up_tx.send(m).await.is_err() {
                    break "closed by Proxmox";
                }
            }
            m = up_rx.next() => {
                let m = match m {
                    Some(Ok(Up::Binary(b))) => Down::Binary(b),
                    Some(Ok(Up::Text(t))) => Down::Text(t.as_str().into()),
                    Some(Ok(Up::Ping(_) | Up::Pong(_) | Up::Frame(_))) => continue,
                    _ => break "closed by Proxmox",
                };
                bytes += m.clone().into_data().len() as u64;
                if down_tx.send(m).await.is_err() {
                    break "closed by the browser";
                }
            }
            _ = &mut deadline => break "time limit",
            _ = check.tick() => {
                if let (Some(auth), Some(sid)) = (&state.auth, &caller.sid)
                    && auth.user(sid).await.is_none()
                {
                    break "session ended";
                }
            }
        }
    };
    let _ = down_tx
        .send(Down::Close(Some(CloseFrame {
            code: 1000,
            reason: reason.into(),
        })))
        .await;
    let _ = up_tx.send(Up::Close(None)).await;
    tracing::info!(
        msg = "console",
        user = user(&caller),
        pve,
        guest = p.console.guest,
        result = "closed",
        reason,
        seconds = started.elapsed().as_secs(),
        bytes
    );
}
