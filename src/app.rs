//! Pure dashboard state. IO is returned as an action for the terminal loop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::input::TextInput;
use crate::keymap::{self, Command};
use crate::model::{NetEntry, RuleInfo, SandboxInfo, Status, validate_name};
use crate::policy::{self, DEFAULT_HOST_PORTS, Preset, Source, Target};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Logs,
    Network,
    Rules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Cards,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusFilter {
    All,
    Allowed,
    Blocked,
}

impl StatusFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Allowed => "Allowed",
            Self::Blocked => "Blocked",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::All => Self::Allowed,
            Self::Allowed => Self::Blocked,
            Self::Blocked => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateStage {
    Agent,
    Configure,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateField {
    Name,
    Workspace,
    Back,
    Create,
}

impl CreateField {
    fn step(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Name, false) | (Self::Back, true) => Self::Workspace,
            (Self::Workspace, false) | (Self::Create, true) => Self::Back,
            (Self::Back, false) | (Self::Name, true) => Self::Create,
            (Self::Create, false) | (Self::Workspace, true) => Self::Name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderPicker {
    pub(crate) path: PathBuf,
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) filter: TextInput,
    pub(crate) row: usize,
    pub(crate) error: Option<String>,
    pub(crate) loading: bool,
}

impl FolderPicker {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            directories: Vec::new(),
            filter: TextInput::default(),
            row: 0,
            error: None,
            loading: true,
        }
    }

    pub(crate) fn visible(&self) -> Vec<&PathBuf> {
        let query = self.filter.text().to_lowercase();
        self.directories
            .iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(&query))
            })
            .collect()
    }

    fn visit(&mut self, path: PathBuf) -> Action {
        *self = Self::new(path.clone());
        Action::LoadFolders { path }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateWizard {
    pub(crate) agent_index: usize,
    pub(crate) stage: CreateStage,
    pub(crate) name: TextInput,
    pub(crate) custom_name: bool,
    pub(crate) project: PathBuf,
    pub(crate) field: CreateField,
    pub(crate) folder: FolderPicker,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsField {
    Preset,
    HostPorts,
}

/// The `Ctrl-S` form: the default preset and the Mac ports new sandboxes may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsForm {
    /// Index into [`Preset::ALL`].
    pub(crate) preset: usize,
    pub(crate) host_ports: TextInput,
    pub(crate) field: SettingsField,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Popup {
    Create(CreateWizard),
    /// Details of one sandbox, kept by name so a refresh cannot switch it.
    Info {
        name: String,
    },
    Settings(SettingsForm),
    ConfirmRemove {
        name: String,
    },
    Help,
    Preset {
        index: usize,
        then_create: bool,
    },
    AddRule {
        target: TextInput,
        allowed: bool,
        sandbox: Option<String>,
        error: Option<String>,
    },
    ConfirmRuleRemove {
        rule: RuleInfo,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
    Attach {
        name: String,
    },
    Shell {
        name: String,
    },
    Create {
        agent: String,
        project: PathBuf,
        name: String,
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
    LoadFolders {
        path: PathBuf,
    },
    Allow {
        target: String,
        sandbox: Option<String>,
    },
    Deny {
        target: String,
        sandbox: Option<String>,
    },
    RemoveRule {
        target: String,
        sandbox: Option<String>,
    },
    SetPreset {
        preset: Preset,
        then_create: bool,
    },
    SaveSettings {
        preset: Preset,
        host_ports: Vec<u16>,
    },
}

type NetworkKey = (String, String, u16);

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) sandboxes: Vec<SandboxInfo>,
    pub(crate) selected: usize,
    pub(crate) tab: Tab,
    pub(crate) focus: Focus,
    pub(crate) logs: Vec<String>,
    pub(crate) popup: Option<Popup>,
    pub(crate) status_line: Option<String>,
    pub(crate) project: PathBuf,
    pub(crate) home: Option<PathBuf>,
    pub(crate) agents: Vec<&'static str>,
    pub(crate) sandbox_name: fn(&str, &Path) -> String,
    pub(crate) policy: Option<Preset>,
    /// The host ports `policy.toml` gives new sandboxes; `None` while it does not exist.
    pub(crate) host_ports: Option<Vec<u16>>,
    pub(crate) network: Vec<NetEntry>,
    pub(crate) network_row: usize,
    pub(crate) rules: Vec<RuleInfo>,
    pub(crate) rule_row: usize,
    /// A rule just added from the Rules tab; selected once the reloaded rules list has it.
    pub(crate) pending_rule: Option<(String, Option<String>)>,
    pub(crate) credentials: Vec<String>,
    pub(crate) no_color: bool,
    pub(crate) full_network: bool,
    pub(crate) network_search: TextInput,
    pub(crate) searching: bool,
    pub(crate) status_filter: StatusFilter,
    pub(crate) sandbox_filter: Option<String>,
    pub(crate) detail_scroll: usize,
    pub(crate) popup_scroll: usize,
    detail_scroll_limit: usize,
    popup_scroll_limit: usize,
    expanded: Option<NetworkKey>,
    search_before: String,
    /// Per sandbox, the git files of its project that changed since its last agent session
    /// started (RFC 0001), already formatted; absent or empty when nothing changed.
    pub(crate) git_changes: BTreeMap<String, Vec<String>>,
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
            focus: Focus::Cards,
            logs: Vec::new(),
            popup: None,
            status_line: None,
            project,
            home,
            agents,
            sandbox_name,
            policy: None,
            host_ports: None,
            network: Vec::new(),
            network_row: 0,
            rules: Vec::new(),
            rule_row: 0,
            pending_rule: None,
            credentials: Vec::new(),
            no_color: false,
            full_network: false,
            network_search: TextInput::default(),
            searching: false,
            status_filter: StatusFilter::All,
            sandbox_filter: None,
            detail_scroll: 0,
            popup_scroll: 0,
            detail_scroll_limit: 0,
            popup_scroll_limit: 0,
            expanded: None,
            search_before: String::new(),
            git_changes: BTreeMap::new(),
        }
    }

    pub(crate) fn selected_sandbox(&self) -> Option<&SandboxInfo> {
        self.sandboxes.get(self.selected)
    }

    /// The git changes recorded for the selected sandbox's project.
    pub(crate) fn selected_git_changes(&self) -> &[String] {
        self.selected_sandbox()
            .and_then(|sandbox| self.git_changes.get(&sandbox.name))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn network_rows(&self) -> Vec<&NetEntry> {
        let scope = self.rule_scope();
        if !self.full_network && scope.is_none() {
            return Vec::new();
        }
        let query = if self.full_network {
            self.network_search.text().to_lowercase()
        } else {
            String::new()
        };
        self.network
            .iter()
            .filter(|entry| {
                scope.as_ref().is_none_or(|name| entry.sandbox == *name)
                    && (!self.full_network
                        || match self.status_filter {
                            StatusFilter::All => true,
                            StatusFilter::Allowed => entry.allowed,
                            StatusFilter::Blocked => !entry.allowed,
                        })
                    && format!("{} {}:{}", entry.sandbox, entry.host, entry.port)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }

    pub(crate) fn selected_network(&self) -> Option<&NetEntry> {
        self.network_rows().get(self.network_row).copied()
    }

    pub(crate) fn network_is_expanded(&self) -> bool {
        self.expanded.is_some()
    }

    pub(crate) fn is_expanded(&self, entry: &NetEntry) -> bool {
        self.expanded.as_ref() == Some(&network_key(entry))
    }

    pub(crate) fn rule_scope(&self) -> Option<String> {
        if self.full_network {
            self.sandbox_filter.clone()
        } else {
            self.selected_sandbox().map(|sandbox| sandbox.name.clone())
        }
    }

    pub(crate) fn rule_rows(&self) -> Vec<&RuleInfo> {
        let scope = self.rule_scope();
        if !self.full_network && scope.is_none() {
            return Vec::new();
        }
        let query = if self.full_network {
            self.network_search.text().to_lowercase()
        } else {
            String::new()
        };
        self.rules
            .iter()
            .filter(|rule| {
                (rule.sandbox.is_none()
                    || scope
                        .as_ref()
                        .is_none_or(|name| rule.sandbox.as_ref() == Some(name)))
                    && format!(
                        "{} {} {}",
                        rule.target,
                        rule.source.name(),
                        rule.sandbox.as_deref().unwrap_or("global")
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }

    pub(crate) fn set_sandboxes(&mut self, sandboxes: Vec<SandboxInfo>) {
        let previous = self.selected_sandbox().map(|sandbox| sandbox.name.clone());
        let on_new = !self.sandboxes.is_empty() && self.selected == self.sandboxes.len();
        self.sandboxes = sandboxes;
        self.selected = match previous.as_deref() {
            Some(name) => self
                .sandboxes
                .iter()
                .position(|sandbox| sandbox.name == name)
                .unwrap_or_else(|| self.selected.min(self.sandboxes.len())),
            None if on_new => self.sandboxes.len(),
            None => self.selected.min(self.sandboxes.len()),
        };
        if previous.as_deref() != self.selected_sandbox().map(|sandbox| sandbox.name.as_str()) {
            self.logs.clear();
            self.reset_rows();
        }
        self.clamp_rows();
    }

    pub(crate) fn select_named(&mut self, name: &str) {
        if let Some(index) = self
            .sandboxes
            .iter()
            .position(|sandbox| sandbox.name == name)
        {
            if index != self.selected {
                self.logs.clear();
                self.detail_scroll = 0;
            }
            self.selected = index;
            self.clamp_rows();
        }
    }

    /// The IO loop supplies the pure renderer's measured bound before input and redraw.
    pub(crate) fn set_detail_scroll_limit(&mut self, limit: usize) {
        self.detail_scroll_limit = limit;
        self.detail_scroll = self.detail_scroll.min(limit);
    }

    /// Keep help and info scrolling inside the measured viewport, including after resize.
    pub(crate) fn set_popup_scroll_limit(&mut self, limit: usize) {
        self.popup_scroll_limit = limit;
        self.popup_scroll = self.popup_scroll.min(limit);
    }

    pub(crate) fn set_logs(&mut self, name: &str, logs: Vec<String>) {
        if self
            .selected_sandbox()
            .is_some_and(|sandbox| sandbox.name == name)
        {
            self.logs = logs;
        }
    }

    pub(crate) fn set_network(&mut self, policy: Option<Preset>, network: Vec<NetEntry>) {
        let previous = self.selected_network().map(network_key);
        self.policy = policy;
        self.network = network;
        if let Some(previous) = previous {
            self.network_row = self
                .network_rows()
                .iter()
                .position(|entry| network_key(entry) == previous)
                .unwrap_or(self.network_row);
        }
        self.clamp_rows();
    }

    pub(crate) fn set_rules(&mut self, rules: Vec<RuleInfo>) {
        let previous = self.rule_rows().get(self.rule_row).copied().cloned();
        self.rules = rules;
        if let Some((target, sandbox)) = self.pending_rule.clone()
            && let Some(index) = self
                .rule_rows()
                .iter()
                .position(|rule| rule.target == target && rule.sandbox == sandbox)
        {
            self.pending_rule = None;
            self.rule_row = index;
        } else if let Some(previous) = previous {
            self.rule_row = self
                .rule_rows()
                .iter()
                .position(|rule| **rule == previous)
                .unwrap_or(self.rule_row);
        }
        self.clamp_rows();
    }

    pub(crate) fn set_folders(&mut self, path: &Path, result: Result<Vec<PathBuf>, String>) {
        let Some(Popup::Create(wizard)) = &mut self.popup else {
            return;
        };
        if wizard.stage != CreateStage::Folder || wizard.folder.path != path {
            return;
        }
        wizard.folder.loading = false;
        match result {
            Ok(directories) => {
                wizard.folder.directories = directories;
                wizard.folder.error = None;
            }
            Err(error) => wizard.folder.error = Some(error),
        }
        wizard.folder.row = wizard.folder.row.min(wizard.folder.visible().len() + 1);
    }

    fn clamp_rows(&mut self) {
        self.network_row = self
            .network_row
            .min(self.network_rows().len().saturating_sub(1));
        self.rule_row = self.rule_row.min(self.rule_rows().len().saturating_sub(1));
        if self.expanded.as_ref().is_some_and(|key| {
            !self
                .network_rows()
                .iter()
                .any(|entry| network_key(entry) == *key)
        }) {
            self.expanded = None;
        }
    }

    pub(crate) fn update(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if key.modifiers.contains(KeyModifiers::ALT)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && !matches!(key.code, KeyCode::Char('h' | 's' | 'u')))
        {
            return None;
        }
        if let Some(popup) = self.popup.take() {
            return self.update_popup(popup, key);
        }
        if self.searching {
            self.update_search(key);
            return None;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('h') {
            self.show_help();
            return None;
        }
        self.update_main(keymap::command(self, key)?)
    }

    pub(crate) fn update_paste(&mut self, text: &str) {
        match self.popup.as_mut() {
            Some(Popup::Create(wizard))
                if wizard.stage == CreateStage::Configure && wizard.field == CreateField::Name =>
            {
                wizard.name.insert(text);
                wizard.custom_name = true;
                wizard.error = None;
            }
            Some(Popup::Create(wizard)) if wizard.stage == CreateStage::Folder => {
                wizard.folder.filter.insert(text);
                wizard.folder.row = 0;
            }
            Some(Popup::AddRule { target, error, .. }) => {
                target.insert(text);
                *error = None;
            }
            Some(Popup::Settings(form)) if form.field == SettingsField::HostPorts => {
                form.host_ports.insert(text);
                form.error = None;
            }
            None if self.searching => {
                self.network_search.insert(text);
                self.reset_rows();
            }
            _ => {}
        }
    }

    fn update_main(&mut self, command: Command) -> Option<Action> {
        match command {
            Command::Quit => return Some(Action::Quit),
            Command::Help => self.show_help(),
            Command::FocusNext => {
                self.focus = if self.focus == Focus::Cards {
                    Focus::Detail
                } else {
                    Focus::Cards
                }
            }
            Command::FocusCards => self.focus = Focus::Cards,
            Command::FocusDetail => self.focus = Focus::Detail,
            Command::Previous => return self.move_row(false),
            Command::Next => return self.move_row(true),
            Command::PreviousTab => return self.change_tab(false),
            Command::NextTab => return self.change_tab(true),
            Command::Enter => return self.enter(),
            Command::Create => self.open_create(),
            Command::Shell => {
                return self.selected_sandbox().map(|sandbox| Action::Shell {
                    name: sandbox.name.clone(),
                });
            }
            Command::Toggle => return self.toggle(),
            Command::Remove => self.open_remove(),
            Command::Network => {
                self.full_network = !self.full_network;
                self.tab = Tab::Network;
                self.reset_rows();
            }
            Command::Preset => {
                self.popup = Some(Popup::Preset {
                    index: preset_index(self.policy.unwrap_or(Preset::Balanced)),
                    then_create: false,
                });
            }
            Command::Info => {
                if let Some(sandbox) = self.selected_sandbox() {
                    self.popup = Some(Popup::Info {
                        name: sandbox.name.clone(),
                    });
                    self.popup_scroll = 0;
                }
            }
            Command::Settings => self.open_settings(),
            Command::Allow => return self.allow(),
            Command::AllowAll => return self.rule_action(true, true),
            Command::Block => return self.rule_action(false, false),
            Command::Search => {
                self.search_before = self.network_search.text().to_owned();
                self.searching = true;
            }
            Command::Status => {
                self.status_filter = self.status_filter.next();
                self.reset_rows();
            }
            Command::Sandbox => self.next_sandbox_filter(),
            Command::Back => self.back(),
            Command::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            Command::PageDown => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add(5)
                    .min(self.detail_scroll_limit);
            }
            Command::Confirm | Command::Edit => {}
        }
        None
    }

    fn open_settings(&mut self) {
        let ports = self.host_ports.as_deref().unwrap_or(&DEFAULT_HOST_PORTS);
        let text = ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        self.popup = Some(Popup::Settings(SettingsForm {
            preset: preset_index(self.policy.unwrap_or(Preset::Balanced)),
            host_ports: TextInput::new(&text),
            field: SettingsField::Preset,
            error: None,
        }));
    }

    fn show_help(&mut self) {
        self.popup_scroll = 0;
        self.popup = Some(Popup::Help);
    }

    fn reset_rows(&mut self) {
        self.network_row = 0;
        self.rule_row = 0;
        self.expanded = None;
        self.detail_scroll = 0;
    }

    fn move_row(&mut self, down: bool) -> Option<Action> {
        if !self.full_network && self.focus == Focus::Cards {
            return self.select(step_index(self.selected, down, self.sandboxes.len()));
        }
        match self.tab {
            Tab::Network => {
                self.network_row = step_index(
                    self.network_row,
                    down,
                    self.network_rows().len().saturating_sub(1),
                );
                self.expanded = None;
                self.detail_scroll = 0;
            }
            Tab::Rules => {
                self.rule_row = step_index(
                    self.rule_row,
                    down,
                    self.rule_rows().len().saturating_sub(1),
                );
            }
            Tab::Logs | Tab::Overview => {
                self.detail_scroll = step_index(self.detail_scroll, down, self.detail_scroll_limit);
            }
        }
        None
    }

    fn change_tab(&mut self, forward: bool) -> Option<Action> {
        self.tab = if self.full_network {
            if self.tab == Tab::Network {
                Tab::Rules
            } else {
                Tab::Network
            }
        } else {
            match (self.tab, forward) {
                (Tab::Overview, true) | (Tab::Network, false) => Tab::Logs,
                (Tab::Logs, true) | (Tab::Rules, false) => Tab::Network,
                (Tab::Network, true) | (Tab::Overview, false) => Tab::Rules,
                (Tab::Rules, true) | (Tab::Logs, false) => Tab::Overview,
            }
        };
        self.detail_scroll = 0;
        self.load_logs_if_needed()
    }

    fn enter(&mut self) -> Option<Action> {
        if self.full_network || self.focus == Focus::Detail {
            if self.tab == Tab::Network
                && let Some(key) = self.selected_network().map(network_key)
            {
                self.detail_scroll = 0;
                self.expanded = if self.expanded.as_ref() == Some(&key) {
                    None
                } else {
                    Some(key)
                };
            }
            return None;
        }
        let Some(sandbox) = self.selected_sandbox() else {
            self.open_create();
            return None;
        };
        let name = sandbox.name.clone();
        Some(if sandbox.agent.is_some() {
            Action::Attach { name }
        } else {
            Action::Shell { name }
        })
    }

    fn toggle(&self) -> Option<Action> {
        let sandbox = self.selected_sandbox()?;
        let name = sandbox.name.clone();
        Some(match sandbox.status {
            Status::Stopped | Status::Crashed => Action::Start { name },
            _ => Action::Stop { name },
        })
    }

    fn select(&mut self, index: usize) -> Option<Action> {
        if index == self.selected {
            return None;
        }
        self.selected = index.min(self.sandboxes.len());
        self.logs.clear();
        self.detail_scroll = 0;
        self.reset_rows();
        self.load_logs_if_needed()
    }

    fn load_logs_if_needed(&self) -> Option<Action> {
        if self.tab != Tab::Logs {
            return None;
        }
        self.selected_sandbox().map(|sandbox| Action::LoadLogs {
            name: sandbox.name.clone(),
        })
    }

    pub(crate) fn open_create(&mut self) {
        if self.policy.is_none() {
            self.popup = Some(Popup::Preset {
                index: preset_index(Preset::Balanced),
                then_create: true,
            });
            return;
        }
        let agent = self.agents.first().copied().unwrap_or_default();
        self.popup = Some(Popup::Create(CreateWizard {
            agent_index: 0,
            stage: CreateStage::Agent,
            name: TextInput::new(&(self.sandbox_name)(agent, &self.project)),
            custom_name: false,
            project: self.project.clone(),
            field: CreateField::Name,
            folder: FolderPicker::new(self.project.clone()),
            error: None,
        }));
    }

    fn open_remove(&mut self) {
        if (self.full_network || self.focus == Focus::Detail) && self.tab == Tab::Rules {
            if let Some(rule) = self.rule_rows().get(self.rule_row).copied() {
                if rule.source == Source::Preset {
                    self.status_line =
                        Some("preset rules are read-only; press p to change the preset".to_owned());
                } else {
                    self.popup = Some(Popup::ConfirmRuleRemove { rule: rule.clone() });
                }
            }
        } else if let Some(sandbox) = self.selected_sandbox() {
            self.popup = Some(Popup::ConfirmRemove {
                name: sandbox.name.clone(),
            });
        }
    }

    fn allow(&mut self) -> Option<Action> {
        if self.tab == Tab::Rules {
            if !self.full_network
                && !self
                    .selected_sandbox()
                    .is_some_and(|sandbox| sandbox.proxied)
            {
                self.status_line =
                    Some("select a proxied sandbox or use n for global rules".to_owned());
                return None;
            }
            self.popup = Some(Popup::AddRule {
                target: TextInput::default(),
                allowed: true,
                sandbox: self.rule_scope(),
                error: None,
            });
            None
        } else {
            self.rule_action(true, false)
        }
    }

    fn rule_action(&self, allowed: bool, everywhere: bool) -> Option<Action> {
        let entry = self.selected_network()?;
        if !everywhere && entry.allowed == allowed {
            return None;
        }
        // Log-only names can outlive cards. Historical entries still identify their exact scope.
        if self
            .sandboxes
            .iter()
            .find(|sandbox| sandbox.name == entry.sandbox)
            .is_some_and(|sandbox| !sandbox.proxied)
        {
            return None;
        }
        let sandbox = if everywhere {
            None
        } else {
            Some(entry.sandbox.clone())
        };
        let target = entry.host.clone();
        Some(if allowed {
            Action::Allow { target, sandbox }
        } else {
            Action::Deny { target, sandbox }
        })
    }

    fn next_sandbox_filter(&mut self) {
        let mut names: Vec<_> = self
            .sandboxes
            .iter()
            .map(|sandbox| sandbox.name.clone())
            .chain(self.network.iter().map(|entry| entry.sandbox.clone()))
            .chain(self.rules.iter().filter_map(|rule| rule.sandbox.clone()))
            .collect();
        names.sort();
        names.dedup();
        self.sandbox_filter = match &self.sandbox_filter {
            None => names.first().cloned(),
            Some(name) => names
                .iter()
                .position(|candidate| candidate == name)
                .and_then(|index| names.get(index + 1))
                .cloned(),
        };
        self.reset_rows();
    }

    fn back(&mut self) {
        if self.full_network && !self.network_search.text().is_empty() {
            self.network_search.set_text("");
            self.reset_rows();
        } else if self.expanded.is_some() {
            self.expanded = None;
        } else if self.full_network {
            self.full_network = false;
            self.reset_rows();
        }
    }

    fn update_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.network_search.set_text(&self.search_before);
                self.searching = false;
            }
            KeyCode::Enter => self.searching = false,
            _ => {
                self.network_search.update(key);
            }
        }
        self.reset_rows();
    }

    fn update_popup(&mut self, popup: Popup, key: KeyEvent) -> Option<Action> {
        // Only text fields process control modifiers (Ctrl-U); unrelated modified commands do nothing.
        match popup {
            Popup::Create(wizard) => return self.update_wizard(wizard, key),
            Popup::AddRule {
                target,
                allowed,
                sandbox,
                error,
            } => return self.update_rule(target, allowed, sandbox, error, key),
            Popup::Settings(form) => return self.update_settings(form, key),
            _ if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.popup = Some(popup);
            }
            Popup::Help => self.update_help(key.code),
            Popup::Info { name } => self.update_info(name, key.code),
            Popup::ConfirmRemove { name } => match key.code {
                KeyCode::Char('y') => return Some(Action::Remove { name }),
                KeyCode::Esc | KeyCode::Char('n') => {}
                _ => self.popup = Some(Popup::ConfirmRemove { name }),
            },
            Popup::ConfirmRuleRemove { rule } => match key.code {
                KeyCode::Char('y') => {
                    return Some(Action::RemoveRule {
                        target: rule.target,
                        sandbox: rule.sandbox,
                    });
                }
                KeyCode::Esc | KeyCode::Char('n') => {}
                _ => self.popup = Some(Popup::ConfirmRuleRemove { rule }),
            },
            Popup::Preset { index, then_create } => {
                return self.update_preset(index, then_create, key.code);
            }
        }
        None
    }

    fn update_help(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('?' | 'q') => return,
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_scroll = self.popup_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_scroll = step_index(self.popup_scroll, true, self.popup_scroll_limit);
            }
            _ => {}
        }
        self.popup = Some(Popup::Help);
    }

    fn update_info(&mut self, name: String, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('i' | 'q') => return,
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_scroll = self.popup_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_scroll = step_index(self.popup_scroll, true, self.popup_scroll_limit);
            }
            _ => {}
        }
        self.popup = Some(Popup::Info { name });
    }

    /// Ports are checked with the parser `msbx policy host-ports` uses; a bad list keeps the form.
    fn update_settings(&mut self, mut form: SettingsForm, key: KeyEvent) -> Option<Action> {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc if plain => return None,
            KeyCode::Tab | KeyCode::BackTab if plain => {
                form.field = match form.field {
                    SettingsField::Preset => SettingsField::HostPorts,
                    SettingsField::HostPorts => SettingsField::Preset,
                };
            }
            KeyCode::Enter if plain => match policy::parse_host_ports(form.host_ports.text()) {
                Ok(host_ports) => {
                    return Some(Action::SaveSettings {
                        preset: Preset::ALL
                            .get(form.preset)
                            .copied()
                            .unwrap_or(Preset::Balanced),
                        host_ports,
                    });
                }
                Err(error) => {
                    form.error = Some(format!("{error:#}"));
                    form.field = SettingsField::HostPorts;
                }
            },
            _ if form.field == SettingsField::HostPorts => {
                if form.host_ports.update(key) {
                    form.error = None;
                }
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k') if plain => {
                form.preset = form.preset.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l' | 'j') if plain => {
                form.preset = (form.preset + 1).min(Preset::ALL.len() - 1);
            }
            _ => {}
        }
        self.popup = Some(Popup::Settings(form));
        None
    }

    fn update_preset(
        &mut self,
        mut index: usize,
        then_create: bool,
        code: KeyCode,
    ) -> Option<Action> {
        match code {
            KeyCode::Esc => return None,
            KeyCode::Enter => {
                return Some(Action::SetPreset {
                    preset: Preset::ALL.get(index).copied().unwrap_or(Preset::Balanced),
                    then_create,
                });
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h' | 'k') => {
                index = index.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l' | 'j') => {
                index = (index + 1).min(Preset::ALL.len() - 1);
            }
            _ => {}
        }
        self.popup = Some(Popup::Preset { index, then_create });
        None
    }

    fn update_rule(
        &mut self,
        mut target: TextInput,
        mut allowed: bool,
        sandbox: Option<String>,
        mut error: Option<String>,
        key: KeyEvent,
    ) -> Option<Action> {
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Tab | KeyCode::BackTab => allowed = !allowed,
            KeyCode::Enter => match Target::parse(target.text()) {
                Ok(target) => {
                    self.pending_rule = Some((target.to_string(), sandbox.clone()));
                    return Some(if allowed {
                        Action::Allow {
                            target: target.to_string(),
                            sandbox,
                        }
                    } else {
                        Action::Deny {
                            target: target.to_string(),
                            sandbox,
                        }
                    });
                }
                Err(failure) => error = Some(failure.to_string()),
            },
            _ => {
                if target.update(key) {
                    error = None;
                }
            }
        }
        self.popup = Some(Popup::AddRule {
            target,
            allowed,
            sandbox,
            error,
        });
        None
    }

    fn update_wizard(&mut self, mut wizard: CreateWizard, key: KeyEvent) -> Option<Action> {
        let action = match wizard.stage {
            CreateStage::Agent => {
                if key.code == KeyCode::Esc {
                    return None;
                }
                self.update_agent(&mut wizard, key);
                None
            }
            CreateStage::Configure => self.update_configure(&mut wizard, key),
            CreateStage::Folder => self.update_folder(&mut wizard, key),
        };
        if !matches!(action, Some(Action::Create { .. })) {
            self.popup = Some(Popup::Create(wizard));
        }
        action
    }

    fn update_agent(&self, wizard: &mut CreateWizard, key: KeyEvent) {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        let last = self.agents.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                wizard.agent_index = wizard.agent_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                wizard.agent_index = (wizard.agent_index + 1).min(last);
            }
            KeyCode::Enter if !self.agents.is_empty() => {
                wizard.stage = CreateStage::Configure;
                wizard.field = CreateField::Name;
            }
            _ => {}
        }
        self.default_name(wizard);
    }

    fn default_name(&self, wizard: &mut CreateWizard) {
        if !wizard.custom_name {
            let agent = self
                .agents
                .get(wizard.agent_index)
                .copied()
                .unwrap_or_default();
            wizard
                .name
                .set_text(&(self.sandbox_name)(agent, &wizard.project));
        }
    }

    fn update_configure(&self, wizard: &mut CreateWizard, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => wizard.stage = CreateStage::Agent,
            KeyCode::Tab => wizard.field = wizard.field.step(false),
            KeyCode::BackTab => wizard.field = wizard.field.step(true),
            KeyCode::Enter => return self.choose_field(wizard),
            _ if wizard.field == CreateField::Name => {
                let before = wizard.name.text().to_owned();
                wizard.name.update(key);
                if before != wizard.name.text() {
                    wizard.custom_name = true;
                    wizard.error = None;
                }
            }
            _ => {}
        }
        None
    }

    fn choose_field(&self, wizard: &mut CreateWizard) -> Option<Action> {
        match wizard.field {
            CreateField::Name => wizard.field = CreateField::Workspace,
            CreateField::Workspace => {
                wizard.stage = CreateStage::Folder;
                return Some(wizard.folder.visit(wizard.project.clone()));
            }
            CreateField::Back => wizard.stage = CreateStage::Agent,
            CreateField::Create => {
                if let Err(error) = validate_name(wizard.name.text()) {
                    wizard.error = Some(error);
                    wizard.field = CreateField::Name;
                    return None;
                }
                if self
                    .sandboxes
                    .iter()
                    .any(|sandbox| sandbox.name == wizard.name.text())
                {
                    wizard.error = Some(
                        "name already exists; choose another name or attach to its card".to_owned(),
                    );
                    return None;
                }
                return self
                    .agents
                    .get(wizard.agent_index)
                    .map(|agent| Action::Create {
                        agent: (*agent).to_owned(),
                        project: wizard.project.clone(),
                        name: wizard.name.text().to_owned(),
                    });
            }
        }
        None
    }

    fn update_folder(&self, wizard: &mut CreateWizard, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => wizard.stage = CreateStage::Configure,
            KeyCode::Up => wizard.folder.row = wizard.folder.row.saturating_sub(1),
            KeyCode::Down => {
                wizard.folder.row = (wizard.folder.row + 1).min(wizard.folder.visible().len() + 1);
            }
            KeyCode::Enter => return self.choose_folder(wizard),
            _ => {
                if wizard.folder.filter.update(key) {
                    wizard.folder.row = 0;
                }
            }
        }
        None
    }

    fn choose_folder(&self, wizard: &mut CreateWizard) -> Option<Action> {
        match wizard.folder.row {
            0 => {
                if wizard.folder.loading || wizard.folder.error.is_some() {
                    return None;
                }
                wizard.project.clone_from(&wizard.folder.path);
                wizard.stage = CreateStage::Configure;
                wizard.field = CreateField::Create;
                self.default_name(wizard);
            }
            1 => {
                let parent = wizard.folder.path.parent()?.to_path_buf();
                return Some(wizard.folder.visit(parent));
            }
            index => {
                let path = (*wizard.folder.visible().get(index - 2)?).clone();
                return Some(wizard.folder.visit(path));
            }
        }
        None
    }
}

fn step_index(index: usize, down: bool, last: usize) -> usize {
    if down {
        index.saturating_add(1).min(last)
    } else {
        index.saturating_sub(1)
    }
}

fn network_key(entry: &NetEntry) -> NetworkKey {
    (entry.sandbox.clone(), entry.host.clone(), entry.port)
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

    fn entry(sandbox: &str, host: &str, allowed: bool) -> NetEntry {
        NetEntry {
            sandbox: sandbox.to_owned(),
            host: host.to_owned(),
            port: 443,
            allowed,
            count: 1,
            first_seen: "2026-10-29 12:00:00".to_owned(),
            last_seen: "2026-10-29 12:00:00".to_owned(),
            allowed_count: u64::from(allowed),
            blocked_count: u64::from(!allowed),
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
    fn dashboard_tab_changes_focus_without_changing_detail_tab() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn dashboard_horizontal_keys_do_not_move_network_rows() {
        let mut app = app();
        app.tab = Tab::Network;
        press(&mut app, KeyCode::Right);
        assert_eq!(app.network_row, 0);
    }

    #[test]
    fn dashboard_create_requires_configuration_before_submission() {
        let mut app = app();
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.popup.is_some());
    }

    #[test]
    fn dashboard_repeat_cannot_submit_or_quit() {
        let mut app = app();
        let mut repeat = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(app.update(repeat), None);
    }

    #[test]
    fn dashboard_control_letters_do_not_become_plain_commands() {
        let mut app = app();
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT);
        assert_eq!(app.update(key), None);
    }

    #[test]
    fn git_changes_follow_the_selected_sandbox() {
        let mut app = app();
        assert_eq!(app.selected_git_changes(), [] as [String; 0]);
        app.git_changes
            .insert(name("b"), vec![name(".git/hooks/pre-commit (new)")]);
        assert_eq!(app.selected_git_changes(), [] as [String; 0]);
        app.select_named("b");
        assert_eq!(
            app.selected_git_changes(),
            [name(".git/hooks/pre-commit (new)")]
        );
        app.selected = app.sandboxes.len();
        assert_eq!(app.selected_git_changes(), [] as [String; 0]);
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
    fn wizard_mut(app: &mut App) -> anyhow::Result<&mut CreateWizard> {
        match &mut app.popup {
            Some(Popup::Create(wizard)) => Ok(wizard),
            _ => anyhow::bail!("expected create wizard"),
        }
    }

    #[test]
    fn focus_routes_vertical_navigation_and_brackets_cycle_tabs() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Detail);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected, 0);
        assert_eq!(
            press(&mut app, KeyCode::Char(']')),
            Some(Action::LoadLogs { name: name("a") })
        );
        assert_eq!(app.tab, Tab::Logs);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.network_row, 1);
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.tab, Tab::Logs);
        app.logs = vec![name("old")];
        assert_eq!(
            press(&mut app, KeyCode::Down),
            Some(Action::LoadLogs { name: name("c") })
        );
        assert_eq!(app.logs, Vec::<String>::new());
    }

    #[test]
    fn network_selection_and_expansion_follow_identity_across_refresh() -> anyhow::Result<()> {
        let mut app = app();
        app.tab = Tab::Network;
        app.focus = Focus::Detail;
        app.network_row = 1;
        press(&mut app, KeyCode::Enter);
        app.set_network(
            Some(Preset::Balanced),
            vec![
                entry("a", "api.openai.com", true),
                entry("a", "new.test", false),
            ],
        );
        assert_eq!(app.network_row, 0);
        assert!(
            app.is_expanded(
                app.selected_network()
                    .ok_or_else(|| anyhow::anyhow!("selected row"))?
            )
        );
        app.set_network(Some(Preset::Balanced), vec![]);
        assert_eq!(app.network_row, 0);
        assert!(app.expanded.is_none());
        Ok(())
    }

    #[test]
    fn full_network_search_filters_scope_and_actions_target_row_sandbox() {
        let mut app = app();
        press(&mut app, KeyCode::Char('n'));
        assert!(app.full_network);
        press(&mut app, KeyCode::Char('/'));
        app.update_paste("elsewhere");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.network_rows().len(), 1);
        assert_eq!(
            press(&mut app, KeyCode::Char('a')),
            Some(Action::Allow {
                target: name("elsewhere.test"),
                sandbox: Some(name("b"))
            })
        );
        assert_eq!(press(&mut app, KeyCode::Char('b')), None);
        assert_eq!(
            press(&mut app, KeyCode::Char('A')),
            Some(Action::Allow {
                target: name("elsewhere.test"),
                sandbox: None
            })
        );
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.network_rows(), Vec::<&NetEntry>::new());
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.network_rows().len(), 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.full_network);
        assert_eq!(app.network_search.text(), "");
        press(&mut app, KeyCode::Esc);
        assert!(!app.full_network);
    }

    #[test]
    fn search_owns_command_letters_and_escape_restores_previous_query() {
        let mut app = app();
        press(&mut app, KeyCode::Char('n'));
        app.network_search.set_text("old");
        press(&mut app, KeyCode::Char('/'));
        for character in "qarsbn".chars() {
            assert_eq!(press(&mut app, KeyCode::Char(character)), None);
        }
        assert!(app.searching);
        assert_eq!(app.network_search.text(), "oldqarsbn");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.network_search.text(), "old");
        assert!(app.full_network);
    }

    #[test]
    fn sandbox_filter_includes_history_and_rules_only_names() {
        let mut app = app();
        app.network.push(entry("z-old", "old.test", true));
        app.set_rules(vec![RuleInfo {
            target: name("x.test"),
            source: Source::Sandbox,
            sandbox: Some(name("y-rules")),
            allowed: true,
        }]);
        press(&mut app, KeyCode::Char('n'));
        let mut choices = Vec::new();
        for _ in 0..6 {
            press(&mut app, KeyCode::Char('s'));
            choices.push(app.sandbox_filter.clone());
        }
        assert_eq!(
            choices,
            vec![
                Some(name("a")),
                Some(name("b")),
                Some(name("c")),
                Some(name("y-rules")),
                Some(name("z-old")),
                None
            ]
        );
    }

    #[test]
    fn wizard_edits_custom_name_and_preserves_it_when_going_back() -> anyhow::Result<()> {
        let mut app = app();
        app.open_create();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(wizard_mut(&mut app)?.name.text(), "pi-api");
        app.update(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        app.update_paste("my-sandbox");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(wizard_mut(&mut app)?.name.text(), "my-sandbox");
        wizard_mut(&mut app)?.field = CreateField::Create;
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Create {
                agent: name("shell"),
                project: PathBuf::from("/work/api"),
                name: name("my-sandbox")
            })
        );
        assert!(app.popup.is_none());
        Ok(())
    }

    #[test]
    fn invalid_or_existing_name_keeps_configuration_open() -> anyhow::Result<()> {
        let mut app = app();
        for value in ["", "BAD NAME", "a"] {
            app.open_create();
            let wizard = wizard_mut(&mut app)?;
            wizard.stage = CreateStage::Configure;
            wizard.field = CreateField::Create;
            wizard.name.set_text(value);
            assert_eq!(press(&mut app, KeyCode::Enter), None);
            assert!(wizard_mut(&mut app)?.error.is_some());
        }
        Ok(())
    }

    #[test]
    fn folder_results_are_correlated_and_default_name_tracks_chosen_folder() -> anyhow::Result<()> {
        let mut app = app();
        app.open_create();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::LoadFolders {
                path: PathBuf::from("/work/api")
            })
        );
        app.set_folders(Path::new("/stale"), Ok(vec![PathBuf::from("/stale/no")]));
        assert_eq!(
            wizard_mut(&mut app)?.folder.directories,
            Vec::<PathBuf>::new()
        );
        app.set_folders(
            Path::new("/work/api"),
            Ok(vec![
                PathBuf::from("/work/api/docs"),
                PathBuf::from("/work/api/src"),
            ]),
        );
        app.update_paste("src");
        assert_eq!(wizard_mut(&mut app)?.folder.visible().len(), 1);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::LoadFolders {
                path: PathBuf::from("/work/api/src")
            })
        );
        app.set_folders(Path::new("/work/api/src"), Ok(Vec::new()));
        press(&mut app, KeyCode::Enter);
        let wizard = wizard_mut(&mut app)?;
        assert_eq!(wizard.stage, CreateStage::Configure);
        assert_eq!(wizard.field, CreateField::Create);
        assert_eq!(wizard.name.text(), "codex-src");
        assert_eq!(wizard.project, PathBuf::from("/work/api/src"));
        Ok(())
    }

    #[test]
    fn failed_folder_cannot_be_selected_but_parent_remains_reachable() -> anyhow::Result<()> {
        let mut app = app();
        app.open_create();
        let wizard = wizard_mut(&mut app)?;
        wizard.stage = CreateStage::Folder;
        app.set_folders(Path::new("/work/api"), Err(name("permission denied")));
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert_eq!(wizard_mut(&mut app)?.stage, CreateStage::Folder);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::LoadFolders {
                path: PathBuf::from("/work")
            })
        );
        Ok(())
    }

    #[test]
    fn rule_removal_captures_scope_and_preset_is_read_only() {
        let mut app = app();
        app.focus = Focus::Detail;
        app.tab = Tab::Rules;
        let global = RuleInfo {
            target: name("example.com"),
            source: Source::Global,
            sandbox: None,
            allowed: true,
        };
        app.set_rules(vec![
            global.clone(),
            RuleInfo {
                source: Source::Preset,
                ..global
            },
        ]);
        press(&mut app, KeyCode::Char('r'));
        app.rule_row = 1;
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.popup.is_some());
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            Some(Action::RemoveRule {
                target: name("example.com"),
                sandbox: None
            })
        );
        press(&mut app, KeyCode::Char('r'));
        assert!(app.popup.is_none());
        assert!(
            app.status_line
                .as_deref()
                .is_some_and(|text| text.contains("read-only"))
        );
    }

    #[test]
    fn an_added_rule_is_selected_once_the_rules_reload() {
        let mut app = app();
        app.focus = Focus::Detail;
        app.tab = Tab::Rules;
        let preset = RuleInfo {
            target: name("pypi.org"),
            source: Source::Preset,
            sandbox: None,
            allowed: true,
        };
        app.set_rules(vec![preset.clone()]);
        press(&mut app, KeyCode::Char('a'));
        app.update_paste("example.com");
        assert!(press(&mut app, KeyCode::Enter).is_some());
        let added = RuleInfo {
            target: name("example.com"),
            source: Source::Sandbox,
            sandbox: Some(name("a")),
            allowed: true,
        };
        app.set_rules(vec![added.clone(), preset.clone()]);
        assert_eq!(app.rule_rows().get(app.rule_row), Some(&&added));
        assert_eq!(app.pending_rule, None, "used once");
        app.rule_row = 1;
        app.set_rules(vec![added, preset.clone()]);
        assert_eq!(
            app.rule_rows().get(app.rule_row),
            Some(&&preset),
            "later reloads keep the cursor"
        );
    }

    #[test]
    fn add_rule_validates_and_preserves_scope_for_allow_and_block() {
        let mut app = app();
        app.focus = Focus::Detail;
        app.tab = Tab::Rules;
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(matches!(
            app.popup,
            Some(Popup::AddRule { error: Some(_), .. })
        ));
        app.update_paste("EXAMPLE.COM:443");
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Deny {
                target: name("example.com:443"),
                sandbox: Some(name("a"))
            })
        );
        press(&mut app, KeyCode::Char('n'));
        app.tab = Tab::Rules;
        press(&mut app, KeyCode::Char('a'));
        app.update_paste("test.org");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Allow {
                target: name("test.org"),
                sandbox: None
            })
        );
    }

    #[test]
    fn dashboard_modified_enter_cannot_submit_forms() -> anyhow::Result<()> {
        let mut app = app();
        app.open_create();
        press(&mut app, KeyCode::Enter);
        wizard_mut(&mut app)?.field = CreateField::Create;
        assert_eq!(
            app.update(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
            None
        );
        assert!(app.popup.is_some());
        Ok(())
    }
    #[test]
    fn disappearing_selection_clears_logs_from_previous_sandbox() {
        let mut app = app();
        app.logs = vec![name("a private output")];
        app.detail_scroll = 4;
        app.set_sandboxes(vec![sandbox("b", Status::Running)]);
        assert_eq!(
            app.selected_sandbox().map(|sandbox| sandbox.name.as_str()),
            Some("b")
        );
        assert_eq!(app.logs, Vec::<String>::new());
        assert_eq!(app.detail_scroll, 0);
    }

    fn ctrl(app: &mut App, letter: char) -> Option<Action> {
        app.update(KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL))
    }

    fn settings(app: &App) -> Option<&SettingsForm> {
        match &app.popup {
            Some(Popup::Settings(form)) => Some(form),
            _ => None,
        }
    }

    #[test]
    fn i_opens_info_for_the_selected_card_and_closes_with_esc_i_or_q() {
        let mut app = app();
        app.select_named("b");
        for close in [KeyCode::Esc, KeyCode::Char('i'), KeyCode::Char('q')] {
            assert_eq!(press(&mut app, KeyCode::Char('i')), None);
            assert_eq!(app.popup, Some(Popup::Info { name: name("b") }));
            assert_eq!(press(&mut app, close), None, "closing never quits");
            assert_eq!(app.popup, None);
        }
        app.focus = Focus::Detail;
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(
            app.popup,
            Some(Popup::Info { name: name("b") }),
            "the selected card's info opens from the detail pane too"
        );
    }

    #[test]
    fn info_scrolls_within_the_measured_limit_and_owns_its_keys() {
        let mut app = app();
        press(&mut app, KeyCode::Char('i'));
        app.set_popup_scroll_limit(3);
        for _ in 0..5 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.popup_scroll, 3);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.popup_scroll, 2, "one step back from the bottom");
        assert_eq!(
            press(&mut app, KeyCode::Char('x')),
            None,
            "no shell from info"
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('s')),
            None,
            "no stop from info"
        );
        assert_eq!(app.popup, Some(Popup::Info { name: name("a") }));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(app.popup_scroll, 0, "reopening starts at the top");
    }

    #[test]
    fn info_needs_a_sandbox_card_and_the_split_view() {
        let mut app = app();
        app.selected = app.sandboxes.len();
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(app.popup, None, "+ new sandbox has no info");
        app.selected = 0;
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(
            app.popup, None,
            "the full network view has no selected card"
        );
    }

    #[test]
    fn ctrl_s_opens_settings_with_the_current_preset_and_ports() {
        let mut app = app();
        app.set_network(Some(Preset::LockedDown), Vec::new());
        app.host_ports = Some(vec![1234, 8080]);
        assert_eq!(ctrl(&mut app, 's'), None);
        let form = settings(&app).cloned();
        assert_eq!(form.as_ref().map(|form| form.preset), Some(2));
        assert_eq!(
            form.as_ref().map(|form| form.host_ports.text().to_owned()),
            Some(name("1234, 8080"))
        );
        assert_eq!(form.map(|form| form.field), Some(SettingsField::Preset));
        press(&mut app, KeyCode::Right);
        assert_eq!(settings(&app).map(|form| form.preset), Some(2), "clamps");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(settings(&app).map(|form| form.preset), Some(1));
        assert_eq!(
            press(&mut app, KeyCode::Char('q')),
            None,
            "q does not quit a form"
        );
        assert!(settings(&app).is_some());
    }

    #[test]
    fn settings_validate_ports_inline_and_save_both_fields() {
        let mut app = app();
        ctrl(&mut app, 's');
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            settings(&app).map(|form| form.field),
            Some(SettingsField::HostPorts)
        );
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        let error = settings(&app).and_then(|form| form.error.clone());
        assert!(
            error
                .as_deref()
                .is_some_and(|error| error.contains("\"11434x\"")),
            "{error:?}"
        );
        press(&mut app, KeyCode::Backspace);
        assert_eq!(
            settings(&app).and_then(|form| form.error.clone()),
            None,
            "editing clears the error"
        );
        ctrl(&mut app, 'u');
        app.update_paste("8080 9000");
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::SaveSettings {
                preset: Preset::LockedDown,
                host_ports: vec![8080, 9000],
            })
        );
        assert_eq!(app.popup, None);
    }

    #[test]
    fn settings_without_a_policy_start_from_the_defaults_and_esc_cancels() {
        let mut app = app();
        app.set_network(None, Vec::new());
        app.host_ports = None;
        ctrl(&mut app, 's');
        let form = settings(&app).cloned();
        assert_eq!(form.as_ref().map(|form| form.preset), Some(1));
        assert_eq!(
            form.map(|form| form.host_ports.text().to_owned()),
            Some(name("1234, 11434"))
        );
        press(&mut app, KeyCode::Tab);
        ctrl(&mut app, 'u');
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::SaveSettings {
                preset: Preset::Balanced,
                host_ports: Vec::new(),
            }),
            "an empty list is valid: no Mac ports"
        );
        ctrl(&mut app, 's');
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert_eq!(app.popup, None);
    }

    #[test]
    fn detail_scroll_clamps_to_display_rows_and_reverses_at_the_bottom() {
        let mut app = app();
        app.focus = Focus::Detail;
        app.tab = Tab::Logs;
        app.logs = vec!["long line".repeat(100)];
        app.set_detail_scroll_limit(12);
        for _ in 0..20 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.detail_scroll, 12);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.detail_scroll, 11);
        app.set_detail_scroll_limit(2);
        assert_eq!(app.detail_scroll, 2);
    }
}
