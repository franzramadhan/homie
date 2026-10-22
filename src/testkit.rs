//! Test helpers: a local HTTP server with canned answers that stands in for
//! Proxmox, Grafana, GitHub, Tailscale, OpenBao, an OIDC provider or the
//! Kubernetes API.

use axum::{Json, Router, routing::get};
use serde_json::Value;

/// Serves `app` on a random loopback port; returns its base URL.
pub async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// A route answering `GET path` with `body`.
pub fn json(path: &str, body: Value) -> Router {
    Router::new().route(path, get(move || async move { Json(body) }))
}

/// A Kubernetes list response for `kind`.
pub fn list(kind: &str, items: Value) -> Value {
    serde_json::json!({ "kind": format!("{kind}List"), "apiVersion": "v1", "metadata": {}, "items": items })
}

/// A kube client for a `serve`d fake API server.
pub fn kube_client(url: &str) -> kube::Client {
    kube::Client::try_from(kube::Config::new(url.parse().unwrap())).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::Node;

    #[tokio::test]
    async fn fake_api_server() {
        let url = serve(json(
            "/api/v1/nodes",
            list("Node", serde_json::json!([{"metadata": {"name": "n1"}}])),
        ))
        .await;
        let nodes = kube::Api::<Node>::all(kube_client(&url))
            .list(&Default::default())
            .await
            .unwrap();
        assert_eq!(nodes.items[0].metadata.name.as_deref(), Some("n1"));
    }
}
