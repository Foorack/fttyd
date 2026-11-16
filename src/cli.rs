//! ttyd-compatible command line parsing.
//!
//! ttyd parses arguments with glibc `getopt_long`: options may appear before or after
//! the command, long options accept unambiguous prefixes, short options can be bundled
//! or carry attached values (`-p8080`), and `--` terminates option parsing. This module
//! reproduces those rules instead of using a declarative parser so that an existing
//! ttyd invocation keeps its exact meaning.
//!
//! Deliberate divergences from ttyd, all of them chosen because ttyd's behaviour
//! silently does the wrong thing:
//! * unknown options are an error instead of being logged and ignored;
//! * numeric options reject trailing garbage (`-p 8080x`) instead of truncating;
//! * option parsing stops at the first non-option argument. glibc permutes the
//!   command line, so `ttyd bash -l` silently drops `-l` (it is parsed as an option
//!   and discarded) and `ttyd bash -W` turns on writable mode. Here `--` is implied,
//!   which is what `ttyd -- bash -l` does explicitly;
//! * `-t` takes exactly one `key=value` per occurrence, as `--help` documents.

use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_PORT: u16 = 7681;
pub const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";
pub const DEFAULT_PING_INTERVAL: u64 = 5;
pub const DEFAULT_SRV_BUF_SIZE: usize = 4096;
pub const DEFAULT_LOG_LEVEL: i32 = 7;
const MAX_BASE_PATH_LEN: usize = 128;

/// Fully resolved configuration, ready to run a server from.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub interface: Option<String>,
    pub socket_owner: Option<String>,
    /// Base64 of `user:password`, exactly as ttyd stores and echoes it.
    pub credential: Option<String>,
    pub auth_header: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub sig_code: i32,
    pub cwd: Option<PathBuf>,
    pub url_arg: bool,
    pub writable: bool,
    pub client_options: Vec<(String, serde_json::Value)>,
    pub terminal_type: String,
    pub check_origin: bool,
    pub max_clients: usize,
    pub once: bool,
    pub exit_no_conn: bool,
    pub browser: bool,
    pub index: Option<PathBuf>,
    /// Normalised base path: empty, or `/prefix` without a trailing slash.
    pub base_path: String,
    pub ping_interval: u64,
    pub srv_buf_size: usize,
    pub ipv6: bool,
    pub ssl: bool,
    pub ssl_cert: Option<PathBuf>,
    pub ssl_key: Option<PathBuf>,
    pub ssl_ca: Option<PathBuf>,
    pub debug: i32,
    pub command: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            interface: None,
            socket_owner: None,
            credential: None,
            auth_header: None,
            uid: None,
            gid: None,
            sig_code: libc::SIGHUP,
            cwd: None,
            url_arg: false,
            writable: false,
            client_options: Vec::new(),
            terminal_type: DEFAULT_TERMINAL_TYPE.to_string(),
            check_origin: false,
            max_clients: 0,
            once: false,
            exit_no_conn: false,
            browser: false,
            index: None,
            base_path: String::new(),
            ping_interval: DEFAULT_PING_INTERVAL,
            srv_buf_size: DEFAULT_SRV_BUF_SIZE,
            ipv6: false,
            ssl: false,
            ssl_cert: None,
            ssl_key: None,
            ssl_ca: None,
            debug: DEFAULT_LOG_LEVEL,
            command: Vec::new(),
        }
    }
}

impl Config {
    /// Path a client connects to for the terminal websocket.
    pub fn ws_path(&self) -> String {
        format!("{}/ws", self.base_path)
    }

    /// Path of the token endpoint.
    pub fn token_path(&self) -> String {
        format!("{}/token", self.base_path)
    }

    /// Path of the index document. Always has a trailing slash.
    pub fn index_path(&self) -> String {
        format!("{}/", self.base_path)
    }

    /// `-i` pointing at a `*.sock`/`*.socket` path selects a UNIX socket listener.
    pub fn unix_socket_path(&self) -> Option<&str> {
        let iface = self.interface.as_deref()?;
        if iface.ends_with(".sock") || iface.ends_with(".socket") {
            Some(iface)
        } else {
            None
        }
    }

    pub fn client_options_json(&self) -> String {
        let mut map = serde_json::Map::new();
        for (key, value) in &self.client_options {
            map.insert(key.clone(), value.clone());
        }
        serde_json::Value::Object(map).to_string()
    }
}

#[derive(Debug)]
pub enum Invocation {
    Help,
    Version,
    Run(Box<Config>),
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    pub message: String,
    pub exit_code: i32,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self { message: message.into(), exit_code: 255 }
    }

    fn value(message: impl Into<String>) -> Self {
        Self { message: message.into(), exit_code: 1 }
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

struct LongOpt {
    name: &'static str,
    short: char,
}

/// Every long option ttyd accepts, in its own order.
const LONG_OPTS: &[LongOpt] = &[
    LongOpt { name: "port", short: 'p' },
    LongOpt { name: "interface", short: 'i' },
    LongOpt { name: "socket-owner", short: 'U' },
    LongOpt { name: "credential", short: 'c' },
    LongOpt { name: "auth-header", short: 'H' },
    LongOpt { name: "uid", short: 'u' },
    LongOpt { name: "gid", short: 'g' },
    LongOpt { name: "signal", short: 's' },
    LongOpt { name: "cwd", short: 'w' },
    LongOpt { name: "index", short: 'I' },
    LongOpt { name: "base-path", short: 'b' },
    LongOpt { name: "ping-interval", short: 'P' },
    LongOpt { name: "srv-buf-size", short: 'f' },
    LongOpt { name: "ipv6", short: '6' },
    LongOpt { name: "ssl", short: 'S' },
    LongOpt { name: "ssl-cert", short: 'C' },
    LongOpt { name: "ssl-key", short: 'K' },
    LongOpt { name: "ssl-ca", short: 'A' },
    LongOpt { name: "url-arg", short: 'a' },
    LongOpt { name: "writable", short: 'W' },
    LongOpt { name: "terminal-type", short: 'T' },
    LongOpt { name: "client-option", short: 't' },
    LongOpt { name: "check-origin", short: 'O' },
    LongOpt { name: "max-clients", short: 'm' },
    LongOpt { name: "once", short: 'o' },
    LongOpt { name: "exit-no-conn", short: 'q' },
    LongOpt { name: "browser", short: 'B' },
    LongOpt { name: "debug", short: 'd' },
    LongOpt { name: "version", short: 'v' },
    LongOpt { name: "help", short: 'h' },
];

fn short_takes_arg(c: char) -> bool {
    matches!(
        c,
        'p' | 'i'
            | 'U'
            | 'c'
            | 'H'
            | 'u'
            | 'g'
            | 's'
            | 'w'
            | 'I'
            | 'b'
            | 'P'
            | 'f'
            | 'C'
            | 'K'
            | 'A'
            | 't'
            | 'T'
            | 'm'
            | 'd'
    )
}

fn short_exists(c: char) -> bool {
    LONG_OPTS.iter().any(|opt| opt.short == c)
}

/// Parses `args`, excluding the program name.
pub fn parse(prog: &str, args: &[String]) -> Result<Invocation> {
    let mut cfg = Config::default();
    let mut positionals: Vec<String> = Vec::new();
    let mut only_positional = false;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].clone();
        i += 1;

        if only_positional || arg == "-" || !arg.starts_with('-') {
            // Everything from here on is the command, including option-looking
            // tokens: command flags must reach the command untouched.
            positionals.push(arg);
            positionals.extend(args[i..].iter().cloned());
            break;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }

        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let short = resolve_long(prog, name)?;
            if short_takes_arg(short) {
                let value = match inline {
                    Some(value) => value,
                    None => next_value(args, &mut i).ok_or_else(|| {
                        CliError::usage(format!("{prog}: option '--{name}' requires an argument"))
                    })?,
                };
                if let Some(done) = apply_value(&mut cfg, short, &value, args, &mut i, prog)? {
                    return Ok(done);
                }
            } else {
                if inline.is_some() {
                    return Err(CliError::usage(format!(
                        "{prog}: option '--{name}' doesn't allow an argument"
                    )));
                }
                if let Some(done) = apply_flag(&mut cfg, short, prog)? {
                    return Ok(done);
                }
            }
            continue;
        }

        // Short options, possibly bundled and/or with an attached value.
        let chars: Vec<char> = arg.chars().skip(1).collect();
        let mut index = 0;
        while index < chars.len() {
            let short = chars[index];
            if !short_exists(short) {
                return Err(CliError::usage(format!("{prog}: invalid option -- '{short}'")));
            }
            if short_takes_arg(short) {
                let rest: String = chars[index + 1..].iter().collect();
                let value = if rest.is_empty() {
                    next_value(args, &mut i).ok_or_else(|| {
                        CliError::usage(format!("{prog}: option requires an argument -- '{short}'"))
                    })?
                } else {
                    rest
                };
                if let Some(done) = apply_value(&mut cfg, short, &value, args, &mut i, prog)? {
                    return Ok(done);
                }
                break;
            }
            if let Some(done) = apply_flag(&mut cfg, short, prog)? {
                return Ok(done);
            }
            index += 1;
        }
    }

    cfg.command = positionals;
    if cfg.command.is_empty() {
        return Err(CliError::usage(format!("{prog}: missing start command")));
    }
    Ok(Invocation::Run(Box::new(cfg)))
}

fn next_value(args: &[String], i: &mut usize) -> Option<String> {
    if *i < args.len() {
        let value = args[*i].clone();
        *i += 1;
        Some(value)
    } else {
        None
    }
}

fn resolve_long(prog: &str, name: &str) -> Result<char> {
    if name.is_empty() {
        return Err(CliError::usage(format!("{prog}: unrecognized option '--'")));
    }
    if let Some(exact) = LONG_OPTS.iter().find(|opt| opt.name == name) {
        return Ok(exact.short);
    }
    let candidates: Vec<&LongOpt> =
        LONG_OPTS.iter().filter(|opt| opt.name.starts_with(name)).collect();
    match candidates.len() {
        1 => Ok(candidates[0].short),
        0 => Err(CliError::usage(format!("{prog}: unrecognized option '--{name}'"))),
        _ => {
            let names: Vec<String> =
                candidates.iter().map(|opt| format!("'--{}'", opt.name)).collect();
            Err(CliError::usage(format!(
                "{prog}: option '--{name}' is ambiguous; possibilities: {}",
                names.join(" ")
            )))
        }
    }
}

fn apply_flag(cfg: &mut Config, c: char, _prog: &str) -> Result<Option<Invocation>> {
    match c {
        'a' => cfg.url_arg = true,
        'W' => cfg.writable = true,
        'O' => cfg.check_origin = true,
        'o' => cfg.once = true,
        'q' => cfg.exit_no_conn = true,
        'B' => cfg.browser = true,
        '6' => cfg.ipv6 = true,
        'S' => cfg.ssl = true,
        'h' => return Ok(Some(Invocation::Help)),
        'v' => return Ok(Some(Invocation::Version)),
        other => {
            return Err(CliError::usage(format!("invalid option -- '{other}'")));
        }
    }
    Ok(None)
}

fn apply_value(
    cfg: &mut Config,
    c: char,
    value: &str,
    args: &[String],
    i: &mut usize,
    prog: &str,
) -> Result<Option<Invocation>> {
    match c {
        'p' => {
            let port = parse_int("port", value)?;
            if !(0..=65535).contains(&port) {
                return Err(CliError::usage(format!("{prog}: invalid port: {value}")));
            }
            cfg.port = port as u16;
        }
        'i' => cfg.interface = Some(value.to_string()),
        'U' => cfg.socket_owner = Some(value.to_string()),
        'c' => {
            if !value.contains(':') {
                return Err(CliError::usage(format!(
                    "{prog}: invalid credential, format: username:password"
                )));
            }
            use base64::Engine as _;
            cfg.credential = Some(base64::engine::general_purpose::STANDARD.encode(value));
        }
        'H' => cfg.auth_header = Some(value.to_string()),
        'u' => {
            let uid = parse_int("uid", value)?;
            if uid < 0 {
                return Err(CliError::usage(format!("{prog}: invalid uid: {value}")));
            }
            cfg.uid = Some(uid as u32);
        }
        'g' => {
            let gid = parse_int("gid", value)?;
            if gid < 0 {
                return Err(CliError::usage(format!("{prog}: invalid gid: {value}")));
            }
            cfg.gid = Some(gid as u32);
        }
        's' => {
            let sig = signal_from_name(value);
            if sig <= 0 {
                return Err(CliError::usage(format!("{prog}: invalid signal: {value}")));
            }
            cfg.sig_code = sig;
        }
        'w' => cfg.cwd = Some(PathBuf::from(value)),
        'I' => {
            let path = expand_home(value);
            let meta = std::fs::metadata(&path).map_err(|err| {
                CliError::usage(format!(
                    "Can not stat index.html: {}, error: {err}",
                    path.display()
                ))
            })?;
            if meta.is_dir() {
                return Err(CliError::usage(format!(
                    "Invalid index.html path: {}, is it a dir?",
                    path.display()
                )));
            }
            cfg.index = Some(path);
        }
        'b' => cfg.base_path = normalize_base_path(value),
        'P' => {
            let interval = parse_int("ping-interval", value)?;
            if interval < 0 {
                return Err(CliError::usage(format!(
                    "{prog}: invalid ping interval: {value}"
                )));
            }
            cfg.ping_interval = interval as u64;
        }
        'f' => {
            let size = parse_int("srv-buf-size", value)?;
            if size <= 0 {
                return Err(CliError::usage(format!(
                    "{prog}: invalid srv-buf-size: {value}"
                )));
            }
            cfg.srv_buf_size = size as usize;
        }
        'C' => cfg.ssl_cert = Some(PathBuf::from(value)),
        'K' => cfg.ssl_key = Some(PathBuf::from(value)),
        'A' => cfg.ssl_ca = Some(PathBuf::from(value)),
        'T' => cfg.terminal_type = value.to_string(),
        'm' => {
            let max = parse_int("max-clients", value)?;
            if max < 0 {
                return Err(CliError::usage(format!(
                    "{prog}: invalid max-clients: {value}"
                )));
            }
            cfg.max_clients = max as usize;
        }
        'd' => cfg.debug = parse_int("debug", value)? as i32,
        't' => add_client_option(cfg, value, prog)?,
        other => {
            return Err(CliError::usage(format!("{prog}: invalid option -- '{other}'")));
        }
    }
    Ok(None)
}

fn add_client_option(cfg: &mut Config, raw: &str, prog: &str) -> Result<()> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err(CliError::usage(format!(
            "{prog}: invalid client option: {raw}, format: key=value"
        )));
    };
    // ttyd parses the value as JSON and falls back to a plain string.
    let parsed =
        serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
    cfg.client_options.push((key.to_string(), parsed));
    Ok(())
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn normalize_base_path(value: &str) -> String {
    let mut path = value.to_string();
    while path.ends_with('/') {
        path.pop();
    }
    path.truncate(MAX_BASE_PATH_LEN);
    path
}

/// `strtol(str, _, 0)`: base prefixes and an optional sign are honoured.
fn parse_int(name: &str, value: &str) -> Result<i64> {
    let trimmed = value.trim_start();
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let (radix, digits) = if let Some(hex) = digits.strip_prefix("0x").or(digits.strip_prefix("0X")) {
        (16, hex)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };
    let magnitude = if digits.is_empty() {
        if radix == 8 { 0 } else { return Err(invalid_value(name, value)) }
    } else {
        i64::from_str_radix(digits, radix).map_err(|_| invalid_value(name, value))?
    };
    Ok(if negative { -magnitude } else { magnitude })
}

fn invalid_value(name: &str, value: &str) -> CliError {
    CliError::value(format!("fttyd: invalid value for {name}: {value}"))
}

/// Signals ttyd can name, using the same table it does.
const SIGNALS: &[(i32, &str)] = &[
    (libc::SIGHUP, "HUP"),
    (libc::SIGINT, "INT"),
    (libc::SIGQUIT, "QUIT"),
    (libc::SIGILL, "ILL"),
    (libc::SIGTRAP, "TRAP"),
    (libc::SIGABRT, "ABRT"),
    (libc::SIGBUS, "BUS"),
    (libc::SIGFPE, "FPE"),
    (libc::SIGKILL, "KILL"),
    (libc::SIGUSR1, "USR1"),
    (libc::SIGSEGV, "SEGV"),
    (libc::SIGUSR2, "USR2"),
    (libc::SIGPIPE, "PIPE"),
    (libc::SIGALRM, "ALRM"),
    (libc::SIGTERM, "TERM"),
    (libc::SIGSTKFLT, "STKFLT"),
    (libc::SIGCHLD, "CHLD"),
    (libc::SIGCONT, "CONT"),
    (libc::SIGSTOP, "STOP"),
    (libc::SIGTSTP, "TSTP"),
    (libc::SIGTTIN, "TTIN"),
    (libc::SIGTTOU, "TTOU"),
    (libc::SIGURG, "URG"),
    (libc::SIGXCPU, "XCPU"),
    (libc::SIGXFSZ, "XFSZ"),
    (libc::SIGVTALRM, "VTALRM"),
    (libc::SIGPROF, "PROF"),
    (libc::SIGWINCH, "WINCH"),
    (libc::SIGIO, "IO"),
    (libc::SIGPWR, "PWR"),
    (libc::SIGSYS, "SYS"),
];

pub fn signal_from_name(name: &str) -> i32 {
    let upper = name.to_ascii_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    if let Some((sig, _)) = SIGNALS.iter().find(|(_, label)| *label == bare) {
        return *sig;
    }
    upper.parse::<i32>().unwrap_or_default()
}

pub fn signal_name(sig: i32) -> String {
    match SIGNALS.iter().find(|(value, _)| *value == sig) {
        Some((_, label)) => format!("SIG{label}"),
        None => format!("SIG{sig}"),
    }
}

pub fn help_text(prog: &str) -> String {
    format!(
        "{prog} is a tool for sharing terminal over the web\n\
         \n\
         USAGE:\n    \
         {prog} [options] <command> [<arguments...>]\n\
         \n\
         VERSION:\n    \
         {VERSION}\n\
         \n\
         OPTIONS:\n    \
         -p, --port              Port to listen (default: 7681, use `0` for random port)\n    \
         -i, --interface         Network interface to bind (eg: eth0), or UNIX domain socket path (eg: /var/run/ttyd.sock)\n    \
         -U, --socket-owner      User owner of the UNIX domain socket file, when enabled (eg: user:group)\n    \
         -c, --credential        Credential for basic authentication (format: username:password)\n    \
         -H, --auth-header       HTTP Header name for auth proxy, this will configure ttyd to let a HTTP reverse proxy handle authentication\n    \
         -u, --uid               User id to run with\n    \
         -g, --gid               Group id to run with\n    \
         -s, --signal            Signal to send to the command when exit it (default: 1, SIGHUP)\n    \
         -w, --cwd               Working directory to be set for the child program\n    \
         -a, --url-arg           Allow client to send command line arguments in URL (eg: http://localhost:7681?arg=foo&arg=bar)\n    \
         -W, --writable          Allow clients to write to the TTY (readonly by default)\n    \
         -t, --client-option     Send option to client (format: key=value), repeat to add more options\n    \
         -T, --terminal-type     Terminal type to report, default: xterm-256color\n    \
         -O, --check-origin      Do not allow websocket connection from different origin\n    \
         -m, --max-clients       Maximum clients to support (default: 0, no limit)\n    \
         -o, --once              Accept only one client and exit on disconnection\n    \
         -q, --exit-no-conn      Exit on all clients disconnection\n    \
         -B, --browser           Open terminal with the default system browser\n    \
         -I, --index             Custom index.html path\n    \
         -b, --base-path         Expected base path for requests coming from a reverse proxy (eg: /mounted/here, max length: 128)\n    \
         -P, --ping-interval     Websocket ping interval(sec) (default: 5)\n    \
         -f, --srv-buf-size      Maximum chunk of file (in bytes) that can be sent at once, a larger value may improve throughput (default: 4096)\n    \
         -6, --ipv6              Enable IPv6 support\n    \
         -S, --ssl               Enable SSL\n    \
         -C, --ssl-cert          SSL certificate file path\n    \
         -K, --ssl-key           SSL key file path\n    \
         -A, --ssl-ca            SSL CA file path for client certificate verification\n    \
         -d, --debug             Set log level (default: 7)\n    \
         -v, --version           Print the version and exit\n    \
         -h, --help              Print this text and exit\n\
         \n\
         Visit https://github.com/Foorack/fttyd to get more information and report bugs.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(args: &[&str]) -> Config {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match parse("fttyd", &args).expect("parse") {
            Invocation::Run(cfg) => *cfg,
            other => panic!("unexpected invocation: {other:?}"),
        }
    }

    #[test]
    fn defaults_match_ttyd() {
        let cfg = cfg(&["bash"]);
        assert_eq!(cfg.port, 7681);
        assert_eq!(cfg.sig_code, libc::SIGHUP);
        assert_eq!(signal_name(cfg.sig_code), "SIGHUP");
        assert_eq!(cfg.terminal_type, "xterm-256color");
        assert_eq!(cfg.ping_interval, 5);
        assert_eq!(cfg.srv_buf_size, 4096);
        assert_eq!(cfg.debug, 7);
        assert!(!cfg.writable);
        assert_eq!(cfg.command, vec!["bash".to_string()]);
    }

    #[test]
    fn bundled_short_flags() {
        let cfg = cfg(&["-Wa", "bash"]);
        assert!(cfg.writable);
        assert!(cfg.url_arg);
        assert_eq!(cfg.command, vec!["bash".to_string()]);
    }

    #[test]
    fn attached_short_value() {
        assert_eq!(cfg(&["-p9000", "bash"]).port, 9000);
    }

    #[test]
    fn option_parsing_stops_at_the_command() {
        let cfg = cfg(&["bash", "-W"]);
        assert!(!cfg.writable);
        assert_eq!(cfg.command, vec!["bash".to_string(), "-W".to_string()]);
    }

    #[test]
    fn double_dash_is_also_accepted() {
        let cfg = cfg(&["--", "bash", "-W"]);
        assert!(!cfg.writable);
        assert_eq!(cfg.command, vec!["bash".to_string(), "-W".to_string()]);
    }

    #[test]
    fn command_arguments_are_preserved() {
        let cfg = cfg(&["-W", "bash", "-l", "-i"]);
        assert!(cfg.writable);
        assert_eq!(cfg.command, vec!["bash".to_string(), "-l".to_string(), "-i".to_string()]);
    }

    #[test]
    fn complex_commands_survive_untouched() {
        let cfg = cfg(&["-W", "tmux", "new", "-As", "main"]);
        assert_eq!(
            cfg.command,
            vec!["tmux".to_string(), "new".to_string(), "-As".to_string(), "main".to_string()]
        );
    }

    #[test]
    fn missing_command_is_an_error() {
        let args: Vec<String> = vec!["-W".to_string()];
        let err = parse("fttyd", &args).unwrap_err();
        assert!(err.message.contains("missing start command"));
    }

    #[test]
    fn help_and_version_short_circuit() {
        let args: Vec<String> = vec!["-h".to_string()];
        assert!(matches!(parse("fttyd", &args).unwrap(), Invocation::Help));
        let args: Vec<String> = vec!["-v".to_string()];
        assert!(matches!(parse("fttyd", &args).unwrap(), Invocation::Version));
        let args: Vec<String> = vec!["-Wv".to_string()];
        assert!(matches!(parse("fttyd", &args).unwrap(), Invocation::Version));
    }

    #[test]
    fn credential_is_base64_encoded() {
        let cfg = cfg(&["-c", "user:pass", "bash"]);
        assert_eq!(cfg.credential.as_deref(), Some("dXNlcjpwYXNz"));
    }

    #[test]
    fn credential_requires_colon() {
        let args: Vec<String> = vec!["-c".into(), "userpass".into()];
        let err = parse("fttyd", &args).unwrap_err();
        assert!(err.message.contains("username:password"));
        assert_eq!(err.exit_code, 255);
    }

    #[test]
    fn client_options_parse_json_values() {
        let cfg = cfg(&[
            "-t",
            "fontSize=20",
            "-t",
            "titleFixed=hello",
            "-t",
            "cursorBlink=true",
            "bash",
        ]);
        let value: serde_json::Value = serde_json::from_str(&cfg.client_options_json()).unwrap();
        assert_eq!(value["fontSize"], serde_json::json!(20));
        assert_eq!(value["titleFixed"], serde_json::json!("hello"));
        assert_eq!(value["cursorBlink"], serde_json::json!(true));
    }

    #[test]
    fn client_options_can_be_repeated() {
        let cfg = cfg(&["-t", "a=1", "-t", "b=2", "bash"]);
        assert_eq!(cfg.client_options.len(), 2);
        assert_eq!(cfg.command, vec!["bash".to_string()]);
    }

    #[test]
    fn client_option_without_assignment_is_rejected() {
        let args: Vec<String> = vec!["-t".into(), "bash".into()];
        assert!(parse("fttyd", &args).unwrap_err().message.contains("format: key=value"));
    }

    #[test]
    fn base_path_is_normalised() {
        let cfg = cfg(&["-b", "/mounted/here/", "bash"]);
        assert_eq!(cfg.base_path, "/mounted/here");
        assert_eq!(cfg.ws_path(), "/mounted/here/ws");
        assert_eq!(cfg.token_path(), "/mounted/here/token");
        assert_eq!(cfg.index_path(), "/mounted/here/");
    }

    #[test]
    fn empty_base_path_yields_root_paths() {
        let cfg = cfg(&["bash"]);
        assert_eq!(cfg.ws_path(), "/ws");
        assert_eq!(cfg.index_path(), "/");
    }

    #[test]
    fn long_options_accept_unique_prefixes() {
        assert!(cfg(&["--writ", "bash"]).writable);
    }

    #[test]
    fn ambiguous_long_option_is_rejected() {
        let args: Vec<String> = vec!["--s".into()];
        assert!(parse("fttyd", &args).unwrap_err().message.contains("ambiguous"));
    }

    #[test]
    fn unknown_option_is_rejected() {
        let args: Vec<String> = vec!["--nope".into()];
        assert!(parse("fttyd", &args).unwrap_err().message.contains("unrecognized option"));
    }

    #[test]
    fn signals_accept_names_and_numbers() {
        assert_eq!(signal_from_name("hup"), libc::SIGHUP);
        assert_eq!(signal_from_name("SIGTERM"), libc::SIGTERM);
        assert_eq!(signal_from_name("KILL"), libc::SIGKILL);
        assert_eq!(signal_from_name("9"), 9);
        assert_eq!(signal_from_name("nope"), 0);
        assert_eq!(signal_name(libc::SIGHUP), "SIGHUP");
    }

    #[test]
    fn unix_socket_detection() {
        assert_eq!(cfg(&["-i", "/tmp/x.sock", "bash"]).unix_socket_path(), Some("/tmp/x.sock"));
        assert_eq!(cfg(&["-i", "eth0", "bash"]).unix_socket_path(), None);
    }

    #[test]
    fn int_parsing_follows_strtol() {
        assert_eq!(parse_int("port", "0x10").unwrap(), 16);
        assert_eq!(parse_int("port", "010").unwrap(), 8);
        assert_eq!(parse_int("port", "-1").unwrap(), -1);
        assert!(parse_int("port", "8080x").is_err());
        assert!(parse_int("port", "").is_err());
    }

    #[test]
    fn custom_index_must_exist_and_not_be_a_dir() {
        let args: Vec<String> = vec!["-I".into(), "/definitely/not/here".into()];
        assert!(parse("fttyd", &args).unwrap_err().message.contains("Can not stat index.html"));
    }
}
