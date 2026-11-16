//! ttyd's wire protocol.
//!
//! Client to server: `0` input, `1` resize + JSON, `2` pause, `3` resume, `{` JSON control.
//! Server to client: `0` output, `1` window title, `2` client preferences.
//!
//! The frames are byte-identical to ttyd's, so ttyd's own frontend works unchanged.

use serde::Deserialize;

// client -> server
pub const INPUT: u8 = b'0';
pub const RESIZE_TERMINAL: u8 = b'1';
pub const PAUSE: u8 = b'2';
pub const RESUME: u8 = b'3';
pub const JSON_DATA: u8 = b'{';

// server -> client
pub const OUTPUT: u8 = b'0';
pub const SET_WINDOW_TITLE: u8 = b'1';
pub const SET_PREFERENCES: u8 = b'2';
/// fttyd extension: the child's working directory, so the sidebar can follow it.
/// ttyd's client ignores unknown commands, so this stays protocol compatible.
pub const SET_CWD: u8 = b'3';

/// Normal closure: the child exited with status 0.
pub const CLOSE_NORMAL: u16 = 1000;
/// Abnormal closure: the child failed or was killed.
pub const CLOSE_ABNORMAL: u16 = 1006;
/// Policy violation: the websocket token was rejected.
pub const CLOSE_POLICY_VIOLATION: u16 = 1008;
/// Sent when the server itself is going away, so clients can say so instead of guessing.
pub const CLOSE_GOING_AWAY: u16 = 1001;

/// Builds a server frame: one command byte followed by the payload.
pub fn frame(command: u8, payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(payload.len() + 1);
    message.push(command);
    message.extend_from_slice(payload);
    message
}

/// Initial `1<title>` message: `"<command> (<hostname>)"`, exactly as ttyd sends it.
pub fn initial_title(command: &str) -> Vec<u8> {
    let hostname = hostname();
    let title = if hostname.is_empty() {
        command.to_string()
    } else {
        format!("{command} ({hostname})")
    };
    frame(SET_WINDOW_TITLE, title.as_bytes())
}

/// Initial `2<prefs>` message: the JSON built from `-t` options.
pub fn initial_preferences(prefs_json: &str) -> Vec<u8> {
    frame(SET_PREFERENCES, prefs_json.as_bytes())
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a valid writable buffer of the length we pass.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Terminal dimensions reported by the client, tolerating missing or invalid fields
/// the way ttyd's json-c parsing does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub struct WindowSize {
    #[serde(default)]
    pub columns: u16,
    #[serde(default)]
    pub rows: u16,
}

impl WindowSize {
    pub fn parse(payload: &[u8]) -> Self {
        serde_json::from_slice(payload).unwrap_or_default()
    }

    /// Only sizes a terminal can actually use are applied.
    pub fn is_usable(self) -> bool {
        self.columns > 0 && self.rows > 0
    }
}

/// The `{...}` control message the client sends right after connecting.
#[derive(Debug, Default, Deserialize)]
pub struct ControlMessage {
    #[serde(rename = "AuthToken", default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub columns: u16,
    #[serde(default)]
    pub rows: u16,
}

impl ControlMessage {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        serde_json::from_slice(payload).ok()
    }

    pub fn window_size(&self) -> WindowSize {
        WindowSize {
            columns: self.columns,
            rows: self.rows,
        }
    }
}

/// Builds the `3{"path": ...}` frame for the sidebar.
pub fn cwd_frame(path: &str) -> Vec<u8> {
    let payload = serde_json::json!({ "path": path }).to_string();
    frame(SET_CWD, payload.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_prefix_the_command_byte() {
        assert_eq!(frame(OUTPUT, b"hi"), b"0hi".to_vec());
        assert_eq!(frame(SET_WINDOW_TITLE, b"t"), b"1t".to_vec());
    }

    #[test]
    fn window_size_tolerates_missing_and_bad_fields() {
        assert_eq!(
            WindowSize::parse(br#"{"columns":80,"rows":24}"#),
            WindowSize {
                columns: 80,
                rows: 24
            }
        );
        assert_eq!(
            WindowSize::parse(br#"{"columns":80}"#),
            WindowSize {
                columns: 80,
                rows: 0
            }
        );
        assert_eq!(WindowSize::parse(b"not json"), WindowSize::default());
    }

    #[test]
    fn window_size_rejects_zero_dimensions() {
        assert!(
            !WindowSize {
                columns: 0,
                rows: 24
            }
            .is_usable()
        );
        assert!(
            WindowSize {
                columns: 80,
                rows: 24
            }
            .is_usable()
        );
    }

    #[test]
    fn control_message_reads_token_and_size() {
        let msg = ControlMessage::parse(br#"{"AuthToken":"abc","columns":120,"rows":40}"#).unwrap();
        assert_eq!(msg.auth_token.as_deref(), Some("abc"));
        assert_eq!(
            msg.window_size(),
            WindowSize {
                columns: 120,
                rows: 40
            }
        );
    }

    #[test]
    fn control_message_without_token() {
        let msg = ControlMessage::parse(br#"{"columns":10,"rows":5}"#).unwrap();
        assert!(msg.auth_token.is_none());
    }

    #[test]
    fn cwd_frame_carries_json() {
        let frame = cwd_frame("/tmp/here");
        assert_eq!(frame[0], SET_CWD);
        assert_eq!(&frame[1..], br#"{"path":"/tmp/here"}"#);
    }

    #[test]
    fn initial_title_includes_hostname() {
        let frame = initial_title("bash");
        assert_eq!(frame[0], SET_WINDOW_TITLE);
        let text = String::from_utf8_lossy(&frame[1..]).into_owned();
        assert!(text.starts_with("bash ("));
    }
}
