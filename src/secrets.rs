//! Agent API keys from the macOS Keychain, through Apple's `security` CLI. Host side only.
//!
//! The SDK reads a forwarded secret from msbx's own environment when it boots a VM, so a
//! Keychain value must be in that environment first. Rust 2024 makes `std::env::set_var`
//! unsafe (and this crate forbids `unsafe`), so msbx re-runs itself once with the values added
//! to the child's environment instead. The values are never printed, logged or passed as
//! arguments; `msbx secret set` lets `security` prompt for the value, so msbx never sees it.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::agents;

/// Keychain service name; the env var is the account.
const SERVICE: &str = "msbx";

/// Set in the re-run's environment so a second run never re-execs again.
const RESOLVED_GUARD: &str = "MSBX_KEYCHAIN_RESOLVED";

/// `msbx secret set VAR`: `security` prompts for the value on the terminal. Returns its exit code.
pub(crate) fn set(var: &str) -> Result<u8> {
    run_security(&set_args(var))
}

/// `msbx secret rm VAR`. Returns `security`'s exit code (44 when there is no such item).
pub(crate) fn rm(var: &str) -> Result<u8> {
    run_security(&rm_args(var))
}

fn run_security(args: &[&str]) -> Result<u8> {
    let status = Command::new("security")
        .args(args)
        .status()
        .context("running security (the macOS Keychain CLI)")?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

fn set_args(var: &str) -> [&str; 7] {
    // `-w` last and without a value makes `security` prompt for it (`add-generic-password -h`).
    ["add-generic-password", "-U", "-s", SERVICE, "-a", var, "-w"]
}

fn rm_args(var: &str) -> [&str; 5] {
    ["delete-generic-password", "-s", SERVICE, "-a", var]
}

fn find_args(var: &str) -> [&str; 6] {
    ["find-generic-password", "-s", SERVICE, "-a", var, "-w"]
}

/// Re-runs msbx with Keychain values for every agent secret var that is unset or empty in the
/// environment. Returns normally when there is nothing to add; on success it never returns.
/// A shell variable always wins over the Keychain. Not macOS, or no `security`: nothing happens.
pub(crate) fn resolve_from_keychain() -> Result<()> {
    if !cfg!(target_os = "macos") || std::env::var_os(RESOLVED_GUARD).is_some() {
        return Ok(());
    }
    let found: Vec<(&str, String)> =
        missing_vars(&agents::secret_vars(), |var| std::env::var(var).ok())
            .into_iter()
            .filter_map(|var| Some((var, keychain_value(var)?)))
            .collect();
    if found.is_empty() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("locating the msbx executable")?;
    let mut command = reexec_command(&exe, std::env::args_os().skip(1), &found);
    #[cfg(unix)]
    {
        let error = std::os::unix::process::CommandExt::exec(&mut command);
        Err(error).with_context(|| format!("re-running {} with Keychain secrets", exe.display()))
    }
    #[cfg(not(unix))]
    {
        drop(command);
        Ok(())
    }
}

/// The vars that `lookup` reports unset or empty, in the given order.
fn missing_vars<'a>(vars: &[&'a str], lookup: impl Fn(&str) -> Option<String>) -> Vec<&'a str> {
    vars.iter()
        .copied()
        .filter(|var| lookup(var).is_none_or(|value| value.is_empty()))
        .collect()
}

/// The Keychain item for `var`, or `None` when there is none, it is empty, or `security`
/// cannot run. Its stderr is dropped so a missing item stays silent.
fn keychain_value(var: &str) -> Option<String> {
    let output = Command::new("security")
        .args(find_args(var))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(trim_newline)
        .filter(|value| !value.is_empty())
}

/// `security -w` prints the value followed by one newline; values keep any other whitespace.
fn trim_newline(mut value: String) -> String {
    if value.ends_with('\n') {
        value.pop();
    }
    value
}

/// msbx again with the same arguments, the found values and the guard added to its environment.
fn reexec_command(
    exe: &Path,
    args: impl IntoIterator<Item = OsString>,
    found: &[(&str, String)],
) -> Command {
    let mut command = Command::new(exe);
    command
        .args(args)
        .envs(found.iter().map(|(var, value)| (*var, value.as_str())))
        .env(RESOLVED_GUARD, "1");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_arguments_name_the_service_and_account() {
        assert_eq!(
            set_args("OPENAI_API_KEY"),
            [
                "add-generic-password",
                "-U",
                "-s",
                "msbx",
                "-a",
                "OPENAI_API_KEY",
                "-w"
            ]
        );
        assert_eq!(
            rm_args("OPENAI_API_KEY"),
            [
                "delete-generic-password",
                "-s",
                "msbx",
                "-a",
                "OPENAI_API_KEY"
            ]
        );
        assert_eq!(
            find_args("OPENAI_API_KEY"),
            [
                "find-generic-password",
                "-s",
                "msbx",
                "-a",
                "OPENAI_API_KEY",
                "-w"
            ]
        );
    }

    #[test]
    fn only_unset_or_empty_vars_are_looked_up() {
        let vars = ["A", "B", "C"];
        let missing = missing_vars(&vars, |var| match var {
            "A" => Some("set".to_owned()),
            "B" => Some(String::new()),
            _ => None,
        });
        assert_eq!(missing, ["B", "C"]);
        assert_eq!(
            missing_vars(&vars, |_| Some("x".to_owned())),
            [] as [&str; 0]
        );
    }

    #[test]
    fn only_the_single_trailing_newline_is_trimmed() {
        assert_eq!(trim_newline("sk-1\n".to_owned()), "sk-1");
        assert_eq!(trim_newline("sk-1\n\n".to_owned()), "sk-1\n");
        assert_eq!(trim_newline(" sk-1 ".to_owned()), " sk-1 ");
        assert_eq!(trim_newline(String::new()), "");
    }

    #[test]
    fn reexec_keeps_args_and_adds_values_and_guard() {
        let args = ["agent", "codex", "--", "--version"].map(OsString::from);
        let found = [("OPENAI_API_KEY", "sk-test".to_owned())];
        let command = reexec_command(Path::new("/opt/bin/msbx"), args.clone(), &found);
        assert_eq!(command.get_program(), "/opt/bin/msbx");
        assert!(command.get_args().eq(args.iter().map(OsString::as_os_str)));
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("OPENAI_API_KEY".as_ref(), Some("sk-test".as_ref()))));
        assert!(envs.contains(&(RESOLVED_GUARD.as_ref(), Some("1".as_ref()))));
    }
}
