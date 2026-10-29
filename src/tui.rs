//! Terminal setup, the event loop, and suspending the TUI to hand the terminal to `msb exec`.
//! All of the TUI's IO lives here so `app.rs` and `ui.rs` stay pure.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::ValueEnum;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};

use crate::agents::{self, Agent, AgentId};
use crate::app::{Action, App, Popup};
use crate::model::{NetEntry, SandboxInfo};
use crate::netlog::{self, Log};
use crate::paths::Dirs;
use crate::policy::{self, Policy, Preset, Target};
use crate::{attach, proxy, sandbox, ui};

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
    },
    Start(String),
    Stop(String),
    Remove(String),
    Logs(String),
}

#[derive(Debug, PartialEq)]
enum Foreground {
    Attach { name: String, agent: &'static Agent },
    Exec { name: String, command: &'static str },
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
    Preset {
        preset: Preset,
        then_create: bool,
    },
}

/// Results background tasks send back to the event loop, the single owner of [`App`].
enum Message {
    Sandboxes(Result<Vec<SandboxInfo>>),
    Logs {
        name: String,
        logs: Result<Vec<String>>,
    },
    Network {
        policy: Option<Preset>,
        entries: Vec<NetEntry>,
    },
    Status(String),
    /// A sandbox the user just created; select its card once it is listed.
    Created(String),
}

/// Input plus periodic refresh. Dropping it stops both, so while the TUI is suspended nothing
/// else reads stdin or repaints.
struct Session {
    events: EventStream,
    refresh: JoinHandle<()>,
}

impl Session {
    fn start(sender: UnboundedSender<Message>, dirs: Dirs) -> Self {
        let refresh = tokio::spawn(async move {
            let mut ticks = interval(REFRESH);
            loop {
                ticks.tick().await;
                if sender
                    .send(Message::Sandboxes(sandbox::list().await))
                    .is_err()
                {
                    return;
                }
                let read_dirs = dirs.clone();
                let (policy, entries) =
                    tokio::task::spawn_blocking(move || read_network(&read_dirs))
                        .await
                        .unwrap_or_default();
                if sender.send(Message::Network { policy, entries }).is_err() {
                    return;
                }
            }
        });
        Self {
            events: EventStream::new(),
            refresh,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.refresh.abort();
    }
}

/// The preset and the proxy's connection log, as the Network tab shows them. Unreadable files
/// show as "not initialised" / no rows rather than stopping the TUI.
fn read_network(dirs: &Dirs) -> (Option<Preset>, Vec<NetEntry>) {
    let policy = policy::load(&dirs.policy_file())
        .ok()
        .flatten()
        .map(|policy| policy.preset);
    let log = Log::load(&dirs.network_log()).unwrap_or_default();
    let entries = netlog::sorted(log.entries(), None)
        .into_iter()
        .map(|entry| NetEntry {
            sandbox: entry.sandbox.clone(),
            host: entry.host.clone(),
            port: entry.port,
            allowed: entry.allowed,
            count: entry.count,
            last_seen: netlog::local_time(entry.last_seen, "%H:%M:%S"),
        })
        .collect();
    (policy, entries)
}

/// The name `msbx agent` gives `agent`'s sandbox for `project`; empty for an unknown agent.
pub(crate) fn preview_name(agent: &str, project: &Path) -> String {
    agents::find(agent).map_or_else(String::new, |agent| sandbox::sandbox_name(agent, project))
}

/// Runs the TUI until the user quits. The terminal is restored on every return path; a panic
/// is covered by the hook `ratatui::try_init` installs.
pub(crate) async fn run(project: PathBuf, home: Option<PathBuf>, dirs: Dirs) -> Result<()> {
    let agents = AgentId::value_variants()
        .iter()
        .map(|id| id.agent().id)
        .collect();
    let mut app = App::new(project, home, agents, preview_name);
    let (policy, entries) = read_network(&dirs);
    app.set_network(policy, entries);
    let mut terminal = ratatui::try_init().context("setting up the terminal")?;
    let result = event_loop(&mut terminal, &mut app, &dirs).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, dirs: &Dirs) -> Result<()> {
    let (sender, mut receiver) = unbounded_channel();
    while let Some(foreground) = live(terminal, app, &sender, &mut receiver, dirs).await? {
        app.status_line = Some(suspend(terminal, foreground, dirs).await?);
    }
    Ok(())
}

/// Drives the TUI until the user quits (`None`) or picks something that needs the terminal.
async fn live(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    sender: &UnboundedSender<Message>,
    receiver: &mut UnboundedReceiver<Message>,
    dirs: &Dirs,
) -> Result<Option<Foreground>> {
    let mut session = Session::start(sender.clone(), dirs.clone());
    let mut frames = interval(FRAME);
    frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    loop {
        tokio::select! {
            _ = frames.tick() => if dirty {
                terminal.draw(|frame| ui::render(frame, app)).context("drawing the TUI")?;
                dirty = false;
            },
            Some(event) = session.events.next() => {
                dirty = true;
                if let Event::Key(key) = event.context("reading terminal input")?
                    && let Some(action) = app.update(key)
                {
                    match step(action, &app.sandboxes) {
                        Ok(Step::Quit) => return Ok(None),
                        Ok(Step::Suspend(foreground)) => return Ok(Some(foreground)),
                        Ok(Step::Job(job)) => {
                            if let Some(progress) = progress(&job) {
                                app.status_line = Some(progress);
                            }
                            tokio::spawn(run_job(job, sender.clone(), dirs.clone()));
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
        }
    }
}

fn step(action: Action, sandboxes: &[SandboxInfo]) -> Result<Step> {
    Ok(match action {
        Action::Quit => Step::Quit,
        Action::Start { name } => Step::Job(Job::Start(name)),
        Action::Stop { name } => Step::Job(Job::Stop(name)),
        Action::Remove { name } => Step::Job(Job::Remove(name)),
        Action::LoadLogs { name } => Step::Job(Job::Logs(name)),
        Action::Shell { name } => Step::Suspend(Foreground::Exec {
            name,
            command: SHELL,
        }),
        Action::Attach { name } => {
            let agent = agent_of(sandboxes, &name)?;
            Step::Suspend(Foreground::Attach { name, agent })
        }
        Action::Create { agent, project } => Step::Job(Job::Create {
            agent: agents::find(&agent).with_context(|| format!("unknown agent {agent}"))?,
            project,
        }),
        Action::Allow { target, sandbox } => Step::Policy(Edit::Allow { target, sandbox }),
        Action::Deny { target, sandbox } => Step::Policy(Edit::Deny { target, sandbox }),
        Action::SetPreset {
            preset,
            then_create,
        } => Step::Policy(Edit::Preset {
            preset,
            then_create,
        }),
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
        Edit::Preset { preset, .. } => Some(*preset),
        Edit::Allow { .. } | Edit::Deny { .. } => None,
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
            if then_create {
                app.popup = Some(Popup::Create { agent_index: 0 });
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

/// Stopping can take seconds, so say that something is happening.
fn progress(job: &Job) -> Option<String> {
    match job {
        Job::Create { agent, project } => Some(format!(
            "creating {}… (the first run installs {})",
            sandbox::sandbox_name(agent, project),
            agent.id
        )),
        Job::Start(name) => Some(format!("starting {name}…")),
        Job::Stop(name) => Some(format!("stopping {name}…")),
        Job::Remove(name) => Some(format!("removing {name}…")),
        Job::Logs(_) => None,
    }
}

// Send errors only mean the TUI has quit, leaving nobody to tell.
async fn run_job(job: Job, sender: UnboundedSender<Message>, dirs: Dirs) {
    let (result, done, name) = match job {
        Job::Logs(name) => {
            let logs = sandbox::logs(&name, LOG_LINES).await;
            let _ = sender.send(Message::Logs { name, logs });
            return;
        }
        Job::Create { agent, project } => {
            let notes = sender.clone();
            let notify = move |note: String| {
                let _ = notes.send(Message::Status(note));
            };
            let created = create(agent, &project, &dirs, &notify).await;
            let _ = sender.send(Message::Sandboxes(sandbox::list().await));
            let _ = match created {
                Ok(name) => {
                    let _ = sender.send(Message::Status(format!(
                        "created {name}; press ↵ to attach"
                    )));
                    sender.send(Message::Created(name))
                }
                Err(error) => sender.send(Message::Status(format!("{error:#}"))),
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
    let _ = sender.send(Message::Status(status));
    let _ = sender.send(Message::Sandboxes(sandbox::list().await));
}

/// Creates (or starts) the sandbox without attaching. Like `start`, it needs the proxy first.
async fn create(
    agent: &'static Agent,
    project: &Path,
    dirs: &Dirs,
    notify: sandbox::Notify<'_>,
) -> Result<String> {
    proxy::ensure_running(dirs).await?;
    sandbox::ensure(agent, project, notify).await
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
        Message::Network { policy, entries } => app.set_network(policy, entries),
        Message::Sandboxes(Err(error))
        | Message::Logs {
            logs: Err(error), ..
        } => {
            app.status_line = Some(format!("{error:#}"));
        }
        Message::Status(status) => app.status_line = Some(status),
        Message::Created(name) => app.select_named(&name),
    }
}

/// Hands the real terminal to `foreground`, then brings the TUI back. Returns the status line
/// to show; a failure of `foreground` itself is reported there rather than ending the TUI.
async fn suspend(
    terminal: &mut DefaultTerminal,
    foreground: Foreground,
    dirs: &Dirs,
) -> Result<String> {
    ratatui::restore();
    let outcome = run_foreground(foreground, dirs).await;
    *terminal = ratatui::try_init().context("resuming the TUI")?;
    // The new terminal's buffers are blank, so wiping the screen is all a clear needs.
    // `Terminal::clear` would also query the cursor position, a round trip some PTYs never answer.
    terminal
        .backend_mut()
        .clear()
        .context("clearing the terminal")?;
    Ok(outcome.unwrap_or_else(|error| format!("{error:#}")))
}

async fn run_foreground(foreground: Foreground, dirs: &Dirs) -> Result<String> {
    let (name, code) = match foreground {
        Foreground::Attach { name, agent } => {
            proxy::ensure_running(dirs).await?;
            sandbox::prepare_existing(&name, agent, &|note| eprintln!("{note}")).await?;
            let code = attach::exec(&name, agent.command, &[]).await?;
            (name, code)
        }
        Foreground::Exec { name, command } => {
            let code = attach::exec(&name, command, &[]).await?;
            (name, code)
        }
    };
    Ok(format!("exited {name} (code {code})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    fn sandbox(name: &str, agent: Option<&str>) -> SandboxInfo {
        SandboxInfo {
            name: name.to_owned(),
            status: Status::Running,
            agent: agent.map(str::to_owned),
            project: None,
            image: None,
            vcpus: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
            uptime: None,
            network: None,
            proxied: true,
            rx_bytes: None,
            tx_bytes: None,
            secrets: Vec::new(),
            mounts: Vec::new(),
        }
    }

    fn sandboxes() -> Vec<SandboxInfo> {
        vec![
            sandbox("codex-api", Some("codex")),
            sandbox("pi-notes", Some("pi")),
            sandbox("foreign", None),
            sandbox("future", Some("claude")),
        ]
    }

    fn name(value: &str) -> String {
        value.to_owned()
    }

    fn step_for(action: Action) -> Result<Step> {
        step(action, &sandboxes())
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
            let (preset, entries) = read_network(&dirs);
            assert_eq!(preset, Some(Preset::Open));
            assert_eq!(entries, [] as [NetEntry; 0]);
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
        assert_eq!(
            first,
            (
                Some(Preset::Balanced),
                Some(Popup::Create { agent_index: 0 }),
                Some("network preset: balanced".to_owned())
            )
        );
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
        for (sandbox, agent) in [("codex-api", AgentId::Codex), ("pi-notes", AgentId::Pi)] {
            assert_eq!(
                step_for(Action::Attach {
                    name: name(sandbox)
                })?,
                Step::Suspend(Foreground::Attach {
                    name: name(sandbox),
                    agent: agent.agent(),
                })
            );
        }
        Ok(())
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
            })?,
            Step::Job(Job::Create {
                agent: AgentId::Pi.agent(),
                project: project.clone(),
            }),
            "creating stays on the dashboard; ↵ attaches later"
        );
        let unknown = step_for(Action::Create {
            agent: name("claude"),
            project,
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
