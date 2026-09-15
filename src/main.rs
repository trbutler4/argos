use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use serde::Serialize;

mod tmux;

#[derive(Parser, Debug)]
#[command(
    name = "cmd-center",
    version,
    about = "A local command center for tmux work"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List local tmux sessions.
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "PATH")]
        socket: Option<String>,
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
        let mut command = Cli::command();
        let _ = command.print_help();
        println!();
        return ExitCode::SUCCESS;
    };

    match command {
        Command::List { json, socket } => run_list(json, socket),
    }
}

fn run_list(json: bool, socket: Option<String>) -> ExitCode {
    let observed_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let hostname = match std::fs::read_to_string("/proc/sys/kernel/hostname") {
        Ok(value) => value.trim_end_matches(['\r', '\n']).to_owned(),
        Err(error) => {
            eprintln!("error: cannot read local hostname: {error}");
            return ExitCode::from(1);
        }
    };

    let result = tmux::list_sessions(socket.as_deref());
    let (host, failed) = match result {
        Ok(sessions) => (
            Host {
                id: hostname,
                status: "ok",
                sessions,
                error: None,
            },
            false,
        ),
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            (
                Host {
                    id: hostname,
                    status: "error",
                    sessions: Vec::new(),
                    error: Some(HostError {
                        code: error.code,
                        message: error.message,
                    }),
                },
                true,
            )
        }
    };
    let snapshot = Snapshot {
        schema_version: 1,
        observed_at_unix_ms,
        hosts: vec![host],
    };
    if json {
        match serde_json::to_string(&snapshot) {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("error: cannot encode JSON: {error}");
                return ExitCode::from(1);
            }
        }
    } else {
        let host = &snapshot.hosts[0];
        println!("{} ({})", host.id, host.status);
        if host.sessions.is_empty() && !failed {
            println!("  no tmux sessions");
        }
        for session in &host.sessions {
            println!(
                "  {}: {} windows, {} attached clients",
                serde_json::to_string(&session.name).unwrap_or_default(),
                session.windows,
                session.attached_clients
            );
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
    fn json_escapes_control_characters() {
        let snapshot = Snapshot {
            schema_version: 1,
            observed_at_unix_ms: 1,
            hosts: vec![Host {
                id: "host".into(),
                status: "ok",
                sessions: vec![tmux::Session {
                    id: "$1".into(),
                    name: "a\t\n\"".into(),
                    windows: 1,
                    attached_clients: 0,
                }],
                error: None,
            }],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("a\\t\\n\\\""));
        assert!(!json.contains('\n'));
    }
}
