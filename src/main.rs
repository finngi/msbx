mod agents;
mod app;
mod attach;
mod model;
mod sandbox;
mod tui;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::agents::AgentId;

/// Run coding agents in microsandbox microVMs.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start (creating if needed) the agent's sandbox for a folder and attach to the agent.
    Run {
        agent: AgentId,
        /// Project folder to mount at the same path inside the sandbox [default: current dir].
        path: Option<PathBuf>,
        /// Arguments passed to the agent, after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
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
        Some(Command::Run { agent, path, args }) => {
            let project = project_folder(path)?;
            sandbox::use_runtime(&attach::msb_path()?);
            let agent = agent.agent();
            let name = sandbox::ensure(agent, &project).await?;
            attach::exec(&name, agent.command, &args).await
        }
    }
}

/// The canonical project folder, so the TUI and `msbx run` name and label sandboxes alike.
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
