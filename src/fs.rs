//! Filesystem access for the sidebar: listings, path validation and confinement.
//!
//! Paths arrive from the browser as raw bytes so that any name the OS allows round
//! trips unchanged; a listing only exposes a `path` for names that are valid UTF-8,
//! because everything the UI can *act* on has to survive a JSON hop.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: message.into() }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: message.into() }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        let status = match err.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            std::io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self { status, message: err.to_string() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message }).to_string();
        (
            self.status,
            [(axum::http::header::CONTENT_TYPE, "application/json;charset=utf-8")],
            body,
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    /// Absolute path the API accepts back, or `None` for names that are not UTF-8.
    pub path: Option<String>,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub mode: Option<String>,
    pub mode_octal: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub symlink_target: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub name: String,
    pub entries: Vec<Entry>,
}

/// Removes `.` and `..` components lexically. The result is still subject to a
/// confinement check, so this only normalises what the client sees.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                // Never climb above the root; `/..` is `/`.
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

/// Rejects a path outside every configured root. With no roots the whole filesystem
/// is reachable, which is the same power the adjacent terminal already has.
pub fn enforce_roots(roots: &[PathBuf], path: &Path) -> Result<(), ApiError> {
    if roots.is_empty() {
        return Ok(());
    }
    let candidate = canonical_or_parent(path)?;
    let allowed = roots.iter().any(|root| {
        canonical_or_parent(root)
            .map(|root| candidate == root || candidate.starts_with(&root))
            .unwrap_or(false)
    });
    if allowed {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!("{} is outside the allowed roots", path.display())))
    }
}

/// Canonicalises the deepest existing ancestor, so a not-yet-created upload target is
/// still checked against the real location of its parent.
fn canonical_or_parent(path: &Path) -> Result<PathBuf, ApiError> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(_) => match path.parent() {
            Some(parent) => {
                let parent = std::fs::canonicalize(parent)?;
                let name = path.file_name().unwrap_or_default();
                Ok(parent.join(name))
            }
            None => Err(ApiError::bad_request("path has no parent")),
        },
    }
}

/// Lists a directory. Entries that cannot be stat'ed are reported with an error
/// instead of failing the whole listing.
pub fn list(roots: &[PathBuf], dir: &Path) -> Result<Listing, ApiError> {
    enforce_roots(roots, dir)?;
    let metadata = std::fs::metadata(dir)?;
    if !metadata.is_dir() {
        return Err(ApiError::bad_request(format!("{} is not a directory", dir.display())));
    }

    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = match item {
            Ok(item) => item,
            Err(err) => {
                // A single unreadable entry (raced deletion, permissions) must not
                // abort the listing.
                entries.push(Entry {
                    name: "<unreadable entry>".to_string(),
                    path: None,
                    kind: EntryKind::Other,
                    size: None,
                    mtime: None,
                    mode: None,
                    mode_octal: None,
                    uid: None,
                    gid: None,
                    symlink_target: None,
                    error: Some(err.to_string()),
                });
                continue;
            }
        };
        entries.push(describe(&item));
    }

    entries.sort_by(|left, right| {
        let left_dir = left.kind == EntryKind::Dir;
        let right_dir = right.kind == EntryKind::Dir;
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(Listing {
        path: dir.to_string_lossy().into_owned(),
        parent: dir.parent().filter(|parent| *parent != dir).map(|parent| parent.to_string_lossy().into_owned()),
        name: dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string()),
        entries,
    })
}

fn describe(item: &std::fs::DirEntry) -> Entry {
    let file_name = item.file_name();
    let name = file_name.to_string_lossy().into_owned();
    let addressable = file_name.to_str().is_some();
    let path = addressable
        .then(|| item.path().to_string_lossy().into_owned());

    let metadata = match std::fs::symlink_metadata(item.path()) {
        Ok(metadata) => metadata,
        Err(err) => {
            return Entry {
                name,
                path,
                kind: EntryKind::Other,
                size: None,
                mtime: None,
                mode: None,
                mode_octal: None,
                uid: None,
                gid: None,
                symlink_target: None,
                error: Some(err.to_string()),
            };
        }
    };

    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };

    let symlink_target = file_type
        .is_symlink()
        .then(|| std::fs::read_link(item.path()).ok())
        .flatten()
        .map(|target| target.to_string_lossy().into_owned());

    let is_dir = file_type.is_dir();
    Entry {
        name,
        path,
        kind,
        // Directory sizes would require a recursive walk, so they stay unknown.
        size: (!is_dir).then_some(metadata.len()),
        mtime: metadata.modified().ok().and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
        }),
        mode: Some(mode_string(metadata.mode(), &file_type)),
        mode_octal: Some(metadata.mode() & 0o7777),
        uid: Some(metadata.uid()),
        gid: Some(metadata.gid()),
        symlink_target,
        error: None,
    }
}

/// `-rwxr-xr-x` style mode, which is what `ls -l` shows.
pub fn mode_string(mode: u32, file_type: &std::fs::FileType) -> String {
    let type_char = if file_type.is_dir() {
        'd'
    } else if file_type.is_symlink() {
        'l'
    } else if file_type.is_fifo() {
        'p'
    } else if file_type.is_socket() {
        's'
    } else if file_type.is_block_device() {
        'b'
    } else if file_type.is_char_device() {
        'c'
    } else {
        '-'
    };
    let mut out = String::with_capacity(10);
    out.push(type_char);
    let bits = [(0o400, 'r'), (0o200, 'w'), (0o100, 'x'), (0o040, 'r'), (0o020, 'w'), (0o010, 'x'), (0o004, 'r'), (0o002, 'w'), (0o001, 'x')];
    for (flag, character) in bits {
        out.push(if mode & flag != 0 { character } else { '-' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fttyd-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normalisation_removes_dot_and_dotdot() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("/a/../..")), PathBuf::from("/"));
    }

    #[test]
    fn mode_strings_look_like_ls() {
        let dir = temp_dir("mode");
        let file = dir.join("f");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let meta = std::fs::symlink_metadata(&file).unwrap();
        assert_eq!(mode_string(meta.mode(), &meta.file_type()), "-rw-r-----");
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        assert_eq!(mode_string(meta.mode(), &meta.file_type()), "drwxr-xr-x");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_sorts_directories_first_then_names() {
        let dir = temp_dir("list");
        std::fs::write(dir.join("b.txt"), b"1").unwrap();
        std::fs::write(dir.join("A.txt"), b"1").unwrap();
        std::fs::create_dir(dir.join("z-dir")).unwrap();
        std::fs::create_dir(dir.join("a-dir")).unwrap();

        let listing = list(&[], &dir).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["a-dir", "z-dir", "A.txt", "b.txt"]);
        assert_eq!(listing.entries[0].kind, EntryKind::Dir);
        assert_eq!(listing.entries[2].size, Some(1));
        assert_eq!(listing.entries[0].size, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_reports_symlinks_without_following_them() {
        let dir = temp_dir("link");
        std::fs::write(dir.join("target.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).unwrap();

        let listing = list(&[], &dir).unwrap();
        let link = listing.entries.iter().find(|entry| entry.name == "link.txt").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.symlink_target.as_deref(), Some("target.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_a_file_is_rejected() {
        let dir = temp_dir("notdir");
        let file = dir.join("f");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(list(&[], &file).unwrap_err().status, StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_directory_is_not_found() {
        assert_eq!(
            list(&[], Path::new("/nonexistent-fttyd-dir")).unwrap_err().status,
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn roots_confine_listings() {
        let root = temp_dir("root");
        let outside = temp_dir("outside");
        std::fs::create_dir(root.join("inside")).unwrap();

        assert!(enforce_roots(std::slice::from_ref(&root), &root.join("inside")).is_ok());
        assert_eq!(
            enforce_roots(std::slice::from_ref(&root), &outside).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            enforce_roots(std::slice::from_ref(&root), &root.join("../outside")).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn roots_reject_symlink_escapes() {
        let root = temp_dir("escape-root");
        let outside = temp_dir("escape-outside");
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert_eq!(
            enforce_roots(std::slice::from_ref(&root), &root.join("link")).unwrap_err().status,
            StatusCode::FORBIDDEN
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn no_roots_allows_everything() {
        assert!(enforce_roots(&[], Path::new("/etc")).is_ok());
    }
}
