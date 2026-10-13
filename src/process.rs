//! Bounded, cancellable capture of short-lived commands. No detached readers.
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    fmt,
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub(crate) const OUTPUT_CAP: usize = 4 * 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

#[derive(Debug)]
pub(crate) enum RunError {
    Spawn(io::Error),
    Timeout,
    Cancelled,
    OutputLimit,
    Wait(io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot start command: {e}"),
            Self::Wait(e) => write!(f, "command I/O or wait failed: {e}"),
            Self::Timeout => f.write_str("command timed out"),
            Self::Cancelled => f.write_str("command cancelled"),
            Self::OutputLimit => f.write_str("command output exceeded limit"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(e) | Self::Wait(e) => Some(e),
            _ => None,
        }
    }
}

struct CapturedChild {
    child: Child,
    reaped: bool,
}

impl CapturedChild {
    fn exited(&mut self) -> io::Result<bool> {
        #[cfg(target_os = "linux")]
        {
            use nix::{
                errno::Errno,
                sys::wait::{Id, WaitPidFlag, WaitStatus, waitid},
            };
            // Hold the group leader until cleanup so its ID cannot be reused.
            match waitid(
                Id::Pid(Pid::from_raw(self.child.id() as i32)),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG,
            ) {
                Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => Ok(true),
                Ok(_) | Err(Errno::EINTR) => Ok(false),
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.reaped = self.child.try_wait()?.is_some();
            Ok(self.reaped)
        }
    }

    fn kill_group(&self) {
        if !self.reaped {
            let _ = killpg(Pid::from_raw(self.child.id() as i32), Signal::SIGKILL);
        }
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        self.kill_group();
        let result = self.child.wait();
        self.reaped = result.is_ok();
        result
    }
}

impl Drop for CapturedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.finish();
        }
    }
}

pub(crate) fn run(command: Command, deadline: Instant) -> Result<Output, RunError> {
    run_cancellable(command, deadline, &Cancellation::default())
}

pub(crate) fn run_cancellable(
    mut command: Command,
    deadline: Instant,
    cancel: &Cancellation,
) -> Result<Output, RunError> {
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(RunError::Timeout);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = CapturedChild {
        child: command.spawn().map_err(RunError::Spawn)?,
        reaped: false,
    };
    let mut stdout = child.child.stdout.take().expect("stdout piped");
    let mut stderr = child.child.stderr.take().expect("stderr piped");
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(|e| RunError::Wait(e.into()))?;
        fcntl(
            fd,
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(|e| RunError::Wait(e.into()))?;
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof) = (false, false);
    loop {
        if cancel.is_cancelled() {
            return Err(RunError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(RunError::Timeout);
        }
        if !out_eof {
            out_eof = read_available(&mut stdout, &mut out)?;
        }
        if !err_eof {
            err_eof = read_available(&mut stderr, &mut err)?;
        }
        if out.len() + err.len() > OUTPUT_CAP {
            return Err(RunError::OutputLimit);
        }
        if child.exited().map_err(RunError::Wait)? && out_eof && err_eof {
            let status = child.finish().map_err(RunError::Wait)?;
            return Ok(Output {
                stdout: out,
                stderr: err,
                status,
            });
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn read_available(pipe: &mut impl Read, data: &mut Vec<u8>) -> Result<bool, RunError> {
    let mut buffer = [0; 8192];
    // A continuously writing child must not starve cancellation or the other pipe.
    for _ in 0..8 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                data.extend_from_slice(&buffer[..n]);
                if data.len() > OUTPUT_CAP {
                    return Err(RunError::OutputLimit);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(RunError::Wait(e)),
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_with_inherited_pipe() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10 & exit 0"]);
        assert!(matches!(
            run(command, Instant::now() + Duration::from_millis(50)),
            Err(RunError::Timeout)
        ));
    }

    #[test]
    fn output_limit() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes x"]);
        assert!(matches!(
            run(command, Instant::now() + Duration::from_secs(2)),
            Err(RunError::OutputLimit)
        ));
    }

    #[test]
    fn cancellation_interrupts_a_quiet_child() {
        let cancel = Cancellation::default();
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(40));
                cancel.cancel();
            });
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 10"]);
            let started = Instant::now();
            assert!(matches!(
                run_cancellable(command, started + Duration::from_secs(10), &cancel),
                Err(RunError::Cancelled)
            ));
            assert!(started.elapsed() < Duration::from_secs(1));
        });
    }
}
