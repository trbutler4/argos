use crate::{config, tmux, vm};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::{fs::MetadataExt, process::CommandExt};

#[derive(Debug, PartialEq, Eq)]
struct TmuxEnv {
    socket: String,
    pid: u64,
    pane: String,
}

#[derive(Debug, PartialEq, Eq)]
struct AttachTarget {
    id: String,
    socket: Option<String>,
    ssh_alias: Option<String>,
}

impl AttachTarget {
    fn is_remote(&self) -> bool {
        self.ssh_alias.is_some()
    }
}

fn parse_tmux_env(value: &str, pane_value: Option<&str>) -> Result<TmuxEnv, String> {
    let mut parts = value.rsplitn(3, ',');
    let client = parts.next().ok_or("TMUX is malformed")?;
    let pid = parts.next().ok_or("TMUX is malformed")?;
    let socket = parts.next().ok_or("TMUX is malformed")?;
    if socket.is_empty() || pid.parse::<u64>().is_err() || client.parse::<u64>().is_err() {
        return Err("TMUX has invalid numeric suffixes".into());
    }
    let pane = pane_value.ok_or("TMUX_PANE is required inside tmux")?;
    if !pane.starts_with('%') || pane[1..].parse::<u64>().is_err() {
        return Err("TMUX_PANE must be a numeric %pane ID".into());
    }
    Ok(TmuxEnv {
        socket: socket.into(),
        pid: pid.parse().map_err(|_| "invalid TMUX PID")?,
        pane: pane.into(),
    })
}

fn select<'a>(sessions: &'a [tmux::Session], query: &str) -> Result<&'a tmux::Session, String> {
    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| s.id == query || s.name == query)
        .collect();
    match matches.as_slice() {
        [session] => Ok(session),
        [] => Err(format!("tmux session not found: {query:?}")),
        _ => Err(format!("tmux session is ambiguous: {query:?}")),
    }
}

pub(crate) struct AttachOptions {
    pub(crate) target: String,
    pub(crate) socket: Option<String>,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) local: bool,
    pub(crate) state_dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum AttachSpec {
    Vm {
        id: String,
    },
    HostTmux {
        session: String,
        host: Option<String>,
    },
}

pub(crate) fn attach(options: AttachOptions) -> ExitCode {
    let spec = match parse_attach_spec(&options.target, options.host.as_deref()) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    match spec {
        AttachSpec::Vm { id } => attach_vm_session(id, options),
        AttachSpec::HostTmux { session, host } => {
            if options.state_dir.is_some() {
                eprintln!("error: host session targets do not accept --state-dir");
                return ExitCode::from(2);
            }
            attach_host_tmux(
                &session,
                options.socket,
                options.config_path,
                host,
                options.local,
            )
        }
    }
}

fn parse_attach_spec(target: &str, explicit_host: Option<&str>) -> Result<AttachSpec, String> {
    if let Some(id) = target.strip_prefix("vm:") {
        if id.is_empty() {
            return Err("vm session target must be vm:<id>".into());
        }
        if explicit_host.is_some() {
            return Err("--host is not valid with vm:<id> session targets".into());
        }
        return Ok(AttachSpec::Vm { id: id.into() });
    }
    if let Some(rest) = target.strip_prefix("host:") {
        let Some((host, session)) = rest.split_once(':') else {
            return Err("host session target must be host:<host>:<session>".into());
        };
        if host.is_empty() || session.is_empty() {
            return Err("host session target must be host:<host>:<session>".into());
        }
        if explicit_host.is_some() {
            return Err("use either host:<host>:<session> or --host, not both".into());
        }
        return Ok(AttachSpec::HostTmux {
            session: session.into(),
            host: Some(host.into()),
        });
    }
    Ok(AttachSpec::HostTmux {
        session: target.into(),
        host: explicit_host.map(str::to_owned),
    })
}

fn attach_vm_session(id: String, options: AttachOptions) -> ExitCode {
    if options.socket.is_some() || options.config_path.is_some() || options.local {
        eprintln!("error: vm:<id> session targets only accept --state-dir");
        return ExitCode::from(2);
    }
    vm::guest_command(vm::GuestCommandOptions {
        id,
        state_dir: options.state_dir,
        tmux: true,
    })
}

fn attach_host_tmux(
    session: &str,
    socket: Option<String>,
    config_path: Option<PathBuf>,
    host: Option<String>,
    local: bool,
) -> ExitCode {
    let mut target = match resolve_target(socket, config_path, host, local) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("error: attach requires a terminal on stdin and stdout");
        return ExitCode::from(1);
    }
    let inside = match std::env::var("TMUX") {
        Ok(value) if !value.is_empty() => {
            match parse_tmux_env(&value, std::env::var("TMUX_PANE").ok().as_deref()) {
                Ok(env) => Some(env),
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::from(1);
                }
            }
        }
        Ok(_) | Err(std::env::VarError::NotPresent) => None,
        Err(_) => {
            eprintln!("error: TMUX is not valid UTF-8");
            return ExitCode::from(1);
        }
    };

    if let Some(env) = &inside
        && !target.is_remote()
    {
        if !same_socket(&env.socket, target.socket.as_deref()) {
            eprintln!("error: refusing to nest across different tmux servers");
            return ExitCode::from(1);
        }
        target.socket = Some(env.socket.clone());
    }

    let sessions = match tmux::discover(
        target.socket.as_deref(),
        target.ssh_alias.as_deref(),
        Duration::from_secs(3),
    ) {
        Ok(sessions) => sessions,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            return ExitCode::from(1);
        }
    };
    let selected = match select(&sessions, session) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };

    match (inside.as_ref(), target.ssh_alias.as_deref()) {
        (Some(env), None) => switch_inside(env, &selected.id),
        (None, None) => exec_local_tmux(target.socket.as_deref(), &selected.id),
        (Some(env), Some(alias)) => {
            open_remote_window(env, &target.id, alias, target.socket.as_deref(), selected)
        }
        (None, Some(alias)) => exec_remote_ssh(alias, target.socket.as_deref(), &selected.id),
    }
}

fn resolve_target(
    socket: Option<String>,
    config_path: Option<PathBuf>,
    host: Option<String>,
    local: bool,
) -> Result<AttachTarget, String> {
    if local || socket.is_some() {
        return Ok(AttachTarget {
            id: "local".into(),
            socket,
            ssh_alias: None,
        });
    }
    let loaded = match config_path {
        Some(p) => config::load(&p).map(Some),
        None => config::default_path().map_or(Ok(None), |p| config::load_implicit(&p)),
    }?;
    let cfg = match loaded {
        Some(c) => c,
        None => {
            if host.is_some() {
                return Err("--host requires a configured inventory".into());
            }
            return Ok(AttachTarget {
                id: "local".into(),
                socket: None,
                ssh_alias: None,
            });
        }
    };
    let id = host.as_deref().unwrap_or(&cfg.client.machine_id);
    let machine = cfg
        .machines
        .get(id)
        .ok_or_else(|| format!("unknown host id: {id}"))?;
    Ok(AttachTarget {
        id: id.into(),
        socket: machine.socket.clone(),
        ssh_alias: machine.ssh_alias.clone(),
    })
}

fn exec_local_tmux(socket: Option<&str>, target: &str) -> ExitCode {
    let mut command = Command::new("tmux");
    command.args(["-u", "-N"]);
    if let Some(path) = socket {
        command.args(["-S", path]);
    }
    command.args(["attach-session", "-t", target]);
    #[cfg(unix)]
    {
        let error = command.exec();
        eprintln!("error: cannot execute tmux: {error}");
        ExitCode::from(1)
    }
    #[cfg(not(unix))]
    {
        eprintln!("error: native attach is unsupported on this platform");
        ExitCode::from(1)
    }
}

fn exec_remote_ssh(alias: &str, socket: Option<&str>, target: &str) -> ExitCode {
    let mut command = remote_ssh_command(alias, socket, target, true);
    #[cfg(unix)]
    {
        let error = command.exec();
        eprintln!("error: cannot execute ssh: {error}");
        ExitCode::from(1)
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) if status.success() => ExitCode::SUCCESS,
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(error) => {
                eprintln!("error: cannot execute ssh: {error}");
                ExitCode::from(1)
            }
        }
    }
}

fn open_remote_window(
    env: &TmuxEnv,
    host_id: &str,
    alias: &str,
    socket: Option<&str>,
    target: &tmux::Session,
) -> ExitCode {
    let label = format!("argos:{host_id}:{}", target.name);
    let mut tmux = Command::new("tmux");
    tmux.args([
        "-u",
        "-N",
        "-S",
        &env.socket,
        "new-window",
        "-n",
        &label,
        &remote_ssh_shell_command(alias, socket, &target.id),
    ]);
    match tmux::process::run(tmux, Instant::now() + Duration::from_secs(3)) {
        Ok(output) if output.status.success() => ExitCode::SUCCESS,
        Ok(output) => {
            eprintln!(
                "error: tmux new-window failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!("error: tmux new-window timed out");
            ExitCode::from(1)
        }
    }
}

fn remote_ssh_command(alias: &str, socket: Option<&str>, target: &str, force_tty: bool) -> Command {
    let mut command = Command::new("ssh");
    if force_tty {
        command.arg("-tt");
    }
    command.env("LC_ALL", "C").args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "PermitLocalCommand=no",
        alias,
        &remote_attach_script(socket, target),
    ]);
    command
}

fn remote_ssh_shell_command(alias: &str, socket: Option<&str>, target: &str) -> String {
    let mut parts = vec!["exec".into(), "ssh".into(), "-tt".into()];
    for arg in [
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "PermitLocalCommand=no",
        alias,
        &remote_attach_script(socket, target),
    ] {
        parts.push(shell_quote(arg));
    }
    parts.join(" ")
}

fn remote_attach_script(socket: Option<&str>, target: &str) -> String {
    let mut args = vec!["-u".to_string(), "-N".to_string()];
    if let Some(path) = socket {
        args.push("-S".into());
        args.push(path.into());
    }
    args.extend(["attach-session".into(), "-t".into(), target.into()]);
    let tmux = args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(path) = socket {
        format!(
            "if [ -e {} ] && [ ! -S {} ]; then echo 'argos: invalid_socket' >&2; exit 125; fi; LC_ALL=C exec tmux {tmux}",
            shell_quote(path),
            shell_quote(path)
        )
    } else {
        format!("LC_ALL=C exec tmux {tmux}")
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn same_socket(a: &str, b: Option<&str>) -> bool {
    let Some(b) = b else {
        return true;
    };
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

fn switch_inside(env: &TmuxEnv, target: &str) -> ExitCode {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut query = Command::new("tmux");
    query.args([
        "-u",
        "-N",
        "-S",
        &env.socket,
        "display-message",
        "-p",
        "-t",
        &env.pane,
        "#{session_id}",
    ]);
    let current = match tmux::process::run(query, deadline) {
        Ok(o) if o.status.success() => {
            let value = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if value
                .strip_prefix('$')
                .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
            {
                value
            } else {
                eprintln!("error: tmux returned invalid current session ID: {value:?}");
                return ExitCode::from(1);
            }
        }
        _ => {
            eprintln!("error: cannot query current tmux session");
            return ExitCode::from(1);
        }
    };
    let mut clients = Command::new("tmux");
    clients.args([
        "-u",
        "-N",
        "-S",
        &env.socket,
        "list-clients",
        "-F",
        "#{client_name}",
        "-t",
        &current,
    ]);
    let output = match tmux::process::run(clients, deadline) {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!("error: cannot query tmux clients");
            return ExitCode::from(1);
        }
    };
    let client_listing = String::from_utf8_lossy(&output.stdout);
    let names: Vec<_> = client_listing.lines().filter(|x| !x.is_empty()).collect();
    if names.len() != 1 {
        eprintln!(
            "error: invoking tmux session has {} clients; refusing ambiguous switch",
            names.len()
        );
        return ExitCode::from(1);
    }
    let mut switch = Command::new("tmux");
    switch.args([
        "-u",
        "-N",
        "-S",
        &env.socket,
        "switch-client",
        "-c",
        names[0],
        "-t",
        target,
    ]);
    match tmux::process::run(switch, deadline) {
        Ok(o) if o.status.success() => ExitCode::SUCCESS,
        _ => {
            eprintln!("error: tmux switch-client failed");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_target_layers() {
        assert_eq!(
            parse_attach_spec("vm:test", None).unwrap(),
            AttachSpec::Vm { id: "test".into() }
        );
        assert_eq!(
            parse_attach_spec("host:desktop:qvattro", None).unwrap(),
            AttachSpec::HostTmux {
                session: "qvattro".into(),
                host: Some("desktop".into())
            }
        );
        assert_eq!(
            parse_attach_spec("qvattro", Some("desktop")).unwrap(),
            AttachSpec::HostTmux {
                session: "qvattro".into(),
                host: Some("desktop".into())
            }
        );
        assert!(parse_attach_spec("vm:", None).is_err());
        assert!(parse_attach_spec("vm:test", Some("desktop")).is_err());
        assert!(parse_attach_spec("host:desktop", None).is_err());
    }

    #[test]
    fn env_parsing_allows_commas() {
        let e = parse_tmux_env("/tmp/a,b,12,7", Some("%3")).unwrap();
        assert_eq!(e.socket, "/tmp/a,b");
        assert_eq!(e.pid, 12);
    }

    #[test]
    fn env_rejects_bad_pane() {
        assert!(parse_tmux_env("/tmp/a,1,2", None).is_err());
        assert!(parse_tmux_env("/tmp/a,1,2", Some("x")).is_err());
    }

    #[test]
    fn refuses_id_name_ambiguity() {
        let sessions = vec![
            tmux::Session {
                id: "$1".into(),
                name: "one".into(),
                windows: 1,
                attached_clients: 0,
            },
            tmux::Session {
                id: "$2".into(),
                name: "$1".into(),
                windows: 1,
                attached_clients: 0,
            },
        ];
        assert!(select(&sessions, "$1").is_err());
    }

    #[test]
    fn exact_match_only() {
        let s = vec![tmux::Session {
            id: "$1".into(),
            name: "work".into(),
            windows: 1,
            attached_clients: 0,
        }];
        assert!(select(&s, "wo").is_err());
        assert_eq!(select(&s, "work").unwrap().id, "$1");
    }

    #[test]
    fn remote_script_quotes_exact_target() {
        let script = remote_attach_script(Some("/tmp/a b'sock"), "$12");
        assert!(script.contains("'/tmp/a b'\\''sock'"));
        assert!(script.contains("'$12'"));
        assert!(script.contains("exec tmux"));
    }
}
