//! Linux terminal/process ownership. No target selection or picker protocol.
use nix::{
    errno::Errno,
    sys::{
        signal::{SigSet, SigmaskHow, Signal, kill, killpg, pthread_sigmask},
        termios::{self, SetArg, Termios},
        wait::{Id, WaitPidFlag, WaitStatus, waitid},
    },
    unistd::{Pid, getpgrp, tcgetpgrp, tcsetpgrp},
};
use signal_hook::{consts::signal::*, iterator::Signals};
use std::{
    io::{self, Write},
    os::unix::process::CommandExt,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) enum ProcessOutcome {
    Exited(i32),
    Signaled(Signal),
    Interrupted(u8),
    SpawnFailed(io::Error),
}

fn flush_display(writer: &mut impl Write) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match writer.flush() {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            result => return result,
        }
    }
}

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
    pub(crate) fn new() -> io::Result<Self> {
        let group = getpgrp();
        if tcgetpgrp(io::stdin())? != group {
            return Err(io::Error::other(
                "Argos must run in the terminal's foreground process group",
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

    pub(crate) fn attach(&mut self, mut command: Command) -> io::Result<ProcessOutcome> {
        // Native clients own foreground terminal I/O and resize while attached.
        // Argos remains in its original group and only supervises their lifetime.
        command.process_group(0);
        let child = match command.spawn() {
            Ok(child) => OwnedChild(child),
            Err(error) => return Ok(ProcessOutcome::SpawnFailed(error)),
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
                            return Ok(ProcessOutcome::Interrupted((128 + signal) as u8));
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
                let status = match waitid(
                    Id::Pid(child.pid()),
                    WaitPidFlag::WEXITED
                        | WaitPidFlag::WSTOPPED
                        | WaitPidFlag::WNOHANG
                        | WaitPidFlag::WNOWAIT,
                ) {
                    Err(Errno::EINTR) => continue,
                    result => result?,
                };
                match status {
                    WaitStatus::Exited(_, code) => return Ok(ProcessOutcome::Exited(code)),
                    WaitStatus::Signaled(_, signal, _) => {
                        return Ok(ProcessOutcome::Signaled(signal));
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
        self.reclaim()
            .map_err(|e| io::Error::new(e.kind(), format!("cannot reclaim terminal: {e}")))?;
        // Native clients normally reset these themselves. Also repair modes
        // after a crash or forced cleanup, when they cannot run their handlers.
        let mut stdout = io::stdout().lock();
        crossterm::queue!(
            stdout,
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        )
        .map_err(|e| io::Error::new(e.kind(), format!("cannot queue terminal reset: {e}")))?;
        // Display flush can transiently return WouldBlock after signal cleanup.
        // Retry the existing buffer, never requeue partial escape codes.
        flush_display(&mut stdout).map_err(|e| {
            io::Error::new(e.kind(), format!("cannot restore terminal display: {e}"))
        })?;
        result.map_err(|e: io::Error| {
            io::Error::new(e.kind(), format!("cannot supervise attachment: {e}"))
        })
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
    fn retries_transient_flush_without_rewriting_buffer() {
        struct Output {
            bytes: Vec<u8>,
            flushes: usize,
        }
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                match self.flushes {
                    1 => Err(io::ErrorKind::WouldBlock.into()),
                    2 => Err(io::ErrorKind::Interrupted.into()),
                    _ => Ok(()),
                }
            }
        }
        let mut output = Output {
            bytes: Vec::new(),
            flushes: 0,
        };
        output.write_all(b"\x1b[?25h").unwrap();
        flush_display(&mut output).unwrap();
        assert_eq!(output.bytes, b"\x1b[?25h");
        assert_eq!(output.flushes, 3);
    }
}
