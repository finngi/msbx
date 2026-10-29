mod agents;
mod app;
mod attach;
mod model;
mod sandbox;
mod tui;
mod ui;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::agents::AgentId;

/// msbx (microsandbox extended): a dashboard and agent launcher for microsandbox microVMs.
#[derive(Parser)]
#[command(
    version,
    after_help = "Run msbx with no command to open the dashboard.\n\
                  Any other command is passed to msb: msbx ls, msbx logs NAME, msbx run IMAGE …"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start (creating if needed) the agent's sandbox for a folder and attach to the agent.
    Agent {
        agent: AgentId,
        /// Project folder to mount at the same path inside the sandbox [default: current dir].
        path: Option<PathBuf>,
        /// Arguments passed to the agent, after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Any command msbx does not define is msb's, so it is forwarded untouched.
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        None => {
            let project = project_folder(None)?;
            sandbox::use_runtime(&attach::msb_path()?);
            tui::run(project, std::env::home_dir()).await?;
            Ok(0)
        }
        Some(Command::Agent { agent, path, args }) => {
            let project = project_folder(path)?;
            sandbox::use_runtime(&attach::msb_path()?);
            let agent = agent.agent();
            let name = sandbox::ensure(agent, &project).await?;
            attach::exec(&name, agent.command, &args).await
        }
        Some(Command::External(args)) => attach::passthrough(&args),
    }
}

/// The canonical project folder, so the TUI and `msbx agent` name and label sandboxes alike.
fn project_folder(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path,
        None => std::env::current_dir().context("reading the current directory")?,
    };
    let project = path
        .canonicalize()
        .with_context(|| format!("resolving project folder {}", path.display()))?;
    if !project.is_dir() {
        bail!("{} is not a directory", project.display());
    }
    Ok(project)
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result, bail};
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn external(line: &[&str]) -> Result<Vec<OsString>> {
        let Some(Command::External(args)) = Cli::try_parse_from(line)?.command else {
            bail!("expected {line:?} to pass through to msb");
        };
        Ok(args)
    }

    fn agent_command(line: &[&str]) -> Result<(AgentId, Option<PathBuf>, Vec<String>)> {
        let Some(Command::Agent { agent, path, args }) = Cli::try_parse_from(line)?.command else {
            bail!("expected {line:?} to parse as the agent command");
        };
        Ok((agent, path, args))
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_means_tui() -> Result<()> {
        assert!(Cli::try_parse_from(["msbx"])?.command.is_none());
        Ok(())
    }

    #[test]
    fn agent_takes_an_agent_path_and_args() -> Result<()> {
        assert_eq!(
            agent_command(&["msbx", "agent", "codex"])?,
            (AgentId::Codex, None, vec![])
        );
        assert_eq!(
            agent_command(&["msbx", "agent", "pi", "/tmp"])?,
            (AgentId::Pi, Some(PathBuf::from("/tmp")), vec![])
        );
        assert_eq!(
            agent_command(&["msbx", "agent", "codex", "--", "--version"])?,
            (AgentId::Codex, None, vec!["--version".to_owned()])
        );
        Ok(())
    }

    #[test]
    fn unknown_agent_lists_the_valid_agents() -> Result<()> {
        let error = Cli::try_parse_from(["msbx", "agent", "claude"])
            .err()
            .context("an unknown agent must not parse")?;
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        assert!(
            error
                .to_string()
                .contains("[possible values: codex, pi, shell]"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn other_commands_pass_through_with_their_flags() -> Result<()> {
        assert_eq!(external(&["msbx", "ls", "--running"])?, ["ls", "--running"]);
        assert_eq!(
            external(&["msbx", "exec", "-t", "box", "--", "bash"])?,
            ["exec", "-t", "box", "--", "bash"]
        );
        assert_eq!(external(&["msbx", "logs", "x", "-f"])?, ["logs", "x", "-f"]);
        Ok(())
    }

    #[test]
    fn msb_run_passes_through_with_separator_and_quoted_args() -> Result<()> {
        assert_eq!(
            external(&["msbx", "run", "alpine", "--", "sh", "-c", "echo 'a b'"])?,
            ["run", "alpine", "--", "sh", "-c", "echo 'a b'"]
        );
        Ok(())
    }

    #[test]
    fn help_and_version_stay_with_msbx() -> Result<()> {
        for (flag, kind) in [
            ("--help", ErrorKind::DisplayHelp),
            ("-h", ErrorKind::DisplayHelp),
            ("--version", ErrorKind::DisplayVersion),
            ("-V", ErrorKind::DisplayVersion),
        ] {
            let error = Cli::try_parse_from(["msbx", flag])
                .err()
                .with_context(|| format!("{flag} must be handled by clap"))?;
            assert_eq!(error.kind(), kind, "{flag}");
        }
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("microsandbox extended"), "{help}");
        assert!(help.contains("agent"), "{help}");
        assert!(
            help.contains("Any other command is passed to msb"),
            "{help}"
        );
        Ok(())
    }
}
