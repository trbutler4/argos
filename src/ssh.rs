//! Shared policy for interactive host and guest SSH transports.
use crate::process;
use std::{
    fmt,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) enum ConfigError {
    Process(process::RunError),
    Rejected(String),
    UnsupportedProxy,
}

impl ConfigError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Process(process::RunError::Cancelled) => "cancelled",
            Self::Process(process::RunError::Timeout) => "timeout",
            Self::UnsupportedProxy => "unsupported_proxy",
            _ => "ssh_config_error",
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Process(e) => write!(f, "cannot inspect SSH configuration: {e}"),
            Self::Rejected(message) => write!(f, "cannot inspect SSH configuration: {message}"),
            Self::UnsupportedProxy => f.write_str("Argos connections do not yet support ProxyCommand or ProxyJump; use direct SSH inventory aliases"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Process(e) => Some(e),
            _ => None,
        }
    }
}

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

pub(crate) fn validate_destination(alias: &str) -> Result<(), ConfigError> {
    validate_destination_cancellable(
        alias,
        Instant::now() + Duration::from_secs(3),
        &process::Cancellation::default(),
    )
}

pub(crate) fn validate_destination_cancellable(
    alias: &str,
    deadline: Instant,
    cancel: &process::Cancellation,
) -> Result<(), ConfigError> {
    let mut command = interactive_command();
    command.arg(alias);
    validate_command_cancellable(&command, deadline, cancel)
}

/// Inspect the exact destination/port/options before adding the remote command.
pub(crate) fn validate_command(command: &Command) -> Result<(), ConfigError> {
    validate_command_cancellable(
        command,
        Instant::now() + Duration::from_secs(3),
        &process::Cancellation::default(),
    )
}

fn validate_command_cancellable(
    command: &Command,
    deadline: Instant,
    cancel: &process::Cancellation,
) -> Result<(), ConfigError> {
    let mut inspect = Command::new(command.get_program());
    inspect
        .arg("-G")
        .args(command.get_args())
        .env("LC_ALL", "C");
    let output =
        process::run_cancellable(inspect, deadline, cancel).map_err(ConfigError::Process)?;
    if !output.status.success() {
        return Err(ConfigError::Rejected(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((key, value)) = line.split_once(' ')
            && matches!(key, "proxycommand" | "proxyjump")
            && value != "none"
        {
            return Err(ConfigError::UnsupportedProxy);
        }
    }
    Ok(())
}
