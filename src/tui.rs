//! Terminal setup, the event loop, and suspending the TUI to hand the terminal to `msb exec`.
//! All of the TUI's IO lives here so `app.rs` and `ui.rs` stay pure.

use std::collections::BTreeMap;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::ValueEnum;
use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{MissedTickBehavior, interval};

use crate::agents::{self, Agent, AgentId};
use crate::app::{Action, App};
use crate::model::SandboxInfo;
use crate::paths::Dirs;
use crate::policy::{self, Policy, Preset, Target};
use crate::tui_data::{self, NetworkData, read_network};
use crate::{attach, guard, proxy, sandbox, ui};

/// Redraws happen at most this often, and only when something changed.
const FRAME: Duration = Duration::from_millis(33);
const REFRESH: Duration = Duration::from_secs(2);
const LOG_LINES: usize = 500;
/// `x` opens this in any sandbox, including ones msbx did not create.
const SHELL: &str = "bash";

/// What the event loop does for an [`Action`].
#[derive(Debug, PartialEq)]
enum Step {
    Quit,
    /// Runs in a background task while the TUI stays live.
    Job(Job),
    /// Needs the real terminal, so the TUI is suspended around it.
    Suspend(Foreground),
    /// A small, immediate edit of `policy.toml`.
    Policy(Edit),
}

#[derive(Debug, PartialEq)]
enum Job {
    /// Creates (or starts) the agent's sandbox in the background; the dashboard stays up and
    /// ↵ attaches once it is ready.
    Create {
        agent: &'static Agent,
        project: PathBuf,
        name: String,
    },
    Folders(PathBuf),
    Start(String),
    Stop(String),
    Remove(String),
    Logs(String),
}

#[derive(Debug, PartialEq)]
enum Foreground {
    Attach {
        name: String,
        agent: &'static Agent,
        /// The project, when the sandbox has one, so its git files are checked around the session.
        project: Option<PathBuf>,
    },
    Exec {
        name: String,
        command: &'static str,
        project: Option<PathBuf>,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum Edit {
    Allow {
        target: String,
        sandbox: Option<String>,
    },
    Deny {
        target: String,
        sandbox: Option<String>,
    },
    Remove {
        target: String,
        sandbox: Option<String>,
    },
    Preset {
        preset: Preset,
        then_create: bool,
    },
    /// The `Ctrl-S` form: the preset and the host ports for new sandboxes, together.
    Settings {
        preset: Preset,
        host_ports: Vec<u16>,
    },
}

/// Results background tasks send back to the event loop, the single owner of [`App`].
enum Message {
    Sandboxes(Result<Vec<SandboxInfo>>),
    Logs {
        name: String,
        logs: Result<Vec<String>>,
    },
    Network(Result<NetworkData>),
    Credentials(Vec<String>),
    Folders {
        path: PathBuf,
        directories: Result<Vec<PathBuf>>,
    },
    Status(String),
    /// A sandbox the user just created; select its card once it is listed.
    Created(String),
    /// Per sandbox, the git files of its project that changed since its last session started.
    Git(BTreeMap<String, Vec<String>>),
}

/// Input plus periodic refresh. Dropping it stops both, so while the TUI is suspended nothing
/// else reads stdin or repaints.
struct Session {
    events: EventStream,
    refresh: Option<JoinHandle<()>>,
}

/// Fixture mode shares the real event/render loop but never reaches runtime or policy writes.
#[derive(Clone, Debug)]
enum RunMode {
    Live,
    #[cfg(debug_assertions)]
    Fixture {
        workspace: PathBuf,
        logs: std::collections::BTreeMap<String, Vec<String>>,
    },
}

impl RunMode {
    fn permits(&self, action: &Action) -> bool {
        #[cfg(not(debug_assertions))]
        let _ = action;
        match self {
            Self::Live => true,
            #[cfg(debug_assertions)]
            Self::Fixture { .. } => matches!(
                action,
                Action::Quit | Action::LoadFolders { .. } | Action::LoadLogs { .. }
            ),
        }
    }

    fn workspace(&self) -> Option<&Path> {
        match self {
            Self::Live => None,
            #[cfg(debug_assertions)]
            Self::Fixture { workspace, .. } => Some(workspace),
        }
    }

    fn fixture_logs(&self, name: &str) -> Option<Vec<String>> {
        #[cfg(not(debug_assertions))]
        let _ = name;
        match self {
            Self::Live => None,
            #[cfg(debug_assertions)]
            Self::Fixture { logs, .. } => Some(logs.get(name).cloned().unwrap_or_default()),
        }
    }
}

impl Session {
    fn start(sender: Sender<Message>, dirs: Dirs, mode: &RunMode) -> Self {
        let refresh = mode.workspace().is_none().then(|| {
            tokio::spawn(async move {
                let _ = sender
                    .send(Message::Credentials(tui_data::credentials().await))
                    .await;
                let mut ticks = interval(REFRESH);
                ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    ticks.tick().await;
                    let sandboxes = sandbox::list().await;
                    let projects = projects_of(sandboxes.as_deref().unwrap_or_default());
                    if sender.send(Message::Sandboxes(sandboxes)).await.is_err() {
                        return;
                    }
                    let read_dirs = dirs.clone();
                    let (result, git) = match tokio::task::spawn_blocking(move || {
                        (read_network(&read_dirs), read_git(&read_dirs, &projects))
                    })
                    .await
                    {
                        Ok(pair) => pair,
                        Err(error) => (
                            Err(anyhow::Error::from(error).context("reading network data")),
                            BTreeMap::new(),
                        ),
                    };
                    if sender.send(Message::Network(result)).await.is_err()
                        || sender.send(Message::Git(git)).await.is_err()
                    {
                        return;
                    }
                }
            })
        });
        Self {
            events: EventStream::new(),
            refresh,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(refresh) = &self.refresh {
            refresh.abort();
        }
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = execute!(stdout(), DisableBracketedPaste, Show);
    ratatui::restore();
}

fn enable_paste() -> Result<()> {
    execute!(stdout(), EnableBracketedPaste).context("enabling bracketed paste")
}

/// Ratatui's hook restores raw/alternate mode. Add cleanup for the mode we enable ourselves.
fn install_paste_cleanup() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableBracketedPaste, Show);
        previous(info);
    }));
}

/// Sandbox name and project of every sandbox msbx created for a folder.
fn projects_of(sandboxes: &[SandboxInfo]) -> Vec<(String, PathBuf)> {
    sandboxes
        .iter()
        .filter(|sandbox| sandbox.agent.is_some())
        .filter_map(|sandbox| Some((sandbox.name.clone(), sandbox.project.clone()?)))
        .collect()
}

/// Git files changed since each sandbox's last session started (RFC 0001). A project that
/// cannot be read shows one line saying so rather than nothing.
fn read_git(dirs: &Dirs, projects: &[(String, PathBuf)]) -> BTreeMap<String, Vec<String>> {
    projects
        .iter()
        .map(|(name, project)| {
            let lines = match guard::check(dirs, name, project) {
                Ok(changes) => changes.iter().map(ToString::to_string).collect(),
                Err(error) => vec![format!("check failed: {error:#}")],
            };
            (name.clone(), lines)
        })
        .collect()
}

/// The name `msbx agent` gives `agent`'s sandbox for `project`; empty for an unknown agent.
pub(crate) fn preview_name(agent: &str, project: &Path) -> String {
    agents::find(agent).map_or_else(String::new, |agent| sandbox::sandbox_name(agent, project))
}

/// Runs the TUI until the user quits. The terminal is restored on every return path; a panic
/// is covered by the hook `ratatui::try_init` installs.
pub(crate) async fn run(project: PathBuf, home: Option<PathBuf>, dirs: Dirs) -> Result<()> {
    let mut app = new_app(project, home);
    apply(&mut app, Message::Network(read_network(&dirs)));
    run_app(app, dirs, RunMode::Live).await
}

fn new_app(project: PathBuf, home: Option<PathBuf>) -> App {
    let agents = AgentId::value_variants()
        .iter()
        .map(|id| id.agent().id)
        .collect();
    let mut app = App::new(project, home, agents, preview_name);
    app.no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    app
}

#[cfg(debug_assertions)]
pub(crate) async fn run_fixture(path: &Path, dirs: Dirs) -> Result<()> {
    let fixture = crate::tui_fixture::load(path)?;
    let mut app = new_app(fixture.workspace.clone(), None);
    app.set_sandboxes(fixture.sandboxes);
    app.set_network(
        fixture.policy.as_ref().map(|policy| policy.preset),
        tui_data::aggregate_network(&fixture.network),
    );
    app.set_rules(
        fixture
            .policy
            .as_ref()
            .map_or_else(Vec::new, tui_data::rule_rows),
    );
    app.host_ports = fixture
        .policy
        .as_ref()
        .map(|policy| policy.host_ports.clone());
    app.credentials = fixture.credentials;
    app.git_changes = fixture.git_changes;
    let mode = RunMode::Fixture {
        workspace: fixture.workspace,
        logs: fixture.logs,
    };
    run_app(app, dirs, mode).await
}

async fn run_app(mut app: App, dirs: Dirs, mode: RunMode) -> Result<()> {
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init().context("setting up the terminal")?;
    install_paste_cleanup();
    enable_paste()?;
    event_loop(&mut terminal, &mut app, &dirs, &mode).await
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    dirs: &Dirs,
    mode: &RunMode,
) -> Result<()> {
    let (sender, mut receiver) = channel(32);
    let mut jobs = JoinSet::new();
    while let Some(foreground) =
        live(terminal, app, &sender, &mut receiver, dirs, mode, &mut jobs).await?
    {
        app.status_line = Some(suspend(terminal, foreground, dirs).await?);
    }
    Ok(())
}

/// Drives the TUI until the user quits (`None`) or picks something that needs the terminal.
async fn live(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    sender: &Sender<Message>,
    receiver: &mut Receiver<Message>,
    dirs: &Dirs,
    mode: &RunMode,
    jobs: &mut JoinSet<()>,
) -> Result<Option<Foreground>> {
    let mut session = Session::start(sender.clone(), dirs.clone(), mode);
    let mut frames = interval(FRAME);
    frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    loop {
        tokio::select! {
            _ = frames.tick() => if dirty {
                measure_scroll_limits(terminal, app)?;
                terminal.draw(|frame| ui::render(frame, app)).context("drawing the TUI")?;
                dirty = false;
            },
            Some(event) = session.events.next() => {
                dirty = true;
                let action = match event.context("reading terminal input")? {
                    Event::Key(key) => {
                        measure_scroll_limits(terminal, app)?;
                        app.update(key)
                    },
                    Event::Paste(text) => { app.update_paste(&text); None },
                    _ => None,
                };
                if let Some(action) = action {
                    match step(action, &app.sandboxes, mode) {
                        Ok(Step::Quit) => return Ok(None),
                        Ok(Step::Suspend(foreground)) => return Ok(Some(foreground)),
                        Ok(Step::Job(job)) => {
                            if let Some(progress) = progress(&job) {
                                app.status_line = Some(progress);
                            }
                            jobs.spawn(run_job(job, sender.clone(), dirs.clone(), mode.clone()));
                        }
                        Ok(Step::Policy(edit)) => edit_policy(app, dirs, edit).await,
                        Err(error) => app.status_line = Some(format!("{error:#}")),
                    }
                }
            },
            Some(message) = receiver.recv() => {
                apply(app, message);
                dirty = true;
            },
            Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                if let Err(error) = result {
                    app.status_line = Some(format!("dashboard job failed: {error}"));
                    dirty = true;
                }
            },
        }
    }
}

fn measure_scroll_limits(terminal: &DefaultTerminal, app: &mut App) -> Result<()> {
    let size = terminal.size().context("measuring the terminal")?;
    let limit = ui::detail_scroll_limit(app, size.width, size.height);
    app.set_detail_scroll_limit(limit);
    app.set_popup_scroll_limit(ui::popup_scroll_limit(app, size.width, size.height));
    Ok(())
}

fn step(action: Action, sandboxes: &[SandboxInfo], mode: &RunMode) -> Result<Step> {
    anyhow::ensure!(
        mode.permits(&action),
        "fixture: sandbox and policy actions are disabled"
    );
    Ok(match action {
        Action::Quit => Step::Quit,
        Action::Start { name } => Step::Job(Job::Start(name)),
        Action::Stop { name } => Step::Job(Job::Stop(name)),
        Action::Remove { name } => Step::Job(Job::Remove(name)),
        Action::LoadLogs { name } => Step::Job(Job::Logs(name)),
        Action::Shell { name } => {
            let project = project_of(sandboxes, &name);
            Step::Suspend(Foreground::Exec {
                name,
                command: SHELL,
                project,
            })
        }
        Action::Attach { name } => {
            let agent = agent_of(sandboxes, &name)?;
            let project = project_of(sandboxes, &name);
            Step::Suspend(Foreground::Attach {
                name,
                agent,
                project,
            })
        }
        Action::LoadFolders { path } => Step::Job(Job::Folders(path)),
        Action::Create {
            agent,
            project,
            name,
        } => Step::Job(Job::Create {
            agent: agents::find(&agent).with_context(|| format!("unknown agent {agent}"))?,
            project,
            name,
        }),
        Action::Allow { target, sandbox } => Step::Policy(Edit::Allow { target, sandbox }),
        Action::Deny { target, sandbox } => Step::Policy(Edit::Deny { target, sandbox }),
        Action::RemoveRule { target, sandbox } => Step::Policy(Edit::Remove { target, sandbox }),
        Action::SetPreset {
            preset,
            then_create,
        } => Step::Policy(Edit::Preset {
            preset,
            then_create,
        }),
        Action::SaveSettings { preset, host_ports } => {
            Step::Policy(Edit::Settings { preset, host_ports })
        }
    })
}

/// Applies a policy edit at once (a tiny file write), asks the proxy to reload it, and reports
/// it on the status line. The Network tab's preset updates right away.
async fn edit_policy(app: &mut App, dirs: &Dirs, edit: Edit) {
    let then_create = matches!(
        edit,
        Edit::Preset {
            then_create: true,
            ..
        }
    );
    let preset = match &edit {
        Edit::Preset { preset, .. } | Edit::Settings { preset, .. } => Some(*preset),
        Edit::Allow { .. } | Edit::Deny { .. } | Edit::Remove { .. } => None,
    };
    match apply_edit(dirs, edit) {
        Ok(status) => {
            app.status_line = Some(match proxy::reload(dirs).await {
                Ok(()) => status,
                Err(error) => format!("{status}; {error:#}"),
            });
            if let Some(preset) = preset {
                app.policy = Some(preset);
            }
            apply(app, Message::Network(read_network(dirs)));
            if then_create {
                app.open_create();
            }
        }
        Err(error) => app.status_line = Some(format!("{error:#}")),
    }
}

fn apply_edit(dirs: &Dirs, edit: Edit) -> Result<String> {
    let path = dirs.policy_file();
    let (mut policy, status) = match edit {
        Edit::Preset {
            preset,
            then_create: _,
        } => {
            let mut policy = policy::load(&path)?.unwrap_or_else(|| Policy::new(preset));
            policy.preset = preset;
            (policy, format!("network preset: {preset}"))
        }
        Edit::Settings { preset, host_ports } => {
            // Like the first preset, saving settings creates the policy file if needed.
            let mut policy = policy::load(&path)?.unwrap_or_else(|| Policy::new(preset));
            policy.preset = preset;
            policy.host_ports = policy::normalize_host_ports(host_ports)?;
            let status = format!(
                "settings saved: preset {preset}; host ports {} (applies to new sandboxes)",
                policy::port_list(&policy.host_ports)
            );
            (policy, status)
        }
        Edit::Allow { target, sandbox } => {
            let parsed = Target::parse(&target)?;
            let mut policy = policy::load(&path)?.context("network policy is not initialised")?;
            policy.allow(&[parsed], sandbox.as_deref());
            (policy, edit_status("allowed", &target, sandbox.as_deref()))
        }
        Edit::Deny { target, sandbox } => {
            let parsed = Target::parse(&target)?;
            let mut policy = policy::load(&path)?.context("network policy is not initialised")?;
            policy.deny(&[parsed], sandbox.as_deref());
            (policy, edit_status("denied", &target, sandbox.as_deref()))
        }
        Edit::Remove { target, sandbox } => {
            let parsed = Target::parse(&target)?;
            let mut policy = policy::load(&path)?.context("network policy is not initialised")?;
            policy.remove(&parsed, sandbox.as_deref())?;
            (policy, edit_status("removed", &target, sandbox.as_deref()))
        }
    };
    policy
        .sandboxes
        .retain(|_, rules| !rules.allow.is_empty() || !rules.deny.is_empty());
    policy::save(&path, &policy)?;
    Ok(status)
}

fn edit_status(verb: &str, target: &str, sandbox: Option<&str>) -> String {
    match sandbox {
        Some(name) => format!("{verb} {target} for {name}"),
        None => format!("{verb} {target} everywhere"),
    }
}

/// The agent recorded in sandbox `name`'s `msbx.agent` label.
fn agent_of(sandboxes: &[SandboxInfo], name: &str) -> Result<&'static Agent> {
    let label = sandboxes
        .iter()
        .find(|sandbox| sandbox.name == name)
        .and_then(|sandbox| sandbox.agent.as_deref())
        .with_context(|| format!("sandbox {name} has no msbx agent"))?;
    agents::find(label).with_context(|| format!("sandbox {name} has unknown agent {label}"))
}

/// The project recorded in sandbox `name`'s `msbx.project` label.
fn project_of(sandboxes: &[SandboxInfo], name: &str) -> Option<PathBuf> {
    sandboxes
        .iter()
        .find(|sandbox| sandbox.name == name)
        .and_then(|sandbox| sandbox.project.clone())
}

/// Stopping can take seconds, so say that something is happening.
fn progress(job: &Job) -> Option<String> {
    match job {
        Job::Create { agent, name, .. } => Some(format!(
            "creating {name}… (the first run installs {})",
            agent.id
        )),
        Job::Start(name) => Some(format!("starting {name}…")),
        Job::Stop(name) => Some(format!("stopping {name}…")),
        Job::Remove(name) => Some(format!("removing {name}…")),
        Job::Logs(_) | Job::Folders(_) => None,
    }
}

// Send errors only mean the TUI has quit, leaving nobody to tell.
async fn run_job(job: Job, sender: Sender<Message>, dirs: Dirs, mode: RunMode) {
    let (result, done, name) = match job {
        Job::Folders(path) => {
            let read_path = path.clone();
            let root = mode.workspace().map(Path::to_path_buf);
            let directories = tokio::task::spawn_blocking(move || {
                tui_data::list_folders(&read_path, root.as_deref())
            })
            .await
            .context("listing workspace folders")
            .and_then(std::convert::identity);
            let _ = sender.send(Message::Folders { path, directories }).await;
            return;
        }
        Job::Logs(name) => {
            let logs = match mode.fixture_logs(&name) {
                Some(logs) => Ok(logs),
                None => sandbox::logs(&name, LOG_LINES).await,
            };
            let _ = sender.send(Message::Logs { name, logs }).await;
            return;
        }
        Job::Create {
            agent,
            project,
            name,
        } => {
            let notes = sender.clone();
            let notify = move |note: String| {
                let _ = notes.try_send(Message::Status(note));
            };
            let created = create(agent, &project, &name, &dirs, &notify).await;
            let _ = sender.send(Message::Sandboxes(sandbox::list().await)).await;
            let _ = match created {
                Ok(name) => {
                    let _ = sender
                        .send(Message::Status(format!(
                            "created {name}; press ↵ to attach"
                        )))
                        .await;
                    sender.send(Message::Created(name)).await
                }
                Err(error) => sender.send(Message::Status(format!("{error:#}"))).await,
            };
            return;
        }
        Job::Start(name) => (start(&name, &dirs).await, "started", name),
        Job::Stop(name) => (sandbox::stop(&name).await, "stopped", name),
        Job::Remove(name) => (sandbox::remove(&name).await, "removed", name),
    };
    let status = match result {
        Ok(()) => format!("{done} {name}"),
        Err(error) => format!("{error:#}"),
    };
    let _ = sender.send(Message::Status(status)).await;
    let _ = sender.send(Message::Sandboxes(sandbox::list().await)).await;
}

/// Creates a new sandbox without attaching, with the host ports `policy.toml` has now. Like
/// `start`, it needs the proxy first.
async fn create(
    agent: &'static Agent,
    project: &Path,
    name: &str,
    dirs: &Dirs,
    notify: sandbox::Notify<'_>,
) -> Result<String> {
    let policy = policy::load(&dirs.policy_file())?.context("network policy is not initialised")?;
    proxy::ensure_running(dirs).await?;
    sandbox::ensure_named(agent, project, name, &policy.host_ports, notify).await
}

/// A started sandbox dials the proxy at once, so make sure one is there first.
async fn start(name: &str, dirs: &Dirs) -> Result<()> {
    proxy::ensure_running(dirs).await?;
    sandbox::start(name).await
}

fn apply(app: &mut App, message: Message) {
    match message {
        Message::Sandboxes(Ok(sandboxes)) => app.set_sandboxes(sandboxes),
        Message::Logs {
            name,
            logs: Ok(logs),
        } => app.set_logs(&name, logs),
        Message::Network(Ok(data)) => {
            app.host_ports = data.host_ports;
            app.set_network(data.preset, data.entries);
            app.set_rules(data.rules);
        }
        Message::Credentials(credentials) => app.credentials = credentials,
        Message::Folders { path, directories } => {
            app.set_folders(&path, directories.map_err(|error| format!("{error:#}")));
        }
        Message::Sandboxes(Err(error))
        | Message::Network(Err(error))
        | Message::Logs {
            logs: Err(error), ..
        } => {
            app.status_line = Some(format!("{error:#}"));
        }
        Message::Status(status) => app.status_line = Some(status),
        Message::Created(name) => app.select_named(&name),
        Message::Git(changes) => app.git_changes = changes,
    }
}

/// Hands the real terminal to `foreground`, then brings the TUI back. Returns the status line
/// to show; a failure of `foreground` itself is reported there rather than ending the TUI.
async fn suspend(
    terminal: &mut DefaultTerminal,
    foreground: Foreground,
    dirs: &Dirs,
) -> Result<String> {
    restore_terminal();
    let outcome = run_foreground(foreground, dirs).await;
    enable_raw_mode().context("resuming terminal input")?;
    execute!(stdout(), EnterAlternateScreen).context("resuming the TUI")?;
    *terminal =
        ratatui::Terminal::new(CrosstermBackend::new(stdout())).context("resuming the terminal")?;
    enable_paste()?;
    // The new terminal's buffers are blank, so wiping the screen is all a clear needs.
    // `Terminal::clear` would also query the cursor position, a round trip some PTYs never answer.
    terminal
        .backend_mut()
        .clear()
        .context("clearing the terminal")?;
    Ok(outcome.unwrap_or_else(|error| format!("{error:#}")))
}

async fn run_foreground(foreground: Foreground, dirs: &Dirs) -> Result<String> {
    let (name, command, project) = match foreground {
        Foreground::Attach {
            name,
            agent,
            project,
        } => {
            proxy::ensure_running(dirs).await?;
            sandbox::prepare_existing(&name, agent, &|note| eprintln!("{note}")).await?;
            (name, agent.command, project)
        }
        Foreground::Exec {
            name,
            command,
            project,
        } => (name, command, project),
    };
    // The terminal is the agent's now, so warnings go to stderr like `msbx agent`'s; the status
    // line and the card's badge repeat them once the dashboard is back (RFC 0001).
    let session = match &project {
        Some(project) => {
            let (session, warning) = guard::Session::start(dirs, &name, project)?;
            if let Some(warning) = warning {
                eprintln!("{warning}");
            }
            Some(session)
        }
        None => None,
    };
    let code = attach::exec(&name, command, &[]).await?;
    let mut status = format!("exited {name} (code {code})");
    if let Some(warning) = session
        .map(|session| session.finish())
        .transpose()?
        .flatten()
    {
        eprintln!("{warning}");
        status.push_str("; git files changed, see Overview");
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;
    use crate::netlog::{self, Log};
    use crate::tui_data::{aggregate_network, list_folders, rule_rows};

    #[cfg(debug_assertions)]
    #[cfg(debug_assertions)]
    #[test]
    fn fixture_dispatch_rejects_every_lifecycle_and_policy_write() -> Result<()> {
        let mode = RunMode::Fixture {
            workspace: PathBuf::from("/tmp/fixture"),
            logs: std::collections::BTreeMap::new(),
        };
        let writes = [
            Action::Create {
                agent: name("codex"),
                project: PathBuf::from("/work"),
                name: name("chosen"),
            },
            Action::Start {
                name: name("codex-api"),
            },
            Action::Stop {
                name: name("codex-api"),
            },
            Action::Remove {
                name: name("codex-api"),
            },
            Action::Attach {
                name: name("codex-api"),
            },
            Action::Shell {
                name: name("codex-api"),
            },
            Action::Allow {
                target: name("api.test"),
                sandbox: None,
            },
            Action::Deny {
                target: name("api.test"),
                sandbox: None,
            },
            Action::RemoveRule {
                target: name("api.test"),
                sandbox: None,
            },
            Action::SetPreset {
                preset: Preset::Open,
                then_create: false,
            },
            Action::SaveSettings {
                preset: Preset::Open,
                host_ports: vec![8080],
            },
        ];
        for action in writes {
            assert!(step(action, &sandboxes(), &mode).is_err());
        }
        assert_eq!(step(Action::Quit, &[], &mode)?, Step::Quit);
        assert_eq!(
            step(
                Action::LoadFolders {
                    path: PathBuf::from("/tmp/fixture")
                },
                &[],
                &mode
            )?,
            Step::Job(Job::Folders(PathBuf::from("/tmp/fixture")))
        );
        assert_eq!(
            step(
                Action::LoadLogs {
                    name: name("codex-api")
                },
                &[],
                &mode
            )?,
            Step::Job(Job::Logs(name("codex-api")))
        );
        Ok(())
    }

    #[test]
    fn rule_removal_changes_only_the_captured_scope() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("msbx-tui-remove-rule-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let outcome = (|| -> Result<()> {
            let target = Target::parse("same.test")?;
            let mut policy = Policy::new(Preset::Balanced);
            policy.allow(std::slice::from_ref(&target), None);
            policy.deny(std::slice::from_ref(&target), Some("box"));
            policy::save(&dirs.policy_file(), &policy)?;
            apply_edit(
                &dirs,
                Edit::Remove {
                    target: name("same.test"),
                    sandbox: Some(name("box")),
                },
            )?;
            let saved = policy::load(&dirs.policy_file())?.context("saved policy")?;
            assert_eq!(saved.allow, [target]);
            assert!(!saved.sandboxes.contains_key("box"));
            assert!(
                apply_edit(
                    &dirs,
                    Edit::Remove {
                        target: name("same.test"),
                        sandbox: Some(name("box"))
                    }
                )
                .is_err()
            );
            assert_eq!(policy::load(&dirs.policy_file())?, Some(saved));
            Ok(())
        })();
        std::fs::remove_dir_all(root)?;
        outcome
    }

    #[test]
    fn endpoint_counts_saturate_instead_of_overflowing() -> Result<()> {
        let entries = vec![
            netlog::Entry {
                sandbox: name("box"),
                host: name("api.test"),
                port: 443,
                allowed: true,
                rule: name("one"),
                count: u64::MAX,
                first_seen: 1,
                last_seen: 2,
            },
            netlog::Entry {
                sandbox: name("box"),
                host: name("api.test"),
                port: 443,
                allowed: false,
                rule: name("two"),
                count: 2,
                first_seen: 2,
                last_seen: 3,
            },
        ];
        let rows = aggregate_network(&entries);
        let row = rows.first().context("aggregated row")?;
        assert_eq!(row.count, u64::MAX);
        assert_eq!(row.allowed_count, u64::MAX);
        assert_eq!(row.blocked_count, 2);
        Ok(())
    }

    #[test]
    fn endpoint_aggregation_keeps_history_counts_and_latest_outcome() -> Result<()> {
        let mut log = Log::default();
        log.record("box", "api.test", 443, false, "default", 100);
        log.record("box", "api.test", 443, false, "default", 110);
        log.record("box", "api.test", 443, true, "allow api.test", 120);
        log.record("box", "api.test", 80, true, "allow api.test", 120);
        log.record("other", "api.test", 443, false, "default", 120);
        let rows = aggregate_network(log.entries());
        assert_eq!(rows.len(), 3);
        let row = rows
            .iter()
            .find(|row| row.sandbox == "box" && row.port == 443)
            .context("aggregated box endpoint")?;
        assert_eq!((row.count, row.allowed_count, row.blocked_count), (3, 1, 2));
        assert!(row.allowed);
        assert_eq!(row.first_seen, netlog::local_time(100, "%Y-%m-%d %H:%M:%S"));
        assert_eq!(row.last_seen, netlog::local_time(120, "%Y-%m-%d %H:%M:%S"));
        log.record("box", "api.test", 443, false, "new deny", 120);
        let rows = aggregate_network(log.entries());
        let row = rows
            .iter()
            .find(|row| row.sandbox == "box" && row.port == 443)
            .context("aggregated endpoint after equal-time deny")?;
        assert!(!row.allowed, "blocked wins the timestamp tie");
        Ok(())
    }

    #[test]
    fn effective_rule_rows_preserve_scope_without_shared_duplicates() -> Result<()> {
        let mut policy = Policy::new(Preset::Balanced);
        policy.allow(&[Target::parse("global.test")?], None);
        policy.deny(&[Target::parse("box.test")?], Some("box"));
        policy.allow(&[Target::parse("other.test")?], Some("other"));
        let rows = rule_rows(&policy);
        assert_eq!(
            rows.iter()
                .filter(|row| row.target == "global.test")
                .count(),
            1
        );
        let boxed = rows
            .iter()
            .find(|row| row.target == "box.test")
            .context("box rule")?;
        assert_eq!(boxed.sandbox.as_deref(), Some("box"));
        assert_eq!(boxed.source, policy::Source::Sandbox);
        assert!(!boxed.allowed);
        assert!(rows.iter().any(|row| row.source == policy::Source::Preset));
        Ok(())
    }

    #[test]
    fn folders_list_directories_and_confine_fixture_navigation() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-folders-{}", std::process::id()));
        std::fs::create_dir_all(root.join("workspace/Zulu"))?;
        std::fs::create_dir_all(root.join("workspace/alpha"))?;
        std::fs::write(root.join("workspace/file.txt"), "file")?;
        let workspace = root.join("workspace").canonicalize()?;
        let outcome = (|| -> Result<()> {
            let folders = list_folders(&workspace, Some(&workspace))?;
            assert_eq!(folders, [workspace.join("alpha"), workspace.join("Zulu")]);
            assert!(list_folders(&root, Some(&workspace)).is_err());
            assert!(list_folders(&workspace.join("file.txt"), None).is_err());
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(
                    root.parent().context("temp parent")?,
                    workspace.join("escape"),
                )?;
                assert!(list_folders(&workspace.join("escape"), Some(&workspace)).is_err());
            }
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }

    fn sandbox(name: &str, agent: Option<&str>) -> SandboxInfo {
        SandboxInfo {
            name: name.to_owned(),
            status: Status::Running,
            agent: agent.map(str::to_owned),
            project: None,
            image: None,
            created: None,
            vcpus: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
            disk_used_bytes: None,
            disk_free_bytes: None,
            uptime: None,
            network: None,
            proxied: true,
            host_ports: None,
            user: None,
            restricted: None,
            git_guarded: true,
            rx_bytes: None,
            tx_bytes: None,
            secrets: Vec::new(),
            mounts: Vec::new(),
        }
    }

    fn sandboxes() -> Vec<SandboxInfo> {
        vec![
            SandboxInfo {
                project: Some(PathBuf::from("/work/api")),
                ..sandbox("codex-api", Some("codex"))
            },
            sandbox("pi-notes", Some("pi")),
            sandbox("foreign", None),
            sandbox("future", Some("claude")),
        ]
    }

    fn name(value: &str) -> String {
        value.to_owned()
    }

    fn step_for(action: Action) -> Result<Step> {
        step(action, &sandboxes(), &RunMode::Live)
    }

    #[test]
    fn lifecycle_and_logs_run_in_the_background() -> Result<()> {
        assert_eq!(step_for(Action::Quit)?, Step::Quit);
        let cases = [
            (Action::Start { name: name("a") }, Job::Start(name("a"))),
            (Action::Stop { name: name("a") }, Job::Stop(name("a"))),
            (Action::Remove { name: name("a") }, Job::Remove(name("a"))),
            (Action::LoadLogs { name: name("a") }, Job::Logs(name("a"))),
        ];
        for (action, job) in cases {
            assert_eq!(step_for(action)?, Step::Job(job));
        }
        Ok(())
    }

    #[test]
    fn policy_actions_become_edits() -> Result<()> {
        assert_eq!(
            step_for(Action::Allow {
                target: name("pypi.org"),
                sandbox: Some(name("codex-api"))
            })?,
            Step::Policy(Edit::Allow {
                target: name("pypi.org"),
                sandbox: Some(name("codex-api"))
            })
        );
        assert_eq!(
            step_for(Action::Deny {
                target: name("pypi.org"),
                sandbox: None
            })?,
            Step::Policy(Edit::Deny {
                target: name("pypi.org"),
                sandbox: None
            })
        );
        assert_eq!(
            step_for(Action::SetPreset {
                preset: Preset::Open,
                then_create: true
            })?,
            Step::Policy(Edit::Preset {
                preset: Preset::Open,
                then_create: true
            })
        );
        Ok(())
    }

    #[test]
    fn edits_initialise_and_change_the_policy_file() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-edit-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let outcome = (|| -> Result<()> {
            let missing = apply_edit(
                &dirs,
                Edit::Allow {
                    target: name("pypi.org"),
                    sandbox: None,
                },
            );
            assert!(missing.is_err(), "allow before init");
            assert_eq!(
                apply_edit(
                    &dirs,
                    Edit::Preset {
                        preset: Preset::LockedDown,
                        then_create: true
                    }
                )?,
                "network preset: locked-down"
            );
            assert_eq!(
                apply_edit(
                    &dirs,
                    Edit::Allow {
                        target: name("pypi.org"),
                        sandbox: Some(name("codex-api"))
                    }
                )?,
                "allowed pypi.org for codex-api"
            );
            assert_eq!(
                apply_edit(
                    &dirs,
                    Edit::Deny {
                        target: name("evil.test"),
                        sandbox: None
                    }
                )?,
                "denied evil.test everywhere"
            );
            assert_eq!(
                apply_edit(
                    &dirs,
                    Edit::Preset {
                        preset: Preset::Open,
                        then_create: false
                    }
                )?,
                "network preset: open"
            );
            let policy = policy::load(&dirs.policy_file())?.context("policy file")?;
            assert_eq!(policy.preset, Preset::Open);
            assert_eq!(policy.deny, [Target::parse("evil.test")?]);
            assert_eq!(
                policy.sandboxes["codex-api"].allow,
                [Target::parse("pypi.org")?]
            );
            let data = read_network(&dirs)?;
            assert_eq!(data.preset, Some(Preset::Open));
            assert_eq!(data.entries, [] as [crate::model::NetEntry; 0]);
            Ok(())
        })();
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        outcome
    }

    #[test]
    fn settings_create_or_update_the_policy_and_keep_its_rules() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-settings-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let outcome = (|| -> Result<()> {
            assert_eq!(
                step_for(Action::SaveSettings {
                    preset: Preset::Open,
                    host_ports: vec![8080],
                })?,
                Step::Policy(Edit::Settings {
                    preset: Preset::Open,
                    host_ports: vec![8080],
                })
            );
            let status = apply_edit(
                &dirs,
                Edit::Settings {
                    preset: Preset::LockedDown,
                    host_ports: vec![8080, 1234],
                },
            )?;
            assert_eq!(
                status,
                "settings saved: preset locked-down; host ports 1234, 8080 (applies to new \
                 sandboxes)"
            );
            let created = policy::load(&dirs.policy_file())?.context("saving creates the file")?;
            assert_eq!(created.preset, Preset::LockedDown);
            assert_eq!(created.host_ports, [1234, 8080]);

            apply_edit(
                &dirs,
                Edit::Allow {
                    target: name("pypi.org"),
                    sandbox: None,
                },
            )?;
            apply_edit(
                &dirs,
                Edit::Settings {
                    preset: Preset::Balanced,
                    host_ports: Vec::new(),
                },
            )?;
            let saved = policy::load(&dirs.policy_file())?.context("saved policy")?;
            assert_eq!(saved.preset, Preset::Balanced);
            assert_eq!(saved.host_ports, [] as [u16; 0]);
            assert_eq!(saved.allow, [Target::parse("pypi.org")?], "rules survive");
            assert_eq!(read_network(&dirs)?.host_ports, Some(vec![]));
            assert!(
                apply_edit(
                    &dirs,
                    Edit::Settings {
                        preset: Preset::Open,
                        host_ports: vec![0],
                    }
                )
                .is_err()
            );
            assert_eq!(policy::load(&dirs.policy_file())?, Some(saved));
            Ok(())
        })();
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        outcome
    }

    #[tokio::test]
    async fn edit_policy_opens_create_after_the_first_preset() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-first-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let mut app = App::new(PathBuf::from("/work"), None, vec!["codex"], preview_name);
        edit_policy(
            &mut app,
            &dirs,
            Edit::Preset {
                preset: Preset::Balanced,
                then_create: true,
            },
        )
        .await;
        let first = (app.policy, app.popup.clone(), app.status_line.clone());
        app.popup = None;
        edit_policy(
            &mut app,
            &dirs,
            Edit::Allow {
                target: name("*.com"),
                sandbox: None,
            },
        )
        .await;
        let second = (app.popup.clone(), app.status_line.clone());
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        assert_eq!(first.0, Some(Preset::Balanced));
        assert!(matches!(first.1, Some(crate::app::Popup::Create(wizard))
            if wizard.stage == crate::app::CreateStage::Agent && wizard.agent_index == 0));
        assert_eq!(first.2.as_deref(), Some("network preset: balanced"));
        assert_eq!(second.0, None, "a failed edit opens nothing");
        assert!(
            second
                .1
                .as_deref()
                .is_some_and(|line| line.contains("*.com")),
            "{:?}",
            second.1
        );
        Ok(())
    }

    #[test]
    fn attach_prepares_the_labelled_agent_before_running_it() -> Result<()> {
        for (sandbox, agent, project) in [
            (
                "codex-api",
                AgentId::Codex,
                Some(PathBuf::from("/work/api")),
            ),
            ("pi-notes", AgentId::Pi, None),
        ] {
            assert_eq!(
                step_for(Action::Attach {
                    name: name(sandbox)
                })?,
                Step::Suspend(Foreground::Attach {
                    name: name(sandbox),
                    agent: agent.agent(),
                    project,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn projects_are_read_only_for_sandboxes_msbx_created() {
        let listed = projects_of(&sandboxes());
        assert_eq!(listed, [(name("codex-api"), PathBuf::from("/work/api"))]);
    }

    #[test]
    fn read_git_reports_each_project_and_check_failures() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-tui-git-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let project = root.join("proj");
        std::fs::create_dir_all(&project)?;
        let outcome = (|| -> Result<()> {
            let no_baseline = read_git(&dirs, &[(name("box"), project.clone())]);
            assert_eq!(no_baseline["box"], [] as [String; 0]);
            std::fs::create_dir_all(dirs.git_guard_dir())?;
            std::fs::write(dirs.git_guard_dir().join("box.json"), "not json")?;
            let broken = read_git(&dirs, &[(name("box"), project.clone())]);
            assert!(broken["box"][0].starts_with("check failed: "), "{broken:?}");
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }

    #[test]
    fn attach_fails_without_a_known_agent() {
        for (sandbox, message) in [
            ("foreign", "sandbox foreign has no msbx agent"),
            ("gone", "sandbox gone has no msbx agent"),
            ("future", "sandbox future has unknown agent claude"),
        ] {
            let error = step_for(Action::Attach {
                name: name(sandbox),
            })
            .err()
            .map(|error| error.to_string());
            assert_eq!(error.as_deref(), Some(message));
        }
    }

    #[test]
    fn shell_runs_bash_whatever_the_agent() -> Result<()> {
        assert_eq!(
            step_for(Action::Shell {
                name: name("codex-api")
            })?,
            Step::Suspend(Foreground::Exec {
                name: name("codex-api"),
                command: "bash",
                project: Some(PathBuf::from("/work/api")),
            })
        );
        Ok(())
    }

    #[test]
    fn create_resolves_the_agent() -> Result<()> {
        let project = PathBuf::from("/work/api");
        assert_eq!(
            step_for(Action::Create {
                agent: name("pi"),
                project: project.clone(),
                name: name("pi-custom"),
            })?,
            Step::Job(Job::Create {
                agent: AgentId::Pi.agent(),
                project: project.clone(),
                name: name("pi-custom"),
            }),
            "creating stays on the dashboard; ↵ attaches later"
        );
        let unknown = step_for(Action::Create {
            agent: name("claude"),
            project,
            name: name("claude-custom"),
        });
        assert_eq!(
            unknown.err().map(|error| error.to_string()).as_deref(),
            Some("unknown agent claude")
        );
        Ok(())
    }

    #[test]
    fn preview_name_matches_msbx_run_naming() {
        for folder in [
            "/work/My App",
            "/",
            "/work/a  .  b",
            &format!("/{}", "x".repeat(300)),
        ] {
            let project = Path::new(folder);
            for id in AgentId::value_variants() {
                assert_eq!(
                    preview_name(id.agent().id, project),
                    sandbox::sandbox_name(id.agent(), project)
                );
            }
        }
        assert_eq!(preview_name("claude", Path::new("/work/api")), "");
    }

    #[test]
    fn only_lifecycle_jobs_report_progress() {
        assert_eq!(
            progress(&Job::Stop(name("a"))).as_deref(),
            Some("stopping a…")
        );
        assert_eq!(progress(&Job::Logs(name("a"))), None);
    }
}
