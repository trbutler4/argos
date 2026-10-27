//! Shared CLI discovery and one cancellable, bounded picker refresh job.
use crate::{config, jcode, process::Cancellation, ssh, tmux, vm};
use serde::Serialize;
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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

#[derive(Clone)]
pub(crate) struct ProbeTarget {
    pub(crate) id: String,
    pub(crate) ssh_alias: Option<String>,
    pub(crate) socket: Option<String>,
}

#[derive(Clone)]
pub(crate) struct SnapshotRequest {
    pub(crate) machines: Vec<ProbeTarget>,
    pub(crate) timeout_secs: u64,
    pub(crate) parallel: usize,
}

pub(crate) fn snapshot_request(
    socket: Option<String>,
    config_path: Option<PathBuf>,
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
        Ok(None) => return Err(("--host requires a configured inventory".into(), 2)),
        Err(error) => return Err((error, 2)),
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
        .map(|(id, machine)| ProbeTarget {
            id: id.clone(),
            ssh_alias: machine.ssh_alias.clone(),
            socket: machine.socket.clone(),
        })
        .collect();
    Ok(SnapshotRequest {
        machines,
        timeout_secs: config.client.connect_timeout_seconds,
        parallel: config.client.max_parallel_probes,
    })
}

fn local_request(socket: Option<String>) -> io::Result<SnapshotRequest> {
    let id = std::fs::read_to_string("/proc/sys/kernel/hostname")?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    Ok(SnapshotRequest {
        machines: vec![ProbeTarget {
            id,
            ssh_alias: None,
            socket,
        }],
        timeout_secs: 3,
        parallel: 1,
    })
}

pub(crate) fn observed_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy)]
enum ProbePolicy {
    ReadOnly,
    ForAttachment,
}

fn probe(
    target: &ProbeTarget,
    timeout: Duration,
    cancel: &Cancellation,
    policy: ProbePolicy,
) -> Host {
    let deadline = Instant::now() + timeout;
    let result = (|| {
        if let ProbePolicy::ForAttachment = policy
            && let Some(alias) = &target.ssh_alias
        {
            ssh::validate_destination_cancellable(alias, deadline, cancel).map_err(|error| {
                HostError {
                    code: error.code().into(),
                    message: error.to_string(),
                }
            })?;
        }
        tmux::discover_cancellable(
            target.socket.as_deref(),
            target.ssh_alias.as_deref(),
            deadline.saturating_duration_since(Instant::now()),
            cancel,
        )
        .map_err(|error| HostError {
            code: error.code,
            message: error.message,
        })
    })();
    match result {
        Ok(sessions) => Host {
            id: target.id.clone(),
            status: "ok",
            sessions,
            error: None,
        },
        Err(error) => Host {
            id: target.id.clone(),
            status: "error",
            sessions: Vec::new(),
            error: Some(error),
        },
    }
}

fn probe_hosts(
    request: &SnapshotRequest,
    cancel: &Cancellation,
    policy: ProbePolicy,
    publish: impl Fn(Host) + Sync,
) {
    // Local data should not queue behind unreachable remote hosts.
    let targets: Vec<_> = request
        .machines
        .iter()
        .filter(|target| target.ssh_alias.is_none())
        .chain(
            request
                .machines
                .iter()
                .filter(|target| target.ssh_alias.is_some()),
        )
        .collect();
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..request.parallel.max(1).min(request.machines.len()) {
            let (next, publish, targets) = (&next, &publish, &targets);
            scope.spawn(move || {
                while !cancel.is_cancelled() {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(target) = targets.get(index) else {
                        break;
                    };
                    let host = probe(
                        target,
                        Duration::from_secs(request.timeout_secs),
                        cancel,
                        policy,
                    );
                    if !cancel.is_cancelled() {
                        publish(host);
                    }
                }
            });
        }
    });
}

pub(crate) fn discover_snapshot(request: &SnapshotRequest) -> Snapshot {
    let (tx, rx) = mpsc::channel();
    probe_hosts(
        request,
        &Cancellation::default(),
        ProbePolicy::ReadOnly,
        |host| {
            let _ = tx.send(host);
        },
    );
    drop(tx);
    let mut hosts: Vec<Host> = rx.into_iter().collect();
    hosts.sort_by(|a, b| a.id.cmp(&b.id));
    for host in &hosts {
        if let Some(error) = &host.error {
            eprintln!("error: {}: {}: {:?}", host.id, error.code, error.message);
        }
    }
    Snapshot {
        schema_version: 1,
        observed_at_unix_ms: observed_now(),
        hosts,
    }
}

pub(crate) enum Update {
    Host(Host),
    Vms(Result<vm::VmSnapshot, vm::VmError>),
    Agents(jcode::Guest),
    Finished,
}

/// Exactly one job per picker. Drop cancels probes, closes the bounded channel
/// (unblocking producers), and joins all workers before attachment can begin.
pub(crate) struct RefreshJob {
    cancel: Cancellation,
    receiver: Option<mpsc::Receiver<Update>>,
    worker: Option<JoinHandle<()>>,
}

impl RefreshJob {
    pub(crate) fn start(request: SnapshotRequest, state_dir: Option<PathBuf>) -> io::Result<Self> {
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(16);
        let worker = thread::Builder::new()
            .name("argos-discovery".into())
            .spawn(move || {
                thread::scope(|scope| {
                    scope.spawn(|| {
                        let result = vm::snapshot_cancellable(state_dir, &worker_cancel);
                        if !worker_cancel.is_cancelled() {
                            let guests = result.as_ref().ok().cloned();
                            let _ = tx.send(Update::Vms(result));
                            if let Some(snapshot) = guests {
                                jcode::probe_guests(
                                    &snapshot,
                                    request.parallel,
                                    Duration::from_secs(request.timeout_secs),
                                    &worker_cancel,
                                    |guest| {
                                        let _ = tx.send(Update::Agents(guest));
                                    },
                                );
                            }
                        }
                    });
                    probe_hosts(
                        &request,
                        &worker_cancel,
                        ProbePolicy::ForAttachment,
                        |host| {
                            let _ = tx.send(Update::Host(host));
                        },
                    );
                });
                let _ = tx.send(Update::Finished);
            })?;
        Ok(Self {
            cancel,
            receiver: Some(rx),
            worker: Some(worker),
        })
    }

    pub(crate) fn try_recv(&self) -> Result<Update, mpsc::TryRecvError> {
        self.receiver
            .as_ref()
            .expect("active refresh receiver")
            .try_recv()
    }

    pub(crate) fn stop(&mut self) -> io::Result<()> {
        self.cancel.cancel();
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("discovery worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for RefreshJob {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
