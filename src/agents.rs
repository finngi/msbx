//! The agents msbx knows how to launch. Adding one is a new `static` plus an `AgentId` variant.

/// OCI image every agent sandbox boots from.
pub(crate) const IMAGE: &str = "node:24-bookworm-slim";

/// A host secret forwarded to the sandbox: the guest sees a placeholder in `env_var`, and the
/// network proxy substitutes the real value only on requests to `host`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Secret {
    pub(crate) env_var: &'static str,
    pub(crate) host: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Agent {
    /// Stable identifier used in sandbox names, labels and the CLI.
    pub(crate) id: &'static str,
    #[allow(
        dead_code,
        reason = "shown by the TUI, which lands in a separate change"
    )]
    pub(crate) name: &'static str,
    /// Runs once as root inside a fresh sandbox.
    pub(crate) install: &'static str,
    pub(crate) command: &'static str,
    pub(crate) secrets: &'static [Secret],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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

static CODEX: Agent = Agent {
    id: "codex",
    name: "Codex",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
              && npm install -g @openai/codex@0.148.0",
    command: "codex",
    secrets: &[Secret {
        env_var: "OPENAI_API_KEY",
        host: "api.openai.com",
    }],
};

static PI: Agent = Agent {
    id: "pi",
    name: "Pi",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
              && npm install -g @earendil-works/pi-coding-agent@1.4.0",
    command: "pi",
    // Pi talks to local models on the host, so there is nothing to forward.
    secrets: &[],
};

static SHELL: Agent = Agent {
    id: "shell",
    name: "Shell",
    install: "apt-get update && apt-get install -y --no-install-recommends ca-certificates git curl",
    command: "bash",
    secrets: &[],
};

/// A secret resolved from the host environment, ready to hand to the sandbox builder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResolvedSecret {
    pub(crate) env_var: &'static str,
    pub(crate) value: String,
    pub(crate) host: &'static str,
}

/// Splits an agent's secrets into those set on the host and the names of those that are not.
/// An empty value counts as unset: forwarding it would only produce a confusing auth failure.
pub(crate) fn select_secrets(
    agent: &Agent,
    lookup: impl Fn(&str) -> Option<String>,
) -> (Vec<ResolvedSecret>, Vec<&'static str>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for secret in agent.secrets {
        match lookup(secret.env_var).filter(|value| !value.is_empty()) {
            Some(value) => found.push(ResolvedSecret {
                env_var: secret.env_var,
                value,
                host: secret.host,
            }),
            None => missing.push(secret.env_var),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

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
    fn select_secrets_forwards_set_vars() {
        let (found, missing) = select_secrets(&CODEX, |var| {
            (var == "OPENAI_API_KEY").then(|| "sk-test".to_owned())
        });
        assert_eq!(
            found,
            [ResolvedSecret {
                env_var: "OPENAI_API_KEY",
                value: "sk-test".to_owned(),
                host: "api.openai.com",
            }]
        );
        assert_eq!(missing, [] as [&str; 0]);
    }

    #[test]
    fn select_secrets_reports_unset_and_empty_vars() {
        let (found, missing) = select_secrets(&CODEX, |_| None);
        assert_eq!(found, []);
        assert_eq!(missing, ["OPENAI_API_KEY"]);

        let (found, missing) = select_secrets(&CODEX, |_| Some(String::new()));
        assert_eq!(found, []);
        assert_eq!(missing, ["OPENAI_API_KEY"]);
    }

    #[test]
    fn agents_without_secrets_select_nothing() {
        let (found, missing) = select_secrets(&PI, |_| Some("unused".to_owned()));
        assert_eq!(found, []);
        assert_eq!(missing, [] as [&str; 0]);
    }
}
