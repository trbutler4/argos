//! Persisted VM records: schema, validation, reads, and atomic publication.
use super::{VmError, invalid_args, now_ms, state_error, valid_package_attr, valid_port_name};
use crate::{host, process::Cancellation};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VmSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) state_root: String,
    pub(crate) vms: Vec<VmRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct VmRecord {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) host: String,
    #[serde(default)]
    pub(crate) project: Option<String>,
    #[serde(default)]
    pub(crate) source_repo: Option<String>,
    #[serde(default)]
    pub(crate) profile_path: Option<String>,
    #[serde(default)]
    pub(crate) guest_workdir: Option<String>,
    #[serde(default)]
    pub(crate) packages: Vec<String>,
    #[serde(default)]
    pub(crate) ports: Vec<VmPortMapping>,
    #[serde(default)]
    pub(crate) repo_path: Option<String>,
    #[serde(default)]
    pub(crate) workdir: Option<String>,
    #[serde(default)]
    pub(crate) instance_dir: Option<String>,
    #[serde(default)]
    pub(crate) flake_path: Option<String>,
    #[serde(default)]
    pub(crate) microvm_config: Option<String>,
    #[serde(default)]
    pub(crate) runner_path: Option<String>,
    #[serde(default)]
    pub(crate) log_path: Option<String>,
    #[serde(default)]
    pub(crate) pid: Option<u32>,
    #[serde(default)]
    pub(crate) console_session: Option<String>,
    #[serde(default)]
    pub(crate) ssh_host: Option<String>,
    #[serde(default)]
    pub(crate) ssh_port: Option<u16>,
    #[serde(default)]
    pub(crate) ssh_user: Option<String>,
    #[serde(default)]
    pub(crate) created_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub(crate) updated_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub(crate) started_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct VmPortMapping {
    pub(crate) name: String,
    pub(crate) guest: u16,
    pub(crate) host: u16,
}

pub(super) fn state_root(override_dir: Option<PathBuf>) -> Result<PathBuf, VmError> {
    if let Some(path) = override_dir {
        if !path.is_absolute() {
            return Err(invalid_args("--state-dir must be absolute"));
        }
        return Ok(path);
    }
    Ok(host::state_root())
}

pub(super) fn load_snapshot(root: &Path) -> Result<VmSnapshot, VmError> {
    load_snapshot_cancellable(root, &Cancellation::default())
}

pub(super) fn load_snapshot_cancellable(
    root: &Path,
    cancel: &Cancellation,
) -> Result<VmSnapshot, VmError> {
    let mut vms = load_vms_cancellable(&root.join("vms"), cancel)?;
    vms.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(VmSnapshot {
        schema_version: 1,
        observed_at_unix_ms: now_ms(),
        state_root: root.display().to_string(),
        vms,
    })
}

pub(super) fn load_vms(dir: &Path) -> Result<Vec<VmRecord>, VmError> {
    load_vms_cancellable(dir, &Cancellation::default())
}

fn load_vms_cancellable(dir: &Path, cancel: &Cancellation) -> Result<Vec<VmRecord>, VmError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(state_error(format!(
                "cannot read VM state directory {}: {error}",
                dir.display()
            )));
        }
    };
    let mut records = Vec::new();
    for entry in entries {
        if cancel.is_cancelled() {
            return Err(VmError {
                code: "cancelled",
                message: "VM discovery cancelled".into(),
            });
        }
        let entry = entry.map_err(|error| {
            state_error(format!(
                "cannot read VM state entry in {}: {error}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        records.push(load_record(&path)?);
    }
    Ok(records)
}

pub(super) fn load_record(path: &Path) -> Result<VmRecord, VmError> {
    let text = fs::read_to_string(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => VmError {
            code: "not_found",
            message: format!("VM state does not exist: {}", path.display()),
        },
        _ => state_error(format!("cannot read VM state {}: {error}", path.display())),
    })?;
    let record: VmRecord = serde_json::from_str(&text).map_err(|error| VmError {
        code: "invalid_state",
        message: format!("invalid VM state {}: {error}", path.display()),
    })?;
    validate_record(&record, path)?;
    Ok(record)
}

pub(super) fn write_record(path: &Path, record: &VmRecord) -> Result<(), VmError> {
    validate_record(record, path)?;
    let json = serde_json::to_string_pretty(record).unwrap();
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, format!("{json}\n")).map_err(|error| {
        state_error(format!(
            "cannot write temporary VM state {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, path).map_err(|error| {
        state_error(format!(
            "cannot publish VM state {}: {error}",
            path.display()
        ))
    })
}

pub(super) fn validate_record(record: &VmRecord, path: &Path) -> Result<(), VmError> {
    if record.schema_version != 1 {
        return Err(invalid(path, "unsupported schema_version"));
    }
    for (field, value) in [
        ("id", record.id.as_str()),
        ("name", record.name.as_str()),
        ("status", record.status.as_str()),
        ("host", record.host.as_str()),
    ] {
        if value.is_empty() || value.chars().any(|c| c == '\0' || c.is_control()) {
            return Err(invalid(path, &format!("invalid {field}")));
        }
    }
    if !valid_id(&record.id) {
        return Err(invalid(path, "invalid id"));
    }
    if let Some(workdir) = &record.guest_workdir
        && (!workdir.starts_with('/') || workdir.chars().any(|c| c == '\0' || c.is_control()))
    {
        return Err(invalid(path, "invalid guest_workdir"));
    }
    let mut packages = BTreeSet::new();
    for package in &record.packages {
        if !valid_package_attr(package) || !packages.insert(package) {
            return Err(invalid(path, "invalid package"));
        }
    }
    let (mut port_names, mut guest_ports, mut host_ports) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for mapping in &record.ports {
        if !valid_port_name(&mapping.name)
            || mapping.guest == 0
            || mapping.host == 0
            || !port_names.insert(mapping.name.as_str())
            || !guest_ports.insert(mapping.guest)
            || !host_ports.insert(mapping.host)
        {
            return Err(invalid(path, "invalid port mapping"));
        }
    }
    Ok(())
}

fn invalid(path: &Path, reason: &str) -> VmError {
    VmError {
        code: "invalid_state",
        message: format!("invalid VM state {}: {reason}", path.display()),
    }
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
