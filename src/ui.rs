//! Pure dashboard rendering: state enters as data; no IO or terminal lifecycle lives here.

use std::path::Path;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Wrap,
};

use crate::app::{
    App, CreateField, CreateStage, CreateWizard, Focus, Popup, SettingsField, SettingsForm, Tab,
};
use crate::input::TextInput;
use crate::keymap;
use crate::model::{NetEntry, RuleInfo, SandboxInfo, Status};
use crate::policy::{self, Preset, Source};

const ACCENT: Color = Color::Cyan;
const CARDS_WIDTH: u16 = 34;
const CARD_HEIGHT: u16 = 7;
const NEW_CARD_HEIGHT: u16 = 3;
const LABEL_WIDTH: usize = 11;

pub(crate) fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 40 || area.height < 12 {
        frame.render_widget(
            Paragraph::new("Resize to at least 40x12\nq / Ctrl-C quit"),
            area,
        );
    } else {
        render_dashboard(frame, app);
    }
    if app.no_color {
        frame
            .buffer_mut()
            .set_style(area, Style::new().fg(Color::Reset).bg(Color::Reset));
    }
}

fn dashboard_regions(area: Rect, status: bool) -> [Rect; 5] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(status)),
        Constraint::Length(1),
    ])
    .areas(area)
}

fn panel_regions(area: Rect) -> [Rect; 3] {
    Layout::horizontal([
        Constraint::Length(CARDS_WIDTH),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .areas(area)
}

fn pane_regions(inner: Rect) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner)
}

/// Pure geometry shared with the IO loop, so wrapping and screen size bound state scrolling.
pub(crate) fn detail_scroll_limit(app: &App, width: u16, height: u16) -> usize {
    if width < 40 || height < 12 {
        return 0;
    }
    let [_, _, body, _, _] =
        dashboard_regions(Rect::new(0, 0, width, height), app.status_line.is_some());
    let panel = if !app.full_network && width >= 100 {
        let [_, _, detail] = panel_regions(body);
        detail
    } else {
        body
    };
    let [_, _, _, content] = pane_regions(Block::bordered().inner(panel));
    let mut visible = content.height;
    let lines = match app.tab {
        Tab::Overview => app.selected_sandbox().map_or_else(Vec::new, |sandbox| {
            overview(sandbox, app.policy, app.selected_git_changes())
        }),
        Tab::Logs => app.logs.iter().cloned().map(Line::from).collect(),
        Tab::Network => {
            let Some(entry) = app
                .selected_network()
                .filter(|entry| app.is_expanded(entry))
            else {
                return 0;
            };
            let lines = connection_details(entry, content.width);
            if lines.len() > usize::from(visible) {
                visible = visible.saturating_sub(1);
            }
            lines
        }
        Tab::Rules => return 0,
    };
    wrap_lines(lines, content.width)
        .len()
        .saturating_sub(usize::from(visible))
}

/// Measure the help or info viewport with the same popup and row layout used by rendering.
pub(crate) fn popup_scroll_limit(app: &App, width: u16, height: u16) -> usize {
    if width < 40 || height < 12 {
        return 0;
    }
    let area = popup_bounds(Rect::new(0, 0, width, height), 78, height.saturating_sub(2));
    let inner = Block::bordered()
        .padding(Padding::horizontal(1))
        .inner(area);
    let [rows, _] = help_regions(inner);
    let total = match &app.popup {
        Some(Popup::Help) => keymap::help(app).len(),
        Some(Popup::Info { name }) => wrap_lines(info_lines(app, name), rows.width).len(),
        _ => return 0,
    };
    total.saturating_sub(usize::from(rows.height).max(1))
}

fn render_dashboard(frame: &mut Frame, app: &App) {
    let [header, _, body, status, footer] =
        dashboard_regions(frame.area(), app.status_line.is_some());
    render_header(frame, header);
    if app.full_network {
        render_full_network(frame, app, body);
    } else if body.width < 100 {
        match app.focus {
            Focus::Cards => render_cards(frame, app, body),
            Focus::Detail => render_detail(frame, app, body),
        }
    } else {
        let [cards, _, detail] = panel_regions(body);
        render_cards(frame, app, cards);
        render_detail(frame, app, detail);
    }
    if let Some(message) = &app.status_line {
        frame.render_widget(Paragraph::new(message.as_str()).yellow(), status);
    }
    render_footer(frame, app, footer);
    render_overlay(frame, app);
}

fn render_header(frame: &mut Frame, area: Rect) {
    let left = vec![
        Line::from("msbx".bold()),
        Line::from("   microVM sandboxes for coding agents".dim()),
    ];
    let right = Line::from(vec!["?".bold(), " help".dim()]);
    frame.render_widget(spread(left, right, area.width), area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = keymap::hints(app);
    let recovery = if app.popup.is_some() || app.searching {
        hints.iter().find(|(key, _)| key.contains("Esc")).copied()
    } else {
        hints.iter().find(|(key, _)| *key == "?").copied()
    };
    let right = recovery.map_or_else(Line::default, |(key, label)| {
        Line::from(format!("{key} {label}"))
    });
    let chunks = hints
        .iter()
        .filter(|hint| Some(**hint) != recovery)
        .map(|(key, label)| Line::from(vec![(*key).bold(), format!(" {label}  ").dim()]))
        .collect();
    frame.render_widget(spread(chunks, right, area.width), area);
}

/// Keep complete hint chunks and reserve the right-hand recovery action.
fn spread<'a>(left: Vec<Line<'a>>, right: Line<'a>, width: u16) -> Line<'a> {
    let right_width = right.width();
    let gap = usize::from(right_width > 0) * 2;
    let left_room = usize::from(width).saturating_sub(right_width + gap);
    let mut spans = Vec::new();
    let mut used = 0;
    for chunk in left {
        if used + chunk.width() > left_room {
            break;
        }
        used += chunk.width();
        spans.extend(chunk.spans);
    }
    spans.push(
        " ".repeat(usize::from(width).saturating_sub(used + right_width))
            .into(),
    );
    spans.extend(right.spans);
    Line::from(spans)
}

fn pane(title: &str, focused: bool) -> Block<'static> {
    let marker = if focused { ">" } else { " " };
    Block::bordered()
        .title(format!(" {marker} {title} "))
        .border_style(Style::new().fg(if focused { ACCENT } else { Color::DarkGray }))
}

fn render_cards(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane("Sandboxes", app.focus == Focus::Cards);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut y = inner.y;
    for index in first_visible_card(app, inner.height)..=app.sandboxes.len() {
        let height = card_height(app, index);
        if y.saturating_add(height) > inner.bottom() {
            break;
        }
        let card = Rect::new(inner.x, y, inner.width, height);
        if let Some(sandbox) = app.sandboxes.get(index) {
            render_card(frame, app, sandbox, index == app.selected, card);
        } else {
            render_new_card(frame, index == app.selected, card);
        }
        y += height;
    }
    if app.sandboxes.is_empty() {
        let hint = Rect::new(inner.x, y, inner.width, inner.bottom().saturating_sub(y));
        frame.render_widget(
            Paragraph::new("No sandboxes yet.\nPress c or Enter to choose an agent and workspace.")
                .wrap(Wrap { trim: false })
                .dim(),
            hint,
        );
    }
}

fn card_height(app: &App, index: usize) -> u16 {
    if index < app.sandboxes.len() {
        CARD_HEIGHT
    } else {
        NEW_CARD_HEIGHT
    }
}

fn first_visible_card(app: &App, height: u16) -> usize {
    let mut first = 0;
    while first < app.selected {
        let needed: usize = (first..=app.selected)
            .map(|index| usize::from(card_height(app, index)))
            .sum();
        if needed <= usize::from(height) {
            break;
        }
        first += 1;
    }
    first
}

fn card_block(selected: bool) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(if selected { ACCENT } else { Color::DarkGray }))
        .padding(Padding::horizontal(1))
}

fn render_card(frame: &mut Frame, app: &App, sandbox: &SandboxInfo, selected: bool, area: Rect) {
    let block = card_block(selected);
    let width = block.inner(area).width;
    let (symbol, colour) = status_symbol(sandbox.status);
    let agent = Line::from(sandbox.agent.clone().unwrap_or_else(|| "—".into()));
    let hint = if selected && app.focus == Focus::Cards {
        Line::from("↵ attach".dim())
    } else {
        Line::default()
    };
    let project = sandbox.project.as_deref().map_or_else(
        || "—".into(),
        |path| shorten_home(path, app.home.as_deref()),
    );
    let marker = if selected { "▸ " } else { "" };
    // Git files of the project changed since the last session (RFC 0001).
    let badge = if app
        .git_changes
        .get(&sandbox.name)
        .is_some_and(|changes| !changes.is_empty())
    {
        Line::from("git!".fg(Color::Red).bold())
    } else {
        Line::default()
    };
    let lines = vec![
        spread(
            vec![Line::from(vec![
                marker.into(),
                symbol.fg(colour),
                " ".into(),
                sandbox.name.clone().bold(),
            ])],
            badge,
            width,
        ),
        spread(vec![agent], hint, width),
        Line::from(project.dim()),
        card_metrics(sandbox),
        action_row(sandbox.status),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_new_card(frame: &mut Frame, selected: bool, area: Rect) {
    let block = card_block(selected).border_type(BorderType::LightDoubleDashed);
    let marker = if selected { "▸ " } else { "" };
    frame.render_widget(
        Paragraph::new(format!("{marker}+ new sandbox  (c)"))
            .centered()
            .block(block),
        area,
    );
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane("Details", app.focus == Focus::Detail);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(sandbox) = app.selected_sandbox() else {
        let hint = if app.sandboxes.is_empty() {
            "No sandboxes yet.\nPress c to create one."
        } else {
            "Press Enter or c to create a sandbox,\nor ↑ to choose one."
        };
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: false }).dim(), inner);
        return;
    };
    let [title, tabs, _, content] = pane_regions(inner);
    frame.render_widget(Line::from(sandbox.name.clone().bold()), title);
    frame.render_widget(tab_line(app.tab, false), tabs);
    match app.tab {
        Tab::Overview => render_scrolled(
            frame,
            content,
            overview(sandbox, app.policy, app.selected_git_changes()),
            app.detail_scroll,
        ),
        Tab::Logs => render_logs(frame, app, content),
        Tab::Network => render_network(frame, app, content),
        Tab::Rules => render_rules(frame, app, content),
    }
}

fn tab_line(active: Tab, full: bool) -> Line<'static> {
    let tabs = [
        ("Overview", Tab::Overview),
        ("Logs", Tab::Logs),
        ("Network", Tab::Network),
        ("Rules", Tab::Rules),
    ];
    let mut spans = Vec::new();
    for (label, tab) in tabs {
        if full && matches!(tab, Tab::Overview | Tab::Logs) {
            continue;
        }
        if tab == active {
            spans.push(format!("[{label}]").fg(ACCENT).bold());
        } else {
            spans.push(format!(" {label} ").dim());
        }
        spans.push(" ".into());
    }
    spans.push(" [/] tabs".dim());
    Line::from(spans)
}

fn render_scrolled(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>, scroll: usize) {
    let lines = wrap_lines(lines, area.width);
    let last = lines.len().saturating_sub(usize::from(area.height));
    let offset = u16::try_from(scroll.min(last)).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), area);
}

/// Wrap before scrolling so the cursor measures displayed rows, including long paths/hosts.
fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for line in lines {
        let mut current = Line::default().style(line.style);
        let mut used = 0;
        for span in line.spans {
            let mut chunk = String::new();
            for character in span.content.chars() {
                let size = Line::from(character.to_string()).width();
                if used + size > usize::from(width) {
                    current
                        .spans
                        .push(Span::styled(std::mem::take(&mut chunk), span.style));
                    output.push(current);
                    current = Line::default().style(line.style);
                    used = 0;
                }
                chunk.push(character);
                used += size;
            }
            current.spans.push(Span::styled(chunk, span.style));
        }
        output.push(current);
    }
    output
}

fn render_logs(frame: &mut Frame, app: &App, area: Rect) {
    if app.logs.is_empty() {
        frame.render_widget(
            Paragraph::new("No logs yet.\nRun the agent from its card to produce output.")
                .dim()
                .wrap(Wrap { trim: false }),
            area,
        );
    } else {
        render_scrolled(
            frame,
            area,
            app.logs.iter().cloned().map(Line::from).collect(),
            app.detail_scroll,
        );
    }
}

fn render_full_network(frame: &mut Frame, app: &App, area: Rect) {
    let block = pane("Network · Esc back", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [tabs, filters, search, content] = pane_regions(inner);
    frame.render_widget(tab_line(app.tab, true), tabs);
    let scope = app.sandbox_filter.as_deref().unwrap_or("All sandboxes");
    frame.render_widget(
        Line::from(if app.tab == Tab::Rules {
            format!("s {scope}")
        } else {
            format!("s {scope}   o {}", app.status_filter.label())
        }),
        filters,
    );
    if app.searching {
        render_input(frame, search, &app.network_search, "/ ", true);
    } else {
        let query = app.network_search.text();
        frame.render_widget(
            Paragraph::new(if query.is_empty() {
                "/ search".to_owned()
            } else {
                format!("/ {query} · Esc clears")
            })
            .dim(),
            search,
        );
    }
    if app.tab == Tab::Rules {
        render_rules(frame, app, content);
    } else {
        render_network(frame, app, content);
    }
}

fn render_network(frame: &mut Frame, app: &App, area: Rect) {
    if !app.full_network
        && let Some(sandbox) = app.selected_sandbox()
        && !sandbox.proxied
    {
        let message = if sandbox.agent.is_some() {
            "Created before network policy. Recreate this sandbox to route traffic through the msbx proxy."
        } else {
            "This sandbox was not created by msbx. Its traffic is not visible to the msbx proxy."
        };
        frame.render_widget(
            Paragraph::new(message).dim().wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let rows = app.network_rows();
    if rows.is_empty() {
        let message = if app.network_search.text().is_empty()
            && app.sandbox_filter.is_none()
            && app.status_filter.label().eq_ignore_ascii_case("all")
        {
            "No connections yet.\nRun an agent and make a network request to see traffic here."
        } else {
            "No matching connections.\nEsc clears search; o and s change filters."
        };
        frame.render_widget(
            Paragraph::new(message).dim().wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if let Some(entry) = app
        .selected_network()
        .filter(|entry| app.is_expanded(entry))
    {
        render_expanded_network(frame, app, area, &rows, entry);
    } else {
        let [summary, table] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let blocked = rows.iter().filter(|entry| !entry.allowed).count();
        frame.render_widget(
            Line::from(format!(
                "{} endpoints · {} allowed · {blocked} blocked",
                rows.len(),
                rows.len() - blocked
            ))
            .dim(),
            summary,
        );
        render_network_table(frame, app, table, &rows);
    }
}

fn render_network_table(frame: &mut Frame, app: &App, area: Rect, entries: &[&NetEntry]) {
    let full = app.full_network;
    let sandbox_width = if area.width >= 75 { 16 } else { 10 };
    let fixed_width = 31 + if full { sandbox_width + 1 } else { 0 };
    let host_width = area.width.saturating_sub(fixed_width);
    let mut widths = vec![
        Constraint::Length(8),
        Constraint::Min(10),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(7),
    ];
    let mut labels = vec!["Time", "Host", "Port", "Hits", "Status"];
    if full {
        widths.insert(2, Constraint::Length(sandbox_width));
        labels.insert(2, "Sandbox");
    }
    let rows = entries.iter().map(|entry| {
        let seen = entry
            .last_seen
            .rsplit(' ')
            .next()
            .unwrap_or(&entry.last_seen);
        let mut cells = vec![
            Cell::from(seen),
            Cell::from(truncate_text(&entry.host, host_width)),
            Cell::from(entry.port.to_string()),
            Cell::from(truncate_text(&entry.count.to_string(), 5)),
            Cell::from(verdict(entry.allowed)).style(verdict_style(entry.allowed)),
        ];
        if full {
            cells.insert(2, Cell::from(truncate_text(&entry.sandbox, sandbox_width)));
        }
        Row::new(cells)
    });
    let table = Table::new(rows, widths)
        .header(Row::new(labels).bold())
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");
    let visible = usize::from(area.height.saturating_sub(1)).max(1);
    let selected = app.network_row.min(entries.len().saturating_sub(1));
    let mut state = TableState::default()
        .with_selected(Some(selected))
        .with_offset(selected.saturating_sub(visible - 1));
    frame.render_stateful_widget(table, area, &mut state);
}

fn verdict(allowed: bool) -> &'static str {
    if allowed { "Allowed" } else { "Blocked" }
}
fn verdict_style(allowed: bool) -> Style {
    Style::new().fg(if allowed { Color::Green } else { Color::Red })
}

fn connection_details(entry: &NetEntry, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!("{} · port {}", verdict(entry.allowed), entry.port))
            .style(verdict_style(entry.allowed))
            .bold(),
    ];
    for (label, value) in [
        ("Host", entry.host.clone()),
        ("Sandbox", entry.sandbox.clone()),
        ("First seen", entry.first_seen.clone()),
        ("Last seen", entry.last_seen.clone()),
        ("Hits", entry.count.to_string()),
        ("Allowed hits", entry.allowed_count.to_string()),
        ("Blocked hits", entry.blocked_count.to_string()),
    ] {
        lines.extend(wrapped_field(label, &value, width));
    }
    lines
}

fn render_expanded_network(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    entries: &[&NetEntry],
    entry: &NetEntry,
) {
    let lines = connection_details(entry, area.width);
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    if height.saturating_add(3) <= area.height {
        let [table, _, details] = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length(1),
            Constraint::Length(height),
        ])
        .areas(area);
        render_network_table(frame, app, table, entries);
        render_scrolled(frame, details, lines, app.detail_scroll);
    } else if height > area.height {
        let [details, hint] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        render_scrolled(frame, details, lines, app.detail_scroll);
        let keys = keymap::hints(app)
            .iter()
            .filter(|(key, _)| key.starts_with("Pg") || *key == "Enter")
            .map(|(key, label)| format!("{key} {label}"))
            .collect::<Vec<_>>()
            .join(" · ");
        frame.render_widget(Paragraph::new(keys).dim(), hint);
    } else {
        render_scrolled(frame, area, lines, app.detail_scroll);
    }
}

fn wrapped_field(label: &str, value: &str, width: u16) -> Vec<Line<'static>> {
    let prefix = format!("{label}: ");
    let indent = Line::from(prefix.as_str())
        .width()
        .min(usize::from(width.saturating_sub(1)));
    let room = usize::from(width).saturating_sub(indent).max(1);
    let mut lines = Vec::new();
    let mut chunk = String::new();
    for character in value.chars() {
        if !chunk.is_empty() && Line::from(format!("{chunk}{character}")).width() > room {
            let start = if lines.is_empty() {
                prefix.clone()
            } else {
                " ".repeat(indent)
            };
            lines.push(Line::from(format!("{start}{chunk}")));
            chunk.clear();
        }
        chunk.push(character);
    }
    let start = if lines.is_empty() {
        prefix
    } else {
        " ".repeat(indent)
    };
    lines.push(Line::from(format!("{start}{chunk}")));
    lines
}

fn render_rules(frame: &mut Frame, app: &App, area: Rect) {
    let rules = app.rule_rows();
    if rules.is_empty() {
        frame.render_widget(Paragraph::new("No rules here.\nPress a to add a rule, p to choose a preset.\nIn a filtered view, Esc clears search.").dim().wrap(Wrap { trim: false }), area);
        return;
    }
    let [table_area, hint] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);
    let rows = rules.iter().map(|rule| {
        Row::new([
            Cell::from(rule.source.name()),
            Cell::from(truncate_text(rule.sandbox.as_deref().unwrap_or("all"), 11)),
            Cell::from(truncate_text(
                &rule.target,
                table_area.width.saturating_sub(30),
            )),
            Cell::from(verdict(rule.allowed)).style(verdict_style(rule.allowed)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Min(10),
            Constraint::Length(7),
        ],
    )
    .header(Row::new(["Source", "Scope", "Target", "Status"]).bold())
    .column_spacing(1)
    .row_highlight_style(Style::new().add_modifier(Modifier::BOLD))
    .highlight_symbol("▸ ");
    let visible = usize::from(table_area.height.saturating_sub(1)).max(1);
    let selected = app.rule_row.min(rules.len().saturating_sub(1));
    let mut state = TableState::default()
        .with_selected(Some(selected))
        .with_offset(selected.saturating_sub(visible - 1));
    frame.render_stateful_widget(table, table_area, &mut state);
    let note = rules.get(selected).map_or("", |rule| {
        if rule.source == Source::Preset {
            "Preset rule · read-only · p changes preset"
        } else {
            "a add rule · r remove selected rule"
        }
    });
    frame.render_widget(Paragraph::new(note).dim().wrap(Wrap { trim: false }), hint);
}

fn render_overlay(frame: &mut Frame, app: &App) {
    match &app.popup {
        Some(Popup::Create(wizard)) => render_create(frame, app, wizard),
        Some(Popup::ConfirmRemove { name }) => {
            render_confirmation(
                frame,
                app,
                "Remove sandbox",
                &format!("Remove sandbox {name}?"),
            );
        }
        Some(Popup::Help) => render_help(frame, app),
        Some(Popup::Info { name }) => render_info(frame, app, name),
        Some(Popup::Settings(form)) => render_settings(frame, app, form),
        Some(Popup::Preset { index, then_create }) => {
            render_preset(frame, app, *index, *then_create);
        }
        Some(Popup::AddRule {
            target,
            allowed,
            sandbox,
            error,
        }) => render_rule_input(
            frame,
            app,
            target,
            *allowed,
            sandbox.as_deref(),
            error.as_deref(),
        ),
        Some(Popup::ConfirmRuleRemove { rule }) => render_rule_confirmation(frame, app, rule),
        None => {}
    }
}

fn popup_bounds(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(Constraint::Max(width), Constraint::Max(height))
}

fn popup_area(frame: &mut Frame, title: &str, width: u16, height: u16) -> Rect {
    let area = popup_bounds(frame.area(), width, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(" {title} ").bold())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn render_create(frame: &mut Frame, app: &App, wizard: &CreateWizard) {
    match wizard.stage {
        CreateStage::Agent => render_agents(frame, app, wizard.agent_index),
        CreateStage::Configure => render_configuration(frame, app, wizard),
        CreateStage::Folder => render_folders(frame, app, wizard),
    }
}

fn render_agents(frame: &mut Frame, app: &App, selected: usize) {
    let inner = popup_area(frame, "New sandbox · 1/2 Choose agent", 78, 17);
    let [intro, choices, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Choose an agent. Credentials are checked on the host.")
            .wrap(Wrap { trim: false })
            .dim(),
        intro,
    );
    let mut lines = Vec::new();
    let mut selected_end = 0;
    for (index, id) in app.agents.iter().enumerate() {
        let description = crate::agents::find(id).map_or("", |agent| agent.description);
        let marker = if index == selected { "▸" } else { " " };
        let mut title = Line::from(truncate_text(
            &format!("{marker} {id} · {description}"),
            choices.width,
        ));
        if index == selected {
            title = title.fg(ACCENT).bold();
        }
        lines.push(title);
        let credential = app
            .credentials
            .get(index)
            .map_or("checking credentials…", String::as_str);
        lines.extend(credential_lines(credential, choices.width, 2));
        if index == selected {
            selected_end = lines.len();
        }
        lines.push(Line::default());
    }
    let visible = usize::from(choices.height);
    let start = selected_end.saturating_sub(visible);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(visible)
                .collect::<Vec<_>>(),
        ),
        choices,
    );
    render_footer(frame, app, hint);
}

fn credential_lines(credential: &str, width: u16, maximum: usize) -> Vec<Line<'static>> {
    let source = credential
        .split("; ")
        .map(|part| Line::from(part.to_owned()).dim())
        .collect();
    let mut lines = wrap_lines(source, width);
    if lines.len() > maximum {
        lines.truncate(maximum);
        if let Some(last) = lines.last_mut() {
            *last = Line::from(truncate_text(&format!("{last}…"), width)).dim();
        }
    }
    lines
}

fn truncate_text(text: &str, width: u16) -> String {
    if Line::from(text).width() <= usize::from(width) {
        return text.to_owned();
    }
    let mut output = String::new();
    let room = usize::from(width.saturating_sub(1));
    for character in text.chars() {
        if Line::from(format!("{output}{character}")).width() > room {
            break;
        }
        output.push(character);
    }
    if width > 0 {
        output.push('…');
    }
    output
}

fn render_configuration(frame: &mut Frame, app: &App, wizard: &CreateWizard) {
    let agent = app
        .agents
        .get(wizard.agent_index)
        .copied()
        .unwrap_or("agent");
    let inner = popup_area(frame, &format!("Configure {agent} · 2/2"), 78, 18);
    let credential = app
        .credentials
        .get(wizard.agent_index)
        .map_or("checking credentials…", String::as_str);
    let credential_lines = credential_lines(credential, inner.width, 4);
    let credential_height = u16::try_from(credential_lines.len())
        .unwrap_or(4)
        .clamp(2, 4);
    let [
        credentials,
        name_label,
        name,
        _,
        workspace,
        project,
        warning,
        error,
        buttons,
        hint,
    ] = Layout::vertical([
        Constraint::Length(credential_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(credential_lines), credentials);
    frame.render_widget(
        Line::from(field_label("Name", wizard.field == CreateField::Name)),
        name_label,
    );
    render_input(
        frame,
        name,
        &wizard.name,
        "",
        wizard.field == CreateField::Name,
    );
    frame.render_widget(
        Line::from(field_label(
            "Workspace · Enter chooses folder",
            wizard.field == CreateField::Workspace,
        )),
        workspace,
    );
    frame.render_widget(
        Paragraph::new(shorten_home(&wizard.project, app.home.as_deref()))
            .wrap(Wrap { trim: false }),
        project,
    );
    frame.render_widget(
        Paragraph::new("The agent can edit files in this folder.").dim(),
        warning,
    );
    if let Some(message) = &wizard.error {
        frame.render_widget(
            Paragraph::new(message.as_str())
                .red()
                .wrap(Wrap { trim: false }),
            error,
        );
    }
    let back = if wizard.field == CreateField::Back {
        "[Back]"
    } else {
        " Back "
    };
    let create = if wizard.field == CreateField::Create {
        "[Create]"
    } else {
        " Create "
    };
    frame.render_widget(Line::from(format!("{back}    {create}")).bold(), buttons);
    render_footer(frame, app, hint);
}

fn field_label(label: &str, focused: bool) -> Span<'static> {
    let marker = if focused { "▸" } else { " " };
    let text = format!("{marker} {label}");
    if focused {
        text.fg(ACCENT).bold()
    } else {
        text.into()
    }
}

fn render_input(frame: &mut Frame, area: Rect, input: &TextInput, prefix: &str, focused: bool) {
    let prefix_width = u16::try_from(Line::from(prefix).width()).unwrap_or(u16::MAX);
    let width = area.width.saturating_sub(prefix_width);
    let (text, cursor) = input.visible(width);
    frame.render_widget(Paragraph::new(format!("{prefix}{text}")).underlined(), area);
    if focused && width > 0 && area.height > 0 {
        frame.set_cursor_position((
            area.x
                .saturating_add(prefix_width)
                .saturating_add(cursor.min(width - 1)),
            area.y,
        ));
    }
}

fn render_folders(frame: &mut Frame, app: &App, wizard: &CreateWizard) {
    let inner = popup_area(frame, "Workspace folder", 78, 19);
    let [path, filter, list, status, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(wizard.folder.path.display().to_string())
            .wrap(Wrap { trim: false })
            .bold(),
        path,
    );
    render_input(
        frame,
        filter,
        &wizard.folder.filter,
        "Filter folders > ",
        true,
    );
    let directories = wizard.folder.visible();
    let mut choices = vec!["Create here · use this folder".to_owned(), "..".to_owned()];
    choices.extend(directories.iter().map(|path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }));
    let visible = usize::from(list.height).max(1);
    let start = wizard.folder.row.saturating_sub(visible - 1);
    let lines = choices
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, label)| Line::from(field_label(label, index == wizard.folder.row)))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), list);
    let message = if let Some(error) = &wizard.folder.error {
        error.as_str()
    } else if wizard.folder.loading {
        "Loading folders…"
    } else if directories.is_empty() && !wizard.folder.filter.text().is_empty() {
        "No matching folders. Ctrl-U clears the filter."
    } else if directories.is_empty() {
        "No child folders. Choose Create here or .."
    } else {
        "Type to filter · only folders are shown"
    };
    frame.render_widget(
        Paragraph::new(message).dim().wrap(Wrap { trim: false }),
        status,
    );
    render_footer(frame, app, hint);
}

fn render_confirmation(frame: &mut Frame, app: &App, title: &str, message: &str) {
    let width = frame.area().width.min(78);
    let lines = wrap_lines(
        message
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect(),
        width.saturating_sub(4),
    );
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4)
        .max(8);
    let inner = popup_area(
        frame,
        title,
        width,
        height.min(frame.area().height.saturating_sub(2)),
    );
    let [body, _, actions] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(lines), body);
    render_footer(frame, app, actions);
}

fn render_rule_confirmation(frame: &mut Frame, app: &App, rule: &RuleInfo) {
    render_confirmation(
        frame,
        app,
        "Remove network rule",
        &format!(
            "Remove {} rule {}?\nScope: {} ({})\nFuture connections use the remaining rules.",
            verdict(rule.allowed),
            rule.target,
            rule.sandbox.as_deref().unwrap_or("all sandboxes"),
            rule.source.name()
        ),
    );
}

fn render_rule_input(
    frame: &mut Frame,
    app: &App,
    target: &TextInput,
    allowed: bool,
    sandbox: Option<&str>,
    error: Option<&str>,
) {
    let inner = popup_area(frame, "Add network rule", 72, 13);
    let [scope, kind, label, input, explanation, error_area, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!(
            "Scope: {}",
            sandbox.unwrap_or("all sandboxes (global)")
        ))
        .wrap(Wrap { trim: false }),
        scope,
    );
    frame.render_widget(
        Line::from(format!(
            "Action: {} · Tab changes",
            if allowed { "Allow" } else { "Block" }
        ))
        .style(verdict_style(allowed)),
        kind,
    );
    frame.render_widget(
        Paragraph::new("Host, wildcard, IP or CIDR; optional :port").dim(),
        label,
    );
    render_input(frame, input, target, "> ", true);
    frame.render_widget(Paragraph::new("Examples: api.example.com:443 or *.example.com\nDeny rules still take precedence over allows.").dim().wrap(Wrap { trim: false }), explanation);
    if let Some(message) = error {
        frame.render_widget(
            Paragraph::new(message).red().wrap(Wrap { trim: false }),
            error_area,
        );
    }
    render_footer(frame, app, hint);
}

fn help_regions(inner: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner)
}

fn render_help(frame: &mut Frame, app: &App) {
    let hints = keymap::help(app);
    let inner = popup_area(
        frame,
        "Keys · current view",
        78,
        frame.area().height.saturating_sub(2),
    );
    let lines = hints
        .iter()
        .map(|(key, label)| Line::from(vec![format!("{key:<17}").bold(), (*label).into()]))
        .collect();
    render_scrolled_popup(frame, app, inner, lines);
}

/// Rows from `app.popup_scroll`, clamped so the last page stays full, then the popup's keys and
/// the visible range.
fn render_scrolled_popup(frame: &mut Frame, app: &App, inner: Rect, lines: Vec<Line<'static>>) {
    let [rows, footer] = help_regions(inner);
    let visible = usize::from(rows.height).max(1);
    let total = lines.len();
    let start = app.popup_scroll.min(total.saturating_sub(visible));
    let shown: Vec<Line<'static>> = lines.into_iter().skip(start).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), rows);
    let end = (start + visible).min(total);
    let keys = keymap::hints(app)
        .iter()
        .map(|(key, label)| Line::from(format!("{key} {label}  ")))
        .collect();
    frame.render_widget(
        spread(
            keys,
            Line::from(format!("{}–{end}/{total}", start + 1)),
            footer.width,
        ),
        footer,
    );
}

fn render_info(frame: &mut Frame, app: &App, name: &str) {
    let inner = popup_area(
        frame,
        &format!("Info · {name}"),
        78,
        frame.area().height.saturating_sub(2),
    );
    let [rows, _] = help_regions(inner);
    render_scrolled_popup(
        frame,
        app,
        inner,
        wrap_lines(info_lines(app, name), rows.width),
    );
}

/// Everything known about one sandbox, by section. Fields the runtime does not report show `—`.
fn info_lines(app: &App, name: &str) -> Vec<Line<'static>> {
    let Some(sandbox) = app.sandboxes.iter().find(|sandbox| sandbox.name == name) else {
        return vec![Line::from(format!("{name} is no longer listed."))];
    };
    let cpus = sandbox
        .vcpus
        .map_or_else(|| "—".to_owned(), |count| format!("{count} vCPU"));
    let cpus = match sandbox.cpu_percent {
        Some(percent) => format!("{cpus} · {percent:.0}% busy"),
        None => cpus,
    };
    let mut lines = vec![
        heading("Identity"),
        row("name", sandbox.name.clone()),
        row("agent", or_dash(sandbox.agent.clone())),
        row(
            "project",
            or_dash(
                sandbox
                    .project
                    .as_ref()
                    .map(|path| path.display().to_string()),
            ),
        ),
        row("status", status_word(sandbox.status).to_owned()),
        row("created", or_dash(sandbox.created.clone())),
        row("image", or_dash(sandbox.image.clone())),
        Line::default(),
        heading("Resources"),
        row("cpus", cpus),
        row("memory", memory(sandbox, " / ")),
        row("disk", disk(sandbox)),
        row("uptime", or_dash(sandbox.uptime.map(format_duration))),
        row("traffic", or_dash(traffic(sandbox))),
        Line::default(),
        heading("Security"),
        row("user", or_dash(sandbox.user.clone())),
        row(
            "profile",
            match sandbox.restricted {
                Some(true) => "restricted".to_owned(),
                Some(false) => "default".to_owned(),
                None => "—".to_owned(),
            },
        ),
        row(
            "git guard",
            if sandbox.git_guarded { "yes" } else { "no" }.to_owned(),
        ),
        Line::default(),
        heading("Network"),
    ];
    lines.extend(network_info(app, sandbox));
    lines.push(Line::default());
    lines.push(heading("Mounts and secrets"));
    let mounts = if sandbox.mounts.is_empty() {
        vec!["—".to_owned()]
    } else {
        sandbox.mounts.clone()
    };
    for (index, mount) in mounts.into_iter().enumerate() {
        lines.push(row(if index == 0 { "mounts" } else { "" }, mount));
    }
    let secrets = if sandbox.secrets.is_empty() {
        "none".to_owned()
    } else {
        format!("{} (names only)", sandbox.secrets.join(", "))
    };
    lines.push(row("secrets", secrets));
    lines
}

/// Proxy, preset and the host ports applied at create next to the ones configured now.
fn network_info(app: &App, sandbox: &SandboxInfo) -> Vec<Line<'static>> {
    if !sandbox.proxied {
        return vec![
            row(
                "proxy",
                format!("no · msb policy {}", or_dash(sandbox.network.clone())),
            ),
            row("preset", "— (policy.toml does not apply)".to_owned()),
        ];
    }
    let rules = app
        .rules
        .iter()
        .filter(|rule| rule.sandbox.as_deref() == Some(sandbox.name.as_str()))
        .count();
    let mut lines = vec![
        row("proxy", "yes · policy.toml decides per host".to_owned()),
        row("preset", preset_word(app.policy).to_owned()),
    ];
    match (&sandbox.host_ports, &app.host_ports) {
        (None, _) => lines.push(row("host ports", "unknown".to_owned())),
        (Some(applied), configured) => {
            lines.push(row(
                "host ports",
                format!("{} (set at create)", policy::port_list(applied)),
            ));
            match configured {
                Some(now) if now != applied => {
                    lines.push(
                        row(
                            "configured",
                            format!("{} · differs", policy::port_list(now)),
                        )
                        .fg(Color::Yellow),
                    );
                    lines.push(
                        row("", format!("recreate to apply: msb rm -f {}", sandbox.name))
                            .fg(Color::Yellow),
                    );
                }
                Some(_) => lines.push(row("configured", "the same".to_owned())),
                None => lines.push(row("configured", "— (no policy.toml)".to_owned())),
            }
        }
    }
    lines.push(row("rules", format!("{rules} for this sandbox only")));
    lines
}

fn heading(title: &str) -> Line<'static> {
    Line::from(title.to_owned().fg(ACCENT).bold())
}

fn disk(sandbox: &SandboxInfo) -> String {
    match (sandbox.disk_used_bytes, sandbox.disk_free_bytes) {
        (None, None) => "—".to_owned(),
        (used, free) => format!(
            "{} used · {} free",
            or_dash(used.map(format_bytes)),
            or_dash(free.map(format_bytes))
        ),
    }
}

fn render_settings(frame: &mut Frame, app: &App, form: &SettingsForm) {
    let inner = popup_area(frame, "Settings", 72, 16);
    let [
        intro,
        preset_label,
        presets,
        describe,
        _,
        ports_label,
        input,
        note,
        error_area,
        hint,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let intro_text = if app.policy.is_none() {
        "policy.toml does not exist yet; saving creates it."
    } else {
        "Defaults for msbx sandboxes, saved in policy.toml."
    };
    frame.render_widget(
        Paragraph::new(intro_text).dim().wrap(Wrap { trim: false }),
        intro,
    );
    let on_preset = form.field == SettingsField::Preset;
    frame.render_widget(
        Line::from(field_label("Default network preset", on_preset)),
        preset_label,
    );
    let mut choices = vec![Span::from("  ")];
    for (index, preset) in Preset::ALL.iter().enumerate() {
        choices.push(if index == form.preset {
            format!("[{}]", preset.title()).fg(ACCENT).bold()
        } else {
            format!(" {} ", preset.title()).into()
        });
        choices.push("  ".into());
    }
    frame.render_widget(Line::from(choices), presets);
    let chosen = Preset::ALL
        .get(form.preset)
        .copied()
        .unwrap_or(Preset::Balanced);
    let [_, describe] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(describe);
    frame.render_widget(
        Paragraph::new(chosen.describe())
            .dim()
            .wrap(Wrap { trim: false }),
        describe,
    );
    let on_ports = form.field == SettingsField::HostPorts;
    frame.render_widget(
        Line::from(field_label(
            "Host ports on your Mac (host.microsandbox.internal)",
            on_ports,
        )),
        ports_label,
    );
    render_input(frame, input, &form.host_ports, "> ", on_ports);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Comma or space separated; empty allows none."),
            Line::from("Applies to new sandboxes; recreate existing ones."),
        ])
        .dim()
        .wrap(Wrap { trim: false }),
        note,
    );
    if let Some(message) = &form.error {
        frame.render_widget(
            Paragraph::new(message.as_str())
                .red()
                .wrap(Wrap { trim: false }),
            error_area,
        );
    }
    render_footer(frame, app, hint);
}

fn render_preset(frame: &mut Frame, app: &App, selected: usize, then_create: bool) {
    let inner = popup_area(frame, "Network policy", 78, 14);
    let [intro, choices, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let text = if then_create {
        "Choose once for all sandboxes; p changes it later."
    } else {
        "Preset rules apply to all sandboxes."
    };
    frame.render_widget(Paragraph::new(text).dim().wrap(Wrap { trim: false }), intro);
    let lines = Preset::ALL
        .iter()
        .enumerate()
        .flat_map(|(index, preset)| {
            [
                Line::from(field_label(preset.title(), index == selected)),
                Line::from(format!("  {}", preset.describe())).dim(),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), choices);
    render_footer(frame, app, hint);
}

fn card_metrics(sandbox: &SandboxInfo) -> Line<'static> {
    if sandbox.status != Status::Running {
        let (_, colour) = status_symbol(sandbox.status);
        return Line::from(status_word(sandbox.status).fg(colour));
    }
    let cpu = sandbox.cpu_percent.map_or_else(
        || "—".to_owned(),
        |percent| format!("{} {percent:.0}%", bar(percent, 6)),
    );
    Line::from(vec![
        "cpu ".dim(),
        cpu.into(),
        "  mem ".dim(),
        memory(sandbox, "/").into(),
    ])
}

fn action_row(status: Status) -> Line<'static> {
    let toggle = match status {
        Status::Stopped | Status::Crashed => "tart   ",
        _ => "top    ",
    };
    Line::from(vec![
        "s".fg(ACCENT).bold(),
        toggle.dim(),
        "e".dim(),
        "x".fg(ACCENT).bold(),
        "ec    ".dim(),
        "r".fg(ACCENT).bold(),
        "emove".dim(),
    ])
}

fn status_symbol(status: Status) -> (&'static str, Color) {
    match status {
        Status::Running => ("●", Color::Green),
        Status::Starting => ("◌", Color::Cyan),
        Status::Stopped => ("■", Color::Yellow),
        Status::Crashed => ("✕", Color::Red),
        Status::Other => ("?", Color::DarkGray),
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Starting => "starting",
        Status::Stopped => "stopped",
        Status::Crashed => "crashed",
        Status::Other => "unknown",
    }
}

fn overview(
    sandbox: &SandboxInfo,
    policy: Option<Preset>,
    git_changes: &[String],
) -> Vec<Line<'static>> {
    let word = status_word(sandbox.status);
    let status = match sandbox.uptime {
        Some(uptime) => format!("{word} · up {}", format_duration(uptime)),
        None => word.to_owned(),
    };
    let vcpus = sandbox
        .vcpus
        .map_or_else(|| "—".to_owned(), |count| format!("{count} vCPU"));
    let cpu = match sandbox.cpu_percent {
        Some(percent) => format!("{vcpus}   {} {percent:.0}%", bar(percent, 10)),
        None => vcpus,
    };
    let secrets = if sandbox.secrets.is_empty() {
        "none".to_owned()
    } else {
        format!("{} (host-side)", sandbox.secrets.join(", "))
    };
    let network = if sandbox.proxied {
        format!("policy {} (proxied)", preset_word(policy))
    } else {
        or_dash(sandbox.network.clone())
    };
    let mut lines = vec![
        row("name", sandbox.name.clone()),
        row("agent", or_dash(sandbox.agent.clone())),
        row(
            "workspace",
            sandbox
                .project
                .as_ref()
                .map_or_else(|| "—".into(), |path| path.display().to_string()),
        ),
        row("status", status),
        row("cpu", cpu),
        row("memory", memory(sandbox, " / ")),
        row("network", network),
    ];
    if let Some(traffic) = traffic(sandbox) {
        lines.push(row("traffic", traffic));
    }
    lines.extend(git_rows(sandbox, git_changes));
    lines.push(row("secrets", secrets));
    let mounts = if sandbox.mounts.is_empty() {
        vec!["—".to_owned()]
    } else {
        sandbox.mounts.clone()
    };
    for (index, mount) in mounts.into_iter().enumerate() {
        lines.push(row(if index == 0 { "mounts" } else { "" }, mount));
    }
    lines.push(row("image", or_dash(sandbox.image.clone())));
    lines
}

/// The `git` row: whether hooks and config are guarded in the VM, then what changed on the host
/// since the last session started, for sandboxes msbx created (RFC 0001).
fn git_rows(sandbox: &SandboxInfo, git_changes: &[String]) -> Vec<Line<'static>> {
    if sandbox.agent.is_none() {
        return Vec::new();
    }
    let state = if sandbox.git_guarded {
        "guarded".to_owned()
    } else {
        format!("not guarded; recreate: msb rm -f {}", sandbox.name)
    };
    let mut lines = vec![row("git", state)];
    if !git_changes.is_empty() {
        lines.push(Line::from(vec![
            format!("{:LABEL_WIDTH$}", "").dim(),
            "changed since last session; review first:"
                .fg(Color::Red)
                .bold(),
        ]));
        lines.extend(
            git_changes
                .iter()
                .map(|change| row("", format!("  {change}")).fg(Color::Red)),
        );
    }
    lines
}

fn preset_word(policy: Option<Preset>) -> &'static str {
    policy.map_or("not initialised", Preset::name)
}

/// `↓ 48.2M  ↑ 3.1M` from the runtime's metrics, when it reports them.
fn traffic(sandbox: &SandboxInfo) -> Option<String> {
    match (sandbox.rx_bytes, sandbox.tx_bytes) {
        (None, None) => None,
        (rx, tx) => Some(format!(
            "↓ {}  ↑ {}",
            or_dash(rx.map(format_bytes)),
            or_dash(tx.map(format_bytes))
        )),
    }
}

fn row(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        format!("{label:LABEL_WIDTH$}").dim(),
        Span::from(value),
    ])
}

fn or_dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_owned())
}

fn memory(sandbox: &SandboxInfo, separator: &str) -> String {
    let used = sandbox.memory_bytes.map(format_bytes);
    let limit = sandbox.memory_limit_bytes.map(format_bytes);
    match (used, limit) {
        (None, None) => "—".to_owned(),
        (used, limit) => format!("{}{separator}{}", or_dash(used), or_dash(limit)),
    }
}

fn bar(percent: f64, width: u16) -> String {
    let filled = (0..width)
        .filter(|cell| (f64::from(*cell) + 0.5) * 100.0 / f64::from(width) < percent)
        .count();
    format!(
        "{}{}",
        "▮".repeat(filled),
        "▯".repeat(usize::from(width) - filled)
    )
}

fn shorten_home(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Binary units with one decimal below 10, e.g. `2.1G`, `32G`, `512B`.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut unit = 0;
    let mut scale: u128 = 1;
    while unit + 1 < UNITS.len() && u128::from(bytes) >= scale * 1024 {
        scale *= 1024;
        unit += 1;
    }
    let tenths = (u128::from(bytes) * 10 + scale / 2) / scale;
    if unit == 0 || tenths >= 100 {
        format!("{}{}", (tenths + 5) / 10, UNITS[unit])
    } else {
        format!("{}.{}{}", tenths / 10, tenths % 10, UNITS[unit])
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::{CreateField, CreateStage, Focus};
    use crate::input::TextInput;
    use crate::model::RuleInfo;
    use crate::policy::Source;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1536), "1.5K");
        assert_eq!(format_bytes(GIB * 21 / 10), "2.1G");
        assert_eq!(format_bytes(GIB * 32), "32G");
        assert_eq!(format_bytes(GIB * 1024 * 3), "3.0T");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(12 * 60 + 30)), "12m");
        assert_eq!(format_duration(Duration::from_secs(3 * 3_600)), "3h");
        assert_eq!(format_duration(Duration::from_secs(2 * 86_400 + 5)), "2d");
    }

    #[test]
    fn bar_fills_proportionally_and_clamps() {
        assert_eq!(bar(0.0, 6), "▯▯▯▯▯▯");
        assert_eq!(bar(18.0, 10), "▮▮▯▯▯▯▯▯▯▯");
        assert_eq!(bar(100.0, 6), "▮▮▮▮▮▮");
        assert_eq!(bar(250.0, 4), "▮▮▮▮");
    }

    #[test]
    fn every_status_has_a_distinct_symbol_and_word() {
        let statuses = [
            Status::Running,
            Status::Starting,
            Status::Stopped,
            Status::Crashed,
            Status::Other,
        ];
        let symbols: std::collections::HashSet<_> = statuses
            .iter()
            .map(|status| status_symbol(*status).0)
            .collect();
        let words: std::collections::HashSet<_> =
            statuses.iter().map(|status| status_word(*status)).collect();
        assert_eq!(symbols.len(), statuses.len());
        assert_eq!(words.len(), statuses.len());
    }

    #[test]
    fn shortens_home_prefix() {
        let home = Path::new("/home/dev");
        assert_eq!(
            shorten_home(Path::new("/home/dev/api"), Some(home)),
            "~/api"
        );
        assert_eq!(shorten_home(home, Some(home)), "~");
        assert_eq!(shorten_home(Path::new("/srv/api"), Some(home)), "/srv/api");
        assert_eq!(shorten_home(Path::new("/srv/api"), None), "/srv/api");
    }

    #[test]
    fn traffic_needs_at_least_one_counter() {
        let mut sandbox = info("x", Status::Running);
        assert_eq!(traffic(&sandbox), None);
        sandbox.rx_bytes = Some(1536);
        assert_eq!(traffic(&sandbox).as_deref(), Some("↓ 1.5K  ↑ —"));
        sandbox.tx_bytes = Some(0);
        assert_eq!(traffic(&sandbox).as_deref(), Some("↓ 1.5K  ↑ 0B"));
    }

    fn info(name: &str, status: Status) -> SandboxInfo {
        SandboxInfo {
            name: name.to_owned(),
            status,
            agent: None,
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
            proxied: false,
            host_ports: None,
            user: None,
            restricted: None,
            git_guarded: false,
            rx_bytes: None,
            tx_bytes: None,
            secrets: Vec::new(),
            mounts: Vec::new(),
        }
    }

    fn entry(
        sandbox: &str,
        host: &str,
        port: u16,
        allowed: bool,
        count: u64,
        seen: &str,
    ) -> NetEntry {
        NetEntry {
            sandbox: sandbox.to_owned(),
            host: host.to_owned(),
            port,
            allowed,
            count,
            last_seen: format!("2026-10-29 {seen}"),
            first_seen: "2026-10-29 11:45:00".into(),
            allowed_count: if allowed { count } else { 0 },
            blocked_count: if allowed { 0 } else { count },
        }
    }

    fn populated() -> App {
        let mut app = App::new(
            PathBuf::from("/home/dev/dev/xap/api"),
            Some(PathBuf::from("/home/dev")),
            vec!["codex", "pi", "shell"],
            crate::tui::preview_name,
        );
        let running = SandboxInfo {
            agent: Some("codex".to_owned()),
            project: Some(PathBuf::from("/home/dev/dev/xap/api")),
            image: Some("node:24-bookworm-slim".to_owned()),
            created: Some("2026-10-29 11:40".to_owned()),
            vcpus: Some(18),
            cpu_percent: Some(18.0),
            memory_bytes: Some(GIB * 21 / 10),
            memory_limit_bytes: Some(GIB * 32),
            disk_used_bytes: Some(GIB * 12 / 10),
            disk_free_bytes: Some(GIB * 88 / 10),
            uptime: Some(Duration::from_secs(12 * 60)),
            network: Some("public".to_owned()),
            proxied: true,
            host_ports: Some(vec![1234, 11434]),
            user: Some("node".to_owned()),
            restricted: Some(true),
            git_guarded: true,
            rx_bytes: Some(48 * 1024 * 1024 + 200 * 1024),
            tx_bytes: Some(3 * 1024 * 1024 + 100 * 1024),
            secrets: vec!["OPENAI_API_KEY".to_owned()],
            mounts: vec!["/home/dev/dev/xap/api -> same path".to_owned()],
            ..info("codex-api", Status::Running)
        };
        let stopped = SandboxInfo {
            agent: Some("pi".to_owned()),
            project: Some(PathBuf::from("/home/dev/notes")),
            proxied: true,
            git_guarded: true,
            ..info("pi-notes", Status::Stopped)
        };
        let older = SandboxInfo {
            network: Some("public + host".to_owned()),
            ..info("scratch", Status::Running)
        };
        app.credentials = vec![
            "OPENAI_API_KEY: shell/Keychain available; shared login: missing".into(),
            "shared login: present".into(),
            "none required".into(),
        ];
        app.set_sandboxes(vec![running, stopped, older]);
        app.set_network(
            Some(Preset::Balanced),
            vec![
                entry("codex-api", "pypi.org", 443, false, 7, "12:04:51"),
                entry("codex-api", "169.254.169.254", 80, false, 1, "12:01:10"),
                entry("codex-api", "api.openai.com", 443, true, 42, "12:04:50"),
                entry("codex-api", "registry.npmjs.org", 443, true, 3, "11:58:02"),
                entry("pi-notes", "elsewhere.test", 443, false, 1, "10:00:00"),
            ],
        );
        app
    }

    fn draw(app: &App, width: u16, height: u16) -> anyhow::Result<Terminal<TestBackend>> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| render(frame, app))?;
        Ok(terminal)
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode) {
        let _ = app.update(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    fn snapshots(name: &str, app: &App) -> anyhow::Result<()> {
        let mut screens = Vec::new();
        for (width, height) in [(100, 30), (80, 24), (60, 20)] {
            screens.push(format!(
                "{width}x{height}\n{}",
                draw(app, width, height)?.backend()
            ));
        }
        insta::assert_snapshot!(name, screens.join("\n"));
        Ok(())
    }

    fn detail(tab: Tab) -> App {
        let mut app = populated();
        app.focus = Focus::Detail;
        app.tab = tab;
        app
    }

    fn full_network() -> App {
        let mut app = detail(Tab::Network);
        press(&mut app, crossterm::event::KeyCode::Char('n'));
        app
    }

    fn with_wizard(stage: CreateStage) -> App {
        let mut app = populated();
        press(&mut app, crossterm::event::KeyCode::Char('c'));
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.stage = stage;
            wizard.name.set_text("codex-api");
            wizard.folder.loading = false;
        }
        app
    }

    fn rules() -> App {
        let mut app = detail(Tab::Rules);
        app.set_rules(vec![
            RuleInfo {
                target: "api.openai.com".into(),
                source: Source::Preset,
                sandbox: None,
                allowed: true,
            },
            RuleInfo {
                target: "*.github.com:443".into(),
                source: Source::Global,
                sandbox: None,
                allowed: true,
            },
            RuleInfo {
                target: "telemetry.example.com".into(),
                source: Source::Sandbox,
                sandbox: Some("codex-api".into()),
                allowed: false,
            },
        ]);
        app
    }

    #[test]
    fn snapshot_dashboard_cards() -> anyhow::Result<()> {
        let mut app = populated();
        app.status_line = Some("created pi-notes; Enter attaches".into());
        snapshots("dashboard_cards", &app)
    }

    #[test]
    fn snapshot_dashboard_overview() -> anyhow::Result<()> {
        snapshots("dashboard_overview", &detail(Tab::Overview))
    }

    #[test]
    fn snapshot_overview_stopped() -> anyhow::Result<()> {
        let mut app = detail(Tab::Overview);
        app.selected = 1;
        snapshots("overview_stopped", &app)
    }

    fn with_git_changes() -> App {
        let mut app = detail(Tab::Overview);
        app.git_changes.insert(
            "codex-api".to_owned(),
            vec![
                ".git/hooks/pre-commit (new)".to_owned(),
                ".git/config: core.fsmonitor = ./tools/watch.sh (new)".to_owned(),
            ],
        );
        app
    }

    #[test]
    fn snapshot_git_changes() -> anyhow::Result<()> {
        snapshots("git_changes", &with_git_changes())
    }

    #[test]
    fn snapshot_git_changes_cards() -> anyhow::Result<()> {
        let mut app = with_git_changes();
        app.focus = Focus::Cards;
        snapshots("git_changes_cards", &app)
    }

    #[test]
    fn snapshot_overview_unguarded() -> anyhow::Result<()> {
        let mut app = detail(Tab::Overview);
        app.selected = 1;
        app.sandboxes[1].git_guarded = false;
        snapshots("overview_unguarded", &app)
    }

    #[test]
    fn git_badge_is_red_and_only_on_changed_cards() -> anyhow::Result<()> {
        let mut app = populated();
        app.git_changes
            .insert("pi-notes".to_owned(), vec![".git (new)".to_owned()]);
        let terminal = draw(&app, 100, 30)?;
        let buffer = terminal.backend().buffer();
        let text = format!("{buffer:?}");
        assert_eq!(text.matches("git!").count(), 1, "{text}");
        // The badge ends at the right edge of the pi-notes card's title row (row 11).
        let cell =
            anyhow::Context::context(buffer.cell((27, 11)), "the pi-notes title row is on screen")?;
        assert_eq!(cell.symbol(), "g", "{text}");
        assert_eq!(cell.fg, Color::Red);
        let red_rows = with_git_changes();
        let text = format!("{:?}", draw(&red_rows, 100, 30)?.backend().buffer());
        assert!(text.contains("review first"), "{text}");
        Ok(())
    }

    #[test]
    fn snapshot_overview_foreign() -> anyhow::Result<()> {
        let mut app = detail(Tab::Overview);
        app.selected = 2;
        snapshots("overview_foreign", &app)
    }

    #[test]
    fn snapshot_network_older() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        app.sandboxes[0].proxied = false;
        snapshots("network_older", &app)
    }

    #[test]
    fn snapshot_folders_empty() -> anyhow::Result<()> {
        snapshots("folders_empty", &with_wizard(CreateStage::Folder))
    }

    #[test]
    fn snapshot_rule_input_empty() -> anyhow::Result<()> {
        let mut app = rules();
        press(&mut app, crossterm::event::KeyCode::Char('a'));
        snapshots("rule_input_empty", &app)
    }

    #[test]
    fn snapshot_dashboard_empty() -> anyhow::Result<()> {
        let mut app = populated();
        app.set_sandboxes(Vec::new());
        snapshots("dashboard_empty", &app)
    }

    #[test]
    fn snapshot_new_card_and_scroll() -> anyhow::Result<()> {
        let mut app = populated();
        app.selected = app.sandboxes.len();
        snapshots("dashboard_new_card", &app)
    }

    #[test]
    fn snapshot_logs() -> anyhow::Result<()> {
        let mut app = detail(Tab::Logs);
        app.logs = (1..=40).map(|index| format!("log line {index}")).collect();
        app.detail_scroll = 20;
        snapshots("logs_scrolled", &app)
    }

    #[test]
    fn snapshot_logs_empty() -> anyhow::Result<()> {
        snapshots("logs_empty", &detail(Tab::Logs))
    }

    #[test]
    fn snapshot_network() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        app.network_row = 2;
        snapshots("network", &app)
    }

    #[test]
    fn snapshot_network_empty() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        app.set_network(Some(Preset::LockedDown), Vec::new());
        snapshots("network_empty", &app)
    }

    #[test]
    fn snapshot_network_foreign() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        app.selected = 2;
        snapshots("network_foreign", &app)
    }

    #[test]
    fn snapshot_full_network() -> anyhow::Result<()> {
        snapshots("full_network", &full_network())
    }

    #[test]
    fn snapshot_full_network_expanded() -> anyhow::Result<()> {
        let mut app = full_network();
        press(&mut app, crossterm::event::KeyCode::Enter);
        snapshots("full_network_expanded", &app)
    }

    #[test]
    fn snapshot_full_network_empty() -> anyhow::Result<()> {
        let mut app = full_network();
        app.set_network(Some(Preset::Balanced), Vec::new());
        snapshots("full_network_empty", &app)
    }

    #[test]
    fn snapshot_network_search() -> anyhow::Result<()> {
        let mut app = full_network();
        press(&mut app, crossterm::event::KeyCode::Char('/'));
        app.network_search.set_text("pypi");
        snapshots("network_search", &app)
    }

    #[test]
    fn snapshot_network_no_matches() -> anyhow::Result<()> {
        let mut app = full_network();
        app.network_search.set_text("nothing-matches");
        snapshots("network_no_matches", &app)
    }

    #[test]
    fn snapshot_rules() -> anyhow::Result<()> {
        snapshots("rules", &rules())
    }

    #[test]
    fn snapshot_rules_empty() -> anyhow::Result<()> {
        snapshots("rules_empty", &detail(Tab::Rules))
    }

    #[test]
    fn snapshot_rule_input() -> anyhow::Result<()> {
        let mut app = rules();
        press(&mut app, crossterm::event::KeyCode::Char('a'));
        if let Some(Popup::AddRule { target, .. }) = &mut app.popup {
            target.set_text("*.example.com:443");
        }
        snapshots("rule_input", &app)
    }

    #[test]
    fn snapshot_rule_error() -> anyhow::Result<()> {
        let mut app = rules();
        app.popup = Some(Popup::AddRule {
            target: TextInput::new("bad host"),
            allowed: false,
            sandbox: Some("codex-api".into()),
            error: Some("target cannot contain spaces".into()),
        });
        snapshots("rule_error", &app)
    }

    #[test]
    fn snapshot_rule_remove() -> anyhow::Result<()> {
        let mut app = rules();
        app.rule_row = 2;
        press(&mut app, crossterm::event::KeyCode::Char('r'));
        snapshots("rule_remove", &app)
    }

    #[test]
    fn snapshot_create_agents() -> anyhow::Result<()> {
        snapshots("create_agents", &with_wizard(CreateStage::Agent))
    }

    #[test]
    fn snapshot_create_configure() -> anyhow::Result<()> {
        snapshots("create_configure", &with_wizard(CreateStage::Configure))
    }

    #[test]
    fn snapshot_create_invalid() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Configure);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.name.set_text("bad name");
            wizard.error = Some("use lowercase letters, digits and single interior hyphens".into());
            wizard.field = CreateField::Create;
        }
        snapshots("create_invalid", &app)
    }

    #[test]
    fn snapshot_folders() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Folder);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.folder.directories = ["api", "docs", "web"]
                .map(|name| wizard.folder.path.join(name))
                .to_vec();
            wizard.folder.row = 3;
        }
        snapshots("folders", &app)
    }

    #[test]
    fn snapshot_folders_no_matches() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Folder);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.folder.filter.set_text("nothing");
        }
        snapshots("folders_no_matches", &app)
    }

    #[test]
    fn snapshot_folder_error() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Folder);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.folder.error = Some("reading /private/locked: permission denied".into());
        }
        snapshots("folder_error", &app)
    }

    #[test]
    fn snapshot_confirm_remove() -> anyhow::Result<()> {
        let mut app = populated();
        app.popup = Some(Popup::ConfirmRemove {
            name: "codex-api".into(),
        });
        snapshots("confirm_remove", &app)
    }

    #[test]
    fn snapshot_help() -> anyhow::Result<()> {
        let mut app = full_network();
        app.popup = Some(Popup::Help);
        snapshots("help", &app)
    }

    #[test]
    fn snapshot_help_scrolled() -> anyhow::Result<()> {
        let mut app = full_network();
        app.popup = Some(Popup::Help);
        app.popup_scroll = 99;
        snapshots("help_scrolled", &app)
    }

    #[test]
    fn snapshot_preset() -> anyhow::Result<()> {
        let mut app = populated();
        app.popup = Some(Popup::Preset {
            index: 1,
            then_create: true,
        });
        snapshots("preset", &app)
    }

    #[test]
    fn snapshot_full_rules() -> anyhow::Result<()> {
        let mut app = rules();
        app.full_network = true;
        app.status_filter = crate::app::StatusFilter::Allowed;
        snapshots("full_rules", &app)
    }

    #[test]
    fn snapshot_full_rules_empty() -> anyhow::Result<()> {
        let mut app = detail(Tab::Rules);
        app.full_network = true;
        snapshots("full_rules_empty", &app)
    }

    #[test]
    fn snapshot_network_expanded() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        press(&mut app, crossterm::event::KeyCode::Enter);
        snapshots("network_expanded", &app)
    }

    #[test]
    fn snapshot_network_filtered() -> anyhow::Result<()> {
        let mut app = full_network();
        app.status_filter = crate::app::StatusFilter::Blocked;
        app.sandbox_filter = Some("pi-notes".into());
        snapshots("network_filtered", &app)
    }

    #[test]
    fn snapshot_folder_loading() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Folder);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.folder.loading = true;
        }
        snapshots("folder_loading", &app)
    }

    fn long_connection() -> App {
        let mut app = full_network();
        let mut row = entry(
            &"sandbox-".repeat(15),
            &format!("{}.example.com", "long-host-".repeat(23)),
            65535,
            false,
            999_999,
            "12:04:51",
        );
        row.allowed_count = 1;
        row.blocked_count = 999_998;
        app.set_network(Some(Preset::Balanced), vec![row]);
        press(&mut app, crossterm::event::KeyCode::Enter);
        app
    }

    #[test]
    fn snapshot_long_network_details() -> anyhow::Result<()> {
        snapshots("network_long_details", &long_connection())
    }

    #[test]
    fn snapshot_long_network_details_scrolled() -> anyhow::Result<()> {
        let mut app = long_connection();
        app.detail_scroll = 128;
        snapshots("network_long_details_scrolled", &app)
    }

    #[test]
    fn expanded_long_identity_scrolls_without_blanking() -> anyhow::Result<()> {
        let mut app = long_connection();
        let first = draw(&app, 60, 20)?.backend().to_string();
        assert!(first.contains("PgUp") && first.contains("PgDn"), "{first}");
        app.set_detail_scroll_limit(detail_scroll_limit(&app, 60, 20));
        for _ in 0..10 {
            press(&mut app, crossterm::event::KeyCode::PageDown);
        }
        let last = draw(&app, 60, 20)?.backend().to_string();
        assert!(last.contains("Blocked hits: 999998"), "{last}");
        assert!(last.contains("Last seen:"), "{last}");
        Ok(())
    }

    fn long_rule_confirmation() -> App {
        let mut app = rules();
        app.popup = Some(Popup::ConfirmRuleRemove {
            rule: RuleInfo {
                target: format!("{}.example.com:443", "long-target-".repeat(19)),
                source: Source::Sandbox,
                sandbox: Some(format!("{}scope-tail", "sandbox-".repeat(14))),
                allowed: false,
            },
        });
        app
    }

    #[test]
    fn confirmation_keeps_full_rule_and_scope_visible() -> anyhow::Result<()> {
        let app = long_rule_confirmation();
        let text = draw(&app, 60, 20)?.backend().to_string();
        assert!(text.contains("Future connections"), "{text}");
        let compact: String = text
            .chars()
            .filter(|character| {
                character.is_ascii() && !character.is_whitespace() && *character != '\"'
            })
            .collect();
        if let Some(Popup::ConfirmRuleRemove { rule }) = &app.popup {
            assert!(compact.contains(&rule.target), "{text}");
            assert!(
                compact.contains(rule.sandbox.as_deref().unwrap_or_default()),
                "{text}"
            );
        }
        Ok(())
    }

    #[test]
    fn snapshot_long_rule_confirmation() -> anyhow::Result<()> {
        snapshots("rule_remove_long", &long_rule_confirmation())
    }

    fn agent_with_long_credential() -> App {
        let mut app = with_wizard(CreateStage::Agent);
        app.credentials[0] = format!(
            "shared login: unknown ({})",
            "long filesystem error ".repeat(30)
        );
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.agent_index = 2;
        }
        app
    }

    #[test]
    fn selected_agent_is_visible_after_long_credential_errors() -> anyhow::Result<()> {
        let text = draw(&agent_with_long_credential(), 60, 20)?
            .backend()
            .to_string();
        assert!(text.contains("▸ shell"), "{text}");
        assert!(text.contains("none required"), "{text}");
        Ok(())
    }

    #[test]
    fn snapshot_agent_credential_unknown() -> anyhow::Result<()> {
        snapshots("agent_credentials_unknown", &agent_with_long_credential())
    }

    #[test]
    fn wrapped_log_scroll_limit_reaches_the_last_line() -> anyhow::Result<()> {
        let mut app = detail(Tab::Logs);
        app.logs = vec![format!("{} tail-marker", "long-line-".repeat(120))];
        let limit = detail_scroll_limit(&app, 60, 20);
        assert!(limit > 0);
        app.detail_scroll = limit;
        let text = draw(&app, 60, 20)?.backend().to_string();
        assert!(text.contains("tail-marker"), "{text}");
        Ok(())
    }

    #[test]
    fn help_at_bottom_reverses_after_one_up() -> anyhow::Result<()> {
        let mut app = full_network();
        press(&mut app, crossterm::event::KeyCode::Char('?'));
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 60, 20));
        for _ in 0..30 {
            press(&mut app, crossterm::event::KeyCode::Down);
        }
        let bottom = draw(&app, 60, 20)?.backend().to_string();
        assert!(bottom.contains("6–20/20"), "{bottom}");
        press(&mut app, crossterm::event::KeyCode::Up);
        let previous = draw(&app, 60, 20)?.backend().to_string();
        assert!(previous.contains("5–19/20"), "{previous}");
        assert_eq!(app.popup_scroll, 4);
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 100, 30));
        assert_eq!(app.popup_scroll, 0);
        let resized = draw(&app, 100, 30)?.backend().to_string();
        assert!(resized.contains("1–20/20"), "{resized}");
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 60, 16));
        for _ in 0..30 {
            press(&mut app, crossterm::event::KeyCode::Down);
        }
        assert_eq!(app.popup_scroll, 9);
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 60, 20));
        assert_eq!(app.popup_scroll, 5);
        press(&mut app, crossterm::event::KeyCode::Up);
        assert_eq!(app.popup_scroll, 4);
        Ok(())
    }

    fn ctrl(app: &mut App, letter: char) {
        let _ = app.update(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(letter),
            crossterm::event::KeyModifiers::CONTROL,
        ));
    }

    /// codex-api's info, created with the default ports while policy.toml now says 1234, 8080.
    fn with_info() -> App {
        let mut app = populated();
        app.host_ports = Some(vec![1234, 8080]);
        app.set_rules(vec![RuleInfo {
            target: "telemetry.example.com".into(),
            source: Source::Sandbox,
            sandbox: Some("codex-api".into()),
            allowed: false,
        }]);
        press(&mut app, crossterm::event::KeyCode::Char('i'));
        app
    }

    #[test]
    fn snapshot_info() -> anyhow::Result<()> {
        snapshots("info", &with_info())
    }

    #[test]
    fn snapshot_info_bottom() -> anyhow::Result<()> {
        let mut app = with_info();
        app.popup_scroll = usize::MAX;
        snapshots("info_bottom", &app)
    }

    #[test]
    fn snapshot_info_foreign() -> anyhow::Result<()> {
        let mut app = populated();
        app.selected = 2;
        press(&mut app, crossterm::event::KeyCode::Char('i'));
        snapshots("info_foreign", &app)
    }

    #[test]
    fn info_scroll_limit_reaches_the_last_line_and_reverses() -> anyhow::Result<()> {
        let mut app = with_info();
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 60, 20));
        for _ in 0..60 {
            press(&mut app, crossterm::event::KeyCode::Down);
        }
        let bottom = draw(&app, 60, 20)?.backend().to_string();
        assert!(bottom.contains("secrets"), "{bottom}");
        let limit = app.popup_scroll;
        assert!(limit > 0, "the info outgrows 60x20");
        press(&mut app, crossterm::event::KeyCode::Char('k'));
        assert_eq!(app.popup_scroll, limit - 1, "one step back moves at once");
        app.set_popup_scroll_limit(popup_scroll_limit(&app, 100, 40));
        assert_eq!(app.popup_scroll, 0, "everything fits once the screen grows");
        Ok(())
    }

    #[test]
    fn info_flags_port_drift_and_settings_errors_by_colour() -> anyhow::Result<()> {
        let app = with_info();
        let terminal = draw(&app, 100, 40)?;
        let buffer = terminal.backend().buffer();
        let text = terminal.backend().to_string();
        assert!(text.contains("recreate to apply"), "{text}");
        let drift = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "8" && cell.fg == Color::Yellow);
        assert!(drift.is_some(), "the configured ports row is yellow");

        let mut app = populated();
        ctrl(&mut app, 's');
        press(&mut app, crossterm::event::KeyCode::Tab);
        press(&mut app, crossterm::event::KeyCode::Char('x'));
        press(&mut app, crossterm::event::KeyCode::Enter);
        let terminal = draw(&app, 80, 24)?;
        let text = terminal.backend().to_string();
        assert!(text.contains("invalid host port"), "{text}");
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "i" && cell.fg == Color::Red),
            "the error is red"
        );
        Ok(())
    }

    #[test]
    fn snapshot_settings() -> anyhow::Result<()> {
        let mut app = populated();
        app.host_ports = Some(vec![1234, 11434]);
        ctrl(&mut app, 's');
        snapshots("settings", &app)
    }

    #[test]
    fn snapshot_settings_error() -> anyhow::Result<()> {
        let mut app = populated();
        app.host_ports = Some(vec![1234, 11434]);
        ctrl(&mut app, 's');
        press(&mut app, crossterm::event::KeyCode::Tab);
        press(&mut app, crossterm::event::KeyCode::Char(','));
        press(&mut app, crossterm::event::KeyCode::Char('0'));
        press(&mut app, crossterm::event::KeyCode::Enter);
        snapshots("settings_error", &app)
    }

    #[test]
    fn snapshot_settings_no_policy() -> anyhow::Result<()> {
        let mut app = populated();
        app.set_network(None, Vec::new());
        ctrl(&mut app, 's');
        snapshots("settings_no_policy", &app)
    }

    #[test]
    fn rules_do_not_advertise_an_inactive_status_filter() -> anyhow::Result<()> {
        let mut app = rules();
        app.full_network = true;
        app.status_filter = crate::app::StatusFilter::Allowed;
        let text = draw(&app, 60, 20)?.backend().to_string();
        assert!(!text.contains("o Allowed"), "{text}");
        Ok(())
    }

    #[test]
    fn focused_pane_is_named_at_narrow_widths() -> anyhow::Result<()> {
        let mut app = populated();
        for (width, height) in [(80, 24), (60, 20)] {
            let cards = draw(&app, width, height)?.backend().to_string();
            assert!(cards.contains("> Sandboxes"), "{cards}");
            press(&mut app, crossterm::event::KeyCode::Right);
            let detail = draw(&app, width, height)?.backend().to_string();
            assert!(detail.contains("> Details"), "{detail}");
            press(&mut app, crossterm::event::KeyCode::Left);
        }
        Ok(())
    }

    #[test]
    fn full_network_names_table_columns_and_filter_controls() -> anyhow::Result<()> {
        let text = draw(&full_network(), 80, 24)?.backend().to_string();
        for expected in ["All sandboxes", "Host", "Port", "Hits", "Status", "Sandbox"] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        Ok(())
    }

    #[test]
    fn table_truncation_is_explicit_for_large_counts() -> anyhow::Result<()> {
        let mut app = full_network();
        app.set_network(
            Some(Preset::Balanced),
            vec![entry(
                "long-sandbox-name",
                "long-endpoint-name.example.com",
                443,
                true,
                123_456,
                "12:04:51",
            )],
        );
        let text = draw(&app, 60, 20)?.backend().to_string();
        assert!(text.contains("1234…"), "{text}");
        assert!(text.contains("long-sand…"), "{text}");
        Ok(())
    }

    #[test]
    fn no_color_resets_cells_and_keeps_focus_and_status() -> anyhow::Result<()> {
        let mut app = detail(Tab::Network);
        app.no_color = true;
        let terminal = draw(&app, 60, 20)?;
        for cell in &terminal.backend().buffer().content {
            assert_eq!(cell.fg, Color::Reset);
            assert_eq!(cell.bg, Color::Reset);
        }
        let text = terminal.backend().to_string();
        assert!(text.contains("> Detail"));
        assert!(text.contains("Blocked"));
        assert!(text.contains("Allowed"));
        Ok(())
    }

    #[test]
    fn selected_row_past_a_page_and_full_details_remain_visible() -> anyhow::Result<()> {
        let mut app = full_network();
        app.set_network(
            Some(Preset::Balanced),
            (0..30)
                .map(|index| {
                    entry(
                        "codex-api",
                        &format!("host-{index}.example.com"),
                        443,
                        false,
                        7,
                        "12:04:51",
                    )
                })
                .collect(),
        );
        app.network_row = 29;
        press(&mut app, crossterm::event::KeyCode::Enter);
        let text = draw(&app, 60, 20)?.backend().to_string();
        for expected in [
            "host-29.example.com",
            "First seen",
            "Last seen",
            "Allowed hits",
            "Blocked hits",
            "codex-api",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        Ok(())
    }

    #[test]
    fn long_input_keeps_its_cursor_inside_the_frame() -> anyhow::Result<()> {
        let mut app = with_wizard(CreateStage::Configure);
        if let Some(Popup::Create(wizard)) = &mut app.popup {
            wizard.name.set_text(&"long-project-".repeat(20));
        }
        let mut terminal = draw(&app, 60, 20)?;
        let cursor = ratatui::backend::Backend::get_cursor_position(terminal.backend_mut())?;
        assert!(cursor.x < 60 && cursor.y < 20);
        Ok(())
    }

    #[test]
    fn narrow_and_tiny_terminals_do_not_panic() -> anyhow::Result<()> {
        let mut apps = vec![
            populated(),
            full_network(),
            rules(),
            with_wizard(CreateStage::Agent),
            with_wizard(CreateStage::Configure),
            with_wizard(CreateStage::Folder),
        ];
        let mut help = populated();
        help.popup = Some(Popup::Help);
        apps.push(help);
        for app in apps {
            for (width, height) in [(60, 20), (10, 3), (1, 1)] {
                draw(&app, width, height)?;
            }
        }
        Ok(())
    }
}
