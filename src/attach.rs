//! Hands the terminal to `msb`: `msb exec -t` to attach to a sandbox, and any msb command msbx
//! does not define itself.
//!
//! msb already does PTY setup, resize and raw mode well, so msbx just runs it with inherited
//! stdio. `tokio::process` is used for attach because callers are async: waiting must not block a
//! runtime worker thread.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use anyhow::{Context, Result};

const MSB_MISSING: &str =
    "msb not found — install microsandbox: curl -fsSL https://install.microsandbox.dev | sh";

/// The `msb` msbx runs and points the SDK at, in the SDK's own order (microsandbox 0.7.13
/// `setup/runtime.rs` `resolve_runtime_candidates`): `MSB_PATH`, then the runtime in
/// `$MSB_HOME` or `~/.microsandbox` that the official installer writes, then `msb` on `PATH`
/// (Homebrew). Running the same `msb` as the SDK keeps `msb exec` on the database format the
/// SDK writes.
pub(crate) fn msb_path() -> Result<PathBuf> {
    resolve_msb(
        std::env::var_os("MSB_PATH"),
        msb_home(),
        std::env::var_os("PATH"),
    )
}

/// `$MSB_HOME`, or `~/.microsandbox` (microsandbox-utils 0.7.13 `resolve_home`); an empty
/// `MSB_HOME` counts as unset.
fn msb_home() -> Option<PathBuf> {
    std::env::var_os("MSB_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".microsandbox")))
}

fn resolve_msb(
    msb_path: Option<OsString>,
    home: Option<PathBuf>,
    path: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(msb) = msb_path.filter(|msb| !msb.is_empty()) {
        return Ok(PathBuf::from(msb));
    }
    if let Some(msb) = home
        .map(|home| home.join("bin").join("msb"))
        .filter(|msb| msb.is_file())
    {
        return Ok(msb);
    }
    let path = path.context(MSB_MISSING)?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("msb"))
        .find(|candidate| candidate.is_file())
        .context(MSB_MISSING)
}

/// Runs `command args...` in sandbox `name` on the current terminal and returns its exit code.
pub(crate) async fn exec(name: &str, command: &str, args: &[String]) -> Result<u8> {
    let msb = msb_path()?;
    let mut child = tokio::process::Command::new(&msb)
        .args(exec_args(name, command, args))
        .spawn()
        .with_context(|| format!("running {}", msb.display()))?;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status.with_context(|| format!("waiting for msb exec in {name}"))?;
                return Ok(exit_code(status));
            }
            // The terminal sends Ctrl-C to msb as well; stay alive to report how it exits.
            signal = tokio::signal::ctrl_c() => {
                if signal.is_err() {
                    let status = child.wait().await.with_context(|| format!("waiting for msb exec in {name}"))?;
                    return Ok(exit_code(status));
                }
            }
        }
    }
}

/// Runs `msb args...` in place of msbx, so the terminal, signals and exit code are exactly msb's.
/// On Unix this only returns if msb could not be started.
pub(crate) fn passthrough(args: &[OsString]) -> Result<u8> {
    let msb = msb_path()?;
    let mut command = msb_command(&msb, args);
    #[cfg(unix)]
    {
        let error = std::os::unix::process::CommandExt::exec(&mut command);
        Err(error).with_context(|| format!("running {}", msb.display()))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .with_context(|| format!("running {}", msb.display()))?;
        Ok(exit_code(status))
    }
}

fn msb_command(msb: &Path, args: &[OsString]) -> std::process::Command {
    let mut command = std::process::Command::new(msb);
    command.args(args);
    command
}

fn exec_args(name: &str, command: &str, args: &[String]) -> Vec<OsString> {
    ["exec", "-t", name, "--", command]
        .into_iter()
        .map(OsString::from)
        .chain(args.iter().map(OsString::from))
        .collect()
}

/// Maps a child's status to a shell-style exit code: its own code, or 128 + signal if killed.
fn exit_code(status: ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return u8::try_from(code).unwrap_or(1);
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return u8::try_from(128 + signal).unwrap_or(1);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msb_is_found_in_the_sdks_order() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-msb-path-{}", std::process::id()));
        let home = root.join("home");
        let on_path = root.join("path");
        std::fs::create_dir_all(home.join("bin"))?;
        std::fs::create_dir_all(&on_path)?;
        std::fs::write(on_path.join("msb"), "")?;
        let path = Some(on_path.clone().into_os_string());
        let outcome = (|| -> Result<()> {
            assert_eq!(
                resolve_msb(None, Some(home.clone()), path.clone())?,
                on_path.join("msb"),
                "no home runtime: PATH"
            );
            std::fs::write(home.join("bin/msb"), "")?;
            assert_eq!(
                resolve_msb(None, Some(home.clone()), path.clone())?,
                home.join("bin/msb"),
                "the installer's runtime beats PATH, as in the SDK"
            );
            assert_eq!(
                resolve_msb(Some("/opt/msb".into()), Some(home.clone()), path.clone())?,
                PathBuf::from("/opt/msb")
            );
            assert_eq!(
                resolve_msb(Some("".into()), Some(home.clone()), path.clone())?,
                home.join("bin/msb"),
                "an empty MSB_PATH counts as unset"
            );
            assert!(resolve_msb(None, None, None).is_err());
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }

    #[test]
    fn exec_args_put_the_command_after_the_separator() {
        let args = exec_args(
            "shell-demo",
            "bash",
            &["-c".to_owned(), "echo hi".to_owned()],
        );
        assert_eq!(
            args,
            ["exec", "-t", "shell-demo", "--", "bash", "-c", "echo hi"]
        );
    }

    #[test]
    fn msb_command_forwards_args_verbatim() {
        let args = ["run", "alpine", "--", "sh", "-c", "echo 'a b'"].map(OsString::from);
        let command = msb_command(Path::new("/opt/bin/msb"), &args);
        assert_eq!(command.get_program(), "/opt/bin/msb");
        assert!(command.get_args().eq(args.iter().map(OsString::as_os_str)));
    }

    #[cfg(unix)]
    #[test]
    fn exit_codes_follow_shell_conventions() {
        use std::os::unix::process::ExitStatusExt;
        // Raw wait statuses: exit code in the high byte, terminating signal in the low bits.
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(42 << 8)), 42);
        assert_eq!(exit_code(ExitStatus::from_raw(2)), 130);
    }
}
