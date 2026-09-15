#[path = "process.rs"]
pub(crate) mod process;

use serde::Serialize;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
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

pub fn discover(
    socket: Option<&str>,
    ssh_alias: Option<&str>,
    timeout: Duration,
) -> Result<Vec<Session>, TmuxError> {
    if ssh_alias.is_none()
        && let Some(path) = socket
    {
        validate_socket(path)?;
    }
    let deadline = Instant::now() + timeout;
    let output = if let Some(alias) = ssh_alias {
        let mut command = Command::new("ssh");
        command.env("LC_ALL", "C").args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("ConnectTimeout={}", timeout.as_secs().max(1)),
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "PermitLocalCommand=no",
            alias,
        ]);
        let mut remote = Vec::new();
        if let Some(path) = socket {
            remote.push("-S".into());
            remote.push(path.to_owned());
        }
        remote.extend([
            "-u".into(),
            "-N".into(),
            "list-sessions".into(),
            "-F".into(),
            "#{session_id}|#{session_windows}|#{session_attached}|#{session_name}".into(),
        ]);
        let script = if let Some(path) = socket {
            format!(
                "if [ -e {} ] && [ ! -S {} ]; then echo 'argos: invalid_socket' >&2; exit 125; fi; LC_ALL=C tmux {}",
                shell_quote(path),
                shell_quote(path),
                remote
                    .iter()
                    .map(|s| shell_quote(s))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        } else {
            format!(
                "LC_ALL=C tmux {}",
                remote
                    .iter()
                    .map(|s| shell_quote(s))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        command.arg(script);
        process::run(command, deadline).map_err(|e| run_error(e, "ssh"))?
    } else {
        let mut command = Command::new("tmux");
        command.args(["-u", "-N"]);
        if let Some(path) = socket {
            command.args(["-S", path]);
        }
        command
            .args([
                "list-sessions",
                "-F",
                "#{session_id}|#{session_windows}|#{session_attached}|#{session_name}",
            ])
            .env("LC_ALL", "C");
        process::run(command, deadline).map_err(|e| run_error(e, "tmux"))?
    };
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let error = if ssh_alias.is_some() && output.status.code() == Some(255) {
            classify_ssh(&message, output.status.code())
        } else {
            classify_tmux(&message, output.status.code())
        };
        if error.code == "no_server" || error.code == "no_socket" {
            return Ok(Vec::new());
        }
        return Err(error);
    }
    let listing =
        String::from_utf8(output.stdout).map_err(|_| protocol("tmux returned invalid UTF-8"))?;
    listing.lines().map(parse_session_line).collect()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn validate_socket(path: &str) -> Result<(), TmuxError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            #[cfg(unix)]
            if !m.file_type().is_socket() {
                return Err(TmuxError {
                    code: "invalid_socket".into(),
                    message: format!("tmux socket path is not a Unix socket: {path}"),
                });
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(TmuxError {
            code: "socket_error".into(),
            message: format!("cannot inspect tmux socket {path}: {e}"),
        }),
    }
}
fn parse_session_line(line: &str) -> Result<Session, TmuxError> {
    let mut f = line.splitn(4, '|');
    let id = f
        .next()
        .filter(|x| {
            x.strip_prefix('$')
                .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
        })
        .ok_or_else(|| protocol("invalid session id"))?;
    let windows = f
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| protocol("invalid window count"))?;
    let attached_clients = f
        .next()
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| protocol("invalid attached-client count"))?;
    let name = f.next().ok_or_else(|| protocol("missing session name"))?;
    Ok(Session {
        id: id.into(),
        name: name.into(),
        windows,
        attached_clients,
    })
}
fn protocol(message: &str) -> TmuxError {
    TmuxError {
        code: "protocol_error".into(),
        message: message.into(),
    }
}
fn classify_tmux(message: &str, status: Option<i32>) -> TmuxError {
    let l = message.to_ascii_lowercase();
    let code = if status == Some(125) && l.contains("argos: invalid_socket") {
        "invalid_socket"
    } else if status == Some(127) {
        "tmux_not_found"
    } else if l == "no server running" || l.starts_with("no server running ") || l == "no sessions"
    {
        "no_server"
    } else if l.starts_with("error connecting to ") && l.ends_with(" (no such file or directory)") {
        "no_socket"
    } else if l.ends_with(" (permission denied)") || l.ends_with(" (operation not permitted)") {
        "permission_denied"
    } else {
        "tmux_error"
    };
    TmuxError {
        code: code.into(),
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
fn classify_ssh(message: &str, status: Option<i32>) -> TmuxError {
    let l = message.to_ascii_lowercase();
    let code = if status == Some(127)
        && (l.contains("tmux: not found") || l.contains("tmux: command not found"))
    {
        "tmux_not_found"
    } else if l.contains("host key verification failed")
        || l.contains("remote host identification has changed")
    {
        "host_key_error"
    } else if l.contains("permission denied") || l.contains("authentication failed") {
        "authentication_failed"
    } else if l.contains("connection timed out") || l.contains("connecttimeout") {
        "timeout"
    } else if l.contains("could not resolve hostname")
        || l.contains("no route")
        || l.contains("connection refused")
        || l.contains("network is unreachable")
    {
        "unreachable"
    } else {
        "ssh_error"
    };
    TmuxError {
        code: code.into(),
        message: if message.is_empty() {
            format!(
                "ssh exited unsuccessfully{}",
                status.map(|s| format!(" ({s})")).unwrap_or_default()
            )
        } else {
            message.into()
        },
    }
}
fn run_error(error: process::RunError, tool: &str) -> TmuxError {
    match error {
        process::RunError::Spawn(e) => TmuxError {
            code: if e.kind() == std::io::ErrorKind::NotFound {
                format!("{tool}_not_found")
            } else {
                format!("{tool}_spawn_error")
            },
            message: format!("cannot execute {tool}: {e}"),
        },
        process::RunError::Timeout => TmuxError {
            code: "timeout".into(),
            message: format!("{tool} discovery timed out"),
        },
        process::RunError::OutputLimit => TmuxError {
            code: "output_limit".into(),
            message: format!("{tool} output exceeded limit"),
        },
        process::RunError::Wait(e) => TmuxError {
            code: "process_error".into(),
            message: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_errors_are_distinct() {
        assert_eq!(
            classify_tmux("zsh: command not found: tmux", Some(127)).code,
            "tmux_not_found"
        );
        assert_eq!(
            classify_tmux("no server running on /example/socket", Some(1)).code,
            "no_server"
        );
        assert_eq!(
            classify_ssh("Host key verification failed.", Some(255)).code,
            "host_key_error"
        );
        assert_eq!(
            classify_ssh("Permission denied (publickey).", Some(255)).code,
            "authentication_failed"
        );
        assert_eq!(
            classify_ssh(
                "connect to host offending.example: Connection refused",
                Some(255)
            )
            .code,
            "unreachable"
        );
    }
    #[test]
    fn quote_injection() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
    #[test]
    fn parse_pipe_name() {
        let s = parse_session_line("$1|2|0|a|b 💡").unwrap();
        assert_eq!(s.name, "a|b 💡");
    }
    #[test]
    fn reject_bad() {
        assert_eq!(
            parse_session_line("x|1|0|n").unwrap_err().code,
            "protocol_error"
        );
    }
}
