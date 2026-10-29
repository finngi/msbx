//! TUI state and key handling. Pure: side effects are returned as [`Action`]s for the event loop.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::model::{NetEntry, SandboxInfo, Status};
use crate::policy::Preset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Logs,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Popup {
    Create {
        agent_index: usize,
    },
    ConfirmRemove {
        name: String,
    },
    Help,
    /// Pick a network preset; `then_create` opens the create popup afterwards (first run).
    Preset {
        index: usize,
        then_create: bool,
    },
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
    /// Allow `target` in `policy.toml`, for one sandbox or (`None`) everywhere.
    Allow {
        target: String,
        sandbox: Option<String>,
    },
    Deny {
        target: String,
        sandbox: Option<String>,
    },
    /// Write the preset; initialises the policy file when there is none yet.
    SetPreset {
        preset: Preset,
        then_create: bool,
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
    /// The global preset; `None` until `policy.toml` exists.
    pub(crate) policy: Option<Preset>,
    /// The proxy's connection log for every sandbox, blocked first then newest.
    pub(crate) network: Vec<NetEntry>,
    /// Cursor into the selected sandbox's rows of `network`.
    pub(crate) network_row: usize,
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
            policy: None,
            network: Vec::new(),
            network_row: 0,
        }
    }

    /// The selected sandbox, or `None` when the "+ new sandbox" card is selected.
    pub(crate) fn selected_sandbox(&self) -> Option<&SandboxInfo> {
        self.sandboxes.get(self.selected)
    }

    /// The Network tab rows of the selected sandbox.
    pub(crate) fn network_rows(&self) -> Vec<&NetEntry> {
        let Some(sandbox) = self.selected_sandbox() else {
            return Vec::new();
        };
        self.network
            .iter()
            .filter(|entry| entry.sandbox == sandbox.name)
            .collect()
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

    /// Selects the card for `name`, if it is listed; otherwise leaves the selection alone.
    pub(crate) fn select_named(&mut self, name: &str) {
        if let Some(index) = self
            .sandboxes
            .iter()
            .position(|sandbox| sandbox.name == name)
        {
            self.selected = index;
        }
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

    /// Replaces the connection log and preset, keeping the row cursor in range.
    pub(crate) fn set_network(&mut self, policy: Option<Preset>, network: Vec<NetEntry>) {
        self.policy = policy;
        self.network = network;
        let rows = self.network_rows().len();
        self.network_row = self.network_row.min(rows.saturating_sub(1));
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
                    Tab::Logs => Tab::Network,
                    Tab::Network => Tab::Overview,
                };
                self.load_logs_if_needed(name)
            }
            (KeyCode::Char('c'), _) | (KeyCode::Enter, None) => {
                self.open_create();
                None
            }
            (KeyCode::Char('?'), _) => {
                self.popup = Some(Popup::Help);
                None
            }
            (KeyCode::Char('p'), _) => {
                self.popup = Some(Popup::Preset {
                    index: preset_index(self.policy.unwrap_or(Preset::Balanced)),
                    then_create: false,
                });
                None
            }
            (KeyCode::Enter, Some(name)) => Some(self.attach(name)),
            (KeyCode::Char('x'), Some(name)) => Some(Action::Shell { name }),
            (KeyCode::Char('s'), Some(name)) => Some(self.toggle(name)),
            (KeyCode::Char('r'), Some(name)) => {
                self.popup = Some(Popup::ConfirmRemove { name });
                None
            }
            (KeyCode::Left | KeyCode::Char('h'), Some(_)) if self.tab == Tab::Network => {
                self.network_row = self.network_row.saturating_sub(1);
                None
            }
            (KeyCode::Right | KeyCode::Char('l'), Some(_)) if self.tab == Tab::Network => {
                let last = self.network_rows().len().saturating_sub(1);
                self.network_row = (self.network_row + 1).min(last);
                None
            }
            (KeyCode::Char('a'), Some(name)) if self.tab == Tab::Network => {
                self.rule(|target| Action::Allow {
                    target,
                    sandbox: Some(name),
                })
            }
            (KeyCode::Char('A'), Some(_)) if self.tab == Tab::Network => {
                self.rule(|target| Action::Allow {
                    target,
                    sandbox: None,
                })
            }
            (KeyCode::Char('d'), Some(name)) if self.tab == Tab::Network => {
                self.rule(|target| Action::Deny {
                    target,
                    sandbox: Some(name),
                })
            }
            _ => None,
        }
    }

    /// The create popup, or the preset question first when the policy is not initialised.
    fn open_create(&mut self) {
        self.popup = Some(match self.policy {
            Some(_) => Popup::Create { agent_index: 0 },
            None => Popup::Preset {
                index: preset_index(Preset::Balanced),
                then_create: true,
            },
        });
    }

    /// A rule for the host under the cursor; nothing for sandboxes the proxy does not serve.
    fn rule(&self, action: impl FnOnce(String) -> Action) -> Option<Action> {
        if !self
            .selected_sandbox()
            .is_some_and(|sandbox| sandbox.proxied)
        {
            return None;
        }
        let host = self.network_rows().get(self.network_row)?.host.clone();
        Some(action(host))
    }

    /// Popups are modal: they consume every key, and stay open unless the key closes them.
    fn update_popup(&mut self, popup: Popup, code: KeyCode) -> Option<Action> {
        match (popup, code) {
            (Popup::Help, KeyCode::Esc | KeyCode::Char('?' | 'q'))
            | (Popup::ConfirmRemove { .. }, KeyCode::Esc | KeyCode::Char('n'))
            | (Popup::Create { .. } | Popup::Preset { .. }, KeyCode::Esc) => None,
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
            (Popup::Preset { index, then_create }, KeyCode::Enter) => Some(Action::SetPreset {
                preset: Preset::ALL.get(index).copied().unwrap_or(Preset::Balanced),
                then_create,
            }),
            (
                Popup::Preset { index, then_create },
                KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k'),
            ) => {
                self.popup = Some(Popup::Preset {
                    index: index.saturating_sub(1),
                    then_create,
                });
                None
            }
            (
                Popup::Preset { index, then_create },
                KeyCode::Right | KeyCode::Down | KeyCode::Char('l' | 'j'),
            ) => {
                self.popup = Some(Popup::Preset {
                    index: (index + 1).min(Preset::ALL.len() - 1),
                    then_create,
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
        self.network_row = 0;
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

fn preset_index(preset: Preset) -> usize {
    Preset::ALL
        .iter()
        .position(|candidate| *candidate == preset)
        .unwrap_or(1)
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
            proxied: true,
            rx_bytes: None,
            tx_bytes: None,
            secrets: Vec::new(),
            mounts: Vec::new(),
        }
    }

    fn entry(sandbox: &str, host: &str, allowed: bool) -> NetEntry {
        NetEntry {
            sandbox: sandbox.to_owned(),
            host: host.to_owned(),
            port: 443,
            allowed,
            count: 1,
            last_seen: "12:00:00".to_owned(),
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
        app.set_network(
            Some(Preset::Balanced),
            vec![
                entry("a", "pypi.org", false),
                entry("b", "elsewhere.test", false),
                entry("a", "api.openai.com", true),
            ],
        );
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        app.update(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn name(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn select_named_moves_to_a_listed_sandbox_only() {
        let mut app = app();
        app.select_named("b");
        assert_eq!(app.selected_sandbox().map(|s| s.name.as_str()), Some("b"));
        app.select_named("not-listed");
        assert_eq!(
            app.selected_sandbox().map(|s| s.name.as_str()),
            Some("b"),
            "an unknown name keeps the selection"
        );
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
        assert_eq!(app.tab, Tab::Network);
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
    fn create_asks_for_a_preset_first_when_the_policy_is_missing() {
        let mut app = app();
        app.set_network(None, Vec::new());
        assert_eq!(press(&mut app, KeyCode::Char('c')), None);
        assert_eq!(
            app.popup,
            Some(Popup::Preset {
                index: 1,
                then_create: true
            })
        );
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.popup,
            Some(Popup::Preset {
                index: 2,
                then_create: true
            }),
            "clamps at Locked Down"
        );
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::SetPreset {
                preset: Preset::Balanced,
                then_create: true
            })
        );
        assert_eq!(app.popup, None);
    }

    #[test]
    fn p_opens_the_preset_picker_on_the_current_preset() {
        let mut app = app();
        app.set_network(Some(Preset::LockedDown), Vec::new());
        assert_eq!(press(&mut app, KeyCode::Char('p')), None);
        assert_eq!(
            app.popup,
            Some(Popup::Preset {
                index: 2,
                then_create: false
            })
        );
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::SetPreset {
                preset: Preset::Open,
                then_create: false
            })
        );
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.popup, None);
    }

    #[test]
    fn network_keys_act_on_the_row_under_the_cursor() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Network);
        let hosts: Vec<&str> = app.network_rows().iter().map(|e| e.host.as_str()).collect();
        assert_eq!(hosts, ["pypi.org", "api.openai.com"]);
        assert_eq!(
            press(&mut app, KeyCode::Char('a')),
            Some(Action::Allow {
                target: name("pypi.org"),
                sandbox: Some(name("a"))
            })
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('A')),
            Some(Action::Allow {
                target: name("pypi.org"),
                sandbox: None
            })
        );
        for _ in 0..5 {
            press(&mut app, KeyCode::Right);
        }
        assert_eq!(app.network_row, 1, "clamps at the last row");
        assert_eq!(
            press(&mut app, KeyCode::Char('d')),
            Some(Action::Deny {
                target: name("api.openai.com"),
                sandbox: Some(name("a"))
            })
        );
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.network_row, 0);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected, 1);
        assert_eq!(app.network_row, 0, "cursor resets with the selection");
    }

    #[test]
    fn network_keys_are_ignored_off_the_tab_and_for_older_sandboxes() {
        let mut app = app();
        assert_eq!(press(&mut app, KeyCode::Char('a')), None, "overview tab");
        assert_eq!(press(&mut app, KeyCode::Left), None);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        let mut older = sandbox("a", Status::Running);
        older.proxied = false;
        app.set_sandboxes(vec![older]);
        assert_eq!(press(&mut app, KeyCode::Char('a')), None, "not proxied");
        assert_eq!(press(&mut app, KeyCode::Char('d')), None);
        app.set_sandboxes(vec![sandbox("empty", Status::Running)]);
        assert_eq!(press(&mut app, KeyCode::Char('A')), None, "no rows");
        app.selected = 1;
        assert_eq!(press(&mut app, KeyCode::Char('a')), None, "new card");
    }

    #[test]
    fn set_network_clamps_the_cursor() {
        let mut app = app();
        app.tab = Tab::Network;
        app.network_row = 1;
        app.set_network(Some(Preset::Open), vec![entry("a", "only.test", false)]);
        assert_eq!(app.network_row, 0);
        assert_eq!(app.policy, Some(Preset::Open));
        app.set_network(Some(Preset::Open), Vec::new());
        assert_eq!(app.network_row, 0);
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
