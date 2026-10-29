//! The only module that talks to the microsandbox SDK. Everything it exposes is plain msbx data.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use microsandbox::logs::LogOptions;
use microsandbox::sandbox::{
    MAX_SANDBOX_LIST_LIMIT, RootfsSource, SandboxHandle, SandboxStatus, VolumeMount,
};
use microsandbox::{
    MAX_SANDBOX_NAME_BYTES, MicrosandboxError, NetworkPolicy, NetworkProfile, Sandbox,
    SandboxMetrics, SecretSource,
};
use microsandbox_types::{Action, Destination, DestinationGroup, Direction, NetworkSpec};

use crate::agents::{self, Agent};
use crate::model::{SandboxInfo, Status};

const LABEL_AGENT: &str = "msbx.agent";
const LABEL_PROJECT: &str = "msbx.project";
const INSTALL_MARKER: &str = "/root/.msbx-installed";

/// Guest memory size. This is a ceiling, not a reservation: the host only backs pages the guest
/// actually touches, so an idle sandbox costs far less than this.
const MEMORY_MIB: u32 = 32 * 1024;

/// How long a graceful stop may take before we give up (without killing the VM).
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Lines of install output kept in the error when the one-time install fails.
const INSTALL_ERROR_LINES: usize = 40;

/// Points the SDK at the runtime behind the `msb` found on `PATH`.
///
/// The SDK only discovers its runtime through `MSB_PATH` or `~/.microsandbox/bin`. Homebrew
/// instead installs `bin/msb` as a wrapper script around `libexec/msb` (next to libkrunfw), so
/// without this the SDK cannot find the runtime the `msb` CLI uses. `MSB_PATH` still wins.
pub(crate) fn use_runtime(msb_on_path: &Path) {
    microsandbox::config::set_sdk_msb_path(runtime_binary(msb_on_path));
}

fn runtime_binary(msb_on_path: &Path) -> PathBuf {
    let resolved = msb_on_path
        .canonicalize()
        .unwrap_or_else(|_| msb_on_path.to_path_buf());
    match resolved.parent().map(|bin| bin.join("../libexec/msb")) {
        Some(homebrew) if homebrew.is_file() => homebrew,
        _ => resolved,
    }
}

/// `<agent>-<folder>`, reduced to `[a-z0-9-]` and the SDK's name length limit.
pub(crate) fn sandbox_name(agent: &Agent, project: &Path) -> String {
    let folder = project
        .file_name()
        .map(|name| sanitise(&name.to_string_lossy()))
        .unwrap_or_default();
    let mut name = if folder.is_empty() {
        agent.id.to_owned()
    } else {
        format!("{}-{folder}", agent.id)
    };
    // Sanitised names are ASCII, so truncating by bytes cannot split a character.
    name.truncate(MAX_SANDBOX_NAME_BYTES);
    name.trim_end_matches('-').to_owned()
}

fn sanitise(folder: &str) -> String {
    let mut out = String::with_capacity(folder.len());
    for c in folder.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c != '-' || !out.ends_with('-') {
            out.push(c);
        }
    }
    out.trim_matches('-').to_owned()
}

/// Connects to (or creates and boots) the agent's sandbox for `project`, installs the agent on
/// first use, and returns the sandbox name. The VM keeps running after this returns.
///
/// `project` must be absolute; it is mounted and used as the working directory at the same path.
pub(crate) async fn ensure(agent: &Agent, project: &Path) -> Result<String> {
    let name = sandbox_name(agent, project);
    let project_path = project
        .to_str()
        .with_context(|| format!("project path {} is not valid UTF-8", project.display()))?;
    check_existing(&name, project_path).await?;

    let (secrets, missing) = agents::select_secrets(agent, |var| std::env::var(var).ok());
    for var in missing {
        eprintln!("note: {var} is not set on the host; not forwarding it to {name}");
    }
    let mut builder = Sandbox::builder(&name)
        .image(agents::IMAGE)
        .cpus(host_cpus())
        .memory(MEMORY_MIB)
        .volume(project_path, |mount| mount.bind(project))
        .workdir(project_path)
        .label(LABEL_AGENT, agent.id)
        .label(LABEL_PROJECT, project_path)
        .network(|network| {
            network.policy(NetworkPolicy::from_profiles([
                NetworkProfile::Public,
                NetworkProfile::Host,
            ]))
        })
        .detached(true);
    for secret in secrets {
        // Store only a reference: the SDK reads the host variable each time it boots the VM, so
        // the key never lands in microsandbox's database (unlike `secret_env`, which stores it).
        builder = builder.secret(|entry| {
            entry
                .env(secret.env_var)
                .source(SecretSource::env(secret.env_var))
                .allow(secret.host)
        });
    }
    let sandbox = builder
        .connect_or_create()
        .await
        .with_context(|| format!("starting sandbox {name}"))?;

    let installed = install_once(&sandbox, agent, &name).await;
    // A detached sandbox's handle never owns the VM, so releasing it leaves the VM running.
    sandbox.detach().await;
    installed.map(|()| name)
}

/// Two folders with the same name map to the same sandbox; refuse to hand one project's
/// sandbox (and its mount) to another.
async fn check_existing(name: &str, project: &str) -> Result<()> {
    let handle = match Sandbox::get(name).await {
        Ok(handle) => handle,
        Err(MicrosandboxError::SandboxNotFound(_)) => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("looking up sandbox {name}")),
    };
    let config = handle
        .config()
        .with_context(|| format!("reading config of sandbox {name}"))?;
    match config.spec.labels.get(LABEL_PROJECT) {
        Some(existing) if existing == project => Ok(()),
        Some(existing) => bail!(
            "sandbox {name} already exists for {existing}; remove it with `msb rm -f {name}` \
             or run msbx from that folder"
        ),
        None => bail!("sandbox {name} already exists and was not created by msbx"),
    }
}

fn host_cpus() -> u8 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    u8::try_from(cores).unwrap_or(u8::MAX)
}

async fn install_once(sandbox: &Sandbox, agent: &Agent, name: &str) -> Result<()> {
    let probe = sandbox
        .shell(format!("test -f {INSTALL_MARKER}"))
        .await
        .with_context(|| format!("checking install marker in {name}"))?;
    if probe.status().success {
        return Ok(());
    }

    eprintln!("installing {} in {name} (first run only)...", agent.id);
    let script = format!("set -e\n{}\ntouch {INSTALL_MARKER}", agent.install);
    let output = sandbox
        .shell(script)
        .await
        .with_context(|| format!("running install script in {name}"))?;
    if !output.status().success {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(output.stdout_bytes()),
            String::from_utf8_lossy(output.stderr_bytes())
        );
        bail!(
            "installing {} in {name} failed with exit code {}:\n{}",
            agent.id,
            output.status().code,
            last_lines(&combined, INSTALL_ERROR_LINES).join("\n")
        );
    }
    Ok(())
}

fn last_lines(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// Every sandbox the local runtime knows about, msbx-created or not.
#[allow(
    dead_code,
    reason = "used by the TUI, which lands in a separate change"
)]
pub(crate) async fn list() -> Result<Vec<SandboxInfo>> {
    // Metrics only decorate the list; a failure to read them must not hide the sandboxes.
    let metrics = microsandbox::all_sandbox_metrics()
        .await
        .unwrap_or_default();
    let mut infos = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page_cursor = cursor.take();
        let page = Sandbox::list_with(|list| {
            let list = list.limit(MAX_SANDBOX_LIST_LIMIT);
            match page_cursor {
                Some(cursor) => list.cursor(cursor),
                None => list,
            }
        })
        .await
        .context("listing sandboxes")?;
        infos.extend(
            page.sandboxes
                .iter()
                .map(|handle| info(handle, metrics.get(handle.name()))),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(infos),
        }
    }
}

fn info(handle: &SandboxHandle, metrics: Option<&SandboxMetrics>) -> SandboxInfo {
    // An unreadable stored config (e.g. from a newer runtime) leaves the detail fields empty.
    let config = handle.config().ok();
    let spec = config.as_ref().map(|config| &config.spec);
    let configured_memory = spec.map(|spec| u64::from(spec.resources.memory_mib) * 1024 * 1024);
    SandboxInfo {
        name: handle.name().to_owned(),
        status: status(handle.status_snapshot()),
        agent: spec.and_then(|spec| spec.labels.get(LABEL_AGENT).cloned()),
        project: spec.and_then(|spec| spec.labels.get(LABEL_PROJECT).map(PathBuf::from)),
        image: spec.map(|spec| image(&spec.image)),
        vcpus: spec.map(|spec| spec.resources.cpus),
        cpu_percent: metrics.map(|metrics| f64::from(metrics.cpu_percent)),
        memory_bytes: metrics.map(|metrics| metrics.memory_bytes),
        memory_limit_bytes: metrics
            .map(|metrics| metrics.memory_limit_bytes)
            .or(configured_memory),
        uptime: metrics.map(|metrics| metrics.uptime),
        network: spec.map(|spec| network_summary(&spec.network)),
        secrets: spec
            .and_then(|spec| spec.network.secrets.as_ref())
            .map(|secrets| secrets.secrets.iter().map(|s| s.env_var.clone()).collect())
            .unwrap_or_default(),
        mounts: spec
            .map(|spec| spec.mounts.iter().map(mount).collect())
            .unwrap_or_default(),
    }
}

fn status(status: SandboxStatus) -> Status {
    match status {
        SandboxStatus::Running => Status::Running,
        SandboxStatus::Starting => Status::Starting,
        SandboxStatus::Stopped => Status::Stopped,
        SandboxStatus::Crashed => Status::Crashed,
        _ => Status::Other,
    }
}

fn image(source: &RootfsSource) -> String {
    match source {
        RootfsSource::Oci(oci) => oci.reference.clone(),
        RootfsSource::Bind { path, .. } | RootfsSource::DiskImage { path, .. } => {
            path.display().to_string()
        }
    }
}

fn mount(mount: &VolumeMount) -> String {
    match mount {
        VolumeMount::Bind { host, guest, .. } => format!("{} -> {guest}", host.display()),
        VolumeMount::Named { name, guest, .. } => format!("{name} -> {guest}"),
        other => other.guest().to_owned(),
    }
}

/// Names the destination groups the policy allows outbound, e.g. `public + host`.
fn network_summary(network: &NetworkSpec) -> String {
    if !network.enabled {
        return "disabled".to_owned();
    }
    let Some(policy) = &network.policy else {
        return "default".to_owned();
    };
    if policy.default_egress == Action::Allow {
        return "unrestricted".to_owned();
    }
    let groups: Vec<&str> = policy
        .rules
        .iter()
        // Port-scoped rules (such as the DNS allowance every profile adds) are not profiles.
        .filter(|rule| {
            rule.action == Action::Allow
                && rule.direction != Direction::Ingress
                && rule.ports.is_empty()
        })
        .filter_map(|rule| match rule.destination {
            Destination::Group(DestinationGroup::Public) => Some("public"),
            Destination::Group(DestinationGroup::Private) => Some("private"),
            Destination::Group(DestinationGroup::Host) => Some("host"),
            _ => None,
        })
        .collect();
    if groups.is_empty() {
        "custom".to_owned()
    } else {
        groups.join(" + ")
    }
}

#[allow(
    dead_code,
    reason = "used by the TUI, which lands in a separate change"
)]
pub(crate) async fn start(name: &str) -> Result<()> {
    let sandbox = Sandbox::start_detached(name)
        .await
        .with_context(|| format!("starting sandbox {name}"))?;
    sandbox.detach().await;
    Ok(())
}

/// Stops gracefully; on timeout returns an error and leaves the VM running rather than killing it.
#[allow(
    dead_code,
    reason = "used by the TUI, which lands in a separate change"
)]
pub(crate) async fn stop(name: &str) -> Result<()> {
    let handle = get(name).await?;
    stop_handle(&handle).await
}

async fn stop_handle(handle: &SandboxHandle) -> Result<()> {
    handle
        .stop_with_timeout(STOP_TIMEOUT)
        .await
        .with_context(|| format!("stopping sandbox {}", handle.name()))
}

#[allow(
    dead_code,
    reason = "used by the TUI, which lands in a separate change"
)]
pub(crate) async fn remove(name: &str) -> Result<()> {
    let handle = get(name).await?;
    if handle.status_snapshot().has_active_runtime_state() {
        stop_handle(&handle).await?;
    }
    handle
        .remove()
        .await
        .with_context(|| format!("removing sandbox {name}"))
}

/// The last `max_lines` lines of the sandbox's captured output.
#[allow(
    dead_code,
    reason = "used by the TUI, which lands in a separate change"
)]
pub(crate) async fn logs(name: &str, max_lines: usize) -> Result<Vec<String>> {
    let handle = get(name).await?;
    // Entries are output chunks holding at least one line each, so this tail is enough.
    let options = LogOptions {
        tail: Some(max_lines),
        ..LogOptions::default()
    };
    let entries = handle
        .logs(&options)
        .await
        .with_context(|| format!("reading logs of sandbox {name}"))?;
    let text: String = entries
        .iter()
        .map(|entry| String::from_utf8_lossy(&entry.data))
        .collect();
    Ok(last_lines(&text, max_lines))
}

async fn get(name: &str) -> Result<SandboxHandle> {
    Sandbox::get(name)
        .await
        .with_context(|| format!("looking up sandbox {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentId;
    use microsandbox_types::{NetworkPolicy as StoredPolicy, PortRange, Protocol, Rule};

    fn name_for(folder: &str) -> String {
        sandbox_name(AgentId::Shell.agent(), &Path::new("/work").join(folder))
    }

    #[test]
    fn names_are_lowercased_and_sanitised() {
        assert_eq!(name_for("sandbox-test"), "shell-sandbox-test");
        assert_eq!(name_for("My Project"), "shell-my-project");
        assert_eq!(name_for("my.app.v2"), "shell-my-app-v2");
        assert_eq!(name_for("--Weird__Name--"), "shell-weird-name");
        assert_eq!(name_for("a  .  b"), "shell-a-b");
    }

    #[test]
    fn non_ascii_becomes_dashes() {
        assert_eq!(name_for("Café Ünïcödé"), "shell-caf-n-c-d");
        assert_eq!(name_for("日本語"), "shell");
        assert_eq!(sandbox_name(AgentId::Pi.agent(), Path::new("/")), "pi");
    }

    #[test]
    fn long_names_fit_the_sdk_limit_without_a_trailing_dash() {
        let long = name_for(&"a".repeat(300));
        assert_eq!(long.len(), MAX_SANDBOX_NAME_BYTES);
        assert!(long.starts_with("shell-aaa"));

        // "shell-" + 121 'a' is 127 bytes; the dash would be byte 128 and must be trimmed.
        let cut_at_dash = name_for(&format!("{}-bbbb", "a".repeat(121)));
        assert_eq!(cut_at_dash.len(), 127);
        assert!(!cut_at_dash.ends_with('-'));
    }

    #[test]
    fn generated_names_pass_sdk_validation() {
        for folder in ["x", "My Project", "日本語", "..", &"z".repeat(500)] {
            let name = name_for(folder);
            microsandbox::validate_sandbox_name(&name)
                .unwrap_or_else(|error| panic!("{name:?}: {error}"));
        }
    }

    #[test]
    fn last_lines_keeps_the_tail() {
        assert_eq!(last_lines("a\nb\nc\n", 2), ["b", "c"]);
        assert_eq!(last_lines("a", 5), ["a"]);
        assert_eq!(last_lines("", 5), [] as [String; 0]);
    }

    fn group_rule(group: DestinationGroup, ports: Vec<PortRange>) -> Rule {
        Rule {
            direction: Direction::Egress,
            destination: Destination::Group(group),
            protocols: vec![Protocol::Tcp],
            ports,
            action: Action::Allow,
        }
    }

    #[test]
    fn network_summary_names_allowed_groups_and_ignores_dns() {
        // Mirrors NetworkPolicy::from_profiles([Public, Host]) in microsandbox-network 0.7.11.
        let mut network = NetworkSpec {
            enabled: true,
            ..NetworkSpec::default()
        };
        network.policy = Some(StoredPolicy {
            default_egress: Action::Deny,
            default_ingress: Action::Allow,
            rules: vec![
                group_rule(
                    DestinationGroup::Host,
                    vec![PortRange { start: 53, end: 53 }],
                ),
                group_rule(DestinationGroup::Public, vec![]),
                group_rule(DestinationGroup::Host, vec![]),
            ],
        });
        assert_eq!(network_summary(&network), "public + host");

        network.enabled = false;
        assert_eq!(network_summary(&network), "disabled");
    }

    /// Boots a real VM: `cargo test -- --ignored`. Needs msb on PATH and network access.
    #[tokio::test]
    #[ignore = "boots a real microVM"]
    async fn shell_sandbox_shares_the_project_folder() -> Result<()> {
        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-it-{}", std::process::id()))
            .join("Shared Folder");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        let agent = AgentId::Shell.agent();

        let name = ensure(agent, &project).await?;
        let outcome = async {
            let sandbox = Sandbox::get(&name).await?.connect().await?;
            let written = sandbox.shell("echo from-guest > written-in-vm.txt").await;
            sandbox.detach().await;
            assert!(written?.status().success);
            let listed = list().await?.into_iter().find(|info| info.name == name);
            anyhow::Ok(listed)
        }
        .await;
        remove(&name).await?;

        let listed = outcome?.context("created sandbox missing from list()")?;
        assert_eq!(listed.agent.as_deref(), Some("shell"));
        assert_eq!(listed.project.as_deref(), Some(project.as_path()));
        assert_eq!(listed.network.as_deref(), Some("public + host"));
        let on_host = std::fs::read_to_string(project.join("written-in-vm.txt"))?;
        assert_eq!(on_host.trim(), "from-guest");
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;
        Ok(())
    }

    static SECRET_AGENT: Agent = Agent {
        id: "secret",
        name: "Secret test",
        install: "true",
        command: "true",
        secrets: &[agents::Secret {
            env_var: "MSBX_TEST_SECRET",
            host: "example.com",
        }],
    };

    /// Boots a real VM and proves the secret value is never persisted. Run with a value unique
    /// to this run, since SQLite free pages can keep bytes from earlier (deleted) rows:
    /// `MSBX_TEST_SECRET=msbx-test-secret-$RANDOM$RANDOM cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "boots a real microVM; needs MSBX_TEST_SECRET set"]
    async fn secret_values_are_not_stored() -> Result<()> {
        let value = std::env::var("MSBX_TEST_SECRET")
            .context("set MSBX_TEST_SECRET to a unique dummy value to run this test")?;
        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-secret-{}", std::process::id()))
            .join("proj");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;

        let name = ensure(&SECRET_AGENT, &project).await?;
        let outcome = async {
            let handle = Sandbox::get(&name).await?;
            let stored = format!(
                "{}{}",
                handle.config_json(),
                handle.active_config_json().unwrap_or_default()
            );
            let sandbox = handle.connect().await?;
            let guest = sandbox.shell("printenv MSBX_TEST_SECRET").await;
            sandbox.detach().await;
            let database = std::fs::read_dir(microsandbox::config::config()?.home().join("db"))?
                .map(|entry| std::fs::read(entry?.path()))
                .collect::<std::io::Result<Vec<_>>>()?
                .concat();
            anyhow::Ok((stored, guest?.stdout()?, database))
        }
        .await;
        remove(&name).await?;
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;

        let (stored, guest_env, database) = outcome?;
        assert!(stored.contains("MSBX_TEST_SECRET"), "secret entry missing");
        assert!(!stored.contains(&value), "stored config leaks the value");
        assert!(
            !database
                .windows(value.len())
                .any(|window| window == value.as_bytes()),
            "microsandbox database files contain the value"
        );
        assert!(!guest_env.contains(&value), "guest sees the real value");
        assert_eq!(guest_env.trim(), "$MSB_MSBX_TEST_SECRET");
        Ok(())
    }
}
