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
pub const DEMO_TTL: Duration = Duration::from_secs(3600);
const REFRESH_MARGIN_SECS: i64 = 60;

pub struct Auth {
    pub oidc: Option<oidc::Oidc>,
    pub local: Option<local::Local>,
    pub public_url: Url,
    /// OIDC email to role (RFC 0016)
    roles: std::collections::BTreeMap<String, String>,
    pub demo: Option<crate::config::Demo>,
    local_role: Option<String>,
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
    role: Option<String>,
    demo: bool,
}

pub struct User {
    pub name: String,
    pub id_token: Option<String>,
    pub role: Option<String>,
}

/// `[auth] roles` with lowercased emails: the provider's email is lowercased
/// too (see oidc), as for the allowlist.
fn oidc_roles(cfg: &Config) -> std::collections::BTreeMap<String, String> {
    cfg.auth
        .iter()
        .flat_map(|a| &a.roles)
        .map(|(email, role)| (email.to_lowercase(), role.clone()))
        .collect()
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
            roles: oidc_roles(cfg),
            demo: cfg.demo.clone(),
            local_role: cfg.local_login.as_ref().and_then(|l| l.role.clone()),
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
        let role = self.role_for(&t.email);
        tracing::info!(msg = "login", method = "oidc", user = %t.email, role = role.as_deref().unwrap_or(""), result = "ok");
        login("oidc", "ok");
        Ok(self.create(Session {
            user: t.email,
            id_token: Some(t.id_token),
            id_exp: t.id_exp,
            refresh: t.refresh,
            created: Instant::now(),
            role,
            demo: false,
        }))
    }

    pub async fn local_login(&self, username: &str, password: &str) -> Result<String> {
        let local = self
            .local
            .as_ref()
            .ok_or("password login is not configured")?;
        if let Err(e) = local.verify(username, password).await {
            tracing::warn!(msg = "login", method = "local", user = %username, result = "denied");
            login("local", "denied");
            return Err(e);
        }
        tracing::info!(msg = "login", method = "local", user = %username, result = "ok");
        login("local", "ok");
        Ok(self.create(Session {
            user: username.to_string(),
            id_token: None,
            id_exp: i64::MAX,
            refresh: None,
            created: Instant::now(),
            role: self.local_role.clone(),
            demo: false,
        }))
    }

    /// The role of an OIDC user; emails compare case-insensitively.
    fn role_for(&self, email: &str) -> Option<String> {
        self.roles.get(&email.to_lowercase()).cloned()
    }

    /// A passwordless session with the demo role (RFC 0016). At most
    /// `max_sessions` demo sessions exist; the oldest goes first.
    pub fn demo_login(&self) -> Result<String> {
        let demo = self.demo.as_ref().ok_or("the demo is not configured")?;
        {
            let mut sessions = self.sessions.lock().unwrap();
            let mut demos: Vec<(String, Instant)> = sessions
                .iter()
                .filter(|(_, s)| s.demo)
                .map(|(k, s)| (k.clone(), s.created))
                .collect();
            if demos.len() >= demo.max_sessions {
                demos.sort_by_key(|d| d.1);
                for (sid, _) in demos.iter().take(demos.len() + 1 - demo.max_sessions) {
                    sessions.remove(sid);
                }
            }
        }
        tracing::info!(msg = "login", method = "demo", role = %demo.role, result = "ok");
        login("demo", "ok");
        Ok(self.create(Session {
            user: "demo".into(),
            id_token: None,
            id_exp: i64::MAX,
            refresh: None,
            created: Instant::now(),
            role: Some(demo.role.clone()),
            demo: true,
        }))
    }

    /// The session's user, with an OIDC ID token refreshed when it is about
    /// to expire. `None` when the session is unknown, too old, or can no
    /// longer produce a valid ID token.
    pub async fn user(&self, sid: &str) -> Option<User> {
        let s = self.sessions.lock().unwrap().get(sid).cloned()?;
        let ttl = if s.demo { DEMO_TTL } else { SESSION_TTL };
        if s.created.elapsed() > ttl {
            self.logout(sid);
            return None;
        }
        let now = jiff::Timestamp::now().as_second();
        let (Some(oidc), Some(_)) = (&self.oidc, &s.id_token) else {
            return Some(User {
                name: s.user,
                id_token: s.id_token,
                role: s.role,
            });
        };
        if s.id_exp - now > REFRESH_MARGIN_SECS {
            return Some(User {
                name: s.user,
                id_token: s.id_token,
                role: s.role,
            });
        }
        match oidc.refresh(&s.user, s.refresh.as_deref()).await {
            Ok(t) => {
                let user = User {
                    name: s.user.clone(),
                    id_token: Some(t.id_token.clone()),
                    role: s.role.clone(),
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
                    role: s.role,
                })
            }
            Err(e) => {
                tracing::info!(msg = "session ended", user = %s.user, reason = %e);
                self.logout(sid);
                None
            }
        }
    }

    /// The session's user name as it is, without a token refresh.
    pub fn name(&self, sid: &str) -> Option<String> {
        Some(self.sessions.lock().unwrap().get(sid)?.user.clone())
    }

    /// Server-side sessions, for `homie_sessions_active` (RFC 0024).
    pub fn sessions(&self) -> usize {
        self.sessions.lock().unwrap().len()
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

fn login(method: &str, result: &str) {
    crate::metrics::inc(
        "homie_logins_total",
        &[("method", method), ("result", result)],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_roles_match_any_case() {
        let mut cfg = Config::default();
        let mut a: crate::config::Auth = serde_json::from_value(serde_json::json!({
            "issuer": "https://idp.example", "client_id": "homie"
        }))
        .unwrap();
        a.roles.insert("Viewer@Example.com".into(), "demo".into());
        cfg.auth = Some(a);
        let auth = Auth {
            oidc: None,
            local: None,
            public_url: Url::parse("https://homie.example").unwrap(),
            roles: oidc_roles(&cfg),
            demo: None,
            local_role: None,
            sessions: Mutex::new(HashMap::new()),
        };
        assert_eq!(auth.role_for("viewer@example.com").as_deref(), Some("demo"));
        assert_eq!(auth.role_for("VIEWER@example.COM").as_deref(), Some("demo"));
        assert_eq!(auth.role_for("other@example.com"), None);
    }

    #[tokio::test]
    async fn demo_sessions_are_capped_and_carry_the_role() {
        let auth = Auth {
            oidc: None,
            local: None,
            public_url: Url::parse("https://homie.example").unwrap(),
            roles: Default::default(),
            demo: Some(crate::config::Demo {
                role: "demo".into(),
                max_sessions: 2,
            }),
            local_role: None,
            sessions: Mutex::new(HashMap::new()),
        };
        let first = auth.demo_login().unwrap();
        let second = auth.demo_login().unwrap();
        let third = auth.demo_login().unwrap();
        assert!(
            auth.user(&first).await.is_none(),
            "the oldest demo session goes"
        );
        let u = auth.user(&third).await.unwrap();
        assert_eq!((u.name.as_str(), u.role.as_deref()), ("demo", Some("demo")));
        assert!(auth.user(&second).await.is_some());
    }

    /// Keeps `demo_login` equal to `demo_after` and `demo_role` in
    /// spec/sessions.bend, whose laws (LAWS.bend) are proven by
    /// `bend PROOF.bend`.
    #[tokio::test]
    async fn demo_cap_matches_spec() {
        for n in 0..5 {
            for m in 0..4 {
                let demo = |max_sessions| {
                    Some(crate::config::Demo {
                        role: "demo".into(),
                        max_sessions,
                    })
                };
                let mut auth = Auth {
                    oidc: None,
                    local: None,
                    public_url: Url::parse("https://homie.example").unwrap(),
                    roles: Default::default(),
                    demo: demo(100),
                    local_role: None,
                    sessions: Mutex::new(HashMap::new()),
                };
                for _ in 0..n {
                    auth.demo_login().unwrap();
                }
                auth.demo = demo(1 + m);
                let sid = auth.demo_login().unwrap();
                let count = auth
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|s| s.demo)
                    .count();
                assert_eq!(count, 1 + n.min(m), "n={n} m={m}");
                let u = auth.user(&sid).await.unwrap();
                assert_eq!(u.role.as_deref(), Some("demo"), "n={n} m={m}");
            }
        }
    }

    #[tokio::test]
    async fn no_login_method_means_no_auth() {
        let cfg = crate::server::harness::config("");
        assert!(Auth::new(&cfg).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn plain_http_cookies_and_missing_methods() {
        let cfg = crate::server::harness::config(&format!(
            "public_url = \"http://homie.lan:7777\"\nlisten = \"0.0.0.0:7777\"\n\
             [local_login]\npassword_hash = \"{}\"\n",
            crate::server::harness::password_hash()
        ));
        let auth = Auth::new(&cfg).await.unwrap().unwrap();
        assert_eq!(auth.cookie_name(), "homie");
        assert_eq!(
            auth.cookie("abc", 60),
            "homie=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=60"
        );
        assert_eq!(auth.probe(), "password login for \"homie\"");
        let e = |r: Result<String>| r.unwrap_err().to_string();
        assert_eq!(
            e(auth.oidc_callback("c", "s").await),
            "OIDC login is not configured"
        );
        assert_eq!(e(auth.demo_login()), "the demo is not configured");
        assert_eq!(
            e(auth.local_login("homie", "wrong").await),
            "wrong username or password"
        );
        let sid = auth
            .local_login("homie", crate::server::harness::PASSWORD)
            .await
            .unwrap();
        let u = auth.user(&sid).await.unwrap();
        assert_eq!((u.name.as_str(), u.id_token, u.role), ("homie", None, None));
        auth.logout(&sid);
        assert!(auth.user(&sid).await.is_none());

        let none = Auth {
            local: None,
            ..auth
        };
        assert_eq!(
            e(none.local_login("homie", "x").await),
            "password login is not configured"
        );
    }

    #[tokio::test]
    async fn old_sessions_end() {
        let auth = Auth {
            oidc: None,
            local: None,
            public_url: Url::parse("https://homie.example").unwrap(),
            roles: Default::default(),
            demo: Some(crate::config::Demo {
                role: "demo".into(),
                max_sessions: 5,
            }),
            local_role: None,
            sessions: Mutex::new(HashMap::new()),
        };
        let sid = auth.demo_login().unwrap();
        // an Instant before the machine booted cannot exist; skip then
        let Some(old) = Instant::now().checked_sub(DEMO_TTL + Duration::from_secs(1)) else {
            return;
        };
        auth.sessions.lock().unwrap().get_mut(&sid).unwrap().created = old;
        assert!(auth.user(&sid).await.is_none());
        assert!(auth.sessions.lock().unwrap().is_empty(), "and are removed");
    }

    /// An OIDC session whose ID token expires at `exp` (seconds from now).
    fn oidc_session(auth: &Auth, exp: i64, refresh: Option<&str>) -> String {
        auth.create(Session {
            user: "me@example.com".into(),
            id_token: Some("old".into()),
            id_exp: jiff::Timestamp::now().as_second() + exp,
            refresh: refresh.map(String::from),
            created: Instant::now(),
            role: None,
            demo: false,
        })
    }

    #[tokio::test]
    async fn id_tokens_are_refreshed_or_the_session_ends() {
        use crate::server::harness::provider;
        let op = provider(serde_json::json!({"email": "me@example.com"})).await;
        let cfg = crate::server::harness::config(&format!(
            "public_url = \"https://homie.example\"\n{}\n[roles.viewer]\npages = [\"infra\"]\n",
            op.section()
        ));
        let auth = Auth::new(&cfg).await.unwrap().unwrap();
        assert_eq!(auth.probe(), format!("OIDC provider {} discovered", op.url));

        // far from expiry: no refresh
        let sid = oidc_session(&auth, 3600, Some("r1"));
        assert_eq!(
            auth.user(&sid).await.unwrap().id_token.as_deref(),
            Some("old")
        );

        // close to expiry: a new ID token, the refresh token is kept
        let sid = oidc_session(&auth, 10, Some("r1"));
        let fresh = auth.user(&sid).await.unwrap().id_token.unwrap();
        assert_ne!(fresh, "old");
        let s = auth.sessions.lock().unwrap()[&sid].clone();
        assert_eq!(s.refresh.as_deref(), Some("r1"));
        assert!(s.id_exp > jiff::Timestamp::now().as_second() + 500);

        // expired and no refresh token: the session ends
        let sid = oidc_session(&auth, -10, None);
        assert!(auth.user(&sid).await.is_none());
        assert!(!auth.sessions.lock().unwrap().contains_key(&sid));

        // expired and the provider refuses: the session ends too
        op.set(|s| s.refresh_ok = false);
        let sid = oidc_session(&auth, -10, Some("r1"));
        assert!(auth.user(&sid).await.is_none());
    }
}
