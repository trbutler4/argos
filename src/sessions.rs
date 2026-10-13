use crate::{config, ssh, tmux, vm};
use std::fmt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
struct AttachTarget {
    location: HostLocation,
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

/// Discovery preferences are independent of the selected attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowseOptions {
    pub(crate) socket: Option<String>,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) local: bool,
    pub(crate) state_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostLocation {
    Local { socket: Option<String> },
    Inventory { machine: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuestMode {
    Tmux,
    Shell,
}

/// Parsed once at entry; the picker constructs these directly from its model.
/// A host target cannot accidentally acquire VM shell mode or VM state flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionTarget {
    Vm {
        id: String,
        mode: GuestMode,
    },
    HostTmux {
        session: String,
        location: HostLocation,
    },
}

impl SessionTarget {
    pub(crate) fn parse(target: &str, options: &BrowseOptions) -> Result<Self, String> {
        let explicit_host = options.host.as_deref();
        if let Some(id) = target.strip_prefix("vm:") {
            if id.is_empty() {
                return Err("vm session target must be vm:<id>".into());
            }
            if explicit_host.is_some() {
                return Err("--host is not valid with vm:<id> session targets".into());
            }
            if options.socket.is_some() || options.local {
                return Err("vm:<id> session targets only accept --config and --state-dir".into());
            }
            let target = Self::Vm {
                id: id.into(),
                mode: GuestMode::Tmux,
            };
            target.validate()?;
            return Ok(target);
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
            if options.local || options.socket.is_some() {
                return Err("host targets cannot be combined with --local or --socket".into());
            }
            return Ok(Self::HostTmux {
                session: session.into(),
                location: HostLocation::Inventory {
                    machine: Some(host.into()),
                },
            });
        }
        Ok(Self::HostTmux {
            session: target.into(),
            location: if options.local || options.socket.is_some() {
                HostLocation::Local {
                    socket: options.socket.clone(),
                }
            } else {
                HostLocation::Inventory {
                    machine: options.host.clone(),
                }
            },
        })
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Vm { id, .. } if !vm::valid_id(id) => {
                Err("VM id must be 1-64 chars of letters, digits, '_' or '-'".into())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn requires_inventory(&self) -> bool {
        matches!(
            self,
            Self::HostTmux {
                location: HostLocation::Inventory { machine: Some(_) },
                ..
            }
        )
    }

    pub(crate) fn uses_tmux(&self) -> bool {
        !matches!(
            self,
            Self::Vm {
                mode: GuestMode::Shell,
                ..
            }
        )
    }
}

impl fmt::Display for SessionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vm { id, .. } => write!(f, "vm:{id}"),
            Self::HostTmux {
                location:
                    HostLocation::Inventory {
                        machine: Some(host),
                    },
                session,
            } => write!(f, "host:{host}:{session}"),
            Self::HostTmux { session, .. } => f.write_str(session),
        }
    }
}

pub(crate) struct PreparedAttachment {
    pub(crate) command: Command,
    pub(crate) reconnect_target: SessionTarget,
}

/// Resolve targets and build commands. Only the connection owner executes them.
pub(crate) fn prepare(
    target: &SessionTarget,
    options: &BrowseOptions,
) -> Result<PreparedAttachment, String> {
    let (session, location) = match target {
        SessionTarget::Vm { id, mode } => {
            let command = vm::prepare_guest_command(vm::GuestCommandOptions {
                id: id.clone(),
                state_dir: options.state_dir.clone(),
                mode: *mode,
            })?;
            return Ok(PreparedAttachment {
                command,
                reconnect_target: target.clone(),
            });
        }
        SessionTarget::HostTmux { session, location } => (session, location),
    };
    let target = resolve_target(location, options.config_path.as_deref())?;
    if let Some(alias) = &target.ssh_alias {
        ssh::validate_destination(alias)?;
    }
    let sessions = tmux::discover(
        target.socket.as_deref(),
        target.ssh_alias.as_deref(),
        Duration::from_secs(3),
    )
    .map_err(|error| format!("{}: {}", error.code, error.message))?;
    let selected = select(&sessions, session)?;
    let command = if let Some(alias) = &target.ssh_alias {
        let mut command = ssh::interactive_command();
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
    Ok(PreparedAttachment {
        command,
        reconnect_target: SessionTarget::HostTmux {
            session: selected.id.clone(),
            location: target.location,
        },
    })
}

fn resolve_target(
    location: &HostLocation,
    config_path: Option<&std::path::Path>,
) -> Result<AttachTarget, String> {
    let host = match location {
        HostLocation::Local { socket } => {
            return Ok(AttachTarget {
                location: location.clone(),
                socket: socket.clone(),
                ssh_alias: None,
            });
        }
        HostLocation::Inventory { machine } => machine.as_deref(),
    };
    let loaded = match config_path {
        Some(p) => config::load(p).map(Some),
        None => config::default_path().map_or(Ok(None), |p| config::load_implicit(&p)),
    }?;
    let cfg = match loaded {
        Some(c) => c,
        None => {
            if host.is_some() {
                return Err("--host requires a configured inventory".into());
            }
            return Ok(AttachTarget {
                location: HostLocation::Local { socket: None },
                socket: None,
                ssh_alias: None,
            });
        }
    };
    let id = host.unwrap_or(&cfg.client.machine_id);
    let machine = cfg
        .machines
        .get(id)
        .ok_or_else(|| format!("unknown host id: {id}"))?;
    Ok(AttachTarget {
        location: HostLocation::Inventory {
            machine: Some(id.into()),
        },
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
        let defaults = BrowseOptions::default();
        assert_eq!(
            SessionTarget::parse("vm:test", &defaults).unwrap(),
            SessionTarget::Vm {
                id: "test".into(),
                mode: GuestMode::Tmux
            }
        );
        assert_eq!(
            SessionTarget::parse("host:desktop:work", &defaults).unwrap(),
            SessionTarget::HostTmux {
                session: "work".into(),
                location: HostLocation::Inventory {
                    machine: Some("desktop".into())
                }
            }
        );
        let explicit = BrowseOptions {
            host: Some("desktop".into()),
            ..Default::default()
        };
        assert_eq!(
            SessionTarget::parse("work", &explicit).unwrap(),
            SessionTarget::HostTmux {
                session: "work".into(),
                location: HostLocation::Inventory {
                    machine: Some("desktop".into())
                }
            }
        );
        assert!(SessionTarget::parse("vm:", &defaults).is_err());
        assert!(SessionTarget::parse("vm:test", &explicit).is_err());
        assert!(SessionTarget::parse("host:desktop", &defaults).is_err());
        let local = BrowseOptions {
            local: true,
            ..Default::default()
        };
        assert!(SessionTarget::parse("vm:test", &local).is_err());
        assert!(SessionTarget::parse("host:desktop:work", &local).is_err());
        assert!(SessionTarget::parse("vm:../outside", &defaults).is_err());
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
