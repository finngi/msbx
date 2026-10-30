//! Filesystem and credential reads for the dashboard; no rendering or application state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::ValueEnum;

use crate::agents::{Agent, AgentId};
use crate::model::{NetEntry, RuleInfo};
use crate::netlog::{self, Entry, Log};
use crate::paths::Dirs;
use crate::policy::{self, Policy, Preset, Source, Verdict};
use crate::sandbox;

pub(crate) struct NetworkData {
    pub(crate) preset: Option<Preset>,
    /// The host ports new sandboxes get; `None` without a policy file.
    pub(crate) host_ports: Option<Vec<u16>>,
    pub(crate) entries: Vec<NetEntry>,
    pub(crate) rules: Vec<RuleInfo>,
}

pub(crate) fn read_network(dirs: &Dirs) -> Result<NetworkData> {
    let policy = policy::load(&dirs.policy_file())?;
    let log = Log::load(&dirs.network_log())?;
    Ok(NetworkData {
        preset: policy.as_ref().map(|policy| policy.preset),
        host_ports: policy.as_ref().map(|policy| policy.host_ports.clone()),
        entries: aggregate_network(log.entries()),
        rules: policy.as_ref().map_or_else(Vec::new, rule_rows),
    })
}

/// The raw log keeps separate rule/outcome rows; the dashboard groups one endpoint's history.
/// The log has second precision, so a blocked result wins an otherwise ambiguous time tie.
pub(crate) fn aggregate_network(entries: &[Entry]) -> Vec<NetEntry> {
    let mut groups: BTreeMap<(&str, &str, u16), Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        groups
            .entry((&entry.sandbox, &entry.host, entry.port))
            .or_default()
            .push(entry);
    }
    let mut rows: Vec<_> = groups
        .into_iter()
        .filter_map(|((sandbox, host, port), entries)| {
            let latest = entries
                .iter()
                .max_by_key(|entry| (entry.last_seen, !entry.allowed))?;
            let first_seen = entries.iter().map(|entry| entry.first_seen).min()?;
            let (allowed_count, blocked_count) =
                entries
                    .iter()
                    .fold((0_u64, 0_u64), |(allow, block), entry| {
                        if entry.allowed {
                            (allow.saturating_add(entry.count), block)
                        } else {
                            (allow, block.saturating_add(entry.count))
                        }
                    });
            Some((
                latest.last_seen,
                NetEntry {
                    sandbox: sandbox.to_owned(),
                    host: host.to_owned(),
                    port,
                    allowed: latest.allowed,
                    count: allowed_count.saturating_add(blocked_count),
                    allowed_count,
                    blocked_count,
                    first_seen: netlog::local_time(first_seen, "%Y-%m-%d %H:%M:%S"),
                    last_seen: netlog::local_time(latest.last_seen, "%Y-%m-%d %H:%M:%S"),
                },
            ))
        })
        .collect();
    rows.sort_by(|(time_a, a), (time_b, b)| {
        a.allowed
            .cmp(&b.allowed)
            .then(time_b.cmp(time_a))
            .then(a.host.cmp(&b.host))
            .then(a.port.cmp(&b.port))
            .then(a.sandbox.cmp(&b.sandbox))
    });
    rows.into_iter().map(|(_, entry)| entry).collect()
}

/// Shared rules appear once; sandbox rows retain the scope needed for later edits. Most specific
/// first, like `msbx policy ls`: sandbox rules, then global, then the preset baseline.
pub(crate) fn rule_rows(policy: &Policy) -> Vec<RuleInfo> {
    let shared = policy.rules(None).into_iter().map(|rule| RuleInfo {
        target: rule.target.to_string(),
        source: rule.source,
        sandbox: None,
        allowed: rule.verdict == Verdict::Allow,
    });
    let scoped = policy.sandboxes.keys().flat_map(|name| {
        policy
            .rules(Some(name))
            .into_iter()
            .filter(|rule| rule.source == Source::Sandbox)
            .map(|rule| RuleInfo {
                target: rule.target.to_string(),
                source: rule.source,
                sandbox: Some(name.clone()),
                allowed: rule.verdict == Verdict::Allow,
            })
    });
    scoped.chain(shared).collect()
}

/// Only the immediate directories are needed. Fixture navigation cannot leave its workspace.
pub(crate) fn list_folders(path: &Path, confined_to: Option<&Path>) -> Result<Vec<PathBuf>> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("opening folder {}", path.display()))?;
    if let Some(root) = confined_to {
        ensure!(
            canonical.starts_with(root),
            "fixture folders must stay inside {}",
            root.display()
        );
    }
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(&canonical)
        .with_context(|| format!("listing folder {}", path.display()))?
    {
        let entry = entry.with_context(|| format!("reading folder {}", path.display()))?;
        if entry.path().is_dir() {
            folders.push(path.join(entry.file_name()));
        }
    }
    folders.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .cmp(
                &b.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            )
            .then(a.cmp(b))
    });
    Ok(folders)
}

/// Startup already resolved shell/Keychain values into the process environment.
/// Only availability reaches application state; login checks never read credential contents.
pub(crate) async fn credentials() -> Vec<String> {
    let mut summaries = Vec::new();
    for id in AgentId::value_variants() {
        let agent = id.agent();
        let api_key = agent
            .secrets
            .iter()
            .all(|secret| std::env::var_os(secret.env_var).is_some_and(|value| !value.is_empty()));
        summaries.push(credential_summary(
            agent,
            api_key,
            sandbox::shared_login(agent).await,
        ));
    }
    summaries
}

fn credential_summary(agent: &Agent, api_key: bool, login: Result<Option<bool>>) -> String {
    let mut parts = Vec::new();
    if !agent.secrets.is_empty() {
        parts.push(format!(
            "OPENAI_API_KEY: {} (shell/Keychain)",
            if api_key { "available" } else { "missing" }
        ));
    }
    if let Some(volume) = sandbox::login_volume(agent) {
        let state = match login {
            Ok(Some(true)) => "present".to_owned(),
            Ok(Some(false) | None) => "missing".to_owned(),
            Err(error) => format!("unknown ({error:#})"),
        };
        parts.push(format!("{volume}: {state}"));
    }
    if parts.is_empty() {
        "no credentials required".to_owned()
    } else {
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_rows_list_sandbox_then_global_then_preset() -> Result<()> {
        let mut policy = Policy::new(Preset::Balanced);
        policy.allow(&[policy::Target::parse("cdn.example.com")?], None);
        policy.allow(&[policy::Target::parse("example.com")?], Some("codex-api"));
        let sources: Vec<_> = rule_rows(&policy).iter().map(|rule| rule.source).collect();
        let first_preset = sources
            .iter()
            .position(|source| *source == Source::Preset)
            .context("the balanced preset has rules")?;
        assert_eq!(&sources[..2], [Source::Sandbox, Source::Global]);
        assert!(
            sources[first_preset..]
                .iter()
                .all(|source| *source == Source::Preset)
        );
        Ok(())
    }

    #[test]
    fn credentials_distinguish_availability_missing_unknown_and_none() {
        let codex = AgentId::Codex.agent();
        let available = credential_summary(codex, true, Ok(Some(true)));
        assert!(available.contains("OPENAI_API_KEY: available (shell/Keychain)"));
        assert!(available.contains("msbx-login-codex: present"));
        assert!(credential_summary(codex, false, Ok(Some(false))).contains("missing"));
        assert!(
            credential_summary(
                AgentId::Pi.agent(),
                false,
                Err(anyhow::anyhow!("unavailable"))
            )
            .contains("unknown")
        );
        assert_eq!(
            credential_summary(AgentId::Shell.agent(), false, Ok(None)),
            "no credentials required"
        );
    }

    #[test]
    fn corrupt_network_files_remain_errors() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-read-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        std::fs::create_dir_all(&dirs.state)?;
        let outcome = (|| -> Result<()> {
            std::fs::write(dirs.network_log(), "not json")?;
            assert!(read_network(&dirs).is_err());
            std::fs::write(dirs.network_log(), "[]")?;
            assert_eq!(read_network(&dirs)?.entries, [] as [NetEntry; 0]);
            Ok(())
        })();
        std::fs::remove_dir_all(root)?;
        outcome
    }
}
