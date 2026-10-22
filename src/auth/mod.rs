//! Login methods (OIDC, built-in username and password) and the server-side
//! sessions they create. The cookie only carries an opaque session id.

pub mod local;
pub mod oidc;

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use openidconnect::CsrfToken;
use url::Url;

use crate::{Result, config::Config};

// ponytail: fixed session lifetime; make it configurable if anyone asks
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const REFRESH_MARGIN_SECS: i64 = 60;

pub struct Auth {
    pub oidc: Option<oidc::Oidc>,
    pub local: Option<local::Local>,
    pub public_url: Url,
    // ponytail: sessions live in memory; one replica, a restart means one re-login
    sessions: Mutex<HashMap<String, Session>>,
}

#[derive(Clone)]
struct Session {
    user: String,
    // only OIDC sessions have one; forward_token clusters need it
    id_token: Option<String>,
    id_exp: i64,
    refresh: Option<String>,
    created: Instant,
}

pub struct User {
    pub name: String,
    pub id_token: Option<String>,
}

impl Auth {
    /// `None` when neither login method is configured.
    pub async fn new(cfg: &Config) -> Result<Option<Self>> {
        if cfg.auth.is_none() && cfg.local_login.is_none() {
            return Ok(None);
        }
        let public_url = Url::parse(&cfg.public_url)?;
        let oidc = match &cfg.auth {
            Some(a) => Some(oidc::Oidc::new(a, &public_url).await?),
            None => None,
        };
        let local = match &cfg.local_login {
            Some(l) => Some(local::Local::new(l, cfg.state_dir.as_deref()).await?),
            None => None,
        };
        Ok(Some(Self {
            oidc,
            local,
            public_url,
            sessions: Mutex::new(HashMap::new()),
        }))
    }

    /// `__Host-` cookies need HTTPS. A plain-HTTP public_url (local login on a
    /// LAN without TLS) gets a plain cookie name and no Secure flag.
    pub fn cookie_name(&self) -> &'static str {
        if self.secure() {
            "__Host-homie"
        } else {
            "homie"
        }
    }

    fn secure(&self) -> bool {
        self.public_url.scheme() == "https"
            || matches!(
                self.public_url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]")
            )
    }

    pub fn cookie(&self, sid: &str, max_age: u64) -> String {
        let secure = if self.secure() { "; Secure" } else { "" };
        format!(
            "{}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
            self.cookie_name()
        )
    }

    fn create(&self, s: Session) -> String {
        let sid = CsrfToken::new_random().secret().clone();
        self.sessions.lock().unwrap().insert(sid.clone(), s);
        sid
    }

    pub async fn oidc_callback(&self, code: &str, state: &str) -> Result<String> {
        let oidc = self.oidc.as_ref().ok_or("OIDC login is not configured")?;
        let t = oidc.callback(code, state).await?;
        tracing::info!(msg = "login", method = "oidc", user = %t.email, result = "ok");
        Ok(self.create(Session {
            user: t.email,
            id_token: Some(t.id_token),
            id_exp: t.id_exp,
            refresh: t.refresh,
            created: Instant::now(),
        }))
    }

    pub async fn local_login(&self, username: &str, password: &str) -> Result<String> {
        let local = self
            .local
            .as_ref()
            .ok_or("password login is not configured")?;
        if let Err(e) = local.verify(username, password).await {
            tracing::warn!(msg = "login", method = "local", user = %username, result = "denied");
            return Err(e);
        }
        tracing::info!(msg = "login", method = "local", user = %username, result = "ok");
        Ok(self.create(Session {
            user: username.to_string(),
            id_token: None,
            id_exp: i64::MAX,
            refresh: None,
            created: Instant::now(),
        }))
    }

    /// The session's user, with an OIDC ID token refreshed when it is about
    /// to expire. `None` when the session is unknown, too old, or can no
    /// longer produce a valid ID token.
    pub async fn user(&self, sid: &str) -> Option<User> {
        let s = self.sessions.lock().unwrap().get(sid).cloned()?;
        if s.created.elapsed() > SESSION_TTL {
            self.logout(sid);
            return None;
        }
        let now = jiff::Timestamp::now().as_second();
        let (Some(oidc), Some(_)) = (&self.oidc, &s.id_token) else {
            return Some(User {
                name: s.user,
                id_token: s.id_token,
            });
        };
        if s.id_exp - now > REFRESH_MARGIN_SECS {
            return Some(User {
                name: s.user,
                id_token: s.id_token,
            });
        }
        match oidc.refresh(&s.user, s.refresh.as_deref()).await {
            Ok(t) => {
                let user = User {
                    name: s.user.clone(),
                    id_token: Some(t.id_token.clone()),
                };
                let fresh = Session {
                    id_token: Some(t.id_token),
                    id_exp: t.id_exp,
                    refresh: t.refresh,
                    ..s
                };
                self.sessions.lock().unwrap().insert(sid.to_string(), fresh);
                Some(user)
            }
            Err(e) if s.id_exp > now => {
                tracing::debug!(msg = "token refresh failed, current token still valid", error = %e);
                Some(User {
                    name: s.user,
                    id_token: s.id_token,
                })
            }
            Err(e) => {
                tracing::info!(msg = "session ended", user = %s.user, reason = %e);
                self.logout(sid);
                None
            }
        }
    }

    pub fn logout(&self, sid: &str) {
        self.sessions.lock().unwrap().remove(sid);
    }

    pub fn probe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(o) = &self.oidc {
            parts.push(o.probe());
        }
        if let Some(l) = &self.local {
            parts.push(format!("password login for {:?}", l.username));
        }
        parts.join(", ")
    }
}
