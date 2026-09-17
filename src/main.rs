use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::{
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod config;
mod host;
mod sessions;
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
    /// Work with attachable sessions, including host tmux and VM tmux.
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
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
    /// Create task-scoped VM workspaces from repos.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Subcommand, Debug)]
enum SessionsCommand {
    /// List host tmux sessions.
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
    /// Attach to a host tmux session or VM session.
    Attach {
        target: String,
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"])]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"])]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"])]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
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
    /// Create a local VM record, work directory, optional repo clone, and microVM config.
    Create {
        name: String,
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create if needed, start, then attach to the VM-owned tmux session.
    Up {
        name: String,
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        no_attach: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one VM record, including workspace and exposed links.
    Show {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Reapply this VM's repo profile or an explicit .argos.toml profile.
    Update {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Build and start a local VM by id.
    Start {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print or follow a local VM's console log.
    Logs {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = 200)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    /// Stop a local VM by id.
    Stop {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Stop and remove a local VM's record, instance data, and work directory.
    Rm {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// SSH into a local VM guest shell.
    Shell {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
    },
    /// SSH into a local VM guest and join its tmux session.
    Tmux {
        id: String,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum TaskCommand {
    /// Create a stopped VM workspace for one repo task.
    Create {
        task: String,
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create if needed, start, then attach to a task VM workspace.
    Up {
        task: String,
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
        #[arg(long, value_name = "ID")]
        id: Option<String>,
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        #[arg(long, value_name = "PATH")]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH")]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        no_attach: bool,
        #[arg(long)]
        json: bool,
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
        Command::Sessions { command } => match command {
            SessionsCommand::List {
                json,
                socket,
                config,
                host,
                local,
            } => run_list(json, socket, config, host, local),
            SessionsCommand::Attach {
                target,
                socket,
                config,
                host,
                local,
                state_dir,
            } => sessions::attach(sessions::AttachOptions {
                target,
                socket,
                config_path: config,
                host,
                local,
                state_dir,
            }),
        },
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
            VmCommand::Create {
                name,
                id,
                repo,
                project,
                host,
                state_dir,
                work_root,
                config,
                profile,
                dry_run,
                json,
            } => vm::create(vm::CreateOptions {
                name,
                id,
                repo,
                project,
                host,
                state_dir,
                work_root,
                config,
                profile,
                dry_run,
                json,
            }),
            VmCommand::Show {
                id,
                state_dir,
                json,
            } => vm::show(vm::ShowOptions {
                id,
                state_dir,
                json,
            }),
            VmCommand::Update {
                id,
                state_dir,
                config,
                profile,
                dry_run,
                json,
            } => vm::update(vm::UpdateOptions {
                id,
                state_dir,
                config,
                profile,
                dry_run,
                json,
            }),
            VmCommand::Up {
                name,
                id,
                repo,
                project,
                host,
                state_dir,
                work_root,
                config,
                profile,
                no_attach,
                json,
            } => vm::up(vm::UpOptions {
                name,
                id,
                repo,
                project,
                host,
                state_dir,
                work_root,
                config,
                profile,
                no_attach,
                json,
            }),
            VmCommand::Start {
                id,
                state_dir,
                config,
                json,
            } => vm::start(vm::StartOptions {
                id,
                state_dir,
                config,
                json,
            }),
            VmCommand::Logs {
                id,
                state_dir,
                lines,
                follow,
            } => vm::logs(vm::LogsOptions {
                id,
                state_dir,
                lines,
                follow,
            }),
            VmCommand::Stop {
                id,
                state_dir,
                json,
            } => vm::stop(vm::StopOptions {
                id,
                state_dir,
                json,
            }),
            VmCommand::Rm {
                id,
                state_dir,
                force,
                dry_run,
                json,
            } => vm::remove(vm::RemoveOptions {
                id,
                state_dir,
                force,
                dry_run,
                json,
            }),
            VmCommand::Shell { id, state_dir } => vm::guest_command(vm::GuestCommandOptions {
                id,
                state_dir,
                tmux: false,
            }),
            VmCommand::Tmux { id, state_dir } => vm::guest_command(vm::GuestCommandOptions {
                id,
                state_dir,
                tmux: true,
            }),
        },
        Command::Task { command } => match command {
            TaskCommand::Create {
                task,
                repo,
                project,
                id,
                host,
                state_dir,
                work_root,
                config,
                profile,
                dry_run,
                json,
            } => vm::task_create(vm::TaskCreateOptions {
                task,
                repo,
                project,
                id,
                host,
                state_dir,
                work_root,
                config,
                profile,
                dry_run,
                json,
            }),
            TaskCommand::Up {
                task,
                repo,
                project,
                id,
                host,
                state_dir,
                work_root,
                config,
                profile,
                no_attach,
                json,
            } => vm::task_up(vm::TaskUpOptions {
                task,
                repo,
                project,
                id,
                host,
                state_dir,
                work_root,
                config,
                profile,
                no_attach,
                json,
            }),
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
