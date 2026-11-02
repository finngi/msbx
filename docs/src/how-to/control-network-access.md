# Control network access

msbx sandboxes send their outbound traffic through a proxy msbx runs on your machine. A policy
file says which hosts they may reach. Changes apply to the next connection; nothing needs a
restart.

## Pick a preset

The first `msbx agent` asks once. To choose up front, or in a script:

```sh
msbx policy init balanced       # open | balanced | locked-down
```

| Preset | Default | Baseline |
|---|---|---|
| `open` | allow | none; add `deny` rules for what you want to block |
| `balanced` | block | agent API hosts, Codex sign-in, npm, Debian, PyPI, crates.io, Go, GitHub |
| `locked-down` | block | none |

Change it later with `msbx policy preset <preset>` or `p` in the dashboard.

## Allow or deny a host

```sh
msbx policy allow pypi.org,files.pythonhosted.org
msbx policy allow "**.example.com" --sandbox codex-api
msbx policy deny 169.254.0.0/16
msbx policy rm pypi.org
```

Targets: `example.com` (that host, any port), `*.example.com` (one extra label),
`**.example.com` (any depth below the apex), `**` (everything), any of them with `:443`, an IP, or
a CIDR. Deny beats allow. Quote `*` for your shell.

`--sandbox NAME` scopes the rule to one sandbox; without it the rule applies everywhere.

## See what happened

```sh
msbx policy log                  # BLOCKED, then ALLOWED
msbx policy log codex-api --blocked --limit 20
msbx policy check pypi.org:443   # Allowed or Blocked and the rule; exit 1 when blocked
msbx policy ls --sandbox codex-api
```

In the dashboard, press `Tab` to focus Detail and `[` / `]` to select Network. Rows group the
log by sandbox, host and port. Move with `↑` / `↓`, then `Enter` expands the history. Press `a`
to allow a blocked host for its sandbox, `A` to allow it everywhere, or `b` to block an allowed
host there. Changes affect future connections; historical counts stay unchanged.

Press `n` for traffic across all sandboxes. `/` searches, `o` cycles status and `s` cycles
sandboxes. The Rules tab lists effective preset, global and sandbox rules. Press `a` to add a
rule or `r` to confirm removing an explicit rule; preset rules change with `p`. New rules in
the full view apply globally when All sandboxes is selected, or to the selected sandbox.

## Allow a blocked host from the log

1. Run `msbx policy log --blocked`, or open the Network tab.
2. `msbx policy allow <host>`, or press `a` on the row.
3. Run the command in the sandbox again.

If the rule shows `name-mismatch`, the name the sandbox asked for did not resolve to the address
it connected to (common with CDNs). Allow the address or its network instead, or try again.

## The proxy

`msbx agent`, the dashboard and `msbx start` start the proxy when it is not running. It keeps
running after msbx exits, because the sandboxes do.

```sh
msbx proxy status    # running: msbx 0.1.0 (pid 123), policy ~/.config/msbx/policy.toml
msbx proxy stop      # flushes the log and exits; the next msbx command starts it again
msbx proxy serve     # run it in the foreground yourself
```

Its output goes to `~/.local/state/msbx/proxy.log`. Use `msbx start NAME` to start sandboxes:
msbx puts the proxy token in `msb`'s environment, and plain `msb start` lacks it, so a
proxied sandbox does not start.

## Files

| File | Holds |
|---|---|
| `~/.config/msbx/policy.toml` | preset and rules; you can edit it by hand |
| `~/.local/state/msbx/proxy-token` | the credential sandboxes present to the proxy (mode 600) |
| `~/.local/state/msbx/network-log.json` | the connection log |
| `~/.local/state/msbx/proxy.log` | the proxy's own output |

`XDG_CONFIG_HOME` and `XDG_STATE_HOME` move the first and the rest.

## Older sandboxes

Sandboxes created before this feature do not use the proxy, so the policy does not apply to
them. The Network tab says so. Recreate them with `msb rm -f NAME` and `msbx agent` again.

## What the policy does not cover

- DNS: the sandbox can resolve any name.
- Services on your Mac (`host.microsandbox.internal`): only the host ports, by default Ollama
  (11434) and LM Studio (1234), fixed when the sandbox is created. Change them for new
  sandboxes with `msbx policy host-ports` or `Ctrl-S` in the dashboard.
- Traffic that carries no host name (plain TCP) is decided by IP alone.

See the [security model](../explanation/security-model.md).
