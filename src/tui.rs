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
use crate::app::{Action, App};
use crate::model::SandboxInfo;
use crate::{attach, sandbox, ui};

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
}

#[derive(Debug, PartialEq)]
enum Job {
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
    },
    Exec {
        name: String,
        command: &'static str,
    },
    Create {
        agent: &'static Agent,
        project: PathBuf,
    },
}

/// Results background tasks send back to the event loop, the single owner of [`App`].
enum Message {
    Sandboxes(Result<Vec<SandboxInfo>>),
    Logs {
        name: String,
        logs: Result<Vec<String>>,
    },
    Status(String),
}

/// Input plus periodic refresh. Dropping it stops both, so while the TUI is suspended nothing
/// else reads stdin or repaints.
struct Session {
    events: EventStream,
    refresh: JoinHandle<()>,
}

impl Session {
    fn start(sender: UnboundedSender<Message>) -> Self {
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

/// The name `msbx agent` gives `agent`'s sandbox for `project`; empty for an unknown agent.
pub(crate) fn preview_name(agent: &str, project: &Path) -> String {
    agents::find(agent).map_or_else(String::new, |agent| sandbox::sandbox_name(agent, project))
}

/// Runs the TUI until the user quits. The terminal is restored on every return path; a panic
/// is covered by the hook `ratatui::try_init` installs.
pub(crate) async fn run(project: PathBuf, home: Option<PathBuf>) -> Result<()> {
    let agents = AgentId::value_variants()
        .iter()
        .map(|id| id.agent().id)
        .collect();
    let mut app = App::new(project, home, agents, preview_name);
    let mut terminal = ratatui::try_init().context("setting up the terminal")?;
    let result = event_loop(&mut terminal, &mut app).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let (sender, mut receiver) = unbounded_channel();
    while let Some(foreground) = live(terminal, app, &sender, &mut receiver).await? {
        app.status_line = Some(suspend(terminal, foreground).await?);
    }
    Ok(())
}

/// Drives the TUI until the user quits (`None`) or picks something that needs the terminal.
async fn live(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    sender: &UnboundedSender<Message>,
    receiver: &mut UnboundedReceiver<Message>,
) -> Result<Option<Foreground>> {
    let mut session = Session::start(sender.clone());
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
                            tokio::spawn(run_job(job, sender.clone()));
                        }
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
        Action::Create { agent, project } => Step::Suspend(Foreground::Create {
            agent: agents::find(&agent).with_context(|| format!("unknown agent {agent}"))?,
            project,
        }),
    })
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
        Job::Start(name) => Some(format!("starting {name}…")),
        Job::Stop(name) => Some(format!("stopping {name}…")),
        Job::Remove(name) => Some(format!("removing {name}…")),
        Job::Logs(_) => None,
    }
}

// Send errors only mean the TUI has quit, leaving nobody to tell.
async fn run_job(job: Job, sender: UnboundedSender<Message>) {
    let (result, done, name) = match job {
        Job::Logs(name) => {
            let logs = sandbox::logs(&name, LOG_LINES).await;
            let _ = sender.send(Message::Logs { name, logs });
            return;
        }
        Job::Start(name) => (sandbox::start(&name).await, "started", name),
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

fn apply(app: &mut App, message: Message) {
    match message {
        Message::Sandboxes(Ok(sandboxes)) => app.set_sandboxes(sandboxes),
        Message::Logs {
            name,
            logs: Ok(logs),
        } => app.set_logs(&name, logs),
        Message::Sandboxes(Err(error))
        | Message::Logs {
            logs: Err(error), ..
        } => {
            app.status_line = Some(format!("{error:#}"));
        }
        Message::Status(status) => app.status_line = Some(status),
    }
}

/// Hands the real terminal to `foreground`, then brings the TUI back. Returns the status line
/// to show; a failure of `foreground` itself is reported there rather than ending the TUI.
async fn suspend(terminal: &mut DefaultTerminal, foreground: Foreground) -> Result<String> {
    ratatui::restore();
    let outcome = run_foreground(foreground).await;
    *terminal = ratatui::try_init().context("resuming the TUI")?;
    // The new terminal's buffers are blank, so wiping the screen is all a clear needs.
    // `Terminal::clear` would also query the cursor position, a round trip some PTYs never answer.
    terminal
        .backend_mut()
        .clear()
        .context("clearing the terminal")?;
    Ok(outcome.unwrap_or_else(|error| format!("{error:#}")))
}

async fn run_foreground(foreground: Foreground) -> Result<String> {
    let (name, code) = match foreground {
        Foreground::Attach { name, agent } => {
            sandbox::prepare_existing(&name, agent).await?;
            let code = attach::exec(&name, agent.command, &[]).await?;
            (name, code)
        }
        Foreground::Exec { name, command } => {
            let code = attach::exec(&name, command, &[]).await?;
            (name, code)
        }
        Foreground::Create { agent, project } => {
            let name = sandbox::ensure(agent, &project).await?;
            let code = attach::exec(&name, agent.command, &[]).await?;
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
            Step::Suspend(Foreground::Create {
                agent: AgentId::Pi.agent(),
                project: project.clone(),
            })
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
