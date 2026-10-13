//! Shared policy for interactive host and guest SSH transports.
use crate::process;
use std::{
    process::Command,
    time::{Duration, Instant},
};

pub(crate) fn interactive_command() -> Command {
    let mut command = Command::new("ssh");
    command.arg("-tt").env("LC_ALL", "C");
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "PermitLocalCommand=no",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
        "ConnectTimeout=3",
        "ConnectionAttempts=1",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=2",
        "EscapeChar=none",
        "ForkAfterAuthentication=no",
        "StdinNull=no",
    ] {
        command.args(["-o", option]);
    }
    command
}

pub(crate) fn validate_destination(alias: &str) -> Result<(), String> {
    let mut command = interactive_command();
    command.arg(alias);
    validate_command(&command)
}

/// Inspect the exact destination/port/options before adding the remote command.
pub(crate) fn validate_command(command: &Command) -> Result<(), String> {
    let mut inspect = Command::new(command.get_program());
    inspect
        .arg("-G")
        .args(command.get_args())
        .env("LC_ALL", "C");
    let output = process::run(inspect, Instant::now() + Duration::from_secs(3))
        .map_err(|_| "cannot inspect SSH configuration".to_owned())?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect SSH configuration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((key, value)) = line.split_once(' ')
            && matches!(key, "proxycommand" | "proxyjump")
            && value != "none"
        {
            return Err("Argos connections do not yet support ProxyCommand or ProxyJump; use direct SSH inventory aliases".into());
        }
    }
    Ok(())
}
