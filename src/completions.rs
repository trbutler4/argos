//! Dynamic shell completion candidates.
//!
//! Completers run on every Tab press, so they must stay fast and side-effect
//! free. They only read local state: the private inventory file and this host's
//! Argos state directory. They never open SSH connections, never mutate hosts,
//! and never fail the shell: any error yields an empty candidate list.

use clap_complete::engine::CompletionCandidate;
use std::time::Duration;

use crate::{config, tmux, vm};

/// Local tmux discovery is best effort during completion.
const LOCAL_TMUX_TIMEOUT: Duration = Duration::from_millis(250);

/// Configured machine IDs from the private inventory, for `--host`.
pub(crate) fn hosts() -> Vec<CompletionCandidate> {
    let Ok(Some((config, _))) = config::load_selected(None) else {
        return Vec::new();
    };
    config
        .machines
        .iter()
        .map(|(id, machine)| {
            let help = match machine.ssh_alias.as_deref() {
                Some(alias) => format!("ssh {alias}"),
                None => "local".to_string(),
            };
            CompletionCandidate::new(id).help(Some(help.into()))
        })
        .collect()
}

/// VM IDs recorded in this host's Argos state directory.
pub(crate) fn vm_ids() -> Vec<CompletionCandidate> {
    vm_records()
        .into_iter()
        .map(|(id, name, status)| {
            CompletionCandidate::new(id).help(Some(format!("{name} ({status})").into()))
        })
        .collect()
}

/// Attachable entry points for `argos sessions attach`.
///
/// Offers `vm:<id>` for every local VM record, `host:<host>:` prefixes for
/// every configured machine, and bare local tmux session names. Remote host
/// sessions are deliberately omitted because enumerating them requires SSH.
pub(crate) fn attach_targets() -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    for (id, name, status) in vm_records() {
        candidates.push(
            CompletionCandidate::new(format!("vm:{id}"))
                .help(Some(format!("VM {name} ({status})").into())),
        );
    }
    if let Ok(Some((config, _))) = config::load_selected(None) {
        for id in config.machines.keys() {
            candidates.push(
                CompletionCandidate::new(format!("host:{id}:"))
                    .help(Some(format!("tmux session on {id}").into())),
            );
        }
    }
    for session in local_sessions() {
        candidates.push(CompletionCandidate::new(session.name).help(Some(
            format!("local tmux, {} windows", session.windows).into(),
        )));
    }
    candidates
}

fn vm_records() -> Vec<(String, String, String)> {
    let Ok(snapshot) = vm::snapshot(None) else {
        return Vec::new();
    };
    snapshot
        .vms
        .into_iter()
        .map(|record| (record.id, record.name, record.status))
        .collect()
}

fn local_sessions() -> Vec<tmux::Session> {
    tmux::discover(None, None, LOCAL_TMUX_TIMEOUT).unwrap_or_default()
}
