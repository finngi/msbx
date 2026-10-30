//! The only way msbx runs git on the host, and only to read: a repository's config file as
//! data, and your global identity. Every call runs `/usr/bin/git` from `/` with a cleared
//! environment (a project folder on `PATH`, say through direnv, must not supply the binary),
//! hooks, fsmonitor, includes, prompts and network off (RFC 0001).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Never the `git` on `PATH`: a project can put one there.
const GIT: &str = "/usr/bin/git";

/// Command-line settings beat every config file, so a repository cannot undo them.
const OVERRIDES: [&str; 2] = ["core.hooksPath=/dev/null", "core.fsmonitor=false"];

/// `git` from `/`, with a cleared environment and nothing that could run or fetch: no hooks,
/// no fsmonitor, no prompts, no protocols, no lazy fetches, no lock files, no pager.
fn isolated() -> Command {
    let mut command = Command::new(GIT);
    command
        .current_dir("/")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_LAZY_FETCH", "1")
        .arg("--no-pager");
    for setting in OVERRIDES {
        command.arg("-c").arg(setting);
    }
    command
}

/// For reading one repository's files: no system or global config either, so only the named
/// file is parsed.
pub(crate) fn command() -> Command {
    let mut command = isolated();
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    command
}

/// For reading your own global config: `HOME` and `XDG_CONFIG_HOME` say where it is.
fn global_command() -> Command {
    let mut command = isolated();
    for var in ["HOME", "XDG_CONFIG_HOME"] {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    command
}

/// The settings in one config file that name a program, a path git reads, or another config
/// file: the list RFC 0001 reviews. Includes are not followed; `include.path` itself is listed.
/// A key set more than once (`--add`) keeps every value, in file order.
pub(crate) fn risky_settings(config: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let output = command()
        .args(["config", "--file"])
        .arg(config)
        .args(["--no-includes", "--null", "--list"])
        .output()
        .with_context(|| format!("running {GIT} config on {}", config.display()))?;
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

/// Entries of `git config --list --null`: `key\nvalue\0`; a key without `\n` has no value.
fn parse_list(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.split_once('\n').unwrap_or((entry, "")))
}

/// Whether git may run something, read another file, or send data elsewhere because of this
/// setting. Sections and variable names are case-insensitive in git; subsection names keep
/// their case. The table follows the git 2.56 manual (RFC 0001 "What git executes").
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
        // every signing setting (gpg.program, gpg.<format>.program, gpg.ssh.*), repository
        // extensions (layout), and the two protected-config sections an attacker may still try.
        "hook" | "pager" | "gpg" | "extensions" | "safe" | "protocol" => true,
        // `alias.x = !cmd` and the newer `alias.x.command = !cmd`.
        "alias" | "submodule" => shell || (section == "submodule" && last == "url"),
        "filter" => has_subsection,
        "diff" => {
            matches!(last, "external" | "tool")
                || (has_subsection && matches!(last, "command" | "textconv"))
        }
        "merge" => last == "tool" || (has_subsection && last == "driver"),
        "credential" => last == "helper",
        // Transport selects helpers (`ext::`, `<scheme>::`) and remote-side programs.
        "remote" => matches!(
            last,
            "uploadpack" | "receivepack" | "vcs" | "proxy" | "url" | "pushurl"
        ),
        "difftool" | "mergetool" | "browser" | "man" => matches!(last, "cmd" | "path"),
        "sequence" => last == "editor",
        "interactive" => last == "difffilter",
        "gc" => last == "recentobjectshook",
        "uploadpack" => last == "packobjectshook",
        "include" | "includeif" => last == "path",
        "sendemail" => last.ends_with("cmd") || last.ends_with("command") || last == "smtpserver",
        "trailer" => matches!(last, "cmd" | "command"),
        "tar" => last == "command",
        "web" | "help" | "instaweb" => matches!(last, "browser" | "httpd"),
        "init" => last == "templatedir",
        "url" => matches!(last, "insteadof" | "pushinsteadof"),
        "http" => last == "proxy",
        // These switch signing on, which runs gpg.program.
        "commit" | "tag" | "push" => last == "gpgsign",
        "log" => last == "showsignature",
        "lfs" => rest.contains("customtransfer") || last == "standalonetransferagent",
        _ => false,
    }
}

/// Your `user.name` and `user.email` from the global git config, when both are set.
pub(crate) fn identity() -> Option<(String, String)> {
    let get = |key: &str| {
        let output = global_command()
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

    fn env_of(command: &Command) -> Vec<(String, Option<String>)> {
        command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn config_reads_use_an_absolute_git_from_root_with_a_clean_environment() {
        let command = command();
        assert_eq!(command.get_program(), "/usr/bin/git");
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
        assert!(
            command.get_envs().count() > 0,
            "the environment is replaced"
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--no-pager",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false"
            ]
        );
        let env = env_of(&command);
        for (key, value) in [
            ("PATH", "/usr/bin:/bin"),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ALLOW_PROTOCOL", ""),
            ("GIT_NO_LAZY_FETCH", "1"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ] {
            assert!(
                env.contains(&(key.to_owned(), Some(value.to_owned()))),
                "{key}={value} in {env:?}"
            );
        }
        assert!(
            !env.iter().any(|(key, _)| key == "HOME"),
            "no home, so no global config"
        );
    }

    #[test]
    fn identity_reads_keep_only_home_and_xdg_from_the_environment() {
        let env = env_of(&global_command());
        assert!(
            !env.iter().any(|(key, _)| key == "GIT_CONFIG_GLOBAL"),
            "{env:?}"
        );
        assert!(env.iter().all(|(key, _)| {
            matches!(
                key.as_str(),
                "HOME"
                    | "XDG_CONFIG_HOME"
                    | "PATH"
                    | "GIT_OPTIONAL_LOCKS"
                    | "GIT_TERMINAL_PROMPT"
                    | "GIT_ALLOW_PROTOCOL"
                    | "GIT_NO_LAZY_FETCH"
            )
        }));
    }

    #[test]
    fn risky_keys_name_programs_paths_or_includes() {
        let risky = [
            ("core.hooksPath", ".husky/_"),
            ("core.fsmonitor", "true"),
            ("core.sshCommand", "evil"),
            ("core.pager", "evil"),
            ("core.askPass", "evil"),
            ("core.worktree", "../elsewhere"),
            ("hook.lint.command", "./x"),
            ("hook.lint.event", "pre-commit"),
            ("hook.pre-commit.enabled", "false"),
            ("alias.st", "!sh -c evil"),
            ("alias.st.command", "!sh -c evil"),
            ("filter.lfs.clean", "git-lfs clean -- %f"),
            ("filter.LFS.process", "evil"),
            ("diff.external", "evil"),
            ("diff.tool", "evil"),
            ("diff.word.command", "evil"),
            ("diff.word.textconv", "evil"),
            ("merge.tool", "evil"),
            ("merge.ours.driver", "true"),
            ("credential.helper", "!evil"),
            ("credential.https://x.helper", "store"),
            ("gpg.program", "evil"),
            ("gpg.format", "ssh"),
            ("gpg.ssh.defaultKeyCommand", "evil"),
            ("commit.gpgSign", "true"),
            ("tag.gpgSign", "true"),
            ("push.gpgSign", "true"),
            ("log.showSignature", "true"),
            ("extensions.worktreeConfig", "true"),
            ("remote.origin.url", "ext::sh -c evil"),
            ("remote.origin.pushurl", "evil::x"),
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
            ("sendemail.smtpServer", "/tmp/evil"),
            ("tar.tgz.command", "evil"),
            ("trailer.sign.cmd", "evil"),
            ("web.browser", "evil"),
            ("init.templateDir", "/tmp/t"),
            ("submodule.x.update", "!evil"),
            ("submodule.x.url", "ext::evil"),
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
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("branch.main.remote", "origin"),
            ("diff.renames", "true"),
            ("merge.conflictstyle", "diff3"),
            ("submodule.x.update", "checkout"),
            ("submodule.x.path", "x"),
            ("commit.verbose", "true"),
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

    /// Reruns the test above in a child process whose `PATH` starts with a folder holding a
    /// fake `git`; the fake must never run.
    #[cfg(unix)]
    #[test]
    fn hostile_path_does_not_select_a_project_executable() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("msbx-fake-git-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let outcome = (|| -> Result<(std::process::Output, bool)> {
            let marker = root.join("executed");
            let fake = root.join("git");
            std::fs::write(
                &fake,
                format!("#!/bin/sh\ntouch '{}'\nexit 91\n", marker.display()),
            )?;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))?;
            let output = Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "hostgit::tests::risky_settings_reads_one_file_without_includes",
                ])
                .env("PATH", format!("{}:/usr/bin:/bin", root.display()))
                .output()?;
            Ok((output, marker.exists()))
        })();
        std::fs::remove_dir_all(&root)?;
        let (output, executed) = outcome?;
        assert!(!executed, "a git from PATH was executed");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }
}
