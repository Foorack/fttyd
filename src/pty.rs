//! Terminal process management: one PTY and one child per websocket connection,
//! matching ttyd's model.
//!
//! A session is split in two so the websocket loop can wait on output and exit
//! concurrently while it writes input and applies resizes:
//!
//! * [`PtyControl`] writes input, resizes and signals the child,
//! * [`PtyOutput`] yields output chunks and the final exit status.

use std::io::{Read, Write};
use std::path::PathBuf;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

use crate::proto::WindowSize;

/// How many input chunks may queue up before the websocket read loop waits.
const INPUT_QUEUE: usize = 256;
/// How many PTY output chunks may queue up before the reader stops reading.
const OUTPUT_QUEUE: usize = 1;

/// Everything needed to start a child, resolved per connection.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub terminal_type: String,
    pub user: Option<String>,
    pub size: WindowSize,
    pub read_buffer: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: u32,
    pub signal: Option<String>,
}

impl ExitStatus {
    /// ttyd maps a clean exit to 1000 and everything else to 1006.
    pub fn close_code(&self) -> u16 {
        if self.code == 0 { crate::proto::CLOSE_NORMAL } else { crate::proto::CLOSE_ABNORMAL }
    }
}

pub struct PtyControl {
    // `MasterPty` is `Send` but not `Sync`; the mutex makes the control handle usable
    // from the websocket task, which must produce a `Send` future. It is only ever
    // locked for the duration of a non-blocking ioctl.
    master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    pid: Option<u32>,
    input: mpsc::Sender<Vec<u8>>,
}

pub struct PtyOutput {
    pub output: mpsc::Receiver<Vec<u8>>,
    pub exited: oneshot::Receiver<ExitStatus>,
}

/// Spawns `spec.command` on a fresh PTY.
pub fn spawn(spec: &SessionSpec) -> std::io::Result<(PtyControl, PtyOutput)> {
    let Some((program, args)) = spec.command.split_first() else {
        return Err(std::io::Error::other("no command to run"));
    };

    let pty_system = native_pty_system();
    let size = PtySize {
        rows: spec.size.rows.max(1),
        cols: spec.size.columns.max(1),
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_system.openpty(size).map_err(|err| std::io::Error::other(err.to_string()))?;

    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.env("TERM", &spec.terminal_type);
    if let Some(user) = &spec.user {
        command.env("TTYD_USER", user);
    }
    if let Some(cwd) = &spec.cwd {
        command.cwd(cwd);
    }

    let mut child =
        pair.slave.spawn_command(command).map_err(|err| std::io::Error::other(err.to_string()))?;
    // Releasing the slave side is what makes the child see EOF when it exits.
    drop(pair.slave);

    let pid = child.process_id();
    let mut reader =
        pair.master.try_clone_reader().map_err(|err| std::io::Error::other(err.to_string()))?;
    let mut writer =
        pair.master.take_writer().map_err(|err| std::io::Error::other(err.to_string()))?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_QUEUE);
    let (exit_tx, exit_rx) = oneshot::channel::<ExitStatus>();
    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(INPUT_QUEUE);
    let read_buffer = spec.read_buffer.max(1);

    std::thread::Builder::new().name("fttyd-pty-read".into()).spawn(move || {
        let mut buffer = vec![0u8; read_buffer];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    // `blocking_send` implements flow control: when the consumer stops
                    // draining, this thread parks and the kernel PTY buffer fills,
                    // which throttles the child instead of growing our memory.
                    if output_tx.blocking_send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                // EIO is how a Linux PTY master reports that the child went away.
                Err(_) => break,
            }
        }
    })?;

    std::thread::Builder::new().name("fttyd-pty-write".into()).spawn(move || {
        while let Some(chunk) = input_rx.blocking_recv() {
            if writer.write_all(&chunk).is_err() {
                break;
            }
        }
    })?;

    std::thread::Builder::new().name("fttyd-pty-wait".into()).spawn(move || {
        let status = match child.wait() {
            Ok(status) => ExitStatus {
                code: status.exit_code(),
                signal: status.signal().map(|signal| signal.to_string()),
            },
            // The child could not be reaped: report it as an abnormal exit.
            Err(_) => ExitStatus { code: u32::from(u8::MAX), signal: None },
        };
        let _ = exit_tx.send(status);
    })?;

    Ok((
        PtyControl { master: std::sync::Mutex::new(pair.master), pid, input: input_tx },
        PtyOutput { output: output_rx, exited: exit_rx },
    ))
}

impl PtyControl {
    /// Queues bytes for the child, waiting if the child is not draining its input.
    pub async fn write(&self, data: Vec<u8>) -> bool {
        self.input.send(data).await.is_ok()
    }

    pub fn resize(&self, size: WindowSize) -> std::io::Result<()> {
        if !size.is_usable() {
            return Ok(());
        }
        let master = self
            .master
            .lock()
            .map_err(|_| std::io::Error::other("pty control lock poisoned"))?;
        master
            .resize(PtySize {
                rows: size.rows,
                cols: size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    /// Sends `signal` to the child, like ttyd's `--signal` handling on disconnect.
    pub fn signal(&self, signal: i32) {
        if let Some(pid) = self.pid {
            // SAFETY: sending a signal to a pid is safe; a stale pid just fails.
            unsafe {
                libc::kill(pid as libc::pid_t, signal);
            }
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &[&str]) -> SessionSpec {
        SessionSpec {
            command: command.iter().map(|part| part.to_string()).collect(),
            cwd: None,
            terminal_type: "xterm-256color".into(),
            user: None,
            size: WindowSize { columns: 80, rows: 24 },
            read_buffer: 4096,
        }
    }

    #[test]
    fn clean_exit_closes_normally() {
        assert_eq!(ExitStatus { code: 0, signal: None }.close_code(), crate::proto::CLOSE_NORMAL);
        assert_eq!(ExitStatus { code: 1, signal: None }.close_code(), crate::proto::CLOSE_ABNORMAL);
        assert_eq!(
            ExitStatus { code: 129, signal: Some("SIGHUP".into()) }.close_code(),
            crate::proto::CLOSE_ABNORMAL
        );
    }

    #[tokio::test]
    async fn echoes_output_and_reports_exit() {
        let (control, mut output) = spawn(&spec(&["/bin/sh", "-c", "printf hello; exit 0"])).unwrap();
        let mut collected = Vec::new();
        while let Some(chunk) = output.output.recv().await {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(collected, b"hello");
        let status = output.exited.await.unwrap();
        assert_eq!(status.code, 0);
        assert!(control.pid().is_some());
    }

    #[tokio::test]
    async fn exit_code_is_reported() {
        let (_control, mut output) = spawn(&spec(&["/bin/sh", "-c", "exit 3"])).unwrap();
        while output.output.recv().await.is_some() {}
        let status = output.exited.await.unwrap();
        assert_eq!(status.code, 3);
        assert_eq!(status.close_code(), crate::proto::CLOSE_ABNORMAL);
    }

    #[tokio::test]
    async fn input_reaches_the_child() {
        let (control, mut output) =
            spawn(&spec(&["/bin/sh", "-c", "read line; printf 'got:%s' \"$line\""])).unwrap();
        assert!(control.write(b"hi\n".to_vec()).await);
        let mut collected = Vec::new();
        while let Some(chunk) = output.output.recv().await {
            collected.extend_from_slice(&chunk);
            if String::from_utf8_lossy(&collected).contains("got:hi") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&collected);
        assert!(text.contains("got:hi"), "unexpected output: {text:?}");
        control.signal(libc::SIGKILL);
    }

    #[tokio::test]
    async fn signal_terminates_the_child() {
        let (control, mut output) = spawn(&spec(&["/bin/sh", "-c", "sleep 30"])).unwrap();
        control.signal(libc::SIGKILL);
        while output.output.recv().await.is_some() {}
        assert!(output.exited.await.is_ok(), "child should have exited after SIGKILL");
    }

    #[tokio::test]
    async fn terminal_type_is_exported() {
        let (_control, mut output) =
            spawn(&spec(&["/bin/sh", "-c", "printf %s \"$TERM\"; exit 0"])).unwrap();
        let mut collected = Vec::new();
        while let Some(chunk) = output.output.recv().await {
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(String::from_utf8_lossy(&collected), "xterm-256color");
    }
}
