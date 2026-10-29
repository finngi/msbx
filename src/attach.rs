//! Hands the terminal to a command inside a sandbox via `msb exec -t`.
//!
//! msb already does PTY setup, resize and raw mode well, so msbx just runs it with inherited
//! stdio. `tokio::process` is used because callers are async: waiting must not block a runtime
//! worker thread.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitStatus;

use anyhow::{Context, Result};

const MSB_MISSING: &str =
    "msb not found — install microsandbox: brew install superradcompany/tap/microsandbox";

/// The `msb` executable on `PATH`.
pub(crate) fn msb_path() -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context(MSB_MISSING)?;
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
