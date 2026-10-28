//! Read-only, strictly trusted guest artifact retrieval.
use super::state::{load_record, state_root, valid_id};
use super::{VmError, invalid_args, shell_quote, ssh_port_for_id, state_error};
use crate::process;
use std::{
    fs::{self, OpenOptions},
    net::Ipv4Addr,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct FetchOptions {
    pub(crate) id: String,
    pub(crate) guest_path: String,
    pub(crate) output: PathBuf,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) json: bool,
}

pub(crate) fn fetch(options: FetchOptions) -> ExitCode {
    match retrieve(&options) {
        Ok(hash) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"id":options.id,"guest_path":options.guest_path,"output":options.output.to_string_lossy(),"sha256":hash})
                );
            } else {
                println!(
                    "Fetched {} to {} (sha256 {})",
                    options.id,
                    options.output.display(),
                    hash
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {}", error.message);
            ExitCode::from(if error.code == "invalid_args" { 2 } else { 1 })
        }
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn retrieve(options: &FetchOptions) -> Result<String, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args("invalid VM id"));
    }
    if !options.guest_path.starts_with('/') || options.guest_path.chars().any(char::is_control) {
        return Err(invalid_args(
            "guest path must be absolute and contain no control characters",
        ));
    }
    if options.output.file_name().is_none() {
        return Err(invalid_args("output must name a file"));
    }
    let root = state_root(options.state_dir.clone())?;
    match fs::symlink_metadata(&options.output) {
        Ok(_) => return Err(failure("output already exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(failure("cannot inspect output")),
    }
    let record = load_record(&root.join("vms").join(format!("{}.json", options.id)))?;
    if record.id != options.id {
        return Err(failure("VM state id does not match requested VM"));
    }
    if record.status != "running" {
        return Err(failure("VM is not running"));
    }
    let host = record.ssh_host.as_deref().unwrap_or("127.0.0.1");
    if !host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback()) {
        return Err(failure("guest SSH host must be IPv4 loopback"));
    }
    let user = record.ssh_user.as_deref().unwrap_or("root");
    if user.is_empty()
        || user.starts_with('-')
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(failure("invalid guest SSH user"));
    }
    let trust = record
        .instance_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("instances").join(&record.id))
        .join("known_hosts");
    if !fs::symlink_metadata(&trust).is_ok_and(|m| m.is_file() && m.len() > 0) {
        return Err(failure("existing regular known_hosts file required"));
    }
    if !trust.is_absolute()
        || trust.to_str().is_none_or(|s| {
            s.chars()
                .any(|c| c.is_control() || matches!(c, '"' | '\\' | '%'))
        })
    {
        return Err(failure("unsupported known_hosts path"));
    }
    let port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    if port == 0 {
        return Err(failure("invalid guest SSH port"));
    }
    let ssh = || {
        let mut c = Command::new("ssh");
        c.args(["-F", "/dev/null", "-4", "-T", "-p", &port.to_string()]);
        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=/dev/null",
            "UpdateHostKeys=no",
            "VerifyHostKeyDNS=no",
            "ProxyCommand=none",
            "ProxyJump=none",
            "ConnectTimeout=10",
            "ConnectionAttempts=1",
            "ClearAllForwardings=yes",
            "ForwardAgent=no",
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            c.args(["-o", option]);
        }
        c.arg("-o")
            .arg(format!("UserKnownHostsFile=\"{}\"", trust.display()))
            .arg(format!("{user}@{host}"));
        c
    };
    // One deadline covers metadata, transfer and local verification.
    let deadline = Instant::now() + Duration::from_secs(120);
    let quoted = shell_quote(&options.guest_path);
    let remote_hash = || {
        let mut c = ssh();
        c.arg(format!("test -f {quoted} && sha256sum < {quoted}"));
        digest(c, deadline)
    };
    let before = remote_hash()?;
    let parent = options
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let (temporary, file) = (0..100)
        .find_map(|n| {
            let path = parent.join(format!(".argos-fetch-{}-{n}.tmp", std::process::id()));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .ok()
                .map(|file| (Temporary(path), file))
        })
        .ok_or_else(|| failure("cannot create private output temporary file"))?;
    let mut c = ssh();
    c.arg(format!("test -f {quoted} && exec cat -- {quoted}"));
    transfer(c, &file, deadline)?;
    file.sync_all()
        .map_err(|_| failure("cannot sync artifact"))?;
    let after = remote_hash()?;
    let mut c = Command::new("sha256sum");
    // stdin is owned by the bounded runner, so pass a filename with -- instead.
    c.args(["--zero", "--"]).arg(&temporary.0);
    let local = digest(c, deadline)?;
    if before != after || before != local {
        return Err(failure("artifact checksum mismatch"));
    }
    fs::hard_link(&temporary.0, &options.output)
        .map_err(|_| failure("cannot publish artifact without overwriting output"))?;
    Ok(local)
}

fn transfer(mut c: Command, file: &fs::File, deadline: Instant) -> Result<(), VmError> {
    if Instant::now() >= deadline {
        return Err(failure("artifact transfer deadline expired"));
    }
    c.stdin(Stdio::null())
        .stdout(Stdio::from(
            file.try_clone()
                .map_err(|_| failure("cannot clone output file"))?,
        ))
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = process::CapturedChild::new(
        c.spawn()
            .map_err(|_| failure("cannot start artifact transfer"))?,
    );
    loop {
        // Share the capture runner's platform-specific wait and cleanup policy.
        // Linux holds the group leader with WNOWAIT until group cleanup.
        let exited = child.exited().map_err(|_| ());
        if exited == Ok(true) || exited.is_err() || Instant::now() >= deadline {
            let status = child.finish();
            if exited != Ok(true) {
                return Err(failure(
                    "artifact transfer timed out or could not be waited",
                ));
            }
            return match status {
                Ok(status) if status.success() => Ok(()),
                _ => Err(failure("artifact transfer failed")),
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn failure(message: &str) -> VmError {
    state_error(message.to_owned())
}

fn digest(command: Command, deadline: Instant) -> Result<String, VmError> {
    let output = process::run(command, deadline)
        .map_err(|_| failure("checksum command failed or timed out"))?;
    if !output.status.success() {
        return Err(failure("checksum command failed"));
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| failure("invalid checksum output"))?;
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failure("invalid checksum output"));
    }
    Ok(hash.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_completion_reports_exit_status() {
        let file = OpenOptions::new().write(true).open("/dev/null").unwrap();
        for (script, success) in [
            ("printf artifact", true),
            ("printf partial; exit 17", false),
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            assert_eq!(
                transfer(command, &file, Instant::now() + Duration::from_secs(2)).is_ok(),
                success
            );
        }
    }

    #[test]
    fn streaming_timeout_is_bounded() {
        let file = OpenOptions::new().write(true).open("/dev/null").unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60 & wait"]);
        let started = Instant::now();
        assert!(transfer(command, &file, started + Duration::from_millis(50)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn checksum_parser_rejects_noise_and_failure() {
        for script in ["printf bad", "printf '%064d' 0; exit 1", "yes noise"] {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            assert!(digest(command, Instant::now() + Duration::from_secs(2)).is_err());
        }
    }
}
