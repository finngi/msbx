//! `~/.config/msbx/config.toml`, the subset of RFC 0002 that RFC 0012 needs: the image, CPUs and
//! memory new sandboxes get, and per agent the image, the auto-approve flag and extra arguments.
//! A missing file means the built-in defaults; an unknown key or bad value is an error with its
//! line and column.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};

use crate::agents::{self, Agent, AgentId};
use crate::model::CreateOptions;

/// sbx's memory rule: half the host's RAM, clamped to 512 MiB..32 GiB.
const MIN_MEMORY_MIB: u32 = 512;
const MAX_MEMORY_MIB: u32 = 32 * 1024;
/// When the host's RAM cannot be read.
const FALLBACK_MEMORY_MIB: u32 = 4 * 1024;

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) defaults: Defaults,
    #[serde(default)]
    pub(crate) agents: BTreeMap<AgentId, AgentConfig>,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Defaults {
    #[serde(default, deserialize_with = "image")]
    pub(crate) image: Option<String>,
    #[serde(default, deserialize_with = "cpus")]
    pub(crate) cpus: Option<u8>,
    #[serde(default, deserialize_with = "memory_mib")]
    pub(crate) memory_mib: Option<u32>,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentConfig {
    #[serde(default, deserialize_with = "image")]
    pub(crate) image: Option<String>,
    /// Pass the agent's skip-permission-prompts flag; on unless set to `false`.
    pub(crate) auto_approve: Option<bool>,
    /// Added after the agent's default arguments on every launch.
    #[serde(default)]
    pub(crate) args: Vec<String>,
}

fn image<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let image = String::deserialize(deserializer)?;
    if image.is_empty() || image.contains(char::is_whitespace) {
        return Err(serde::de::Error::custom(
            "image must be an OCI reference such as \"node:24-bookworm\"",
        ));
    }
    Ok(Some(image))
}

fn cpus<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u8>, D::Error> {
    match u8::deserialize(deserializer)? {
        0 => Err(serde::de::Error::custom("cpus must be 1 to 255")),
        cpus => Ok(Some(cpus)),
    }
}

fn memory_mib<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    let memory = u32::deserialize(deserializer)?;
    if memory < MIN_MEMORY_MIB {
        return Err(serde::de::Error::custom(format!(
            "memory_mib must be at least {MIN_MEMORY_MIB}"
        )));
    }
    Ok(Some(memory))
}

impl Config {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    fn agent(&self, agent: &Agent) -> Option<&AgentConfig> {
        self.agents
            .iter()
            .find(|(id, _)| id.agent().id == agent.id)
            .map(|(_, settings)| settings)
    }

    /// What a new sandbox for `agent` is created with: built-in defaults, then `[defaults]`,
    /// then `[agents.<id>]`. `host_memory_mib` is the host's RAM, when known.
    pub(crate) fn create_options(
        &self,
        agent: &Agent,
        host_ports: Vec<u16>,
        host_memory_mib: Option<u64>,
    ) -> CreateOptions {
        let image = self
            .agent(agent)
            .and_then(|settings| settings.image.clone())
            .or_else(|| self.defaults.image.clone())
            .unwrap_or_else(|| agents::IMAGE.to_owned());
        CreateOptions {
            image,
            cpus: self.defaults.cpus.unwrap_or_else(host_cpus),
            memory_mib: self
                .defaults
                .memory_mib
                .unwrap_or_else(|| default_memory_mib(host_memory_mib)),
            host_ports,
        }
    }

    /// The arguments after the agent's command for this launch (see [`agents::launch_args`]).
    pub(crate) fn launch_args(&self, agent: &Agent, user_args: &[String]) -> Vec<String> {
        let settings = self.agent(agent);
        agents::launch_args(
            agent,
            settings.and_then(|settings| settings.auto_approve) != Some(false),
            settings.map_or(&[], |settings| settings.args.as_slice()),
            user_args,
        )
    }
}

/// Reads `path`; a missing file is the built-in defaults.
pub(crate) fn load(path: &Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Config::parse(&text).with_context(|| format!("reading config {}", path.display()))
}

/// Every host CPU, as the SDK's `u8` allows.
fn host_cpus() -> u8 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    u8::try_from(cores).unwrap_or(u8::MAX)
}

fn default_memory_mib(host_memory_mib: Option<u64>) -> u32 {
    host_memory_mib.map_or(FALLBACK_MEMORY_MIB, |host| {
        u32::try_from(host / 2)
            .unwrap_or(MAX_MEMORY_MIB)
            .clamp(MIN_MEMORY_MIB, MAX_MEMORY_MIB)
    })
}

/// The host's RAM in MiB: `sysctl -n hw.memsize` on macOS, `MemTotal` in `/proc/meminfo` on
/// Linux. `None` when neither can be read.
pub(crate) fn host_memory_mib() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
        return Some(bytes / (1024 * 1024));
    }
    meminfo_mib(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

fn meminfo_mib(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024;

    #[test]
    fn missing_or_empty_file_means_built_in_defaults() -> Result<()> {
        assert_eq!(Config::parse("")?, Config::default());
        let missing = std::env::temp_dir().join(format!("msbx-no-config-{}", std::process::id()));
        assert_eq!(load(&missing.join("config.toml"))?, Config::default());
        let options = Config::default().create_options(AgentId::Codex.agent(), vec![1234], None);
        assert_eq!(options.image, "node:24-bookworm");
        assert_eq!(options.cpus, host_cpus());
        assert_eq!(options.memory_mib, FALLBACK_MEMORY_MIB);
        assert_eq!(options.host_ports, [1234]);
        Ok(())
    }

    #[test]
    fn memory_is_half_the_host_clamped() {
        assert_eq!(default_memory_mib(Some(16 * GIB)), 8 * 1024);
        assert_eq!(default_memory_mib(Some(128 * GIB)), MAX_MEMORY_MIB);
        assert_eq!(default_memory_mib(Some(600)), MIN_MEMORY_MIB);
        assert_eq!(default_memory_mib(None), FALLBACK_MEMORY_MIB);
    }

    #[test]
    fn meminfo_total_is_read_in_mib() {
        let meminfo = "MemTotal:       16314740 kB\nMemFree:         1000 kB\n";
        assert_eq!(meminfo_mib(meminfo), Some(15932));
        assert_eq!(meminfo_mib("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn agent_settings_override_defaults() -> Result<()> {
        let config = Config::parse(
            r#"
            [defaults]
            image = "debian:bookworm"
            cpus = 4
            memory_mib = 8192

            [agents.claude]
            image = "ubuntu:24.04"
            auto_approve = false
            args = ["--model", "opus"]

            [agents.docker-agent]
            args = ["--debug"]
            "#,
        )?;
        let claude = config.create_options(AgentId::Claude.agent(), vec![], Some(64 * GIB));
        assert_eq!(
            (claude.image.as_str(), claude.cpus, claude.memory_mib),
            ("ubuntu:24.04", 4, 8192)
        );
        let codex = config.create_options(AgentId::Codex.agent(), vec![], None);
        assert_eq!(codex.image, "debian:bookworm");

        assert_eq!(
            config.launch_args(AgentId::Claude.agent(), &["-c".to_owned()]),
            ["--model", "opus", "-c"]
        );
        assert_eq!(
            config.launch_args(AgentId::DockerAgent.agent(), &[]),
            ["run", "--yolo", "--debug"]
        );
        assert_eq!(
            config.launch_args(AgentId::Codex.agent(), &[]),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        Ok(())
    }

    fn error(text: &str) -> String {
        Config::parse(text).map_or_else(|error| format!("{error:#}"), |_| String::new())
    }

    #[test]
    fn errors_name_the_line_and_the_problem() {
        let typo = error("[defaults]\ncpu = 4\n");
        assert!(
            typo.contains("line 2") && typo.contains("unknown field `cpu`"),
            "{typo}"
        );
        let agent = error("[agents.amp]\nargs = []\n");
        assert!(
            agent.contains("line 1") && agent.contains("unknown variant `amp`"),
            "{agent}"
        );
        assert!(error("[defaults]\ncpus = 0\n").contains("cpus must be 1 to 255"));
        assert!(error("[defaults]\ncpus = 300\n").contains("line 2"));
        assert!(error("[defaults]\nmemory_mib = 100\n").contains("at least 512"));
        assert!(error("[agents.pi]\nimage = \"\"\n").contains("OCI reference"));
        assert!(error("[agents.pi]\nauto_approve = \"no\"\n").contains("line 2"));
    }
}
