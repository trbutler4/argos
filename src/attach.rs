use crate::{config, tmux};
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

pub(crate) fn run(
    session: &str,
    socket: Option<String>,
    config_path: Option<PathBuf>,
    host: Option<String>,
    local: bool,
) -> ExitCode {
    let mut chosen = match resolve_socket(socket, config_path, host, local) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("error: {e}");
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
    if let Some(env) = &inside {
        if !same_socket(&env.socket, chosen.as_deref()) {
            eprintln!("error: refusing to nest across different tmux servers");
            return ExitCode::from(1);
        }
        // Use the parsed socket explicitly, including paths containing commas.
        chosen = Some(env.socket.clone());
    }
    let sessions = match tmux::discover(chosen.as_deref(), None, Duration::from_secs(3)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}: {}", e.code, e.message);
            return ExitCode::from(1);
        }
    };
    let target = match select(&sessions, session) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    if let Some(env) = &inside {
        return switch_inside(env, &target.id);
    }
    let mut command = Command::new("tmux");
    command.args(["-u", "-N"]);
    if let Some(path) = chosen {
        command.args(["-S", &path]);
    }
    command.args(["attach-session", "-t", &target.id]);
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

fn resolve_socket(
    socket: Option<String>,
    config_path: Option<PathBuf>,
    host: Option<String>,
    local: bool,
) -> Result<Option<String>, String> {
    if local || socket.is_some() {
        return Ok(socket);
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
            return Ok(None);
        }
    };
    let id = host.as_deref().unwrap_or(&cfg.client.machine_id);
    let machine = cfg
        .machines
        .get(id)
        .ok_or_else(|| format!("unknown host id: {id}"))?;
    if machine.ssh_alias.is_some() {
        return Err("remote attachment is not implemented yet".into());
    }
    Ok(machine.socket.clone())
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
    fn env_parsing_allows_commas() {
        let e = parse_tmux_env("/tmp/a,b,12,7", Some("%3")).unwrap();
        assert_eq!(e.socket, "/tmp/a,b");
    }
    #[test]
    fn env_rejects_bad_pane() {
        assert!(parse_tmux_env("/tmp/a,1,2,%x", None).is_err());
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
}
