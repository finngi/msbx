//! Renders [`App`] to a ratatui frame. Pure: reads state, draws, nothing else.

use std::path::Path;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::app::{App, Popup, Tab};
use crate::model::{NetEntry, SandboxInfo, Status};
use crate::policy::Preset;

const ACCENT: Color = Color::Cyan;
const CARDS_WIDTH: u16 = 34;
const CARD_HEIGHT: u16 = 7;
const NEW_CARD_HEIGHT: u16 = 3;
const LABEL_WIDTH: usize = 9;
const KEYS: [(&str, &str); 9] = [
    ("↑↓", "select"),
    ("tab", "pane"),
    ("↵", "attach"),
    ("c", "create"),
    ("s", "start/stop"),
    ("x", "shell"),
    ("r", "remove"),
    ("?", "help"),
    ("q", "quit"),
];
/// Keys that only work on the Network tab.
const NETWORK_KEYS: [(&str, &str); 5] = [
    ("←→", "row"),
    ("a", "allow"),
    ("A", "allow all"),
    ("d", "deny"),
    ("p", "preset"),
];

pub(crate) fn render(frame: &mut Frame, app: &App) {
    let [header, _, body, footer] = frame.area().layout(&Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ]));
    render_header(frame, header);
    let [cards, _, detail] = body.layout(&Layout::horizontal([
        Constraint::Length(CARDS_WIDTH),
        Constraint::Length(2),
        Constraint::Fill(1),
    ]));
    render_cards(frame, app, cards);
    render_detail(frame, app, detail);
    render_footer(frame, app, footer);
    match &app.popup {
        Some(Popup::Create { agent_index }) => render_create(frame, app, *agent_index),
        Some(Popup::ConfirmRemove { name }) => render_confirm_remove(frame, name),
        Some(Popup::Help) => render_help(frame),
        Some(Popup::Preset { index, then_create }) => render_preset(frame, *index, *then_create),
        None => {}
    }
}

fn render_header(frame: &mut Frame, area: Rect) {
    let left = vec![
        Line::from("msbx".bold()),
        Line::from("   microVM sandboxes for coding agents · microsandbox".dim()),
    ];
    let right = Line::from(vec!["?".bold(), " help".dim()]);
    frame.render_widget(spread(left, right, area.width), area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = |(key, label): &(&'static str, &'static str)| {
        Line::from(vec![key.bold(), format!(" {label}  ").dim()])
    };
    let hints = if app.tab == Tab::Network {
        KEYS[..2]
            .iter()
            .chain(&NETWORK_KEYS)
            .chain(&KEYS[7..])
            .map(hint)
            .collect()
    } else {
        KEYS.iter().map(hint).collect()
    };
    let status = app
        .status_line
        .as_deref()
        .map(|line| Line::from(line.to_owned().yellow()))
        .unwrap_or_default();
    frame.render_widget(spread(hints, status, area.width), area);
}

/// Left chunks and a right part on one line. Whole left chunks are dropped when space runs
/// out, keeping a two-column gap, so a hint is never cut in half.
fn spread<'a>(left: Vec<Line<'a>>, right: Line<'a>, width: u16) -> Line<'a> {
    let right_width = right.width();
    let gap = if right_width == 0 { 0 } else { 2 };
    let left_room = usize::from(width).saturating_sub(right_width + gap);
    let mut spans = Vec::new();
    let mut used = 0;
    for chunk in left {
        let chunk_width = chunk.width();
        if used + chunk_width > left_room {
            break;
        }
        used += chunk_width;
        spans.extend(chunk.spans);
    }
    spans.push(
        " ".repeat(usize::from(width).saturating_sub(used + right_width))
            .into(),
    );
    spans.extend(right.spans);
    Line::from(spans)
}

fn render_cards(frame: &mut Frame, app: &App, area: Rect) {
    let mut y = area.y;
    for index in first_visible_card(app, area.height)..=app.sandboxes.len() {
        let height = card_height(app, index);
        if y + height > area.bottom() {
            break;
        }
        let card = Rect::new(area.x, y, area.width, height);
        match app.sandboxes.get(index) {
            Some(sandbox) => render_card(frame, app, sandbox, index == app.selected, card),
            None => render_new_card(frame, index == app.selected, card),
        }
        y += height;
    }
}

fn card_height(app: &App, index: usize) -> u16 {
    if index < app.sandboxes.len() {
        CARD_HEIGHT
    } else {
        NEW_CARD_HEIGHT
    }
}

/// Scrolls just far enough that the selected card fits at the bottom.
fn first_visible_card(app: &App, height: u16) -> usize {
    let mut first = 0;
    while first < app.selected {
        let needed: u16 = (first..=app.selected)
            .map(|index| card_height(app, index))
            .sum();
        if needed <= height {
            break;
        }
        first += 1;
    }
    first
}

fn card_block<'a>(selected: bool) -> Block<'a> {
    let colour = if selected { ACCENT } else { Color::DarkGray };
    Block::bordered()
        .border_style(Style::new().fg(colour))
        .padding(Padding::horizontal(1))
}

fn render_card(frame: &mut Frame, app: &App, sandbox: &SandboxInfo, selected: bool, area: Rect) {
    let block = card_block(selected);
    let width = block.inner(area).width;
    let (symbol, colour) = status_symbol(sandbox.status);
    let agent = Line::from(sandbox.agent.clone().unwrap_or_else(|| "—".into()));
    let enter_hint = match (selected, &sandbox.agent) {
        (false, _) => Line::default(),
        (true, Some(_)) => Line::from("↵ attach".dim()),
        (true, None) => Line::from("↵ shell".dim()),
    };
    let project = sandbox.project.as_deref().map_or_else(
        || "—".into(),
        |path| shorten_home(path, app.home.as_deref()),
    );
    let lines = vec![
        Line::from(vec![
            symbol.fg(colour),
            " ".into(),
            sandbox.name.clone().bold(),
        ]),
        spread(vec![agent], enter_hint, width),
        Line::from(project.dim()),
        card_metrics(sandbox),
        action_row(sandbox.status),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
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

fn render_new_card(frame: &mut Frame, selected: bool, area: Rect) {
    let block = card_block(selected).border_type(BorderType::LightDoubleDashed);
    let text = Line::from(vec!["+ new sandbox  ".into(), "(c)".dim()]).centered();
    frame.render_widget(Paragraph::new(text).block(block), area);
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

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(sandbox) = app.selected_sandbox() else {
        let lines = if app.sandboxes.is_empty() {
            vec![Line::from("No sandboxes yet — press c to create one".dim())]
        } else {
            vec![
                Line::from("New sandbox".bold()),
                Line::default(),
                Line::from("Press ↵ or c to create a sandbox for".dim()),
                Line::from(shorten_home(&app.project, app.home.as_deref())),
            ]
        };
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    let [title, tabs, rule, content] = area.layout(&Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ]));
    frame.render_widget(Line::from(sandbox.name.clone().bold()), title);
    frame.render_widget(tab_line(app.tab), tabs);
    frame.render_widget(
        Line::from("─".repeat(usize::from(rule.width)).dark_gray()),
        rule,
    );
    let lines = match app.tab {
        Tab::Overview => overview(sandbox, app.policy),
        Tab::Logs => log_tail(&app.logs, content.height),
        Tab::Network => network_tab(app, sandbox, content.height),
    };
    frame.render_widget(Paragraph::new(lines), content);
}

fn tab_line(active: Tab) -> Line<'static> {
    let tab = |label: &'static str, tab: Tab| {
        if tab == active {
            label.fg(ACCENT).bold().underlined()
        } else {
            label.dim()
        }
    };
    Line::from(vec![
        tab("Overview", Tab::Overview),
        "   ".into(),
        tab("Logs", Tab::Logs),
        "   ".into(),
        tab("Network", Tab::Network),
    ])
}

fn overview(sandbox: &SandboxInfo, policy: Option<Preset>) -> Vec<Line<'static>> {
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
        row("status", status),
        row("cpu", cpu),
        row("memory", memory(sandbox, " / ")),
        row("network", network),
    ];
    if let Some(traffic) = traffic(sandbox) {
        lines.push(row("traffic", traffic));
    }
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

fn network_tab(app: &App, sandbox: &SandboxInfo, height: u16) -> Vec<Line<'static>> {
    let rows = app.network_rows();
    let blocked = rows.iter().filter(|entry| !entry.allowed).count();
    let mut header = vec![format!("policy {}", preset_word(app.policy)).into()];
    if let Some(traffic) = traffic(sandbox) {
        header.push(" · ".dim());
        header.push(traffic.into());
    }
    header.push(" · ".dim());
    header.push(format!("{blocked} blocked").into());
    let mut lines = vec![Line::from(header), Line::default()];
    if !sandbox.proxied {
        lines.push(Line::from(
            "created before network policy; recreate with".dim(),
        ));
        lines.push(Line::from(format!("msb rm -f {}", sandbox.name)));
        return lines;
    }
    if rows.is_empty() {
        lines.push(Line::from("No connections yet".dim()));
        return lines;
    }
    let visible = usize::from(height).saturating_sub(lines.len()).max(1);
    let start = app.network_row.saturating_sub(visible - 1);
    for (index, entry) in rows.iter().enumerate().skip(start).take(visible) {
        lines.push(network_row(entry, index == app.network_row));
    }
    lines
}

fn network_row(entry: &NetEntry, selected: bool) -> Line<'static> {
    let (symbol, colour) = if entry.allowed {
        ("✓", Color::Green)
    } else {
        ("✗", Color::Red)
    };
    let cursor = if selected { "▸ " } else { "  " };
    let target = format!("{}:{}", entry.host, entry.port);
    let mut line = Line::from(vec![
        cursor.fg(ACCENT),
        symbol.fg(colour),
        " ".into(),
        format!("{target:<32}").into(),
        format!("{:>6}", entry.count).into(),
        "  ".into(),
        entry.last_seen.clone().dim(),
    ]);
    if selected {
        line = line.bold();
    }
    line
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

fn log_tail(logs: &[String], height: u16) -> Vec<Line<'static>> {
    if logs.is_empty() {
        return vec![Line::from("No logs yet".dim())];
    }
    let start = logs.len().saturating_sub(usize::from(height));
    logs[start..]
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect()
}

fn popup_block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(" {title} ").bold())
        .padding(Padding::new(2, 2, 1, 0))
}

fn render_popup(frame: &mut Frame, title: &str, lines: Vec<Line<'_>>, width: u16) {
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(3);
    let area = frame
        .area()
        .centered(Constraint::Length(width), Constraint::Length(height));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(popup_block(title)), area);
}

fn render_create(frame: &mut Frame, app: &App, agent_index: usize) {
    let mut choices = vec![format!("{:LABEL_WIDTH$}", "agent").dim()];
    for (index, agent) in app.agents.iter().enumerate() {
        let choice = format!(" {agent} ");
        choices.push(if index == agent_index {
            choice.fg(Color::Black).bg(ACCENT).bold()
        } else {
            choice.dim()
        });
        choices.push(" ".into());
    }
    let agent = app.agents.get(agent_index).copied().unwrap_or_default();
    let lines = vec![
        Line::from(choices),
        Line::default(),
        row("project", shorten_home(&app.project, app.home.as_deref())),
        row("name", (app.sandbox_name)(agent, &app.project)),
        Line::default(),
        Line::from(vec![
            "←→".bold(),
            " agent  ".dim(),
            "↵".bold(),
            " create  ".dim(),
            "esc".bold(),
            " cancel".dim(),
        ]),
    ];
    render_popup(frame, "New sandbox", lines, 56);
}

fn render_confirm_remove(frame: &mut Frame, name: &str) {
    let lines = vec![
        Line::from(vec!["Remove ".into(), name.bold(), "?".into()]),
        Line::default(),
        Line::from(vec![
            "y".bold(),
            " remove  ".dim(),
            "n".bold(),
            " cancel".dim(),
        ]),
    ];
    render_popup(frame, "Remove sandbox", lines, 48);
}

fn render_preset(frame: &mut Frame, index: usize, then_create: bool) {
    let mut lines = Vec::new();
    if then_create {
        lines.push(Line::from(
            "Choose once for all sandboxes; change it later with p.".dim(),
        ));
        lines.push(Line::default());
    }
    for (position, preset) in Preset::ALL.iter().enumerate() {
        let title = format!(" {:<12}", preset.title());
        let title = if position == index {
            title.fg(Color::Black).bg(ACCENT).bold()
        } else {
            title.into()
        };
        lines.push(Line::from(vec![title, " ".into(), preset.describe().dim()]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        "↑↓".bold(),
        " choose  ".dim(),
        "↵".bold(),
        " select  ".dim(),
        "esc".bold(),
        " cancel".dim(),
    ]));
    render_popup(frame, "Network policy", lines, 76);
}

fn render_help(frame: &mut Frame) {
    let hint = |(key, label): &(&'static str, &'static str)| {
        Line::from(vec![format!("{key:8}").bold(), label.dim()])
    };
    let mut lines: Vec<Line<'static>> = KEYS.iter().map(hint).collect();
    lines.push(Line::default());
    lines.push(Line::from("Network tab".bold()));
    lines.extend(NETWORK_KEYS.iter().map(hint));
    render_popup(frame, "Keys", lines, 34);
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
            vcpus: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
            uptime: None,
            network: None,
            proxied: false,
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
            last_seen: seen.to_owned(),
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
            vcpus: Some(18),
            cpu_percent: Some(18.0),
            memory_bytes: Some(GIB * 21 / 10),
            memory_limit_bytes: Some(GIB * 32),
            uptime: Some(Duration::from_secs(12 * 60)),
            network: Some("public".to_owned()),
            proxied: true,
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
            ..info("pi-notes", Status::Stopped)
        };
        let older = SandboxInfo {
            network: Some("public + host".to_owned()),
            ..info("scratch", Status::Running)
        };
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

    fn draw(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
    }

    #[test]
    fn snapshot_populated() {
        let mut app = populated();
        app.status_line = Some("started pi-notes".to_owned());
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_stopped_and_foreign_overview() {
        let mut app = populated();
        app.selected = 2;
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_new_card_selected() {
        let mut app = populated();
        app.selected = 3;
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_empty() {
        let app = App::new(
            PathBuf::from("/home/dev/api"),
            None,
            vec!["codex"],
            crate::tui::preview_name,
        );
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_create_popup() {
        let mut app = populated();
        app.popup = Some(Popup::Create { agent_index: 1 });
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_confirm_remove_popup() {
        let mut app = populated();
        app.popup = Some(Popup::ConfirmRemove {
            name: "codex-api".to_owned(),
        });
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_help_popup() {
        let mut app = populated();
        app.popup = Some(Popup::Help);
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_logs_tab() {
        let mut app = populated();
        app.tab = Tab::Logs;
        app.logs = (1..=40).map(|n| format!("log line {n}")).collect();
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_network_tab_populated() {
        let mut app = populated();
        app.tab = Tab::Network;
        app.network_row = 2;
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_network_tab_empty() {
        let mut app = populated();
        app.tab = Tab::Network;
        app.selected = 1;
        app.set_network(Some(Preset::LockedDown), Vec::new());
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_network_tab_older_sandbox() {
        let mut app = populated();
        app.tab = Tab::Network;
        app.selected = 2;
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn snapshot_preset_popup() {
        let mut app = populated();
        app.popup = Some(Popup::Preset {
            index: 1,
            then_create: true,
        });
        insta::assert_snapshot!(draw(&app, 100, 30).backend());
    }

    #[test]
    fn network_tab_scrolls_to_the_cursor() {
        let mut app = populated();
        app.tab = Tab::Network;
        app.network_row = 3;
        // 8 rows of detail: title, tabs, rule, header, blank, then 3 visible rows.
        let terminal = draw(&app, 100, 10);
        let text = format!("{:?}", terminal.backend().buffer());
        assert!(text.contains("registry.npmjs.org"), "{text}");
        assert!(!text.contains("pypi.org:443"), "{text}");
    }

    #[test]
    fn snapshot_scrolls_to_selected_card() {
        let mut app = populated();
        app.selected = 3;
        insta::assert_snapshot!(draw(&app, 100, 16).backend());
    }

    #[test]
    fn snapshot_narrow() {
        let mut app = populated();
        app.popup = Some(Popup::Create { agent_index: 0 });
        insta::assert_snapshot!(draw(&app, 60, 20).backend());
    }

    #[test]
    fn narrow_and_tiny_terminals_do_not_panic() {
        let mut app = populated();
        for popup in [
            None,
            Some(Popup::Help),
            Some(Popup::Create { agent_index: 0 }),
            Some(Popup::Preset {
                index: 0,
                then_create: false,
            }),
        ] {
            for tab in [Tab::Overview, Tab::Logs, Tab::Network] {
                app.tab = tab;
                app.popup = popup.clone();
                draw(&app, 60, 20);
                draw(&app, 10, 3);
                draw(&app, 1, 1);
            }
        }
    }
}
