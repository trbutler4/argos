use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub client: Client,
    pub machines: BTreeMap<String, Machine>,
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
}
