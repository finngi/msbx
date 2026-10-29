//! The msbx network proxy: an HTTP CONNECT proxy on `127.0.0.1:47447` that every msbx sandbox
//! is created to use (microsandbox `SandboxBuilder::proxy`). msb dials it with `CONNECT ip:port`
//! and Basic credentials (sandbox name, the token in `$XDG_STATE_HOME/msbx/proxy-token`); the
//! proxy recovers the hostname from the guest's first bytes, applies `policy.toml` live, logs the
//! decision and relays or closes. Also the process lifecycle: `serve`, `status`, `stop` and
//! `ensure_running`, which starts a detached proxy for the paths that create or start sandboxes.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::time::{Instant, interval, timeout, timeout_at};

use crate::netlog::Log;
use crate::paths::Dirs;
use crate::policy::{self, Policy, Preset, Verdict};
use crate::wire::{self, Request, Sniff};

/// Baked into every sandbox's config at create, so it never changes.
pub(crate) const ADDRESS: &str = "127.0.0.1:47447";
/// The SDK and `msb` read the proxy password from this host variable when a sandbox starts.
pub(crate) const TOKEN_VAR: &str = "MSBX_PROXY_TOKEN";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Requests being read at once (CONNECT heads, status, stop, reload). Kept apart from the
/// tunnel limit so control requests still get through when every tunnel slot is busy.
const MAX_PENDING: usize = 512;
const SERVICE_UNAVAILABLE: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for the guest's first bytes; server-first protocols send none.
const SNIFF_TIMEOUT: Duration = Duration::from_secs(2);
const SNIFF_LIMIT: usize = 16 * 1024;
const LOOKUP_TIMEOUT: Duration = Duration::from_millis(1500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const SPAWN_WAIT: Duration = Duration::from_secs(2);
const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\n";
const UNAUTHORISED: &[u8] = b"HTTP/1.1 407 Proxy Authentication Required\r\n\
    Proxy-Authenticate: Basic realm=\"msbx\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// How often the proxy looks for policy changes and flushes the log, and how many tunnels it
/// keeps open at once. Injected so tests run fast and can fill every tunnel slot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) reload: Duration,
    pub(crate) flush: Duration,
    pub(crate) max_tunnels: usize,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            reload: Duration::from_secs(1),
            flush: Duration::from_secs(2),
            max_tunnels: 512,
        }
    }
}

/// What `GET /msbx-status` returns; only an msbx proxy answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Status {
    pub(crate) msbx: String,
    pub(crate) pid: u32,
    pub(crate) policy: PathBuf,
}

struct Shared {
    token: String,
    policy_path: PathBuf,
    policy: watch::Receiver<Arc<Policy>>,
    /// Also held by the reload loop; an authenticated reload request replaces the policy now.
    policy_tx: Arc<watch::Sender<Arc<Policy>>>,
    tunnels: Arc<Semaphore>,
    log: mpsc::Sender<Event>,
    shutdown: watch::Sender<bool>,
}

struct Event {
    sandbox: String,
    host: String,
    port: u16,
    allowed: bool,
    rule: String,
}

/// The proxy token: read from the state dir, created (mode 600) on first use.
pub(crate) fn token(dirs: &Dirs) -> Result<String> {
    let path = dirs.token_file();
    match std::fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => return Ok(text.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    }
    let token = random_token()?;
    crate::paths::write_private(&path, format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// 32 random bytes from the OS, as hex.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    let mut source = std::fs::File::open("/dev/urandom").context("opening /dev/urandom")?;
    std::io::Read::read_exact(&mut source, &mut bytes).context("reading /dev/urandom")?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing to a String cannot fail.
            let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
            hex
        }))
}

/// `msbx proxy serve`: runs until `msbx proxy stop`, SIGTERM or Ctrl-C.
pub(crate) async fn serve(dirs: Dirs) -> Result<()> {
    let listener = match TcpListener::bind(ADDRESS).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            return match status().await? {
                Some(running) => bail!(
                    "msbx proxy {} is already running (pid {})",
                    running.msbx,
                    running.pid
                ),
                None => Err(error).with_context(|| format!("binding {ADDRESS}")),
            };
        }
        Err(error) => return Err(error).with_context(|| format!("binding {ADDRESS}")),
    };
    eprintln!(
        "msbx proxy {VERSION} listening on {ADDRESS} (pid {})",
        std::process::id()
    );
    serve_on(listener, dirs, Timing::default()).await
}

/// Serves on an already bound listener. Returns after a stop request or signal, with the log
/// flushed.
pub(crate) async fn serve_on(listener: TcpListener, dirs: Dirs, timing: Timing) -> Result<()> {
    let token = token(&dirs)?;
    let policy_path = dirs.policy_file();
    let initial = match policy::load(&policy_path) {
        Ok(Some(policy)) => policy,
        Ok(None) => {
            eprintln!(
                "warning: {} does not exist; blocking everything until it does",
                policy_path.display()
            );
            Policy::new(Preset::LockedDown)
        }
        Err(error) => {
            eprintln!("warning: {error:#}; blocking everything until it parses");
            Policy::new(Preset::LockedDown)
        }
    };
    let (policy_tx, policy_rx) = watch::channel(Arc::new(initial));
    let policy_tx = Arc::new(policy_tx);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let (log_tx, log_rx) = mpsc::channel(1024);
    let reloader = tokio::spawn(reload_loop(
        policy_path.clone(),
        policy_tx.clone(),
        timing.reload,
    ));
    let logger = tokio::spawn(log_loop(
        dirs.network_log(),
        log_rx,
        shutdown_rx.clone(),
        timing.flush,
    ));
    let shared = Arc::new(Shared {
        token,
        policy_path,
        policy: policy_rx,
        policy_tx,
        tunnels: Arc::new(Semaphore::new(timing.max_tunnels)),
        log: log_tx,
        shutdown: shutdown_tx,
    });
    let pending = Arc::new(Semaphore::new(MAX_PENDING));
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("listening for SIGTERM")?;
    #[cfg(not(unix))]
    let mut terminate = ();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accepting a proxy connection")?;
                // Never wait here: a full queue drops the newcomer instead of stalling accept.
                let Ok(permit) = pending.clone().try_acquire_owned() else {
                    continue;
                };
                let shared = shared.clone();
                tokio::spawn(async move {
                    // A failed connection only concerns that guest; the proxy carries on.
                    let _ = handle(stream, shared, permit).await;
                });
            }
            _ = shutdown_rx.changed() => break,
            _ = tokio::signal::ctrl_c() => break,
            () = terminate_signal(&mut terminate) => break,
        }
    }
    shared.shutdown.send_replace(true);
    reloader.abort();
    logger.await.context("flushing the network log")?
}

#[cfg(unix)]
async fn terminate_signal(signal: &mut tokio::signal::unix::Signal) {
    signal.recv().await;
}

#[cfg(not(unix))]
async fn terminate_signal(_: &mut ()) {
    std::future::pending::<()>().await;
}

/// Reloads `policy.toml` when its modification time or size changes, which catches hand
/// edits. msbx's own edits also ask for a reload at once (`reload`). A file that fails to parse
/// keeps the last good policy and logs a warning.
async fn reload_loop(path: PathBuf, policy: Arc<watch::Sender<Arc<Policy>>>, every: Duration) {
    let mut last = stamp(&path).await;
    let mut ticks = interval(every);
    loop {
        ticks.tick().await;
        let current = stamp(&path).await;
        if current.is_none() || current == last {
            continue;
        }
        last = current;
        if let Err(error) = load_into(&path, &policy).await {
            eprintln!("warning: {error:#}; keeping the previous policy");
        }
    }
}

/// Reads `policy.toml` and makes it the live policy. A missing file keeps the current one.
async fn load_into(path: &Path, policy: &watch::Sender<Arc<Policy>>) -> Result<()> {
    let path = path.to_path_buf();
    let loaded = tokio::task::spawn_blocking(move || policy::load(&path))
        .await
        .context("reloading the policy")??;
    if let Some(fresh) = loaded {
        policy.send_replace(Arc::new(fresh));
    }
    Ok(())
}

async fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Owns the log: records events, flushes when dirty, and once more at shutdown.
async fn log_loop(
    path: PathBuf,
    mut events: mpsc::Receiver<Event>,
    mut shutdown: watch::Receiver<bool>,
    every: Duration,
) -> Result<()> {
    let mut log = match tokio::task::spawn_blocking({
        let path = path.clone();
        move || Log::load(&path)
    })
    .await
    {
        Ok(Ok(log)) => log,
        Ok(Err(error)) => {
            eprintln!("warning: {error:#}; starting an empty log");
            Log::default()
        }
        Err(error) => bail!("loading the network log: {error}"),
    };
    let mut ticks = interval(every);
    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                log.record(&event.sandbox, &event.host, event.port, event.allowed, &event.rule, now());
            }
            _ = ticks.tick() => flush(&mut log, &path).await?,
            _ = shutdown.changed() => {
                flush(&mut log, &path).await?;
                return Ok(());
            }
        }
    }
}

async fn flush(log: &mut Log, path: &Path) -> Result<()> {
    if !log.is_dirty() {
        return Ok(());
    }
    let mut snapshot = log.clone();
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || snapshot.save(&path))
        .await
        .context("saving the network log")??;
    // Events recorded while the save ran stay dirty: the next flush writes them.
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

async fn handle(
    mut stream: TcpStream,
    shared: Arc<Shared>,
    pending: OwnedSemaphorePermit,
) -> Result<()> {
    let head = timeout(HEAD_TIMEOUT, read_head(&mut stream))
        .await
        .context("waiting for the request head")??;
    match wire::parse_request(&head) {
        Ok(Request::Connect { target, auth }) => {
            let Some(auth) = auth.filter(|auth| same(&auth.password, &shared.token)) else {
                return stream.write_all(UNAUTHORISED).await.context("writing 407");
            };
            let Ok(_tunnel) = shared.tunnels.clone().try_acquire_owned() else {
                return stream
                    .write_all(SERVICE_UNAVAILABLE)
                    .await
                    .context("writing 503");
            };
            // The tunnel now counts against the tunnel limit, not the pending one.
            drop(pending);
            tunnel(stream, target, auth.user, &shared).await
        }
        Ok(Request::Status) => {
            let status = Status {
                msbx: VERSION.to_owned(),
                pid: std::process::id(),
                policy: shared.policy_path.clone(),
            };
            let body = serde_json::to_string(&status).context("serialising the status")?;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(reply.as_bytes())
                .await
                .context("writing the status")
        }
        Ok(Request::Stop { auth }) => {
            if !auth.is_some_and(|auth| same(&auth.password, &shared.token)) {
                return stream.write_all(UNAUTHORISED).await.context("writing 407");
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .context("writing the stop reply")?;
            shared.shutdown.send_replace(true);
            Ok(())
        }
        Ok(Request::Reload { auth }) => {
            if !auth.is_some_and(|auth| same(&auth.password, &shared.token)) {
                return stream.write_all(UNAUTHORISED).await.context("writing 407");
            }
            let reply: &[u8] = match load_into(&shared.policy_path, &shared.policy_tx).await {
                Ok(()) => b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                Err(error) => {
                    eprintln!("warning: {error:#}; keeping the previous policy");
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                }
            };
            stream
                .write_all(reply)
                .await
                .context("writing the reload reply")
        }
        Ok(Request::Other) | Err(_) => stream.write_all(BAD_REQUEST).await.context("writing 400"),
    }
}

/// Reads through the blank line that ends the request head.
async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = wire::head_end(&head) {
            head.truncate(end);
            return Ok(head);
        }
        ensure!(
            head.len() <= wire::MAX_HEAD,
            "request head longer than {} bytes",
            wire::MAX_HEAD
        );
        let read = stream
            .read(&mut chunk)
            .await
            .context("reading the request head")?;
        ensure!(read > 0, "connection closed before the request head ended");
        head.extend_from_slice(&chunk[..read]);
    }
}

/// Equal length and bytes, without an early exit on the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn tunnel(
    mut guest: TcpStream,
    target: SocketAddr,
    sandbox: String,
    shared: &Shared,
) -> Result<()> {
    guest.write_all(ESTABLISHED).await.context("writing 200")?;
    let (first, sniffed) = sniff(&mut guest).await;
    let (name, mismatch) = match sniffed.filter(|name| !is_ip_literal(name)) {
        Some(name) if resolves_to(&name, target).await => (Some(name), None),
        Some(name) => (None, Some(name)),
        None => (None, None),
    };
    let policy = shared.policy.borrow().clone();
    let mut decision = policy.decide(
        &sandbox,
        name.as_deref(),
        Some(target.ip()),
        Some(target.port()),
    );
    if let Some(claimed) = mismatch {
        decision.rule = format!("{} (name-mismatch {claimed})", decision.rule);
    }
    let allowed = decision.verdict == Verdict::Allow;
    // A full log channel or a stopped logger must not stall the connection.
    let _ = shared.log.try_send(Event {
        sandbox,
        host: name.unwrap_or_else(|| target.ip().to_string()),
        port: target.port(),
        allowed,
        rule: decision.rule,
    });
    if !allowed {
        return Ok(()); // dropping `guest` closes it
    }
    let mut upstream = timeout(CONNECT_TIMEOUT, TcpStream::connect(target))
        .await
        .with_context(|| format!("connecting to {target}"))?
        .with_context(|| format!("connecting to {target}"))?;
    upstream
        .write_all(&first)
        .await
        .context("forwarding the first bytes")?;
    // Either side closing ends the tunnel; the byte counts are not needed.
    let _ = tokio::io::copy_bidirectional(&mut guest, &mut upstream).await;
    Ok(())
}

/// The guest's first bytes (forwarded later) and the hostname found in them, if any.
async fn sniff(guest: &mut TcpStream) -> (Vec<u8>, Option<String>) {
    let deadline = Instant::now() + SNIFF_TIMEOUT;
    let mut first = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        match wire::hostname(&first) {
            Sniff::Found(name) => return (first, Some(name)),
            Sniff::None => return (first, None),
            Sniff::NeedMore if first.len() >= SNIFF_LIMIT => return (first, None),
            Sniff::NeedMore => {}
        }
        match timeout_at(deadline, guest.read(&mut chunk)).await {
            Ok(Ok(read)) if read > 0 => first.extend_from_slice(&chunk[..read]),
            _ => return (first, None),
        }
    }
}

fn is_ip_literal(name: &str) -> bool {
    name.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_ok()
}

/// Anti-spoofing: the name only counts when the host resolves it to the CONNECT address.
/// Round-robin and CDN answers can fail this, which is then logged as `name-mismatch`.
async fn resolves_to(name: &str, target: SocketAddr) -> bool {
    match timeout(
        LOOKUP_TIMEOUT,
        tokio::net::lookup_host((name, target.port())),
    )
    .await
    {
        Ok(Ok(mut addresses)) => addresses.any(|address| address.ip() == target.ip()),
        _ => false,
    }
}

/// `msbx proxy status`: `None` when nothing listens on the port; an error when something that
/// is not an msbx proxy does.
pub(crate) async fn status() -> Result<Option<Status>> {
    status_at(ADDRESS.parse().context("parsing the proxy address")?).await
}

async fn status_at(address: SocketAddr) -> Result<Option<Status>> {
    let stream = match timeout(PROBE_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
        Ok(Err(error)) => return Err(error).with_context(|| format!("connecting to {address}")),
        Err(_) => bail!("connecting to {address} timed out"),
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {address}\r\n\r\n",
        wire::STATUS_PATH
    );
    let reply = exchange(stream, request.as_bytes()).await;
    let not_msbx = || {
        anyhow::anyhow!(
            "port {} is in use by something that is not the msbx proxy",
            address.port()
        )
    };
    let Ok(reply) = reply else {
        return Err(not_msbx());
    };
    let body = reply
        .strip_prefix("HTTP/1.1 200")
        .and_then(|rest| rest.split_once("\r\n\r\n"))
        .map(|(_, body)| body)
        .ok_or_else(not_msbx)?;
    let status: Status = serde_json::from_str(body).map_err(|_| not_msbx())?;
    Ok(Some(status))
}

/// Writes a request, then reads the whole reply.
async fn exchange(mut stream: TcpStream, request: &[u8]) -> Result<String> {
    stream
        .write_all(request)
        .await
        .context("writing the request")?;
    let mut reply = Vec::new();
    timeout(SNIFF_TIMEOUT, stream.read_to_end(&mut reply))
        .await
        .context("waiting for the reply")?
        .context("reading the reply")?;
    String::from_utf8(reply).context("reply is not UTF-8")
}

/// `msbx proxy stop`: asks the running proxy to flush its log and exit.
pub(crate) async fn stop(dirs: &Dirs) -> Result<()> {
    let Some(running) = status().await? else {
        eprintln!("no msbx proxy is running on {ADDRESS}");
        return Ok(());
    };
    let reply = authed_post(dirs, wire::STOP_PATH).await?;
    ensure!(
        reply.starts_with("HTTP/1.1 200"),
        "the proxy (pid {}) refused the stop request: {}",
        running.pid,
        reply.lines().next().unwrap_or_default()
    );
    eprintln!("stopped msbx proxy {} (pid {})", running.msbx, running.pid);
    Ok(())
}

/// Asks a running proxy to re-read `policy.toml` now, so an edit applies to the very next
/// connection. Nothing to do when no proxy runs (it reads the file when it starts), or when the
/// running proxy serves a different policy file (another `XDG_CONFIG_HOME`).
pub(crate) async fn reload(dirs: &Dirs) -> Result<()> {
    match status().await? {
        Some(running) if running.policy == dirs.policy_file() => {}
        _ => return Ok(()),
    }
    let reply = authed_post(dirs, wire::RELOAD_PATH).await?;
    ensure!(
        reply.starts_with("HTTP/1.1 200"),
        "the proxy could not reload {}; it keeps the previous policy (see {})",
        dirs.policy_file().display(),
        dirs.proxy_log().display()
    );
    Ok(())
}

/// Sends `POST path` with the proxy token as the Basic password and returns the whole reply.
async fn authed_post(dirs: &Dirs, path: &str) -> Result<String> {
    let token = token(dirs)?;
    let address: SocketAddr = ADDRESS.parse().context("parsing the proxy address")?;
    let stream = timeout(PROBE_TIMEOUT, TcpStream::connect(address))
        .await
        .context("connecting to the proxy")?
        .context("connecting to the proxy")?;
    let credentials = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!(":{token}"),
    );
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nProxy-Authorization: Basic {credentials}\r\n\r\n"
    );
    exchange(stream, request.as_bytes()).await
}

/// Makes sure a proxy answers on the port, starting a detached `msbx proxy serve` if needed.
pub(crate) async fn ensure_running(dirs: &Dirs) -> Result<()> {
    match status().await? {
        Some(running) if running.msbx == VERSION => return Ok(()),
        Some(running) => {
            eprintln!(
                "note: msbx proxy {} (pid {}) is running, but this is msbx {VERSION}; run \
                 `msbx proxy stop` and it will be restarted on the next launch",
                running.msbx, running.pid
            );
            return Ok(());
        }
        None => {}
    }
    spawn_detached(dirs)?;
    let deadline = Instant::now() + SPAWN_WAIT;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if status().await?.is_some() {
            return Ok(());
        }
    }
    bail!(
        "the msbx proxy did not start within {} s; see {}",
        SPAWN_WAIT.as_secs(),
        dirs.proxy_log().display()
    )
}

/// Starts `msbx proxy serve` in its own process group with its output in `proxy.log`, so it
/// outlives this process like the detached sandboxes do.
fn spawn_detached(dirs: &Dirs) -> Result<()> {
    let exe = std::env::current_exe().context("locating the msbx executable")?;
    crate::paths::create_private_dir(&dirs.state)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let log = options
        .open(dirs.proxy_log())
        .with_context(|| format!("opening {}", dirs.proxy_log().display()))?;
    let mut command = std::process::Command::new(&exe);
    command
        .args(["proxy", "serve"])
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .context("duplicating the proxy log handle")?,
        )
        .stderr(log);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command
        .spawn()
        .with_context(|| format!("starting {} proxy serve", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Target;

    struct Fixture {
        root: PathBuf,
        dirs: Dirs,
    }

    impl Fixture {
        fn new(tag: &str) -> Result<Self> {
            let root =
                std::env::temp_dir().join(format!("msbx-proxy-{tag}-{}", std::process::id()));
            let dirs = Dirs {
                config: root.join("config"),
                state: root.join("state"),
            };
            crate::paths::write_private(&dirs.token_file(), b"test-token\n")?;
            Ok(Self { root, dirs })
        }

        fn write_policy(&self, allow: &[String]) -> Result<()> {
            let mut policy = Policy::new(Preset::LockedDown);
            for target in allow {
                policy.allow.push(Target::parse(target)?);
            }
            policy::save(&self.dirs.policy_file(), &policy)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A server that echoes everything it reads.
    async fn echo_server() -> Result<SocketAddr> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut reader, mut writer) = stream.split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                });
            }
        });
        Ok(address)
    }

    fn basic(user: &str, password: &str) -> String {
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{user}:{password}"),
        )
    }

    /// Sends a CONNECT and returns the proxy's status line.
    async fn connect(
        proxy: SocketAddr,
        target: SocketAddr,
        auth: Option<(&str, &str)>,
    ) -> Result<(TcpStream, String)> {
        let mut stream = TcpStream::connect(proxy).await?;
        let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
        if let Some((user, password)) = auth {
            request.push_str("Proxy-Authorization: Basic ");
            request.push_str(&basic(user, password));
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while wire::head_end(&head).is_none() {
            ensure!(
                stream.read(&mut byte).await? == 1,
                "proxy closed before replying"
            );
            head.push(byte[0]);
        }
        let line = String::from_utf8(head)?
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok((stream, line))
    }

    /// Whether bytes sent through an established tunnel come back from the echo server.
    async fn relays(mut stream: TcpStream) -> Result<bool> {
        // No name in the first bytes, so the decision is on the IP alone.
        stream.write_all(b"ping").await?;
        let mut reply = vec![0u8; 64];
        let read = timeout(Duration::from_secs(2), stream.read(&mut reply)).await;
        Ok(matches!(read, Ok(Ok(count)) if count > 0))
    }

    async fn stop_at(proxy: SocketAddr, token: &str) -> Result<String> {
        let stream = TcpStream::connect(proxy).await?;
        let request = format!(
            "POST {} HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
            wire::STOP_PATH,
            basic("", token)
        );
        exchange(stream, request.as_bytes()).await
    }

    async fn reload_at(proxy: SocketAddr, token: &str) -> Result<String> {
        let stream = TcpStream::connect(proxy).await?;
        let request = format!(
            "POST {} HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
            wire::RELOAD_PATH,
            basic("", token)
        );
        exchange(stream, request.as_bytes()).await
    }

    /// msbx's own edits must not wait for the file check: a reload request applies the policy
    /// before it replies, so the very next connection follows it.
    #[tokio::test]
    async fn reload_request_applies_an_edit_before_replying() -> Result<()> {
        let fixture = Fixture::new("reload")?;
        let target = echo_server().await?;
        fixture.write_policy(&[])?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = listener.local_addr()?;
        // A file check far in the future, so only the reload request can apply the edit.
        let timing = Timing {
            reload: Duration::from_secs(3600),
            flush: Duration::from_millis(20),
            ..Timing::default()
        };
        let server = tokio::spawn(serve_on(listener, fixture.dirs.clone(), timing));
        status_at(proxy).await?.context("status missing")?;

        let (stream, _) = connect(proxy, target, Some(("box", "test-token"))).await?;
        assert!(!relays(stream).await?, "blocked before the edit");
        fixture.write_policy(&[target.to_string()])?;
        assert!(reload_at(proxy, "wrong").await?.starts_with("HTTP/1.1 407"));
        assert!(
            reload_at(proxy, "test-token")
                .await?
                .starts_with("HTTP/1.1 200")
        );
        let (stream, _) = connect(proxy, target, Some(("box", "test-token"))).await?;
        assert!(
            relays(stream).await?,
            "allowed right after the reload reply"
        );

        assert!(
            stop_at(proxy, "test-token")
                .await?
                .starts_with("HTTP/1.1 200")
        );
        server.await??;
        Ok(())
    }

    /// With every tunnel slot taken, new tunnels get 503 but status, reload and stop still
    /// answer, so `ensure_running` never mistakes a busy proxy for a missing one.
    #[tokio::test]
    async fn control_requests_answer_when_every_tunnel_is_busy() -> Result<()> {
        let fixture = Fixture::new("busy")?;
        let target = echo_server().await?;
        fixture.write_policy(&[target.to_string()])?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = listener.local_addr()?;
        let timing = Timing {
            max_tunnels: 1,
            ..Timing::default()
        };
        let server = tokio::spawn(serve_on(listener, fixture.dirs.clone(), timing));
        status_at(proxy).await?.context("status missing")?;

        let (mut busy, line) = connect(proxy, target, Some(("box", "test-token"))).await?;
        assert_eq!(line, "HTTP/1.1 200 Connection established");
        busy.write_all(b"ping").await?;
        let (_, line) = connect(proxy, target, Some(("box", "test-token"))).await?;
        assert_eq!(line, "HTTP/1.1 503 Service Unavailable");
        status_at(proxy).await?.context("status while busy")?;
        assert!(
            reload_at(proxy, "test-token")
                .await?
                .starts_with("HTTP/1.1 200")
        );

        drop(busy);
        assert!(
            stop_at(proxy, "test-token")
                .await?
                .starts_with("HTTP/1.1 200")
        );
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn proxy_authenticates_decides_relays_reloads_and_logs() -> Result<()> {
        let fixture = Fixture::new("relay")?;
        let allowed = echo_server().await?;
        let blocked = echo_server().await?;
        fixture.write_policy(&[allowed.to_string()])?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy = listener.local_addr()?;
        let timing = Timing {
            reload: Duration::from_millis(20),
            flush: Duration::from_millis(20),
            ..Timing::default()
        };
        let server = tokio::spawn(serve_on(listener, fixture.dirs.clone(), timing));

        // Status answers without credentials; stop needs them.
        let status = status_at(proxy).await?.context("status missing")?;
        assert_eq!(status.msbx, VERSION);
        assert_eq!(status.pid, std::process::id());
        assert!(stop_at(proxy, "wrong").await?.starts_with("HTTP/1.1 407"));

        // 407 without or with wrong credentials, and nothing else is sent.
        let (mut stream, line) = connect(proxy, allowed, None).await?;
        assert_eq!(line, "HTTP/1.1 407 Proxy Authentication Required");
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await?;
        assert_eq!(rest, [] as [u8; 0]);
        let (_, line) = connect(proxy, allowed, Some(("box", "wrong"))).await?;
        assert!(line.starts_with("HTTP/1.1 407"), "{line}");

        // An allowed target relays bytes, a blocked one is closed after the 200.
        let (stream, line) = connect(proxy, allowed, Some(("box", "test-token"))).await?;
        assert_eq!(line, "HTTP/1.1 200 Connection established");
        assert!(relays(stream).await?);
        let (stream, line) = connect(proxy, blocked, Some(("box", "test-token"))).await?;
        assert_eq!(line, "HTTP/1.1 200 Connection established");
        assert!(!relays(stream).await?);

        // Allowing the blocked target in the file changes the next connection.
        fixture.write_policy(&[allowed.to_string(), blocked.to_string()])?;
        let mut relayed = false;
        for _ in 0..100 {
            let (stream, _) = connect(proxy, blocked, Some(("box", "test-token"))).await?;
            if relays(stream).await? {
                relayed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(relayed, "the policy edit was not picked up");

        // A broken file keeps the last good policy.
        std::fs::write(fixture.dirs.policy_file(), "preset = \"open\"\nnope = 1\n")?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (stream, _) = connect(proxy, allowed, Some(("box", "test-token"))).await?;
        assert!(relays(stream).await?);

        // Stop flushes the log and ends serve_on.
        assert!(
            stop_at(proxy, "test-token")
                .await?
                .starts_with("HTTP/1.1 200")
        );
        timeout(Duration::from_secs(5), server).await???;
        let log = Log::load(&fixture.dirs.network_log())?;
        let rows: Vec<(String, u16, bool, String)> = log
            .entries()
            .iter()
            .map(|entry| {
                (
                    entry.host.clone(),
                    entry.port,
                    entry.allowed,
                    entry.rule.clone(),
                )
            })
            .collect();
        assert!(
            rows.contains(&(
                "127.0.0.1".to_owned(),
                allowed.port(),
                true,
                format!("allow {allowed} (global)")
            )),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(
                "127.0.0.1".to_owned(),
                blocked.port(),
                false,
                "default".to_owned()
            )),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(
                "127.0.0.1".to_owned(),
                blocked.port(),
                true,
                format!("allow {blocked} (global)")
            )),
            "{rows:?}"
        );
        let allowed_row = log
            .entries()
            .iter()
            .find(|entry| entry.port == allowed.port() && entry.allowed)
            .context("allowed row")?;
        assert_eq!(allowed_row.sandbox, "box");
        assert!(allowed_row.count >= 2, "{allowed_row:?}");
        assert!(status_at(proxy).await?.is_none(), "the proxy stopped");
        Ok(())
    }

    #[tokio::test]
    async fn status_tells_an_msbx_proxy_from_other_listeners() -> Result<()> {
        let echo = echo_server().await?;
        let error = status_at(echo)
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(error.contains("not the msbx proxy"), "{error}");
        let unused = TcpListener::bind("127.0.0.1:0").await?;
        let address = unused.local_addr()?;
        drop(unused);
        assert_eq!(status_at(address).await?, None);
        Ok(())
    }

    #[test]
    fn token_is_created_once_with_mode_600() -> Result<()> {
        let fixture = Fixture::new("token")?;
        std::fs::remove_file(fixture.dirs.token_file())?;
        let first = token(&fixture.dirs)?;
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(token(&fixture.dirs)?, first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(fixture.dirs.token_file())?
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_ne!(random_token()?, first);
        Ok(())
    }

    #[test]
    fn same_compares_whole_strings() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "ab"));
        assert!(!same("", "a"));
        assert!(same("", ""));
    }
}
