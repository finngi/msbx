# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before 1.0.0, minor versions may contain breaking changes.

## [Unreleased]

### Added

- Dashboard create wizard with agent descriptions, credential availability, an editable name and a workspace folder picker.
- Expandable network rows with endpoint history and allowed/blocked counts; `n` opens traffic across all sandboxes with search, status and sandbox filters.
- Dashboard Rules view with preset, global and sandbox rules; add rules or confirm removal of explicit rules.
- Contextual help and footer from one keymap, narrow-terminal layouts and `NO_COLOR` support.

- `msbx agent -d` creates or starts the agent's sandbox without attaching.

- `msbx agent <codex|pi|shell> [PATH] [-- ARGS]`: create or reuse a per-project sandbox named `<agent>-<folder>` and attach to it.
- Sandbox defaults: project bind-mounted at the same path, all host CPUs, 32 GiB memory limit, public internet plus host services, agent API keys injected host-side, `node:24-bookworm-slim` image with the agent installed once.
- `msbx` terminal dashboard: sandbox cards, Overview and Logs tabs, and keys to attach, open a shell, create, start/stop and remove sandboxes.
- Pass-through to `msb`: any command msbx does not define runs as `msb` with the same arguments, so `msbx ls` is `msb ls` and `msbx run IMAGE` is `msb run IMAGE`.
- `msbx secret set|rm <VAR>`: keep an agent API key in the macOS Keychain; msbx reads it at startup when the variable is not set in the shell.
- Shared sign-ins (RFC 0004, `agent` scope): Codex and pi `auth.json` files are linked to a per-agent volume `msbx-login-<agent>`, so one sign-in serves every sandbox of that agent and survives `msb rm`.
- pi sandboxes get `/root/.pi/agent/models.json` with the host's local model servers: loopback providers from the host's pi `models.json`, else Ollama and LM Studio found by probing.
- The `OPENAI_API_KEY is not set` note now suggests `msbx secret set`.
- Network policy (RFC 0008): new sandboxes send public TCP traffic through the msbx proxy on `127.0.0.1:47447`, which applies `~/.config/msbx/policy.toml` live: presets `open`, `balanced` and `locked-down`, sbx-style targets (`example.com`, `*.example.com`, `**.example.com`, `**`, `:port`, IPs, CIDRs), global and per-sandbox allow and deny rules, deny over allow.
- `msbx policy init|ls|allow|deny|rm|check|log|preset` and `msbx proxy serve|status|stop`. The first `msbx agent` asks for a preset.
- Dashboard: a Network tab per sandbox with the proxy's log (`a` allow here, `A` allow everywhere, `b` block, `p` preset), `policy <preset> (proxied)` and a traffic row on the Overview.
- `msbx start NAME` and other pass-through commands put the proxy token in `msb`'s environment and start the proxy when needed.

### Changed

- Dashboard navigation: `Tab` and horizontal arrows switch panes, vertical arrows move within the focused pane, and `[` / `]` change detail tabs. Network blocking uses `b`.

- Creating a sandbox from the dashboard no longer attaches. The dashboard stays open, shows progress on the status line and selects the new card; press `Enter` to attach.
- The Network tab tells sandboxes msbx did not create apart from ones created before network policy.
- Sandbox network defaults: deny by default, DNS and the Ollama and LM Studio ports to the host, public TCP through the proxy. Sandboxes created earlier keep `public + host` and ignore the policy.
- `msbx run` is renamed `msbx agent`; `msbx run` now passes through to `msb run`.

### Fixed

- `msbx proxy stop` no longer drops connections logged just before it stopped.

[unreleased]: https://github.com/finngi/msbx/commits/main
