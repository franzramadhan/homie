//! OIDC login (authorization code + PKCE) with any provider that supports
//! discovery, and server-side sessions. Tokens never reach the browser: the
//! cookie only carries an opaque session id.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, HttpRequest, HttpResponse, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreProviderMetadata},
};
use serde_json::Value;
use url::Url;

use crate::{Result, config};

type OidcClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

// ponytail: fixed session lifetime; make it configurable if anyone asks
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const LOGIN_TTL: Duration = Duration::from_secs(600);
const REFRESH_MARGIN_SECS: i64 = 60;
pub const COOKIE: &str = "__Host-homie";

pub struct Auth {
    client: OidcClient,
    http: reqwest::Client,
    cfg: config::Auth,
    pub public_url: Url,
    // ponytail: sessions live in memory; one replica, a restart means one silent re-login
    sessions: Mutex<HashMap<String, Session>>,
    pending: Mutex<HashMap<String, Pending>>,
}

struct Pending {
    verifier: PkceCodeVerifier,
    nonce: Nonce,
    at: Instant,
}

#[derive(Clone)]
struct Session {
    email: String,
    id_token: String,
    id_exp: i64,
    refresh: Option<String>,
    created: Instant,
}

pub struct User {
    pub email: String,
    pub id_token: String,
}

/// The oauth2 crate's HTTP client interface, on top of our reqwest.
async fn send(client: reqwest::Client, req: HttpRequest) -> Result<HttpResponse, reqwest::Error> {
    let resp = client.execute(reqwest::Request::try_from(req)?).await?;
    let mut out = openidconnect::http::Response::builder().status(resp.status());
    for (k, v) in resp.headers() {
        out = out.header(k, v);
    }
    Ok(out
        .body(resp.bytes().await?.to_vec())
        .expect("status and headers come from a valid response"))
}

/// Claims Homie needs beyond the standard ones (email under another name,
/// groups), read from the already verified token.
fn payload(token: &CoreIdToken) -> Result<Value> {
    let jwt = serde_json::to_value(token)?;
    let part = jwt
        .as_str()
        .and_then(|j| j.split('.').nth(1))
        .ok_or("malformed ID token")?;
    Ok(serde_json::from_slice(
        &URL_SAFE_NO_PAD.decode(part.trim_end_matches('='))?,
    )?)
}

impl Auth {
    pub async fn new(cfg: &config::Auth, public_url: &str) -> Result<Self> {
        let public_url = Url::parse(public_url)?;
        crate::http::init_tls();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_only(
                webpki_root_certs::TLS_SERVER_ROOT_CERTS
                    .iter()
                    .map(|c| reqwest::Certificate::from_der(c))
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .timeout(Duration::from_secs(15))
            .build()?;
        let meta =
            CoreProviderMetadata::discover_async(IssuerUrl::new(cfg.issuer.clone())?, &|r| {
                send(http.clone(), r)
            })
            .await
            .map_err(|e| format!("OIDC discovery for {}: {e}", cfg.issuer))?;
        let secret = match &cfg.client_secret {
            Some(s) => Some(ClientSecret::new(s.resolve().await?)),
            None => None,
        };
        let redirect = RedirectUrl::new(format!(
            "{}/auth/callback",
            public_url.as_str().trim_end_matches('/')
        ))?;
        let client = CoreClient::from_provider_metadata(
            meta,
            ClientId::new(cfg.client_id.resolve().await?),
            secret,
        )
        .set_redirect_uri(redirect);
        Ok(Self {
            client,
            http,
            cfg: cfg.clone(),
            public_url,
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// The provider URL to send the browser to.
    pub fn login_url(&self) -> String {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in self.cfg.scopes.iter().filter(|s| *s != "openid") {
            req = req.add_scope(Scope::new(s.clone()));
        }
        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, p| p.at.elapsed() < LOGIN_TTL);
        pending.insert(
            state.secret().clone(),
            Pending {
                verifier,
                nonce,
                at: Instant::now(),
            },
        );
        url.to_string()
    }

    /// Finishes the login. Returns the new session id and the user's email.
    pub async fn callback(&self, code: &str, state: &str) -> Result<(String, String)> {
        let p = self
            .pending
            .lock()
            .unwrap()
            .remove(state)
            .filter(|p| p.at.elapsed() < LOGIN_TTL)
            .ok_or("login expired or unknown, start again")?;
        let token = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))?
            .set_pkce_verifier(p.verifier)
            .request_async(&|r| send(self.http.clone(), r))
            .await
            .map_err(|e| format!("token exchange: {e}"))?;
        let id_token = token
            .id_token()
            .ok_or("the provider returned no ID token")?;
        id_token.claims(&self.client.id_token_verifier(), &p.nonce)?;
        let claims = payload(id_token)?;

        let email = claims[&self.cfg.email_claim]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        let groups: Vec<&str> = match &self.cfg.groups_claim {
            Some(c) => claims[c]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect(),
            None => Vec::new(),
        };
        let allowed = (!email.is_empty()
            && self
                .cfg
                .allowed_emails
                .iter()
                .any(|e| e.to_lowercase() == email))
            || groups
                .iter()
                .any(|g| self.cfg.allowed_groups.iter().any(|a| a == g));
        if !allowed {
            tracing::warn!(msg = "login", user = %email, result = "denied");
            return Err(format!("{email} is not allowed to use this Homie").into());
        }

        let sid = CsrfToken::new_random().secret().clone();
        self.sessions.lock().unwrap().insert(
            sid.clone(),
            Session {
                email: email.clone(),
                id_token: serde_json::to_value(id_token)?
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                id_exp: claims["exp"].as_i64().unwrap_or(0),
                refresh: token.refresh_token().map(|r| r.secret().clone()),
                created: Instant::now(),
            },
        );
        tracing::info!(msg = "login", user = %email, result = "ok");
        Ok((sid, email))
    }

    /// The session's user, with an ID token refreshed when it is about to
    /// expire. `None` when the session is unknown, too old, or can no longer
    /// produce a valid ID token.
    pub async fn user(&self, sid: &str) -> Option<User> {
        let s = self.sessions.lock().unwrap().get(sid).cloned()?;
        if s.created.elapsed() > SESSION_TTL {
            self.logout(sid);
            return None;
        }
        let now = jiff::Timestamp::now().as_second();
        if s.id_exp - now > REFRESH_MARGIN_SECS {
            return Some(User {
                email: s.email,
                id_token: s.id_token,
            });
        }
        match self.refresh(&s).await {
            Ok(fresh) => {
                let user = User {
                    email: fresh.email.clone(),
                    id_token: fresh.id_token.clone(),
                };
                self.sessions.lock().unwrap().insert(sid.to_string(), fresh);
                Some(user)
            }
            Err(e) if s.id_exp > now => {
                tracing::debug!(msg = "token refresh failed, current token still valid", error = %e);
                Some(User {
                    email: s.email,
                    id_token: s.id_token,
                })
            }
            Err(e) => {
                tracing::info!(msg = "session ended", user = %s.email, reason = %e);
                self.logout(sid);
                None
            }
        }
    }

    async fn refresh(&self, s: &Session) -> Result<Session> {
        let rt = RefreshToken::new(
            s.refresh
                .clone()
                .ok_or("no refresh token (request offline_access)")?,
        );
        let token = self
            .client
            .exchange_refresh_token(&rt)?
            .request_async(&|r| send(self.http.clone(), r))
            .await?;
        let id_token = token.id_token().ok_or("refresh returned no ID token")?;
        // refreshed ID tokens carry no nonce
        id_token.claims(&self.client.id_token_verifier(), |_: Option<&Nonce>| Ok(()))?;
        let claims = payload(id_token)?;
        Ok(Session {
            email: s.email.clone(),
            id_token: serde_json::to_value(id_token)?
                .as_str()
                .unwrap_or_default()
                .to_string(),
            id_exp: claims["exp"].as_i64().unwrap_or(0),
            refresh: token
                .refresh_token()
                .map(|r| r.secret().clone())
                .or_else(|| s.refresh.clone()),
            created: s.created,
        })
    }

    pub fn logout(&self, sid: &str) {
        self.sessions.lock().unwrap().remove(sid);
    }

    pub async fn probe(&self) -> Result<String> {
        Ok(format!("OIDC provider {} discovered", self.cfg.issuer))
    }
}
