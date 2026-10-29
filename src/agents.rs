//! The agents msbx knows how to launch. Adding one is a new `static` plus an `AgentId` variant.

use clap::ValueEnum;

/// OCI image every agent sandbox boots from.
pub(crate) const IMAGE: &str = "node:24-bookworm-slim";

/// A host secret forwarded to the sandbox: the guest sees a placeholder in `env_var`, and the
/// network proxy substitutes the real value only on requests to `host`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Secret {
    pub(crate) env_var: &'static str,
    pub(crate) host: &'static str,
}

/// Where an agent keeps its sign-in inside the VM. msbx links that file into the shared login
/// volume so one sign-in serves every sandbox of the agent (RFC 0004).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Login {
    pub(crate) dir: &'static str,
    pub(crate) file: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Agent {
    /// Stable identifier used in sandbox names, labels and the CLI.
    pub(crate) id: &'static str,
    /// Runs once as root inside a fresh sandbox.
    pub(crate) install: &'static str,
    pub(crate) command: &'static str,
    pub(crate) secrets: &'static [Secret],
    pub(crate) login: Option<Login>,
    /// Guest path of a pi-format `models.json` listing the host's local model servers.
    pub(crate) models_file: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AgentId {
    Codex,
    Pi,
    Shell,
}

impl AgentId {
    pub(crate) fn agent(self) -> &'static Agent {
        match self {
            Self::Codex => &CODEX,
            Self::Pi => &PI,
            Self::Shell => &SHELL,
        }
    }
}

/// The agent with this id, as stored in the `msbx.agent` label.
pub(crate) fn find(id: &str) -> Option<&'static Agent> {
    AgentId::from_str(id, false).ok().map(AgentId::agent)
}

static CODEX: Agent = Agent {
    id: "codex",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
              && npm install -g @openai/codex@0.148.0",
    command: "codex",
    secrets: &[Secret {
        env_var: "OPENAI_API_KEY",
        host: "api.openai.com",
    }],
    // Codex 0.148.0 rewrites auth.json in place (truncate + write), so a link survives refreshes.
    login: Some(Login {
        dir: "/root/.codex",
        file: "auth.json",
    }),
    models_file: None,
};

static PI: Agent = Agent {
    id: "pi",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
              && npm install -g @earendil-works/pi-coding-agent@1.4.0",
    command: "pi",
    // Pi talks to local models on the host, so there is nothing to forward.
    secrets: &[],
    // pi 1.4.0 writes auth.json in place under a lock file beside it, so a link survives.
    login: Some(Login {
        dir: "/root/.pi/agent",
        file: "auth.json",
    }),
    models_file: Some("/root/.pi/agent/models.json"),
};

static SHELL: Agent = Agent {
    id: "shell",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git curl",
    command: "bash",
    secrets: &[],
    login: None,
    models_file: None,
};

/// Every env var any agent forwards as a secret, in table order without repeats.
pub(crate) fn secret_vars() -> Vec<&'static str> {
    let mut vars = Vec::new();
    for agent in AgentId::value_variants().iter().map(|id| id.agent()) {
        for secret in agent.secrets {
            if !vars.contains(&secret.env_var) {
                vars.push(secret.env_var);
            }
        }
    }
    vars
}

/// Every host any agent's secret may be sent to, in table order without repeats.
pub(crate) fn secret_hosts() -> Vec<&'static str> {
    let mut hosts = Vec::new();
    for agent in AgentId::value_variants().iter().map(|id| id.agent()) {
        for secret in agent.secrets {
            if !hosts.contains(&secret.host) {
                hosts.push(secret.host);
            }
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
    use super::*;

    #[test]
    fn agent_ids_match_cli_names() {
        for variant in AgentId::value_variants() {
            let cli_name = variant
                .to_possible_value()
                .expect("no variant is skipped")
                .get_name()
                .to_owned();
            assert_eq!(variant.agent().id, cli_name);
        }
    }

    #[test]
    fn agent_lookup_parses_cli_names() {
        assert_eq!(
            AgentId::from_str("codex", false).map(AgentId::agent),
            Ok(&CODEX)
        );
        assert_eq!(AgentId::from_str("pi", false).map(AgentId::agent), Ok(&PI));
        assert_eq!(
            AgentId::from_str("shell", false).map(AgentId::agent),
            Ok(&SHELL)
        );
        assert!(AgentId::from_str("claude", false).is_err());
    }

    #[test]
    fn find_resolves_ids_and_rejects_unknown() {
        assert_eq!(find("codex"), Some(&CODEX));
        assert_eq!(find("shell"), Some(&SHELL));
        assert_eq!(find("Codex"), None, "labels are written lowercase");
        assert_eq!(find("claude"), None);
    }

    #[test]
    fn secret_vars_lists_each_var_once() {
        assert_eq!(secret_vars(), ["OPENAI_API_KEY"]);
    }

    #[test]
    fn secret_hosts_lists_each_host_once() {
        assert_eq!(secret_hosts(), ["api.openai.com"]);
    }

    #[test]
    fn select_secrets_forwards_set_vars() {
        let (present, missing) = select_secrets(&CODEX, |var| {
            (var == "OPENAI_API_KEY").then(|| "sk-test".to_owned())
        });
        assert_eq!(
            present,
            [&Secret {
                env_var: "OPENAI_API_KEY",
                host: "api.openai.com",
            }]
        );
        assert_eq!(missing, [] as [&str; 0]);
    }

    #[test]
    fn select_secrets_reports_unset_and_empty_vars() {
        let (present, missing) = select_secrets(&CODEX, |_| None);
        assert_eq!(present, [] as [&Secret; 0]);
        assert_eq!(missing, ["OPENAI_API_KEY"]);

        let (present, missing) = select_secrets(&CODEX, |_| Some(String::new()));
        assert_eq!(present, [] as [&Secret; 0]);
        assert_eq!(missing, ["OPENAI_API_KEY"]);
    }

    #[test]
    fn agents_without_secrets_select_nothing() {
        let (present, missing) = select_secrets(&PI, |_| Some("unused".to_owned()));
        assert_eq!(present, [] as [&Secret; 0]);
        assert_eq!(missing, [] as [&str; 0]);
    }
}
