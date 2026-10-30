//! One binding inventory for dispatch, contextual footers and help.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, CreateField, CreateStage, Focus, Popup, SettingsField, Tab};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Quit,
    Help,
    FocusNext,
    FocusCards,
    FocusDetail,
    Previous,
    Next,
    PreviousTab,
    NextTab,
    Enter,
    Create,
    Shell,
    Toggle,
    Remove,
    Network,
    Preset,
    Info,
    Settings,
    Allow,
    AllowAll,
    Block,
    Search,
    Status,
    Sandbox,
    Back,
    Confirm,
    Edit,
    PageUp,
    PageDown,
}

#[derive(Clone, Copy)]
enum Scope {
    Main,
    Split,
    Cards,
    Rows,
    Traffic,
    Rules,
    Full,
    FullTraffic,
    Expanded,
    Agent,
    Configure,
    Name,
    Folder,
    AddRule,
    Confirm,
    Preset,
    Help,
    Info,
    Settings,
    SettingsPreset,
    SettingsPorts,
    Search,
}

struct Binding {
    codes: &'static [KeyCode],
    key: &'static str,
    description: &'static str,
    hint: &'static str,
    command: Command,
    scope: Scope,
    control: bool,
}

macro_rules! binding {
    ($scope:ident, $command:ident, $key:literal, $description:literal; $hint:literal; $($code:expr),+ $(,)?) => {
        Binding { codes: &[$($code),+], key: $key, description: $description, hint: $hint,
            command: Command::$command, scope: Scope::$scope, control: false }
    };
    ($scope:ident, $command:ident, $key:literal, $description:literal, $($code:expr),+ $(,)?) => {
        Binding { codes: &[$($code),+], key: $key, description: $description, hint: $description,
            command: Command::$command, scope: Scope::$scope, control: false }
    };
}

use KeyCode::{BackTab, Char, Down, Enter, Esc, Left, Right, Tab as TabKey, Up};

static BINDINGS: &[Binding] = &[
    binding!(Main, Help, "?", "help", Char('?')),
    binding!(Main, Quit, "q", "quit", Char('q')),
    Binding {
        codes: &[Char('c')],
        key: "Ctrl-C",
        description: "quit from any screen",
        hint: "quit",
        command: Command::Quit,
        scope: Scope::Main,
        control: true,
    },
    Binding {
        codes: &[Char('h')],
        key: "Ctrl-H",
        description: "help",
        hint: "help",
        command: Command::Help,
        scope: Scope::Main,
        control: true,
    },
    binding!(Split, FocusNext, "Tab/⇧Tab", "panel", TabKey, BackTab),
    binding!(Split, FocusCards, "←/h", "cards", Left, Char('h')),
    binding!(Split, FocusDetail, "→/l", "detail", Right, Char('l')),
    binding!(Rows, Previous, "↑/k", "previous", Up, Char('k')),
    binding!(Rows, Next, "↓/j", "next", Down, Char('j')),
    binding!(Main, PreviousTab, "[", "previous tab", Char('[')),
    binding!(Main, NextTab, "]", "next tab", Char(']')),
    binding!(Cards, Enter, "Enter", "attach/create"; "attach/new"; Enter),
    binding!(Expanded, PageUp, "PgUp", "details up", KeyCode::PageUp),
    binding!(
        Expanded,
        PageDown,
        "PgDn",
        "details down",
        KeyCode::PageDown
    ),
    binding!(Traffic, Enter, "Enter", "expand/collapse"; "details"; Enter),
    binding!(Main, Create, "c", "create", Char('c')),
    binding!(Main, Network, "n", "full network/back"; "network"; Char('n')),
    binding!(Main, Preset, "p", "preset", Char('p')),
    binding!(Split, Info, "i", "sandbox info"; "info"; Char('i')),
    Binding {
        codes: &[Char('s')],
        key: "Ctrl-S",
        description: "settings: default preset and host ports",
        hint: "settings",
        command: Command::Settings,
        scope: Scope::Main,
        control: true,
    },
    binding!(Cards, Shell, "x", "shell", Char('x')),
    binding!(Cards, Toggle, "s", "start/stop", Char('s')),
    binding!(Cards, Remove, "r", "remove sandbox"; "remove"; Char('r')),
    binding!(Traffic, Allow, "a", "allow blocked host"; "allow"; Char('a')),
    binding!(Traffic, Block, "b", "block allowed host"; "block"; Char('b')),
    binding!(Traffic, AllowAll, "A", "allow everywhere"; "allow all"; Char('A')),
    binding!(Rules, Allow, "a", "add rule", Char('a')),
    binding!(Rules, Remove, "r", "remove rule", Char('r')),
    binding!(Full, Search, "/", "search", Char('/')),
    binding!(Full, Sandbox, "s", "sandbox filter"; "sandbox"; Char('s')),
    binding!(FullTraffic, Status, "o", "status filter"; "status"; Char('o')),
    binding!(Main, Back, "Esc", "back/clear", Esc),
    binding!(Agent, Back, "Esc", "cancel", Esc),
    binding!(Agent, Previous, "↑/k", "previous agent"; "previous"; Up, Char('k')),
    binding!(Agent, Next, "↓/j", "next agent"; "next"; Down, Char('j')),
    binding!(Agent, Enter, "Enter", "configure", Enter),
    binding!(Configure, Back, "Esc", "back", Esc),
    binding!(Configure, FocusNext, "Tab/⇧Tab", "field", TabKey, BackTab),
    binding!(Configure, Enter, "Enter", "choose", Enter),
    binding!(Name, Edit, "type", "edit name · Ctrl-U clear", Char('\0')),
    binding!(Folder, Back, "Esc", "back", Esc),
    binding!(Folder, Previous, "↑/↓", "folder", Up, Down),
    binding!(Folder, Enter, "Enter", "choose", Enter),
    binding!(Folder, Edit, "type", "filter · Ctrl-U clear", Char('\0')),
    binding!(AddRule, Back, "Esc", "cancel", Esc),
    binding!(AddRule, FocusNext, "Tab", "allow/block", TabKey),
    binding!(AddRule, Enter, "Enter", "add rule", Enter),
    binding!(AddRule, Edit, "type", "target · Ctrl-U clear", Char('\0')),
    binding!(Confirm, Back, "n/Esc", "cancel", Char('n'), Esc),
    binding!(Confirm, Confirm, "y", "confirm removal"; "remove"; Char('y')),
    binding!(Preset, Back, "Esc", "cancel", Esc),
    binding!(Preset, Previous, "↑/↓", "preset", Up, Down),
    binding!(Preset, Enter, "Enter", "select", Enter),
    binding!(
        Help,
        Back,
        "Esc/?/q",
        "close help",
        Esc,
        Char('?'),
        Char('q')
    ),
    binding!(Help, Previous, "↑/k", "scroll up", Up, Char('k')),
    binding!(Help, Next, "↓/j", "scroll down", Down, Char('j')),
    binding!(Info, Back, "Esc/i/q", "close info"; "close"; Esc, Char('i'), Char('q')),
    binding!(Info, Previous, "↑/k", "scroll up", Up, Char('k')),
    binding!(Info, Next, "↓/j", "scroll down", Down, Char('j')),
    binding!(Settings, Back, "Esc", "cancel", Esc),
    binding!(Settings, FocusNext, "Tab/⇧Tab", "field", TabKey, BackTab),
    binding!(Settings, Enter, "Enter", "save", Enter),
    binding!(
        SettingsPreset,
        Previous,
        "↑↓←→",
        "preset",
        Up,
        Down,
        Left,
        Right
    ),
    binding!(
        SettingsPorts,
        Edit,
        "type",
        "ports · Ctrl-U clear",
        Char('\0')
    ),
    binding!(Search, Back, "Esc", "cancel search", Esc),
    binding!(Search, Enter, "Enter", "apply search", Enter),
    binding!(Search, Edit, "type", "search · Ctrl-U clear", Char('\0')),
];

fn active(scope: Scope, app: &App, main: bool) -> bool {
    if !main {
        return match (&app.popup, scope) {
            (Some(Popup::Create(wizard)), Scope::Agent) => wizard.stage == CreateStage::Agent,
            (Some(Popup::Create(wizard)), Scope::Configure) => {
                wizard.stage == CreateStage::Configure
            }
            (Some(Popup::Create(wizard)), Scope::Name) => {
                wizard.stage == CreateStage::Configure && wizard.field == CreateField::Name
            }
            (Some(Popup::Create(wizard)), Scope::Folder) => wizard.stage == CreateStage::Folder,
            (Some(Popup::AddRule { .. }), Scope::AddRule)
            | (
                Some(Popup::ConfirmRemove { .. } | Popup::ConfirmRuleRemove { .. }),
                Scope::Confirm,
            )
            | (Some(Popup::Preset { .. }), Scope::Preset)
            | (Some(Popup::Help), Scope::Help)
            | (Some(Popup::Info { .. }), Scope::Info)
            | (Some(Popup::Settings(_)), Scope::Settings) => true,
            (Some(Popup::Settings(form)), Scope::SettingsPreset) => {
                form.field == SettingsField::Preset
            }
            (Some(Popup::Settings(form)), Scope::SettingsPorts) => {
                form.field == SettingsField::HostPorts
            }
            (None, Scope::Search) => app.searching,
            _ => false,
        };
    }
    let detail = app.full_network || app.focus == Focus::Detail;
    match scope {
        Scope::Main | Scope::Rows => true,
        Scope::Split => !app.full_network,
        Scope::Cards => !app.full_network && app.focus == Focus::Cards,
        Scope::Traffic => detail && app.tab == Tab::Network,
        Scope::Rules => detail && app.tab == Tab::Rules,
        Scope::Full => app.full_network,
        Scope::FullTraffic => app.full_network && app.tab == Tab::Network,
        Scope::Expanded => detail && app.tab == Tab::Network && app.network_is_expanded(),
        _ => false,
    }
}

pub(crate) fn command(app: &App, key: KeyEvent) -> Option<Command> {
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    BINDINGS
        .iter()
        .find(|binding| {
            active(binding.scope, app, true)
                && binding.codes.contains(&key.code)
                && binding.control == key.modifiers.contains(KeyModifiers::CONTROL)
        })
        .map(|binding| binding.command)
}

/// Footer hint standing in for the split view's movement keys, which help lists one by one.
const NAVIGATE: (&str, &str) = ("↑↓←→/Tab", "navigate");
/// Footer hint standing in for `[` and `]`.
const TABS: (&str, &str) = ("[ ]", "tabs");

pub(crate) fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let main = app.popup.is_none() && !app.searching;
    let split = main && !app.full_network;
    let mut bindings: Vec<_> = BINDINGS
        .iter()
        .filter(|binding| active(binding.scope, app, main) && !binding.control)
        .filter(|binding| {
            !(split
                && matches!(
                    binding.command,
                    Command::FocusNext
                        | Command::FocusCards
                        | Command::FocusDetail
                        | Command::Previous
                        | Command::Next
                        | Command::PreviousTab
                        | Command::NextTab
                ))
        })
        .collect();
    bindings.sort_by_key(|binding| match binding.command {
        Command::Help | Command::FocusNext | Command::Search => 0,
        Command::Enter
        | Command::Confirm
        | Command::Allow
        | Command::Block
        | Command::Status
        | Command::Sandbox => 1,
        Command::Previous | Command::Next if !main => 2,
        Command::Create | Command::Remove | Command::PageUp | Command::PageDown => 2,
        _ => 3,
    });
    let hints = bindings
        .into_iter()
        .map(|binding| (binding.key, binding.hint));
    if split {
        [NAVIGATE, TABS].into_iter().chain(hints).collect()
    } else {
        hints.collect()
    }
}

pub(crate) fn help(app: &App) -> Vec<(&'static str, &'static str)> {
    BINDINGS
        .iter()
        .filter(|binding| active(binding.scope, app, true))
        .map(|binding| (binding.key, binding.description))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn advertised_main_bindings_dispatch_without_context_collisions() {
        let mut app = App::new(PathBuf::from("/work"), None, vec!["shell"], |agent, _| {
            agent.to_owned()
        });
        for full in [false, true] {
            for focus in [Focus::Cards, Focus::Detail] {
                for tab in [Tab::Overview, Tab::Logs, Tab::Network, Tab::Rules] {
                    app.full_network = full;
                    app.focus = focus;
                    app.tab = tab;
                    let active: Vec<_> = BINDINGS
                        .iter()
                        .filter(|binding| active(binding.scope, &app, true))
                        .collect();
                    for binding in &active {
                        for code in binding.codes {
                            let modifiers = if binding.control {
                                KeyModifiers::CONTROL
                            } else {
                                KeyModifiers::NONE
                            };
                            assert_eq!(
                                command(&app, KeyEvent::new(*code, modifiers)),
                                Some(binding.command),
                                "{}: {focus:?} {tab:?} full={full}",
                                binding.key
                            );
                            assert_eq!(
                                active
                                    .iter()
                                    .filter(|candidate| candidate.control == binding.control
                                        && candidate.codes.contains(code))
                                    .count(),
                                1
                            );
                        }
                    }
                }
            }
        }
    }
}
