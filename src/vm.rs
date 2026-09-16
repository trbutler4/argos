use crate::host;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
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
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    source_repo: Option<String>,
    #[serde(default)]
    repo_path: Option<String>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    microvm_config: Option<String>,
    #[serde(default)]
    created_at_unix_ms: Option<u64>,
    #[serde(default)]
    updated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct VmCreateOutput {
    schema_version: u32,
    dry_run: bool,
    state_root: String,
    state_file: String,
    instance_dir: String,
    workdir: String,
    repo_path: Option<String>,
    microvm_config: String,
    record: VmRecord,
}

#[derive(Debug)]
struct VmError {
    code: &'static str,
    message: String,
}

pub(crate) struct CreateOptions {
    pub(crate) name: String,
    pub(crate) id: Option<String>,
    pub(crate) repo: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) host: Option<String>,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) work_root: Option<PathBuf>,
    pub(crate) dry_run: bool,
    pub(crate) json: bool,
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

pub(crate) fn create(options: CreateOptions) -> ExitCode {
    let output = match create_plan(&options) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {}", error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            return ExitCode::from(code);
        }
    };
    if !options.dry_run
        && let Err(error) = create_vm_state(&output)
    {
        eprintln!("error: {}: {}", error.code, error.message);
        return ExitCode::from(1);
    }
    if options.json {
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_create(&output);
    }
    ExitCode::SUCCESS
}

fn create_plan(options: &CreateOptions) -> Result<VmCreateOutput, VmError> {
    let state_root = state_root(options.state_dir.clone())?;
    let id = match &options.id {
        Some(id) => id.clone(),
        None => slug(&options.name),
    };
    if !valid_id(&id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    let host = options
        .host
        .clone()
        .unwrap_or_else(|| host::current_status().hostname);
    let now = now_ms();
    let state_file = state_root.join("vms").join(format!("{id}.json"));
    if state_file.exists() {
        return Err(VmError {
            code: "conflict",
            message: format!("VM already exists: {id}"),
        });
    }
    let instance_dir = state_root.join("instances").join(&id);
    let microvm_config = instance_dir.join("microvm.nix");
    let work_root = match &options.work_root {
        Some(path) if !path.is_absolute() => {
            return Err(invalid_args("--work-root must be absolute"));
        }
        Some(path) => path.clone(),
        None => state_root.join("workdirs"),
    };
    let workdir = work_root.join(&id);
    let repo_path = options.repo.as_ref().map(|_| workdir.join("repo"));
    let record = VmRecord {
        schema_version: 1,
        id: id.clone(),
        name: options.name.clone(),
        status: "created".into(),
        host,
        project: options.project.clone(),
        source_repo: options.repo.clone(),
        repo_path: repo_path.as_ref().map(|path| path.display().to_string()),
        workdir: Some(workdir.display().to_string()),
        microvm_config: Some(microvm_config.display().to_string()),
        created_at_unix_ms: Some(now),
        updated_at_unix_ms: Some(now),
    };
    validate_record(&record, &state_file)?;
    Ok(VmCreateOutput {
        schema_version: 1,
        dry_run: options.dry_run,
        state_root: state_root.display().to_string(),
        state_file: state_file.display().to_string(),
        instance_dir: instance_dir.display().to_string(),
        workdir: workdir.display().to_string(),
        repo_path: repo_path.map(|path| path.display().to_string()),
        microvm_config: microvm_config.display().to_string(),
        record,
    })
}

fn create_vm_state(output: &VmCreateOutput) -> Result<(), VmError> {
    let state_file = PathBuf::from(&output.state_file);
    if state_file.exists() {
        return Err(VmError {
            code: "conflict",
            message: format!("VM state already exists: {}", state_file.display()),
        });
    }
    let instance_dir = PathBuf::from(&output.instance_dir);
    let workdir = PathBuf::from(&output.workdir);
    fs::create_dir_all(state_file.parent().expect("state file has parent")).map_err(|error| {
        state_error(format!(
            "cannot create VM state directory {}: {error}",
            state_file.parent().unwrap().display()
        ))
    })?;
    fs::create_dir_all(&instance_dir).map_err(|error| {
        state_error(format!(
            "cannot create VM instance directory {}: {error}",
            instance_dir.display()
        ))
    })?;
    fs::create_dir_all(&workdir).map_err(|error| {
        state_error(format!(
            "cannot create VM work directory {}: {error}",
            workdir.display()
        ))
    })?;
    if let Some(repo) = &output.record.source_repo {
        let repo_path = output
            .repo_path
            .as_ref()
            .map(PathBuf::from)
            .expect("repo path exists when source_repo exists");
        if repo_path.exists() {
            return Err(VmError {
                code: "conflict",
                message: format!("repo path already exists: {}", repo_path.display()),
            });
        }
        let status = Command::new("git")
            .args(["clone", "--", repo, repo_path.to_str().unwrap_or_default()])
            .status()
            .map_err(|error| VmError {
                code: "git_error",
                message: format!("cannot execute git clone: {error}"),
            })?;
        if !status.success() {
            return Err(VmError {
                code: "git_error",
                message: format!("git clone failed with status {status}"),
            });
        }
    }
    fs::write(
        &output.microvm_config,
        render_microvm_config(&output.record),
    )
    .map_err(|error| {
        state_error(format!(
            "cannot write microVM config {}: {error}",
            output.microvm_config
        ))
    })?;
    let json = serde_json::to_string_pretty(&output.record).unwrap();
    let temp = state_file.with_extension("json.tmp");
    fs::write(&temp, format!("{json}\n")).map_err(|error| {
        state_error(format!(
            "cannot write temporary VM state {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, &state_file).map_err(|error| {
        state_error(format!(
            "cannot publish VM state {}: {error}",
            state_file.display()
        ))
    })?;
    Ok(())
}

fn state_root(override_dir: Option<PathBuf>) -> Result<PathBuf, VmError> {
    if let Some(path) = override_dir {
        if !path.is_absolute() {
            return Err(invalid_args("--state-dir must be absolute"));
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
            return Err(state_error(format!(
                "cannot read VM state directory {}: {error}",
                dir.display()
            )));
        }
    };
    let mut records = Vec::new();
    for entry in entries {
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
        let text = fs::read_to_string(&path).map_err(|error| {
            state_error(format!("cannot read VM state {}: {error}", path.display()))
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

fn invalid_args(message: &str) -> VmError {
    VmError {
        code: "invalid_args",
        message: message.into(),
    }
}

fn state_error(message: String) -> VmError {
    VmError {
        code: "state_error",
        message,
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for b in value.bytes() {
        let c = b.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c as char);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(64);
    out
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
        if let Some(config) = &vm.microvm_config {
            println!("  microvm: {}", serde_json::to_string(config).unwrap());
        }
    }
}

fn print_create(output: &VmCreateOutput) {
    println!(
        "{} {}",
        if output.dry_run {
            "would create"
        } else {
            "created"
        },
        serde_json::to_string(&output.record.name).unwrap()
    );
    println!("  id: {}", output.record.id);
    println!("  state: {}", output.state_file);
    println!("  instance: {}", output.instance_dir);
    println!("  workdir: {}", output.workdir);
    if let Some(repo_path) = &output.repo_path {
        println!("  repo: {repo_path}");
    }
    println!("  microvm: {}", output.microvm_config);
    if output.dry_run {
        println!("  dry-run: no files written");
    }
}

fn render_microvm_config(record: &VmRecord) -> String {
    format!(
        r#"{{ pkgs, lib, ... }}:
{{
  networking.hostName = {host};
  system.stateVersion = "25.11";

  users.users.root.hashedPassword = "!";
  services.getty.autologinUser = "root";

  environment.systemPackages = with pkgs; [ git tmux ];

  systemd.services.argos-ready = {{
    description = "Argos VM readiness marker";
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {{
      Type = "oneshot";
      StandardOutput = "journal+console";
      StandardError = "journal+console";
    }};
    script = ''
      echo ARGOS_VM_READY id={id} host=$(${{lib.getExe' pkgs.nettools "hostname"}})
    '';
  }};

  microvm = {{
    hypervisor = "qemu";
    socket = "control.socket";
    volumes = [
      {{ mountPoint = "/var"; image = "var.img"; size = 1024; }}
    ];
    shares = [
      {{
        proto = "9p";
        tag = "ro-store";
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
      }}
    ];
  }};
}}
"#,
        host = serde_json::to_string(&format!("argos-{}", record.id)).unwrap(),
        id = record.id
    )
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
    fn slugs_names() {
        assert_eq!(slug("Trade Feature!"), "trade-feature");
        assert_eq!(slug("---"), "");
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
