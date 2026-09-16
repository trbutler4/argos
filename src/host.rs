use crate::{config, tmux::process};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct HostStatus {
    pub(crate) schema_version: u32,
    pub(crate) hostname: String,
    pub(crate) argos_version: String,
    pub(crate) state_root: String,
    pub(crate) runtime_root: Option<String>,
    pub(crate) capabilities: Capabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Capabilities {
    pub(crate) kvm_device_exists: bool,
    pub(crate) kvm_accessible: bool,
    pub(crate) nix_available: bool,
    pub(crate) tmux_available: bool,
    pub(crate) ssh_available: bool,
    pub(crate) can_run_microvms: bool,
}

pub(crate) fn status(json: bool) -> ExitCode {
    let status = current_status();
    if json {
        println!("{}", serde_json::to_string(&status).unwrap());
    } else {
        print_human(&status);
    }
    ExitCode::SUCCESS
}

pub(crate) fn current_status() -> HostStatus {
    HostStatus {
        schema_version: 1,
        hostname: local_hostname().unwrap_or_else(|_| "unknown".into()),
        argos_version: env!("CARGO_PKG_VERSION").to_owned(),
        state_root: state_root().display().to_string(),
        runtime_root: runtime_root().map(|path| path.display().to_string()),
        capabilities: capabilities(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostsStatusSnapshot {
    schema_version: u32,
    observed_at_unix_ms: u64,
    hosts: Vec<HostProbe>,
}

#[derive(Clone, Debug, Serialize)]
struct HostProbe {
    id: String,
    status: &'static str,
    helper: Option<HostStatus>,
    error: Option<HostStatusError>,
}

#[derive(Clone, Debug, Serialize)]
struct HostStatusError {
    code: String,
    message: String,
}

struct HostsStatusRequest {
    machines: Vec<(String, Option<String>)>,
    timeout_secs: u64,
    parallel: usize,
}

pub(crate) fn hosts_status(
    json: bool,
    config_path: Option<PathBuf>,
    host_filter: Option<String>,
) -> ExitCode {
    let request = match hosts_status_request(config_path, host_filter) {
        Ok(request) => request,
        Err((message, code)) => {
            eprintln!("error: {message}");
            return ExitCode::from(code);
        }
    };
    let snapshot = discover_hosts_status(&request);
    if json {
        println!("{}", serde_json::to_string(&snapshot).unwrap());
    } else {
        print_hosts_status(&snapshot);
    }
    if snapshot.hosts.iter().any(|host| host.error.is_some()) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn hosts_status_request(
    config_path: Option<PathBuf>,
    host_filter: Option<String>,
) -> Result<HostsStatusRequest, (String, u8)> {
    let loaded = match config_path {
        Some(path) => config::load(&path).map(Some),
        None => match config::default_path() {
            Some(path) => config::load_implicit(&path),
            None => Ok(None),
        },
    };
    let config = match loaded {
        Ok(Some(config)) => config,
        Ok(None) if host_filter.is_none() => {
            return Ok(HostsStatusRequest {
                machines: vec![(local_hostname().unwrap_or_else(|_| "local".into()), None)],
                timeout_secs: 3,
                parallel: 1,
            });
        }
        Ok(None) => return Err(("--host requires a configured inventory".into(), 2)),
        Err(error) => return Err((error, 2)),
    };
    if let Some(id) = &host_filter
        && !config.machines.contains_key(id)
    {
        return Err((format!("unknown host id: {id}"), 2));
    }
    let machines = config
        .machines
        .iter()
        .filter(|(id, _)| host_filter.as_ref().is_none_or(|filter| filter == *id))
        .map(|(id, machine)| (id.clone(), machine.ssh_alias.clone()))
        .collect();
    Ok(HostsStatusRequest {
        machines,
        timeout_secs: config.client.connect_timeout_seconds,
        parallel: config.client.max_parallel_probes,
    })
}

fn discover_hosts_status(request: &HostsStatusRequest) -> HostsStatusSnapshot {
    let results = Arc::new(Mutex::new(
        (0..request.machines.len())
            .map(|_| None)
            .collect::<Vec<Option<HostProbe>>>(),
    ));
    let next = Arc::new(Mutex::new(0usize));
    std::thread::scope(|scope| {
        for _ in 0..request.parallel.min(request.machines.len().max(1)) {
            let results = Arc::clone(&results);
            let next = Arc::clone(&next);
            let machines = &request.machines;
            let timeout = Duration::from_secs(request.timeout_secs);
            scope.spawn(move || {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        if *n >= machines.len() {
                            break;
                        }
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let (id, ssh_alias) = &machines[i];
                    let helper = match ssh_alias {
                        Some(alias) => remote_status(alias, timeout),
                        None => Ok(current_status()),
                    };
                    results.lock().unwrap()[i] = Some(match helper {
                        Ok(status) => HostProbe {
                            id: id.clone(),
                            status: "ok",
                            helper: Some(status),
                            error: None,
                        },
                        Err(error) => HostProbe {
                            id: id.clone(),
                            status: "error",
                            helper: None,
                            error: Some(error),
                        },
                    });
                }
            });
        }
    });
    HostsStatusSnapshot {
        schema_version: 1,
        observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        hosts: results
            .lock()
            .unwrap()
            .iter_mut()
            .map(|host| host.take().unwrap())
            .collect(),
    }
}

fn remote_status(alias: &str, timeout: Duration) -> Result<HostStatus, HostStatusError> {
    let deadline = Instant::now() + timeout;
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
        "LC_ALL=C argos host status --json",
    ]);
    let output = process::run(command, deadline).map_err(run_error)?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(classify_remote_status_error(&message, output.status.code()));
    }
    let text = String::from_utf8(output.stdout).map_err(|_| HostStatusError {
        code: "protocol_error".into(),
        message: "host status helper returned invalid UTF-8".into(),
    })?;
    let status =
        serde_json::from_str::<HostStatus>(text.trim()).map_err(|error| HostStatusError {
            code: "protocol_error".into(),
            message: format!("host status helper returned invalid JSON: {error}"),
        })?;
    if status.schema_version != 1 {
        return Err(HostStatusError {
            code: "protocol_error".into(),
            message: format!(
                "unsupported host status schema_version: {}",
                status.schema_version
            ),
        });
    }
    Ok(status)
}

fn run_error(error: process::RunError) -> HostStatusError {
    match error {
        process::RunError::Spawn(e) => HostStatusError {
            code: if e.kind() == std::io::ErrorKind::NotFound {
                "ssh_not_found".into()
            } else {
                "ssh_spawn_error".into()
            },
            message: format!("cannot execute ssh: {e}"),
        },
        process::RunError::Timeout => HostStatusError {
            code: "timeout".into(),
            message: "host status check timed out".into(),
        },
        process::RunError::OutputLimit => HostStatusError {
            code: "output_limit".into(),
            message: "host status output exceeded limit".into(),
        },
        process::RunError::Wait(e) => HostStatusError {
            code: "process_error".into(),
            message: format!("cannot wait for ssh: {e}"),
        },
    }
}

fn classify_remote_status_error(message: &str, status: Option<i32>) -> HostStatusError {
    let lower = message.to_ascii_lowercase();
    let code = if status == Some(127)
        && (lower.contains("argos: not found") || lower.contains("argos: command not found"))
    {
        "argos_not_found"
    } else if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
    {
        "host_key_error"
    } else if lower.contains("permission denied") || lower.contains("authentication failed") {
        "authentication_failed"
    } else if lower.contains("connection timed out") || lower.contains("connecttimeout") {
        "timeout"
    } else if lower.contains("could not resolve hostname")
        || lower.contains("no route")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
    {
        "unreachable"
    } else {
        "ssh_error"
    };
    HostStatusError {
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

fn print_hosts_status(snapshot: &HostsStatusSnapshot) {
    for host in &snapshot.hosts {
        println!(
            "{} ({})",
            serde_json::to_string(&host.id).unwrap(),
            host.status
        );
        if let Some(helper) = &host.helper {
            println!(
                "  helper: argos {} on {}",
                helper.argos_version, helper.hostname
            );
            println!("  state: {}", helper.state_root);
            match &helper.runtime_root {
                Some(path) => println!("  runtime: {path}"),
                None => println!("  runtime: unavailable (XDG_RUNTIME_DIR is not set)"),
            }
            println!(
                "  microvms: {}",
                if helper.capabilities.can_run_microvms {
                    "available"
                } else {
                    "unavailable"
                }
            );
            println!(
                "  tools: nix={} tmux={} ssh={} kvm={} accessible={}",
                yes(helper.capabilities.nix_available),
                yes(helper.capabilities.tmux_available),
                yes(helper.capabilities.ssh_available),
                yes(helper.capabilities.kvm_device_exists),
                yes(helper.capabilities.kvm_accessible),
            );
        }
        if let Some(error) = &host.error {
            println!(
                "  error {}: {}",
                error.code,
                serde_json::to_string(&error.message).unwrap()
            );
        }
    }
}

fn print_human(status: &HostStatus) {
    println!("{}", status.hostname);
    println!("  argos: {}", status.argos_version);
    println!("  state: {}", status.state_root);
    match &status.runtime_root {
        Some(path) => println!("  runtime: {path}"),
        None => println!("  runtime: unavailable (XDG_RUNTIME_DIR is not set)"),
    }
    println!(
        "  microvms: {}",
        if status.capabilities.can_run_microvms {
            "available"
        } else {
            "unavailable"
        }
    );
    println!(
        "  tools: nix={} tmux={} ssh={} kvm={} accessible={}",
        yes(status.capabilities.nix_available),
        yes(status.capabilities.tmux_available),
        yes(status.capabilities.ssh_available),
        yes(status.capabilities.kvm_device_exists),
        yes(status.capabilities.kvm_accessible),
    );
}

fn yes(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn capabilities() -> Capabilities {
    let kvm_device_exists = Path::new("/dev/kvm").exists();
    let kvm_accessible = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok();
    let nix_available = command_available("nix");
    let tmux_available = command_available("tmux");
    let ssh_available = command_available("ssh");
    Capabilities {
        kvm_device_exists,
        kvm_accessible,
        nix_available,
        tmux_available,
        ssh_available,
        can_run_microvms: kvm_accessible && nix_available && tmux_available && ssh_available,
    }
}

pub(crate) fn state_root() -> PathBuf {
    if let Some(path) = absolute_env_path("ARGOS_STATE_DIR") {
        return path;
    }
    if let Some(base) = absolute_env_path("XDG_STATE_HOME") {
        return base.join("argos");
    }
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/state/argos")
}

fn runtime_root() -> Option<PathBuf> {
    if let Some(path) = absolute_env_path("ARGOS_RUNTIME_DIR") {
        return Some(path);
    }
    absolute_env_path("XDG_RUNTIME_DIR").map(|base| base.join("argos"))
}

fn absolute_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn command_available(name: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| is_executable(&dir.join(name)))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn local_hostname() -> Result<String, std::io::Error> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_paths_must_be_absolute() {
        assert!(PathBuf::from("/tmp/argos").is_absolute());
        assert!(!PathBuf::from("relative").is_absolute());
    }

    #[test]
    fn yes_formats_booleans() {
        assert_eq!(yes(true), "yes");
        assert_eq!(yes(false), "no");
    }

    #[test]
    fn classifies_remote_status_failures() {
        assert_eq!(
            classify_remote_status_error("argos: command not found", Some(127)).code,
            "argos_not_found"
        );
        assert_eq!(
            classify_remote_status_error("Host key verification failed.", Some(255)).code,
            "host_key_error"
        );
        assert_eq!(
            classify_remote_status_error("Permission denied (publickey).", Some(255)).code,
            "authentication_failed"
        );
        assert_eq!(
            classify_remote_status_error(
                "ssh: connect to host h port 22: Connection refused",
                Some(255)
            )
            .code,
            "unreachable"
        );
    }
}
