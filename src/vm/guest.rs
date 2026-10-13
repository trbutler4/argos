//! Guest SSH preparation and first-use trust. Interactive lifetime stays with
//! the origin-side connection owner, not this module.
use super::state::{load_record, state_root, valid_id};
use super::{
    GuestCommandOptions, VmError, VmRecord, invalid_args, shell_quote, ssh_port_for_id, state_error,
};
use std::{
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime},
};

const SSH_KEYSCAN_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn prepare_guest_command(options: GuestCommandOptions) -> Result<Command, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(VmError {
            code: "shell_error",
            message: "guest shell requires a terminal on stdin and stdout".into(),
        });
    }
    let root = state_root(options.state_dir)?;
    let record = load_record(&root.join("vms").join(format!("{}.json", options.id)))?;
    if record.status != "running" {
        return Err(VmError {
            code: "not_running",
            message: format!("VM is not running: {}", record.id),
        });
    }
    let host = record.ssh_host.as_deref().unwrap_or("127.0.0.1");
    let user = record.ssh_user.as_deref().unwrap_or("root");
    let port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let instance_dir = record
        .instance_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("instances").join(&record.id));
    let known_hosts = instance_dir.join("known_hosts");
    // Never replace recorded trust during reconnects.
    if !known_hosts.exists() {
        refresh_guest_known_hosts(&record, &instance_dir)?;
    }
    let mut command = crate::ssh::interactive_command();
    command
        .args(["-p", &port.to_string()])
        .args([
            "-o",
            &format!("UserKnownHostsFile={}", known_hosts.display()),
        ])
        .arg(format!("{user}@{host}"));
    crate::ssh::validate_command(&command).map_err(|error| VmError {
        code: error.code(),
        message: error.to_string(),
    })?;
    let workdir = record.guest_workdir.as_deref().unwrap_or("/workspace/repo");
    let workspace_prefix = format!(
        "cd {} 2>/dev/null || cd /workspace/repo 2>/dev/null || cd /workspace 2>/dev/null || cd",
        shell_quote(workdir)
    );
    if options.mode == crate::sessions::GuestMode::Tmux {
        command.arg(format!(
            "{workspace_prefix}; unset ARGOS_CONNECT_CLIENT; \
             cfg=/dev/null; if [ -f /etc/tmux.conf ]; then cfg=/etc/tmux.conf; fi; \
             tmux -u -f \"$cfg\" has-session -t '=main' 2>/dev/null || \
             tmux -u -f \"$cfg\" new-session -d -s main || \
             tmux -u -f \"$cfg\" has-session -t '=main' || exit 1; \
             ARGOS_CONNECT_CLIENT=1 exec tmux -u -N -f \"$cfg\" attach-session -E -t '=main'"
        ));
    } else {
        command.arg(format!(
            "{workspace_prefix}; exec ${{SHELL:-/run/current-system/sw/bin/sh}} -l"
        ));
    }
    Ok(command)
}

fn refresh_guest_known_hosts(record: &VmRecord, instance_dir: &Path) -> Result<(), VmError> {
    let host = record.ssh_host.as_deref().unwrap_or("127.0.0.1");
    let port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let known_hosts = instance_dir.join("known_hosts");
    let start = SystemTime::now();
    loop {
        match scan_guest_host_key(host, port) {
            Ok(keys) => {
                fs::create_dir_all(instance_dir).map_err(|error| {
                    state_error(format!(
                        "cannot create VM instance directory {}: {error}",
                        instance_dir.display()
                    ))
                })?;
                let temp = known_hosts.with_extension("known_hosts.tmp");
                fs::write(&temp, keys).map_err(|error| {
                    state_error(format!(
                        "cannot write temporary known_hosts {}: {error}",
                        temp.display()
                    ))
                })?;
                fs::rename(&temp, &known_hosts).map_err(|error| {
                    state_error(format!(
                        "cannot publish known_hosts {}: {error}",
                        known_hosts.display()
                    ))
                })?;
                return Ok(());
            }
            Err(error) => {
                if start.elapsed().unwrap_or_default() > SSH_KEYSCAN_TIMEOUT {
                    return Err(error);
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn scan_guest_host_key(host: &str, port: u16) -> Result<String, VmError> {
    let output = Command::new("ssh-keyscan")
        .args(ssh_keyscan_args(host, port))
        .output()
        .map_err(|error| VmError {
            code: "ssh_error",
            message: format!("cannot execute ssh-keyscan: {error}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let keys = stdout
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    if keys.is_empty() {
        return Err(VmError {
            code: "ssh_error",
            message: format!(
                "could not scan guest SSH host key on {host}:{port}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(format!("{keys}\n"))
}

fn ssh_keyscan_args(host: &str, port: u16) -> Vec<String> {
    vec![
        "-T".into(),
        "5".into(),
        "-t".into(),
        "ed25519".into(),
        "-p".into(),
        port.to_string(),
        host.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_only_guest_ed25519_host_key() {
        assert_eq!(
            ssh_keyscan_args("127.0.0.1", 32026),
            vec!["-T", "5", "-t", "ed25519", "-p", "32026", "127.0.0.1"]
        );
    }
}
