use serde::Serialize;
use std::{
    env,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Debug, Serialize)]
struct HostStatus {
    schema_version: u32,
    hostname: String,
    argos_version: &'static str,
    state_root: String,
    runtime_root: Option<String>,
    capabilities: Capabilities,
}

#[derive(Debug, Serialize)]
struct Capabilities {
    kvm_device_exists: bool,
    kvm_accessible: bool,
    nix_available: bool,
    tmux_available: bool,
    ssh_available: bool,
    can_run_microvms: bool,
}

pub(crate) fn status(json: bool) -> ExitCode {
    let status = HostStatus {
        schema_version: 1,
        hostname: local_hostname().unwrap_or_else(|_| "unknown".into()),
        argos_version: env!("CARGO_PKG_VERSION"),
        state_root: state_root().display().to_string(),
        runtime_root: runtime_root().map(|path| path.display().to_string()),
        capabilities: capabilities(),
    };
    if json {
        println!("{}", serde_json::to_string(&status).unwrap());
    } else {
        print_human(&status);
    }
    ExitCode::SUCCESS
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

fn state_root() -> PathBuf {
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
}
