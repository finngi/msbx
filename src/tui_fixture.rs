//! Explicit debug-only plain data for driving the real terminal without a runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::model::{SandboxInfo, Status};
use crate::netlog::Entry;
use crate::policy::Policy;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fixture {
    pub(crate) workspace: PathBuf,
    #[serde(default, deserialize_with = "sandboxes")]
    pub(crate) sandboxes: Vec<SandboxInfo>,
    #[serde(default)]
    pub(crate) network: Vec<Entry>,
    #[serde(default)]
    pub(crate) policy: Option<Policy>,
    #[serde(default)]
    pub(crate) credentials: Vec<String>,
    #[serde(default)]
    pub(crate) logs: BTreeMap<String, Vec<String>>,
    /// Per sandbox, changed git files to show (RFC 0001); plain text, nothing is checked.
    #[serde(default)]
    pub(crate) git_changes: BTreeMap<String, Vec<String>>,
}

pub(crate) fn load(path: &Path) -> Result<Fixture> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading TUI fixture {}", path.display()))?;
    let mut fixture: Fixture = serde_json::from_str(&text)
        .with_context(|| format!("parsing TUI fixture {}", path.display()))?;
    ensure!(
        fixture.workspace.is_absolute(),
        "fixture workspace must be absolute"
    );
    fixture.workspace = fixture
        .workspace
        .canonicalize()
        .context("opening fixture workspace")?;
    ensure!(
        fixture.workspace.is_dir(),
        "fixture workspace must be a directory"
    );
    Ok(fixture)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxFixture {
    name: String,
    status: String,
    agent: Option<String>,
    project: Option<PathBuf>,
    image: Option<String>,
    created: Option<String>,
    vcpus: Option<u8>,
    cpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
    memory_limit_bytes: Option<u64>,
    disk_used_bytes: Option<u64>,
    disk_free_bytes: Option<u64>,
    uptime_secs: Option<u64>,
    network: Option<String>,
    #[serde(default)]
    proxied: bool,
    host_ports: Option<Vec<u16>>,
    user: Option<String>,
    restricted: Option<bool>,
    #[serde(default)]
    git_guarded: bool,
    rx_bytes: Option<u64>,
    tx_bytes: Option<u64>,
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    mounts: Vec<String>,
}

fn sandboxes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SandboxInfo>, D::Error> {
    Vec::<SandboxFixture>::deserialize(deserializer)?
        .into_iter()
        .map(|sandbox| sandbox.into_info().map_err(serde::de::Error::custom))
        .collect()
}

impl SandboxFixture {
    fn into_info(self) -> Result<SandboxInfo> {
        let status = match self.status.as_str() {
            "running" => Status::Running,
            "starting" => Status::Starting,
            "stopped" => Status::Stopped,
            "crashed" => Status::Crashed,
            "other" => Status::Other,
            other => bail!("unknown fixture sandbox status {other}"),
        };
        Ok(SandboxInfo {
            name: self.name,
            status,
            agent: self.agent,
            project: self.project,
            image: self.image,
            created: self.created,
            vcpus: self.vcpus,
            cpu_percent: self.cpu_percent,
            memory_bytes: self.memory_bytes,
            memory_limit_bytes: self.memory_limit_bytes,
            disk_used_bytes: self.disk_used_bytes,
            disk_free_bytes: self.disk_free_bytes,
            uptime: self.uptime_secs.map(Duration::from_secs),
            network: self.network,
            proxied: self.proxied,
            host_ports: self.host_ports,
            user: self.user,
            restricted: self.restricted,
            git_guarded: self.git_guarded,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
            secrets: self.secrets,
            mounts: self.mounts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_decodes_plain_data_and_rejects_bad_status() -> Result<()> {
        let fixture: Fixture = serde_json::from_str(
            r#"{
            "workspace":"/tmp",
            "sandboxes":[{"name":"codex-api","status":"running","agent":"codex","proxied":true}],
            "policy":{"preset":"balanced"},
            "logs":{"codex-api":["ready"]},
            "git_changes":{"codex-api":[".git/hooks/pre-commit (new)"]}
        }"#,
        )?;
        assert_eq!(
            fixture.git_changes.get("codex-api"),
            Some(&vec![".git/hooks/pre-commit (new)".to_owned()])
        );
        assert_eq!(
            fixture.sandboxes.first().context("fixture sandbox")?.status,
            Status::Running
        );
        assert_eq!(
            fixture.logs.get("codex-api"),
            Some(&vec!["ready".to_owned()])
        );
        assert!(
            serde_json::from_str::<Fixture>(
                r#"{"workspace":"/tmp","sandboxes":[{"name":"x","status":"bogus"}]}"#
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn fixture_requires_an_existing_absolute_directory() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("fixture.json");
        let outcome = (|| -> Result<()> {
            std::fs::write(&path, r#"{"workspace":"relative"}"#)?;
            assert!(load(&path).is_err());
            std::fs::write(&path, serde_json::json!({"workspace":root}).to_string())?;
            assert_eq!(load(&path)?.workspace, root.canonicalize()?);
            Ok(())
        })();
        std::fs::remove_dir_all(root)?;
        outcome
    }
}
