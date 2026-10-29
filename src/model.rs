//! Plain data shared by the sandbox layer and the TUI. No SDK types cross this boundary.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Running,
    Starting,
    Stopped,
    Crashed,
    Other,
}

/// A sandbox as the TUI shows it. Fields the runtime cannot report are `None`/empty.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SandboxInfo {
    pub(crate) name: String,
    pub(crate) status: Status,
    /// From the `msbx.agent` label; `None` for sandboxes msbx did not create.
    pub(crate) agent: Option<String>,
    /// From the `msbx.project` label.
    pub(crate) project: Option<PathBuf>,
    pub(crate) image: Option<String>,
    pub(crate) vcpus: Option<u8>,
    pub(crate) cpu_percent: Option<f64>,
    pub(crate) memory_bytes: Option<u64>,
    pub(crate) memory_limit_bytes: Option<u64>,
    pub(crate) uptime: Option<Duration>,
    /// Human summary of the static network policy, e.g. `public + host`.
    pub(crate) network: Option<String>,
    /// Created with the msbx proxy (label `msbx.network=proxy`), so `policy.toml` applies.
    pub(crate) proxied: bool,
    pub(crate) rx_bytes: Option<u64>,
    pub(crate) tx_bytes: Option<u64>,
    /// Secret env var names only; values never leave the host.
    pub(crate) secrets: Vec<String>,
    /// `host -> guest` mount descriptions.
    pub(crate) mounts: Vec<String>,
}

/// One row of the Network tab: a connection the proxy decided, already formatted for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetEntry {
    pub(crate) sandbox: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) allowed: bool,
    pub(crate) count: u64,
    /// Local `HH:MM:SS`.
    pub(crate) last_seen: String,
}
