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

pub type Consoles = std::sync::Mutex<std::collections::HashMap<String, Pending>>;

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
        crate::metrics::action("console", "denied");
        return error(StatusCode::FORBIDDEN, "consoles are off for this Proxmox");
    }
    let console = match pve.console(&req).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(msg = "console", user = user(&caller), guest, result = "error", error = %e);
            crate::metrics::action("console", "error");
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
fn take(consoles: &Consoles, id: &str, sid: Option<&str>, pve: &str) -> Option<Pending> {
    let p = consoles.lock().unwrap().remove(id)?;
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
    let Some(p) = take(&state.consoles, &id, caller.sid.as_deref(), &name) else {
        tracing::info!(
            msg = "console",
            user = user(&caller),
            pve = name,
            result = "denied",
            reason = "expired id"
        );
        crate::metrics::action("console", "denied");
        return error(StatusCode::FORBIDDEN, "this console expired: open it again");
    };
    let Some(pve) = state.lab.proxmox.get(&name) else {
        return error(StatusCode::NOT_FOUND, "no such Proxmox");
    };
    let upstream = match pve.console_connect(&p.console).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(msg = "console", user = user(&caller), pve = name, guest = p.console.guest, result = "error", error = %e);
            crate::metrics::action("console", "error");
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
    crate::metrics::action("console", "ok");
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
    let _open = crate::metrics::Open::new(&crate::metrics::CONSOLES);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps `take` equal to `granted` and `kept` in spec/console.bend, whose
    /// laws (LAWS.bend) are proven by `bend PROOF.bend`.
    #[test]
    fn take_matches_spec() {
        let b = [false, true];
        for present in b {
            for fresh in b {
                for same_session in b {
                    for same_pve in b {
                        let consoles = Consoles::default();
                        if present {
                            let age = if fresh {
                                Duration::ZERO
                            } else {
                                ID_TTL + Duration::from_secs(1)
                            };
                            consoles.lock().unwrap().insert(
                                "id".into(),
                                Pending {
                                    sid: Some("s1".into()),
                                    pve: "lab".into(),
                                    console: Console::default(),
                                    created: Instant::now() - age,
                                },
                            );
                        }
                        let sid = if same_session { "s1" } else { "s2" };
                        let pve = if same_pve { "lab" } else { "other" };
                        let got = take(&consoles, "id", Some(sid), pve).is_some();
                        let case = format!(
                            "present={present} fresh={fresh} session={same_session} pve={same_pve}"
                        );
                        assert_eq!(got, present && fresh && same_session && same_pve, "{case}");
                        // kept: the id is gone after any attempt
                        assert!(consoles.lock().unwrap().is_empty(), "{case}");
                        assert!(take(&consoles, "id", Some(sid), pve).is_none(), "{case}");
                    }
                }
            }
        }
    }

    /// A Proxmox with two running VMs on VNC: 102's WebSocket pings, then
    /// echoes until "bye", which it answers with a Close; 104's fails.
    async fn pve() -> String {
        use axum::{
            Router,
            extract::ws::Message,
            routing::{get, post},
        };
        let data = |v: serde_json::Value| Json(json!({ "data": v }));
        let app = Router::new()
            .route(
                "/api2/json/cluster/resources",
                get(move || async move {
                    data(json!([
                        {"type": "qemu", "vmid": 102, "name": "vga", "node": "pve1", "status": "running"},
                        {"type": "qemu", "vmid": 104, "name": "broken", "node": "pve1", "status": "running"}
                    ]))
                }),
            )
            .route(
                "/api2/json/nodes/pve1/qemu/{vmid}/config",
                get(move || async move { data(json!({})) }),
            )
            .route(
                "/api2/json/nodes/pve1/qemu/{vmid}/vncproxy",
                post(move || async move {
                    data(json!({"port": "5902", "ticket": "PVEVNC:vnc", "user": "homie@pve!t"}))
                }),
            )
            .route(
                "/api2/json/nodes/pve1/qemu/{vmid}/vncwebsocket",
                get(|Path(vmid): Path<u64>, ws: WebSocketUpgrade| async move {
                    if vmid == 104 {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    ws.protocols(["binary"])
                        .on_upgrade(|mut s| async move {
                            let _ = s.send(Message::Ping(vec![1].into())).await;
                            while let Some(Ok(m)) = s.recv().await {
                                let bye = m == Message::Text("bye".into());
                                let _ = s.send(if bye { Message::Close(None) } else { m }).await;
                            }
                        })
                        .into_response()
                }),
            );
        crate::testkit::serve(app).await
    }

    /// The console routes alone, every request made by `caller`.
    async fn routes(state: &Shared, caller: Caller) -> String {
        let app = axum::Router::new()
            .route("/open/{name}", axum::routing::post(open))
            .route("/ws/{name}/{id}", axum::routing::get(connect))
            .layer(axum::Extension(caller))
            .with_state(state.clone());
        crate::testkit::serve(app).await
    }

    type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    /// The console WebSocket, or the error that refused it.
    async fn ws(url: &str, name: &str, id: &str) -> std::result::Result<Ws, String> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("{}/ws/{name}/{id}", url.replace("http", "ws"))
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("origin", "http://homie.example".parse().unwrap());
        req.headers_mut()
            .insert("sec-websocket-protocol", "binary".parse().unwrap());
        let tcp = tokio::net::TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .unwrap();
        tokio_tungstenite::client_async(req, tcp)
            .await
            .map(|(s, _)| s)
            .map_err(|e| e.to_string())
    }

    async fn open_id(url: &str, name: &str, vmid: u64) -> (u16, serde_json::Value) {
        let r = reqwest::Client::new()
            .post(format!("{url}/open/{name}"))
            .json(&json!({"node": "pve1", "vmid": vmid, "type": "qemu"}))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }

    /// The next message that is not a Pong.
    async fn next(s: &mut Ws) -> Up {
        s.by_ref()
            .filter(|m| std::future::ready(!matches!(m, Ok(Up::Pong(_)))))
            .next()
            .await
            .unwrap()
            .unwrap()
    }

    fn close(reason: &str) -> Up {
        Up::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: 1000.into(),
            reason: reason.into(),
        }))
    }

    #[tokio::test]
    async fn relay_ends_and_says_why() {
        let pve = pve().await;
        let h = crate::server::harness::Homie::start(|_| {
            format!(
                "public_url = \"http://homie.example\"\n\
                 [local_login]\npassword_hash = \"{}\"\n\
                 [proxmox.lab]\nendpoints = [\"{pve}\"]\ntoken = \"homie@pve!t=secret\"\nconsole = true\n",
                crate::server::harness::password_hash()
            )
        })
        .await;
        let anon = routes(&h.state, Caller::default()).await;

        let (status, body) = open_id(&anon, "gone", 102).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (404, Some("no such Proxmox"))
        );

        // an id for a Proxmox that is no longer configured
        h.state.consoles.lock().unwrap().insert(
            "stale".into(),
            Pending {
                sid: None,
                pve: "gone".into(),
                console: Console::default(),
                created: Instant::now(),
            },
        );
        assert_eq!(
            ws(&anon, "gone", "stale").await.err(),
            Some("HTTP error: 404 Not Found".into())
        );

        // Proxmox refuses the WebSocket
        let (status, body) = open_id(&anon, "lab", 104).await;
        assert_eq!(status, 200, "{body}");
        let id = body["id"].as_str().unwrap();
        assert_eq!(
            ws(&anon, "lab", id).await.err(),
            Some("HTTP error: 502 Bad Gateway".into())
        );

        // pings both ways are not relayed; Proxmox closes after "bye"
        let (_, body) = open_id(&anon, "lab", 102).await;
        let mut s = ws(&anon, "lab", body["id"].as_str().unwrap())
            .await
            .unwrap();
        s.send(Up::Ping(vec![2].into())).await.unwrap();
        s.send(Up::text("hi")).await.unwrap();
        assert_eq!(next(&mut s).await, Up::text("hi"));
        s.send(Up::text("bye")).await.unwrap();
        assert_eq!(next(&mut s).await, close("closed by Proxmox"));

        // a session that no longer exists ends the console at the first check
        let ghost = routes(
            &h.state,
            Caller {
                sid: Some("ghost".into()),
                ..Default::default()
            },
        )
        .await;
        let (_, body) = open_id(&ghost, "lab", 102).await;
        let mut s = ws(&ghost, "lab", body["id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(next(&mut s).await, close("session ended"));
    }
}
