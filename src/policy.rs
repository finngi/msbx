//! The network policy: presets, rule targets, the `policy.toml` file and the allow/block decision
//! the proxy makes for each connection. Pure: no IO except the two file functions at the end.
//!
//! Rule targets follow Docker Sandboxes (`sbx policy`): `example.com` is that host on any port,
//! `*.example.com` one extra label, `**.example.com` one or more extra labels (never the apex,
//! like `*.`), `**` everything, any of them with `:port`, plus IPs and CIDRs. Deny beats allow.

use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::agents;

/// Hosts the `balanced` preset allows besides each agent's secret host (`agents.rs`): Codex
/// sign-in and `ChatGPT` mode (Codex 0.148.0 `login/src/server.rs` `DEFAULT_ISSUER`,
/// `model-provider-info/src/lib.rs` `CHATGPT_CODEX_BASE_URL`, `http-client/src/chatgpt_hosts.rs`),
/// then the package sources the install scripts and common toolchains use, and GitHub.
pub(crate) const BALANCED_BASELINE: [&str; 18] = [
    "auth.openai.com",
    "chatgpt.com",
    "**.chatgpt.com",
    "registry.npmjs.org",
    "deb.debian.org",
    "security.debian.org",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "proxy.golang.org",
    "sum.golang.org",
    "github.com",
    "**.github.com",
    "**.githubusercontent.com",
    "api.github.com",
    "objects.githubusercontent.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Preset {
    Open,
    Balanced,
    LockedDown,
}

impl Preset {
    pub(crate) const ALL: [Preset; 3] = [Preset::Open, Preset::Balanced, Preset::LockedDown];

    /// The CLI and file spelling.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Balanced => "balanced",
            Self::LockedDown => "locked-down",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Balanced => "Balanced",
            Self::LockedDown => "Locked Down",
        }
    }

    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Open => "everything is allowed unless you deny it",
            Self::Balanced => "block by default; AI APIs, package registries and GitHub allowed",
            Self::LockedDown => "block everything you have not allowed",
        }
    }

    fn default_verdict(self) -> Verdict {
        match self {
            Self::Open => Verdict::Allow,
            Self::Balanced | Self::LockedDown => Verdict::Block,
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a rule matches, before the optional port.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    /// `**`
    All,
    /// `example.com`
    Host(String),
    /// `*.example.com`: exactly one extra label.
    OneLabel(String),
    /// `**.example.com`: one or more extra labels; the apex itself is not matched.
    AnyDepth(String),
    Ip(IpAddr),
    Cidr {
        network: IpAddr,
        bits: u8,
    },
}

/// A parsed rule target. Construct with [`Target::parse`]; `Display` gives the canonical text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Target {
    pattern: Pattern,
    port: Option<u16>,
}

impl Target {
    /// Parses one target. Hosts are lowercased and lose a trailing dot; IPv6 with a port is
    /// written `[addr]:port`; CIDRs take no port.
    pub(crate) fn parse(input: &str) -> Result<Self> {
        let text = input.trim().to_ascii_lowercase();
        ensure!(!text.is_empty(), "empty rule target");
        let invalid = |reason: &str| anyhow::anyhow!("invalid target {input:?}: {reason}");
        if let Some((network, bits)) = text.split_once('/') {
            let network: IpAddr = network.parse().map_err(|_| invalid("bad CIDR address"))?;
            let bits: u8 = bits
                .parse()
                .map_err(|_| invalid("bad CIDR prefix length"))?;
            let width = ip_bits(network);
            ensure!(
                bits <= width,
                invalid(&format!("prefix length must be at most {width}"))
            );
            return Ok(Self {
                pattern: Pattern::Cidr {
                    network: mask(network, bits),
                    bits,
                },
                port: None,
            });
        }
        if let Ok(ip) = text.parse::<IpAddr>() {
            return Ok(Self {
                pattern: Pattern::Ip(ip),
                port: None,
            });
        }
        let (host, port) = split_port(&text).map_err(invalid)?;
        if let Some(v6) = host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let ip: Ipv6Addr = v6.parse().map_err(|_| invalid("bad IPv6 address"))?;
            return Ok(Self {
                pattern: Pattern::Ip(IpAddr::V6(ip)),
                port,
            });
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(Self {
                pattern: Pattern::Ip(ip),
                port,
            });
        }
        let host = host.strip_suffix('.').unwrap_or(host);
        let pattern = if host == "**" {
            Pattern::All
        } else if let Some(suffix) = host.strip_prefix("**.") {
            check_suffix(suffix).map_err(invalid)?;
            Pattern::AnyDepth(suffix.to_owned())
        } else if let Some(suffix) = host.strip_prefix("*.") {
            check_suffix(suffix).map_err(invalid)?;
            Pattern::OneLabel(suffix.to_owned())
        } else {
            check_labels(host).map_err(invalid)?;
            Pattern::Host(host.to_owned())
        };
        Ok(Self { pattern, port })
    }

    /// The `(host, ip, port)` this target names when it is a single host or IP rather than a
    /// pattern, for `msbx policy check`.
    pub(crate) fn endpoint(&self) -> Option<(Option<&str>, Option<IpAddr>, Option<u16>)> {
        match &self.pattern {
            Pattern::Host(host) => Some((Some(host), None, self.port)),
            Pattern::Ip(ip) => Some((None, Some(*ip), self.port)),
            Pattern::All | Pattern::OneLabel(_) | Pattern::AnyDepth(_) | Pattern::Cidr { .. } => {
                None
            }
        }
    }

    /// Whether this target covers a connection. Host patterns need a known hostname; IP
    /// patterns look at the address; `**` covers everything. A port-scoped target needs a port.
    pub(crate) fn matches(
        &self,
        host: Option<&str>,
        ip: Option<IpAddr>,
        port: Option<u16>,
    ) -> bool {
        if let Some(wanted) = self.port
            && port != Some(wanted)
        {
            return false;
        }
        match &self.pattern {
            Pattern::All => true,
            Pattern::Host(name) => host == Some(name.as_str()),
            Pattern::OneLabel(suffix) => {
                extra_labels(host, suffix).is_some_and(|rest| !rest.contains('.'))
            }
            Pattern::AnyDepth(suffix) => extra_labels(host, suffix).is_some(),
            Pattern::Ip(wanted) => ip == Some(*wanted),
            Pattern::Cidr { network, bits } => {
                ip.is_some_and(|ip| ip_bits(ip) == ip_bits(*network) && mask(ip, *bits) == *network)
            }
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.pattern, self.port) {
            (Pattern::All, _) => f.write_str("**")?,
            (Pattern::Host(host), _) => f.write_str(host)?,
            (Pattern::OneLabel(suffix), _) => write!(f, "*.{suffix}")?,
            (Pattern::AnyDepth(suffix), _) => write!(f, "**.{suffix}")?,
            (Pattern::Ip(IpAddr::V6(ip)), Some(_)) => write!(f, "[{ip}]")?,
            (Pattern::Ip(ip), _) => write!(f, "{ip}")?,
            (Pattern::Cidr { network, bits }, _) => write!(f, "{network}/{bits}")?,
        }
        match self.port {
            Some(port) => write!(f, ":{port}"),
            None => Ok(()),
        }
    }
}

impl TryFrom<String> for Target {
    type Error = anyhow::Error;

    fn try_from(text: String) -> Result<Self> {
        Self::parse(&text)
    }
}

impl From<Target> for String {
    fn from(target: Target) -> Self {
        target.to_string()
    }
}

/// `host`'s labels in front of `.suffix`, or `None` when it is not under the suffix.
fn extra_labels<'a>(host: Option<&'a str>, suffix: &str) -> Option<&'a str> {
    host?
        .strip_suffix(suffix)?
        .strip_suffix('.')
        .filter(|rest| !rest.is_empty())
}

/// Splits a trailing `:port`. A bare IPv6 address never reaches here (it parses whole first),
/// so any colon outside brackets is the port separator.
fn split_port(text: &str) -> std::result::Result<(&str, Option<u16>), &'static str> {
    let Some((host, port)) = text.rsplit_once(':') else {
        return Ok((text, None));
    };
    if host.starts_with('[') && !host.ends_with(']') {
        return Err("IPv6 addresses with a port are written [addr]:port");
    }
    let port: u16 = port
        .parse()
        .map_err(|_| "port must be a number from 1 to 65535")?;
    if port == 0 {
        return Err("port must be a number from 1 to 65535");
    }
    Ok((host, Some(port)))
}

fn check_suffix(suffix: &str) -> std::result::Result<(), &'static str> {
    check_labels(suffix)?;
    if !suffix.contains('.') {
        return Err("a wildcard suffix needs at least two labels, e.g. *.example.com");
    }
    Ok(())
}

fn check_labels(host: &str) -> std::result::Result<(), &'static str> {
    if host.is_empty() {
        return Err("empty host");
    }
    if host.len() > 253 {
        return Err("host is longer than 253 bytes");
    }
    for label in host.split('.') {
        if label.is_empty() {
            return Err("empty label (two dots in a row, or a leading dot)");
        }
        if label.len() > 63 {
            return Err("a label is longer than 63 bytes");
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("a label starts or ends with '-'");
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err("only letters, digits, '-' and '_' are allowed in host names");
        }
    }
    Ok(())
}

fn ip_bits(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

/// The address with everything after the first `bits` bits cleared.
fn mask(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let kept = u32::from(v4) & u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            IpAddr::V4(kept.into())
        }
        IpAddr::V6(v6) => {
            let kept = u128::from(v6) & u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            IpAddr::V6(kept.into())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allow,
    Block,
}

/// Where a rule comes from, for `msbx policy ls` and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Preset,
    Global,
    Sandbox,
}

impl Source {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::Global => "global",
            Self::Sandbox => "sandbox",
        }
    }
}

/// One effective rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectiveRule<'a> {
    pub(crate) verdict: Verdict,
    pub(crate) target: &'a Target,
    pub(crate) source: Source,
}

impl EffectiveRule<'_> {
    /// `allow pypi.org (global)`: the text the log and `check` show.
    pub(crate) fn describe(&self) -> String {
        let action = match self.verdict {
            Verdict::Allow => "allow",
            Verdict::Block => "deny",
        };
        format!("{action} {} ({})", self.target, self.source.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) verdict: Verdict,
    /// The deciding rule's description, or `default`.
    pub(crate) rule: String,
}

/// Rules scoped to one sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<Target>,
}

impl Rules {
    fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

/// The contents of `policy.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) preset: Preset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<Target>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) sandboxes: BTreeMap<String, Rules>,
}

static BASELINE: LazyLock<Vec<Target>> = LazyLock::new(|| {
    // The table is a constant checked by `baseline_targets_parse`, so a bad entry cannot
    // reach a user without failing the tests.
    agents::secret_hosts()
        .into_iter()
        .chain(BALANCED_BASELINE)
        .filter_map(|host| Target::parse(host).ok())
        .collect()
});

impl Policy {
    pub(crate) fn new(preset: Preset) -> Self {
        Self {
            preset,
            allow: Vec::new(),
            deny: Vec::new(),
            sandboxes: BTreeMap::new(),
        }
    }

    /// Every rule that applies to `sandbox` (or only the shared ones for `None`), denies before
    /// allows and the most specific source first.
    pub(crate) fn rules(&self, sandbox: Option<&str>) -> Vec<EffectiveRule<'_>> {
        let scoped = sandbox.and_then(|name| self.sandboxes.get(name));
        let empty: &[Target] = &[];
        let baseline: &[Target] = if self.preset == Preset::Balanced {
            &BASELINE
        } else {
            empty
        };
        let layers: [(Verdict, &[Target], Source); 5] = [
            (
                Verdict::Block,
                scoped.map_or(empty, |rules| &rules.deny),
                Source::Sandbox,
            ),
            (Verdict::Block, &self.deny, Source::Global),
            (
                Verdict::Allow,
                scoped.map_or(empty, |rules| &rules.allow),
                Source::Sandbox,
            ),
            (Verdict::Allow, &self.allow, Source::Global),
            (Verdict::Allow, baseline, Source::Preset),
        ];
        layers
            .into_iter()
            .flat_map(|(verdict, targets, source)| {
                targets.iter().map(move |target| EffectiveRule {
                    verdict,
                    target,
                    source,
                })
            })
            .collect()
    }

    /// Any matching deny blocks; else any matching allow allows; else the preset decides.
    pub(crate) fn decide(
        &self,
        sandbox: &str,
        host: Option<&str>,
        ip: Option<IpAddr>,
        port: Option<u16>,
    ) -> Decision {
        let rules = self.rules(Some(sandbox));
        let matching = rules
            .iter()
            .find(|rule| rule.target.matches(host, ip, port));
        match matching {
            Some(rule) => Decision {
                verdict: rule.verdict,
                rule: rule.describe(),
            },
            None => Decision {
                verdict: self.preset.default_verdict(),
                rule: "default".to_owned(),
            },
        }
    }

    /// Adds allow rules (removing the same targets from the scope's deny list, so the command
    /// has an effect). Returns the targets that were new.
    pub(crate) fn allow(&mut self, targets: &[Target], sandbox: Option<&str>) -> Vec<Target> {
        let (allow, deny) = self.lists(sandbox);
        deny.retain(|target| !targets.contains(target));
        push_new(allow, targets)
    }

    pub(crate) fn deny(&mut self, targets: &[Target], sandbox: Option<&str>) -> Vec<Target> {
        let (allow, deny) = self.lists(sandbox);
        allow.retain(|target| !targets.contains(target));
        push_new(deny, targets)
    }

    /// Removes `target` from both lists of the scope. Errors when it was in neither.
    pub(crate) fn remove(&mut self, target: &Target, sandbox: Option<&str>) -> Result<()> {
        let (allow, deny) = self.lists(sandbox);
        let before = allow.len() + deny.len();
        allow.retain(|existing| existing != target);
        deny.retain(|existing| existing != target);
        let removed = before != allow.len() + deny.len();
        self.sandboxes.retain(|_, rules| !rules.is_empty());
        if !removed {
            match sandbox {
                Some(name) => bail!("no rule for {target} in sandbox {name}"),
                None => bail!("no global rule for {target}"),
            }
        }
        Ok(())
    }

    fn lists(&mut self, sandbox: Option<&str>) -> (&mut Vec<Target>, &mut Vec<Target>) {
        match sandbox {
            Some(name) => {
                let rules = self.sandboxes.entry(name.to_owned()).or_default();
                (&mut rules.allow, &mut rules.deny)
            }
            None => (&mut self.allow, &mut self.deny),
        }
    }

    pub(crate) fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("serialising the policy")
    }

    pub(crate) fn from_toml(text: &str) -> Result<Self> {
        toml::from_str(text).context("parsing the policy")
    }
}

fn push_new(list: &mut Vec<Target>, targets: &[Target]) -> Vec<Target> {
    let mut added = Vec::new();
    for target in targets {
        if !list.contains(target) {
            list.push(target.clone());
            added.push(target.clone());
        }
    }
    added
}

/// Parses comma-separated targets, as `msbx policy allow a.com,b.com` takes them.
pub(crate) fn parse_targets(text: &str) -> Result<Vec<Target>> {
    let targets: Vec<Target> = text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(Target::parse)
        .collect::<Result<_>>()?;
    ensure!(!targets.is_empty(), "no rule target given");
    Ok(targets)
}

/// The policy file, or `None` when it does not exist yet ("not initialised").
pub(crate) fn load(path: &Path) -> Result<Option<Policy>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Policy::from_toml(&text)
        .map(Some)
        .with_context(|| format!("reading {}", path.display()))
}

/// Writes the policy atomically with mode 600.
pub(crate) fn save(path: &Path, policy: &Policy) -> Result<()> {
    crate::paths::write_private(path, policy.to_toml()?.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Target {
        Target::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    fn v4(text: &str) -> Option<IpAddr> {
        text.parse().ok()
    }

    #[test]
    fn accepted_targets_round_trip_to_canonical_text() {
        let cases = [
            ("example.com", "example.com"),
            ("Example.COM.", "example.com"),
            ("localhost", "localhost"),
            ("*.example.com", "*.example.com"),
            ("**.example.com", "**.example.com"),
            ("**", "**"),
            ("**:443", "**:443"),
            ("example.com:443", "example.com:443"),
            ("my_host.example.com", "my_host.example.com"),
            ("10.0.0.1", "10.0.0.1"),
            ("10.0.0.1:8080", "10.0.0.1:8080"),
            ("10.1.2.3/8", "10.0.0.0/8"),
            ("0.0.0.0/0", "0.0.0.0/0"),
            ("2001:db8::1", "2001:db8::1"),
            ("[2001:db8::1]:443", "[2001:db8::1]:443"),
            ("2001:DB8::/32", "2001:db8::/32"),
            ("  pypi.org  ", "pypi.org"),
        ];
        for (input, canonical) in cases {
            assert_eq!(target(input).to_string(), canonical, "{input}");
            assert_eq!(target(canonical), target(input), "{input} re-parses");
        }
    }

    #[test]
    fn rejected_targets_name_the_problem() {
        let cases = [
            ("", "empty"),
            ("*.com", "two labels"),
            ("**.com", "two labels"),
            ("*", "only letters"),
            ("a..b.com", "empty label"),
            (".example.com", "empty label"),
            ("-bad.com", "starts or ends with '-'"),
            ("exa mple.com", "only letters"),
            ("*.*.com", "only letters"),
            ("foo.*.com", "only letters"),
            ("example.com:0", "1 to 65535"),
            ("example.com:65536", "1 to 65535"),
            ("example.com:https", "1 to 65535"),
            ("10.0.0.0/33", "at most 32"),
            ("2001:db8::/129", "at most 128"),
            ("10.0.0.0/x", "prefix length"),
            ("[2001:db8::1:443", "[addr]:port"),
            ("[2001:db8::zz]:443", "bad IPv6"),
            ("300.1.1.1/8", "bad CIDR address"),
            (&"a".repeat(64), "63 bytes"),
        ];
        for (input, fragment) in cases {
            let error = match Target::parse(input) {
                Ok(target) => panic!("{input:?} parsed as {target}"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains(fragment), "{input:?}: {error}");
        }
    }

    #[test]
    fn host_patterns_match_only_with_a_name() {
        let any = v4("1.2.3.4");
        assert!(target("example.com").matches(Some("example.com"), any, Some(443)));
        assert!(!target("example.com").matches(Some("www.example.com"), any, Some(443)));
        assert!(!target("example.com").matches(None, any, Some(443)));

        let one = target("*.example.com");
        assert!(one.matches(Some("api.example.com"), None, None));
        assert!(!one.matches(Some("v2.api.example.com"), None, None));
        assert!(!one.matches(Some("example.com"), None, None));
        assert!(!one.matches(Some("notexample.com"), None, None));

        let deep = target("**.example.com");
        assert!(deep.matches(Some("api.example.com"), None, None));
        assert!(deep.matches(Some("v2.api.example.com"), None, None));
        assert!(
            !deep.matches(Some("example.com"), None, None),
            "apex is not covered"
        );
        assert!(!deep.matches(Some("evilexample.com"), None, None));

        assert!(target("**").matches(None, any, None));
        assert!(target("**").matches(Some("x"), None, None));
    }

    #[test]
    fn ports_scope_a_rule() {
        let scoped = target("example.com:443");
        assert!(scoped.matches(Some("example.com"), None, Some(443)));
        assert!(!scoped.matches(Some("example.com"), None, Some(80)));
        assert!(!scoped.matches(Some("example.com"), None, None));
        assert!(target("**:22").matches(None, v4("9.9.9.9"), Some(22)));
        assert!(!target("**:22").matches(None, v4("9.9.9.9"), Some(23)));
    }

    #[test]
    fn ip_and_cidr_patterns_match_the_address() {
        assert!(target("10.0.0.1").matches(Some("any.name"), v4("10.0.0.1"), Some(1)));
        assert!(!target("10.0.0.1").matches(None, v4("10.0.0.2"), Some(1)));
        assert!(!target("10.0.0.1").matches(None, None, Some(1)));
        assert!(target("10.0.0.1:80").matches(None, v4("10.0.0.1"), Some(80)));
        assert!(!target("10.0.0.1:80").matches(None, v4("10.0.0.1"), Some(81)));

        let cidr = target("10.0.0.0/8");
        assert!(cidr.matches(None, v4("10.255.1.2"), None));
        assert!(!cidr.matches(None, v4("11.0.0.1"), None));
        assert!(
            !cidr.matches(None, "::ffff:10.0.0.1".parse().ok(), None),
            "no v4/v6 mixing"
        );
        assert!(target("0.0.0.0/0").matches(None, v4("203.0.113.9"), None));
        assert!(!target("0.0.0.0/0").matches(None, "2001:db8::1".parse().ok(), None));

        let v6 = target("2001:db8::/32");
        assert!(v6.matches(None, "2001:db8:ffff::1".parse().ok(), None));
        assert!(!v6.matches(None, "2001:db9::1".parse().ok(), None));
        assert!(target("[2001:db8::1]:443").matches(None, "2001:db8::1".parse().ok(), Some(443)));
    }

    fn sample(preset: Preset) -> Policy {
        let mut policy = Policy::new(preset);
        policy.allow = vec![target("pypi.org"), target("*.allowed.test")];
        policy.deny = vec![target("evil.test"), target("**.allowed.test:22")];
        policy.sandboxes.insert(
            "box".to_owned(),
            Rules {
                allow: vec![target("box-only.test")],
                deny: vec![target("pypi.org")],
            },
        );
        policy
    }

    fn decide(policy: &Policy, sandbox: &str, host: &str, port: u16) -> (Verdict, String) {
        let decision = policy.decide(sandbox, Some(host), v4("203.0.113.1"), Some(port));
        (decision.verdict, decision.rule)
    }

    #[test]
    fn deny_beats_allow_across_scopes() {
        let policy = sample(Preset::LockedDown);
        assert_eq!(
            decide(&policy, "other", "pypi.org", 443),
            (Verdict::Allow, "allow pypi.org (global)".to_owned())
        );
        assert_eq!(
            decide(&policy, "box", "pypi.org", 443),
            (Verdict::Block, "deny pypi.org (sandbox)".to_owned()),
            "a sandbox deny beats a global allow"
        );
        assert_eq!(
            decide(&policy, "box", "api.allowed.test", 22),
            (
                Verdict::Block,
                "deny **.allowed.test:22 (global)".to_owned()
            ),
            "a global deny beats a global allow"
        );
        assert_eq!(
            decide(&policy, "box", "api.allowed.test", 443),
            (Verdict::Allow, "allow *.allowed.test (global)".to_owned())
        );
        assert_eq!(
            decide(&policy, "box", "box-only.test", 443),
            (Verdict::Allow, "allow box-only.test (sandbox)".to_owned())
        );
        assert_eq!(
            decide(&policy, "other", "box-only.test", 443),
            (Verdict::Block, "default".to_owned()),
            "sandbox rules do not leak"
        );
    }

    #[test]
    fn presets_decide_what_no_rule_matches() {
        assert_eq!(
            decide(&sample(Preset::Open), "box", "unknown.test", 443),
            (Verdict::Allow, "default".to_owned())
        );
        assert_eq!(
            decide(&sample(Preset::Open), "box", "evil.test", 443),
            (Verdict::Block, "deny evil.test (global)".to_owned()),
            "open still honours denies"
        );
        assert_eq!(
            decide(&sample(Preset::Balanced), "box", "unknown.test", 443),
            (Verdict::Block, "default".to_owned())
        );
        assert_eq!(
            decide(&sample(Preset::Balanced), "box", "registry.npmjs.org", 443),
            (
                Verdict::Allow,
                "allow registry.npmjs.org (preset)".to_owned()
            )
        );
        assert_eq!(
            decide(&sample(Preset::Balanced), "box", "api.openai.com", 443),
            (Verdict::Allow, "allow api.openai.com (preset)".to_owned()),
            "agent secret hosts are in the baseline"
        );
        assert_eq!(
            decide(
                &sample(Preset::Balanced),
                "box",
                "raw.githubusercontent.com",
                443
            ),
            (
                Verdict::Allow,
                "allow **.githubusercontent.com (preset)".to_owned()
            )
        );
        assert_eq!(
            decide(
                &sample(Preset::LockedDown),
                "box",
                "registry.npmjs.org",
                443
            ),
            (Verdict::Block, "default".to_owned())
        );
    }

    #[test]
    fn decisions_without_a_name_use_ip_rules_only() {
        let mut policy = Policy::new(Preset::LockedDown);
        policy.allow = vec![target("pypi.org"), target("203.0.113.0/24")];
        let by_ip = policy.decide("box", None, v4("203.0.113.7"), Some(443));
        assert_eq!(by_ip.verdict, Verdict::Allow);
        assert_eq!(by_ip.rule, "allow 203.0.113.0/24 (global)");
        let no_match = policy.decide("box", None, v4("198.51.100.1"), Some(443));
        assert_eq!(no_match.verdict, Verdict::Block);
        let by_name = policy.decide("box", Some("pypi.org"), None, None);
        assert_eq!(by_name.verdict, Verdict::Allow);
    }

    #[test]
    fn baseline_targets_parse() {
        for host in BALANCED_BASELINE
            .iter()
            .copied()
            .chain(agents::secret_hosts())
        {
            target(host);
        }
        assert_eq!(
            BASELINE.len(),
            BALANCED_BASELINE.len() + agents::secret_hosts().len()
        );
    }

    #[test]
    fn rules_list_most_specific_first() {
        let policy = sample(Preset::Balanced);
        let listed: Vec<String> = policy
            .rules(Some("box"))
            .iter()
            .map(EffectiveRule::describe)
            .collect();
        assert_eq!(
            listed[..6],
            [
                "deny pypi.org (sandbox)",
                "deny evil.test (global)",
                "deny **.allowed.test:22 (global)",
                "allow box-only.test (sandbox)",
                "allow pypi.org (global)",
                "allow *.allowed.test (global)",
            ]
        );
        assert_eq!(listed[6], "allow api.openai.com (preset)");
        assert_eq!(policy.rules(None).len(), 4 + BASELINE.len());
        assert_eq!(sample(Preset::LockedDown).rules(None).len(), 4);
    }

    #[test]
    fn allow_deny_and_rm_edit_the_right_scope() -> Result<()> {
        let mut policy = Policy::new(Preset::LockedDown);
        let added = policy.allow(&[target("a.test"), target("b.test")], None);
        assert_eq!(added.len(), 2);
        assert!(
            policy.allow(&[target("a.test")], None).is_empty(),
            "no duplicates"
        );
        policy.deny(&[target("a.test")], Some("box"));
        assert_eq!(policy.sandboxes["box"].deny, [target("a.test")]);
        assert_eq!(
            policy.allow,
            [target("a.test"), target("b.test")],
            "global untouched"
        );

        policy.allow(&[target("a.test")], Some("box"));
        assert!(
            policy.sandboxes["box"].deny.is_empty(),
            "allow lifts the scope's deny"
        );
        assert_eq!(policy.sandboxes["box"].allow, [target("a.test")]);
        policy.deny(&[target("a.test")], Some("box"));
        assert!(
            policy.sandboxes["box"].allow.is_empty(),
            "deny lifts the scope's allow"
        );

        policy.remove(&target("a.test"), Some("box"))?;
        assert!(
            !policy.sandboxes.contains_key("box"),
            "empty tables are dropped"
        );
        assert!(policy.remove(&target("a.test"), Some("box")).is_err());
        policy.remove(&target("a.test"), None)?;
        assert_eq!(policy.allow, [target("b.test")]);
        assert!(policy.remove(&target("zzz.test"), None).is_err());
        Ok(())
    }

    #[test]
    fn parse_targets_splits_on_commas() -> Result<()> {
        assert_eq!(
            parse_targets("a.test, b.test:443 ,,")?,
            [target("a.test"), target("b.test:443")]
        );
        assert!(parse_targets(" , ").is_err());
        assert!(parse_targets("a.test,*.com").is_err());
        Ok(())
    }

    #[test]
    fn file_round_trips_and_omits_empty_lists() -> Result<()> {
        let policy = sample(Preset::Balanced);
        let text = policy.to_toml()?;
        assert!(text.starts_with("preset = \"balanced\"\n"), "{text}");
        assert!(text.contains("[sandboxes.box]"), "{text}");
        assert_eq!(Policy::from_toml(&text)?, policy);

        let minimal = Policy::new(Preset::LockedDown).to_toml()?;
        assert_eq!(minimal, "preset = \"locked-down\"\n");
        assert_eq!(
            Policy::from_toml("preset = \"open\"")?,
            Policy::new(Preset::Open)
        );
        Ok(())
    }

    #[test]
    fn hand_edits_and_bad_files_are_reported() {
        let hand_written = "preset = \"balanced\"\nallow = [\"PyPI.org\", \"*.Example.com:443\"]\n\
                            [sandboxes.\"codex-api\"]\ndeny = [\"**\"]\n";
        let policy = Policy::from_toml(hand_written).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(
            policy.allow,
            [target("pypi.org"), target("*.example.com:443")]
        );
        assert_eq!(policy.sandboxes["codex-api"].deny, [target("**")]);

        let unknown = Policy::from_toml("preset = \"open\"\nalow = [\"x.test\"]").err();
        let message = unknown.map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(message.contains("unknown field `alow`"), "{message}");
        assert!(
            Policy::from_toml("allow = []").is_err(),
            "preset is required"
        );
        assert!(Policy::from_toml("preset = \"strict\"").is_err());
        let bad_target = Policy::from_toml("preset = \"open\"\ndeny = [\"*.com\"]")
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        assert!(bad_target.contains("*.com"), "{bad_target}");
    }

    #[cfg(unix)]
    #[test]
    fn load_and_save_use_the_file_with_mode_600() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("msbx-policy-{}", std::process::id()));
        let path = root.join("policy.toml");
        let outcome = (|| -> Result<()> {
            assert_eq!(load(&path)?, None, "missing file means not initialised");
            let policy = sample(Preset::LockedDown);
            save(&path, &policy)?;
            assert_eq!(load(&path)?, Some(policy));
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
            std::fs::write(&path, "preset = \"open\"\nbogus = 1\n")?;
            let error = load(&path)
                .err()
                .map(|e| format!("{e:#}"))
                .unwrap_or_default();
            assert!(
                error.contains("policy.toml") && error.contains("bogus"),
                "{error}"
            );
            Ok(())
        })();
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        outcome
    }
}
