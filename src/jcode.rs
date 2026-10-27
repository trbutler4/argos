//! Passive guest Jcode discovery. Never prepares trust or starts a daemon.
use crate::{
    discovery::observed_now,
    process::{self, Cancellation},
    vm,
};
use serde::{Deserialize, Serialize};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) const SCOPE: &str = "client_connected_debug_sessions";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Session {
    pub(crate) session_id: String,
    pub(crate) friendly_name: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) is_processing: Option<bool>,
    pub(crate) working_dir: Option<String>,
    pub(crate) swarm_id: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) token_usage: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProbeError {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Guest {
    pub(crate) vm_id: String,
    pub(crate) status: String,
    pub(crate) scope: &'static str,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) sessions: Vec<Session>,
    pub(crate) error: Option<ProbeError>,
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    schema_version: u32,
    observed_at_unix_ms: u64,
    scope: &'static str,
    vms: Vec<Guest>,
}

pub(crate) fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn failure(code: &str, message: impl Into<String>) -> ProbeError {
    ProbeError {
        code: code.into(),
        message: sanitize(&message.into().chars().take(2048).collect::<String>()),
    }
}

// Like artifact retrieval, passive discovery ignores user SSH configuration and
// requires pre-existing per-guest trust. Interactive first-use keyscan is forbidden.
fn command(record: &vm::VmRecord, root: &Path) -> Result<Command, ProbeError> {
    let host = record
        .ssh_host
        .as_deref()
        .ok_or_else(|| failure("endpoint_missing", "recorded guest SSH host required"))?;
    if !host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback()) {
        return Err(failure(
            "invalid_endpoint",
            "guest SSH host must be IPv4 loopback",
        ));
    }
    let port = record
        .ssh_port
        .filter(|p| *p != 0)
        .ok_or_else(|| failure("endpoint_missing", "recorded guest SSH port required"))?;
    let user = record
        .ssh_user
        .as_deref()
        .ok_or_else(|| failure("endpoint_missing", "recorded guest SSH user required"))?;
    if user.is_empty()
        || user.starts_with('-')
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(failure("invalid_endpoint", "invalid guest SSH user"));
    }
    let trust = record
        .instance_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("instances").join(&record.id))
        .join("known_hosts");
    if !std::fs::symlink_metadata(&trust).is_ok_and(|m| m.is_file() && m.len() > 0) {
        return Err(failure(
            "host_key_untrusted",
            "existing regular guest known_hosts required; connect explicitly to establish trust",
        ));
    }
    if !trust.is_absolute()
        || trust.to_str().is_none_or(|s| {
            s.chars()
                .any(|c| c.is_control() || matches!(c, '"' | '\\' | '%'))
        })
    {
        return Err(failure(
            "host_key_untrusted",
            "unsupported known_hosts path",
        ));
    }
    let mut command = Command::new("ssh");
    command
        .env("LC_ALL", "C")
        .args(["-F", "/dev/null", "-4", "-T", "-n", "-p", &port.to_string()]);
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=/dev/null",
        "UpdateHostKeys=no",
        "VerifyHostKeyDNS=no",
        "ProxyCommand=none",
        "ProxyJump=none",
        "ConnectTimeout=3",
        "ConnectionAttempts=1",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "PermitLocalCommand=no",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
    ] {
        command.args(["-o", option]);
    }
    command
        .arg("-o")
        .arg(format!("UserKnownHostsFile=\"{}\"", trust.display()))
        .arg(format!("{user}@{host}"));
    // Default debug resolution selects the main socket itself. Never pass a
    // '-debug' socket: debug derives that suffix from the main socket.
    command
        .arg("command -v jcode >/dev/null 2>&1 || exit 127; exec jcode --no-update debug sessions");
    Ok(command)
}

fn sessions(
    record: &vm::VmRecord,
    root: &Path,
    timeout: Duration,
    cancel: &Cancellation,
) -> Result<Vec<Session>, ProbeError> {
    let output = process::run_cancellable(command(record, root)?, Instant::now() + timeout, cancel)
        .map_err(|e| {
            let code = match e {
                process::RunError::Timeout => "timeout",
                process::RunError::Cancelled => "cancelled",
                process::RunError::OutputLimit => "output_limit",
                _ => "probe_failed",
            };
            failure(code, e.to_string())
        })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let lower = message.to_ascii_lowercase();
        let code = if output.status.code() == Some(127) {
            "jcode_missing"
        } else if lower.contains("host key verification failed")
            || lower.contains("remote host identification has changed")
        {
            "host_key_untrusted"
        } else if lower.contains("permission denied") {
            "authentication_failed"
        } else if output.status.code() == Some(255) {
            "unreachable"
        } else if lower.contains("disabled") {
            "debug_disabled"
        } else {
            "debug_unavailable"
        };
        return Err(failure(
            code,
            if message.trim().is_empty() {
                format!("guest debug probe exited {}", output.status)
            } else {
                message.trim().to_string()
            },
        ));
    }
    let mut sessions: Vec<Session> = serde_json::from_slice(&output.stdout).map_err(|e| {
        failure(
            "invalid_output",
            format!("invalid Jcode debug sessions JSON: {e}"),
        )
    })?;
    for s in &mut sessions {
        s.session_id = sanitize(&s.session_id);
        for field in [
            &mut s.friendly_name,
            &mut s.provider,
            &mut s.model,
            &mut s.working_dir,
            &mut s.swarm_id,
            &mut s.status,
            &mut s.detail,
        ]
        .into_iter()
        .flatten()
        {
            *field = sanitize(field);
        }
    }
    Ok(sessions)
}

pub(crate) fn probe(
    record: &vm::VmRecord,
    root: &Path,
    timeout: Duration,
    cancel: &Cancellation,
) -> Guest {
    let result = if record.status == "running" {
        sessions(record, root, timeout, cancel)
    } else {
        Err(failure("stopped", "VM is not running; not probed"))
    };
    let (status, sessions, error) = match result {
        Ok(s) => ("ok".into(), s, None),
        Err(e) => (
            if e.code == "stopped" {
                "stopped"
            } else {
                "error"
            }
            .into(),
            Vec::new(),
            Some(e),
        ),
    };
    Guest {
        vm_id: record.id.clone(),
        status,
        scope: SCOPE,
        observed_at_unix_ms: observed_now(),
        sessions,
        error,
    }
}

pub(crate) fn probe_guests(
    snapshot: &vm::VmSnapshot,
    parallel: usize,
    timeout: Duration,
    cancel: &Cancellation,
    publish: impl Fn(Guest) + Sync,
) {
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..parallel.clamp(1, 16).min(snapshot.vms.len()) {
            let (next, publish) = (&next, &publish);
            scope.spawn(move || {
                while !cancel.is_cancelled() {
                    let Some(record) = snapshot.vms.get(next.fetch_add(1, Ordering::Relaxed))
                    else {
                        break;
                    };
                    let guest = probe(record, Path::new(&snapshot.state_root), timeout, cancel);
                    if !cancel.is_cancelled() {
                        publish(guest);
                    }
                }
            });
        }
    });
}

pub(crate) fn list(state_dir: Option<PathBuf>, json: bool) -> ExitCode {
    let snapshot = match vm::snapshot(state_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(if e.code == "invalid_args" { 2 } else { 1 });
        }
    };
    let (tx, rx) = mpsc::channel();
    probe_guests(
        &snapshot,
        4,
        Duration::from_secs(3),
        &Cancellation::default(),
        |g| {
            let _ = tx.send(g);
        },
    );
    drop(tx);
    let mut vms: Vec<_> = rx.into_iter().collect();
    vms.sort_by(|a, b| a.vm_id.cmp(&b.vm_id));
    let failed = vms.iter().any(|g| g.status == "error");
    let snapshot = Snapshot {
        schema_version: 1,
        observed_at_unix_ms: observed_now(),
        scope: SCOPE,
        vms,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string(&snapshot).expect("serialize agents")
        );
    } else {
        println!("Jcode client-connected debug sessions (not persisted/headless inventory)");
        for guest in snapshot.vms {
            if let Some(error) = guest.error {
                println!(
                    "{}: {}: {}",
                    sanitize(&guest.vm_id),
                    error.code,
                    error.message
                );
            } else {
                println!(
                    "{}: {} client-connected sessions",
                    sanitize(&guest.vm_id),
                    guest.sessions.len()
                );
                for session in guest.sessions {
                    println!(
                        "  {}  {}  {}",
                        session
                            .friendly_name
                            .as_deref()
                            .unwrap_or(&session.session_id),
                        session.status.as_deref().unwrap_or("unknown"),
                        session.model.as_deref().unwrap_or("unknown")
                    );
                }
            }
        }
    }
    ExitCode::from(u8::from(failed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nullable_busy_fields_and_additive_fields() {
        let s: Vec<Session> = serde_json::from_str(r#"[{"session_id":"s1","friendly_name":null,"provider":null,"model":null,"is_processing":null,"working_dir":null,"swarm_id":null,"status":"busy","detail":null,"token_usage":null,"future":true}]"#).unwrap();
        assert_eq!(s[0].status.as_deref(), Some("busy"));
        assert!(s[0].is_processing.is_none());
        assert!(serde_json::from_str::<Vec<Session>>(r#"[{"status":"idle"}]"#).is_err());
    }
    #[test]
    fn remote_labels_cannot_control_terminal() {
        assert_eq!(sanitize("hi\n\u{1b}[2J\u{202e}"), "hi  [2J ");
    }
}
