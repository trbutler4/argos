use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub windows: u32,
    pub attached_clients: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TmuxError {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
struct RawSession {
    id: String,
    windows: u32,
    attached_clients: u32,
}

pub fn list_sessions(socket: Option<&str>) -> Result<Vec<Session>, TmuxError> {
    if let Some(socket) = socket {
        validate_socket(socket)?;
    }
    let mut command = Command::new("tmux");
    // Force UTF-8 even in a minimal noninteractive environment.
    command.args(["-u", "-N"]);
    if let Some(socket) = socket {
        command.args(["-S", socket]);
    }
    command
        .args([
            "list-sessions",
            "-F",
            "#{session_id}|#{session_windows}|#{session_attached}",
        ])
        .env("LC_ALL", "C");
    let output = command.output().map_err(spawn_error)?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let error = classify_error(&message, output.status.code());
        if error.code == "no_server" || error.code == "no_socket" {
            return Ok(Vec::new());
        }
        return Err(error);
    }
    let mut sessions = Vec::new();
    let listing = String::from_utf8(output.stdout)
        .map_err(|_| protocol("tmux returned invalid UTF-8 in session listing"))?;
    for line in listing.lines() {
        let raw = parse_session_line(line)?;
        let name = display_name(socket, &raw.id)?;
        sessions.push(Session {
            id: raw.id,
            name,
            windows: raw.windows,
            attached_clients: raw.attached_clients,
        });
    }
    Ok(sessions)
}

fn validate_socket(path: &str) -> Result<(), TmuxError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            if !metadata.file_type().is_socket() {
                return Err(TmuxError {
                    code: "invalid_socket".into(),
                    message: format!("tmux socket path is not a Unix socket: {path}"),
                });
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TmuxError {
            code: "socket_error".into(),
            message: format!("cannot inspect tmux socket {path}: {error}"),
        }),
    }
}

fn display_name(socket: Option<&str>, id: &str) -> Result<String, TmuxError> {
    let mut command = Command::new("tmux");
    // Force UTF-8 even in a minimal noninteractive environment.
    command.args(["-u", "-N"]);
    if let Some(socket) = socket {
        command.args(["-S", socket]);
    }
    command
        .args(["display-message", "-p", "-t", id, "#{session_name}"])
        .env("LC_ALL", "C");
    let output = command.output().map_err(spawn_error)?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(TmuxError {
            code: "session_disappeared".into(),
            message: if message.is_empty() {
                format!("session {id} disappeared while listing")
            } else {
                format!("session {id} could not be queried: {message}")
            },
        });
    }
    let bytes = output.stdout;
    let name = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    String::from_utf8(name.to_vec()).map_err(|_| TmuxError {
        code: "protocol_error".into(),
        message: format!("tmux returned invalid UTF-8 for session {id}"),
    })
}

fn parse_session_line(line: &str) -> Result<RawSession, TmuxError> {
    let mut fields = line.split('|');
    let id = fields
        .next()
        .filter(|s| valid_id(s))
        .ok_or_else(|| protocol("invalid session id"))?;
    let windows = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| protocol("invalid window count"))?;
    let attached_clients = fields
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| protocol("invalid attached-client count"))?;
    if fields.next().is_some() {
        return Err(protocol("unexpected fields in session listing"));
    }
    Ok(RawSession {
        id: id.into(),
        windows,
        attached_clients,
    })
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix('$')
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}
fn protocol(message: &str) -> TmuxError {
    TmuxError {
        code: "protocol_error".into(),
        message: message.into(),
    }
}

fn spawn_error(error: std::io::Error) -> TmuxError {
    let code = if error.kind() == std::io::ErrorKind::NotFound {
        "tmux_not_found"
    } else {
        "tmux_spawn_error"
    };
    TmuxError {
        code: code.into(),
        message: format!("cannot execute tmux: {error}"),
    }
}

fn classify_error(message: &str, status: Option<i32>) -> TmuxError {
    let lower = message.to_ascii_lowercase();
    if lower == "no server running"
        || lower.starts_with("no server running ")
        || lower == "no sessions"
        || lower.starts_with("no sessions ")
    {
        TmuxError {
            code: "no_server".into(),
            message: message.into(),
        }
    } else if lower.starts_with("error connecting to ")
        && lower.ends_with(" (no such file or directory)")
    {
        TmuxError {
            code: "no_socket".into(),
            message: message.into(),
        }
    } else if lower.ends_with(" (permission denied)")
        || lower.ends_with(" (operation not permitted)")
    {
        TmuxError {
            code: "permission_denied".into(),
            message: message.into(),
        }
    } else {
        TmuxError {
            code: "tmux_error".into(),
            message: if message.is_empty() {
                format!(
                    "tmux exited unsuccessfully{}",
                    status.map(|s| format!(" ({s})")).unwrap_or_default()
                )
            } else {
                message.into()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_numeric_listing() {
        assert_eq!(parse_session_line("$3|2|1").unwrap().id, "$3");
    }
    #[test]
    fn rejects_bad_listing() {
        assert_eq!(
            parse_session_line("name|2|1").unwrap_err().code,
            "protocol_error"
        );
    }
    #[test]
    fn classifies_no_server() {
        assert_eq!(
            classify_error("no server running on /tmp", Some(1)).code,
            "no_server"
        );
    }
    #[test]
    fn classifies_only_native_missing_socket_error() {
        assert_eq!(
            classify_error(
                "error connecting to /tmp/x (No such file or directory)",
                Some(1)
            )
            .code,
            "no_socket"
        );
        assert_eq!(
            classify_error(
                "socket /tmp/error connecting to fake (No such file or directory) path",
                Some(1)
            )
            .code,
            "tmux_error"
        );
    }
    #[test]
    fn permission_errno_is_suffix_only() {
        assert_eq!(
            classify_error(
                "error connecting to /tmp/permission denied (No such file or directory)",
                Some(1)
            )
            .code,
            "no_socket"
        );
        assert_eq!(
            classify_error("error connecting to /tmp/x (Permission denied)", Some(1)).code,
            "permission_denied"
        );
    }
    #[test]
    fn preserves_trailing_carriage_return_in_name() {
        let bytes = b"a\r\n";
        let name = bytes.strip_suffix(b"\n").unwrap();
        assert_eq!(name, b"a\r");
    }
}
