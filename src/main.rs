use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::{
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod attach;
mod config;
mod tmux;

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
}
#[derive(Serialize)]
struct Snapshot {
    schema_version: u32,
    observed_at_unix_ms: u64,
    hosts: Vec<Host>,
}
#[derive(Serialize)]
struct Host {
    id: String,
    status: &'static str,
    sessions: Vec<tmux::Session>,
    error: Option<HostError>,
}
#[derive(Serialize)]
struct HostError {
    code: String,
    message: String,
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
    }
}

fn run_list(
    json: bool,
    socket: Option<String>,
    config_path: Option<std::path::PathBuf>,
    host_filter: Option<String>,
    force_local: bool,
) -> ExitCode {
    if force_local || socket.is_some() {
        return run_local(json, socket);
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
        Ok(None) if host_filter.is_none() => return run_local(json, None),
        Ok(None) => {
            eprintln!("error: --host requires a configured inventory");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    if let Some(id) = &host_filter
        && !config.machines.contains_key(id)
    {
        eprintln!("error: unknown host id: {id}");
        return ExitCode::from(2);
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
    discover_and_print(
        json,
        machines,
        config.client.connect_timeout_seconds,
        config.client.max_parallel_probes,
    )
}

fn run_local(json: bool, socket: Option<String>) -> ExitCode {
    match local_hostname() {
        Ok(id) => discover_and_print(json, vec![(id, None, socket)], 3, 1),
        Err(error) => {
            eprintln!("error: cannot read local hostname: {error}");
            ExitCode::from(1)
        }
    }
}

fn local_hostname() -> Result<String, std::io::Error> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}
fn discover_and_print(
    json: bool,
    machines: Vec<(String, Option<String>, Option<String>)>,
    timeout_secs: u64,
    parallel: usize,
) -> ExitCode {
    let results = Arc::new(Mutex::new(
        (0..machines.len())
            .map(|_| None)
            .collect::<Vec<Option<Host>>>(),
    ));
    let next = Arc::new(Mutex::new(0usize));
    std::thread::scope(|scope| {
        for _ in 0..parallel.min(machines.len().max(1)) {
            let results = Arc::clone(&results);
            let next = Arc::clone(&next);
            let machines = &machines;
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
    let failed = hosts.iter().any(|h| h.error.is_some());
    let snap = Snapshot {
        schema_version: 1,
        observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        hosts,
    };
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
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
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
