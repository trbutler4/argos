//! Connection policy: select, prepare, supervise, then interpret the result.
use crate::sessions::{BrowseOptions, SessionTarget};
use crate::terminal::{ProcessOutcome, TerminalOwner};
use crate::{config, sessions, snapshot_request, tui};
use std::{io::IsTerminal, process::ExitCode};

const PICKER_STATUS: i32 = 78;

/// Text parsing belongs at CLI entry, not inside the connection loop.
pub(crate) fn run(target: Option<String>, options: BrowseOptions) -> ExitCode {
    let target = match target
        .as_deref()
        .map(|target| SessionTarget::parse(target, &options))
        .transpose()
    {
        Ok(target) => target,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    run_target(target, options)
}

/// VM commands already know their target and need no text round trip.
pub(crate) fn run_target(target: Option<SessionTarget>, mut options: BrowseOptions) -> ExitCode {
    if let Err(error) = configure(target.as_ref(), &mut options) {
        eprintln!("error: {error}");
        return ExitCode::from(2);
    }
    match run_inner(target, &options) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn configure(target: Option<&SessionTarget>, options: &mut BrowseOptions) -> Result<(), String> {
    if let Some(target) = target {
        target.validate()?;
    }
    // Normalize browser defaults only. A VM target has no host-local flag to
    // repair, and an explicit host target never inherits picker-only options.
    if options.config_path.is_none() && !options.local && options.socket.is_none() {
        if let Some(path) = config::default_path()
            && config::load_implicit(&path)?.is_some()
        {
            options.config_path = Some(path);
        } else if options.host.is_none() && !target.is_some_and(SessionTarget::requires_inventory) {
            options.local = true;
        } else {
            return Err("--host and host:<host>:<session> require a configured inventory".into());
        }
    }
    snapshot_request(
        options.socket.clone(),
        options.config_path.clone(),
        options.host.clone(),
        options.local,
    )
    .map_err(|(message, _)| message)?;
    Ok(())
}

pub(crate) fn check_origin() -> Result<(), String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("Argos attachment requires a terminal on stdin and stdout".into());
    }
    for name in [
        "TMUX",
        "TMUX_PANE",
        "SSH_CONNECTION",
        "SSH_CLIENT",
        "SSH_TTY",
    ] {
        if std::env::var_os(name).is_some_and(|value| !value.is_empty()) {
            return Err(format!(
                "Argos must start in the original terminal outside tmux and SSH ({name} is set); use the managed return-to-picker binding to switch connections"
            ));
        }
    }
    Ok(())
}

fn run_inner(mut next: Option<SessionTarget>, options: &BrowseOptions) -> Result<u8, String> {
    check_origin()?;
    let mut terminal = TerminalOwner::new().map_err(|e| e.to_string())?;
    let mut previous = None;
    let mut error = None;
    loop {
        if let Some(code) = terminal.poll_idle().map_err(|e| e.to_string())? {
            return Ok(code);
        }
        let selected = match next.take() {
            Some(selected) => selected,
            None => {
                let request = snapshot_request(
                    options.socket.clone(),
                    options.config_path.clone(),
                    options.host.clone(),
                    options.local,
                )
                .map_err(|(message, _)| message)?;
                // Transport policy stays here, never in terminal rendering.
                for (_, alias, _) in &request.machines {
                    if let Some(alias) = alias {
                        crate::ssh::validate_destination(alias)?;
                    }
                }
                match tui::pick_session(
                    request,
                    options,
                    previous.as_ref(),
                    error.as_deref(),
                    &mut terminal,
                )
                .map_err(|e| e.to_string())?
                {
                    tui::PickResult::Select(target) => target,
                    tui::PickResult::Retry => match previous.clone() {
                        Some(previous) => previous,
                        None => return Ok(0),
                    },
                    tui::PickResult::Quit(code) => return Ok(code),
                }
            }
        };
        let prepared = match sessions::prepare(&selected, options) {
            Ok(prepared) => prepared,
            Err(message) => {
                previous = Some(selected);
                error = Some(message);
                continue;
            }
        };
        previous = Some(prepared.reconnect_target);
        if let Some(code) = terminal.poll_idle().map_err(|e| e.to_string())? {
            return Ok(code);
        }
        let status = terminal
            .attach(prepared.command)
            .map_err(|e| e.to_string())?;
        match interpret_outcome(&selected, status) {
            ConnectionOutcome::Exit(code) => return Ok(code),
            ConnectionOutcome::Picker => error = None,
            ConnectionOutcome::Recover(message) => error = Some(message),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ConnectionOutcome {
    Exit(u8),
    Picker,
    Recover(String),
}

fn interpret_outcome(target: &SessionTarget, status: ProcessOutcome) -> ConnectionOutcome {
    match status {
        ProcessOutcome::Exited(0) => ConnectionOutcome::Exit(0),
        ProcessOutcome::Exited(PICKER_STATUS) if target.uses_tmux() => ConnectionOutcome::Picker,
        ProcessOutcome::Exited(PICKER_STATUS) => ConnectionOutcome::Exit(PICKER_STATUS as u8),
        ProcessOutcome::Exited(code) => {
            ConnectionOutcome::Recover(format!("Attachment exited with status {code}."))
        }
        ProcessOutcome::Signaled(signal) => {
            ConnectionOutcome::Recover(format!("Attachment terminated by {signal}."))
        }
        ProcessOutcome::SpawnFailed(error) => {
            ConnectionOutcome::Recover(format!("Cannot start attachment: {error}"))
        }
        ProcessOutcome::Interrupted(code) => ConnectionOutcome::Exit(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::GuestMode;
    use nix::sys::signal::Signal;

    #[test]
    fn picker_status_is_tmux_protocol_not_shell_exit() {
        let tmux = SessionTarget::Vm {
            id: "test".into(),
            mode: GuestMode::Tmux,
        };
        let shell = SessionTarget::Vm {
            id: "test".into(),
            mode: GuestMode::Shell,
        };
        assert_eq!(
            interpret_outcome(&tmux, ProcessOutcome::Exited(78)),
            ConnectionOutcome::Picker
        );
        assert_eq!(
            interpret_outcome(&shell, ProcessOutcome::Exited(78)),
            ConnectionOutcome::Exit(78)
        );
        assert_eq!(
            interpret_outcome(&tmux, ProcessOutcome::Exited(0)),
            ConnectionOutcome::Exit(0)
        );
        assert_eq!(
            interpret_outcome(&tmux, ProcessOutcome::Interrupted(143)),
            ConnectionOutcome::Exit(143)
        );
        for code in [1, 2, 77, 79, 255] {
            assert!(matches!(
                interpret_outcome(&tmux, ProcessOutcome::Exited(code)),
                ConnectionOutcome::Recover(_)
            ));
        }
        assert!(matches!(
            interpret_outcome(&tmux, ProcessOutcome::Signaled(Signal::SIGTERM)),
            ConnectionOutcome::Recover(_)
        ));
    }
}
