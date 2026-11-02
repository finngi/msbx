//! The agents msbx knows how to launch: Docker Sandboxes' (`sbx run --help`) plus pi, with sbx's
//! names. Each is installed inside the VM at create from its vendor's own npm package, release or
//! installer; msbx never redistributes a binary. Adding one is a new `static` plus an `AgentId`
//! variant. Versions, flags and file paths were checked in a `node:24-bookworm` microVM on
//! 2026-10-30 (RFC 0012).

use clap::ValueEnum;
use serde::Deserialize;

/// OCI image a sandbox boots from unless `config.toml` names another.
pub(crate) const IMAGE: &str = "node:24-bookworm";

/// Node.js installed from nodejs.org when an npm agent's image has no `npm` (latest 24.x LTS on
/// 2026-10-30).
pub(crate) const NODE_VERSION: &str = "v24.22.2";

/// Debian packages every install needs; the install script adds those an image lacks.
pub(crate) const BASE_PACKAGES: [&str; 4] = ["ca-certificates", "curl", "git", "bash"];

/// A host secret forwarded to the sandbox: the guest sees a placeholder in `env_var`, and the
/// network proxy substitutes the real value only on requests to one of `hosts`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Secret {
    pub(crate) env_var: &'static str,
    pub(crate) hosts: &'static [&'static str],
}

/// A file holding an agent's sign-in inside the VM, relative to the agent user's home (`dir` is
/// empty for a file directly in it). msbx links it into the shared login volume so one sign-in
/// serves every sandbox of the agent (RFC 0004).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Login {
    pub(crate) dir: &'static str,
    pub(crate) file: &'static str,
}

/// How an agent gets into a fresh sandbox.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Install {
    /// Nothing beyond the base packages.
    Nothing,
    /// `npm install -g` of this exact package version, as root.
    Npm(&'static str),
    /// A script run as root.
    Root(&'static str),
    /// The vendor's installer, run as the agent user. It puts `bins` in `~/.local/bin`, which
    /// root then links into `/usr/local/bin` so `msb exec` finds them on its `PATH`.
    Script {
        url: &'static str,
        bins: &'static [&'static str],
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Agent {
    /// Stable identifier used in sandbox names, labels, the CLI and `config.toml`.
    pub(crate) id: &'static str,
    pub(crate) description: &'static str,
    pub(crate) install: Install,
    /// Debian packages the install needs besides [`BASE_PACKAGES`].
    pub(crate) packages: &'static [&'static str],
    pub(crate) command: &'static str,
    /// Always passed first (kiro's `chat`, docker-agent's `run`), unless replaced (see
    /// [`launch_args`]).
    pub(crate) default_args: &'static [&'static str],
    /// The flag that skips the agent's permission prompts, passed by default like sbx does.
    pub(crate) auto_approve: &'static [&'static str],
    /// Each var is forwarded when set on the host; any one of them is enough.
    pub(crate) secrets: &'static [Secret],
    /// The first file is the credential whose presence means "signed in"; any others are
    /// companions shared with it.
    pub(crate) logins: &'static [Login],
    /// Path, relative to the agent user's home, of a pi-format `models.json` listing the host's
    /// local model servers.
    pub(crate) models_file: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AgentId {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Devin,
    DockerAgent,
    Droid,
    Gemini,
    Kiro,
    Opencode,
    Pi,
    Shell,
}

impl AgentId {
    pub(crate) fn agent(self) -> &'static Agent {
        match self {
            Self::Claude => &CLAUDE,
            Self::Codex => &CODEX,
            Self::Copilot => &COPILOT,
            Self::Cursor => &CURSOR,
            Self::Devin => &DEVIN,
            Self::DockerAgent => &DOCKER_AGENT,
            Self::Droid => &DROID,
            Self::Gemini => &GEMINI,
            Self::Kiro => &KIRO,
            Self::Opencode => &OPENCODE,
            Self::Pi => &PI,
            Self::Shell => &SHELL,
        }
    }
}

/// The agent with this id, as stored in the `msbx.agent` label.
pub(crate) fn find(id: &str) -> Option<&'static Agent> {
    AgentId::from_str(id, false).ok().map(AgentId::agent)
}

// Secrets. Hosts are the vendor's documented API host where there is one, else Docker Sandboxes'
// built-in list (https://docs.docker.com/ai/sandboxes/configuration/credentials/). A var used by
// several agents is one constant, so it always reaches the same hosts.

const ANTHROPIC_API_KEY: Secret = Secret {
    env_var: "ANTHROPIC_API_KEY",
    hosts: &["api.anthropic.com"],
};
/// A long-lived subscription token from `claude setup-token` on the host.
const CLAUDE_CODE_OAUTH_TOKEN: Secret = Secret {
    env_var: "CLAUDE_CODE_OAUTH_TOKEN",
    hosts: &["api.anthropic.com"],
};
const OPENAI_API_KEY: Secret = Secret {
    env_var: "OPENAI_API_KEY",
    hosts: &["api.openai.com"],
};
const GEMINI_API_KEY: Secret = Secret {
    env_var: "GEMINI_API_KEY",
    hosts: &["generativelanguage.googleapis.com"],
};
/// Gemini API or Vertex AI express mode.
const GOOGLE_API_KEY: Secret = Secret {
    env_var: "GOOGLE_API_KEY",
    hosts: &[
        "generativelanguage.googleapis.com",
        "aiplatform.googleapis.com",
    ],
};
/// Copilot's token hosts: GitHub's API and the Copilot API per plan.
const COPILOT_HOSTS: &[&str] = &[
    "api.github.com",
    "api.githubcopilot.com",
    "api.individual.githubcopilot.com",
    "api.business.githubcopilot.com",
    "api.enterprise.githubcopilot.com",
];
const COPILOT_GITHUB_TOKEN: Secret = Secret {
    env_var: "COPILOT_GITHUB_TOKEN",
    hosts: COPILOT_HOSTS,
};
const GH_TOKEN: Secret = Secret {
    env_var: "GH_TOKEN",
    hosts: COPILOT_HOSTS,
};
const GITHUB_TOKEN: Secret = Secret {
    env_var: "GITHUB_TOKEN",
    hosts: COPILOT_HOSTS,
};
const CURSOR_API_KEY: Secret = Secret {
    env_var: "CURSOR_API_KEY",
    hosts: &["api2.cursor.sh", "api3.cursor.sh", "repo42.cursor.sh"],
};
const FACTORY_API_KEY: Secret = Secret {
    env_var: "FACTORY_API_KEY",
    hosts: &["api.factory.ai", "app.factory.ai", "relay.factory.ai"],
};
/// `OpenCode` Zen (<https://docs.docker.com/ai/sandboxes/agents/opencode/>).
const OPENCODE_API_KEY: Secret = Secret {
    env_var: "OPENCODE_API_KEY",
    hosts: &["opencode.ai"],
};

static CLAUDE: Agent = Agent {
    id: "claude",
    description: "Claude Code; API key, token or sign-in",
    install: Install::Npm("@anthropic-ai/claude-code@2.1.314"),
    packages: &[],
    command: "claude",
    default_args: &[],
    auto_approve: &["--dangerously-skip-permissions"],
    secrets: &[ANTHROPIC_API_KEY, CLAUDE_CODE_OAUTH_TOKEN],
    // On Linux the token is in ~/.claude/.credentials.json and the account in ~/.claude.json
    // (https://code.claude.com/docs/en/setup).
    logins: &[
        Login {
            dir: ".claude",
            file: ".credentials.json",
        },
        Login {
            dir: "",
            file: ".claude.json",
        },
    ],
    models_file: None,
};

static CODEX: Agent = Agent {
    id: "codex",
    description: "OpenAI Codex; API key or sign-in",
    install: Install::Npm("@openai/codex@0.166.0"),
    packages: &[],
    command: "codex",
    default_args: &[],
    auto_approve: &["--dangerously-bypass-approvals-and-sandbox"],
    secrets: &[OPENAI_API_KEY],
    // Codex rewrites auth.json in place (truncate + write), so a link survives refreshes.
    logins: &[Login {
        dir: ".codex",
        file: "auth.json",
    }],
    models_file: None,
};

static COPILOT: Agent = Agent {
    id: "copilot",
    description: "GitHub Copilot; GitHub token or sign-in",
    install: Install::Npm("@github/copilot@1.0.105"),
    packages: &[],
    command: "copilot",
    default_args: &[],
    auto_approve: &["--yolo"],
    // In this order of precedence; classic `ghp_` tokens are not supported.
    secrets: &[COPILOT_GITHUB_TOKEN, GH_TOKEN, GITHUB_TOKEN],
    // Without a system keychain the token is saved in plain text here.
    logins: &[Login {
        dir: ".copilot",
        file: "config.json",
    }],
    models_file: None,
};

static CURSOR: Agent = Agent {
    id: "cursor",
    description: "Cursor CLI; API key or sign-in",
    install: Install::Script {
        url: "https://cursor.com/install",
        bins: &["agent", "cursor-agent"],
    },
    packages: &[],
    command: "cursor-agent",
    default_args: &[],
    auto_approve: &["--yolo"],
    secrets: &[CURSOR_API_KEY],
    // `$XDG_CONFIG_HOME/cursor/auth.json` on Linux, written in place (traced in the VM).
    logins: &[Login {
        dir: ".config/cursor",
        file: "auth.json",
    }],
    models_file: None,
};

static DEVIN: Agent = Agent {
    id: "devin",
    description: "Devin CLI; sign-in",
    install: Install::Script {
        url: "https://cli.devin.ai/install.sh",
        bins: &["devin"],
    },
    packages: &[],
    command: "devin",
    default_args: &[],
    auto_approve: &[
        "--permission-mode",
        "dangerous",
        "--respect-workspace-trust=false",
    ],
    secrets: &[],
    // `devin auth status` names this file.
    logins: &[Login {
        dir: ".local/share/devin",
        file: "credentials.toml",
    }],
    models_file: None,
};

/// docker-agent v1.154.0 from GitHub releases, checked against the release's SHA-256 digests.
const DOCKER_AGENT_INSTALL: &str = r#"case "$(uname -m)" in
  x86_64) arch=amd64 sum=6098684e7559b784c32f88c16dca062d7f00f281793f0c0c0a31a9b755c9b7da ;;
  aarch64 | arm64) arch=arm64 sum=6f0e8a7fa25ad90e54b6381bb81387ce53bc692110b683f7441cfc79cc2d6342 ;;
  *) echo "no docker-agent build for $(uname -m)" >&2; exit 1 ;;
esac
dir=$(mktemp -d)
curl -fsSL -o "$dir/docker-agent" "https://github.com/docker/docker-agent/releases/download/v1.154.0/docker-agent-linux-$arch"
echo "$sum  $dir/docker-agent" | sha256sum -c -
install -m 755 "$dir/docker-agent" /usr/local/bin/docker-agent
rm -rf "$dir""#;

static DOCKER_AGENT: Agent = Agent {
    id: "docker-agent",
    description: "Docker Agent; provider API keys",
    install: Install::Root(DOCKER_AGENT_INSTALL),
    packages: &[],
    command: "docker-agent",
    default_args: &["run"],
    auto_approve: &["--yolo"],
    secrets: &[OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY],
    logins: &[],
    models_file: None,
};

static DROID: Agent = Agent {
    id: "droid",
    description: "Factory Droid; API key or sign-in",
    install: Install::Npm("droid@1.0.0"),
    packages: &[],
    command: "droid",
    default_args: &[],
    // Interactive droid has no skip-all flag; `high` is its most autonomous level.
    auto_approve: &["--auto", "high"],
    secrets: &[FACTORY_API_KEY],
    // Without a keyring droid keeps the encrypted credential and its key side by side
    // (strings in the droid 1.0.0 binary: "auth.v2.file exists but auth.v2.key is missing").
    logins: &[
        Login {
            dir: ".factory",
            file: "auth.v2.file",
        },
        Login {
            dir: ".factory",
            file: "auth.v2.key",
        },
    ],
    models_file: None,
};

static GEMINI: Agent = Agent {
    id: "gemini",
    description: "Gemini CLI; API key or sign-in",
    install: Install::Npm("@google/gemini-cli@0.66.0"),
    packages: &[],
    command: "gemini",
    default_args: &[],
    auto_approve: &["--approval-mode=yolo"],
    secrets: &[GEMINI_API_KEY, GOOGLE_API_KEY],
    logins: &[Login {
        dir: ".gemini",
        file: "oauth_creds.json",
    }],
    models_file: None,
};

static KIRO: Agent = Agent {
    id: "kiro",
    description: "Kiro CLI; sign in in each sandbox",
    install: Install::Script {
        url: "https://cli.kiro.dev/install",
        bins: &["kiro-cli", "kiro-cli-chat", "kiro-cli-term"],
    },
    packages: &["unzip"],
    command: "kiro-cli",
    default_args: &["chat"],
    auto_approve: &["--trust-all-tools"],
    secrets: &[],
    // Kiro keeps its sign-in inside a SQLite database with other state
    // (~/.local/share/kiro-cli/data.sqlite3), which a link to a shared file cannot safely share.
    logins: &[],
    models_file: None,
};

static OPENCODE: Agent = Agent {
    id: "opencode",
    description: "OpenCode; Zen or provider key, sign-in",
    install: Install::Npm("opencode-ai@1.19.5"),
    packages: &[],
    command: "opencode",
    default_args: &[],
    auto_approve: &["--auto"],
    secrets: &[
        OPENCODE_API_KEY,
        ANTHROPIC_API_KEY,
        OPENAI_API_KEY,
        GEMINI_API_KEY,
    ],
    logins: &[Login {
        dir: ".local/share/opencode",
        file: "auth.json",
    }],
    models_file: None,
};

static PI: Agent = Agent {
    id: "pi",
    description: "pi; local models, shared sign-in",
    install: Install::Npm("@earendil-works/pi-coding-agent@1.4.0"),
    packages: &[],
    command: "pi",
    default_args: &[],
    // pi asks for no permissions.
    auto_approve: &[],
    // Pi talks to local models on the host, so there is nothing to forward.
    secrets: &[],
    // pi 1.4.0 writes auth.json in place under a lock file beside it, so a link survives.
    logins: &[Login {
        dir: ".pi/agent",
        file: "auth.json",
    }],
    models_file: Some(".pi/agent/models.json"),
};

static SHELL: Agent = Agent {
    id: "shell",
    description: "Bash, git, curl, Python, gcc; no agent",
    install: Install::Nothing,
    packages: &[],
    command: "bash",
    default_args: &[],
    auto_approve: &[],
    secrets: &[],
    logins: &[],
    models_file: None,
};

/// The command line after the agent's command. Like sbx: arguments that start with a flag are
/// added after the defaults (`default_args`, the auto-approve flag unless `auto_approve` is off,
/// then `config_args`); arguments that start with a word, such as a subcommand, replace them.
pub(crate) fn launch_args(
    agent: &Agent,
    auto_approve: bool,
    config_args: &[String],
    user_args: &[String],
) -> Vec<String> {
    if user_args.first().is_some_and(|arg| !arg.starts_with('-')) {
        return user_args.to_vec();
    }
    let approve: &[&str] = if auto_approve {
        agent.auto_approve
    } else {
        &[]
    };
    agent
        .default_args
        .iter()
        .chain(approve)
        .map(|arg| (*arg).to_owned())
        .chain(config_args.iter().cloned())
        .chain(user_args.iter().cloned())
        .collect()
}

fn all_secrets() -> impl Iterator<Item = &'static Secret> {
    AgentId::value_variants()
        .iter()
        .flat_map(|id| id.agent().secrets)
}

/// Every env var any agent forwards as a secret, in table order without repeats.
pub(crate) fn secret_vars() -> Vec<&'static str> {
    let mut vars = Vec::new();
    for secret in all_secrets() {
        if !vars.contains(&secret.env_var) {
            vars.push(secret.env_var);
        }
    }
    vars
}

/// Every host any agent's secret may be sent to, in table order without repeats.
pub(crate) fn secret_hosts() -> Vec<&'static str> {
    let mut hosts = Vec::new();
    for host in all_secrets().flat_map(|secret| secret.hosts) {
        if !hosts.contains(host) {
            hosts.push(*host);
        }
    }
    hosts
}

/// Splits an agent's secrets into those set on the host and the names of those that are not.
/// Only presence is checked: the value itself is read by the SDK from the host environment each
/// time the sandbox starts. An empty value counts as unset because the SDK rejects it at start.
pub(crate) fn select_secrets(
    agent: &Agent,
    lookup: impl Fn(&str) -> Option<String>,
) -> (Vec<&'static Secret>, Vec<&'static str>) {
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for secret in agent.secrets {
        if lookup(secret.env_var).is_some_and(|value| !value.is_empty()) {
            present.push(secret);
        } else {
            missing.push(secret.env_var);
        }
    }
    (present, missing)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anyhow::{Context, Result};

    use super::*;

    fn all() -> impl Iterator<Item = &'static Agent> {
        AgentId::value_variants().iter().map(|id| id.agent())
    }

    #[test]
    fn agent_ids_match_cli_names_and_sbx() -> Result<()> {
        let mut names = Vec::new();
        for variant in AgentId::value_variants() {
            let cli_name = variant
                .to_possible_value()
                .context("no variant is skipped")?
                .get_name()
                .to_owned();
            assert_eq!(variant.agent().id, cli_name);
            names.push(cli_name);
        }
        assert_eq!(
            names,
            [
                "claude",
                "codex",
                "copilot",
                "cursor",
                "devin",
                "docker-agent",
                "droid",
                "gemini",
                "kiro",
                "opencode",
                "pi",
                "shell"
            ]
        );
        Ok(())
    }

    #[test]
    fn find_resolves_ids_and_rejects_unknown() {
        assert_eq!(find("codex"), Some(&CODEX));
        assert_eq!(find("docker-agent"), Some(&DOCKER_AGENT));
        assert_eq!(find("shell"), Some(&SHELL));
        assert_eq!(find("Codex"), None, "labels are written lowercase");
        assert_eq!(find("amp"), None);
    }

    #[test]
    fn npm_installs_pin_an_exact_version() {
        for agent in all() {
            if let Install::Npm(spec) = agent.install {
                let (name, version) = spec.rsplit_once('@').unwrap_or_default();
                assert!(!name.is_empty(), "{spec}");
                assert!(
                    version.split('.').count() == 3
                        && version
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || byte == b'.'),
                    "{spec}"
                );
            }
        }
    }

    #[test]
    fn a_var_reaches_the_same_hosts_for_every_agent() {
        let mut hosts: BTreeMap<&str, &[&str]> = BTreeMap::new();
        for secret in all_secrets() {
            assert!(!secret.hosts.is_empty(), "{}", secret.env_var);
            let first = *hosts.entry(secret.env_var).or_insert(secret.hosts);
            assert_eq!(first, secret.hosts, "{}", secret.env_var);
        }
    }

    #[test]
    fn login_files_have_unique_names_on_the_volume() {
        for agent in all() {
            let mut files: Vec<&str> = agent.logins.iter().map(|login| login.file).collect();
            files.sort_unstable();
            files.dedup();
            assert_eq!(files.len(), agent.logins.len(), "{}", agent.id);
            for login in agent.logins {
                assert!(!login.dir.starts_with('/') && !login.dir.ends_with('/'));
                assert!(!login.file.contains('/') && !login.dir.contains(".."));
            }
        }
    }

    #[test]
    fn secret_vars_and_hosts_list_each_once() {
        let vars = secret_vars();
        assert_eq!(
            vars,
            [
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "OPENAI_API_KEY",
                "COPILOT_GITHUB_TOKEN",
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "CURSOR_API_KEY",
                "GOOGLE_API_KEY",
                "FACTORY_API_KEY",
                "GEMINI_API_KEY",
                "OPENCODE_API_KEY",
            ]
        );
        let hosts = secret_hosts();
        let mut unique = hosts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), hosts.len());
        assert!(hosts.contains(&"api.githubcopilot.com"));
    }

    #[test]
    fn select_secrets_forwards_set_vars_and_reports_unset_and_empty() {
        let (present, missing) = select_secrets(&GEMINI, |var| {
            (var == "GOOGLE_API_KEY").then(|| "test-value".to_owned())
        });
        assert_eq!(present, [&GOOGLE_API_KEY]);
        assert_eq!(missing, ["GEMINI_API_KEY"]);

        let (present, missing) = select_secrets(&CODEX, |_| Some(String::new()));
        assert_eq!(present, [] as [&Secret; 0]);
        assert_eq!(missing, ["OPENAI_API_KEY"]);

        let (present, missing) = select_secrets(&PI, |_| Some("unused".to_owned()));
        assert_eq!(present, [] as [&Secret; 0]);
        assert_eq!(missing, [] as [&str; 0]);
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn launch_args_follow_sbx() {
        // Defaults alone, then flags appended after them.
        assert_eq!(
            launch_args(&KIRO, true, &[], &[]),
            ["chat", "--trust-all-tools"]
        );
        assert_eq!(
            launch_args(
                &CLAUDE,
                true,
                &strings(&["--model", "opus"]),
                &strings(&["-c"])
            ),
            ["--dangerously-skip-permissions", "--model", "opus", "-c"]
        );
        // Off in config.toml: only the fixed subcommand stays.
        assert_eq!(
            launch_args(&DOCKER_AGENT, false, &[], &strings(&["--debug"])),
            ["run", "--debug"]
        );
        // A word first replaces every default, as `sbx run AGENT -- login` does.
        assert_eq!(
            launch_args(
                &CODEX,
                true,
                &strings(&["--x"]),
                &strings(&["login", "--device-auth"])
            ),
            ["login", "--device-auth"]
        );
        assert_eq!(launch_args(&SHELL, true, &[], &[]), [] as [&str; 0]);
    }
}
