//! HTTP routing: ttyd's endpoints plus the fttyd API surface.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::auth::{self, Auth};
use crate::state::{AppState, AuthUser};
use crate::ws;

pub fn router(state: Arc<AppState>) -> Router {
    let cfg = &state.cfg;
    let mut router = Router::new()
        .route(&cfg.index_path(), get(index_handler))
        .route(&cfg.token_path(), get(token_handler))
        .route(&cfg.ws_path(), get(ws::ws_handler))
        .route(&format!("{}assets/{{*path}}", cfg.index_path()), get(asset_handler))
        .route(&format!("{}api/info", cfg.index_path()), get(crate::api::info))
        .route(&format!("{}api/download", cfg.index_path()), get(crate::api::download))
        .route(&format!("{}api/archive", cfg.index_path()), get(crate::api::archive))
        .route(&format!("{}api/stats", cfg.index_path()), get(stats_handler))
        .route(
            &format!("{}api/upload", cfg.index_path()),
            axum::routing::put(crate::api::upload_put).post(crate::api::upload_multipart),
        )
        .route(
            &format!("{}api/fs", cfg.index_path()),
            get(crate::api::list)
                .patch(crate::api::mutate)
                .delete(crate::api::delete),
        )
        .fallback(not_found);

    if !cfg.base_path.is_empty() {
        // ttyd redirects `/base-path` to `/base-path/`.
        router = router.route(&cfg.base_path, get(redirect_handler));
    }

    router
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

async fn authenticate(State(state): State<Arc<AppState>>, mut request: Request, next: Next) -> Response {
    match auth::check(&state.cfg, request.headers()) {
        Auth::Allowed { user } => {
            request.extensions_mut().insert(AuthUser(user));
            next.run(request).await
        }
        Auth::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"ttyd\""))],
            "",
        )
            .into_response(),
        Auth::ProxyUnauthorized => (
            StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            [(header::PROXY_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"ttyd\""))],
            "",
        )
            .into_response(),
    }
}

async fn index_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(path) = &state.cfg.index {
        return match tokio::fs::read(path).await {
            Ok(body) => html_response(body),
            Err(err) => {
                tracing::error!("failed to read index {}: {err}", path.display());
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to read index").into_response()
            }
        };
    }
    if not_modified(&headers, &index_etag()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mut response = html_response(crate::assets::index_html());
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&index_etag()).unwrap_or(HeaderValue::from_static("\"0\"")),
    );
    response
}

fn html_response(body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/html")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

fn index_etag() -> String {
    // The embedded index changes only when the binary does.
    format!("\"{}\"", cli_version())
}

fn cli_version() -> &'static str {
    crate::cli::VERSION
}

/// ttyd's `/token`: it echoes the configured credential, which is what its client
/// sends back in the `AuthToken` field of the control message.
async fn token_handler(State(state): State<Arc<AppState>>) -> Response {
    let credential = state.cfg.credential.as_deref().unwrap_or("");
    let body = format!("{{\"token\": \"{credential}\"}}");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json;charset=utf-8"))],
        body,
    )
        .into_response()
}

async fn redirect_handler(State(state): State<Arc<AppState>>) -> Response {
    let location = HeaderValue::from_str(&state.cfg.index_path())
        .unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::FOUND, [(header::LOCATION, location)], "").into_response()
}

async fn asset_handler(State(_state): State<Arc<AppState>>, Path(path): Path<String>, headers: HeaderMap) -> Response {
    // The route prefix `/assets/` is not part of the capture, but embedded paths
    // are bundler-relative, so it has to be restored.
    let path = format!("assets/{}", path.trim_start_matches('/'));
    let Some(asset) = crate::assets::get(&path) else {
        return (StatusCode::NOT_FOUND, "asset not found").into_response();
    };
    if not_modified(&headers, &asset.etag) {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    let cache = if asset.immutable {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-cache")
    };

    let gzip = asset.is_compressible() && accepts_gzip(&headers);
    let body = if gzip { crate::assets::gzipped(&path, &asset.body) } else { Arc::new(asset.body) };

    let mut response = Response::new(Body::from(body.as_ref().clone()));
    let headers_out = response.headers_mut();
    headers_out.insert(header::CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    headers_out.insert(header::CACHE_CONTROL, cache);
    if let Ok(value) = HeaderValue::from_str(&asset.etag) {
        headers_out.insert(header::ETAG, value);
    }
    if gzip {
        headers_out.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        headers_out.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    response
}

async fn stats_handler(State(state): State<Arc<AppState>>) -> Response {
    crate::stats::stream(state.stats.subscribe()).into_response()
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').any(|candidate| candidate.trim() == etag))
        .unwrap_or(false)
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("gzip"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        AppState::new(crate::cli::Config { command: vec!["bash".into()], ..crate::cli::Config::default() }, crate::stats::Stats::new())
    }

    async fn status(app: &Router, uri: &str) -> StatusCode {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        app.clone().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn routes_index_token_and_assets() {
        let app = router(test_state());
        assert_eq!(status(&app, "/").await, StatusCode::OK);
        assert_eq!(status(&app, "/token").await, StatusCode::OK);
        assert_eq!(status(&app, "/nope").await, StatusCode::NOT_FOUND);
        let js = crate::assets::paths().into_iter().find(|path| path.ends_with(".js")).unwrap();
        assert_eq!(status(&app, &format!("/{js}")).await, StatusCode::OK);
        // A nested path below assets/ still resolves.
        assert_eq!(status(&app, "/assets/nested/missing.js").await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn token_echoes_the_credential() {
        let mut cfg = crate::cli::Config { command: vec!["bash".into()], ..crate::cli::Config::default() };
        cfg.credential = Some("dXNlcjpwYXNz".into());
        let app = router(AppState::new(cfg, crate::stats::Stats::new()));
        let request = Request::builder()
            .uri("/token")
            .header("authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), br#"{"token": "dXNlcjpwYXNz"}"#);
    }

    #[tokio::test]
    async fn missing_credential_is_challenged() {
        let mut cfg = crate::cli::Config { command: vec!["bash".into()], ..crate::cli::Config::default() };
        cfg.credential = Some("dXNlcjpwYXNz".into());
        let app = router(AppState::new(cfg, crate::stats::Stats::new()));
        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Basic realm=\"ttyd\"");
    }

    #[tokio::test]
    async fn base_path_moves_every_endpoint() {
        let mut cfg = crate::cli::Config { command: vec!["bash".into()], ..crate::cli::Config::default() };
        cfg.base_path = "/mounted/here".into();
        let app = router(AppState::new(cfg, crate::stats::Stats::new()));
        assert_eq!(status(&app, "/mounted/here").await, StatusCode::FOUND);
        assert_eq!(status(&app, "/mounted/here/").await, StatusCode::OK);
        assert_eq!(status(&app, "/mounted/here/token").await, StatusCode::OK);
        assert_eq!(status(&app, "/").await, StatusCode::NOT_FOUND);
    }

    #[test]
    fn gzip_is_only_used_when_accepted() {
        let mut headers = HeaderMap::new();
        assert!(!accepts_gzip(&headers));
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("deflate"));
        assert!(!accepts_gzip(&headers));
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert!(accepts_gzip(&headers));
    }

    #[test]
    fn etag_matching_handles_lists() {
        let mut headers = HeaderMap::new();
        assert!(!not_modified(&headers, "\"a\""));
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"b\", \"a\""));
        assert!(not_modified(&headers, "\"a\""));
    }
}
