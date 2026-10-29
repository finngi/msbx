//! TUI state and key handling. Pure: side effects are returned as [`Action`]s for the event loop.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::{SandboxInfo, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Logs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Popup {
    Create { agent_index: usize },
    ConfirmRemove { name: String },
    Help,
}

/// Side effects the event loop performs on behalf of [`App::update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    /// Run the sandbox's agent.
    Attach {
        name: String,
    },
    Shell {
        name: String,
    },
    Create {
        agent: String,
        project: PathBuf,
    },
    Start {
        name: String,
    },
    Stop {
        name: String,
    },
    Remove {
        name: String,
    },
    LoadLogs {
        name: String,
    },
}

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) sandboxes: Vec<SandboxInfo>,
    /// Index into the cards; `sandboxes.len()` is the "+ new sandbox" card.
    pub(crate) selected: usize,
    pub(crate) tab: Tab,
    /// Log lines for the selected sandbox.
    pub(crate) logs: Vec<String>,
    pub(crate) popup: Option<Popup>,
    /// Last error or info message.
    pub(crate) status_line: Option<String>,
    /// The folder msbx was started in; the default project for new sandboxes.
    pub(crate) project: PathBuf,
    /// Passed in rather than read from the environment so rendering stays deterministic.
    pub(crate) home: Option<PathBuf>,
    pub(crate) agents: Vec<&'static str>,
    /// The sandbox name `msbx agent` would use for an agent id and project. Injected so the create
    /// popup cannot disagree with the real naming while this module stays free of the SDK.
    pub(crate) sandbox_name: fn(&str, &Path) -> String,
}

impl App {
    pub(crate) fn new(
        project: PathBuf,
        home: Option<PathBuf>,
        agents: Vec<&'static str>,
        sandbox_name: fn(&str, &Path) -> String,
    ) -> Self {
        Self {
            sandboxes: Vec::new(),
            selected: 0,
            tab: Tab::Overview,
            logs: Vec::new(),
            popup: None,
            status_line: None,
            project,
            home,
            agents,
            sandbox_name,
        }
    }

    /// The selected sandbox, or `None` when the "+ new sandbox" card is selected.
    pub(crate) fn selected_sandbox(&self) -> Option<&SandboxInfo> {
        self.sandboxes.get(self.selected)
    }

    /// Replaces the list, keeping the selection on the same sandbox name when it still exists.
    pub(crate) fn set_sandboxes(&mut self, sandboxes: Vec<SandboxInfo>) {
        // With no sandboxes the "+ new" card is the only choice, not a deliberate one.
        let on_new_card = !self.sandboxes.is_empty() && self.selected >= self.sandboxes.len();
        let previous = self.selected_sandbox().map(|sandbox| sandbox.name.clone());
        self.sandboxes = sandboxes;
        self.selected = match previous {
            Some(name) => self
                .sandboxes
                .iter()
                .position(|sandbox| sandbox.name == name)
                .unwrap_or_else(|| self.selected.min(self.sandboxes.len())),
            None if on_new_card => self.sandboxes.len(),
            None => self.selected.min(self.sandboxes.len()),
        };
    }

    /// Shows logs loaded for `name`, unless the selection moved on while they were loading.
    pub(crate) fn set_logs(&mut self, name: &str, logs: Vec<String>) {
        if self
            .selected_sandbox()
            .is_some_and(|sandbox| sandbox.name == name)
        {
            self.logs = logs;
        }
    }

    pub(crate) fn update(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        match self.popup.take() {
            Some(popup) => self.update_popup(popup, key.code),
            None => self.update_main(key.code),
        }
    }

    fn update_main(&mut self, code: KeyCode) -> Option<Action> {
        let name = self.selected_sandbox().map(|sandbox| sandbox.name.clone());
        match (code, name) {
            (KeyCode::Char('q'), _) => Some(Action::Quit),
            (KeyCode::Up | KeyCode::Char('k'), _) => self.select(self.selected.saturating_sub(1)),
            (KeyCode::Down | KeyCode::Char('j'), _) => self.select(self.selected + 1),
            (KeyCode::Tab, name) => {
                self.tab = match self.tab {
                    Tab::Overview => Tab::Logs,
                    Tab::Logs => Tab::Overview,
                };
                self.load_logs_if_needed(name)
            }
            (KeyCode::Char('c'), _) | (KeyCode::Enter, None) => {
                self.popup = Some(Popup::Create { agent_index: 0 });
                None
            }
            (KeyCode::Char('?'), _) => {
                self.popup = Some(Popup::Help);
                None
            }
            (KeyCode::Enter, Some(name)) => Some(self.attach(name)),
            (KeyCode::Char('x'), Some(name)) => Some(Action::Shell { name }),
            (KeyCode::Char('s'), Some(name)) => Some(self.toggle(name)),
            (KeyCode::Char('r'), Some(name)) => {
                self.popup = Some(Popup::ConfirmRemove { name });
                None
            }
            _ => None,
        }
    }

    /// Popups are modal: they consume every key, and stay open unless the key closes them.
    fn update_popup(&mut self, popup: Popup, code: KeyCode) -> Option<Action> {
        match (popup, code) {
            (Popup::Help, KeyCode::Esc | KeyCode::Char('?' | 'q'))
            | (Popup::ConfirmRemove { .. }, KeyCode::Esc | KeyCode::Char('n'))
            | (Popup::Create { .. }, KeyCode::Esc) => None,
            (Popup::ConfirmRemove { name }, KeyCode::Char('y')) => Some(Action::Remove { name }),
            (Popup::Create { agent_index }, KeyCode::Enter) => {
                self.agents.get(agent_index).map(|agent| Action::Create {
                    agent: (*agent).to_owned(),
                    project: self.project.clone(),
                })
            }
            (Popup::Create { agent_index }, KeyCode::Left | KeyCode::Char('h')) => {
                self.popup = Some(Popup::Create {
                    agent_index: agent_index.saturating_sub(1),
                });
                None
            }
            (Popup::Create { agent_index }, KeyCode::Right | KeyCode::Char('l')) => {
                let last = self.agents.len().saturating_sub(1);
                self.popup = Some(Popup::Create {
                    agent_index: (agent_index + 1).min(last),
                });
                None
            }
            (popup, _) => {
                self.popup = Some(popup);
                None
            }
        }
    }

    fn select(&mut self, index: usize) -> Option<Action> {
        let index = index.min(self.sandboxes.len());
        if index == self.selected {
            return None;
        }
        self.selected = index;
        self.logs.clear();
        let name = self.selected_sandbox().map(|sandbox| sandbox.name.clone());
        self.load_logs_if_needed(name)
    }

    fn load_logs_if_needed(&self, name: Option<String>) -> Option<Action> {
        match (self.tab, name) {
            (Tab::Logs, Some(name)) => Some(Action::LoadLogs { name }),
            _ => None,
        }
    }

    /// Sandboxes msbx did not create have no agent to run, so fall back to a shell.
    fn attach(&self, name: String) -> Action {
        match self
            .selected_sandbox()
            .and_then(|sandbox| sandbox.agent.as_ref())
        {
            Some(_) => Action::Attach { name },
            None => Action::Shell { name },
        }
    }

    fn toggle(&self, name: String) -> Action {
        match self.selected_sandbox().map(|sandbox| sandbox.status) {
            Some(Status::Stopped | Status::Crashed) => Action::Start { name },
            _ => Action::Stop { name },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str, status: Status) -> SandboxInfo {
        SandboxInfo {
            name: name.to_owned(),
            status,
            agent: Some("codex".to_owned()),
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

    fn app() -> App {
        let mut app = App::new(
            PathBuf::from("/work/api"),
            None,
            vec!["codex", "pi", "shell"],
            crate::tui::preview_name,
        );
        app.set_sandboxes(vec![
            sandbox("a", Status::Running),
            sandbox("b", Status::Stopped),
            sandbox("c", Status::Crashed),
        ]);
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        app.update(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn name(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn down_and_up_move_and_clamp_including_new_card() {
        let mut app = app();
        for _ in 0..10 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.selected, 3, "clamps on the + new card");
        assert!(app.selected_sandbox().is_none());
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, 2);
        for _ in 0..10 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn tab_cycles_and_loads_logs_on_entering_logs() {
        let mut app = app();
        assert_eq!(
            press(&mut app, KeyCode::Tab),
            Some(Action::LoadLogs { name: name("a") })
        );
        assert_eq!(app.tab, Tab::Logs);
        assert_eq!(press(&mut app, KeyCode::Tab), None);
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn moving_on_logs_tab_reloads_logs_for_new_selection() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        app.logs = vec![name("old")];
        assert_eq!(
            press(&mut app, KeyCode::Down),
            Some(Action::LoadLogs { name: name("b") })
        );
        assert_eq!(app.logs, Vec::<String>::new());
    }

    #[test]
    fn tab_on_new_card_does_not_load_logs() {
        let mut app = app();
        app.selected = 3;
        assert_eq!(press(&mut app, KeyCode::Tab), None);
        assert_eq!(app.tab, Tab::Logs);
    }

    #[test]
    fn enter_attaches_to_sandbox_with_agent() {
        let mut app = app();
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Attach { name: name("a") })
        );
    }

    #[test]
    fn enter_on_sandbox_without_agent_opens_shell() {
        let mut app = app();
        let mut foreign = sandbox("foreign", Status::Running);
        foreign.agent = None;
        app.set_sandboxes(vec![foreign]);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Shell {
                name: name("foreign")
            })
        );
    }

    #[test]
    fn enter_on_new_card_opens_create_popup() {
        let mut app = app();
        app.selected = 3;
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert_eq!(app.popup, Some(Popup::Create { agent_index: 0 }));
    }

    #[test]
    fn x_opens_shell_and_is_ignored_on_new_card() {
        let mut app = app();
        assert_eq!(
            press(&mut app, KeyCode::Char('x')),
            Some(Action::Shell { name: name("a") })
        );
        app.selected = 3;
        assert_eq!(press(&mut app, KeyCode::Char('x')), None);
    }

    #[test]
    fn s_stops_running_and_starts_stopped_or_crashed() {
        let mut app = app();
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            Some(Action::Stop { name: name("a") })
        );
        app.selected = 1;
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            Some(Action::Start { name: name("b") })
        );
        app.selected = 2;
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            Some(Action::Start { name: name("c") })
        );
        app.selected = 3;
        assert_eq!(press(&mut app, KeyCode::Char('s')), None);
    }

    #[test]
    fn create_popup_picks_agent_and_emits_create() {
        let mut app = app();
        assert_eq!(press(&mut app, KeyCode::Char('c')), None);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.popup, Some(Popup::Create { agent_index: 0 }));
        for _ in 0..5 {
            press(&mut app, KeyCode::Right);
        }
        assert_eq!(app.popup, Some(Popup::Create { agent_index: 2 }));
        press(&mut app, KeyCode::Left);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Create {
                agent: name("pi"),
                project: PathBuf::from("/work/api"),
            })
        );
        assert_eq!(app.popup, None);
    }

    #[test]
    fn create_popup_esc_closes_and_ignores_other_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(press(&mut app, KeyCode::Char('q')), None);
        assert_eq!(press(&mut app, KeyCode::Char('s')), None);
        assert!(app.popup.is_some());
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup, None);
    }

    #[test]
    fn remove_requires_confirmation() {
        let mut app = app();
        assert_eq!(press(&mut app, KeyCode::Char('r')), None);
        assert_eq!(app.popup, Some(Popup::ConfirmRemove { name: name("a") }));
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            Some(Action::Remove { name: name("a") })
        );
        assert_eq!(app.popup, None);
    }

    #[test]
    fn remove_cancelled_by_n_or_esc() {
        let mut app = app();
        for cancel in [KeyCode::Char('n'), KeyCode::Esc] {
            press(&mut app, KeyCode::Char('r'));
            assert_eq!(press(&mut app, cancel), None);
            assert_eq!(app.popup, None);
        }
    }

    #[test]
    fn r_on_new_card_is_ignored() {
        let mut app = app();
        app.selected = 3;
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.popup, None);
    }

    #[test]
    fn question_mark_toggles_help_and_esc_closes_it() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.popup, Some(Popup::Help));
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.popup, None);
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup, None);
    }

    #[test]
    fn q_quits_but_closes_help_first() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(press(&mut app, KeyCode::Char('q')), None);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn ctrl_c_quits_even_with_popup_open() {
        let mut app = app();
        press(&mut app, KeyCode::Char('c'));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.update(ctrl_c), Some(Action::Quit));
    }

    #[test]
    fn esc_without_popup_and_unknown_keys_are_ignored() {
        let mut app = app();
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert_eq!(press(&mut app, KeyCode::Char('z')), None);
        assert_eq!(press(&mut app, KeyCode::F(5)), None);
        assert_eq!(app.selected, 0);
        assert_eq!(app.popup, None);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut app = app();
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(app.update(release), None);
    }

    #[test]
    fn set_logs_applies_only_to_the_selected_sandbox() {
        let mut app = app();
        app.set_logs("b", vec![name("stale")]);
        assert_eq!(app.logs, Vec::<String>::new());
        app.set_logs("a", vec![name("fresh")]);
        assert_eq!(app.logs, [name("fresh")]);
        app.selected = 3;
        app.set_logs("a", vec![name("late")]);
        assert_eq!(app.logs, [name("fresh")], "the + new card shows no logs");
    }

    #[test]
    fn set_sandboxes_keeps_selection_by_name() {
        let mut app = app();
        app.selected = 1;
        app.set_sandboxes(vec![
            sandbox("new", Status::Running),
            sandbox("c", Status::Running),
            sandbox("b", Status::Stopped),
        ]);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn set_sandboxes_clamps_when_selected_sandbox_is_gone() {
        let mut app = app();
        app.selected = 2;
        app.set_sandboxes(vec![sandbox("a", Status::Running)]);
        assert_eq!(app.selected, 1, "lands on the + new card");
        app.set_sandboxes(Vec::new());
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn first_set_sandboxes_selects_first_sandbox() {
        let app = app();
        assert_eq!(app.selected, 0);
        assert_eq!(app.selected_sandbox().map(|s| s.name.as_str()), Some("a"));
    }

    #[test]
    fn set_sandboxes_keeps_new_card_selected() {
        let mut app = app();
        app.selected = 3;
        app.set_sandboxes(vec![
            sandbox("a", Status::Running),
            sandbox("b", Status::Running),
            sandbox("c", Status::Running),
            sandbox("d", Status::Running),
        ]);
        assert_eq!(app.selected, 4);
    }
}
