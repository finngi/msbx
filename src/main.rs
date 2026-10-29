mod agents;
mod app;
mod attach;
mod model;
mod models;
mod netlog;
mod paths;
mod policy;
mod policy_cli;
mod proxy;
mod sandbox;
mod secrets;
mod tui;
mod ui;
mod wire;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand};

use crate::agents::AgentId;
use crate::paths::Dirs;
use crate::policy::Preset;
use crate::policy_cli::Filter;

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
        /// Create or start the sandbox, but don't attach; attach later with the same command.
        #[arg(short, long, conflicts_with = "args")]
        detach: bool,
        /// Arguments passed to the agent, after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Keep an agent API key in the macOS Keychain, so it need not be exported in every shell.
    #[command(subcommand)]
    Secret(SecretAction),
    /// Network policy: which hosts sandboxes may reach (applied live by the msbx proxy).
    #[command(subcommand)]
    Policy(PolicyAction),
    /// The msbx network proxy that every sandbox's traffic goes through.
    #[command(subcommand)]
    Proxy(ProxyAction),
    /// Any command msbx does not define is msb's, so it is forwarded untouched.
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Subcommand)]
enum SecretAction {
    /// Store VAR in the Keychain; `security` prompts for the value, msbx never sees it.
    Set {
        #[arg(value_parser = secret_var_parser())]
        var: String,
    },
    /// Remove VAR from the Keychain.
    Rm {
        #[arg(value_parser = secret_var_parser())]
        var: String,
    },
}

/// `--sandbox NAME`: scope a rule to one sandbox instead of all of them.
#[derive(Args, Debug, PartialEq, Eq)]
struct Scope {
    #[arg(long, value_name = "NAME")]
    sandbox: Option<String>,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum PolicyAction {
    /// Create policy.toml with a preset (one-time; use `preset` to change it later).
    Init { preset: Preset },
    /// List the effective rules with their source (preset, global, sandbox).
    Ls {
        #[command(flatten)]
        scope: Scope,
    },
    /// Allow hosts: `example.com`, `*.example.com`, `**.example.com`, `**`, `host:443`, IP, CIDR.
    Allow {
        /// Comma-separated targets.
        targets: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Deny hosts (deny beats allow).
    Deny {
        targets: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Remove a rule.
    Rm {
        target: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Show whether a host or IP (with optional :port) would be allowed; exit 1 when blocked.
    Check {
        target: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Show the connections the proxy blocked and allowed.
    Log {
        /// Only this sandbox.
        name: Option<String>,
        #[arg(long, conflicts_with = "allowed")]
        blocked: bool,
        #[arg(long)]
        allowed: bool,
        /// Rows per section.
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Change the global preset; takes effect on the next connection.
    Preset { preset: Preset },
}

impl PolicyAction {
    /// Whether the action changes policy.toml, so the running proxy must reload it.
    fn writes(&self) -> bool {
        matches!(
            self,
            Self::Init { .. }
                | Self::Allow { .. }
                | Self::Deny { .. }
                | Self::Rm { .. }
                | Self::Preset { .. }
        )
    }
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum ProxyAction {
    /// Run the proxy in the foreground on 127.0.0.1:47447.
    Serve,
    /// Show whether the proxy is running, its version, pid and policy file.
    Status,
    /// Ask the running proxy to flush its log and exit.
    Stop,
}

/// Only the env vars the agent table forwards can be stored.
fn secret_var_parser() -> PossibleValuesParser {
    PossibleValuesParser::new(agents::secret_vars())
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
    let dirs = Dirs::from_env()?;
    match cli.command {
        None => {
            let project = project_folder(None)?;
            // Before any SDK call: secrets and the proxy token are read from msbx's environment
            // when a VM boots.
            secrets::resolve_environment(&dirs)?;
            sandbox::use_runtime(&attach::msb_path()?);
            if policy::load(&dirs.policy_file())?.is_some() {
                proxy::ensure_running(&dirs).await?;
            }
            tui::run(project, std::env::home_dir(), dirs).await?;
            Ok(0)
        }
        Some(Command::Agent {
            agent,
            path,
            detach,
            args,
        }) => {
            let project = project_folder(path)?;
            secrets::resolve_environment(&dirs)?;
            sandbox::use_runtime(&attach::msb_path()?);
            policy_cli::require_or_ask(&dirs)?;
            proxy::ensure_running(&dirs).await?;
            let agent = agent.agent();
            let name = sandbox::ensure(agent, &project, &|note| eprintln!("{note}")).await?;
            if detach {
                eprintln!(
                    "{name} is running; attach with `msbx agent {} {}`",
                    agent.id,
                    project.display()
                );
                return Ok(0);
            }
            attach::exec(&name, agent.command, &args).await
        }
        Some(Command::Secret(SecretAction::Set { var })) => secrets::set(&var),
        Some(Command::Secret(SecretAction::Rm { var })) => secrets::rm(&var),
        Some(Command::Policy(action)) => {
            let writes = action.writes();
            let code = run_policy(action, &dirs)?;
            if writes {
                // Apply the edit before returning, not on the proxy's next file check.
                proxy::reload(&dirs).await?;
            }
            Ok(code)
        }
        Some(Command::Proxy(ProxyAction::Serve)) => proxy::serve(dirs).await.map(|()| 0),
        Some(Command::Proxy(ProxyAction::Status)) => match proxy::status().await? {
            Some(status) => policy_cli::emit(&format!(
                "running: msbx {} (pid {}), policy {}\n",
                status.msbx,
                status.pid,
                status.policy.display()
            ))
            .map(|()| 0),
            None => policy_cli::emit("not running\n").map(|()| 1),
        },
        Some(Command::Proxy(ProxyAction::Stop)) => proxy::stop(&dirs).await.map(|()| 0),
        Some(Command::External(args)) => {
            // `msbx start NAME` and friends need the proxy token in msb's environment, and a
            // proxy to reach once the sandbox is up.
            secrets::resolve_environment(&dirs)?;
            if policy::load(&dirs.policy_file())?.is_some() {
                proxy::ensure_running(&dirs).await?;
            }
            attach::passthrough(&args)
        }
    }
}

fn run_policy(action: PolicyAction, dirs: &Dirs) -> Result<u8> {
    let text = match action {
        PolicyAction::Init { preset } => policy_cli::init(dirs, preset)?,
        PolicyAction::Ls { scope } => {
            policy_cli::ls(&policy_cli::load_required(dirs)?, scope.sandbox.as_deref())
        }
        PolicyAction::Allow { targets, scope } => {
            policy_cli::allow(dirs, &targets, scope.sandbox.as_deref())?
        }
        PolicyAction::Deny { targets, scope } => {
            policy_cli::deny(dirs, &targets, scope.sandbox.as_deref())?
        }
        PolicyAction::Rm { target, scope } => {
            policy_cli::rm(dirs, &target, scope.sandbox.as_deref())?
        }
        PolicyAction::Check { target, scope } => {
            let policy = policy_cli::load_required(dirs)?;
            let (line, allowed) = policy_cli::check(&policy, &target, scope.sandbox.as_deref())?;
            policy_cli::emit(&line)?;
            return Ok(u8::from(!allowed));
        }
        PolicyAction::Log {
            name,
            blocked,
            allowed,
            limit,
        } => {
            let filter = match (blocked, allowed) {
                (true, _) => Filter::Blocked,
                (_, true) => Filter::Allowed,
                _ => Filter::All,
            };
            let log = netlog::Log::load(&dirs.network_log())?;
            policy_cli::log(log.entries(), name.as_deref(), filter, limit, |secs| {
                netlog::local_time(secs, "%H:%M:%S %d-%b")
            })
        }
        PolicyAction::Preset { preset } => policy_cli::set_preset(dirs, preset)?,
    };
    policy_cli::emit(&text)?;
    Ok(0)
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
        let Some(Command::Agent {
            agent, path, args, ..
        }) = Cli::try_parse_from(line)?.command
        else {
            bail!("expected {line:?} to parse as the agent command");
        };
        Ok((agent, path, args))
    }

    #[test]
    fn agent_detach_skips_attaching_and_takes_no_agent_args() -> Result<()> {
        for line in [
            ["msbx", "agent", "codex", "-d"].as_slice(),
            &["msbx", "agent", "--detach", "pi", "/tmp"],
        ] {
            let Some(Command::Agent { detach, .. }) = Cli::try_parse_from(line)?.command else {
                bail!("expected {line:?} to parse as the agent command");
            };
            assert!(detach, "{line:?}");
        }
        let Some(Command::Agent { detach, .. }) =
            Cli::try_parse_from(["msbx", "agent", "codex"])?.command
        else {
            bail!("expected the agent command");
        };
        assert!(!detach, "attaching stays the default");
        assert!(
            Cli::try_parse_from(["msbx", "agent", "codex", "-d", "--", "--version"]).is_err(),
            "agent arguments make no sense without attaching"
        );
        Ok(())
    }

    fn policy_command(line: &[&str]) -> Result<PolicyAction> {
        let Some(Command::Policy(action)) = Cli::try_parse_from(line)?.command else {
            bail!("expected {line:?} to parse as a policy command");
        };
        Ok(action)
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
    fn secret_set_and_rm_take_a_known_var() -> Result<()> {
        let Some(Command::Secret(SecretAction::Set { var })) =
            Cli::try_parse_from(["msbx", "secret", "set", "OPENAI_API_KEY"])?.command
        else {
            bail!("expected secret set");
        };
        assert_eq!(var, "OPENAI_API_KEY");
        let Some(Command::Secret(SecretAction::Rm { var })) =
            Cli::try_parse_from(["msbx", "secret", "rm", "OPENAI_API_KEY"])?.command
        else {
            bail!("expected secret rm");
        };
        assert_eq!(var, "OPENAI_API_KEY");
        assert!(Cli::try_parse_from(["msbx", "secret"]).is_err());
        Ok(())
    }

    #[test]
    fn unknown_secret_var_lists_the_valid_names() -> Result<()> {
        let error = Cli::try_parse_from(["msbx", "secret", "set", "GITHUB_TOKEN"])
            .err()
            .context("an unknown var must not parse")?;
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        assert!(
            error
                .to_string()
                .contains("[possible values: OPENAI_API_KEY]"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn policy_commands_parse_with_scopes_and_filters() -> Result<()> {
        let global = Scope { sandbox: None };
        let boxed = || Scope {
            sandbox: Some("box".to_owned()),
        };
        assert_eq!(
            policy_command(&["msbx", "policy", "init", "locked-down"])?,
            PolicyAction::Init {
                preset: Preset::LockedDown
            }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "ls", "--sandbox", "box"])?,
            PolicyAction::Ls { scope: boxed() }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "allow", "a.test,b.test"])?,
            PolicyAction::Allow {
                targets: "a.test,b.test".to_owned(),
                scope: global
            }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "deny", "**", "--sandbox", "box"])?,
            PolicyAction::Deny {
                targets: "**".to_owned(),
                scope: boxed()
            }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "rm", "a.test", "--sandbox", "box"])?,
            PolicyAction::Rm {
                target: "a.test".to_owned(),
                scope: boxed()
            }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "check", "pypi.org:443"])?,
            PolicyAction::Check {
                target: "pypi.org:443".to_owned(),
                scope: Scope { sandbox: None }
            }
        );
        assert_eq!(
            policy_command(&["msbx", "policy", "log", "box", "--blocked", "--limit", "5"])?,
            PolicyAction::Log {
                name: Some("box".to_owned()),
                blocked: true,
                allowed: false,
                limit: Some(5)
            }
        );
        assert!(Cli::try_parse_from(["msbx", "policy", "log", "--blocked", "--allowed"]).is_err());
        assert_eq!(
            policy_command(&["msbx", "policy", "preset", "open"])?,
            PolicyAction::Preset {
                preset: Preset::Open
            }
        );
        let error = Cli::try_parse_from(["msbx", "policy", "init", "strict"])
            .err()
            .context("an unknown preset must not parse")?;
        assert!(
            error
                .to_string()
                .contains("[possible values: open, balanced, locked-down]"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn proxy_commands_parse() -> Result<()> {
        for (word, action) in [
            ("serve", ProxyAction::Serve),
            ("status", ProxyAction::Status),
            ("stop", ProxyAction::Stop),
        ] {
            let Some(Command::Proxy(parsed)) =
                Cli::try_parse_from(["msbx", "proxy", word])?.command
            else {
                bail!("expected proxy {word}");
            };
            assert_eq!(parsed, action);
        }
        assert!(Cli::try_parse_from(["msbx", "proxy"]).is_err());
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
        assert!(help.contains("policy"), "{help}");
        assert!(
            help.contains("Any other command is passed to msb"),
            "{help}"
        );
        Ok(())
    }
}
