//! OIDC login (authorization code + PKCE) with any provider that supports
//! discovery. Tokens stay on the server; the session layer keeps them.

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

const LOGIN_TTL: Duration = Duration::from_secs(600);

pub struct Oidc {
    client: OidcClient,
    http: reqwest::Client,
    cfg: config::Auth,
    pending: Mutex<HashMap<String, Pending>>,
}

struct Pending {
    verifier: PkceCodeVerifier,
    nonce: Nonce,
    at: Instant,
}

/// What a login or a refresh yields.
pub struct Tokens {
    pub email: String,
    pub id_token: String,
    pub id_exp: i64,
    pub refresh: Option<String>,
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

fn raw(token: &CoreIdToken) -> Result<String> {
    Ok(serde_json::to_value(token)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

impl Oidc {
    pub async fn new(cfg: &config::Auth, public_url: &Url) -> Result<Self> {
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

    /// Finishes the login and checks the allowlists.
    pub async fn callback(&self, code: &str, state: &str) -> Result<Tokens> {
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
            tracing::warn!(msg = "login", method = "oidc", user = %email, result = "denied");
            super::login("oidc", "denied");
            return Err(format!("{email} is not allowed to use this Homie").into());
        }
        Ok(Tokens {
            email,
            id_token: raw(id_token)?,
            id_exp: claims["exp"].as_i64().unwrap_or(0),
            refresh: token.refresh_token().map(|r| r.secret().clone()),
        })
    }

    /// A fresh ID token from the refresh token.
    pub async fn refresh(&self, email: &str, refresh: Option<&str>) -> Result<Tokens> {
        let rt = RefreshToken::new(
            refresh
                .ok_or("no refresh token (request offline_access)")?
                .to_string(),
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
        Ok(Tokens {
            email: email.to_string(),
            id_token: raw(id_token)?,
            id_exp: claims["exp"].as_i64().unwrap_or(0),
            refresh: token
                .refresh_token()
                .map(|r| r.secret().clone())
                .or_else(|| refresh.map(String::from)),
        })
    }

    pub fn probe(&self) -> String {
        format!("OIDC provider {} discovered", self.cfg.issuer)
    }
}
