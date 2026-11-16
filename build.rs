use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

fn main() {
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    println!("cargo:rerun-if-changed=web/vite.config.ts");
    println!("cargo:rerun-if-changed=web/tsconfig.json");
    println!("cargo:rerun-if-changed=build.rs");

    let dist_index = Path::new("web/dist/index.html");
    if !needs_build(dist_index) {
        return;
    }

    if !Path::new("web/node_modules").exists() {
        if let Err(err) = run("bun", &["install", "--frozen-lockfile"], "web") {
            panic_install(err);
        }
    }
    run("bun", &["run", "build"], "web").unwrap_or_else(|err| {
        panic!(
            "failed to build the frontend: {err}\n\
             Install dependencies (cd web && bun install) or make sure `bun` is on PATH."
        )
    });
}

fn panic_install(err: String) -> ! {
    panic!(
        "failed to install frontend dependencies: {err}\n\
         Install them (cd web && bun install) or make sure `bun` is on PATH."
    )
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

/// Rebuilds when the bundle is missing or older than any frontend source.
fn needs_build(dist_index: &Path) -> bool {
    let Ok(dist_time) = dist_index.metadata().and_then(|meta| meta.modified()) else {
        return true;
    };
    newest_mtime(Path::new("web/src"))
        .into_iter()
        .chain(source_mtime("web/index.html"))
        .chain(source_mtime("web/package.json"))
        .chain(source_mtime("web/vite.config.ts"))
        .any(|time| time > dist_time)
}

fn source_mtime(path: &str) -> Option<SystemTime> {
    Path::new(path).metadata().ok().and_then(|meta| meta.modified().ok())
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
