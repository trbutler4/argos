use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::CompleteEnv;
use clap_complete::engine::ArgValueCandidates;
pub(crate) use discovery::{Host, Snapshot, SnapshotRequest, discover_snapshot, snapshot_request};
use std::process::ExitCode;

mod completions;
mod config;
#[cfg(target_os = "linux")]
mod connect;
mod discovery;
#[cfg(target_os = "linux")]
mod terminal;
#[cfg(not(target_os = "linux"))]
mod connect {
    pub(crate) fn check_origin() -> Result<(), String> {
        Err("managed connections currently require Linux".into())
    }
    pub(crate) fn run(
        _: Option<String>,
        _: crate::sessions::BrowseOptions,
    ) -> std::process::ExitCode {
        eprintln!("error: managed connect currently requires Linux");
        std::process::ExitCode::from(1)
    }
    pub(crate) fn run_target(
        _: Option<crate::sessions::SessionTarget>,
        options: crate::sessions::BrowseOptions,
    ) -> std::process::ExitCode {
        run(None, options)
    }
}
mod host;
mod process;
mod sessions;
mod ssh;
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
    /// Open the managed session picker or attach to a host/VM target.
    Connect {
        #[arg(add = ArgValueCandidates::new(completions::attach_targets))]
        target: Option<String>,
        #[arg(long, conflicts_with_all=["config", "host", "local"], value_hint = ValueHint::FilePath)]
        socket: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "local"], value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, conflicts_with_all=["socket", "local"], add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
        #[arg(long, value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
    },
    /// List configured tmux sessions.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"],
              value_hint = ValueHint::FilePath)]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"],
              value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"],
              add = ArgValueCandidates::new(completions::hosts))]
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
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"],
              value_hint = ValueHint::FilePath)]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"],
              value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"],
              add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
        #[arg(long, value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
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
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"],
              value_hint = ValueHint::FilePath)]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"],
              value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"],
              add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
    },
    /// Attach to a host tmux session or VM session.
    Attach {
        #[arg(add = ArgValueCandidates::new(completions::attach_targets))]
        target: String,
        #[arg(long, value_name="PATH", conflicts_with_all=["config", "host", "local"],
              value_hint = ValueHint::FilePath)]
        socket: Option<String>,
        #[arg(long, value_name="PATH", conflicts_with_all=["socket", "local"],
              value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name="ID", conflicts_with_all=["socket", "local"],
              add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, conflicts_with_all=["socket", "config", "host"])]
        local: bool,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
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
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "ID", add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum VmCommand {
    /// Fetch and SHA-256 verify a file from a running VM without changing guest state.
    Fetch {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        guest_path: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        output: std::path::PathBuf,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List VM records in this host's Argos state directory.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        detailed: bool,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
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
        #[arg(long, value_name = "HOST", add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
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
        #[arg(long, value_name = "HOST", add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        no_attach: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one VM record, including workspace and exposed links.
    Show {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Reapply this VM's repo profile or an explicit .argos.toml profile.
    Update {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Build and start a local VM by id.
    Start {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print or follow a local VM's console log.
    Logs {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = 200)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    /// Stop a local VM by id.
    Stop {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Stop then start a local VM by id.
    Restart {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Stop and remove a local VM's record, instance data, and work directory.
    Rm {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
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
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
    },
    /// SSH into a local VM guest and join its tmux session.
    Tmux {
        #[arg(add = ArgValueCandidates::new(completions::vm_ids))]
        id: String,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
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
        #[arg(long, value_name = "HOST", add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
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
        #[arg(long, value_name = "HOST", add = ArgValueCandidates::new(completions::hosts))]
        host: Option<String>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        state_dir: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::DirPath)]
        work_root: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        config: Option<std::path::PathBuf>,
        #[arg(long, value_name = "PATH", value_hint = ValueHint::FilePath)]
        profile: Option<std::path::PathBuf>,
        #[arg(long)]
        no_attach: bool,
        #[arg(long)]
        json: bool,
    },
}
fn main() -> ExitCode {
    // Must run before parsing: when the shell invokes the completer this exits
    // early and never reaches normal command dispatch.
    CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        let mut c = Cli::command();
        let _ = c.print_help();
        println!();
        return ExitCode::SUCCESS;
    };
    match command {
        Command::Connect {
            target,
            socket,
            config,
            host,
            local,
            state_dir,
        } => connect::run(
            target,
            sessions::BrowseOptions {
                socket,
                config_path: config,
                host,
                local,
                state_dir,
            },
        ),
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
            } => connect::run(
                Some(target),
                sessions::BrowseOptions {
                    socket,
                    config_path: config,
                    host,
                    local,
                    state_dir,
                },
            ),
        },
        Command::Tui {
            socket,
            config,
            host,
            local,
            state_dir,
        } => connect::run(
            None,
            sessions::BrowseOptions {
                socket,
                config_path: config,
                host,
                local,
                state_dir,
            },
        ),
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
            VmCommand::Fetch {
                id,
                guest_path,
                output,
                state_dir,
                json,
            } => vm::artifact::fetch(vm::artifact::FetchOptions {
                id,
                guest_path,
                output,
                state_dir,
                json,
            }),
            VmCommand::List {
                json,
                detailed,
                state_dir,
            } => vm::list(json, detailed, state_dir),
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
            VmCommand::Restart {
                id,
                state_dir,
                config,
                json,
            } => vm::restart(vm::RestartOptions {
                id,
                state_dir,
                config,
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
                mode: sessions::GuestMode::Shell,
            }),
            VmCommand::Tmux { id, state_dir } => vm::guest_command(vm::GuestCommandOptions {
                id,
                state_dir,
                mode: sessions::GuestMode::Tmux,
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
