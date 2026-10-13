//! Origin-side ownership of a single native interactive attachment.
use crate::{config, sessions, snapshot_request, tui};
use nix::{
    sys::{
        signal::{SigSet, SigmaskHow, Signal, kill, killpg, pthread_sigmask},
        termios::{self, SetArg, Termios},
        wait::{Id, WaitPidFlag, WaitStatus, waitid},
    },
    unistd::{Pid, getpgrp, tcgetpgrp, tcsetpgrp},
};
use signal_hook::{consts::signal::*, iterator::Signals};
use std::{
    io::{self, IsTerminal},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitCode},
    thread,
    time::{Duration, Instant},
};

pub(crate) const PICKER_STATUS: i32 = 78;

fn foreground(group: Pid) -> io::Result<()> {
    // A background/orphaned origin group must block TTOU while reclaiming its
    // terminal. Catching TTOU alone does not satisfy the kernel's job-control check.
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut old = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut old))?;
    let result = tcsetpgrp(io::stdin(), group);
    pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&old), None)?;
    result.map_err(io::Error::from)
}

pub(crate) fn run(target: Option<String>, mut options: sessions::AttachOptions) -> ExitCode {
    if let Err(error) = configure(target.as_deref(), &mut options) {
        eprintln!("error: {error}");
        return ExitCode::from(2);
    }
    match run_inner(target, &mut options) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn configure(target: Option<&str>, options: &mut sessions::AttachOptions) -> Result<(), String> {
    if let Some(target) = target {
        options.target = target.to_owned();
        sessions::validate_managed_target(options)?;
    }
    // Pin an implicit inventory path, or use explicit local semantics throughout
    // the picker and reconnect loop when no inventory exists.
    if options.config_path.is_none() && !options.local && options.socket.is_none() {
        if let Some(path) = config::default_path()
            && config::load_implicit(&path)?.is_some()
        {
            options.config_path = Some(path);
        } else if options.host.is_none() && !options.target.starts_with("host:") {
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

fn run_inner(target: Option<String>, options: &mut sessions::AttachOptions) -> Result<u8, String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("connect requires a terminal on stdin and stdout".into());
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
                "connect must start in the original terminal outside tmux and SSH ({name} is set)"
            ));
        }
    }
    let request = snapshot_request(
        options.socket.clone(),
        options.config_path.clone(),
        options.host.clone(),
        options.local,
    )
    .map_err(|(message, _)| message)?;
    for (_, alias, _) in &request.machines {
        if let Some(alias) = alias {
            sessions::validate_managed_transport(alias)?;
        }
    }
    let mut terminal = TerminalOwner::new().map_err(|e| e.to_string())?;
    let mut next = target.map(|_| options.clone());
    let mut previous = None;
    let mut error = None;
    loop {
        if let Some(code) = terminal.poll_idle().map_err(|e| e.to_string())? {
            return Ok(code);
        }
        let mut selected = match next.take() {
            Some(selected) => selected,
            None => {
                match tui::pick_host(options, previous.as_ref(), error.as_deref(), &mut terminal)
                    .map_err(|e| e.to_string())?
                {
                    tui::PickResult::Select(selected) => selected,
                    tui::PickResult::Retry => match previous.clone() {
                        Some(previous) => previous,
                        None => return Ok(0),
                    },
                    tui::PickResult::Quit(code) => return Ok(code),
                }
            }
        };
        let command = match sessions::prepare_managed(&mut selected) {
            Ok(command) => command,
            Err(message) => {
                previous = Some(selected);
                error = Some(message);
                continue;
            }
        };
        previous = Some(selected);
        if let Some(code) = terminal.poll_idle().map_err(|e| e.to_string())? {
            return Ok(code);
        }
        match terminal.attach(command).map_err(|e| e.to_string())? {
            AttachmentOutcome::Detached => return Ok(0),
            AttachmentOutcome::Picker => error = None,
            AttachmentOutcome::Failed(message) => error = Some(message),
            AttachmentOutcome::Interrupted(code) => return Ok(code),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AttachmentOutcome {
    Detached,
    Picker,
    Failed(String),
    Interrupted(u8),
}

fn outcome(status: WaitStatus) -> AttachmentOutcome {
    match status {
        WaitStatus::Exited(_, 0) => AttachmentOutcome::Detached,
        WaitStatus::Exited(_, PICKER_STATUS) => AttachmentOutcome::Picker,
        WaitStatus::Exited(_, code) => {
            AttachmentOutcome::Failed(format!("Attachment exited with status {code}."))
        }
        WaitStatus::Signaled(_, signal, _) => {
            AttachmentOutcome::Failed(format!("Attachment terminated by {signal}."))
        }
        _ => unreachable!("only completed children have attachment outcomes"),
    }
}

/// Keep the child's process-group leader unreaped until cleanup, preventing PID
/// reuse while signalling its group. The group contains only this attachment.
struct OwnedChild(Child);

impl OwnedChild {
    fn pid(&self) -> Pid {
        Pid::from_raw(self.0.id() as i32)
    }

    fn terminate(&self, signal: Signal) {
        let _ = killpg(self.pid(), signal);
        let _ = killpg(self.pid(), Signal::SIGCONT);
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if matches!(
                waitid(
                    Id::Pid(self.pid()),
                    WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT
                ),
                Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..))
            ) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = killpg(self.pid(), Signal::SIGKILL);
        let _ = self.0.wait();
    }
}

pub(crate) struct TerminalOwner {
    original: Termios,
    group: Pid,
    signals: Signals,
}

impl TerminalOwner {
    fn new() -> io::Result<Self> {
        let group = getpgrp();
        if tcgetpgrp(io::stdin())? != group {
            return Err(io::Error::other(
                "connect must run in the terminal's foreground process group",
            ));
        }
        Ok(Self {
            original: termios::tcgetattr(io::stdin())?,
            group,
            signals: Signals::new([SIGINT, SIGTERM, SIGHUP, SIGTSTP, SIGCONT, SIGWINCH, SIGTTOU])?,
        })
    }

    fn reclaim(&self) -> io::Result<()> {
        foreground(self.group)?;
        termios::tcsetattr(io::stdin(), SetArg::TCSANOW, &self.original)?;
        Ok(())
    }

    fn suspend(&self) -> io::Result<()> {
        let current = termios::tcgetattr(io::stdin())?;
        self.reclaim()?;
        kill(Pid::this(), Signal::SIGSTOP)?;
        termios::tcsetattr(io::stdin(), SetArg::TCSANOW, &current)?;
        Ok(())
    }

    pub(crate) fn poll_idle(&mut self) -> io::Result<Option<u8>> {
        for signal in self.signals.pending().collect::<Vec<_>>() {
            match signal {
                SIGINT | SIGTERM | SIGHUP => return Ok(Some((128 + signal) as u8)),
                SIGTSTP => self.suspend()?,
                _ => {}
            }
        }
        Ok(None)
    }

    fn attach(&mut self, mut command: Command) -> io::Result<AttachmentOutcome> {
        // Native clients own foreground terminal I/O and resize while attached.
        // Argos remains in its original group and only supervises their lifetime.
        command.process_group(0);
        let child = match command.spawn() {
            Ok(child) => OwnedChild(child),
            Err(error) => {
                return Ok(AttachmentOutcome::Failed(format!(
                    "Cannot start attachment: {error}"
                )));
            }
        };
        let result = (|| {
            foreground(child.pid())?;
            // A fast child can attempt a read before tcsetpgrp and receive TTIN.
            killpg(child.pid(), Signal::SIGCONT)?;
            loop {
                for signal in self.signals.pending().collect::<Vec<_>>() {
                    match signal {
                        SIGINT | SIGTERM | SIGHUP => {
                            child.terminate(Signal::try_from(signal).expect("registered signal"));
                            return Ok(AttachmentOutcome::Interrupted((128 + signal) as u8));
                        }
                        SIGWINCH => {
                            let _ = killpg(child.pid(), Signal::SIGWINCH);
                        }
                        SIGTSTP => {
                            let _ = killpg(child.pid(), Signal::SIGSTOP);
                        }
                        _ => {}
                    }
                }
                match waitid(
                    Id::Pid(child.pid()),
                    WaitPidFlag::WEXITED
                        | WaitPidFlag::WSTOPPED
                        | WaitPidFlag::WNOHANG
                        | WaitPidFlag::WNOWAIT,
                )? {
                    status @ (WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => {
                        return Ok(outcome(status));
                    }
                    WaitStatus::Stopped(_, Signal::SIGTTIN | Signal::SIGTTOU) => {
                        foreground(child.pid())?;
                        killpg(child.pid(), Signal::SIGCONT)?;
                    }
                    WaitStatus::Stopped(..) => {
                        self.suspend()?;
                        foreground(child.pid())?;
                        killpg(child.pid(), Signal::SIGCONT)?;
                    }
                    _ => {}
                }
                thread::sleep(Duration::from_millis(20));
            }
        })();
        drop(child); // terminate owned descendants and reap before any picker
        self.reclaim()?;
        // Native clients normally reset these themselves. Also repair modes
        // after a crash or forced cleanup, when they cannot run their handlers.
        crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        result
    }
}

impl Drop for TerminalOwner {
    fn drop(&mut self) {
        let _ = self.reclaim();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_handoff_detach_failure_and_signal() {
        let pid = Pid::from_raw(123);
        assert_eq!(
            outcome(WaitStatus::Exited(pid, 0)),
            AttachmentOutcome::Detached
        );
        assert_eq!(
            outcome(WaitStatus::Exited(pid, 78)),
            AttachmentOutcome::Picker
        );
        for code in [1, 2, 77, 79, 255] {
            assert!(matches!(
                outcome(WaitStatus::Exited(pid, code)),
                AttachmentOutcome::Failed(_)
            ));
        }
        assert!(matches!(
            outcome(WaitStatus::Signaled(pid, Signal::SIGTERM, false)),
            AttachmentOutcome::Failed(_)
        ));
    }
}
