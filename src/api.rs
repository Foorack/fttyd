//! HTTP API for the sidebar: listings, downloads, archives and (in `-W` mode)
//! mutations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::archive::{self, ArchiveFormat};
use crate::fs::{self, ApiError};
use crate::state::{AppState, AuthUser};

/// Header every mutating request must carry. Browsers cannot set a custom header on a
/// cross-site request without a preflight, and this server grants no CORS policy, so
/// a cross-site form post cannot reach a mutation endpoint even with basic auth.
pub const CSRF_HEADER: &str = "x-fttyd-token";

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    pub path: Option<String>,
}

fn roots(state: &AppState) -> &[PathBuf] {
    &state.cfg.file_roots
}

fn parse_path(query: &PathQuery) -> Result<PathBuf, ApiError> {
    let value = query
        .path
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("missing path"))?;
    if value.is_empty() {
        return Err(ApiError::bad_request("missing path"));
    }
    let path = std::path::PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    if value.contains('\0') {
        return Err(ApiError::bad_request("path contains a NUL byte"));
    }
    Ok(fs::normalize(&path))
}

/// Rejects mutations when the server is read-only, which is ttyd's default.
pub fn require_writable(state: &AppState) -> Result<(), ApiError> {
    if state.cfg.writable {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "server is read-only; start it with --writable",
        ))
    }
}

/// Validates the CSRF token on every non-GET request.
pub fn require_csrf(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = state.cfg.credential.as_deref().unwrap_or("");
    let presented = headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok());
    match presented {
        Some(value) if constant_time_eq(value.as_bytes(), expected.as_bytes()) => Ok(()),
        _ => Err(ApiError::forbidden("missing or invalid CSRF token")),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// Server capabilities, used by the sidebar to decide what to offer.
pub async fn info(State(state): State<Arc<AppState>>) -> Response {
    let cwd = state
        .cfg
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .map(|path| path.to_string_lossy().into_owned());
    let body = serde_json::json!({
        "version": crate::cli::VERSION,
        "writable": state.cfg.writable,
        "path": cwd,
        "roots": roots(&state).iter().map(|root| root.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "csrf_header": CSRF_HEADER,
    });
    json_response(body)
}

/// `GET /api/fs?path=/some/dir` — one directory level, sorted.
pub async fn list(State(state): State<Arc<AppState>>, Query(query): Query<PathQuery>) -> Response {
    let result = parse_path(&query)
        .and_then(|path| fs::list(roots(&state), &path))
        .and_then(|listing| {
            serde_json::to_value(listing)
                .map_err(|err| ApiError::internal(format!("failed to serialise listing: {err}")))
        });
    match result {
        Ok(value) => json_response(value),
        Err(err) => err.into_response(),
    }
}

/// `GET /api/download?path=/some/file` — single file, resumable.
pub async fn download(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Response {
    match download_inner(&state, &query, &headers).await {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

async fn download_inner(
    state: &Arc<AppState>,
    query: &PathQuery,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let path = parse_path(query)?;
    fs::enforce_roots(roots(state), &path)?;

    // Downloads follow a symlink to its target: clicking download on a link means "the
    // file behind it". `enforce_roots` canonicalises, so a link cannot escape the roots.
    let metadata = std::fs::metadata(&path)?;
    if metadata.is_dir() {
        return Err(ApiError::bad_request(
            "directories must be downloaded as an archive",
        ));
    }
    if !metadata.is_file() {
        return Err(ApiError::bad_request(
            "only regular files can be downloaded",
        ));
    }

    let mut file = tokio::fs::File::open(&path).await?;
    let metadata = file.metadata().await?;
    let len = metadata.len();

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let (start, end, partial) = match range.and_then(|value| parse_range(value, len)) {
        Some((start, end)) => (start, end, true),
        None => (0, len.saturating_sub(1), false),
    };

    if len == 0 {
        let mut response = Response::new(Body::empty());
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        finish_download_headers(&mut response, &path, len, None);
        return Ok(response);
    }

    let length = end - start + 1;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let stream = ReaderStream::new(file.take(length));
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    if partial {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let value = format!("bytes {start}-{end}/{len}");
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&value).unwrap_or(HeaderValue::from_static("bytes 0-0/0")),
        );
    }
    finish_download_headers(&mut response, &path, start + length, Some(len));
    Ok(response)
}

fn finish_download_headers(response: &mut Response, path: &Path, _served: u64, total: Option<u64>) {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(total) = total {
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        let _ = total;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    if let Ok(value) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        name.replace(['"', '\\', '\n', '\r'], "_")
    )) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
}

/// Parses `bytes=start-end`, `bytes=start-` and `bytes=-suffix`.
pub fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        // Multiple ranges are legal but rare; serving the whole file is acceptable.
        return None;
    }
    if len == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", "") => return None,
        ("", suffix) => {
            let suffix: u64 = suffix.parse().ok()?;
            if suffix == 0 {
                return None;
            }
            (len.saturating_sub(suffix), len - 1)
        }
        (start, "") => (start.parse().ok()?, len - 1),
        (start, end) => (start.parse().ok()?, end.parse::<u64>().ok()?.min(len - 1)),
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end))
}

#[derive(Debug, Deserialize)]
pub struct ArchiveQuery {
    pub path: Option<String>,
    pub format: Option<String>,
}

/// `GET /api/archive?path=/some/dir&format=zip|tar.gz` — streamed folder download.
pub async fn archive(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ArchiveQuery>,
) -> Response {
    let path = match parse_path(&PathQuery {
        path: query.path.clone(),
    }) {
        Ok(path) => path,
        Err(err) => return err.into_response(),
    };
    let format = match query.format.as_deref().map(ArchiveFormat::parse) {
        Some(Some(format)) => format,
        Some(None) => return ApiError::bad_request("format must be zip or tar.gz").into_response(),
        None => ArchiveFormat::Zip,
    };
    match archive::response(path, roots(&state).to_vec(), format) {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

fn json_response(value: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=utf-8"),
        )],
        value.to_string(),
    )
        .into_response()
}

/// Body of `PATCH /api/fs`: rename, mkdir or chmod.
#[derive(Debug, Deserialize)]
pub struct MutationRequest {
    pub op: String,
    pub path: Option<String>,
    pub to: Option<String>,
    /// Octal permissions string such as `"640"`.
    pub mode: Option<String>,
}

/// `PUT /api/upload?path=/dir/file` — raw body, resumable with `Content-Range`.
pub async fn upload_put(
    State(state): State<Arc<AppState>>,
    user: axum::extract::Extension<AuthUser>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match upload_put_inner(&state, &user.0.0, &query, &headers, body).await {
        Ok(value) => json_response(value),
        Err(err) => err.into_response(),
    }
}

async fn upload_put_inner(
    state: &Arc<AppState>,
    actor: &Option<String>,
    query: &PathQuery,
    headers: &HeaderMap,
    body: Body,
) -> Result<serde_json::Value, ApiError> {
    require_writable(state)?;
    require_csrf(state, headers)?;

    let target = parse_path(query)?;
    fs::enforce_roots(roots(state), &target)?;
    let parent = target
        .parent()
        .ok_or_else(|| ApiError::bad_request("target has no parent directory"))?;
    let parent_meta = std::fs::metadata(parent)?;
    if !parent_meta.is_dir() {
        return Err(ApiError::bad_request(
            "upload target must be inside a directory",
        ));
    }
    if std::fs::metadata(&target)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        return Err(ApiError::bad_request("target is a directory"));
    }

    let content_range = headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .map(parse_content_range)
        .transpose()?;
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    let (start, total) = match content_range {
        Some(range) => (range.start, Some(range.total)),
        None => (0, content_length),
    };

    if start == 0 {
        if let Some(total) = total {
            ensure_free_space(parent, total)?;
        }
    }

    let temporary = temporary_path(&target);
    if start == 0 {
        let _ = std::fs::remove_file(&temporary);
    } else {
        let existing = std::fs::metadata(&temporary)
            .map(|meta| meta.len())
            .unwrap_or(0);
        if existing != start {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                message: format!("resume offset {start} does not match {existing} bytes stored"),
            });
        }
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(start > 0)
        .truncate(start == 0)
        .open(&temporary)
        .await?;

    let mut written = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|err| ApiError::bad_request(format!("upload aborted: {err}")))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        written += chunk.len() as u64;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);

    let complete = match total {
        Some(total) => start + written == total,
        None => true,
    };
    if complete {
        std::fs::rename(&temporary, &target)?;
        tracing::info!(
            "file op: upload {} bytes to {} actor={}",
            start + written,
            target.display(),
            actor.as_deref().unwrap_or("-")
        );
    }

    Ok(serde_json::json!({
        "path": target.to_string_lossy(),
        "size": start + written,
        "complete": complete,
    }))
}

/// `POST /api/upload?path=/dir` — `multipart/form-data`, so plain `curl -F` works.
pub async fn upload_multipart(
    State(state): State<Arc<AppState>>,
    user: axum::extract::Extension<AuthUser>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Response {
    match upload_multipart_inner(&state, &user.0.0, &query, &headers, &mut multipart).await {
        Ok(value) => json_response(value),
        Err(err) => err.into_response(),
    }
}

async fn upload_multipart_inner(
    state: &Arc<AppState>,
    actor: &Option<String>,
    query: &PathQuery,
    headers: &HeaderMap,
    multipart: &mut axum::extract::Multipart,
) -> Result<serde_json::Value, ApiError> {
    require_writable(state)?;
    require_csrf(state, headers)?;

    let directory = parse_path(query)?;
    fs::enforce_roots(roots(state), &directory)?;
    if !std::fs::metadata(&directory)?.is_dir() {
        return Err(ApiError::bad_request("upload target must be a directory"));
    }

    let mut stored = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::bad_request(format!("malformed upload: {err}")))?
    {
        let Some(file_name) = field.file_name().map(|name| name.to_string()) else {
            continue;
        };
        let relative = safe_relative_path(&file_name)?;
        let target = directory.join(&relative);
        fs::enforce_roots(roots(state), &target)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = tokio::fs::File::create(&target).await?;
        let mut size = 0u64;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|err| ApiError::bad_request(format!("malformed upload: {err}")))?
        {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            size += chunk.len() as u64;
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        tracing::info!(
            "file op: upload {} bytes to {} actor={}",
            size,
            target.display(),
            actor.as_deref().unwrap_or("-")
        );
        stored.push(serde_json::json!({
            "path": target.to_string_lossy(),
            "size": size,
        }));
    }

    Ok(serde_json::json!({ "files": stored }))
}

/// `PATCH /api/fs` — rename, mkdir and chmod.
pub async fn mutate(
    State(state): State<Arc<AppState>>,
    user: axum::extract::Extension<AuthUser>,
    headers: HeaderMap,
    axum::Json(request): axum::Json<MutationRequest>,
) -> Response {
    match mutate_inner(&state, &user.0.0, &headers, &request).await {
        Ok(value) => json_response(value),
        Err(err) => err.into_response(),
    }
}

async fn mutate_inner(
    state: &Arc<AppState>,
    actor: &Option<String>,
    headers: &HeaderMap,
    request: &MutationRequest,
) -> Result<serde_json::Value, ApiError> {
    require_writable(state)?;
    require_csrf(state, headers)?;

    let path = parse_path(&PathQuery {
        path: request.path.clone(),
    })?;
    fs::enforce_roots(roots(state), &path)?;
    let actor = actor.as_deref().unwrap_or("-");

    match request.op.as_str() {
        "mkdir" => {
            if std::fs::symlink_metadata(&path).is_ok() {
                return Err(ApiError {
                    status: StatusCode::CONFLICT,
                    message: format!("{} already exists", path.display()),
                });
            }
            std::fs::create_dir(&path)?;
            tracing::info!("file op: mkdir {} actor={actor}", path.display());
        }
        "rename" => {
            let to = parse_path(&PathQuery {
                path: request.to.clone(),
            })?;
            fs::enforce_roots(roots(state), &to)?;
            if std::fs::symlink_metadata(&to).is_ok() {
                return Err(ApiError {
                    status: StatusCode::CONFLICT,
                    message: format!("{} already exists", to.display()),
                });
            }
            std::fs::rename(&path, &to)?;
            tracing::info!(
                "file op: rename {} -> {} actor={actor}",
                path.display(),
                to.display()
            );
        }
        "chmod" => {
            let mode = request
                .mode
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("chmod needs a mode"))?;
            let mode = u32::from_str_radix(mode.trim_start_matches('0').max("0"), 8)
                .map_err(|_| ApiError::bad_request("mode must be an octal string such as 640"))?;
            if mode > 0o7777 {
                return Err(ApiError::bad_request("mode is out of range"));
            }
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
            tracing::info!("file op: chmod {:o} {} actor={actor}", mode, path.display());
        }
        other => return Err(ApiError::bad_request(format!("unknown operation: {other}"))),
    }

    Ok(serde_json::json!({ "ok": true }))
}

/// `DELETE /api/fs?path=/target` — removes a file or a whole directory.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    user: axum::extract::Extension<AuthUser>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Response {
    match delete_inner(&state, &user.0.0, &query, &headers).await {
        Ok(value) => json_response(value),
        Err(err) => err.into_response(),
    }
}

async fn delete_inner(
    state: &Arc<AppState>,
    actor: &Option<String>,
    query: &PathQuery,
    headers: &HeaderMap,
) -> Result<serde_json::Value, ApiError> {
    require_writable(state)?;
    require_csrf(state, headers)?;

    let path = parse_path(query)?;
    fs::enforce_roots(roots(state), &path)?;
    if path == Path::new("/") {
        return Err(ApiError::forbidden(
            "refusing to delete the filesystem root",
        ));
    }
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(&path)?;
    } else {
        std::fs::remove_file(&path)?;
    }
    tracing::info!(
        "file op: delete {} actor={}",
        path.display(),
        actor.as_deref().unwrap_or("-")
    );
    Ok(serde_json::json!({ "ok": true }))
}

struct ContentRange {
    start: u64,
    total: u64,
}

/// Parses `bytes 0-99/1000` and `bytes */1000` (the latter reports the total only).
fn parse_content_range(value: &str) -> Result<ContentRange, ApiError> {
    let invalid = || ApiError::bad_request("invalid Content-Range header");
    let rest = value.trim().strip_prefix("bytes ").ok_or_else(invalid)?;
    let (range, total) = rest.split_once('/').ok_or_else(invalid)?;
    let total: u64 = total.trim().parse().map_err(|_| invalid())?;
    if range.trim() == "*" {
        return Ok(ContentRange {
            start: total,
            total,
        });
    }
    let (start, _end) = range.trim().split_once('-').ok_or_else(invalid)?;
    let start: u64 = start.trim().parse().map_err(|_| invalid())?;
    if start > total {
        return Err(invalid());
    }
    Ok(ContentRange { start, total })
}

fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.fttyd-part"))
}

/// Rejects absolute paths and `..` escapes in a client supplied file name.
fn safe_relative_path(name: &str) -> Result<PathBuf, ApiError> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => {
                return Err(ApiError::bad_request(
                    "upload name escapes the target directory",
                ));
            }
            part if part.contains('\0') => {
                return Err(ApiError::bad_request("upload name contains a NUL byte"));
            }
            part => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(ApiError::bad_request("missing file name"));
    }
    Ok(out)
}

/// Fails with 507 when the target filesystem cannot hold the upload, which turns the
/// most common real-world upload failure into a clear message instead of a truncated
/// file.
fn ensure_free_space(directory: &Path, needed: u64) -> Result<(), ApiError> {
    match free_space(directory) {
        Some(free) if free < needed => Err(ApiError {
            status: StatusCode::INSUFFICIENT_STORAGE,
            message: format!("not enough free space: {needed} bytes needed, {free} available"),
        }),
        _ => Ok(()),
    }
}

/// Free bytes for the unprivileged caller, used for a pre-upload space check.
fn free_space(directory: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs writes into the provided struct and only reads the path.
    unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        Some(stats.f_bavail as u64 * stats.f_frsize as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_forms_are_supported() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=10-", 100), Some((10, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=90-200", 100), Some((90, 99)));
    }

    #[test]
    fn invalid_ranges_fall_back_to_the_whole_file() {
        assert_eq!(parse_range("items=0-9", 100), None);
        assert_eq!(parse_range("bytes=0-9,20-29", 100), None);
        assert_eq!(parse_range("bytes=200-300", 100), None);
        assert_eq!(parse_range("bytes=-0", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn csrf_token_must_match() {
        let mut cfg = crate::cli::Config {
            command: vec!["bash".into()],
            ..crate::cli::Config::default()
        };
        cfg.credential = Some("secret".into());
        let state = AppState::new(cfg, crate::stats::Stats::new());
        let mut headers = HeaderMap::new();
        assert!(require_csrf(&state, &headers).is_err());
        headers.insert(CSRF_HEADER, HeaderValue::from_static("wrong"));
        assert!(require_csrf(&state, &headers).is_err());
        headers.insert(CSRF_HEADER, HeaderValue::from_static("secret"));
        assert!(require_csrf(&state, &headers).is_ok());
    }

    #[test]
    fn csrf_without_credentials_still_requires_the_header() {
        let state = AppState::new(
            crate::cli::Config {
                command: vec!["bash".into()],
                ..crate::cli::Config::default()
            },
            crate::stats::Stats::new(),
        );
        let mut headers = HeaderMap::new();
        assert!(
            require_csrf(&state, &headers).is_err(),
            "header presence is the guard"
        );
        headers.insert(CSRF_HEADER, HeaderValue::from_static(""));
        assert!(require_csrf(&state, &headers).is_ok());
    }

    #[test]
    fn readonly_servers_refuse_mutations() {
        let state = AppState::new(
            crate::cli::Config {
                command: vec!["bash".into()],
                ..crate::cli::Config::default()
            },
            crate::stats::Stats::new(),
        );
        assert!(require_writable(&state).is_err());

        let state = AppState::new(
            crate::cli::Config {
                command: vec!["bash".into()],
                writable: true,
                ..crate::cli::Config::default()
            },
            crate::stats::Stats::new(),
        );
        assert!(require_writable(&state).is_ok());
    }

    #[test]
    fn uploads_are_refused_when_the_disk_cannot_hold_them() {
        let error = ensure_free_space(Path::new("/"), u64::MAX).unwrap_err();
        assert_eq!(error.status, StatusCode::INSUFFICIENT_STORAGE);
        assert!(
            error.message.contains("not enough free space"),
            "{}",
            error.message
        );
        assert!(ensure_free_space(Path::new("/"), 1024).is_ok());
    }

    #[test]
    fn upload_names_cannot_escape_the_target() {
        assert!(safe_relative_path("../escape").is_err());
        assert!(safe_relative_path("a/../../b").is_err());
        assert_eq!(
            safe_relative_path("dir/file.txt").unwrap(),
            PathBuf::from("dir/file.txt")
        );
        assert_eq!(
            safe_relative_path("/abs/file.txt").unwrap(),
            PathBuf::from("abs/file.txt")
        );
        assert!(safe_relative_path("./").is_err());
    }

    #[test]
    fn content_range_forms_are_parsed() {
        let range = parse_content_range("bytes 10-19/20").unwrap();
        assert_eq!((range.start, range.total), (10, 20));
        let empty = parse_content_range("bytes */0").unwrap();
        assert_eq!((empty.start, empty.total), (0, 0));
        assert!(parse_content_range("bytes 10-19").is_err());
        assert!(parse_content_range("items 1-2/3").is_err());
        assert!(parse_content_range("bytes 30-39/20").is_err());
    }

    #[test]
    fn paths_must_be_absolute_and_clean() {
        assert!(
            parse_path(&PathQuery {
                path: Some("relative".into())
            })
            .is_err()
        );
        assert!(parse_path(&PathQuery { path: None }).is_err());
        assert_eq!(
            parse_path(&PathQuery {
                path: Some("/tmp/./a/../b".into())
            })
            .unwrap(),
            PathBuf::from("/tmp/b")
        );
    }
}
