//! Static UI: serve the embedded `frontend/dist` bundle, with an SPA fallback to `index.html`
//! for any non-`/api` GET that doesn't map to an embedded file.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// The Vue production bundle, embedded at compile time.
///
/// The folder path is resolved relative to `CARGO_MANIFEST_DIR` (this crate) by rust-embed,
/// so `../../frontend/dist` points at the repo's `frontend/dist`. The `$CARGO_MANIFEST_DIR`
/// interpolation form the plan suggests needs rust-embed's `interpolate-folder-path` feature,
/// which pulls in a new crate (`shellexpand`) and would disturb the settled `Cargo.lock`; a
/// plain relative path embeds the exact same bundle without touching dependencies.
#[derive(Embed)]
#[folder = "../../frontend/dist"]
struct Assets;

/// Router fallback for everything the `/api` routes don't handle.
///
/// - `/api/*` that reached here is an unknown API path -> `404`.
/// - `/` or a path matching an embedded file -> that file, with its guessed mime.
/// - anything else -> `index.html` (client-side routing / SPA fallback).
pub(crate) async fn static_handler(uri: Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();

    // Unknown API routes must not be masked by the SPA fallback.
    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if path.is_empty() {
        path = "index.html".to_string();
    }

    match Assets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => index_html(),
    }
}

/// Serve `index.html`, or `404` if the bundle wasn't embedded.
fn index_html() -> Response {
    match Assets::get("index.html") {
        Some(content) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
            content.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "index.html not embedded").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_bundle_is_embedded() {
        // The build embeds a real bundle; index.html must be present for the SPA fallback.
        assert!(Assets::get("index.html").is_some());
    }

    #[tokio::test]
    async fn unknown_api_path_is_404() {
        let resp = static_handler("/api/does-not-exist".parse::<Uri>().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn root_serves_index_html() {
        let resp = static_handler("/".parse::<Uri>().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("text/html"));
    }

    #[tokio::test]
    async fn unknown_ui_path_falls_back_to_index() {
        // A client-side route (no embedded file) should still return 200 (index.html).
        let resp = static_handler("/some/deep/link".parse::<Uri>().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
