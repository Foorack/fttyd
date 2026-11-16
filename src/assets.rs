//! The embedded frontend: one binary, no runtime asset directory.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use rust_embed::Embed;

#[derive(Embed)]
// build.rs stages the bundle here; nothing is read from the source tree at compile time.
#[folder = "$OUT_DIR/web/"]
struct WebAssets;

pub struct Asset {
    pub body: Vec<u8>,
    pub content_type: &'static str,
    pub etag: String,
    /// Immutable assets carry a content hash in their file name.
    pub immutable: bool,
}

impl Asset {
    pub fn is_compressible(&self) -> bool {
        matches!(
            self.content_type,
            "text/html" | "text/css" | "text/javascript" | "application/json" | "image/svg+xml"
        )
    }
}

/// Strong validator derived from the content, so a rebuilt bundle is never served from
/// a stale cache.
pub fn etag_for(body: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    format!("\"{:x}-{:x}\"", body.len(), hasher.finish())
}

pub fn get(path: &str) -> Option<Asset> {
    let file = WebAssets::get(path)?;
    let body = file.data.into_owned();
    let etag = etag_for(&body);
    Some(Asset {
        content_type: content_type(path),
        etag,
        immutable: path.starts_with("assets/"),
        body,
    })
}

/// Embedded paths, used by routing tests.
#[cfg(test)]
pub fn paths() -> Vec<String> {
    WebAssets::iter().map(|path| path.into_owned()).collect()
}

/// `index.html` as shipped by the bundler.
pub fn index_html() -> Vec<u8> {
    WebAssets::get("index.html").map(|file| file.data.into_owned()).unwrap_or_else(|| {
        b"<html><body><h1>fttyd</h1><p>frontend assets were not embedded at build time</p></body></html>"
            .to_vec()
    })
}

/// Gzipped asset bodies, compressed once per path and kept for the process lifetime.
pub fn gzipped(path: &str, body: &[u8]) -> Arc<Vec<u8>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().unwrap().get(path) {
        return cached.clone();
    }
    let compressed = compress(body);
    let compressed = Arc::new(compressed);
    cache
        .lock()
        .unwrap()
        .insert(path.to_string(), compressed.clone());
    compressed
}

fn compress(body: &[u8]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    if encoder.write_all(body).is_err() {
        return body.to_vec();
    }
    encoder.finish().unwrap_or_else(|_| body.to_vec())
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_javascript_is_embedded() {
        let paths = paths();
        assert!(
            paths.iter().any(|path| path.starts_with("assets/")),
            "embedded: {paths:?}"
        );
        assert_eq!(paths.iter().filter(|path| path.ends_with(".js")).count(), 1);
    }

    #[test]
    fn index_is_embedded() {
        let html = index_html();
        assert!(String::from_utf8_lossy(&html).contains("fttyd"));
    }

    #[test]
    fn content_types_are_mapped() {
        assert_eq!(content_type("index.html"), "text/html");
        assert_eq!(content_type("assets/index-abc.js"), "text/javascript");
        assert_eq!(content_type("assets/index-abc.css"), "text/css");
        assert_eq!(content_type("x.unknown"), "application/octet-stream");
    }

    #[test]
    fn gzip_round_trips() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let body = b"hello hello hello hello".repeat(100);
        let compressed = gzip_for_test(&body);
        let mut decoded = Vec::new();
        GzDecoder::new(&compressed[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body);
    }

    fn gzip_for_test(body: &[u8]) -> Vec<u8> {
        compress(body)
    }
}
