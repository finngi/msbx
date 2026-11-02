//! Host-side values the SDK reads from msbx's environment when it boots a VM: agent API keys
//! from the macOS Keychain (through Apple's `security` CLI) or, on Linux, the freedesktop Secret
//! Service (through libsecret's `secret-tool`, when it is installed), and the network proxy token.
//!
//! The SDK and `msb` resolve a secret or proxy password with `std::env::var` at spawn time, so
//! a value must be in that environment first. Rust 2024 makes `std::env::set_var` unsafe (and
//! this crate forbids `unsafe`), so msbx re-runs itself once with the values added to the
//! child's environment instead. The values are never printed, logged or passed as arguments;
//! `msbx secret set` lets `security` or `secret-tool` prompt for the value, so msbx never sees it.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::paths::Dirs;
use crate::{agents, proxy};

/// Keychain service name, and Secret Service `service` attribute; the env var is the account.
const SERVICE: &str = "msbx";

/// Where `msbx secret` keeps values on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    /// macOS Keychain, through `/usr/bin/security`.
    Keychain,
    /// freedesktop Secret Service (GNOME Keyring, `KWallet`, `KeePassXC`), through `secret-tool`.
    SecretService,
}

impl Store {
    /// The Keychain on macOS; the Secret Service on other Unixes when `secret-tool` is on `PATH`.
    fn find() -> Option<Self> {
        if cfg!(target_os = "macos") {
            return Some(Self::Keychain);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .any(|dir| dir.join("secret-tool").is_file())
            .then_some(Self::SecretService)
    }

    fn program(self) -> &'static str {
        match self {
            Self::Keychain => "security",
            Self::SecretService => "secret-tool",
        }
    }

    fn set_args(self, var: &str) -> Vec<String> {
        match self {
            // `-w` last and without a value makes `security` prompt for it.
            Self::Keychain => {
                strings(&["add-generic-password", "-U", "-s", SERVICE, "-a", var, "-w"])
            }
            // `store` reads the value from the terminal (or stdin) itself.
            Self::SecretService => strings(&[
                "store",
                &format!("--label=msbx {var}"),
                "service",
                SERVICE,
                "account",
                var,
            ]),
        }
    }

    fn rm_args(self, var: &str) -> Vec<String> {
        match self {
            Self::Keychain => strings(&["delete-generic-password", "-s", SERVICE, "-a", var]),
            Self::SecretService => strings(&["clear", "service", SERVICE, "account", var]),
        }
    }

    fn find_args(self, var: &str) -> Vec<String> {
        match self {
            Self::Keychain => strings(&["find-generic-password", "-s", SERVICE, "-a", var, "-w"]),
            Self::SecretService => strings(&["lookup", "service", SERVICE, "account", var]),
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn store() -> Result<Store> {
    let Some(store) = Store::find() else {
        bail!(
            "no key store: install secret-tool (Debian/Ubuntu: libsecret-tools, Fedora: \
             libsecret) with a Secret Service such as GNOME Keyring, or export the variable instead"
        );
    };
    Ok(store)
}

/// Set in the re-run's environment so a second run never re-execs again.
const RESOLVED_GUARD: &str = "MSBX_ENV_RESOLVED";

/// `msbx secret set VAR`: the store's CLI prompts for the value on the terminal. Returns its
/// exit code.
pub(crate) fn set(var: &str) -> Result<u8> {
    let store = store()?;
    run_store(store, &store.set_args(var))
}

/// `msbx secret rm VAR`. Returns the store CLI's exit code (`security`: 44 when there is no such
/// item).
pub(crate) fn rm(var: &str) -> Result<u8> {
    let store = store()?;
    run_store(store, &store.rm_args(var))
}

fn run_store(store: Store, args: &[String]) -> Result<u8> {
    let status = Command::new(store.program())
        .args(args)
        .status()
        .with_context(|| format!("running {}", store.program()))?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

/// Re-runs msbx with the proxy token and stored values (Keychain or Secret Service) for every
/// agent secret var that is unset or empty in the environment. Returns normally when there is nothing to add;
/// on success it never returns. A shell variable always wins.
pub(crate) fn resolve_environment(dirs: &Dirs) -> Result<()> {
    if std::env::var_os(RESOLVED_GUARD).is_some() {
        return Ok(());
    }
    let lookup = |var: &str| std::env::var(var).ok();
    let mut found: Vec<(&str, String)> = Vec::new();
    if let Some(store) = Store::find() {
        found.extend(
            missing_vars(&agents::secret_vars(), lookup)
                .into_iter()
                .filter_map(|var| Some((var, stored_value(store, var)?))),
        );
    }
    if !missing_vars(&[proxy::TOKEN_VAR], lookup).is_empty() {
        found.push((proxy::TOKEN_VAR, proxy::token(dirs)?));
    }
    if found.is_empty() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("locating the msbx executable")?;
    let mut command = reexec_command(&exe, std::env::args_os().skip(1), &found);
    #[cfg(unix)]
    {
        let error = std::os::unix::process::CommandExt::exec(&mut command);
        Err(error).with_context(|| format!("re-running {} with host-side values", exe.display()))
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

/// The stored item for `var`, or `None` when there is none, it is empty, or the store's CLI
/// cannot run. Its stderr is dropped so a missing item stays silent.
fn stored_value(store: Store, var: &str) -> Option<String> {
    let output = Command::new(store.program())
        .args(store.find_args(var))
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

/// `security -w` prints the value followed by one newline (`secret-tool lookup` adds one only on a
/// terminal); values keep any other whitespace.
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
        let set_args = |var| Store::Keychain.set_args(var);
        let rm_args = |var| Store::Keychain.rm_args(var);
        let find_args = |var| Store::Keychain.find_args(var);
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
    fn secret_tool_arguments_use_service_and_account_attributes() {
        let store = Store::SecretService;
        assert_eq!(store.program(), "secret-tool");
        assert_eq!(
            store.set_args("GH_TOKEN"),
            [
                "store",
                "--label=msbx GH_TOKEN",
                "service",
                "msbx",
                "account",
                "GH_TOKEN"
            ]
        );
        assert_eq!(
            store.rm_args("GH_TOKEN"),
            ["clear", "service", "msbx", "account", "GH_TOKEN"]
        );
        assert_eq!(
            store.find_args("GH_TOKEN"),
            ["lookup", "service", "msbx", "account", "GH_TOKEN"]
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
        let found = [
            ("OPENAI_API_KEY", "sk-test".to_owned()),
            (proxy::TOKEN_VAR, "tok".to_owned()),
        ];
        let command = reexec_command(Path::new("/opt/bin/msbx"), args.clone(), &found);
        assert_eq!(command.get_program(), "/opt/bin/msbx");
        assert!(command.get_args().eq(args.iter().map(OsString::as_os_str)));
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("OPENAI_API_KEY".as_ref(), Some("sk-test".as_ref()))));
        assert!(envs.contains(&("MSBX_PROXY_TOKEN".as_ref(), Some("tok".as_ref()))));
        assert!(envs.contains(&(RESOLVED_GUARD.as_ref(), Some("1".as_ref()))));
    }
}
