# RFC 0008: Network: see a sandbox's policy and what it blocked

- Status: Proposed
- Tracking issue: #TBD

## Summary

On the Overview tab, you'll be able to see each sandbox's network rules and how much data it has sent and received. Later on, an opt-in `balanced` preset will create sandboxes that deny network access by default. Those sandboxes get a Blocked tab that shows the hosts the agent tried to reach and was refused.

## Motivation

People run coding agents in msbx so they can leave them working unattended without worrying. Today they can't see what a sandbox is allowed to reach, or whether anything was refused. The Overview just says `public + host`, and that default allows almost everything, so there isn't much to look at and no stricter setting to try.

Showing the actual rules now, plus an opt-in stricter preset with a list of refused hosts, makes the policy visible and lets people tighten it.

## Guide-level explanation

### Overview (first release)

The "network" row becomes one line per rule. Rules use the same syntax as `msb create --net-rule`, so you can copy them into your own commands.

```text
network   allow@public
          allow@host
          allow@dns
traffic   ↓ 48.2M  ↑ 3.1M
```

### Blocked tab (with `network = "balanced"`, follow-up release)

```text
┌──────────────────────────┐  codex-api
│ ● codex-api              │  Overview   Logs   Blocked (3)
│ codex           ↵ attach │  ─────────────────────────────────────────
│ ~/dev/xap/api            │  policy balanced · 3 blocked · last 12:04
│ cpu ▮▯▯ 18%  mem 2.1G/32G│  Last seen ↓  Host               Hits
│ stop  exec  remove       │  12:04:51     pypi.org              7
└──────────────────────────┘  12:01:10     169.254.169.254       1
                              11:58:02     cdn.jsdelivr.net      2
                              a allow (shows recreate steps)
```

- Each row is one host, newest first, with the number of times it was refused.
- Press `a` on a row to see the steps for allowing that host.

### Allowing a host

The popup shows these steps. msbx does not run them for you.

1. Add the host to `network_allow` in `~/.config/msbx/config.toml`.
2. Remove the sandbox: `msb rm -f codex-api`.
3. Create it again: `msbx agent codex ~/dev/xap/api`.

Your project folder is mounted from your machine, so recreating the sandbox won't change it. Everything else in the sandbox is gone, including packages you've installed and shell history. msbx reinstalls the agent the next time it runs.

## Reference-level explanation

- Rules come from the sandbox's stored config. Traffic comes from microsandbox's metrics. Both refresh every 2 seconds, along with the sandbox list.
- Sandboxes created with `balanced` run microsandbox at debug log level. microsandbox then logs a line each time it refuses a DNS lookup, TCP connection or TLS handshake. msbx reads those lines, takes the host name (or the IP address when there is no name), and counts refusals per host.

### Config keys

These live in `~/.config/msbx/config.toml`. msbx uses them when it creates a sandbox, so changing them has no effect on existing sandboxes.

| Key | Values | Default | Effect |
|---|---|---|---|
| `network` | `"open"`, `"balanced"` | `"open"` | Policy for new sandboxes |
| `network_allow` | list of hosts; `*.example.com` allowed | `[]` | Extra hosts that `balanced` allows |
| `host` | `true`, `false` | `true` | Whether the sandbox can reach services on your machine |

`open` is how it works today: the public internet plus your machine.

`balanced` blocks everything by default, then allows:
- DNS
- `deb.debian.org` and `security.debian.org`, for system packages
- `registry.npmjs.org`, for agent installs
- `github.com`, `*.github.com` and `*.githubusercontent.com`
- the API host of each secret the agent uses, such as `api.openai.com`
- every host in `network_allow`
- your machine, if `host = true`

### Implementation notes

- Policy rows: in `sandbox.rs`, format `NetworkSpec.policy.rules` as `--net-rule` tokens. Read rx/tx from `SandboxMetrics.net_rx_bytes` and `net_tx_bytes`. Add `network_rules: Vec<String>`, `rx_bytes` and `tx_bytes` to `SandboxInfo`.
- Blocked tab: create `balanced` sandboxes with `.log_level(LogLevel::Debug)`. Call `handle.logs` with `sources: vec![LogSource::System]` and keep lines containing "denied by". Take the host from `domain=`, then `sni=`, then `dst=`. Keep `parse_denial(&str) -> Option<String>` and `tally(&[Denial])` pure, and test them against sample lines.
- There is no way to modify the policy: `SandboxModificationPatch` has no network field, and `msb modify` has no `--net` flags.
- Prior art: tobi/wrap reads denials the same way. It has no licence, so use the idea only, not the code.

## Drawbacks

- microsandbox does not log allowed connections. msbx can show total bytes sent and received, but not which hosts were contacted, so there is no "Allowed" list.
- Refused connections are only logged at debug level, and only for sandboxes created with that level. Sandboxes created with `open`, or before this feature existed, have no Blocked tab.
- The log lines are microsandbox's internal diagnostics, not a stable format, so a microsandbox upgrade can break the Blocked tab. msbx pins its microsandbox version for this reason.
- The runtime log only keeps recent history, so older refusals drop out of the list.
- A refused connection to a bare IP address shows the IP, not a host name.
- You cannot change a sandbox's network policy after creating it, whether it is running or stopped. That is why allowing a host means recreating the sandbox.
- With `open`, very little is refused: only private networks and cloud metadata addresses. The Blocked tab would be almost empty, which is why it only comes with `balanced`.

## Security

- With `host = true`, the sandbox can reach every service listening on your machine's 127.0.0.1. That includes databases, Ollama, other sandboxes' published ports and msbx's own port forwards. Set `host = false` if your agent does not need a local model.
- `balanced` limits where a misbehaving agent can send data. It does not stop it entirely: allowed hosts like `github.com` can still receive data.
- Debug logs record the host names the agent looked up. The logs stay in microsandbox's directory on your machine.

## Rationale and alternatives

- **Overview rows now, instead of the full sbx table:** microsandbox does not record allowed connections, so an "Allowed / Blocked" table with hit counts would mostly be invented. Listing the rules and total traffic is cheap, accurate and useful today.
- **Blocked tab only with `balanced`:** under `open` almost nothing is refused, so the tab would sit empty. It is worth building only once a stricter policy exists.
- **Reading microsandbox's debug log lines, instead of waiting for an upstream API:** this works today with the pinned version. It is fragile because the lines are internal diagnostics. Waiting means no feature until microsandbox adds a policy-event API. We read the logs now, keep the parsing in one small tested function, and ask upstream for a proper API.
- **"Allow" shows the recreate steps instead of applying anything:** microsandbox cannot change a sandbox's policy after it is created. Recreating automatically would silently delete guest state, so we show the steps and let the user decide.
- **`balanced` contents:** only what msbx's own install scripts and agents need (Debian packages, npm, GitHub, each agent's API host), plus `network_allow`. A long curated list like sbx's would be guesswork to maintain. Users can add hosts as they see refusals.
- **Not doing it:** users keep the broad default, can't see what the agent may reach, and have no easy way to lock it down.

## Prior art

- Docker sbx ([usage](https://docs.docker.com/ai/sandboxes/usage/), [policies](https://docs.docker.com/ai/sandboxes/security/policy/)): a network panel with a connection log (host, hits, allowed or blocked), inline allow/block, and the presets Open, Balanced and Locked Down. Rule changes there apply live.
- [tobi/wrap](https://github.com/tobi/wrap): `wrap log` lists denied egress by reading microsandbox system logs at debug level. `wrap allow HOST` adds the host to a config file. The repo has no licence, so we take the ideas only, no code.
- [skalenetwork/microsandbox-reef](https://github.com/skalenetwork/microsandbox-reef) (MIT): an msb-based agent manager. Useful for its handling of the SDK, not for network policy.
- [Freudenjunge/microsandbox-tui](https://github.com/Freudenjunge/microsandbox-tui): another TUI for microsandbox. Its licence is unclear, so ideas only.

## Unresolved questions

- What log level does the msb runtime use by default? This needs a real-VM check.
- How noisy and how large does the runtime log get at debug level during a long agent session?
- Does codex sign-in need `chatgpt.com` or `auth.openai.com` in `balanced`?
- Should `balanced` keep `host = true` by default, given that pi needs a local model?
- Will microsandbox accept a request for a stable policy-event API, or for changing the policy on a running sandbox?

## Future possibilities

- Allowing or blocking a host on a running sandbox. microsandbox has no way to do this.
- Editing `network_allow` from the TUI. The popup only shows the steps.
- Recreating sandboxes automatically.
- An "Allowed" list or traffic per host.
- Presets other than `open` and `balanced`.
