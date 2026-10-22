//! The web UI, embedded at build time from `web/dist` (read from disk in
//! debug builds). Unknown paths get `index.html` so client-side routes work.

use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
#[allow_missing = true]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path.starts_with("auth/") || path.starts_with("github/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let (name, file) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (path, f),
        None => match Assets::get("index.html") {
            Some(f) => ("index.html", f),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "web UI not built: run `bun run build` in web/ and rebuild",
                )
                    .into_response();
            }
        },
    };
    // file names under assets/ carry a content hash, so they never change
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache),
        ],
        file.data,
    )
        .into_response()
}
