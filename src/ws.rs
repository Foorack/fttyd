//! The terminal websocket: ttyd's protocol, one child process per connection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::time::{Instant as TokioInstant, timeout};

use crate::auth;
use crate::proto::{self, ControlMessage, WindowSize};
use crate::pty::{ExitStatus, PtyControl, PtyOutput, SessionSpec};
use crate::state::{AppState, AuthUser, ClientGuard};

/// A client that never sends its control message is dropped after this long.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for the exit status once the PTY output has ended.
const EXIT_GRACE: Duration = Duration::from_secs(5);
/// How long to keep flushing output after the child exited.
const DRAIN_GRACE: Duration = Duration::from_millis(200);
/// ttyd hangs up on a client that has been silent for `interval + 7` seconds.
const HANGUP_SLACK_SECS: u64 = 7;
/// How often the child's working directory is checked for the sidebar.
const CWD_POLL_INTERVAL: Duration = Duration::from_millis(1000);

pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    user: axum::extract::Extension<AuthUser>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let cfg = state.cfg.clone();

    if cfg.check_origin && !auth::origin_matches_host(&headers) {
        tracing::warn!("refusing websocket connection from a different origin");
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    if !state.can_accept_client() {
        tracing::warn!("refusing websocket client due to --once/--max-clients");
        return (StatusCode::SERVICE_UNAVAILABLE, "too many clients").into_response();
    }

    let extra_args = if cfg.url_arg { url_args(query.as_deref().unwrap_or("")) } else { Vec::new() };
    let user = user.0.0;

    ws.protocols(["tty"])
        .on_upgrade(move |socket| async move {
            // The guard is created inside the upgraded callback so a failed upgrade
            // can never leak a client slot.
            let _guard = ClientGuard::new(state.clone());
            terminal_session(state, socket, user, extra_args).await;
        })
        .into_response()
}

async fn terminal_session(
    state: Arc<AppState>,
    socket: WebSocket,
    user: Option<String>,
    extra_args: Vec<String>,
) {
    let cfg = state.cfg.clone();
    let (mut sink, mut stream) = socket.split();

    let title = proto::initial_title(&cfg.command.join(" "));
    let prefs = proto::initial_preferences(&cfg.client_options_json());
    if sink.send(Message::Binary(title.into())).await.is_err() {
        return;
    }
    if sink.send(Message::Binary(prefs.into())).await.is_err() {
        return;
    }

    let Some(control) = handshake(&mut sink, &mut stream, &cfg).await else {
        return;
    };

    let mut command = cfg.command.clone();
    command.extend(extra_args);
    let spec = SessionSpec {
        command,
        cwd: cfg.cwd.clone(),
        terminal_type: cfg.terminal_type.clone(),
        user,
        size: control.window_size(),
        read_buffer: cfg.srv_buf_size,
    };

    let (control_handle, output) = match crate::pty::spawn(&spec) {
        Ok(pair) => pair,
        Err(err) => {
            tracing::error!("failed to start {:?}: {err}", spec.command);
            let frame = CloseFrame {
                code: proto::CLOSE_ABNORMAL,
                reason: Utf8Bytes::from(format!("failed to start command: {err}")),
            };
            let _ = sink.send(Message::Close(Some(frame))).await;
            return;
        }
    };
    tracing::info!("started process, pid: {:?}, clients: {}", control_handle.pid(), state.client_count());

    run_session(&cfg, control_handle, output, &mut sink, stream).await;
}

/// Waits for the client's `{...}` control message, which carries the size and the
/// authentication token. Mirrors ttyd, where the child is spawned only at this point.
async fn handshake(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    cfg: &crate::cli::Config,
) -> Option<ControlMessage> {
    let deadline = TokioInstant::now() + HANDSHAKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        let message = timeout(remaining, stream.next()).await.ok().flatten();
        let message = match message {
            None => return None,
            Some(Err(_)) => return None,
            Some(Ok(message)) => message,
        };
        tracing::trace!("handshake message: {message:?}");
        let payload = match message {
            Message::Binary(data) => data.to_vec(),
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Close(_) => return None,
            Message::Ping(payload) => {
                if sink.send(Message::Pong(payload)).await.is_err() {
                    return None;
                }
                continue;
            }
            Message::Pong(_) => continue,
        };
        if payload.first() != Some(&proto::JSON_DATA) {
            // ttyd rejects non-JSON traffic before the token has been presented.
            if cfg.credential.is_some() {
                tracing::warn!("ws client not authenticated");
                return None;
            }
            continue;
        }
        let Some(control) = ControlMessage::parse(&payload) else {
            continue;
        };
        if let Some(credential) = &cfg.credential {
            if control.auth_token.as_deref() != Some(credential.as_str()) {
                tracing::warn!("ws authentication failed");
                let frame = CloseFrame { code: proto::CLOSE_POLICY_VIOLATION, reason: Utf8Bytes::from_static("unauthorized") };
                let _ = sink.send(Message::Close(Some(frame))).await;
                return None;
            }
        }
        return Some(control);
    }
}

async fn run_session(
    cfg: &crate::cli::Config,
    control: PtyControl,
    mut output: PtyOutput,
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    mut stream: futures_util::stream::SplitStream<WebSocket>,
) {
    let ping_interval = Duration::from_secs(cfg.ping_interval.max(1));
    let hangup_after = ping_interval + Duration::from_secs(HANGUP_SLACK_SECS);
    let mut ping = tokio::time::interval(ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut paused = false;
    let mut output_closed = false;
    let mut cwd_ticker = tokio::time::interval(CWD_POLL_INTERVAL);
    cwd_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_cwd: Option<String> = None;
    let mut exit_status: Option<ExitStatus> = None;
    let mut last_rx = Instant::now();
    let mut exited = output.exited;

    let client_gone = loop {
        tokio::select! {
            biased;

            _ = ping.tick() => {
                // Drop silent peers the way ttyd's libwebsockets retry policy does.
                if last_rx.elapsed() > hangup_after {
                    tracing::info!("closing websocket after {hangup_after:?} of silence");
                    break true;
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break true;
                }
            }

            message = stream.next() => {
                match message {
                    None | Some(Err(_)) => break true,
                    Some(Ok(Message::Close(_))) => break true,
                    Some(Ok(Message::Ping(payload))) => {
                        last_rx = Instant::now();
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break true;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => last_rx = Instant::now(),
                    Some(Ok(message)) => {
                        last_rx = Instant::now();
                        if handle_client_message(cfg, &control, message, &mut paused).await.is_err() {
                            break true;
                        }
                    }
                }
            }

            _ = cwd_ticker.tick(), if !output_closed => {
                if let Some(path) = process_cwd(control.pid()) {
                    if last_cwd.as_deref() != Some(path.as_str()) {
                        last_cwd = Some(path.clone());
                        if sink.send(Message::Binary(proto::cwd_frame(&path).into())).await.is_err() {
                            break true;
                        }
                    }
                }
            }

            chunk = output.output.recv(), if !output_closed && !paused => {
                match chunk {
                    Some(data) => {
                        if sink.send(Message::Binary(proto::frame(proto::OUTPUT, &data).into())).await.is_err() {
                            break true;
                        }
                    }
                    None => output_closed = true,
                }
            }

            status = &mut exited => {
                exit_status = status.ok();
                break false;
            }
        }
    };

    if client_gone {
        control.signal(cfg.sig_code);
        tracing::info!("ws closed, signalled child {:?} with {}", control.pid(), crate::cli::signal_name(cfg.sig_code));
        return;
    }

    // The child is gone. Flush whatever is still buffered so the last line of output
    // is not lost, then close with the status-derived code.
    if exit_status.is_none() {
        exit_status = timeout(EXIT_GRACE, exited).await.ok().and_then(|result| result.ok());
    }
    while let Ok(Some(data)) = timeout(DRAIN_GRACE, output.output.recv()).await {
        if sink.send(Message::Binary(proto::frame(proto::OUTPUT, &data).into())).await.is_err() {
            return;
        }
    }

    let code = exit_status.map(|status| status.close_code()).unwrap_or(proto::CLOSE_ABNORMAL);
    tracing::info!("process exited, closing websocket with {code}");
    let frame = CloseFrame { code, reason: Utf8Bytes::from_static("") };
    let _ = sink.send(Message::Close(Some(frame))).await;
}

async fn handle_client_message(
    cfg: &crate::cli::Config,
    control: &PtyControl,
    message: Message,
    paused: &mut bool,
) -> Result<(), ()> {
    let payload = match message {
        Message::Binary(data) => data.to_vec(),
        Message::Text(text) => text.as_bytes().to_vec(),
        _ => return Ok(()),
    };
    let Some((command, body)) = payload.split_first() else {
        return Ok(());
    };
    match *command {
        proto::INPUT => {
            if !cfg.writable {
                return Ok(());
            }
            if !control.write(body.to_vec()).await {
                return Err(());
            }
        }
        proto::RESIZE_TERMINAL => {
            let size = WindowSize::parse(body);
            if let Err(err) = control.resize(size) {
                tracing::warn!("failed to resize pty: {err}");
            }
        }
        proto::PAUSE => *paused = true,
        proto::RESUME => *paused = false,
        proto::JSON_DATA => {}
        other => tracing::debug!("ignored unknown message type: {other:?}"),
    }
    Ok(())
}

/// Reads the child's working directory. Works for any shell, including one that was
/// replaced by `sudo su`, because the kernel follows the process, not the shell.
fn process_cwd(pid: Option<u32>) -> Option<String> {
    let pid = pid?;
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Collects every `arg=` value from the websocket query string, in order, which is
/// what `--url-arg` appends to the command line.
fn url_args(query: &str) -> Vec<String> {
    use percent_encoding::percent_decode_str;

    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            if key != "arg" {
                return None;
            }
            Some(percent_decode_str(value).decode_utf8_lossy().into_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_args_are_collected_in_order() {
        assert_eq!(url_args("arg=a&arg=b%20c&other=x"), vec!["a".to_string(), "b c".to_string()]);
    }

    #[test]
    fn url_args_ignore_unrelated_and_empty_query() {
        assert!(url_args("").is_empty());
        assert!(url_args("foo=1&bar=2").is_empty());
        assert_eq!(url_args("arg="), vec![String::new()]);
    }
}
