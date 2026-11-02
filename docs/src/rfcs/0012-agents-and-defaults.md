# RFC 0012: Agents, natural defaults and a config subset

- Status: Implemented
- Tracking issue: none (local brief)

## Summary

msbx runs the same agents as Docker Sandboxes (`sbx`), with sbx's names, plus pi: `claude`,
`codex`, `copilot`, `cursor`, `devin`, `docker-agent`, `droid`, `gemini`, `kiro`, `opencode`,
`pi` and `shell`. Each is installed in the VM at create from its vendor's package or installer
and launched with its skip-permission-prompts flag, as sbx does. New sandboxes boot
`node:24-bookworm` with half the host's memory, and an optional `~/.config/msbx/config.toml`
sets the image, CPUs, memory and, per agent, the image, the auto-approve flag and extra
arguments.

## Motivation

msbx is becoming a public project. With three agents (Codex, pi, a shell), a slim image that
lacked Python and a compiler, and a 32 GiB memory ceiling chosen for one developer's Mac, it
did not suit most users. sbx users expect their agent by the name they know, keys sent only to
the agent's API, sign-in that survives a new sandbox, and no permission prompt for every
command: the VM is the boundary. Users with other needs need a way to pick another image or
keep the prompts without patching msbx.

## Guide-level explanation

```console
$ msbx agent claude                 # claude --dangerously-skip-permissions
$ msbx agent claude -- -c           # claude --dangerously-skip-permissions -c
$ msbx agent codex -- login --device-auth    # a word first replaces the defaults
$ msbx secret set GH_TOKEN          # Keychain on macOS, Secret Service on Linux
```

```toml
# ~/.config/msbx/config.toml (optional)
[defaults]
memory_mib = 8192

[agents.claude]
auto_approve = false
args = ["--model", "opus"]
```

The [Agents](../reference/agents.md) page lists each agent's install, version, flag, keys,
hosts and sign-in files; [Sandbox defaults](../reference/sandbox-defaults.md) lists the
defaults and the config file.

## Reference-level explanation

- `agents.rs`: `Agent` gains `install: Install` (`Nothing`, `Npm(pkg@version)`, `Root(script)`,
  `Script { url, bins }`), `packages`, `default_args`, `auto_approve`, `secrets: &[Secret]`
  with `Secret { env_var, hosts: &[..] }`, and `logins: &[Login]` (the first is the credential;
  `dir` may be empty for a file in the home). `AgentId` has the twelve sbx names
  (`docker-agent` via clap and serde kebab-case). `launch_args` applies sbx's rule: arguments
  that start with `-` follow the defaults, a word replaces them.
- Install (`sandbox.rs`): as root, `INSTALL_PRELUDE` installs missing `ca-certificates curl git
  bash` (plus per-agent packages) with `apt-get`, creates `node` as uid 1000 when no account
  has that uid, gives that account its home, and installs Node.js 24.22.2 from nodejs.org
  (checked against `SHASUMS256.txt`) when an npm agent's image has no `npm`. Then root runs
  `npm install -g` or the agent's root script (docker-agent: the v1.154.0 release binary,
  checked against its SHA-256). Vendor scripts (cursor, devin, kiro) run as the agent user,
  with stdin closed; success means the expected commands exist in `~/.local/bin`, because
  Devin's installer ends by asking to sign in. Root then links them into `/usr/local/bin`,
  which is on `msb exec`'s `PATH`.
- The agent user is now set by uid, `1000`: microsandbox 0.7.11 refuses to boot a
  sandbox whose user is not in the image's `/etc/passwd` ("guest user not found: node",
  checked with `debian:bookworm-slim`), and the account is only created by the install. The
  home is read from the guest (`$HOME` as that user), so
  Ubuntu's uid 1000 `ubuntu` works. The login volume is mounted at the fixed
  `/home/node/.msbx-login`; `prepare` reads the mount point back from the stored mounts, so
  older sandboxes keep `/root/.msbx-login` or `/home/node/.msbx-login`.
- Secrets: one SDK secret per variable with every host (`SecretBuilder::allow` pushes,
  microsandbox-network 0.7.11 `model/config/builder.rs`). A variable shared by agents is one
  constant, so it always has the same hosts (tested). The not-set note names all of an agent's
  variables once, only when none is set.
- `secrets.rs`: a `Store` enum, the Keychain through `security` on macOS and, on other Unixes
  with `secret-tool` on `PATH`, the Secret Service with attributes `service msbx account VAR`.
  Without either, `msbx secret` errors and lookups are skipped.
- `config.rs`: `Config { defaults: Defaults { image, cpus, memory_mib }, agents:
  BTreeMap<AgentId, AgentConfig { image, auto_approve, args }> }`, `deny_unknown_fields`,
  values checked in deserializers so every error carries TOML's line and column. Missing file:
  built-in defaults. `create_options` resolves built-in < `[defaults]` < `[agents.<id>]` into
  `model::CreateOptions { image, cpus, memory_mib, host_ports }`, which `sandbox::ensure` takes.
  `launch_args` is applied at every attach (CLI and dashboard).
- Memory: half of `sysctl -n hw.memsize` (macOS) or `/proc/meminfo` `MemTotal` (Linux),
  clamped to 512 MiB..32 GiB (sbx's rule); 4 GiB when neither can be read. No new dependency.
- `policy.rs`: the `balanced` baseline adds the install and sign-in hosts (listed with sources
  in the code and on the Agents page), and lists each host once even when it is also a secret
  host.
- Dashboard: the create wizard lists one row per agent, scrolled to the selection, with the
  position in the title and the selected agent's credential summary in a fixed area below; the
  summary names the set variables, or all of them as missing.
- Tests: launch-argument rule, pinned npm versions, consistent secret hosts, unique login file
  names, config precedence and errors, memory clamping, `meminfo` parsing, install script
  shapes and `sh -n` parsing, login scripts for a home-level file and a stored mount, Secret
  Service arguments, wizard rendering at 80x24, 60x20 and a scrolled 60x14.

### Verified

Every agent was installed in a `node:24-bookworm` microVM with plain `msb` (user uid 1000,
`restricted` profile), running the install scripts generated from this source, and printed
its version, and accepted its default arguments (`--version` after them, or `--help` for
`kiro-cli chat` and `docker-agent run`): claude 2.1.314, codex 0.166.0, copilot
1.0.105, cursor 2026.10.22-27dcf5f, devin 3000.11.234, docker-agent v1.154.0, droid 1.0.0,
gemini 0.66.0, kiro 2.37.1, opencode 1.19.5, pi 1.4.0; shell had bash 5.2, git 2.39, Python
3.11 and gcc 12. pi on `debian:bookworm-slim` (Node.js installed from nodejs.org, `node` user
created) and claude and cursor on `ubuntu:24.04` (agent runs as `ubuntu`) also installed and
ran. Sign-in flows were not exercised (no vendor accounts).

## Drawbacks

- Twelve agents means twelve sets of install, flag and file facts that drift. npm packages and
  docker-agent are pinned; cursor, devin and kiro's installers fetch the vendor's latest.
- Starting agents without prompts trusts the sandbox boundary; a user who expected prompts may
  be surprised. The docs say so and `auto_approve = false` restores them.
- Sharing `~/.claude.json` shares Claude Code's settings and project history between Claude
  sandboxes as well as the account.
- `/home/node` appears on images whose uid 1000 is another user (an empty mount parent).

## Security

- The VM is now the main boundary against a misbehaving agent; nothing else about it changed.
- More keys can be forwarded; each goes only to its vendor's API hosts. `GH_TOKEN` and
  `GITHUB_TOKEN` are forwarded to Copilot sandboxes only, for `api.github.com` and Copilot's
  API, which also lets `gh` in that VM use them.
- Vendor installers run as the agent user. Root only installs npm packages, Node.js
  and docker-agent, each checked against a version or digest.
- The `balanced` preset allows more hosts, all of them install or sign-in endpoints.
- On images where uid 1000 is in `sudo` (Ubuntu), `no_new_privs` under the `restricted` profile
  still stops escalation.

## Rationale and alternatives

- npm over native installers where both exist: an exact version, one registry already allowed,
  and the same path for every image with Node. Claude's native installer would auto-update into
  the user's home; the pinned npm package does not.
- Not linking Kiro's SQLite database: SQLite writes journal files beside the database path, so
  a link to a shared file is not safe; Kiro users sign in per sandbox.
- A numeric uid, so no custom image with the user is needed: msbx stays image-agnostic and
  needs no image build.
- Reading the config per launch for arguments but only at create for resources matches what
  can change: the SDK applies resources only at create.

## Prior art

- Docker Sandboxes: agent list, names, launch flags and argument rule
  (<https://docs.docker.com/ai/sandboxes/agents/>), secret host lists
  (<https://docs.docker.com/ai/sandboxes/configuration/credentials/>), memory rule
  (<https://docs.docker.com/reference/cli/sbx/run/>).
- Vendor docs for each agent, linked from the [Agents](../reference/agents.md) page.

## Unresolved questions

- Kiro's sign-in hosts and Devin's model hosts are not documented; `msbx policy log` shows them.
- Droid's shared sign-in comes from binary strings and has not been tried with an account.

## Future possibilities

- Local-model setup for Claude Code, Codex, OpenCode and Copilot, like pi's.
- The rest of RFC 0002 (per-project tables, login scope).
- Host-side OAuth like sbx's `--oauth`, so tokens never enter the VM.
