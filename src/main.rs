use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::{
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod attach;
mod config;
mod host;
mod tmux;
mod tui;
mod vm;

#[derive(Parser, Debug)]
#[command(name = "argos", version, about = "A command center for tmux work")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand, Debug)]
enum Command {
    /// List configured tmux sessions.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"])]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"])]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"])]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
    },
    /// Attach to an exact tmux session name or ID.
    Attach {
        session: String,
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"])]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"])]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"])]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
    },
    /// Open the interactive terminal UI.
    Tui {
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"])]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"])]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"])]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
    },
    /// Host-local helper commands for installed Argos instances.
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Query installed Argos helpers across configured hosts.
    Hosts {
        #[command(subcommand)]
        command: HostsCommand,
    },
    /// Manage Argos VM state on this host.
    Vm {
        #[command(subcommand)]
        command: VmCommand,
    },
}

#[derive(Subcommand, Debug)]
enum HostCommand {
    /// Report this host's Argos helper and VM capability status.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum HostsCommand {
    /// Report installed Argos helper and VM capability status for configured hosts.
    Status {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "ID")]
        host: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum VmCommand {
    /// List VM records in this host's Argos state directory.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
    },
}
#[derive(Clone, Serialize)]
pub(crate) struct Snapshot {
    pub(crate) schema_version: u32,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) hosts: Vec<Host>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Host {
    pub(crate) id: String,
    pub(crate) status: &'static str,
    pub(crate) sessions: Vec<tmux::Session>,
    pub(crate) error: Option<HostError>,
}
#[derive(Clone, Serialize)]
pub(crate) struct HostError {
    pub(crate) code: String,
    pub(crate) message: String,
}

pub(crate) struct SnapshotRequest {
    pub(crate) machines: Vec<(String, Option<String>, Option<String>)>,
    pub(crate) timeout_secs: u64,
    pub(crate) parallel: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        let mut c = Cli::command();
        let _ = c.print_help();
        println!();
        return ExitCode::SUCCESS;
    };
    match command {
        Command::List {
            json,
            socket,
            config,
            host,
            local,
        } => run_list(json, socket, config, host, local),
        Command::Attach {
            session,
            socket,
            config,
            host,
            local,
        } => attach::run(&session, socket, config, host, local),
        Command::Tui {
            socket,
            config,
            host,
            local,
        } => tui::run(socket, config, host, local),
        Command::Host { command } => match command {
            HostCommand::Status { json } => host::status(json),
        },
        Command::Hosts { command } => match command {
            HostsCommand::Status {
                json,
                config,
                host: host_filter,
            } => host::hosts_status(json, config, host_filter),
        },
        Command::Vm { command } => match command {
            VmCommand::List { json, state_dir } => vm::list(json, state_dir),
        },
    }
}

fn run_list(
    json: bool,
    socket: Option<String>,
    config_path: Option<std::path::PathBuf>,
    host_filter: Option<String>,
    force_local: bool,
) -> ExitCode {
    let request = match snapshot_request(socket, config_path, host_filter, force_local) {
        Ok(request) => request,
        Err((message, code)) => {
            eprintln!("error: {message}");
            return ExitCode::from(code);
        }
    };
    let snap = discover_snapshot(&request);
    print_snapshot(json, &snap);
    if snap.hosts.iter().any(|h| h.error.is_some()) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

pub(crate) fn snapshot_request(
    socket: Option<String>,
    config_path: Option<std::path::PathBuf>,
    host_filter: Option<String>,
    force_local: bool,
) -> Result<SnapshotRequest, (String, u8)> {
    if force_local || socket.is_some() {
        return local_request(socket)
            .map_err(|error| (format!("cannot read local hostname: {error}"), 1));
    }
    let loaded = match config_path {
        Some(path) => config::load(&path).map(Some),
        None => match config::default_path() {
            Some(path) => config::load_implicit(&path),
            None => Ok(None),
        },
    };
    let config = match loaded {
        Ok(Some(config)) => config,
        Ok(None) if host_filter.is_none() => {
            return local_request(None)
                .map_err(|error| (format!("cannot read local hostname: {error}"), 1));
        }
        Ok(None) => {
            return Err(("--host requires a configured inventory".into(), 2));
        }
        Err(error) => {
            return Err((error, 2));
        }
    };
    if let Some(id) = &host_filter
        && !config.machines.contains_key(id)
    {
        return Err((format!("unknown host id: {id}"), 2));
    }
    let machines = config
        .machines
        .iter()
        .filter(|(id, _)| host_filter.as_ref().is_none_or(|filter| filter == *id))
        .map(|(id, machine)| {
            (
                id.clone(),
                machine.ssh_alias.clone(),
                machine.socket.clone(),
            )
        })
        .collect();
    Ok(SnapshotRequest {
        machines,
        timeout_secs: config.client.connect_timeout_seconds,
        parallel: config.client.max_parallel_probes,
    })
}

fn local_request(socket: Option<String>) -> Result<SnapshotRequest, std::io::Error> {
    Ok(SnapshotRequest {
        machines: vec![(local_hostname()?, None, socket)],
        timeout_secs: 3,
        parallel: 1,
    })
}

fn local_hostname() -> Result<String, std::io::Error> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}
pub(crate) fn discover_snapshot(request: &SnapshotRequest) -> Snapshot {
    let results = Arc::new(Mutex::new(
        (0..request.machines.len())
            .map(|_| None)
            .collect::<Vec<Option<Host>>>(),
    ));
    let next = Arc::new(Mutex::new(0usize));
    std::thread::scope(|scope| {
        for _ in 0..request.parallel.min(request.machines.len().max(1)) {
            let results = Arc::clone(&results);
            let next = Arc::clone(&next);
            let machines = &request.machines;
            let timeout_secs = request.timeout_secs;
            scope.spawn(move || {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        if *n >= machines.len() {
                            break;
                        }
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let (id, alias, socket) = &machines[i];
                    let r = tmux::discover(
                        socket.as_deref(),
                        alias.as_deref(),
                        Duration::from_secs(timeout_secs),
                    );
                    let h = match r {
                        Ok(s) => Host {
                            id: id.clone(),
                            status: "ok",
                            sessions: s,
                            error: None,
                        },
                        Err(e) => {
                            eprintln!("error: {id}: {}: {:?}", e.code, e.message);
                            Host {
                                id: id.clone(),
                                status: "error",
                                sessions: vec![],
                                error: Some(HostError {
                                    code: e.code,
                                    message: e.message,
                                }),
                            }
                        }
                    };
                    results.lock().unwrap()[i] = Some(h);
                }
            });
        }
    });
    let hosts = results
        .lock()
        .unwrap()
        .iter_mut()
        .map(|h| h.take().unwrap())
        .collect::<Vec<_>>();
    Snapshot {
        schema_version: 1,
        observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        hosts,
    }
}

fn print_snapshot(json: bool, snap: &Snapshot) {
    if json {
        println!("{}", serde_json::to_string(&snap).unwrap());
    } else {
        for h in &snap.hosts {
            println!("{} ({})", serde_json::to_string(&h.id).unwrap(), h.status);
            if h.sessions.is_empty() && h.error.is_none() {
                println!("  no tmux sessions");
            }
            for s in &h.sessions {
                println!(
                    "  {}: {} windows, {} attached clients",
                    serde_json::to_string(&s.name).unwrap(),
                    s.windows,
                    s.attached_clients
                );
            }
            if let Some(e) = &h.error {
                println!(
                    "  error {}: {}",
                    e.code,
                    serde_json::to_string(&e.message).unwrap()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_escapes() {
        let s = Snapshot {
            schema_version: 1,
            observed_at_unix_ms: 1,
            hosts: vec![],
        };
        assert!(!serde_json::to_string(&s).unwrap().contains('\n'));
    }
}
