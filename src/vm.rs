use crate::{config, host};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::IsTerminal,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const NIXPKGS_URL: &str = "github:NixOS/nixpkgs/dc5d91f840324650bac8c379428c7037a416959a";
const MICROVM_URL: &str = "github:microvm-nix/microvm.nix/614e9541186d724438edfd91c3bbc8474dd1b76e";
const START_READY_TIMEOUT: Duration = Duration::from_secs(45);
const SSH_KEYSCAN_TIMEOUT: Duration = Duration::from_secs(15);
const APP_PORT_START: u16 = 43000;
const APP_PORT_END: u16 = 45999;

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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepoProfile {
    #[serde(default)]
    workspace: ProfileWorkspace,
    #[serde(default)]
    vm: ProfileVm,
    #[serde(default)]
    ports: Vec<ProfilePort>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileWorkspace {
    #[serde(default = "default_guest_workdir")]
    workdir: String,
}

impl Default for ProfileWorkspace {
    fn default() -> Self {
        Self {
            workdir: default_guest_workdir(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileVm {
    #[serde(default)]
    packages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilePort {
    name: String,
    guest: u16,
}

#[derive(Debug, Serialize)]
struct VmCreateOutput {
    schema_version: u32,
    dry_run: bool,
    state_root: String,
    state_file: String,
    instance_dir: String,
    workdir: String,
    pub(crate) repo_path: Option<String>,
    microvm_config: String,
    record: VmRecord,
}

#[derive(Debug, Serialize)]
struct VmShowOutput {
    schema_version: u32,
    state_root: String,
    state_file: String,
    record: VmRecord,
    #[serde(skip_serializing)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct VmStartOutput {
    schema_version: u32,
    state_root: String,
    state_file: String,
    instance_dir: String,
    runner_path: String,
    log_path: String,
    pid: u32,
    console_session: String,
    already_running: bool,
    record: VmRecord,
}

#[derive(Debug, Serialize)]
struct VmStopOutput {
    schema_version: u32,
    state_root: String,
    state_file: String,
    pub(crate) pid: Option<u32>,
    already_stopped: bool,
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
    pub(crate) config: Option<PathBuf>,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) dry_run: bool,
    pub(crate) json: bool,
}

pub(crate) struct ShowOptions {
    pub(crate) id: String,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) json: bool,
}

pub(crate) struct StartOptions {
    pub(crate) id: String,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) config: Option<PathBuf>,
    pub(crate) json: bool,
}

pub(crate) struct StopOptions {
    pub(crate) id: String,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) json: bool,
}

pub(crate) struct GuestCommandOptions {
    pub(crate) id: String,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) tmux: bool,
}

pub(crate) fn snapshot(state_dir: Option<PathBuf>) -> Result<VmSnapshot, String> {
    let state_root = state_root(state_dir).map_err(|error| error.message)?;
    load_snapshot(&state_root).map_err(|error| format!("{}: {}", error.code, error.message))
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
    let tmux_config = match load_guest_tmux_config(options.config.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            return ExitCode::from(2);
        }
    };
    let mut output = match create_plan(&options) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {}", error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            return ExitCode::from(code);
        }
    };
    if !options.dry_run
        && let Err(error) = create_vm_state(&mut output, tmux_config.as_deref())
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

pub(crate) fn show(options: ShowOptions) -> ExitCode {
    let output = match show_vm(options) {
        Ok(record) => record,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            return ExitCode::from(code);
        }
    };
    if output.json {
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_show(&output);
    }
    ExitCode::SUCCESS
}

pub(crate) fn start(options: StartOptions) -> ExitCode {
    let json = options.json;
    let output = match start_vm(options) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            return ExitCode::from(code);
        }
    };
    if json {
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_start(&output);
    }
    ExitCode::SUCCESS
}

pub(crate) fn stop(options: StopOptions) -> ExitCode {
    let json = options.json;
    let output = match stop_vm(options) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            return ExitCode::from(code);
        }
    };
    if json {
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_stop(&output);
    }
    ExitCode::SUCCESS
}

pub(crate) fn guest_command(options: GuestCommandOptions) -> ExitCode {
    match run_guest_command(options) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {}: {}", error.code, error.message);
            let code = if error.code == "invalid_args" { 2 } else { 1 };
            ExitCode::from(code)
        }
    }
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
    let flake_path = instance_dir.join("flake.nix");
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
    let profile = load_repo_profile(options.profile.as_deref(), options.repo.as_deref())?;
    let guest_workdir = profile.profile.workspace.workdir.clone();
    let packages = profile.profile.vm.packages.clone();
    let ports = allocate_port_mappings(&profile.profile, &state_root)?;
    let ssh_port = ssh_port_for_id(&id);
    let record = VmRecord {
        schema_version: 1,
        id: id.clone(),
        name: options.name.clone(),
        status: "created".into(),
        host,
        project: options.project.clone(),
        source_repo: options.repo.clone(),
        profile_path: profile.path.map(|path| path.display().to_string()),
        guest_workdir: Some(guest_workdir),
        packages,
        ports,
        repo_path: repo_path.as_ref().map(|path| path.display().to_string()),
        workdir: Some(workdir.display().to_string()),
        instance_dir: Some(instance_dir.display().to_string()),
        flake_path: Some(flake_path.display().to_string()),
        microvm_config: Some(microvm_config.display().to_string()),
        runner_path: None,
        log_path: None,
        pid: None,
        console_session: None,
        ssh_host: Some("127.0.0.1".into()),
        ssh_port: Some(ssh_port),
        ssh_user: Some("root".into()),
        created_at_unix_ms: Some(now),
        updated_at_unix_ms: Some(now),
        started_at_unix_ms: None,
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

fn show_vm(options: ShowOptions) -> Result<VmShowOutput, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    let state_root = state_root(options.state_dir)?;
    let state_file = state_root.join("vms").join(format!("{}.json", options.id));
    let record = load_record(&state_file)?;
    Ok(VmShowOutput {
        schema_version: 1,
        state_root: state_root.display().to_string(),
        state_file: state_file.display().to_string(),
        record,
        json: options.json,
    })
}

fn create_vm_state(output: &mut VmCreateOutput, tmux_config: Option<&str>) -> Result<(), VmError> {
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
    apply_default_cloned_profile(output)?;
    write_guest_tmux_config(&instance_dir, tmux_config)?;
    fs::write(
        &output.microvm_config,
        render_microvm_config(&output.record, tmux_config),
    )
    .map_err(|error| {
        state_error(format!(
            "cannot write microVM config {}: {error}",
            output.microvm_config
        ))
    })?;
    let flake_path = output
        .record
        .flake_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| instance_dir.join("flake.nix"));
    fs::write(&flake_path, render_instance_flake()).map_err(|error| {
        state_error(format!(
            "cannot write microVM flake {}: {error}",
            flake_path.display()
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

fn start_vm(options: StartOptions) -> Result<VmStartOutput, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    let tmux_config = load_guest_tmux_config(options.config.as_deref())?;
    let state_root = state_root(options.state_dir)?;
    let state_file = state_root.join("vms").join(format!("{}.json", options.id));
    let mut record = load_record(&state_file)?;
    let instance_dir = record
        .instance_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| state_root.join("instances").join(&record.id));
    let flake_path = record
        .flake_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| instance_dir.join("flake.nix"));
    let microvm_config = record
        .microvm_config
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| instance_dir.join("microvm.nix"));
    let log_path = instance_dir.join("console.log");
    let console_session = console_session_name(&record.id);
    let ssh_port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let ssh_host = record
        .ssh_host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".into());
    let ssh_user = record.ssh_user.clone().unwrap_or_else(|| "root".into());

    if let Some(pid) = record.pid
        && record.status == "running"
        && (process_alive(pid) || tmux_session_exists(&console_session))
    {
        let runner_path = record
            .runner_path
            .clone()
            .unwrap_or_else(|| instance_dir.join("runner").display().to_string());
        let output = VmStartOutput {
            schema_version: 1,
            state_root: state_root.display().to_string(),
            state_file: state_file.display().to_string(),
            instance_dir: instance_dir.display().to_string(),
            runner_path,
            log_path: record
                .log_path
                .clone()
                .unwrap_or_else(|| log_path.display().to_string()),
            pid,
            console_session: record
                .console_session
                .clone()
                .unwrap_or_else(|| console_session.clone()),
            already_running: true,
            record,
        };
        return Ok(output);
    }
    if tmux_session_exists(&console_session) {
        kill_tmux_session(&console_session);
    }

    fs::create_dir_all(&instance_dir).map_err(|error| {
        state_error(format!(
            "cannot create VM instance directory {}: {error}",
            instance_dir.display()
        ))
    })?;
    record.ssh_host = Some(ssh_host.clone());
    record.ssh_port = Some(ssh_port);
    record.ssh_user = Some(ssh_user);
    write_guest_tmux_config(&instance_dir, tmux_config.as_deref())?;
    fs::write(
        &microvm_config,
        render_microvm_config(&record, tmux_config.as_deref()),
    )
    .map_err(|error| {
        state_error(format!(
            "cannot write microVM config {}: {error}",
            microvm_config.display()
        ))
    })?;
    if !flake_path.exists() {
        fs::write(&flake_path, render_instance_flake()).map_err(|error| {
            state_error(format!(
                "cannot write microVM flake {}: {error}",
                flake_path.display()
            ))
        })?;
    }
    let runner_path = build_runner(&instance_dir)?;
    let run_bin = runner_path.join("bin").join("microvm-run");
    if !run_bin.exists() {
        return Err(state_error(format!(
            "microVM runner is missing {}",
            run_bin.display()
        )));
    }
    let script = format!(
        "exec {} 2>&1 | tee -a {}",
        shell_quote(&run_bin.display().to_string()),
        shell_quote(&log_path.display().to_string())
    );
    start_tmux_console(&console_session, &instance_dir, &script)?;
    wait_for_ready(&console_session, &log_path, &record.id)?;
    let pid = tmux_session_pid(&console_session)?;

    let now = now_ms();
    record.status = "running".into();
    record.instance_dir = Some(instance_dir.display().to_string());
    record.flake_path = Some(flake_path.display().to_string());
    record.microvm_config = Some(microvm_config.display().to_string());
    record.runner_path = Some(runner_path.display().to_string());
    record.log_path = Some(log_path.display().to_string());
    record.pid = Some(pid);
    record.console_session = Some(console_session.clone());
    record.started_at_unix_ms = Some(now);
    record.updated_at_unix_ms = Some(now);
    write_record(&state_file, &record)?;

    Ok(VmStartOutput {
        schema_version: 1,
        state_root: state_root.display().to_string(),
        state_file: state_file.display().to_string(),
        instance_dir: instance_dir.display().to_string(),
        runner_path: runner_path.display().to_string(),
        log_path: log_path.display().to_string(),
        pid,
        console_session,
        already_running: false,
        record,
    })
}

fn stop_vm(options: StopOptions) -> Result<VmStopOutput, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    let state_root = state_root(options.state_dir)?;
    let state_file = state_root.join("vms").join(format!("{}.json", options.id));
    let mut record = load_record(&state_file)?;
    let pid = record.pid;
    let console_session = record
        .console_session
        .clone()
        .unwrap_or_else(|| console_session_name(&record.id));
    let already_stopped =
        pid.is_none_or(|pid| !process_alive(pid)) && !tmux_session_exists(&console_session);
    if !already_stopped && let Some(pid) = pid {
        graceful_shutdown(&record);
        if process_alive(pid) {
            signal_pid(pid, "TERM")?;
            wait_for_exit(pid, Duration::from_secs(5));
        }
        if process_alive(pid) {
            signal_pid(pid, "KILL")?;
            wait_for_exit(pid, Duration::from_secs(2));
        }
    }
    if tmux_session_exists(&console_session) {
        kill_tmux_session(&console_session);
    }
    let now = now_ms();
    record.status = "stopped".into();
    record.pid = None;
    record.console_session = Some(console_session);
    record.updated_at_unix_ms = Some(now);
    write_record(&state_file, &record)?;
    Ok(VmStopOutput {
        schema_version: 1,
        state_root: state_root.display().to_string(),
        state_file: state_file.display().to_string(),
        pid,
        already_stopped,
        record,
    })
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

struct LoadedProfile {
    profile: RepoProfile,
    path: Option<PathBuf>,
}

fn default_guest_workdir() -> String {
    "/workspace/repo".into()
}

fn load_repo_profile(
    explicit: Option<&Path>,
    repo: Option<&str>,
) -> Result<LoadedProfile, VmError> {
    if let Some(path) = explicit {
        let path = normalize_profile_path(path)?;
        let profile = read_repo_profile(&path)?;
        return Ok(LoadedProfile {
            profile,
            path: Some(path),
        });
    }
    if let Some(repo) = repo {
        let path = Path::new(repo).join(".argos.toml");
        if path.is_file() {
            let path = normalize_profile_path(&path)?;
            let profile = read_repo_profile(&path)?;
            return Ok(LoadedProfile {
                profile,
                path: Some(path),
            });
        }
    }
    Ok(LoadedProfile {
        profile: RepoProfile::default(),
        path: None,
    })
}

fn normalize_profile_path(path: &Path) -> Result<PathBuf, VmError> {
    if path.as_os_str().is_empty() {
        return Err(invalid_args("--profile cannot be empty"));
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| state_error(format!("cannot read current directory: {error}")))?
            .join(path)
    };
    Ok(path)
}

fn read_repo_profile(path: &Path) -> Result<RepoProfile, VmError> {
    let text = fs::read_to_string(path).map_err(|error| VmError {
        code: "profile_error",
        message: format!("cannot read repo profile {}: {error}", path.display()),
    })?;
    let profile: RepoProfile = toml::from_str(&text).map_err(|error| VmError {
        code: "profile_error",
        message: format!("invalid repo profile {}: {error}", path.display()),
    })?;
    validate_profile(&profile, path)?;
    Ok(profile)
}

fn validate_profile(profile: &RepoProfile, path: &Path) -> Result<(), VmError> {
    if !profile.workspace.workdir.starts_with('/')
        || profile.workspace.workdir.contains('\0')
        || profile.workspace.workdir.chars().any(char::is_control)
    {
        return Err(VmError {
            code: "profile_error",
            message: format!(
                "invalid repo profile {}: workspace.workdir must be an absolute guest path",
                path.display()
            ),
        });
    }
    let mut packages = BTreeSet::new();
    for package in &profile.vm.packages {
        if !valid_package_attr(package) || !packages.insert(package) {
            return Err(VmError {
                code: "profile_error",
                message: format!(
                    "invalid repo profile {}: vm.packages contains invalid or duplicate attr {package:?}",
                    path.display()
                ),
            });
        }
    }
    let mut port_names = BTreeSet::new();
    let mut guest_ports = BTreeSet::new();
    for port in &profile.ports {
        if !valid_port_name(&port.name)
            || !port_names.insert(port.name.as_str())
            || !guest_ports.insert(port.guest)
            || port.guest == 0
        {
            return Err(VmError {
                code: "profile_error",
                message: format!(
                    "invalid repo profile {}: ports require unique safe names and guest ports",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

fn valid_package_attr(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn valid_port_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn allocate_port_mappings(
    profile: &RepoProfile,
    state_root: &Path,
) -> Result<Vec<VmPortMapping>, VmError> {
    let mut used = used_host_ports(state_root)?;
    let mut mappings = Vec::new();
    for port in &profile.ports {
        let Some(host) = (APP_PORT_START..=APP_PORT_END).find(|candidate| used.insert(*candidate))
        else {
            return Err(VmError {
                code: "state_error",
                message: format!("no free host ports in range {APP_PORT_START}-{APP_PORT_END}"),
            });
        };
        mappings.push(VmPortMapping {
            name: port.name.clone(),
            guest: port.guest,
            host,
        });
    }
    Ok(mappings)
}

fn used_host_ports(state_root: &Path) -> Result<BTreeSet<u16>, VmError> {
    let mut used = BTreeSet::new();
    for record in load_vms(&state_root.join("vms"))? {
        if let Some(port) = record.ssh_port {
            used.insert(port);
        }
        for mapping in record.ports {
            used.insert(mapping.host);
        }
    }
    Ok(used)
}

fn apply_default_cloned_profile(output: &mut VmCreateOutput) -> Result<(), VmError> {
    if output.record.profile_path.is_some() {
        return Ok(());
    }
    let Some(repo_path) = output.repo_path.as_ref().map(PathBuf::from) else {
        return Ok(());
    };
    let profile_path = repo_path.join(".argos.toml");
    if !profile_path.is_file() {
        return Ok(());
    }
    let profile = read_repo_profile(&profile_path)?;
    let state_root = PathBuf::from(&output.state_root);
    output.record.profile_path = Some(profile_path.display().to_string());
    output.record.guest_workdir = Some(profile.workspace.workdir.clone());
    output.record.packages = profile.vm.packages.clone();
    output.record.ports = allocate_port_mappings(&profile, &state_root)?;
    Ok(())
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

fn load_record(path: &Path) -> Result<VmRecord, VmError> {
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

fn write_record(path: &Path, record: &VmRecord) -> Result<(), VmError> {
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

fn build_runner(instance_dir: &Path) -> Result<PathBuf, VmError> {
    let out_link = instance_dir.join("runner");
    let status = Command::new("nix")
        .args([
            "build",
            ".#runner",
            "--out-link",
            out_link.to_str().unwrap_or_default(),
        ])
        .current_dir(instance_dir)
        .status()
        .map_err(|error| VmError {
            code: "nix_error",
            message: format!("cannot execute nix build: {error}"),
        })?;
    if !status.success() {
        return Err(VmError {
            code: "nix_error",
            message: format!("nix build failed with status {status}"),
        });
    }
    Ok(out_link)
}

fn run_guest_command(options: GuestCommandOptions) -> Result<ExitCode, VmError> {
    if !valid_id(&options.id) {
        return Err(invalid_args(
            "VM id must be 1-64 chars of letters, digits, '_' or '-'",
        ));
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(VmError {
            code: "shell_error",
            message: "guest shell requires a terminal on stdin and stdout".into(),
        });
    }
    let state_root = state_root(options.state_dir)?;
    let state_file = state_root.join("vms").join(format!("{}.json", options.id));
    let record = load_record(&state_file)?;
    if record.status != "running" {
        return Err(VmError {
            code: "not_running",
            message: format!("VM is not running: {}", record.id),
        });
    }
    let host = record.ssh_host.as_deref().unwrap_or("127.0.0.1");
    let user = record.ssh_user.as_deref().unwrap_or("root");
    let port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let instance_dir = record
        .instance_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| state_root.join("instances").join(&record.id));
    let known_hosts = instance_dir.join("known_hosts");
    refresh_guest_known_hosts(&record, &instance_dir)?;
    let destination = format!("{user}@{host}");
    let mut command = Command::new("ssh");
    command
        .arg("-tt")
        .args(["-p", &port.to_string()])
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "StrictHostKeyChecking=yes"])
        .args([
            "-o",
            &format!("UserKnownHostsFile={}", known_hosts.display()),
        ])
        .arg(destination);
    let guest_workdir = record.guest_workdir.as_deref().unwrap_or("/workspace/repo");
    let workspace_prefix = format!(
        "cd {} 2>/dev/null || cd /workspace/repo 2>/dev/null || cd /workspace 2>/dev/null || cd",
        shell_quote(guest_workdir)
    );
    if options.tmux {
        command.arg(format!(
            "{workspace_prefix}; if [ -f /etc/tmux.conf ]; then exec tmux -f /etc/tmux.conf new -A -s main; else exec tmux new -A -s main; fi"
        ));
    } else {
        command.arg(format!(
            "{workspace_prefix}; exec ${{SHELL:-/run/current-system/sw/bin/sh}} -l"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("error: cannot execute ssh: {error}");
        Ok(ExitCode::from(1))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(|error| VmError {
            code: "shell_error",
            message: format!("cannot execute ssh: {error}"),
        })?;
        Ok(if status.success() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(status.code().unwrap_or(1) as u8)
        })
    }
}

fn wait_for_ready(session: &str, log_path: &Path, id: &str) -> Result<(), VmError> {
    let marker = format!("ARGOS_VM_READY id={id}");
    let start = SystemTime::now();
    loop {
        if let Ok(mut file) = fs::File::open(log_path) {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            if text.contains(&marker) {
                return Ok(());
            }
        }
        if !tmux_session_exists(session) {
            return Err(VmError {
                code: "start_error",
                message: "microVM console session exited before readiness marker".into(),
            });
        }
        if start.elapsed().unwrap_or_else(|_| Duration::from_secs(0)) > START_READY_TIMEOUT {
            return Err(VmError {
                code: "start_error",
                message: format!(
                    "timed out waiting for readiness marker in {}",
                    log_path.display()
                ),
            });
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn refresh_guest_known_hosts(record: &VmRecord, instance_dir: &Path) -> Result<(), VmError> {
    let host = record.ssh_host.as_deref().unwrap_or("127.0.0.1");
    let port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let known_hosts = instance_dir.join("known_hosts");
    let start = SystemTime::now();

    loop {
        match scan_guest_host_key(host, port) {
            Ok(keys) => {
                fs::create_dir_all(instance_dir).map_err(|error| {
                    state_error(format!(
                        "cannot create VM instance directory {}: {error}",
                        instance_dir.display()
                    ))
                })?;
                let temp = known_hosts.with_extension("known_hosts.tmp");
                fs::write(&temp, keys).map_err(|error| {
                    state_error(format!(
                        "cannot write temporary known_hosts {}: {error}",
                        temp.display()
                    ))
                })?;
                fs::rename(&temp, &known_hosts).map_err(|error| {
                    state_error(format!(
                        "cannot publish known_hosts {}: {error}",
                        known_hosts.display()
                    ))
                })?;
                return Ok(());
            }
            Err(error) => {
                if start.elapsed().unwrap_or_else(|_| Duration::from_secs(0)) > SSH_KEYSCAN_TIMEOUT
                {
                    return Err(error);
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn scan_guest_host_key(host: &str, port: u16) -> Result<String, VmError> {
    let output = Command::new("ssh-keyscan")
        .args(["-T", "5", "-p", &port.to_string(), host])
        .output()
        .map_err(|error| VmError {
            code: "ssh_error",
            message: format!("cannot execute ssh-keyscan: {error}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let keys = stdout
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    if keys.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(VmError {
            code: "ssh_error",
            message: format!(
                "could not scan guest SSH host key on {host}:{port}: {}",
                stderr.trim()
            ),
        });
    }
    Ok(format!("{keys}\n"))
}

fn start_tmux_console(session: &str, instance_dir: &Path, script: &str) -> Result<(), VmError> {
    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", session, "-c"])
        .arg(instance_dir)
        .args(["sh", "-lc", script])
        .status()
        .map_err(|error| VmError {
            code: "start_error",
            message: format!("cannot execute tmux new-session: {error}"),
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(VmError {
            code: "start_error",
            message: format!("tmux new-session failed with status {status}"),
        })
    }
}

fn tmux_session_exists(session: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn tmux_session_pid(session: &str) -> Result<u32, VmError> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "-t", session, "#{pane_pid}"])
        .output()
        .map_err(|error| VmError {
            code: "start_error",
            message: format!("cannot execute tmux display-message: {error}"),
        })?;
    if !output.status.success() {
        return Err(VmError {
            code: "start_error",
            message: format!(
                "tmux display-message failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|error| VmError {
            code: "start_error",
            message: format!("tmux returned invalid pane pid: {error}"),
        })
}

fn kill_tmux_session(session: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn console_session_name(id: &str) -> String {
    format!("argos-vm-{id}")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn process_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

fn graceful_shutdown(record: &VmRecord) {
    let Some(runner_path) = &record.runner_path else {
        return;
    };
    let Some(instance_dir) = &record.instance_dir else {
        return;
    };
    let shutdown = Path::new(runner_path).join("bin").join("microvm-shutdown");
    if !shutdown.exists() {
        return;
    }
    let Ok(mut child) = Command::new(shutdown)
        .current_dir(instance_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let start = SystemTime::now();
    while child.try_wait().ok().flatten().is_none()
        && start.elapsed().unwrap_or_else(|_| Duration::from_secs(0)) < Duration::from_secs(10)
    {
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
}

fn signal_pid(pid: u32, signal: &str) -> Result<(), VmError> {
    let status = Command::new("kill")
        .args([format!("-{signal}"), pid.to_string()])
        .status()
        .map_err(|error| VmError {
            code: "stop_error",
            message: format!("cannot execute kill: {error}"),
        })?;
    if !status.success() && process_alive(pid) {
        return Err(VmError {
            code: "stop_error",
            message: format!("kill -{signal} {pid} failed with status {status}"),
        });
    }
    Ok(())
}

fn wait_for_exit(pid: u32, timeout: Duration) {
    let start = SystemTime::now();
    while process_alive(pid) && start.elapsed().unwrap_or_else(|_| Duration::from_secs(0)) < timeout
    {
        thread::sleep(Duration::from_millis(100));
    }
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
    let mut port_names = BTreeSet::new();
    let mut guest_ports = BTreeSet::new();
    let mut host_ports = BTreeSet::new();
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
        print_record_profile(vm, "  ");
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
    print_record_profile(&output.record, "  ");
    println!("  microvm: {}", output.microvm_config);
    if output.dry_run {
        println!("  dry-run: no files written");
    }
}

fn print_start(output: &VmStartOutput) {
    println!(
        "{} {}",
        if output.already_running {
            "already running"
        } else {
            "started"
        },
        serde_json::to_string(&output.record.name).unwrap()
    );
    println!("  id: {}", output.record.id);
    println!("  pid: {}", output.pid);
    println!("  runner: {}", output.runner_path);
    println!("  log: {}", output.log_path);
    if let (Some(user), Some(host), Some(port)) = (
        output.record.ssh_user.as_ref(),
        output.record.ssh_host.as_ref(),
        output.record.ssh_port,
    ) {
        println!("  ssh: {user}@{host}:{port}");
        println!("  shell: argos vm shell {}", output.record.id);
        println!("  tmux: argos vm tmux {}", output.record.id);
    }
    print_links(&output.record, "  ");
}

fn print_show(output: &VmShowOutput) {
    let vm = &output.record;
    println!(
        "{} ({}) on {}",
        serde_json::to_string(&vm.name).unwrap(),
        vm.status,
        vm.host
    );
    println!("  id: {}", vm.id);
    println!("  state: {}", output.state_file);
    if let Some(project) = &vm.project {
        println!("  project: {}", serde_json::to_string(project).unwrap());
    }
    if let Some(repo) = &vm.source_repo {
        println!("  source: {}", serde_json::to_string(repo).unwrap());
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
    if let (Some(user), Some(host), Some(port)) =
        (vm.ssh_user.as_ref(), vm.ssh_host.as_ref(), vm.ssh_port)
    {
        println!("  ssh: {user}@{host}:{port}");
    }
    print_record_profile(vm, "  ");
}

fn print_record_profile(vm: &VmRecord, prefix: &str) {
    if let Some(profile) = &vm.profile_path {
        println!(
            "{prefix}profile: {}",
            serde_json::to_string(profile).unwrap()
        );
    }
    if let Some(workdir) = &vm.guest_workdir {
        println!(
            "{prefix}guest workdir: {}",
            serde_json::to_string(workdir).unwrap()
        );
    }
    if !vm.packages.is_empty() {
        println!("{prefix}packages: {}", vm.packages.join(", "));
    }
    print_links(vm, prefix);
}

fn print_links(vm: &VmRecord, prefix: &str) {
    if vm.ports.is_empty() {
        return;
    }
    println!("{prefix}links:");
    for mapping in &vm.ports {
        println!(
            "{prefix}  {}: http://127.0.0.1:{} -> guest:{}",
            mapping.name, mapping.host, mapping.guest
        );
    }
}

fn print_stop(output: &VmStopOutput) {
    println!(
        "{} {}",
        if output.already_stopped {
            "already stopped"
        } else {
            "stopped"
        },
        serde_json::to_string(&output.record.name).unwrap()
    );
    println!("  id: {}", output.record.id);
    if let Some(pid) = output.pid {
        println!("  previous pid: {pid}");
    }
}

fn render_instance_flake() -> String {
    format!(
        r#"{{
  description = "Argos generated microVM instance";

  inputs.nixpkgs.url = {nixpkgs};
  inputs.microvm = {{
    url = {microvm};
    inputs.nixpkgs.follows = "nixpkgs";
  }};

  outputs = {{ nixpkgs, microvm, ... }}:
    let system = "x86_64-linux";
    in {{
      packages.${{system}}.runner = (nixpkgs.lib.nixosSystem {{
        inherit system;
        modules = [ microvm.nixosModules.microvm ./microvm.nix ];
      }}).config.microvm.declaredRunner;
    }};
}}
"#,
        nixpkgs = serde_json::to_string(NIXPKGS_URL).unwrap(),
        microvm = serde_json::to_string(MICROVM_URL).unwrap(),
    )
}

fn render_microvm_config(record: &VmRecord, tmux_config: Option<&str>) -> String {
    let ssh_port = record
        .ssh_port
        .unwrap_or_else(|| ssh_port_for_id(&record.id));
    let keys = authorized_keys_nix();
    let mac = mac_for_id(&record.id);
    let tmux_block = render_tmux_config_block(tmux_config);
    let tmux_store_shares = render_tmux_store_shares(tmux_config);
    let workspace_share = render_workspace_share(record);
    let packages = render_system_packages(record);
    let firewall_ports = render_firewall_ports(record);
    let app_forwards = render_app_port_forwards(record);
    format!(
        r#"{{ pkgs, lib, config, ... }}:
{{
  networking.hostName = {host};
  networking.interfaces.eth0.useDHCP = true;
  networking.firewall.allowedTCPPorts = {firewall_ports};
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  system.stateVersion = "25.11";

  users.users.root.hashedPassword = "!";
  users.users.root.openssh.authorizedKeys.keys = {keys};
  services.getty.autologinUser = "root";
  services.openssh = {{
    enable = true;
    settings.PermitRootLogin = "prohibit-password";
    settings.PasswordAuthentication = false;
    hostKeys = [
      {{ path = "/var/lib/argos/ssh/ssh_host_ed25519_key"; type = "ed25519"; }}
    ];
  }};

  environment.systemPackages = with pkgs; {packages};
  environment.etc."gitconfig".text = ''
    [safe]
      directory = /workspace/repo
      directory = /workspace
  '';
  systemd.tmpfiles.rules = [ "d /var/lib/argos/ssh 0700 root root -" ];
{tmux_block}
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
    vcpu = 4;
    mem = 4096;
    socket = "control.socket";
    writableStoreOverlay = "/nix/.rw-store";
    interfaces = [
      {{ type = "user"; id = "eth0"; mac = {mac}; }}
    ];
    forwardPorts = [
      {{ from = "host"; host.address = "127.0.0.1"; host.port = {ssh_port}; guest.port = 22; }}
{app_forwards}
    ];
    volumes = [
      {{ mountPoint = "/var"; image = "var.img"; size = 1024; }}
      {{ mountPoint = config.microvm.writableStoreOverlay; image = "nix-store-overlay.img"; size = 8192; }}
    ];
    shares = [
      {{
        proto = "9p";
        tag = "ro-store";
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
      }}
{tmux_store_shares}{workspace_share}    ];
  }};
}}
"#,
        host = serde_json::to_string(&format!("argos-{}", record.id)).unwrap(),
        id = record.id,
        keys = keys,
        mac = serde_json::to_string(&mac).unwrap(),
        ssh_port = ssh_port,
        tmux_block = tmux_block,
        tmux_store_shares = tmux_store_shares,
        workspace_share = workspace_share,
        packages = packages,
        firewall_ports = firewall_ports,
        app_forwards = app_forwards,
    )
}

fn render_system_packages(record: &VmRecord) -> String {
    let mut packages = vec!["git".to_string(), "tmux".to_string(), "openssh".to_string()];
    for package in &record.packages {
        if !packages.iter().any(|existing| existing == package) {
            packages.push(package.clone());
        }
    }
    format!("[ {} ]", packages.join(" "))
}

fn render_firewall_ports(record: &VmRecord) -> String {
    let mut ports = vec![22_u16];
    for mapping in &record.ports {
        if !ports.contains(&mapping.guest) {
            ports.push(mapping.guest);
        }
    }
    ports.sort_unstable();
    format!(
        "[ {} ]",
        ports
            .into_iter()
            .map(|port| port.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn render_app_port_forwards(record: &VmRecord) -> String {
    let mut out = String::new();
    for mapping in &record.ports {
        out.push_str(&format!(
            "      {{ from = \"host\"; host.address = \"127.0.0.1\"; host.port = {}; guest.port = {}; }}\n",
            mapping.host, mapping.guest
        ));
    }
    out
}

fn render_workspace_share(record: &VmRecord) -> String {
    let Some(workdir) = record.workdir.as_deref() else {
        return String::new();
    };
    format!(
        r#"      {{
        proto = "9p";
        tag = "workspace";
        source = {source};
        mountPoint = "/workspace";
      }}
"#,
        source = serde_json::to_string(workdir).unwrap(),
    )
}

fn render_tmux_config_block(tmux_config: Option<&str>) -> String {
    let Some(_config) = tmux_config else {
        return String::new();
    };
    String::from(
        r#"
  environment.etc."tmux.conf".source = ./guest-tmux.conf;
  environment.etc."argos/tmux.conf".source = ./guest-tmux.conf;
"#,
    )
}

fn render_tmux_store_shares(tmux_config: Option<&str>) -> String {
    let Some(config) = tmux_config else {
        return String::new();
    };
    let mut out = String::new();
    for (index, path) in store_paths_in_text(config).into_iter().enumerate() {
        let source = serde_json::to_string(&path).unwrap();
        out.push_str(&format!(
            r#"      {{
        proto = "9p";
        tag = "tmux-store-{index}";
        source = {source};
        mountPoint = {source};
      }}
"#
        ));
    }
    out
}

fn store_paths_in_text(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("/nix/store/") {
        let candidate = &rest[index..];
        let name_start = "/nix/store/".len();
        let Some(name_end) = candidate[name_start..].find('/') else {
            break;
        };
        let path = &candidate[..name_start + name_end];
        if Path::new(path).exists() && !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_string());
        }
        rest = &candidate[name_start + name_end..];
    }
    paths
}

fn write_guest_tmux_config(instance_dir: &Path, tmux_config: Option<&str>) -> Result<(), VmError> {
    let path = instance_dir.join("guest-tmux.conf");
    let Some(config) = tmux_config else {
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(state_error(format!(
                "cannot remove guest tmux config {}: {error}",
                path.display()
            )));
        }
        return Ok(());
    };
    fs::write(&path, config).map_err(|error| {
        state_error(format!(
            "cannot write guest tmux config {}: {error}",
            path.display()
        ))
    })
}

fn load_guest_tmux_config(path: Option<&Path>) -> Result<Option<String>, VmError> {
    config::guest_tmux_config(path).map_err(|message| VmError {
        code: "config_error",
        message,
    })
}

fn authorized_keys_nix() -> String {
    let mut keys = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        for name in ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"] {
            let path = Path::new(&home).join(".ssh").join(name);
            if let Ok(text) = fs::read_to_string(path) {
                let key = text.trim();
                if !key.is_empty() {
                    keys.push(serde_json::to_string(key).unwrap());
                }
            }
        }
    }
    format!("[ {} ]", keys.join(" "))
}

fn ssh_port_for_id(id: &str) -> u16 {
    let mut hash: u32 = 0;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
    }
    22000 + (hash % 20000) as u16
}

fn mac_for_id(id: &str) -> String {
    let mut hash: u32 = 0xA6_90_05;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(16777619) ^ byte as u32;
    }
    format!(
        "02:7a:{:02x}:{:02x}:{:02x}:{:02x}",
        (hash >> 24) & 0xff,
        (hash >> 16) & 0xff,
        (hash >> 8) & 0xff,
        hash & 0xff
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
    fn extracts_store_paths_from_tmux_config() {
        let cargo = env!("CARGO");
        let root = cargo.strip_suffix("/bin/cargo").unwrap_or(cargo);
        let paths = store_paths_in_text(&format!("run-shell {cargo} and {root}/bin/rustc"));
        if Path::new(root).exists() {
            assert_eq!(paths, vec![root.to_string()]);
        }
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

    #[test]
    fn parses_profile_and_allocates_distinct_host_ports() {
        let root = std::env::temp_dir().join(format!(
            "argos-profile-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let profile_path = root.join(".argos.toml");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &profile_path,
            r#"[workspace]
workdir = "/workspace/repo"

[vm]
packages = ["go", "just"]

[[ports]]
name = "api"
guest = 3001
"#,
        )
        .unwrap();
        let profile = read_repo_profile(&profile_path).unwrap();
        assert_eq!(profile.workspace.workdir, "/workspace/repo");
        assert_eq!(profile.vm.packages, vec!["go", "just"]);
        let first = allocate_port_mappings(&profile, &root).unwrap();
        assert_eq!(first[0].guest, 3001);
        let vms = root.join("vms");
        fs::create_dir_all(&vms).unwrap();
        fs::write(
            vms.join("used.json"),
            serde_json::json!({
                "schema_version": 1,
                "id": "used",
                "name": "Used",
                "status": "created",
                "host": "here",
                "ports": first,
            })
            .to_string(),
        )
        .unwrap();
        let second = allocate_port_mappings(&profile, &root).unwrap();
        assert_eq!(second[0].guest, 3001);
        assert_ne!(second[0].host, first[0].host);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_bad_profile_values() {
        let root = std::env::temp_dir().join(format!(
            "argos-bad-profile-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let bad = root.join("bad.toml");
        fs::write(&bad, "[vm]\npackages = [\"../bad\"]\n").unwrap();
        assert!(read_repo_profile(&bad).is_err());
        fs::write(&bad, "[[ports]]\nname = \"api\"\nguest = 0\n").unwrap();
        assert!(read_repo_profile(&bad).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
