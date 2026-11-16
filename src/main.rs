mod api;
mod archive;
mod assets;
mod auth;
mod cli;
mod fs;
mod proto;
mod pty;
mod server;
mod stats;
mod state;
mod ws;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use cli::{Config, Invocation};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let prog = argv
        .first()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fttyd".to_string());
    let args = if argv.is_empty() { &argv[..] } else { &argv[1..] };

    match cli::parse(&prog, args) {
        Ok(Invocation::Help) => {
            eprint!("{}", cli::help_text(&prog));
            ExitCode::SUCCESS
        }
        Ok(Invocation::Version) => {
            println!("{prog} version {}", cli::VERSION);
            ExitCode::SUCCESS
        }
        Ok(Invocation::Run(cfg)) => run(&prog, *cfg),
        Err(err) => {
            eprintln!("{}", err.message);
            ExitCode::from(err.exit_code as u8)
        }
    }
}

fn run(prog: &str, cfg: Config) -> ExitCode {
    init_logging(prog, cfg.debug);
    if let Err(err) = validate(&cfg) {
        eprintln!("{prog}: {err}");
        return ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{prog}: failed to start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{prog}: {err}");
            ExitCode::FAILURE
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn serve(cfg: Config) -> Result<(), BoxError> {
    let state = state::AppState::new(cfg, stats::Stats::new().spawn(std::time::Duration::from_secs(1)));
    log_config(&state.cfg);

    let (listener, port) = bind(&state).await?;
    drop_privileges(&state.cfg)?;

    if state.cfg.browser {
        open_browser(port, &state.cfg.index_path());
    }

    let app = server::router(state.clone());
    let shutdown = shutdown_signal(state.clone());
    match listener {
        Listener::Tcp(listener) => {
            axum::serve(listener, app).with_graceful_shutdown(shutdown).await?
        }
        Listener::Unix(listener) => {
            axum::serve(listener, app).with_graceful_shutdown(shutdown).await?
        }
    }
    Ok(())
}

enum Listener {
    Tcp(tokio::net::TcpListener),
    Unix(tokio::net::UnixListener),
}

async fn bind(state: &state::AppState) -> Result<(Listener, Option<u16>), BoxError> {
    let cfg = &state.cfg;
    if let Some(path) = cfg.unix_socket_path() {
        prepare_socket_path(path)?;
        let listener = tokio::net::UnixListener::bind(path)?;
        if let Some(owner) = &cfg.socket_owner {
            chown_socket(path, owner)?;
        }
        tracing::info!("listening on unix socket {path}");
        return Ok((Listener::Unix(listener), None));
    }

    let addr = bind_address(cfg)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();
    tracing::info!("listening on port {port}");
    Ok((Listener::Tcp(listener), Some(port)))
}

fn bind_address(cfg: &Config) -> Result<SocketAddr, BoxError> {
    if let Some(interface) = &cfg.interface {
        if let Ok(ip) = interface.parse::<IpAddr>() {
            return Ok(SocketAddr::new(ip, cfg.port));
        }
        let ip = interface_address(interface, cfg.ipv6)?;
        return Ok(SocketAddr::new(ip, cfg.port));
    }
    let ip = if cfg.ipv6 {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    };
    Ok(SocketAddr::new(ip, cfg.port))
}

/// Resolves a network interface name to one of its addresses, which is what binding
/// to an interface means in practice without `SO_BINDTODEVICE` privileges.
fn interface_address(name: &str, prefer_ipv6: bool) -> Result<IpAddr, BoxError> {
    let mut candidates = Vec::new();
    for interface in nix::ifaddrs::getifaddrs()? {
        if interface.interface_name != name {
            continue;
        }
        let Some(address) = interface.address else { continue };
        let Some(address) = address.as_sockaddr_in() else {
            if let Some(address) = address.as_sockaddr_in6() {
                candidates.push(IpAddr::V6(address.ip()));
            }
            continue;
        };
        candidates.push(IpAddr::V4(address.ip()));
    }
    let pick = |want_v6: bool| {
        candidates.iter().copied().find(|ip| ip.is_ipv6() == want_v6)
    };
    pick(prefer_ipv6)
        .or_else(|| pick(!prefer_ipv6))
        .ok_or_else(|| format!("interface {name} has no usable address").into())
}

/// A stale socket file from a previous run would make `bind` fail.
fn prepare_socket_path(path: &str) -> Result<(), BoxError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            std::fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(format!("{path} exists and is not a socket").into()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn chown_socket(path: &str, owner: &str) -> Result<(), BoxError> {
    use nix::unistd::{Group, Uid, User, chown};

    let (user, group) = match owner.split_once(':') {
        Some((user, group)) => (user, Some(group)),
        None => (owner, None),
    };
    let uid = if user.is_empty() {
        None
    } else if let Ok(raw) = user.parse::<u32>() {
        Some(Uid::from_raw(raw))
    } else {
        match User::from_name(user)? {
            Some(found) => Some(found.uid),
            None => return Err(format!("unknown user {user}").into()),
        }
    };
    let gid = match group {
        None => None,
        Some("") => None,
        Some(group) => {
            if let Ok(raw) = group.parse::<u32>() {
                Some(nix::unistd::Gid::from_raw(raw))
            } else {
                match Group::from_name(group)? {
                    Some(found) => Some(found.gid),
                    None => return Err(format!("unknown group {group}").into()),
                }
            }
        }
    };
    chown(path, uid, gid)?;
    Ok(())
}

/// `-u`/`-g` are applied after the listener exists so the socket or port is set up
/// with the privileges of the original user.
fn drop_privileges(cfg: &Config) -> Result<(), BoxError> {
    use nix::unistd::{Gid, Uid, setgid, setgroups, setuid};

    if let Some(gid) = cfg.gid {
        // Supplementary groups would otherwise keep inherited access.
        if let Err(err) = setgroups(&[]) {
            tracing::debug!("setgroups failed: {err}");
        }
        setgid(Gid::from_raw(gid))?;
        tracing::info!("dropped group privileges to gid {gid}");
    }
    if let Some(uid) = cfg.uid {
        setuid(Uid::from_raw(uid))?;
        tracing::info!("dropped user privileges to uid {uid}");
    }
    Ok(())
}

fn open_browser(port: Option<u16>, index_path: &str) {
    let Some(port) = port else { return };
    let url = format!("http://localhost:{port}{index_path}");
    let command = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    match std::process::Command::new(command).arg(&url).spawn() {
        Ok(_) => tracing::info!("opened {url} in the default browser"),
        Err(err) => tracing::warn!("failed to open a browser: {err}"),
    }
}

fn validate(cfg: &Config) -> Result<(), String> {
    if let Some(cwd) = &cfg.cwd {
        if !cwd.is_dir() {
            return Err(format!("working directory does not exist: {}", cwd.display()));
        }
    }
    if cfg.ssl && (cfg.ssl_cert.is_none() || cfg.ssl_key.is_none()) {
        return Err("--ssl requires --ssl-cert and --ssl-key".to_string());
    }
    if cfg.uid.is_some() || cfg.gid.is_some() {
        // nix returns EPERM below, but a clear message beats "operation not permitted".
        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } != 0 {
            return Err("--uid/--gid require starting as root".to_string());
        }
    }
    Ok(())
}

fn log_config(cfg: &Config) {
    tracing::info!("fttyd {}", cli::VERSION);
    tracing::info!("tty configuration:");
    if let Some(credential) = &cfg.credential {
        tracing::info!("  credential: {credential}");
    }
    tracing::info!("  start command: {}", cfg.command.join(" "));
    tracing::info!("  close signal: {} ({})", cli::signal_name(cfg.sig_code), cfg.sig_code);
    tracing::info!("  terminal type: {}", cfg.terminal_type);
    if !cfg.base_path.is_empty() {
        tracing::info!("endpoints:");
        tracing::info!("  base-path: {}", cfg.base_path);
        tracing::info!("  index    : {}", cfg.index_path());
        tracing::info!("  token    : {}", cfg.token_path());
        tracing::info!("  websocket: {}", cfg.ws_path());
    }
    if let Some(header) = &cfg.auth_header {
        tracing::info!("  auth header: {header}");
    }
    if cfg.check_origin {
        tracing::info!("  check origin: true");
    }
    if cfg.url_arg {
        tracing::info!("  allow url arg: true");
    }
    if cfg.max_clients > 0 {
        tracing::info!("  max clients: {}", cfg.max_clients);
    }
    if cfg.once {
        tracing::info!("  once: true");
    }
    if cfg.exit_no_conn {
        tracing::info!("  exit_no_conn: true");
    }
    if let Some(index) = &cfg.index {
        tracing::info!("  custom index.html: {}", index.display());
    }
    if let Some(cwd) = &cfg.cwd {
        tracing::info!("  working directory: {}", cwd.display());
    }
    if !cfg.writable {
        tracing::warn!("The --writable option is not set, will start in readonly mode");
    }
}

fn init_logging(prog: &str, debug: i32) {
    let level = match debug {
        0 => "error",
        1..=3 => "warn",
        4..=14 => "info",
        15..=30 => "debug",
        _ => "trace",
    };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| format!("{prog}={level},fttyd={level}"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .try_init();
}

async fn shutdown_signal(state: Arc<state::AppState>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
        _ = state.shutdown.notified() => tracing::info!("no clients left, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_address_defaults_match_family() {
        let cfg = Config::default();
        assert_eq!(bind_address(&cfg).unwrap(), "0.0.0.0:7681".parse().unwrap());
        let cfg = Config { ipv6: true, ..Config::default() };
        assert_eq!(bind_address(&cfg).unwrap(), "[::]:7681".parse().unwrap());
    }

    #[test]
    fn bind_address_accepts_literal_ips() {
        let cfg = Config { interface: Some("127.0.0.1".into()), port: 9000, ..Config::default() };
        assert_eq!(bind_address(&cfg).unwrap(), "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn loopback_interface_resolves() {
        let cfg = Config { interface: Some("lo".into()), ..Config::default() };
        let addr = bind_address(&cfg).unwrap();
        assert!(addr.ip().is_loopback() || addr.ip().is_unspecified());
    }

    #[test]
    fn missing_cwd_is_rejected() {
        let cfg = Config { cwd: Some("/nonexistent/dir".into()), ..Config::default() };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn ssl_without_certificates_is_rejected() {
        let cfg = Config { ssl: true, ..Config::default() };
        assert!(validate(&cfg).is_err());
    }
}
