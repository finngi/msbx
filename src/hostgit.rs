//! The only way msbx runs git on the host. Every call turns off hooks, fsmonitor, config includes
//! and prompts, so a setting an agent planted in a repository can never run through msbx itself
//! (RFC 0001). msbx only ever reads with git: a repository's config file and your identity.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Command-line settings beat every config file, so a repository cannot undo them.
const OVERRIDES: [&str; 2] = ["core.hooksPath=/dev/null", "core.fsmonitor=false"];

/// `git` with hooks and fsmonitor off, no lock files written and no credential prompts.
pub(crate) fn command() -> Command {
    let mut command = Command::new("git");
    for setting in OVERRIDES {
        command.arg("-c").arg(setting);
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR");
    command
}

/// The settings in one config file that name a program, a path git reads, or another config
/// file: the list RFC 0001 reviews. Includes are not followed; `include.path` itself is listed.
/// A key set more than once (`--add`) keeps every value, in file order.
pub(crate) fn risky_settings(config: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let output = command()
        .args(["config", "--file"])
        .arg(config)
        .args(["--list", "-z", "--no-includes"])
        .output()
        .with_context(|| format!("running git config on {}", config.display()))?;
    if !output.status.success() {
        bail!(
            "git config --file {} failed: {}",
            config.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut settings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in parse_list(&String::from_utf8_lossy(&output.stdout)) {
        if is_risky(key, value) {
            settings
                .entry(key.to_owned())
                .or_default()
                .push(value.to_owned());
        }
    }
    Ok(settings)
}

/// Entries of `git config --list -z`: `key\nvalue\0`; a key without `\n` has no value.
fn parse_list(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.split_once('\n').unwrap_or((entry, "")))
}

/// Whether git may run something, or read another file, because of this setting. Sections and
/// variable names are case-insensitive in git; subsection names keep their case.
pub(crate) fn is_risky(key: &str, value: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let Some((section, rest)) = lower.split_once('.') else {
        return false;
    };
    let last = rest.rsplit('.').next().unwrap_or(rest);
    let has_subsection = rest.contains('.');
    let shell = value.trim_start().starts_with('!');
    match section {
        "core" => matches!(
            last,
            "hookspath"
                | "fsmonitor"
                | "fsmonitorhookversion"
                | "sshcommand"
                | "gitproxy"
                | "askpass"
                | "editor"
                | "pager"
                | "alternaterefscommand"
                | "worktree"
                | "attributesfile"
        ),
        // Config-based hooks (git 2.54+: hook.<name>.command, .event, .enabled), every pager,
        // and the two protected-config sections an attacker may still try.
        "hook" | "pager" | "safe" | "protocol" => true,
        "alias" | "submodule" => shell,
        "filter" => has_subsection,
        "diff" => last == "external" || (has_subsection && matches!(last, "command" | "textconv")),
        "merge" => has_subsection && last == "driver",
        "credential" => last == "helper",
        "gpg" => matches!(last, "program" | "defaultkeycommand"),
        "remote" => matches!(last, "uploadpack" | "receivepack" | "vcs" | "proxy"),
        "difftool" | "mergetool" | "browser" | "man" => matches!(last, "cmd" | "path"),
        "sequence" => last == "editor",
        "interactive" => last == "difffilter",
        "gc" => last == "recentobjectshook",
        "uploadpack" => last == "packobjectshook",
        "include" | "includeif" => last == "path",
        "sendemail" => last.ends_with("cmd"),
        "trailer" => matches!(last, "cmd" | "command"),
        "web" | "help" | "instaweb" => matches!(last, "browser" | "httpd"),
        "init" => last == "templatedir",
        "url" => matches!(last, "insteadof" | "pushinsteadof"),
        "http" => last == "proxy",
        "lfs" => rest.contains("customtransfer") || last == "standalonetransferagent",
        _ => false,
    }
}

/// Your `user.name` and `user.email` from the global git config, when both are set.
pub(crate) fn identity() -> Option<(String, String)> {
    let get = |key: &str| {
        let output = command()
            .args(["config", "--global", "--get", key])
            .output()
            .ok()?;
        let value = String::from_utf8(output.stdout).ok()?;
        let value = value.trim();
        (output.status.success() && !value.is_empty()).then(|| value.to_owned())
    };
    Some((get("user.name")?, get("user.email")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_turns_hooks_and_fsmonitor_off() {
        let command = command();
        assert_eq!(command.get_program(), "git");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false"
            ]
        );
        let env: Vec<_> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(env.contains(&("GIT_OPTIONAL_LOCKS".to_owned(), Some("0".to_owned()))));
        assert!(env.contains(&("GIT_TERMINAL_PROMPT".to_owned(), Some("0".to_owned()))));
        assert!(env.contains(&("GIT_DIR".to_owned(), None)), "{env:?}");
    }

    #[test]
    fn risky_keys_name_programs_paths_or_includes() {
        let risky = [
            ("core.hooksPath", ".husky/_"),
            ("core.fsmonitor", "true"),
            ("core.sshCommand", "evil"),
            ("core.pager", "evil"),
            ("core.worktree", "../elsewhere"),
            ("hook.lint.command", "./x"),
            ("hook.lint.event", "pre-commit"),
            ("alias.st", "!sh -c evil"),
            ("filter.lfs.clean", "git-lfs clean -- %f"),
            ("filter.LFS.process", "evil"),
            ("diff.external", "evil"),
            ("diff.word.command", "evil"),
            ("diff.word.textconv", "evil"),
            ("merge.ours.driver", "true"),
            ("credential.helper", "!evil"),
            ("credential.https://x.helper", "store"),
            ("gpg.program", "evil"),
            ("gpg.ssh.defaultKeyCommand", "evil"),
            ("remote.origin.uploadpack", "/tmp/evil"),
            ("remote.origin.vcs", "evil"),
            ("difftool.x.cmd", "evil"),
            ("mergetool.x.path", "evil"),
            ("pager.log", "evil"),
            ("sequence.editor", "evil"),
            ("interactive.diffFilter", "evil"),
            ("gc.recentObjectsHook", "evil"),
            ("uploadpack.packObjectsHook", "evil"),
            ("include.path", "../evil"),
            ("includeIf.gitdir:/x.path", "../evil"),
            ("sendemail.toCmd", "evil"),
            ("trailer.sign.cmd", "evil"),
            ("web.browser", "evil"),
            ("init.templateDir", "/tmp/t"),
            ("submodule.x.update", "!evil"),
            ("url.https://evil/.insteadOf", "https://github.com/"),
            ("http.proxy", "http://evil"),
            ("lfs.customtransfer.x.path", "evil"),
            ("lfs.standaloneTransferAgent", "x"),
            ("safe.directory", "*"),
            ("protocol.ext.allow", "always"),
        ];
        for (key, value) in risky {
            assert!(is_risky(key, value), "{key} must be risky");
        }
        let plain = [
            ("core.repositoryformatversion", "0"),
            ("core.filemode", "true"),
            ("core.ignorecase", "true"),
            ("user.email", "a@b.c"),
            ("alias.co", "checkout"),
            ("remote.origin.url", "https://github.com/x/y"),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("branch.main.remote", "origin"),
            ("diff.renames", "true"),
            ("merge.conflictstyle", "diff3"),
            ("submodule.x.update", "checkout"),
            ("lfs.url", "https://x"),
            ("nodot", "x"),
        ];
        for (key, value) in plain {
            assert!(!is_risky(key, value), "{key} must not be risky");
        }
    }

    #[test]
    fn list_entries_split_on_nul_and_first_newline() {
        let entries: Vec<_> = parse_list("a.b\n1\n2\0c.d\0\0").collect();
        assert_eq!(entries, [("a.b", "1\n2"), ("c.d", "")]);
    }

    /// Runs real git on a temporary repository.
    #[test]
    fn risky_settings_reads_one_file_without_includes() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-hostgit-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let outcome = (|| -> Result<()> {
            let included = root.join("included");
            std::fs::write(&included, "[core]\n\tsshCommand = from-include\n")?;
            let config = root.join("config");
            std::fs::write(
                &config,
                "[core]\n\trepositoryformatversion = 0\n\tfsmonitor = \"./watch.sh\"\n\
                 [include]\n\tpath = included\n\tpath = other\n[alias]\n\tco = checkout\n\
                 \tx = !evil\n",
            )?;
            let risky = risky_settings(&config)?;
            let expected: BTreeMap<String, Vec<String>> = [
                ("core.fsmonitor", vec!["./watch.sh"]),
                ("include.path", vec!["included", "other"]),
                ("alias.x", vec!["!evil"]),
            ]
            .into_iter()
            .map(|(key, values)| {
                (
                    key.to_owned(),
                    values.into_iter().map(str::to_owned).collect(),
                )
            })
            .collect();
            assert_eq!(risky, expected);
            assert!(risky_settings(&root.join("missing")).is_err());
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }
}
