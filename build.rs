//! Builds (or reuses) the frontend bundle and stages it in `OUT_DIR` for `rust-embed`.
//!
//! Nothing here writes into the source tree: `cargo publish` refuses to verify a package
//! whose build script modified it, and `cargo install` should not need a Node toolchain.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

fn main() {
    for path in [
        "web/src",
        "web/public",
        "web/index.html",
        "web/package.json",
        "web/vite.config.ts",
        "web/tsconfig.json",
        "web/bun.lock",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set by cargo"));
    let dist = Path::new("web/dist");

    if needs_build(&dist.join("index.html")) {
        build_frontend();
    }
    // Staging is a copy into OUT_DIR only, which is also what keeps `cargo publish`
    // happy: the verification build must not touch the package it is verifying.
    stage(dist, &out.join("web"));
}

/// Rebuilds when the bundle is missing or older than any frontend source.
///
/// A published crate ships the bundle and has no `node_modules`, so there the shipped
/// files are used as they are: `cargo install` must not need a Node toolchain.
fn needs_build(dist_index: &Path) -> bool {
    if !Path::new("web/node_modules").exists() {
        return !dist_index.is_file();
    }
    let Ok(dist_time) = dist_index.metadata().and_then(|meta| meta.modified()) else {
        return true;
    };
    newest_mtime(Path::new("web/src"))
        .into_iter()
        .chain(newest_mtime(Path::new("web/public")))
        .chain(source_mtime("web/index.html"))
        .chain(source_mtime("web/package.json"))
        .chain(source_mtime("web/vite.config.ts"))
        .any(|time| time > dist_time)
}

fn build_frontend() {
    run("bun", &["install", "--frozen-lockfile"], "web").unwrap_or_else(|err| {
        panic!(
            "failed to install frontend dependencies: {err}\n\
             Install them (cd web && bun install) or make sure `bun` is on PATH."
        )
    });
    run("bun", &["run", "build"], "web").unwrap_or_else(|err| {
        panic!(
            "failed to build the frontend: {err}\n\
             Install dependencies (cd web && bun install) or make sure `bun` is on PATH."
        )
    });
}

fn run(program: &str, args: &[&str], dir: &str) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Copies the finished bundle into `OUT_DIR`, which is where `rust-embed` reads it from.
fn stage(from: &Path, to: &Path) {
    if to.exists() {
        std::fs::remove_dir_all(to).unwrap_or_else(|err| panic!("failed to clear {to:?}: {err}"));
    }
    copy_tree(from, to);
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|err| panic!("failed to create {to:?}: {err}"));
    let entries =
        std::fs::read_dir(from).unwrap_or_else(|err| panic!("failed to read {from:?}: {err}"));
    for entry in entries.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target)
                .unwrap_or_else(|err| panic!("failed to copy {path:?}: {err}"));
        }
    }
}

fn source_mtime(path: &str) -> Option<SystemTime> {
    Path::new(path)
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let candidate = if path.is_dir() {
            newest_mtime(&path)
        } else {
            path.metadata().ok().and_then(|meta| meta.modified().ok())
        };
        if let Some(time) = candidate {
            newest = Some(match newest {
                Some(current) if current >= time => current,
                _ => time,
            });
        }
    }
    newest
}
