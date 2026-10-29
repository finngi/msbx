//! `msbx policy …`: the commands that read and edit `policy.toml` and the connection log, and the
//! first-run preset question. Rendering is pure (returns `String`) so it is tested; `emit` is
//! the one place that writes to stdout.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::netlog::{self, Entry};
use crate::paths::Dirs;
use crate::policy::{self, Policy, Preset, Target, Verdict};

/// Which rows `msbx policy log` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Blocked,
    Allowed,
}

/// Writes CLI output to stdout.
pub(crate) fn emit(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .context("writing to stdout")
}

/// The policy, or an error telling the user to initialise it.
pub(crate) fn load_required(dirs: &Dirs) -> Result<Policy> {
    policy::load(&dirs.policy_file())?.context(NOT_INITIALISED)
}

const NOT_INITIALISED: &str =
    "network policy is not initialised: run `msbx policy init <open|balanced|locked-down>`";

/// The policy for a path that is about to create a sandbox: asks for a preset on a terminal
/// when the file is missing, and fails with the `init` hint otherwise.
pub(crate) fn require_or_ask(dirs: &Dirs) -> Result<Policy> {
    if let Some(policy) = policy::load(&dirs.policy_file())? {
        return Ok(policy);
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() || !std::io::stderr().is_terminal() {
        bail!(NOT_INITIALISED);
    }
    let preset = ask_preset(stdin.lock(), &mut std::io::stderr())?;
    let policy = Policy::new(preset);
    policy::save(&dirs.policy_file(), &policy)?;
    eprintln!("wrote {} (preset {preset})", dirs.policy_file().display());
    Ok(policy)
}

/// Asks once for a preset; Enter picks Balanced.
pub(crate) fn ask_preset(mut input: impl BufRead, output: &mut impl Write) -> Result<Preset> {
    writeln!(output, "Choose a network policy for msbx sandboxes:")?;
    for (index, preset) in Preset::ALL.iter().enumerate() {
        writeln!(
            output,
            "  {}. {:<12}{}",
            index + 1,
            preset.title(),
            preset.describe()
        )?;
    }
    write!(output, "Preset [1-3, default 2]: ")?;
    output.flush()?;
    let mut line = String::new();
    input
        .read_line(&mut line)
        .context("reading the preset choice")?;
    match line.trim() {
        "" | "2" | "balanced" => Ok(Preset::Balanced),
        "1" | "open" => Ok(Preset::Open),
        "3" | "locked-down" => Ok(Preset::LockedDown),
        other => {
            bail!("{other:?} is not a preset; run `msbx policy init <open|balanced|locked-down>`")
        }
    }
}

pub(crate) fn init(dirs: &Dirs, preset: Preset) -> Result<String> {
    let path = dirs.policy_file();
    if policy::load(&path)?.is_some() {
        bail!(
            "{} already exists; change the preset with `msbx policy preset {preset}`",
            path.display()
        );
    }
    policy::save(&path, &Policy::new(preset))?;
    Ok(format!("wrote {} (preset {preset})\n", path.display()))
}

pub(crate) fn set_preset(dirs: &Dirs, preset: Preset) -> Result<String> {
    let mut policy = load_required(dirs)?;
    policy.preset = preset;
    policy::save(&dirs.policy_file(), &policy)?;
    Ok(format!("preset is now {preset}\n"))
}

/// The effective rules, as a table with their source.
pub(crate) fn ls(policy: &Policy, sandbox: Option<&str>) -> String {
    let mut out = format!("preset {}\n", policy.preset);
    let rules = policy.rules(sandbox);
    if rules.is_empty() {
        out.push_str("no rules\n");
        return out;
    }
    let mut rows = vec![[
        "ACTION".to_owned(),
        "TARGET".to_owned(),
        "SOURCE".to_owned(),
    ]];
    rows.extend(rules.iter().map(|rule| {
        let action = match rule.verdict {
            Verdict::Allow => "allow",
            Verdict::Block => "deny",
        };
        [
            action.to_owned(),
            rule.target.to_string(),
            rule.source.name().to_owned(),
        ]
    }));
    out.push_str(&table(&rows));
    out
}

pub(crate) fn allow(dirs: &Dirs, targets: &str, sandbox: Option<&str>) -> Result<String> {
    let targets = policy::parse_targets(targets)?;
    let mut policy = load_required(dirs)?;
    let added = policy.allow(&targets, sandbox);
    policy::save(&dirs.policy_file(), &policy)?;
    Ok(edited("allowed", &targets, &added, sandbox))
}

pub(crate) fn deny(dirs: &Dirs, targets: &str, sandbox: Option<&str>) -> Result<String> {
    let targets = policy::parse_targets(targets)?;
    let mut policy = load_required(dirs)?;
    let added = policy.deny(&targets, sandbox);
    policy::save(&dirs.policy_file(), &policy)?;
    Ok(edited("denied", &targets, &added, sandbox))
}

pub(crate) fn rm(dirs: &Dirs, target: &str, sandbox: Option<&str>) -> Result<String> {
    let target = Target::parse(target)?;
    let mut policy = load_required(dirs)?;
    policy.remove(&target, sandbox)?;
    policy::save(&dirs.policy_file(), &policy)?;
    Ok(format!("removed {target}{}\n", scope(sandbox)))
}

fn edited(verb: &str, targets: &[Target], added: &[Target], sandbox: Option<&str>) -> String {
    let names: Vec<String> = targets.iter().map(ToString::to_string).collect();
    let note = if added.len() == targets.len() {
        String::new()
    } else {
        " (some were already there)".to_owned()
    };
    format!("{verb} {}{}{note}\n", names.join(", "), scope(sandbox))
}

fn scope(sandbox: Option<&str>) -> String {
    sandbox.map_or_else(String::new, |name| format!(" for sandbox {name}"))
}

/// `msbx policy check`: the decision for a host or IP by name alone, with no DNS lookup.
/// Returns the line to print and whether the connection would be allowed.
pub(crate) fn check(
    policy: &Policy,
    target: &str,
    sandbox: Option<&str>,
) -> Result<(String, bool)> {
    let parsed = Target::parse(target)?;
    let (host, ip, port) = parsed.endpoint().with_context(|| {
        format!("{target} is a pattern; check takes one host or IP, with an optional port")
    })?;
    let decision = policy.decide(sandbox.unwrap_or(""), host, ip, port);
    let allowed = decision.verdict == Verdict::Allow;
    let word = if allowed { "Allowed" } else { "Blocked" };
    Ok((format!("{word}  {target}  {}\n", decision.rule), allowed))
}

/// `msbx policy log`: BLOCKED then ALLOWED, like `sbx policy log`.
pub(crate) fn log(
    entries: &[Entry],
    sandbox: Option<&str>,
    filter: Filter,
    limit: Option<usize>,
    time: impl Fn(u64) -> String,
) -> String {
    let rows = netlog::sorted(entries, sandbox);
    let mut out = String::new();
    for (title, allowed) in [("BLOCKED", false), ("ALLOWED", true)] {
        if (allowed && filter == Filter::Blocked) || (!allowed && filter == Filter::Allowed) {
            continue;
        }
        let section: Vec<&&Entry> = rows
            .iter()
            .filter(|entry| entry.allowed == allowed)
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        out.push_str(title);
        out.push('\n');
        if section.is_empty() {
            out.push_str("  none\n\n");
            continue;
        }
        let mut table_rows =
            vec![["SANDBOX", "HOST", "PORT", "RULE", "LAST-SEEN", "COUNT"].map(str::to_owned)];
        table_rows.extend(section.iter().map(|entry| {
            [
                entry.sandbox.clone(),
                entry.host.clone(),
                entry.port.to_string(),
                entry.rule.clone(),
                time(entry.last_seen),
                entry.count.to_string(),
            ]
        }));
        out.push_str(&table(&table_rows));
        out.push('\n');
    }
    out.trim_end_matches('\n').to_owned() + "\n"
}

/// Left-aligned columns, two spaces apart, the first row being the header.
fn table<const N: usize>(rows: &[[String; N]]) -> String {
    let mut widths = [0usize; N];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        let mut policy = Policy::new(Preset::LockedDown);
        policy.allow = vec![
            Target::parse("pypi.org")
                .ok()
                .unwrap_or_else(|| unreachable!()),
        ];
        policy
    }

    #[test]
    fn ls_lists_rules_with_their_source() {
        let out = ls(&policy(), None);
        assert_eq!(
            out,
            "preset locked-down\nACTION  TARGET    SOURCE\nallow   pypi.org  global\n"
        );
        assert_eq!(
            ls(&Policy::new(Preset::Open), None),
            "preset open\nno rules\n"
        );
        let balanced = ls(&Policy::new(Preset::Balanced), Some("box"));
        assert!(
            balanced.lines().any(|line| {
                line.starts_with("allow   registry.npmjs.org") && line.ends_with("preset")
            }),
            "{balanced}"
        );
    }

    #[test]
    fn check_reports_the_decision_and_rejects_patterns() -> Result<()> {
        let policy = policy();
        assert_eq!(
            check(&policy, "pypi.org", None)?,
            (
                "Allowed  pypi.org  allow pypi.org (global)\n".to_owned(),
                true
            )
        );
        assert_eq!(
            check(&policy, "evil.test:443", Some("box"))?,
            ("Blocked  evil.test:443  default\n".to_owned(), false)
        );
        assert!(!check(&policy, "10.0.0.1", None)?.1);
        assert!(check(&policy, "*.pypi.org", None).is_err());
        assert!(check(&policy, "**", None).is_err());
        assert!(check(&policy, "10.0.0.0/8", None).is_err());
        Ok(())
    }

    fn entries() -> Vec<Entry> {
        let entry = |host: &str, allowed: bool, rule: &str, last_seen: u64, count: u64| Entry {
            sandbox: "box".to_owned(),
            host: host.to_owned(),
            port: 443,
            allowed,
            rule: rule.to_owned(),
            count,
            first_seen: 1,
            last_seen,
        };
        vec![
            entry("pypi.org", false, "default", 20, 7),
            entry(
                "api.openai.com",
                true,
                "allow api.openai.com (preset)",
                30,
                42,
            ),
            entry("evil.test", false, "deny evil.test (global)", 25, 1),
        ]
    }

    #[test]
    fn log_shows_blocked_then_allowed() {
        let out = log(&entries(), None, Filter::All, None, |secs| {
            format!("t{secs}")
        });
        assert_eq!(
            out,
            "BLOCKED\n\
             SANDBOX  HOST       PORT  RULE                     LAST-SEEN  COUNT\n\
             box      evil.test  443   deny evil.test (global)  t25        1\n\
             box      pypi.org   443   default                  t20        7\n\
             \n\
             ALLOWED\n\
             SANDBOX  HOST            PORT  RULE                           LAST-SEEN  COUNT\n\
             box      api.openai.com  443   allow api.openai.com (preset)  t30        42\n"
        );
    }

    #[test]
    fn log_filters_limits_and_handles_empty_sections() {
        let blocked = log(&entries(), None, Filter::Blocked, Some(1), |_| {
            "t".to_owned()
        });
        assert!(blocked.starts_with("BLOCKED\n"), "{blocked}");
        assert!(
            blocked.contains("evil.test") && !blocked.contains("pypi.org"),
            "{blocked}"
        );
        assert!(!blocked.contains("ALLOWED"), "{blocked}");
        let other = log(&entries(), Some("other"), Filter::All, None, |_| {
            "t".to_owned()
        });
        assert_eq!(other, "BLOCKED\n  none\n\nALLOWED\n  none\n");
        let allowed = log(&entries(), Some("box"), Filter::Allowed, None, |_| {
            "t".to_owned()
        });
        assert!(allowed.starts_with("ALLOWED\n"), "{allowed}");
    }

    #[test]
    fn ask_preset_accepts_numbers_names_and_the_default() -> Result<()> {
        let mut shown = Vec::new();
        assert_eq!(ask_preset("\n".as_bytes(), &mut shown)?, Preset::Balanced);
        let prompt = String::from_utf8(shown)?;
        assert!(prompt.contains("1. Open"), "{prompt}");
        assert!(prompt.contains("3. Locked Down"), "{prompt}");
        assert!(prompt.ends_with("Preset [1-3, default 2]: "), "{prompt}");
        assert_eq!(ask_preset("1\n".as_bytes(), &mut Vec::new())?, Preset::Open);
        assert_eq!(
            ask_preset("locked-down\n".as_bytes(), &mut Vec::new())?,
            Preset::LockedDown
        );
        assert!(ask_preset("9\n".as_bytes(), &mut Vec::new()).is_err());
        Ok(())
    }

    #[test]
    fn edits_go_through_the_file() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-policy-cli-{}", std::process::id()));
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
        };
        let outcome = (|| -> Result<()> {
            assert!(load_required(&dirs).is_err());
            assert!(allow(&dirs, "a.test", None).is_err(), "edits need init");
            assert_eq!(
                init(&dirs, Preset::LockedDown)?,
                format!(
                    "wrote {} (preset locked-down)\n",
                    dirs.policy_file().display()
                )
            );
            assert!(init(&dirs, Preset::Open).is_err(), "init is one-time");
            assert_eq!(
                allow(&dirs, "a.test,b.test", None)?,
                "allowed a.test, b.test\n"
            );
            assert_eq!(
                allow(&dirs, "a.test", Some("box"))?,
                "allowed a.test for sandbox box\n"
            );
            assert_eq!(
                deny(&dirs, "a.test,c.test", None)?,
                "denied a.test, c.test\n"
            );
            assert_eq!(
                allow(&dirs, "b.test", None)?,
                "allowed b.test (some were already there)\n"
            );
            assert_eq!(rm(&dirs, "c.test", None)?, "removed c.test\n");
            assert!(rm(&dirs, "c.test", None).is_err());
            assert_eq!(set_preset(&dirs, Preset::Open)?, "preset is now open\n");
            let policy = load_required(&dirs)?;
            assert_eq!(policy.preset, Preset::Open);
            let listed = ls(&policy, Some("box"));
            assert_eq!(
                listed,
                "preset open\nACTION  TARGET  SOURCE\ndeny    a.test  global\n\
                 allow   a.test  sandbox\nallow   b.test  global\n"
            );
            Ok(())
        })();
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        outcome
    }
}
