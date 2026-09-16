use crate::host;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize)]
struct VmSnapshot {
    schema_version: u32,
    observed_at_unix_ms: u64,
    state_root: String,
    vms: Vec<VmRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct VmRecord {
    schema_version: u32,
    id: String,
    name: String,
    status: String,
    host: String,
    project: Option<String>,
    repo_path: Option<String>,
    workdir: Option<String>,
    created_at_unix_ms: Option<u64>,
    updated_at_unix_ms: Option<u64>,
}

#[derive(Debug)]
struct VmError {
    code: &'static str,
    message: String,
}

pub(crate) fn list(json: bool, state_dir: Option<PathBuf>) -> ExitCode {
    let state_root = match state_root(state_dir) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let snapshot = match load_snapshot(&state_root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            return ExitCode::from(1);
        }
    };
    if json {
        println!("{}", serde_json::to_string(&snapshot).unwrap());
    } else {
        print_human(&snapshot);
    }
    ExitCode::SUCCESS
}

fn state_root(override_dir: Option<PathBuf>) -> Result<PathBuf, VmError> {
    if let Some(path) = override_dir {
        if !path.is_absolute() {
            return Err(VmError {
                code: "invalid_state_dir",
                message: "--state-dir must be absolute".into(),
            });
        }
        return Ok(path);
    }
    Ok(host::state_root())
}

fn load_snapshot(state_root: &Path) -> Result<VmSnapshot, VmError> {
    let mut vms = load_vms(&state_root.join("vms"))?;
    vms.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(VmSnapshot {
        schema_version: 1,
        observed_at_unix_ms: now_ms(),
        state_root: state_root.display().to_string(),
        vms,
    })
}

fn load_vms(dir: &Path) -> Result<Vec<VmRecord>, VmError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(VmError {
                code: "state_error",
                message: format!("cannot read VM state directory {}: {error}", dir.display()),
            });
        }
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| VmError {
            code: "state_error",
            message: format!("cannot read VM state entry in {}: {error}", dir.display()),
        })?;
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|error| VmError {
            code: "state_error",
            message: format!("cannot read VM state {}: {error}", path.display()),
        })?;
        let record: VmRecord = serde_json::from_str(&text).map_err(|error| VmError {
            code: "invalid_state",
            message: format!("invalid VM state {}: {error}", path.display()),
        })?;
        validate_record(&record, &path)?;
        records.push(record);
    }
    Ok(records)
}

fn validate_record(record: &VmRecord, path: &Path) -> Result<(), VmError> {
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
    Ok(())
}

fn invalid(path: &Path, reason: &str) -> VmError {
    VmError {
        code: "invalid_state",
        message: format!("invalid VM state {}: {reason}", path.display()),
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn print_human(snapshot: &VmSnapshot) {
    println!("state: {}", snapshot.state_root);
    if snapshot.vms.is_empty() {
        println!("no VMs");
        return;
    }
    for vm in &snapshot.vms {
        println!(
            "{} ({}) on {}",
            serde_json::to_string(&vm.name).unwrap(),
            vm.status,
            vm.host
        );
        println!("  id: {}", vm.id);
        if let Some(project) = &vm.project {
            println!("  project: {}", serde_json::to_string(project).unwrap());
        }
        if let Some(repo_path) = &vm.repo_path {
            println!("  repo: {}", serde_json::to_string(repo_path).unwrap());
        }
        if let Some(workdir) = &vm.workdir {
            println!("  workdir: {}", serde_json::to_string(workdir).unwrap());
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ids() {
        assert!(valid_id("trade-feature_1"));
        assert!(!valid_id("bad/slash"));
        assert!(!valid_id(""));
    }

    #[test]
    fn sorts_by_id() {
        let root =
            std::env::temp_dir().join(format!("argos-vm-test-{}-{}", std::process::id(), now_ms()));
        let vms = root.join("vms");
        fs::create_dir_all(&vms).unwrap();
        fs::write(
            vms.join("b.json"),
            r#"{"schema_version":1,"id":"b","name":"Bee","status":"stopped","host":"here","project":null,"repo_path":null,"workdir":null,"created_at_unix_ms":null,"updated_at_unix_ms":null}"#,
        )
        .unwrap();
        fs::write(
            vms.join("a.json"),
            r#"{"schema_version":1,"id":"a","name":"Aye","status":"running","host":"here","project":null,"repo_path":null,"workdir":null,"created_at_unix_ms":null,"updated_at_unix_ms":null}"#,
        )
        .unwrap();
        let snapshot = load_snapshot(&root).unwrap();
        assert_eq!(snapshot.vms[0].id, "a");
        assert_eq!(snapshot.vms[1].id, "b");
        fs::remove_dir_all(root).unwrap();
    }
}
