use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub client: Client,
    pub machines: BTreeMap<String, Machine>,
    #[serde(default)]
    pub vm: Option<VmConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmConfig {
    #[serde(default)]
    pub guest_tmux: Option<GuestTmuxConfig>,
    #[serde(default)]
    pub defaults: Option<VmDefaultsConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestTmuxConfig {
    #[serde(default)]
    pub config_text: Option<String>,
    #[serde(default)]
    pub config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmDefaultsConfig {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub files: Vec<VmDefaultFileConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmDefaultFileConfig {
    pub target: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub source_path: Option<PathBuf>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct VmDefaults {
    pub packages: Vec<String>,
    pub files: Vec<VmDefaultFile>,
}

#[derive(Clone, Debug)]
pub struct VmDefaultFile {
    pub target: String,
    pub text: String,
    pub mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub machine_id: String,
    #[serde(default = "default_timeout")]
    pub connect_timeout_seconds: u64,
    #[serde(default = "default_parallel")]
    pub max_parallel_probes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub ssh_alias: Option<String>,
    pub socket: Option<String>,
}

fn default_timeout() -> u64 {
    3
}
fn default_parallel() -> usize {
    4
}

pub fn default_path() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(value);
        if path.is_absolute() && !path.as_os_str().is_empty() {
            return Some(path.join("argos/config.toml"));
        }
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".config/argos/config.toml"))
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
    parse(&text, path)
}

pub fn load_implicit(path: &Path) -> Result<Option<Config>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read config {}: {error}", path.display())),
    };
    parse(&text, path).map(Some)
}

pub fn load_selected(path: Option<&Path>) -> Result<Option<(Config, PathBuf)>, String> {
    match path {
        Some(path) => load(path).map(|config| Some((config, path.to_path_buf()))),
        None => {
            let Some(path) = default_path() else {
                return Ok(None);
            };
            load_implicit(&path).map(|config| config.map(|config| (config, path)))
        }
    }
}

pub fn guest_tmux_config(path: Option<&Path>) -> Result<Option<String>, String> {
    let Some((config, config_path)) = load_selected(path)? else {
        return Ok(None);
    };
    let Some(vm) = config.vm.as_ref() else {
        return Ok(None);
    };
    let Some(tmux) = vm.guest_tmux.as_ref() else {
        return Ok(None);
    };
    match (&tmux.config_text, &tmux.config_path) {
        (Some(_), Some(_)) => {
            Err("vm.guest_tmux cannot set both config_text and config_path".into())
        }
        (Some(text), None) => Ok(Some(text.clone())),
        (None, Some(path)) => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            };
            fs::read_to_string(&path).map(Some).map_err(|error| {
                format!(
                    "cannot read vm.guest_tmux.config_path {}: {error}",
                    path.display()
                )
            })
        }
        (None, None) => Ok(None),
    }
}

pub fn vm_defaults(path: Option<&Path>) -> Result<VmDefaults, String> {
    let Some((config, config_path)) = load_selected(path)? else {
        return Ok(VmDefaults::default());
    };
    let Some(vm) = config.vm.as_ref() else {
        return Ok(VmDefaults::default());
    };
    let Some(defaults) = vm.defaults.as_ref() else {
        return Ok(VmDefaults::default());
    };

    let mut files = Vec::new();
    for file in &defaults.files {
        let text = match (&file.text, &file.source_path) {
            (Some(_), Some(_)) => {
                return Err("vm.defaults.files cannot set both text and source_path".into());
            }
            (Some(text), None) => text.clone(),
            (None, Some(path)) => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(path)
                };
                fs::read_to_string(&path).map_err(|error| {
                    format!(
                        "cannot read vm.defaults.files.source_path {}: {error}",
                        path.display()
                    )
                })?
            }
            (None, None) => String::new(),
        };
        files.push(VmDefaultFile {
            target: file.target.clone(),
            text,
            mode: file.mode.clone().unwrap_or_else(|| "0644".into()),
        });
    }

    Ok(VmDefaults {
        packages: defaults.packages.clone(),
        files,
    })
}

fn parse(text: &str, path: &Path) -> Result<Config, String> {
    let config: Config =
        toml::from_str(text).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
    validate(&config)?;
    Ok(config)
}

pub fn validate(config: &Config) -> Result<(), String> {
    if config.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version: {}",
            config.schema_version
        ));
    }
    if !(1..=30).contains(&config.client.connect_timeout_seconds) {
        return Err("client.connect_timeout_seconds must be between 1 and 30".into());
    }
    if !(1..=16).contains(&config.client.max_parallel_probes) {
        return Err("client.max_parallel_probes must be between 1 and 16".into());
    }
    if !valid_id(&config.client.machine_id) {
        return Err(format!("invalid machine id: {}", config.client.machine_id));
    }
    let local = config
        .machines
        .get(&config.client.machine_id)
        .ok_or_else(|| {
            format!(
                "client.machine_id is not configured: {}",
                config.client.machine_id
            )
        })?;
    if local.ssh_alias.is_some() {
        return Err("client.machine_id must be local and cannot have ssh_alias".into());
    }
    if let Some(vm) = &config.vm {
        if let Some(tmux) = &vm.guest_tmux {
            if tmux.config_text.is_some() && tmux.config_path.is_some() {
                return Err("vm.guest_tmux cannot set both config_text and config_path".into());
            }
            if let Some(text) = &tmux.config_text
                && text.contains('\0')
            {
                return Err("vm.guest_tmux.config_text cannot contain NUL bytes".into());
            }
            if let Some(path) = &tmux.config_path {
                if path.as_os_str().is_empty() {
                    return Err("vm.guest_tmux.config_path cannot be empty".into());
                }
                if path.to_string_lossy().chars().any(|c| c == '\0') {
                    return Err("vm.guest_tmux.config_path cannot contain NUL bytes".into());
                }
            }
        }
        if let Some(defaults) = &vm.defaults {
            let mut packages = BTreeSet::new();
            for package in &defaults.packages {
                if !valid_package_attr(package) || !packages.insert(package) {
                    return Err(format!(
                        "vm.defaults.packages contains invalid or duplicate attr {package:?}"
                    ));
                }
            }
            let mut targets = BTreeSet::new();
            for file in &defaults.files {
                if !valid_guest_file_target(&file.target) || !targets.insert(file.target.as_str()) {
                    return Err(format!(
                        "vm.defaults.files contains invalid or duplicate target {:?}",
                        file.target
                    ));
                }
                if file.text.is_some() && file.source_path.is_some() {
                    return Err("vm.defaults.files cannot set both text and source_path".into());
                }
                if let Some(text) = &file.text
                    && text.contains('\0')
                {
                    return Err("vm.defaults.files.text cannot contain NUL bytes".into());
                }
                if let Some(path) = &file.source_path {
                    if path.as_os_str().is_empty() {
                        return Err("vm.defaults.files.source_path cannot be empty".into());
                    }
                    if path.to_string_lossy().chars().any(|c| c == '\0') {
                        return Err("vm.defaults.files.source_path cannot contain NUL bytes".into());
                    }
                }
                if let Some(mode) = &file.mode
                    && !valid_file_mode(mode)
                {
                    return Err("vm.defaults.files.mode must be a 3 or 4 digit octal mode".into());
                }
            }
        }
    }
    for (id, machine) in &config.machines {
        if !valid_id(id) {
            return Err(format!("invalid machine id: {id}"));
        }
        if id != &config.client.machine_id {
            let alias = machine
                .ssh_alias
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("machine {id} requires a non-empty ssh_alias"))?;
            if alias.len() > 255
                || alias.starts_with('-')
                || !alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b))
            {
                return Err(format!("invalid ssh_alias for machine {id}"));
            }
        }
        if let Some(socket) = &machine.socket {
            if socket.is_empty() {
                return Err(format!("invalid socket for machine {id}"));
            }
            if socket.chars().any(|c| c == '\0' || c.is_control()) {
                return Err(format!("invalid socket for machine {id}"));
            }
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

fn valid_guest_file_target(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 4096
        && !value.contains('\0')
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|part| part == "..")
}

fn valid_file_mode(value: &str) -> bool {
    (value.len() == 3 || value.len() == 4) && value.bytes().all(|b| (b'0'..=b'7').contains(&b))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(s: &str) -> Result<Config, String> {
        let c: Config = toml::from_str(s).map_err(|e| e.to_string())?;
        validate(&c)?;
        Ok(c)
    }
    #[test]
    fn defaults_and_schema() {
        let c =
            parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n").unwrap();
        assert_eq!(c.client.connect_timeout_seconds, 3);
        assert_eq!(c.client.max_parallel_probes, 4);
    }
    #[test]
    fn rejects_unknown() {
        assert!(
            parse("schema_version=1\nextra=1\n[client]\nmachine_id='local'\n[machines.local]\n")
                .is_err()
        );
    }
    #[test]
    fn validates_machine_and_alias() {
        assert!(parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n[machines.remote]\nssh_alias='-bad'\n").is_err());
    }
    #[test]
    fn socket_controls_rejected() {
        assert!(parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\nsocket=\"a\\u0000\"\n").is_err());
    }

    #[test]
    fn validates_vm_defaults() {
        let c = parse(
            "schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n[vm.defaults]\npackages=['ripgrep']\n[[vm.defaults.files]]\ntarget='/root/.config/example'\ntext='hello'\nmode='0600'\n",
        )
        .unwrap();
        let defaults = c.vm.unwrap().defaults.unwrap();
        assert_eq!(defaults.packages, vec!["ripgrep"]);
        assert_eq!(defaults.files[0].target, "/root/.config/example");
    }

    #[test]
    fn rejects_bad_vm_defaults() {
        assert!(parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n[vm.defaults]\npackages=['../bad']\n").is_err());
        assert!(parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n[[vm.defaults.files]]\ntarget='relative'\ntext='x'\n").is_err());
        assert!(parse("schema_version=1\n[client]\nmachine_id='local'\n[machines.local]\n[[vm.defaults.files]]\ntarget='/root/x'\ntext='x'\nmode='bad'\n").is_err());
    }
}
