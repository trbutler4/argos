use crate::{config, tmux, vm};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
struct AttachTarget {
    id: String,
    socket: Option<String>,
    ssh_alias: Option<String>,
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

#[derive(Clone, Debug)]
pub(crate) struct AttachOptions {
    pub(crate) target: String,
    pub(crate) socket: Option<String>,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) local: bool,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) vm_shell: bool,
}

pub(crate) fn attach(options: AttachOptions) -> ExitCode {
    crate::connect::run(Some(options.target.clone()), options)
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

pub(crate) fn validate_managed_target(options: &AttachOptions) -> Result<(), String> {
    match parse_attach_spec(&options.target, options.host.as_deref())? {
        AttachSpec::Vm { id } if !vm::valid_id(&id) => {
            Err("VM id must be 1-64 chars of letters, digits, '_' or '-'".into())
        }
        AttachSpec::Vm { .. } if options.socket.is_some() || options.local => {
            Err("vm:<id> session targets only accept --config and --state-dir".into())
        }
        AttachSpec::HostTmux { host: Some(_), .. } if options.local || options.socket.is_some() => {
            Err("host targets cannot be combined with --local or --socket".into())
        }
        _ => Ok(()),
    }
}

/// Resolve targets and build commands. Only the connection owner executes them.
pub(crate) fn prepare_managed(options: &mut AttachOptions) -> Result<Command, String> {
    validate_managed_target(options)?;
    let (session, host) = match parse_attach_spec(&options.target, options.host.as_deref())? {
        AttachSpec::Vm { id } => {
            return vm::prepare_guest_command(vm::GuestCommandOptions {
                id,
                state_dir: options.state_dir.clone(),
                tmux: !options.vm_shell,
            });
        }
        AttachSpec::HostTmux { session, host } => (session, host),
    };
    let target = resolve_target(
        options.socket.clone(),
        options.config_path.clone(),
        host,
        options.local,
    )?;
    if let Some(alias) = &target.ssh_alias {
        validate_managed_transport(alias)?;
    }
    let sessions = tmux::discover(
        target.socket.as_deref(),
        target.ssh_alias.as_deref(),
        Duration::from_secs(3),
    )
    .map_err(|error| format!("{}: {}", error.code, error.message))?;
    let selected = select(&sessions, &session)?;
    let command = if let Some(alias) = &target.ssh_alias {
        let mut command = ssh_command();
        command.args([
            alias,
            &remote_attach_script(target.socket.as_deref(), &selected.id),
        ]);
        command
    } else {
        let mut command = local_tmux_command(target.socket.as_deref(), &selected.id);
        command.env("ARGOS_CONNECT_CLIENT", "1");
        command
    };
    options.target = if options.local || options.socket.is_some() {
        selected.id.clone()
    } else {
        format!("host:{}:{}", target.id, selected.id)
    };
    options.host = None;
    Ok(command)
}

pub(crate) fn validate_managed_transport(alias: &str) -> Result<(), String> {
    let mut command = ssh_command();
    command.arg(alias);
    validate_ssh_command(&command)
}

/// Inspect the exact destination/port/options before adding the remote command.
pub(crate) fn validate_ssh_command(command: &Command) -> Result<(), String> {
    let mut inspect = Command::new(command.get_program());
    inspect
        .arg("-G")
        .args(command.get_args())
        .env("LC_ALL", "C");
    let output = tmux::process::run(inspect, Instant::now() + Duration::from_secs(3))
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

fn local_tmux_command(socket: Option<&str>, target: &str) -> Command {
    let mut command = Command::new("tmux");
    command.args(["-u", "-N"]);
    if let Some(path) = socket {
        command.args(["-S", path]);
    }
    // Do not copy the managed client marker into shared session environment.
    command.args(["attach-session", "-E", "-t", target]);
    command
}

pub(crate) fn ssh_command() -> Command {
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

fn remote_attach_script(socket: Option<&str>, target: &str) -> String {
    let command = local_tmux_command(socket, target);
    let tmux = command
        .get_args()
        .map(|arg| shell_quote(arg.to_str().expect("attachment arguments are UTF-8")))
        .collect::<Vec<_>>()
        .join(" ");
    let check = socket.map(|path| format!(
        "if [ -e {} ] && [ ! -S {} ]; then echo 'argos: invalid_socket' >&2; exit 125; fi; ",
        shell_quote(path), shell_quote(path))).unwrap_or_default();
    format!("{check}ARGOS_CONNECT_CLIENT=1 LC_ALL=C exec tmux {tmux}")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
            parse_attach_spec("host:desktop:work", None).unwrap(),
            AttachSpec::HostTmux {
                session: "work".into(),
                host: Some("desktop".into())
            }
        );
        assert_eq!(
            parse_attach_spec("work", Some("desktop")).unwrap(),
            AttachSpec::HostTmux {
                session: "work".into(),
                host: Some("desktop".into())
            }
        );
        assert!(parse_attach_spec("vm:", None).is_err());
        assert!(parse_attach_spec("vm:test", Some("desktop")).is_err());
        assert!(parse_attach_spec("host:desktop", None).is_err());
    }
    #[test]
    fn exact_unambiguous_selection() {
        let sessions = vec![
            tmux::Session {
                id: "$1".into(),
                name: "work".into(),
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
        assert!(select(&sessions, "wo").is_err());
        assert_eq!(select(&sessions, "work").unwrap().id, "$1");
    }
    #[test]
    fn remote_script_quotes_exact_target() {
        let script = remote_attach_script(Some("/tmp/a b'sock"), "$12");
        assert!(script.contains("'/tmp/a b'\\''sock'"));
        assert!(script.contains("'$12'"));
        assert!(script.contains("exec tmux"));
    }
}
