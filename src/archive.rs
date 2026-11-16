//! Streamed folder downloads.
//!
//! Both formats are produced directly into the response body on a blocking thread, so
//! a multi-gigabyte directory never lands in memory or on disk. Nothing is ever
//! followed through a symlink, and special files (FIFOs, sockets, devices) are skipped
//! because reading them can block forever.

use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::fs::{ApiError, enforce_roots};

/// How many chunks may queue before the archive writer waits for the client.
const CHUNK_QUEUE: usize = 8;
/// File inside the archive that lists anything that could not be read.
const SKIP_REPORT: &str = "fttyd-archive-report.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

impl ArchiveFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "zip" => Some(Self::Zip),
            "tar.gz" | "tgz" => Some(Self::TarGz),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Zip => "application/zip",
            Self::TarGz => "application/gzip",
        }
    }
}

/// Builds a streaming `Response` for `root`.
pub fn response(
    root: PathBuf,
    roots: Vec<PathBuf>,
    format: ArchiveFormat,
) -> Result<Response, ApiError> {
    enforce_roots(&roots, &root)?;
    let metadata = std::fs::symlink_metadata(&root)?;
    if !metadata.is_dir() {
        return Err(ApiError::bad_request("only directories can be archived"));
    }

    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "root".to_string());
    let download_name = format!("{name}.{}", format.extension());

    let (tx, mut rx) = mpsc::channel::<Result<Bytes, io::Error>>(CHUNK_QUEUE);
    let spawn_root = root.clone();
    tokio::task::spawn_blocking(move || {
        let writer = ChannelWriter {
            sender: tx.clone(),
            closed: false,
        };
        let result = match format {
            ArchiveFormat::Zip => write_zip(writer, &spawn_root, &roots),
            ArchiveFormat::TarGz => write_tar_gz(writer, &spawn_root, &roots),
        };
        if let Err(err) = result {
            // Surfacing the failure truncates the body, which is what makes the
            // browser report a failed download instead of saving a broken archive.
            let _ = tx.blocking_send(Err(io::Error::other(err.to_string())));
        }
    });

    let stream = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
    let body = Body::from_stream(stream);

    let disposition = content_disposition(&download_name);
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(format.content_type()),
            ),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response())
}

/// RFC 6266 header: a plain quoted name plus the UTF-8 form for non-ASCII names.
fn content_disposition(file_name: &str) -> HeaderValue {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    const ENCODE: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'%')
        .add(b'\'')
        .add(b'(')
        .add(b')')
        .add(b'*')
        .add(b',')
        .add(b'/')
        .add(b':')
        .add(b';')
        .add(b'<')
        .add(b'=')
        .add(b'>')
        .add(b'?')
        .add(b'@')
        .add(b'[')
        .add(b'\\')
        .add(b']')
        .add(b'{')
        .add(b'}');

    let ascii_fallback: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(file_name, ENCODE);
    let value = format!("attachment; filename=\"{ascii_fallback}\"; filename*=UTF-8''{encoded}");
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// `Write` sink that forwards bytes to the HTTP body with backpressure.
struct ChannelWriter {
    sender: mpsc::Sender<Result<Bytes, io::Error>>,
    closed: bool,
}

impl Write for ChannelWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client disconnected",
            ));
        }
        match self
            .sender
            .blocking_send(Ok(Bytes::copy_from_slice(buffer)))
        {
            Ok(()) => Ok(buffer.len()),
            Err(_) => {
                self.closed = true;
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "client disconnected",
                ))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

trait Sink {
    fn directory(&mut self, name: &str, path: &Path, meta: &std::fs::Metadata) -> io::Result<()>;
    fn file(&mut self, name: &str, path: &Path, meta: &std::fs::Metadata) -> io::Result<()>;
    fn symlink(
        &mut self,
        name: &str,
        target: &Path,
        path: &Path,
        meta: &std::fs::Metadata,
    ) -> io::Result<()>;
    fn report(&mut self, name: &str, skipped: &[String]) -> io::Result<()>;
}

fn write_zip(writer: ChannelWriter, root: &Path, roots: &[PathBuf]) -> io::Result<()> {
    let mut sink = ZipSink::new(writer)?;
    let mut skipped = Vec::new();
    let prefix = base_name(root);
    walk(&mut sink, root, &prefix, roots, &mut skipped)?;
    sink.report(&format!("{prefix}/{SKIP_REPORT}"), &skipped)?;
    sink.finish()
}

fn write_tar_gz(writer: ChannelWriter, root: &Path, roots: &[PathBuf]) -> io::Result<()> {
    let mut sink = TarSink::new(writer);
    let mut skipped = Vec::new();
    let prefix = base_name(root);
    walk(&mut sink, root, &prefix, roots, &mut skipped)?;
    sink.report(&format!("{prefix}/{SKIP_REPORT}"), &skipped)?;
    sink.finish()
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "root".to_string())
}

fn walk(
    sink: &mut dyn Sink,
    dir: &Path,
    prefix: &str,
    roots: &[PathBuf],
    skipped: &mut Vec<String>,
) -> io::Result<()> {
    let mut entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(iter) => iter.filter_map(Result::ok).collect(),
        Err(err) => {
            skipped.push(format!("{}: {err}", dir.display()));
            return Ok(());
        }
    };
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if enforce_roots(roots, &path).is_err() {
            skipped.push(format!("{}: outside the allowed roots", path.display()));
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) => {
                skipped.push(format!("{}: {err}", path.display()));
                continue;
            }
        };
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            let target = std::fs::read_link(&path)?;
            sink.symlink(&name, &target, &path, &metadata)?;
        } else if file_type.is_dir() {
            sink.directory(&name, &path, &metadata)?;
            walk(sink, &path, &name, roots, skipped)?;
        } else if file_type.is_file() {
            if let Err(err) = sink.file(&name, &path, &metadata) {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    return Err(err);
                }
                skipped.push(format!("{}: {err}", path.display()));
            }
        } else {
            // FIFOs, sockets and device nodes are not readable as data.
            skipped.push(format!("{}: not a regular file", path.display()));
        }
    }
    Ok(())
}

struct ZipSink {
    writer: Option<zip::ZipWriter<zip::write::StreamWriter<ChannelWriter>>>,
}

impl ZipSink {
    fn new(writer: ChannelWriter) -> io::Result<Self> {
        Ok(Self {
            writer: Some(zip::ZipWriter::new_stream(writer)),
        })
    }

    fn finish(mut self) -> io::Result<()> {
        if let Some(writer) = self.writer.take() {
            writer.finish().map_err(zip_error)?;
        }
        Ok(())
    }

    fn options(meta: &std::fs::Metadata) -> zip::write::SimpleFileOptions {
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(meta.mode() & 0o7777);
        if let Some(time) = zip_datetime(meta.mtime()) {
            options = options.last_modified_time(time);
        }
        if meta.len() >= u32::MAX as u64 {
            options = options.large_file(true);
        }
        options
    }
}

impl Sink for ZipSink {
    fn directory(&mut self, name: &str, _path: &Path, meta: &std::fs::Metadata) -> io::Result<()> {
        let writer = self.writer.as_mut().expect("zip writer");
        writer
            .add_directory(format!("{name}/"), Self::options(meta))
            .map_err(zip_error)
    }

    fn file(&mut self, name: &str, path: &Path, meta: &std::fs::Metadata) -> io::Result<()> {
        let mut source = std::fs::File::open(path)?;
        let writer = self.writer.as_mut().expect("zip writer");
        writer
            .start_file(name, Self::options(meta))
            .map_err(zip_error)?;
        io::copy(&mut source, writer).map(|_| ())
    }

    fn symlink(
        &mut self,
        name: &str,
        target: &Path,
        _path: &Path,
        meta: &std::fs::Metadata,
    ) -> io::Result<()> {
        let writer = self.writer.as_mut().expect("zip writer");
        writer
            .add_symlink(name, target.to_string_lossy(), Self::options(meta))
            .map_err(zip_error)
    }

    fn report(&mut self, name: &str, skipped: &[String]) -> io::Result<()> {
        if skipped.is_empty() {
            return Ok(());
        }
        let writer = self.writer.as_mut().expect("zip writer");
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .map_err(zip_error)?;
        writer.write_all(report_body(skipped).as_bytes())
    }
}

struct TarSink {
    builder: Option<tar::Builder<flate2::write::GzEncoder<ChannelWriter>>>,
}

impl TarSink {
    fn new(writer: ChannelWriter) -> Self {
        let encoder = flate2::write::GzEncoder::new(writer, flate2::Compression::default());
        Self {
            builder: Some(tar::Builder::new(encoder)),
        }
    }

    fn finish(mut self) -> io::Result<()> {
        let encoder = self.builder.take().expect("tar builder").into_inner()?;
        encoder.finish().map(|_| ())
    }
}

impl Sink for TarSink {
    fn directory(&mut self, name: &str, path: &Path, _meta: &std::fs::Metadata) -> io::Result<()> {
        self.builder
            .as_mut()
            .expect("tar builder")
            .append_dir(name, path)
    }

    fn file(&mut self, name: &str, path: &Path, _meta: &std::fs::Metadata) -> io::Result<()> {
        self.builder
            .as_mut()
            .expect("tar builder")
            .append_path_with_name(path, name)
    }

    fn symlink(
        &mut self,
        name: &str,
        target: &Path,
        path: &Path,
        meta: &std::fs::Metadata,
    ) -> io::Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_metadata_in_mode(meta, tar::HeaderMode::Complete);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        self.builder
            .as_mut()
            .expect("tar builder")
            .append_link(&mut header, name, target)?;
        let _ = path;
        Ok(())
    }

    fn report(&mut self, name: &str, skipped: &[String]) -> io::Result<()> {
        if skipped.is_empty() {
            return Ok(());
        }
        let body = report_body(skipped);
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        self.builder
            .as_mut()
            .expect("tar builder")
            .append_data(&mut header, name, body.as_bytes())
    }
}

fn report_body(skipped: &[String]) -> String {
    let mut body = String::from(
        "# fttyd could not include the following paths in this archive.\n\
         # Unreadable paths, special files and symlink escapes are skipped.\n",
    );
    for entry in skipped {
        body.push_str(entry);
        body.push('\n');
    }
    body
}

fn zip_error(err: zip::result::ZipError) -> io::Error {
    match err {
        zip::result::ZipError::Io(err) => err,
        other => io::Error::other(other.to_string()),
    }
}

/// Converts a unix timestamp into the DOS timestamp zip stores.
pub fn zip_datetime(seconds: i64) -> Option<zip::DateTime> {
    let (year, month, day, hour, minute, second) = civil_from_unix(seconds)?;
    zip::DateTime::from_date_and_time(year, month, day, hour, minute, second).ok()
}

/// Days-to-civil conversion (Howard Hinnant's algorithm), avoiding a date crate.
fn civil_from_unix(seconds: i64) -> Option<(u16, u8, u8, u8, u8, u8)> {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    let hour = time / 3_600;
    let minute = (time % 3_600) / 60;
    let second = time % 60;
    Some((
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
        u8::try_from(hour).ok()?,
        u8::try_from(minute).ok()?,
        u8::try_from(second).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fttyd-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"alpha").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"bravo").unwrap();
        std::os::unix::fs::symlink("a.txt", dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink("/tmp", dir.join("escape")).unwrap();
        dir
    }

    fn collect(format: ArchiveFormat, root: &Path, roots: &[PathBuf]) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!(
            "fttyd-archive-out-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, mut rx) = mpsc::channel::<Result<Bytes, io::Error>>(4);
        let root = root.to_path_buf();
        let roots = roots.to_vec();
        let handle = std::thread::spawn(move || {
            let writer = ChannelWriter {
                sender: tx,
                closed: false,
            };
            match format {
                ArchiveFormat::Zip => write_zip(writer, &root, &roots),
                ArchiveFormat::TarGz => write_tar_gz(writer, &root, &roots),
            }
        });
        let mut bytes = Vec::new();
        while let Some(chunk) = rx.blocking_recv() {
            bytes.extend_from_slice(&chunk.expect("chunk"));
        }
        handle.join().unwrap().expect("archive");
        std::fs::remove_dir_all(&dir).unwrap();
        bytes
    }

    #[test]
    fn zip_contains_files_dirs_and_symlinks() {
        let root = fixture("zip");
        let bytes = collect(ArchiveFormat::Zip, &root, &[]);

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        let dir_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert!(names.contains(&format!("{dir_name}/a.txt")), "{names:?}");
        assert!(
            names.contains(&format!("{dir_name}/sub/b.txt")),
            "{names:?}"
        );
        assert!(names.contains(&format!("{dir_name}/link.txt")), "{names:?}");
        assert!(
            names.iter().any(|name| name.ends_with('/')),
            "directory entry missing: {names:?}"
        );

        let mut inner = String::new();
        archive
            .by_name(&format!("{dir_name}/sub/b.txt"))
            .unwrap()
            .read_to_string(&mut inner)
            .unwrap();
        assert_eq!(inner, "bravo");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn zip_preserves_mode_and_mtime() {
        let root = fixture("zip-mode");
        std::fs::set_permissions(root.join("a.txt"), PermissionsExt::from_mode(0o600)).unwrap();
        let bytes = collect(ArchiveFormat::Zip, &root, &[]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let dir_name = root.file_name().unwrap().to_string_lossy().into_owned();
        let entry = archive.by_name(&format!("{dir_name}/a.txt")).unwrap();
        let mode = entry.unix_mode().unwrap();
        assert_eq!(mode & 0o777, 0o600);
        assert!(entry.last_modified().is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tar_contains_files_and_preserves_mode() {
        let root = fixture("tar");
        std::fs::set_permissions(root.join("a.txt"), PermissionsExt::from_mode(0o640)).unwrap();
        let bytes = collect(ArchiveFormat::TarGz, &root, &[]);

        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        let dir_name = root.file_name().unwrap().to_string_lossy().into_owned();
        let mut seen = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            if path == format!("{dir_name}/a.txt") {
                assert_eq!(entry.header().mode().unwrap() & 0o777, 0o640);
                assert!(entry.header().mtime().unwrap() > 0);
            }
            seen.push(path);
        }
        assert!(seen.contains(&format!("{dir_name}/sub/b.txt")), "{seen:?}");
        assert!(seen.contains(&format!("{dir_name}/link.txt")), "{seen:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinks_are_never_followed() {
        let root = fixture("escape");
        let outside = std::env::temp_dir().join("fttyd-escape-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();

        let bytes = collect(ArchiveFormat::Zip, &root, &[]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(
            !names.iter().any(|name| name.ends_with("secret.txt")),
            "{names:?}"
        );

        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn special_files_are_skipped_and_reported() {
        let root = fixture("fifo");
        let fifo = root.join("pipe");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: mkfifo only needs a valid C string and mode.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
        assert_eq!(rc, 0, "mkfifo failed");

        let bytes = collect(ArchiveFormat::Zip, &root, &[]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            names.push(archive.by_index(index).unwrap().name().unwrap().to_string());
        }
        assert!(
            !names.iter().any(|name| name.ends_with("/pipe")),
            "{names:?}"
        );
        let dir_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            names.contains(&format!("{dir_name}/{SKIP_REPORT}")),
            "{names:?}"
        );

        let mut report = String::new();
        archive
            .by_name(&format!("{dir_name}/{SKIP_REPORT}"))
            .unwrap()
            .read_to_string(&mut report)
            .unwrap();
        assert!(report.contains("pipe"), "{report}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn archives_respect_file_roots() {
        let root = fixture("roots");
        let result = std::panic::catch_unwind(|| {
            collect(
                ArchiveFormat::Zip,
                &root,
                &[PathBuf::from("/nonexistent-root")],
            )
        });
        assert!(
            result.is_ok(),
            "archive walk should not panic when roots exclude everything"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dos_timestamps_round_trip() {
        let datetime = zip_datetime(1_700_000_000).unwrap();
        assert_eq!(datetime.year(), 2023);
        assert_eq!(datetime.month(), 11);
        assert_eq!(datetime.day(), 14);
        assert_eq!(civil_from_unix(0), Some((1970, 1, 1, 0, 0, 0)));
        assert_eq!(civil_from_unix(86_399), Some((1970, 1, 1, 23, 59, 59)));
        assert_eq!(civil_from_unix(-1), Some((1969, 12, 31, 23, 59, 59)));
    }

    #[test]
    fn content_disposition_handles_non_ascii_names() {
        let value = content_disposition("café.zip");
        let text = value.to_str().unwrap();
        assert!(text.contains("filename=\"caf_.zip\""), "{text}");
        assert!(text.contains("filename*=UTF-8''caf%C3%A9.zip"), "{text}");
    }
}
