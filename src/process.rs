use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const OUTPUT_CAP: usize = 4 * 1024 * 1024;
pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: std::process::ExitStatus,
}
pub(crate) enum RunError {
    Spawn(std::io::Error),
    Timeout,
    OutputLimit,
    Wait(std::io::Error),
}
type PipeResult = std::io::Result<Vec<u8>>;

pub(crate) fn run(mut command: Command, deadline: Instant) -> Result<Output, RunError> {
    // Killing the direct child is portable here. Descendants that inherit the
    // pipes may outlive this call; readers are deliberately detached so they
    // cannot make a timed-out discovery hang while waiting for EOF.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let (tx, rx) = mpsc::channel::<(bool, PipeResult)>();
    spawn_reader(true, stdout, tx.clone());
    spawn_reader(false, stderr, tx);
    let mut out = None;
    let mut err = None;
    loop {
        if let Some(status) = child.try_wait().map_err(RunError::Wait)? {
            while (out.is_none() || err.is_none()) && Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok((true, value)) => out = Some(value),
                    Ok((false, value)) => err = Some(value),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let stdout = out.ok_or(RunError::Timeout)?.map_err(RunError::Wait)?;
            let stderr = err.ok_or(RunError::Timeout)?.map_err(RunError::Wait)?;
            if stdout.len() + stderr.len() > OUTPUT_CAP {
                return Err(RunError::OutputLimit);
            }
            return Ok(Output {
                stdout,
                stderr,
                status,
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout);
        }
        while let Ok((is_stdout, value)) = rx.try_recv() {
            if is_stdout {
                out = Some(value);
            } else {
                err = Some(value);
            }
            if out
                .as_ref()
                .is_some_and(|v: &PipeResult| v.as_ref().is_ok_and(|b| b.len() > OUTPUT_CAP))
                || err
                    .as_ref()
                    .is_some_and(|v: &PipeResult| v.as_ref().is_ok_and(|b| b.len() > OUTPUT_CAP))
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::OutputLimit);
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    is_stdout: bool,
    mut pipe: R,
    tx: mpsc::Sender<(bool, PipeResult)>,
) {
    thread::spawn(move || {
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    if data.len() > OUTPUT_CAP {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send((is_stdout, Err(e)));
                    return;
                }
            }
        }
        let _ = tx.send((is_stdout, Ok(data)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn timeout() {
        let mut c = Command::new("sh");
        c.args(["-c", "sleep 10 & exit 0"]);
        assert!(matches!(
            run(c, Instant::now() + Duration::from_millis(50)),
            Err(RunError::Timeout)
        ));
    }
    #[cfg(unix)]
    #[test]
    fn cap() {
        let mut c = Command::new("sh");
        c.args(["-c", "yes x"]);
        assert!(matches!(
            run(c, Instant::now() + Duration::from_secs(2)),
            Err(RunError::OutputLimit)
        ));
    }
}
