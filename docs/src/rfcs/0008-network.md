# RFC 0008: Network: per-host policy through an msbx proxy

- Status: Implemented
- Tracking issue: [#13](https://github.com/finngi/msbx/issues/13)

## Summary

Every msbx sandbox sends its outbound TCP traffic through a small HTTP CONNECT proxy that msbx runs on `127.0.0.1:47447`. The proxy applies `~/.config/msbx/policy.toml`, a Docker Sandboxes (`sbx`) style allow and deny list with three presets, to each connection by host name. Changes apply at once to new connections. The proxy keeps a log of what it allowed and blocked. You manage the policy with `msbx policy …` or from the dashboard's Network tab.

## Motivation

People run coding agents in msbx so they can leave them working unattended. Before this change a sandbox could reach the whole public internet, you could not see what it contacted, and the only way to narrow access was to recreate the sandbox with a different static policy.

microsandbox fixes a sandbox's network policy and proxy when the sandbox is created (`SandboxModificationPatch` has no network field; `msb modify` has no network flags). It does, however, let a sandbox send its traffic through an outbound proxy of our choice. Putting the policy in that proxy is the one channel microsandbox offers for live control, so this belongs in msbx.

## Guide-level explanation

### First run

The first `msbx agent` (or the dashboard's first create) asks once:

```text
Choose a network policy for msbx sandboxes:
  1. Open        everything is allowed unless you deny it
  2. Balanced    block by default; AI APIs, package registries and GitHub allowed
  3. Locked Down block everything you have not allowed
Preset [1-3, default 2]:
```

Without a terminal, msbx stops with `run msbx policy init <open|balanced|locked-down>`.

### Rules

```sh
msbx policy allow pypi.org,files.pythonhosted.org   # everywhere
msbx policy deny "**.example.com" --sandbox codex-api
msbx policy check pypi.org:443                      # Allowed / Blocked, exit 1 when blocked
msbx policy ls --sandbox codex-api
msbx policy log codex-api --blocked
msbx policy preset locked-down
```

| Target | Matches |
|---|---|
| `example.com` | that host, any port; not its subdomains |
| `*.example.com` | one extra label: `api.example.com`; neither `v2.api.example.com` nor `example.com` |
| `**.example.com` | one or more extra labels; not `example.com` itself |
| `**` | everything |
| any of the above with `:443` | that port only |
| `10.0.0.1`, `[2001:db8::1]:443` | that address |
| `10.0.0.0/8`, `2001:db8::/32` | that network, any port |

Deny beats allow, whatever the source. Then any allow wins. Then the preset decides: Open allows, Balanced and Locked Down block. Balanced carries a short baseline (each agent's API host, Codex sign-in hosts, npm, Debian, PyPI, crates.io, Go, GitHub); `msbx policy ls` shows it with the source `preset`.

### Dashboard

The Network tab lists the selected sandbox's connections, blocked first, newest first:

```text
codex-api
Overview   Logs   Network
──────────────────────────────────────────────
policy balanced · ↓ 48M  ↑ 3.1M · 2 blocked

  ✗ pypi.org:443                 7  12:04:51
▸ ✓ api.openai.com:443         42  12:04:50
```

`←`/`→` move the cursor, `a` allows the host for this sandbox, `A` everywhere, `d` denies it here, `p` picks a preset. Each key writes `policy.toml` and asks the proxy to reload it, so the next connection follows the change. The Overview's network row reads `policy balanced (proxied)` and a `traffic` row shows bytes in and out. Sandboxes created before this feature show `created before network policy; recreate with msb rm -f NAME`.

## Reference-level explanation

### Channels in microsandbox 0.7.11

| Fact | Source |
|---|---|
| A sandbox can be given one outbound proxy: `SandboxBuilder::proxy(\|p\| p.http_connect(addr).credentials(user, SecretSource::env(VAR)))`. | `microsandbox/lib/sandbox/builder.rs:752`, `microsandbox-network/lib/proxy/socks.rs:447,478`, `proxy/http_connect.rs:81` |
| The password is read with `std::env::var` in the process that boots the VM. | `microsandbox-network/lib/model/config/resolver.rs:161`, `microsandbox/lib/runtime/spawn.rs:315` |
| msb dials the proxy with `CONNECT <ip>:<port>` plus one `Proxy-Authorization: Basic` header; the target is a socket address, never a name. | `proxy/http_connect.rs:149-230` |
| The proxy is used only for destinations that are not translated. Traffic to the host via `host.microsandbox.internal` bypasses it. | `proxy/types.rs:206` `select_for_destination`, `engine/netstack/poll.rs:540` |
| The static policy is still evaluated on each SYN, before the proxy is dialled. Rules are first-match. | `engine/netstack/poll.rs:414`, `model/policy/types.rs:354` |
| With an HTTP CONNECT proxy, non-DNS UDP is dropped. DNS goes through msb's gateway. | `engine/udp/relay.rs:385-415` |
| The proxy applies to TLS-intercepted and plain TCP alike, so secret injection keeps working. | `sandbox/builder.rs` doc comment on `proxy` |
| `Host` is classified before `Private`, so DNS and the model ports reach the gateway while private ranges stay denied. | `engine/policy/destination.rs:25-55` |

### Sandbox creation (`sandbox.rs`)

New sandboxes get the label `msbx.network=proxy`, the proxy `http_connect("127.0.0.1:47447")` with credentials (sandbox name, `SecretSource::env("MSBX_PROXY_TOKEN")`), and this static policy:

| Rule | Why |
|---|---|
| egress deny by default | private networks, loopback, link-local and metadata addresses match nothing |
| allow host, UDP and TCP port 53 | DNS |
| allow host, TCP 11434 and 1234 | Ollama and LM Studio; host traffic never reaches the proxy, so this is fixed at create |
| allow public, TCP | the proxy decides per host |

`msbx.network` lets the dashboard and CLI tell proxied sandboxes from older ones.

### Token (`secrets.rs`, `proxy.rs`)

`$XDG_STATE_HOME/msbx/proxy-token` holds 32 random bytes as hex, mode 600, created on first use. msbx re-runs itself once with `MSBX_PROXY_TOKEN` (and any Keychain secrets) in its environment, the same mechanism as before, so the SDK and `msb` can resolve the credential. `msbx start NAME` passes through `msb` with the variable set; plain `msb start` without it fails to start a proxied sandbox.

### The proxy (`proxy.rs`, `wire.rs`)

1. Read the request head (at most 8 KiB, 10 s).
2. `GET /msbx-status` answers `{"msbx":"<version>","pid":N,"policy":"<path>"}` without credentials. `POST /msbx-stop` needs the token. Anything else that is not a `CONNECT` gets `400`.
3. A `CONNECT` without the token, or with a wrong one, gets `407` with `Proxy-Authenticate: Basic` and nothing else (RFC 9110 §11.7.1, §15.5.8). The Basic username is the sandbox name.
4. Reply `200 Connection established`, then read the guest's first bytes (up to 16 KiB, 2 s). A TLS ClientHello gives the `server_name` extension (RFC 8446 §4.1.2; RFC 6066 §3: type 0, `host_name` entries only, ASCII, no trailing dot), reassembled across records (RFC 8446 §5.1). An HTTP/1.x request gives `Host` without its port (RFC 9110 §7.2, RFC 9112 §3). Server-first protocols send nothing in 2 s and give no name.
5. Anti-spoofing: resolve the name on the host (`tokio::net::lookup_host`, 1.5 s). The name counts only when the CONNECT address is among the answers; otherwise the decision is on the IP alone and the log rule ends in `(name-mismatch <name>)`.
6. Decide with the current policy, log, then either connect to the CONNECT address (never a re-resolved one), write the buffered bytes and `tokio::io::copy_bidirectional`, or close the guest side.

At most 512 tunnels are open at once; past that, new tunnels get `503`. Requests still being read have their own limit, so status, reload and stop always get through. msbx's own edits (`msbx policy`, the TUI keys) send an authenticated reload request, and the proxy applies the file before it replies. Hand edits are picked up when the file's modification time or size changes, checked once a second. A file that fails to parse keeps the last good policy and logs a warning. The log aggregates per (sandbox, host or IP, port, decision, rule) with count and first/last seen, capped at 5,000 rows, flushed atomically to `network-log.json` every 2 s when it changed and at shutdown.

### Lifecycle

`msbx proxy serve` runs in the foreground; `status` probes `/msbx-status`; `stop` sends the authenticated stop. `msbx agent`, the dashboard (when a policy exists), and `msbx <msb command>` pass-through first ensure a proxy answers: if nothing listens, msbx spawns `msbx proxy serve` in a new process group with its output in `$XDG_STATE_HOME/msbx/proxy.log` and waits up to 2 s. A proxy of another msbx version keeps running with a note to run `msbx proxy stop`. Another program on the port is an error.

### Policy file

`$XDG_CONFIG_HOME/msbx/policy.toml`, default `~/.config/msbx/policy.toml` on macOS too (RFC 0002):

```toml
preset = "balanced"
allow = ["pypi.org", "*.example.com:443"]
deny = ["evil.test"]

[sandboxes.codex-api]
allow = ["box-only.test"]
deny = ["**"]
```

Written atomically (temp file and rename, mode 600). Hand edits work; unknown keys are an error naming the key. A missing file means "not initialised".

### Tests

- `policy.rs`: grammar accept and reject tables, matching for each form, deny over allow across preset, global and sandbox, per-sandbox isolation, preset defaults, file round trip, unknown key, mode 600.
- `wire.rs`: SNI from a ClientHello captured from curl (one record, split records, no SNI, garbage), `Host` with and without port, absent, oversized; CONNECT parsing and Basic auth.
- `proxy.rs`: in-process proxy on an ephemeral port with an echo server: 407, relay, block, live edit, broken file, log flush.
- `app.rs` and `ui.rs`: keys and snapshots for the Network tab and preset popup.
- `sandbox.rs`: an ignored real-VM test (Locked Down plus one host, blocked host, live allow, log rows).

## Drawbacks

- One more process. The proxy outlives msbx, like the sandboxes it serves, and must be running for proxied sandboxes to reach anything.
- Port 47447 is baked into every sandbox and cannot change.
- The host name is only as good as the guest's first bytes. Protocols that send no name are decided by IP.
- The anti-spoofing check can block a legitimate connection when DNS answers differ between the guest's lookup and the host's (round-robin, CDNs with short TTLs). The log shows `name-mismatch` so you can allow the IP or CIDR.
- Host access cannot be changed live: it is fixed at create to the model ports.

## Security

- The token stops other local processes, and a sandbox that could reach the port, from spoofing a sandbox name. The username is informational once the token matches.
- The proxy never logs payloads or credentials. The log holds host names, ports, counts and times.
- DNS still resolves any name, and allowed hosts can still receive data.
- Non-TLS, non-HTTP traffic is decided by IP. A guest that lies in its SNI or `Host` is caught when the name does not resolve to the CONNECT address.
- The proxy binds loopback only. Its status endpoint is unauthenticated and reveals the msbx version, pid and policy path.

See the [security model](../explanation/security-model.md).

## Rationale and alternatives

- A CONNECT proxy over SOCKS5. msb sends a socket address either way, and CONNECT has standard Basic authentication the SDK exposes through `credentials`. SOCKS5 would add UDP relay, which this design does not need.
- Deciding in the proxy, with no recreate: microsandbox cannot change a running sandbox's policy, and recreating loses guest state. The proxy is the only live channel.
- One fixed port for all sandboxes: the port is persisted in each sandbox, and one proxy serves all of them.
- Reading first bytes, where the previous design read msb's debug lines: msb does not log allowed connections, and its log lines are internal diagnostics. The proxy sees every public TCP connection.
- Doing nothing: the broad default stays, with nothing to see and no way to tighten it.

## Prior art

- Docker Sandboxes ([policies](https://docs.docker.com/ai/sandboxes/security/policy/), [policy concepts](https://docs.docker.com/ai/sandboxes/governance/concepts/), [monitoring](https://docs.docker.com/ai/sandboxes/governance/monitor-and-enforce/monitoring/)): the presets Open, Balanced and Locked Down; `example.com`, `*.example.com`, `**.example.com`, ports and CIDRs; "Deny wins"; `sbx policy ls|allow|deny|rm|check|init|log`; a log with BLOCKED and ALLOWED sections; rules apply immediately. This design copies the surface and the semantics.
- [tobi/wrap](https://github.com/tobi/wrap): reads denials from microsandbox's debug log. No licence, so ideas only. Superseded here by the proxy, which also sees allowed connections.
- microsandbox's own `--proxy` option and `SandboxBuilder::proxy`: the channel this design is built on.

## Unresolved questions

- Whether a per-sandbox proxy username should also be checked against the sandbox list, so a renamed or removed sandbox's credentials stop working.
- Whether to cache host lookups, trading freshness for fewer `name-mismatch` results.

## Future possibilities

- HTTP-layer rules (method and path), as sbx has.
- An approval flow for blocked hosts from the dashboard.
- Allowing the host group live, if microsandbox gains a way to change a running sandbox's policy.
