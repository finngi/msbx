//! The only module that talks to the microsandbox SDK. Everything it exposes is plain msbx data.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use microsandbox::logs::LogOptions;
use microsandbox::sandbox::{
    MAX_SANDBOX_LIST_LIMIT, RootfsSource, SandboxHandle, SandboxStatus, SecurityProfile,
    VolumeMount,
};
use microsandbox::{
    MAX_SANDBOX_NAME_BYTES, MicrosandboxError, NetworkAction, NetworkPolicy, Sandbox,
    SandboxMetrics, SecretSource, Volume,
};
use microsandbox_types::{Action, Destination, DestinationGroup, Direction, NetworkSpec};

use crate::agents::{self, Agent, Login};
use crate::model::{SandboxInfo, Status, validate_name};
use crate::{guard, hostgit, models, proxy};

/// Where progress and notes from setting up a sandbox go: stderr for the CLI, the status line
/// for the TUI (which must not print over its own screen).
pub(crate) type Notify<'a> = &'a (dyn Fn(String) + Send + Sync);

const LABEL_AGENT: &str = "msbx.agent";
const LABEL_PROJECT: &str = "msbx.project";
/// `proxy` marks sandboxes created to use the msbx proxy, so `policy.toml` applies to them.
const LABEL_NETWORK: &str = "msbx.network";
const NETWORK_PROXY: &str = "proxy";
/// `guarded` marks sandboxes whose git data is mounted read-only under a non-root agent
/// (RFC 0001); `none` means the project had no repository to guard. Absent on older sandboxes.
const LABEL_GIT: &str = "msbx.git";
const GIT_GUARDED: &str = "guarded";
const GIT_NONE: &str = "none";

/// The unprivileged user the agent runs as: uid 1000 in `node:24-bookworm-slim`. Not root, so
/// it cannot unmount the read-only git mounts; under the `restricted` profile neither can root.
const AGENT_USER: &str = "node";

/// Host ports the sandbox may reach directly (Ollama, LM Studio); fixed at create because
/// host-bound traffic never goes through the proxy.
const HOST_MODEL_PORTS: [u16; 2] = [11434, 1234];
const INSTALL_MARKER: &str = "/root/.msbx-installed";

/// Guest mount point of the per-agent login volume (RFC 0004), under the agent user's home.
const LOGIN_DIR: &str = ".msbx-login";

/// Login volumes are `msbx-login-<agent>`.
const LOGIN_VOLUME_PREFIX: &str = "msbx-login-";

/// Quota of a login volume. Fixed once a volume exists: the SDK rejects a different value.
const LOGIN_QUOTA_MIB: u32 = 16;

/// Exit code of the login script when the sandbox predates the login volume.
const LOGIN_NOT_MOUNTED: i32 = 3;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExistingMode {
    CreateOnly,
    Reuse,
}

/// Connects to (or creates and boots) the agent's sandbox for `project`, installs the agent on
/// first use, and returns the sandbox name. The VM keeps running after this returns.
///
/// `project` must be absolute; it is mounted and used as the working directory at the same path.
pub(crate) async fn ensure(agent: &Agent, project: &Path, notify: Notify<'_>) -> Result<String> {
    let name = sandbox_name(agent, project);
    ensure_with_mode(agent, project, &name, ExistingMode::Reuse, notify).await
}

/// Creates the dashboard's custom name, failing if another creator claims it first.
pub(crate) async fn ensure_named(
    agent: &Agent,
    project: &Path,
    name: &str,
    notify: Notify<'_>,
) -> Result<String> {
    ensure_with_mode(agent, project, name, ExistingMode::CreateOnly, notify).await
}

async fn ensure_with_mode(
    agent: &Agent,
    project: &Path,
    name: &str,
    mode: ExistingMode,
    notify: Notify<'_>,
) -> Result<String> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let project_path = project
        .to_str()
        .with_context(|| format!("project path {} is not valid UTF-8", project.display()))?;
    check_existing(mode, name, agent.id, project_path).await?;
    let login_volume = login_volume(agent);
    if let Some(volume) = &login_volume {
        ensure_login_volume(volume, agent.id).await?;
    }

    let static_policy = proxied_policy()?;
    let (secrets, missing) = agents::select_secrets(agent, |var| std::env::var(var).ok());
    for var in missing {
        notify(format!(
            "note: {var} is not set on the host; not forwarding it to {name} \
             (export it, or store it with `msbx secret set {var}`)"
        ));
    }
    let plan = guard::mounts(project)?;
    for note in &plan.notes {
        notify(note.clone());
    }
    let guards = plan.guards;
    let git_label = if guards.is_empty() {
        GIT_NONE
    } else {
        GIT_GUARDED
    };
    let mut builder = Sandbox::builder(name)
        .image(agents::IMAGE)
        .cpus(host_cpus())
        .memory(MEMORY_MIB)
        .volume(project_path, |mount| mount.bind(project))
        .workdir(project_path)
        // Not root, and no CAP_SYS_ADMIN even for root: the read-only git mounts below cannot
        // be unmounted from inside (RFC 0001, experiments 1 and 3).
        .user(AGENT_USER)
        .security(SecurityProfile::Restricted)
        .label(LABEL_AGENT, agent.id)
        .label(LABEL_PROJECT, project_path)
        .label(LABEL_NETWORK, NETWORK_PROXY)
        .label(LABEL_GIT, git_label)
        .network(|network| network.policy(static_policy))
        // Per-host decisions happen in the msbx proxy; the token is read from the host
        // environment at each start, like secrets, so it never lands in the database.
        .proxy(|proxy| {
            proxy
                .http_connect(proxy::ADDRESS)
                .credentials(name, SecretSource::env(proxy::TOKEN_VAR))
        })
        .detached(true);
    for guard in guards {
        // Same path as on the host, like the project itself; the git dir read-write so it
        // cannot be renamed or replaced, hooks, config and friends read-only.
        let guest = guard
            .path
            .to_str()
            .with_context(|| format!("path {} is not valid UTF-8", guard.path.display()))?;
        builder = builder.volume(guest, |mount| {
            let mount = mount.bind(&guard.path);
            if guard.readonly {
                mount.readonly()
            } else {
                mount
            }
        });
    }
    if let Some(volume) = &login_volume {
        // Files on the volume can be read and replaced, never run (RFC 0004 "Security").
        builder = builder.volume(login_mount(&home_dir(Some(AGENT_USER))), |mount| {
            mount.named(volume.as_str()).noexec().nosuid().nodev()
        });
    }
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
    // `connect_or_create` starts a concurrent winner with its persisted config. Dashboard
    // creation must instead fail atomically if the name was claimed after our early check.
    let sandbox = match mode {
        ExistingMode::CreateOnly => builder.create().await,
        ExistingMode::Reuse => builder.connect_or_create().await,
    }
    .with_context(|| format!("starting sandbox {name}"))?;

    let prepared = prepare(&sandbox, agent, name, notify).await;
    // A detached sandbox's handle never owns the VM, so releasing it leaves the VM running.
    sandbox.detach().await;
    prepared.map(|()| name.to_owned())
}

/// The static policy of a proxied sandbox: deny by default, DNS and the local model ports on the
/// host, and TCP to public addresses (which msb then sends through the proxy). Private, loopback,
/// link-local and metadata addresses match no rule, so the default denies them.
/// Rules are first-match (microsandbox-network 0.7.11 `NetworkPolicy::evaluate_egress`).
fn proxied_policy() -> Result<NetworkPolicy> {
    NetworkPolicy::builder()
        .default_egress(NetworkAction::Deny)
        .default_ingress(NetworkAction::Allow)
        .egress(|rule| rule.udp().tcp().port(53).allow_host())
        .egress(|rule| rule.tcp().ports(HOST_MODEL_PORTS).allow_host())
        .egress(|rule| rule.tcp().allow_public())
        .build()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("building the sandbox network policy")
}

/// Starts the agent's existing sandbox if needed and refreshes its login link and models, for
/// the TUI's attach, which skips `ensure`.
pub(crate) async fn prepare_existing(name: &str, agent: &Agent, notify: Notify<'_>) -> Result<()> {
    ensure_login_volume_for(name).await?;
    let sandbox = get(name)
        .await?
        .connect_or_start_detached()
        .await
        .with_context(|| format!("connecting to sandbox {name}"))?;
    let prepared = prepare(&sandbox, agent, name, notify).await;
    sandbox.detach().await;
    prepared
}

/// Readies a connected sandbox for the agent. Paths come from the sandbox's stored config, so a
/// sandbox created before the git guard (agent root, home `/root`) keeps working.
async fn prepare(sandbox: &Sandbox, agent: &Agent, name: &str, notify: Notify<'_>) -> Result<()> {
    let spec = &sandbox.config().spec;
    if !spec.labels.contains_key(LABEL_GIT) {
        notify(format!(
            "note: {name} was created before git guards; the agent runs as root and can change \
             .git/hooks and .git/config; recreate it with `msb rm -f {name}`"
        ));
    }
    let home = home_dir(spec.runtime.user.as_deref());
    install_once(sandbox, agent, name, notify).await?;
    link_login(sandbox, agent, name, &home, notify).await?;
    write_models(sandbox, agent, name, &home, notify).await?;
    seed_identity(sandbox, name).await
}

/// Home of the guest user the agent runs as; `None` is the image default, root.
fn home_dir(user: Option<&str>) -> String {
    match user {
        None | Some("root") => "/root".to_owned(),
        Some(user) => format!("/home/{}", user.split(':').next().unwrap_or(user)),
    }
}

fn login_mount(home: &str) -> String {
    format!("{home}/{LOGIN_DIR}")
}

/// The named volume holding the agent's sign-in, shared by all its sandboxes; `None` for
/// agents without a login file.
pub(crate) fn login_volume(agent: &Agent) -> Option<String> {
    agent
        .login
        .as_ref()
        .map(|_| format!("{LOGIN_VOLUME_PREFIX}{}", agent.id))
}

/// Checks for a shared sign-in without creating a volume or reading credential contents.
pub(crate) async fn shared_login(agent: &Agent) -> Result<Option<bool>> {
    let Some(login) = &agent.login else {
        return Ok(None);
    };
    let volume = login_volume(agent).context("agent login volume name")?;
    let handle = match Volume::get(&volume).await {
        Ok(handle) => handle,
        Err(MicrosandboxError::VolumeNotFound(_)) => return Ok(Some(false)),
        Err(error) => return Err(error).with_context(|| format!("looking up volume {volume}")),
    };
    handle
        .fs()
        .exists(login.file)
        .await
        .map(Some)
        .with_context(|| format!("checking shared login in {volume}"))
}

/// Creates the login volume unless it exists. Another msbx creating it at the same time is
/// fine: the volume is the same either way.
async fn ensure_login_volume(volume: &str, agent_id: &str) -> Result<()> {
    match Volume::get(volume).await {
        Ok(_) => return Ok(()),
        Err(MicrosandboxError::VolumeNotFound(_)) => {}
        Err(error) => return Err(error).with_context(|| format!("looking up volume {volume}")),
    }
    let created = Volume::builder(volume)
        .quota(LOGIN_QUOTA_MIB)
        .label(LABEL_AGENT, agent_id)
        .create()
        .await;
    match created {
        Ok(_) | Err(MicrosandboxError::VolumeAlreadyExists(_)) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("creating volume {volume}")),
    }
}

/// The SDK checks every named volume when a sandbox starts, so a login volume removed to sign
/// out everywhere must exist again (empty) before the sandbox can start.
async fn ensure_login_volume_for(name: &str) -> Result<()> {
    let config = get(name)
        .await?
        .config()
        .with_context(|| format!("reading config of sandbox {name}"))?;
    // The mount list, not the agent table, says which volume the sandbox needs.
    for mount in &config.spec.mounts {
        if let VolumeMount::Named { name: volume, .. } = mount
            && let Some(agent_id) = volume.strip_prefix(LOGIN_VOLUME_PREFIX)
        {
            ensure_login_volume(volume, agent_id).await?;
        }
    }
    Ok(())
}

/// Links the agent's credential file to the login volume, keeping a sign-in the agent made
/// since the last start. Sandboxes created before the volume existed get a note instead.
async fn link_login(
    sandbox: &Sandbox,
    agent: &Agent,
    name: &str,
    home: &str,
    notify: Notify<'_>,
) -> Result<()> {
    let Some(login) = &agent.login else {
        return Ok(());
    };
    // Root, so a file left on the volume by an older (root) sandbox can be handed to the agent user.
    let output = sandbox
        .shell_with(login_script(login, home), |options| options.user("root"))
        .await
        .with_context(|| format!("linking the {} login in {name}", agent.id))?;
    match output.status().code {
        0 => Ok(()),
        LOGIN_NOT_MOUNTED => {
            notify(format!(
                "note: {name} was created without a login volume, so its {} sign-in is not \
                 shared; recreate it with `msb rm -f {name}`",
                agent.id
            ));
            Ok(())
        }
        code => bail!(
            "linking the {} login in {name} failed with exit code {code}:\n{}",
            agent.id,
            String::from_utf8_lossy(output.stderr_bytes()).trim()
        ),
    }
}

/// Makes `$1/$2` a link to `$3/$2` on the login volume, owned by user `$4`. Paths arrive as
/// arguments, never as shell syntax, and the script runs unchanged on the host in tests.
const LINK_LOGIN: &str = r#"set -eu
umask 077
guest="$1/$2"
shared="$3/$2"
mkdir -p "$1"
# A sandbox could plant a link on the volume to redirect every other sandbox's writes.
if [ -L "$shared" ]; then rm -f "$shared"; fi
if [ -e "$shared" ] && [ ! -f "$shared" ]; then exit 1; fi
# The agent replaced the link with a plain file (a fresh sign-in): the newest wins.
if [ -f "$guest" ] && [ ! -L "$guest" ]; then mv -f "$guest" "$shared"; fi
if [ -e "$guest" ] && [ ! -L "$guest" ]; then exit 1; fi
ln -sfn "$shared" "$guest"
if [ -f "$shared" ]; then chmod 600 "$shared"; chown "$4" "$shared"; fi
chown -h "$4" "$1" "$guest"
"#;

/// Guest shell script that links the agent's credential file to the login volume.
fn login_script(login: &Login, home: &str) -> String {
    // Login paths come from the static agent table, never from user input; `home` is derived
    // from the stored sandbox user, which msbx set.
    let mount = login_mount(home);
    let owner = if home == "/root" { "root" } else { AGENT_USER };
    format!(
        "grep -q ' {mount} ' /proc/mounts || exit {LOGIN_NOT_MOUNTED}\n\
         set -- '{home}/{}' '{}' '{mount}' '{owner}'\n{LINK_LOGIN}",
        login.dir, login.file
    )
}

/// Guest shell script that gives the VM the host's git identity when it has none, so the agent
/// can commit: repository config is read-only in the VM (RFC 0001). Values arrive in the
/// environment, never in the script text.
const SEED_IDENTITY: &str = r#"command -v git >/dev/null 2>&1 || exit 0
git config --global user.name >/dev/null 2>&1 || git config --global user.name "$MSBX_GIT_NAME"
git config --global user.email >/dev/null 2>&1 || git config --global user.email "$MSBX_GIT_EMAIL"
"#;

async fn seed_identity(sandbox: &Sandbox, name: &str) -> Result<()> {
    let Some((user_name, email)) = hostgit::identity() else {
        return Ok(());
    };
    let output = sandbox
        .shell_with(SEED_IDENTITY, |options| {
            options
                .env("MSBX_GIT_NAME", user_name)
                .env("MSBX_GIT_EMAIL", email)
        })
        .await
        .with_context(|| format!("setting the git identity in {name}"))?;
    if !output.status().success {
        bail!(
            "setting the git identity in {name} failed with exit code {}:\n{}",
            output.status().code,
            String::from_utf8_lossy(output.stderr_bytes()).trim()
        );
    }
    Ok(())
}

/// Writes pi's `models.json` for the host's local model servers, or says why there is none.
async fn write_models(
    sandbox: &Sandbox,
    agent: &Agent,
    name: &str,
    home: &str,
    notify: Notify<'_>,
) -> Result<()> {
    let Some(models_file) = agent.models_file else {
        return Ok(());
    };
    let guest_path = &format!("{home}/{models_file}");
    // File reads and socket probes block, so keep them off the runtime threads.
    let json = tokio::task::spawn_blocking(models::guest_models_json)
        .await
        .context("looking for local model servers")??;
    let Some(json) = json else {
        notify(format!(
            "note: no local model server found on the host (Ollama on 11434, LM Studio on \
             1234), so {} in {name} has no models configured",
            agent.id
        ));
        return Ok(());
    };
    let output = sandbox
        .shell_with(models_script(guest_path), |options| {
            options.stdin_bytes(json)
        })
        .await
        .with_context(|| format!("writing {guest_path} in {name}"))?;
    if !output.status().success {
        bail!(
            "writing {guest_path} in {name} failed with exit code {}:\n{}",
            output.status().code,
            String::from_utf8_lossy(output.stderr_bytes()).trim()
        );
    }
    Ok(())
}

/// Guest shell script that stores stdin at `guest_path` with mode 600. The JSON arrives on
/// stdin, so it never passes through shell quoting.
fn models_script(guest_path: &str) -> String {
    let dir = guest_path.rsplit_once('/').map_or("/", |(dir, _)| dir);
    format!("set -e\numask 077\nmkdir -p '{dir}'\ncat > '{guest_path}'\nchmod 600 '{guest_path}'\n")
}

/// Two folders with the same name map to the same sandbox; refuse to hand one project's
/// sandbox (and its mount) to another.
async fn check_existing(mode: ExistingMode, name: &str, agent: &str, project: &str) -> Result<()> {
    let handle = match Sandbox::get(name).await {
        Ok(handle) => handle,
        Err(MicrosandboxError::SandboxNotFound(_)) => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("looking up sandbox {name}")),
    };
    let config = handle
        .config()
        .with_context(|| format!("reading config of sandbox {name}"))?;
    check_identity(
        mode,
        name,
        agent,
        project,
        config.spec.labels.get(LABEL_AGENT).map(String::as_str),
        config.spec.labels.get(LABEL_PROJECT).map(String::as_str),
    )
}

fn check_identity(
    mode: ExistingMode,
    name: &str,
    agent: &str,
    project: &str,
    existing_agent: Option<&str>,
    existing_project: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(
        mode == ExistingMode::Reuse,
        "sandbox {name} already exists; choose another name"
    );
    match existing_project {
        Some(existing) if existing == project && existing_agent == Some(agent) => Ok(()),
        Some(existing) if existing == project => bail!(
            "sandbox {name} belongs to agent {}; choose another name",
            existing_agent.unwrap_or("unknown")
        ),
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

async fn install_once(
    sandbox: &Sandbox,
    agent: &Agent,
    name: &str,
    notify: Notify<'_>,
) -> Result<()> {
    let probe = sandbox
        .shell_with(format!("test -f {INSTALL_MARKER}"), |options| {
            options.user("root")
        })
        .await
        .with_context(|| format!("checking install marker in {name}"))?;
    if probe.status().success {
        return Ok(());
    }

    notify(format!(
        "installing {} in {name} (first run only)...",
        agent.id
    ));
    let script = format!("set -e\n{}\ntouch {INSTALL_MARKER}", agent.install);
    // The agent runs unprivileged; installing needs root.
    let output = sandbox
        .shell_with(script, |options| options.user("root"))
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
        proxied: spec.is_some_and(|spec| {
            spec.labels.get(LABEL_NETWORK).map(String::as_str) == Some(NETWORK_PROXY)
        }),
        git_guarded: spec.is_some_and(|spec| spec.labels.contains_key(LABEL_GIT)),
        rx_bytes: metrics.map(|metrics| metrics.net_rx_bytes),
        tx_bytes: metrics.map(|metrics| metrics.net_tx_bytes),
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

pub(crate) async fn start(name: &str) -> Result<()> {
    ensure_login_volume_for(name).await?;
    let sandbox = Sandbox::start_detached(name)
        .await
        .with_context(|| format!("starting sandbox {name}"))?;
    sandbox.detach().await;
    Ok(())
}

/// Stops gracefully; on timeout returns an error and leaves the VM running rather than killing it.
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

    #[expect(
        clippy::needless_pass_by_value,
        reason = "matches the Notify signature, which hands each note over"
    )]
    fn print_note(note: String) {
        eprintln!("{note}");
    }
    use crate::agents::AgentId;
    use microsandbox_types::{NetworkPolicy as StoredPolicy, PortRange, Protocol, Rule};

    fn name_for(folder: &str) -> String {
        sandbox_name(AgentId::Shell.agent(), &Path::new("/work").join(folder))
    }

    #[test]
    fn dashboard_create_rejects_even_a_matching_existing_name() {
        assert!(
            check_identity(
                ExistingMode::CreateOnly,
                "chosen",
                "codex",
                "/work",
                Some("codex"),
                Some("/work"),
            )
            .is_err()
        );
    }

    #[test]
    fn custom_names_cannot_reuse_another_agent_or_project() {
        assert!(
            check_identity(
                ExistingMode::Reuse,
                "chosen",
                "codex",
                "/work",
                Some("codex"),
                Some("/work")
            )
            .is_ok()
        );
        assert!(
            check_identity(
                ExistingMode::Reuse,
                "chosen",
                "codex",
                "/work",
                Some("pi"),
                Some("/work")
            )
            .is_err()
        );
        assert!(
            check_identity(
                ExistingMode::Reuse,
                "chosen",
                "codex",
                "/work",
                Some("codex"),
                Some("/other")
            )
            .is_err()
        );
        assert!(
            check_identity(ExistingMode::Reuse, "chosen", "codex", "/work", None, None).is_err()
        );
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
            assert!(validate_name(&name).is_ok(), "{name:?}");
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

    /// The msbx dirs for a VM test, from `XDG_CONFIG_HOME` and `XDG_STATE_HOME`, which must be
    /// set (to temporary folders) so no test touches the real `~/.config/msbx`.
    fn test_dirs() -> Result<crate::paths::Dirs> {
        for var in ["XDG_CONFIG_HOME", "XDG_STATE_HOME"] {
            anyhow::ensure!(
                std::env::var_os(var).is_some_and(|value| !value.is_empty()),
                "set {var} to a temporary folder so the test never touches your real msbx files"
            );
        }
        crate::paths::Dirs::from_env()
    }

    /// An in-process proxy on the fixed port with `policy` and the token from
    /// `MSBX_PROXY_TOKEN` (which the SDK resolves when it boots the VM). The caller stops it with
    /// `proxy::stop` and awaits the handle.
    async fn test_proxy(
        dirs: &crate::paths::Dirs,
        policy: &crate::policy::Policy,
    ) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let token = std::env::var(proxy::TOKEN_VAR).context("set MSBX_PROXY_TOKEN")?;
        crate::paths::write_private(&dirs.token_file(), format!("{token}\n").as_bytes())?;
        crate::policy::save(&dirs.policy_file(), policy)?;
        let listener = tokio::net::TcpListener::bind(proxy::ADDRESS)
            .await
            .context("binding the proxy port; stop any running msbx proxy first")?;
        let timing = proxy::Timing {
            reload: Duration::from_millis(200),
            flush: Duration::from_millis(200),
            ..proxy::Timing::default()
        };
        Ok(tokio::spawn(proxy::serve_on(
            listener,
            dirs.clone(),
            timing,
        )))
    }

    /// Boots a real VM: `cargo test -- --ignored`. Needs msb on PATH, network access, and the
    /// variables described on `proxied_sandbox_follows_the_live_policy`.
    #[tokio::test]
    #[ignore = "boots a real microVM; needs XDG_CONFIG_HOME, XDG_STATE_HOME and MSBX_PROXY_TOKEN"]
    async fn shell_sandbox_shares_the_project_folder() -> Result<()> {
        let dirs = test_dirs()?;
        let open = crate::policy::Policy::new(crate::policy::Preset::Open);
        let server = test_proxy(&dirs, &open).await?;
        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-it-{}", std::process::id()))
            .join("Shared Folder");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        let agent = AgentId::Shell.agent();

        let created = ensure(agent, &project, &print_note).await;
        let outcome = async {
            let name = created?;
            let sandbox = Sandbox::get(&name).await?.connect().await?;
            let written = sandbox.shell("echo from-guest > written-in-vm.txt").await;
            sandbox.detach().await;
            let listed = list().await?.into_iter().find(|info| info.name == name);
            remove(&name).await?;
            assert!(written?.status().success);
            anyhow::Ok(listed)
        }
        .await;
        proxy::stop(&dirs).await?;
        tokio::time::timeout(Duration::from_secs(5), server).await???;

        let listed = outcome?.context("created sandbox missing from list()")?;
        assert_eq!(listed.agent.as_deref(), Some("shell"));
        assert_eq!(listed.project.as_deref(), Some(project.as_path()));
        assert_eq!(
            listed.network.as_deref(),
            Some("public"),
            "the port-scoped host rules are not profiles"
        );
        assert!(listed.proxied);
        let on_host = std::fs::read_to_string(project.join("written-in-vm.txt"))?;
        assert_eq!(on_host.trim(), "from-guest");
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;
        Ok(())
    }

    static SECRET_AGENT: Agent = Agent {
        id: "secret",
        description: "test agent",
        install: "true",
        command: "true",
        secrets: &[agents::Secret {
            env_var: "MSBX_TEST_SECRET",
            host: "example.com",
        }],
        login: None,
        models_file: None,
    };

    static LOGIN_AGENT: Agent = Agent {
        id: "logintest",
        description: "test agent",
        install: "true",
        command: "true",
        secrets: &[],
        login: Some(Login {
            dir: ".logintest",
            file: "auth.json",
        }),
        models_file: Some(".logintest/models.json"),
    };

    #[test]
    fn login_volume_is_named_after_the_agent() {
        assert_eq!(
            login_volume(AgentId::Codex.agent()).as_deref(),
            Some("msbx-login-codex")
        );
        assert_eq!(
            login_volume(AgentId::Pi.agent()).as_deref(),
            Some("msbx-login-pi")
        );
        assert_eq!(login_volume(AgentId::Shell.agent()), None);
    }

    #[cfg(unix)]
    fn run_link_script(root: &Path) -> Result<()> {
        // `chown` to the user running the test: the only owner it may set without privileges.
        let owner = std::process::Command::new("id").arg("-un").output()?;
        let output = std::process::Command::new("sh")
            .args(["-c", LINK_LOGIN, "msbx-login"])
            .arg(root.join("guest"))
            .arg("auth.json")
            .arg(root.join("volume"))
            .arg(String::from_utf8(owner.stdout)?.trim())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "link script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    /// Runs the guest link script on the host against a temporary "guest" and "volume".
    #[cfg(unix)]
    #[test]
    fn login_script_migrates_repairs_and_ignores_planted_links() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!("msbx-login-script-{}", std::process::id()));
        std::fs::create_dir(&root)?;
        let outcome = (|| -> Result<()> {
            std::fs::create_dir(root.join("volume"))?;
            run_link_script(&root)?;
            let guest = root.join("guest/auth.json");
            let shared = root.join("volume/auth.json");
            assert_eq!(std::fs::read_link(&guest)?, shared);
            assert!(!shared.exists(), "no empty auth file is seeded");

            // A write through the link lands on the volume.
            std::fs::write(&guest, "first login")?;
            run_link_script(&root)?;
            assert_eq!(std::fs::read_to_string(&shared)?, "first login");
            assert_eq!(
                std::fs::metadata(&shared)?.permissions().mode() & 0o777,
                0o600
            );

            // A fresh sign-in that replaced the link moves onto the volume.
            std::fs::remove_file(&guest)?;
            std::fs::write(&guest, "new login")?;
            run_link_script(&root)?;
            assert_eq!(std::fs::read_to_string(&shared)?, "new login");
            assert!(std::fs::symlink_metadata(&guest)?.file_type().is_symlink());

            // A link planted on the volume is removed, so its target is never written.
            std::fs::write(root.join("victim"), "unchanged")?;
            std::fs::remove_file(&shared)?;
            symlink(root.join("victim"), &shared)?;
            std::fs::remove_file(&guest)?;
            std::fs::write(&guest, "safe login")?;
            run_link_script(&root)?;
            assert_eq!(std::fs::read_to_string(root.join("victim"))?, "unchanged");
            assert!(!std::fs::symlink_metadata(&shared)?.file_type().is_symlink());
            assert_eq!(std::fs::read_to_string(&shared)?, "safe login");
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }

    #[test]
    fn login_script_checks_the_mount_and_passes_paths_as_arguments() {
        let login = Login {
            dir: ".codex",
            file: "auth.json",
        };
        let script = login_script(&login, "/home/node");
        assert!(
            script.starts_with("grep -q ' /home/node/.msbx-login ' /proc/mounts || exit 3\n"),
            "{script}"
        );
        assert!(
            script.contains(
                "set -- '/home/node/.codex' 'auth.json' '/home/node/.msbx-login' 'node'\n"
            ),
            "{script}"
        );
        assert!(script.ends_with(LINK_LOGIN), "{script}");

        // A sandbox created before the git guard keeps its root paths.
        let older = login_script(&login, "/root");
        assert!(
            older.contains("set -- '/root/.codex' 'auth.json' '/root/.msbx-login' 'root'\n"),
            "{older}"
        );
    }

    #[test]
    fn home_follows_the_stored_sandbox_user() {
        assert_eq!(home_dir(None), "/root");
        assert_eq!(home_dir(Some("root")), "/root");
        assert_eq!(home_dir(Some("node")), "/home/node");
        assert_eq!(home_dir(Some("node:node")), "/home/node");
        assert_eq!(login_mount("/home/node"), "/home/node/.msbx-login");
    }

    #[test]
    fn models_script_writes_stdin_with_mode_600() {
        assert_eq!(
            models_script("/home/node/.pi/agent/models.json"),
            "set -e\numask 077\nmkdir -p '/home/node/.pi/agent'\n\
             cat > '/home/node/.pi/agent/models.json'\n\
             chmod 600 '/home/node/.pi/agent/models.json'\n"
        );
    }

    /// Boots a real VM: `cargo test -- --ignored`. Checks the login link, that a fresh sign-in
    /// moves into the volume, that `start` recreates a removed volume, and pi's models.json.
    #[tokio::test]
    #[ignore = "boots a real microVM"]
    async fn login_file_is_linked_into_the_shared_volume() -> Result<()> {
        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-login-{}", std::process::id()))
            .join("proj");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        let volume = login_volume(&LOGIN_AGENT).context("login agent has a volume")?;
        let host_models = models::guest_models_json()?;

        let name = ensure(&LOGIN_AGENT, &project, &print_note).await?;
        // Boxed: the many awaits make this future too large for clippy's stack budget.
        let outcome = Box::pin(async {
            let sandbox = Sandbox::get(&name).await?.connect().await?;
            let first = sandbox
                .shell(
                    "set -e\n\
                     test \"$(readlink /home/node/.logintest/auth.json)\" = /home/node/.msbx-login/auth.json\n\
                     echo '{\"first\":1}' > /home/node/.logintest/auth.json\n\
                     test -f /home/node/.msbx-login/auth.json\n\
                     rm /home/node/.logintest/auth.json\n\
                     echo '{\"second\":2}' > /home/node/.logintest/auth.json\n\
                     cat /home/node/.logintest/models.json 2>/dev/null || true",
                )
                .await;
            sandbox.detach().await;
            let first = first?;
            anyhow::ensure!(first.status().success, "{}", first.stderr()?);
            let guest_models = first.stdout()?;
            match &host_models {
                Some(json) => assert_eq!(guest_models.trim(), json.trim()),
                None => assert!(guest_models.trim().is_empty(), "{guest_models}"),
            }

            // The second ensure moves the fresh sign-in into the volume and restores the link.
            ensure(&LOGIN_AGENT, &project, &print_note).await?;
            let sandbox = Sandbox::get(&name).await?.connect().await?;
            let second = sandbox
                .shell(
                    "test -L /home/node/.logintest/auth.json && \
                     stat -c %a /home/node/.msbx-login/auth.json && cat /home/node/.msbx-login/auth.json",
                )
                .await;
            sandbox.detach().await;
            let second = second?;
            anyhow::ensure!(second.status().success, "{}", second.stderr()?);
            assert_eq!(second.stdout()?.trim(), "600\n{\"second\":2}");
            let on_host = microsandbox::config::config()?
                .volumes_dir()
                .join(&volume)
                .join("auth.json");
            assert_eq!(std::fs::read_to_string(on_host)?.trim(), "{\"second\":2}");

            // Signing out everywhere removes the volume; `start` must bring an empty one back.
            stop(&name).await?;
            Volume::remove(&volume).await?;
            start(&name).await?;
            assert!(Volume::get(&volume).await.is_ok(), "start recreated the volume");
            anyhow::Ok(())
        })
        .await;
        remove(&name).await?;
        Volume::remove(&volume).await?;
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;
        outcome
    }

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

        let name = ensure(&SECRET_AGENT, &project, &print_note).await?;
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

    /// An agent whose install is a no-op; the image's `node` makes the HTTPS requests.
    static NET_AGENT: Agent = Agent {
        id: "nettest",
        description: "test agent",
        install: "true",
        command: "true",
        secrets: &[],
        login: None,
        models_file: None,
    };

    /// `node -e` script that exits 0 when an HTTPS GET to `$1` answers, 1 otherwise.
    const FETCH: &str = "fetch(process.argv[1], {signal: AbortSignal.timeout(15000)})\
        .then(r => process.exit(r.ok || r.status < 500 ? 0 : 1), () => process.exit(1))";

    /// Whether the guest can reach `host` over HTTPS through the proxy.
    async fn guest_reaches(name: &str, host: &str) -> Result<bool> {
        let sandbox = Sandbox::get(name).await?.connect().await?;
        let output = sandbox
            .shell(format!("node -e '{FETCH}' https://{host}/"))
            .await;
        sandbox.detach().await;
        Ok(output?.status().success)
    }

    /// Installs git so the agent user can commit; the host-side guard needs nothing else.
    static GUARD_AGENT: Agent = Agent {
        id: "guardtest",
        description: "git guard test agent",
        install: "apt-get update && apt-get install -y --no-install-recommends git",
        command: "true",
        secrets: &[],
        login: None,
        models_file: None,
    };

    /// Runs `script` in the sandbox as `user` and returns its stdout, failing on a non-zero exit.
    async fn guest(name: &str, user: &str, script: &str) -> Result<String> {
        let sandbox = Sandbox::get(name).await?.connect().await?;
        let output = sandbox
            .shell_with(script, |options| options.user(user))
            .await;
        sandbox.detach().await;
        let output = output?;
        anyhow::ensure!(
            output.status().success,
            "as {user}: {script}\n{}",
            output.stderr()?
        );
        Ok(output.stdout()?)
    }

    /// Boots a real VM and proves RFC 0001's prevention layer: the agent user and root can
    /// neither write hooks or config nor unmount the guards, the agent can still commit, and
    /// a hook that appears on the host side is reported. Same variables as the proxy test.
    #[tokio::test]
    #[ignore = "boots a real microVM; needs XDG_CONFIG_HOME, XDG_STATE_HOME and MSBX_PROXY_TOKEN"]
    async fn git_guard_holds_against_the_agent_and_root() -> Result<()> {
        use crate::guard;

        let dirs = test_dirs()?;
        let open = crate::policy::Policy::new(crate::policy::Preset::Open);
        let server = test_proxy(&dirs, &open).await?;
        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-guard-{}", std::process::id()))
            .join("proj");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        for args in [
            vec!["init", "-q"],
            vec!["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            let output = hostgit::command()
                .current_dir(&project)
                .args(["-c", "user.name=t", "-c", "user.email=t@t.test"])
                .args(&args)
                .output()?;
            anyhow::ensure!(output.status.success(), "git {args:?} failed");
        }

        let created = ensure(&GUARD_AGENT, &project, &print_note).await;
        let outcome = Box::pin(async {
            let name = created?;
            let config = Sandbox::get(&name).await?.config()?;
            assert_eq!(
                config.spec.labels.get(LABEL_GIT).map(String::as_str),
                Some(GIT_GUARDED)
            );
            assert_eq!(config.spec.runtime.user.as_deref(), Some(AGENT_USER));
            assert_eq!(config.spec.security_profile, SecurityProfile::Restricted);
            // The project, `.git`, and read-only `config`, `hooks` and `info` (a fresh repo
            // has no `config.worktree`).
            let guarded: Vec<String> = config.spec.mounts.iter().map(mount).collect();
            assert_eq!(guarded.len(), 5, "{guarded:?}");

            let (session, warning) = guard::Session::start(&dirs, &name, &project)?;
            assert_eq!(warning, None);
            let p = project.display();
            // Every attack line must fail; `!` turns that into success for `set -e`.
            let blocked = format!(
                "set -e\ncd '{p}'\n\
                 test \"$(id -un)\" = node\n\
                 ! echo evil > .git/hooks/pre-commit\n\
                 ! touch .git/info/x\n\
                 ! git config core.fsmonitor /tmp/evil\n\
                 ! git config core.hooksPath /tmp\n\
                 ! mv .git .git2\n\
                 ! mv .git/hooks .git/h2\n\
                 ! umount .git/hooks 2>/dev/null\n\
                 ! umount -l .git/config 2>/dev/null\n\
                 echo blocked-ok"
            );
            assert_eq!(
                guest(&name, AGENT_USER, &blocked).await?.trim(),
                "blocked-ok"
            );
            let as_root = format!(
                "set -e\ncd '{p}'\n\
                 ! umount .git/hooks 2>/dev/null\n\
                 ! mount -o remount,rw .git/hooks 2>/dev/null\n\
                 ! echo evil > .git/hooks/pre-commit\n\
                 ! echo '[core]' >> .git/config\n\
                 echo root-blocked-ok"
            );
            assert_eq!(
                guest(&name, "root", &as_root).await?.trim(),
                "root-blocked-ok"
            );
            let commit = format!(
                "set -e\ncd '{p}'\n\
                 git config --global user.name vm\n\
                 git config --global user.email vm@test\n\
                 echo change > a.txt\n\
                 git add a.txt\n\
                 git commit -q -m from-vm\n\
                 git branch feature\n\
                 git log --oneline | wc -l"
            );
            assert_eq!(guest(&name, AGENT_USER, &commit).await?.trim(), "2");
            let on_host = hostgit::command()
                .current_dir(&project)
                .args(["log", "--format=%s", "-1"])
                .output()?;
            assert_eq!(String::from_utf8_lossy(&on_host.stdout).trim(), "from-vm");
            assert_eq!(
                session.finish()?,
                None,
                "a commit is not a change to guard against"
            );

            // Something that got past the guard shows up in the session report.
            std::fs::write(project.join(".git/hooks/pre-commit"), "#!/bin/sh\nevil\n")?;
            let warning = session.finish()?.context("a planted hook is reported")?;
            assert!(warning.contains(".git/hooks/pre-commit (new)"), "{warning}");
            anyhow::Ok(name)
        })
        .await;
        if let Ok(name) = &outcome {
            remove(name).await?;
        }
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;
        proxy::stop(&dirs).await?;
        tokio::time::timeout(Duration::from_secs(5), server).await???;
        outcome.map(|_| ())
    }

    /// Boots a real VM through an in-process proxy on the fixed port and checks the live policy.
    /// Point every msbx path at temporary folders and give the proxy a one-off token:
    /// `XDG_CONFIG_HOME=$(mktemp -d) XDG_STATE_HOME=$(mktemp -d) MSBX_PROXY_TOKEN=$(openssl rand
    /// -hex 16) cargo test -- --ignored proxied`. Fails if another msbx proxy holds the port.
    #[tokio::test]
    #[ignore = "boots a real microVM; needs XDG_CONFIG_HOME, XDG_STATE_HOME and MSBX_PROXY_TOKEN"]
    async fn proxied_sandbox_follows_the_live_policy() -> Result<()> {
        use crate::netlog::Log;
        use crate::policy::{Policy, Preset, Target};

        let dirs = test_dirs()?;
        let mut policy = Policy::new(Preset::LockedDown);
        policy.allow.push(Target::parse("example.com")?);
        let server = test_proxy(&dirs, &policy).await?;

        use_runtime(&crate::attach::msb_path()?);
        let project = std::env::temp_dir()
            .join(format!("msbx-net-{}", std::process::id()))
            .join("proj");
        std::fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        let name = ensure(&NET_AGENT, &project, &print_note).await?;
        let outcome = Box::pin(async {
            let config = Sandbox::get(&name).await?.config()?;
            let stored = config.spec.network.policy.context("policy stored")?;
            assert_eq!(stored.default_egress, Action::Deny);
            assert!(config.spec.network.outbound_proxy.is_some(), "proxy stored");
            assert_eq!(
                config.spec.labels.get(LABEL_NETWORK).map(String::as_str),
                Some(NETWORK_PROXY)
            );

            assert!(guest_reaches(&name, "example.com").await?, "allowed host");
            assert!(!guest_reaches(&name, "pypi.org").await?, "blocked host");

            let mut policy = policy.clone();
            policy.allow.push(Target::parse("pypi.org")?);
            crate::policy::save(&dirs.policy_file(), &policy)?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(guest_reaches(&name, "pypi.org").await?, "allowed live");
            anyhow::Ok(())
        })
        .await;
        remove(&name).await?;
        std::fs::remove_dir_all(project.parent().context("temp project has a parent")?)?;
        proxy::stop(&dirs).await?;
        tokio::time::timeout(Duration::from_secs(5), server).await???;
        outcome?;

        let log = Log::load(&dirs.network_log())?;
        let rows: Vec<(&str, &str, bool)> = log
            .entries()
            .iter()
            .map(|entry| (entry.sandbox.as_str(), entry.host.as_str(), entry.allowed))
            .collect();
        assert!(
            rows.contains(&(name.as_str(), "example.com", true)),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(name.as_str(), "pypi.org", false)),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(name.as_str(), "pypi.org", true)),
            "{rows:?}"
        );
        Ok(())
    }
}
