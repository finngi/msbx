# RFC 0011: Host ports, sandbox info and settings

- Status: Implemented
- Tracking issue: none (local dashboard brief)

## Summary

Make the Mac ports a new sandbox may reach (`host.microsandbox.internal`) a setting in
`policy.toml`; until now they were fixed to the Ollama and LM Studio ports. Show what a sandbox was created
with in an `i` Info popup on the dashboard, and edit the default preset and the host ports in a
`Ctrl-S` Settings screen, as Docker Sandboxes (`sbx`) does.

## Motivation

[RFC 0008](0008-network.md) fixed host access at TCP 11434 and 1234. A model server, dev
database or MCP server on another Mac port is unreachable from every msbx sandbox, and the only
remedy was the whole `host` profile from `msb`, which skips the msbx proxy. The ports cannot be
decided live by the proxy: microsandbox sends host-bound traffic straight to the host and never
dials the proxy for it (`microsandbox-network-0.7.11/lib/proxy/types.rs:206`
`select_for_destination`), and the SDK's `modify()` builder has no network option. So the ports
must be chosen before create, and the user must be able to see which ports an existing sandbox
has. [RFC 0010](0010-dashboard.md) left out settings and an Info popup; this RFC adds them.

## Guide-level explanation

```console
$ msbx policy host-ports
host ports 1234, 11434
$ msbx policy host-ports 11434 1234 8080
host ports are now 1234, 8080, 11434
this applies to sandboxes created from now on; existing sandboxes keep the ports they were created with
these sandboxes have other host ports; recreate one to apply:
  codex-api (1234, 11434): msb rm -f codex-api && msbx agent codex /Users/me/api
$ msbx policy host-ports --none
host ports are now none (DNS only)
```

Ports are separated by spaces or commas, each 1-65535, at most 32. `--none` leaves only DNS.
The file keeps them as `host_ports = [8080]`; a file without the key means 11434 and 1234, so
existing files keep today's behaviour.

In the dashboard, `i` on a card opens a scrollable Info popup with the sandbox's identity,
resources (including root disk use), security (guest user, profile, git guard), network (proxy,
preset, host ports set at create next to the ones configured now, sandbox-scoped rule count),
mounts and secret names. When the ports differ, the row is yellow and says `recreate to apply`.
`Ctrl-S` opens Settings: the default preset (the same choice as `p`) and the host ports as text.
Enter saves both, Esc cancels. An invalid port list stays in the form with the parser's error.
Without `policy.toml`, saving creates it, like the first-run preset question.

## Reference-level explanation

- `policy.rs`: `Policy::host_ports: Vec<u16>` with serde default `[1234, 11434]`, omitted from
  the file while it is the default, and checked on load by `normalize_host_ports` (no 0,
  sorted, deduplicated, at most 32). `parse_host_ports(&str)` splits on commas and whitespace
  and is the one parser the CLI and the dashboard use. `deny_unknown_fields` stays.
- `sandbox.rs`: `ensure` and `ensure_named` take the ports; `proxied_policy(&ports)` adds
  `tcp().ports(..).allow_host()` only when the list is not empty, because a rule with no ports
  matches every port (`microsandbox-types-0.7.11/lib/domain.rs:3183`). The DNS rule stays.
  `SandboxInfo` gains `host_ports`, read back from the stored static policy (host-group allow
  rules with ports, without the UDP and TCP 53 rule), `None` when the sandbox is not proxied
  or its config is unreadable; and `created` (`SandboxHandle::created_at`), root disk use
  (`SandboxMetrics::upper_used_bytes` / `upper_free_bytes`), guest `user` and whether the
  profile is `restricted`. A unit test serialises the built policy into the stored type, as the
  SDK does, and reads the ports back.
- `main.rs`, `policy_cli.rs`: `msbx policy host-ports [PORT…] [--none]`. Setting it does not
  reload the proxy, which never sees host traffic. It lists sandboxes (best effort) to name the
  msbx sandboxes whose ports differ, with the recreate commands.
- `app.rs`, `keymap.rs`, `ui.rs`: `Popup::Info { name }` (kept by name across refreshes) and
  `Popup::Settings(SettingsForm)`; `Action::SaveSettings`. `i` is bound while the split view is
  shown, `Ctrl-S` everywhere outside forms; both come from the one binding table, so help and
  the footer list them. Info and help share the measured popup scroll.
- `tui.rs`, `tui_data.rs`: the refresh carries the configured ports; `SaveSettings` becomes a
  policy edit that loads or creates `policy.toml`, sets preset and ports, saves atomically and
  reloads the proxy, as `p` does. Dashboard creation reads the ports from `policy.toml`. The
  debug fixture mode refuses the save like every other policy write.
- Tests: parser and file validation, CLI parse and file round trip, drift text, the static
  policy for some and no ports, app transitions (open, scroll, close, validation, save, no
  policy), settings edits through the file, and snapshots of Info (populated, foreign,
  scrolled) and Settings (default, error, no policy) at 100x30, 80x24 and 60x20.

## Drawbacks

One more setting that cannot apply to existing sandboxes, so users must recreate to pick it up;
the Info popup and the CLI say which ones. An msbx proxy from an older version keeps its last
good policy if it cannot parse a file with `host_ports` (it already warns about the version
mismatch); the default is not written, so this only affects users who change the ports.

## Security

Each port added exposes that Mac service to every sandbox created afterwards, without the proxy's host-name checks or log. The setting lives
in `~/.config/msbx/policy.toml`, outside the project, so an agent cannot widen its own access,
and it never changes an existing sandbox. `--none` closes the Mac except DNS. The Info popup
shows secret names only, never values.

## Rationale and alternatives

- Deciding host ports in the proxy: impossible, the traffic never reaches it.
- Opening the whole host group: it would expose every Mac service, which RFC 0008 avoided.
- A per-sandbox port option on `msbx agent`: the setting describes services on the Mac, and a
  per-project setting would belong in [RFC 0002](0002-config.md)'s config file.
- Adding the fields to Overview, without an Info popup ([RFC 0010](0010-dashboard.md)'s
  choice): Overview fits a narrow pane; create-time network settings, disk and security make it
  too long, and sbx users expect `i`.

## Prior art

Docker Sandboxes (`sbx`) has an `i` info view and a settings screen for the default network
policy. The `sbx` traversal supplied with the RFC 0010 brief was the reference; its behaviour
was not independently verified here.

## Unresolved questions

None for this scope. A real VM created with custom ports was not booted while implementing.

## Future possibilities

- Per-project host ports once the config file of RFC 0002 exists.
- Recreating a sandbox from the Info popup, keeping its name and project.
