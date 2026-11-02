# msbx: microsandbox extended

A terminal dashboard and one-command agent launcher for [microsandbox](https://github.com/superradcompany/microsandbox) microVMs. Every other command goes straight to `msb`.

<!-- Screenshot: docs/screenshot.png (to be added) -->

> **Status:** early, pre-1.0. Expect breaking changes. microsandbox itself is beta software.

The documentation is at <https://finngi.github.io/msbx/>.

msbx is an independent project and is not affiliated with or endorsed by superradcompany or the microsandbox project.

## Why

Docker Sandboxes (`sbx`) runs coding agents in microVMs, but it is a proprietary, binary-only product. microsandbox is an open-source (Apache-2.0) microVM runtime whose `msb` CLI and Rust SDK already cover mounts, resource limits, networking, host-side secrets and sandbox lifecycle.

msbx is a thin open layer on top. It adds only what `msb` lacks: a TUI and a single command that creates and attaches to a per-project agent sandbox. Anything else you type is passed to `msb`, so `msbx` can stand in for `msb` everywhere.

## Features

- `msbx agent` starts a coding agent or a shell in a sandbox for the current project: the same agents as Docker Sandboxes (Claude Code, Codex, Copilot, Cursor, Devin, Docker Agent, Droid, Gemini, Kiro, OpenCode) plus pi. Each is installed inside the VM from its vendor's own npm package or installer, and starts with its permission prompts off, as in Docker Sandboxes. The sandbox is created the first time and reused after that.
- `msbx` opens a terminal dashboard of all microsandbox sandboxes, including ones msbx did not create, with live CPU and memory, Overview, Logs, Network and Rules tabs.
- Network policy like Docker Sandboxes: `open`, `balanced` or `locked-down`, plus allow and deny rules per host (`example.com`, `*.example.com`, `**.example.com`, ports, CIDRs), global or per sandbox. Every sandbox's traffic goes through a small msbx proxy, so rule changes apply at once and a log shows what was allowed and blocked.
- `msbx ls`, `msbx logs NAME`, `msbx run IMAGE` and any other command run `msb` with the same arguments.
- `msbx secret set ANTHROPIC_API_KEY` keeps a key in the macOS Keychain, or on Linux in the Secret Service, so you need not export it in every shell.
- Sign in to an agent once: its sign-in files live on a per-agent volume shared by all sandboxes of that agent.
- pi sees the local model servers on your Mac (Ollama, LM Studio) with no setup.
- An optional `~/.config/msbx/config.toml` sets the image, CPUs and memory, and per agent the image, extra arguments or keeping its permission prompts.
- Sandbox defaults:
  - the project folder is mounted at the same path as on the host
  - all host CPUs, and half the host's memory (at least 512 MiB, at most 32 GiB)
  - outbound traffic through the msbx proxy under your policy, plus chosen ports on your Mac (`host.microsandbox.internal`; by default 11434 for Ollama and 1234 for LM Studio)
  - agent API keys stay on the host: the VM sees only a placeholder, and microsandbox's secret proxy substitutes the real key on the way out
  - `node:24-bookworm` image (Node.js 24, git, curl, Python 3, gcc, make), with the agent installed once at creation; another Debian or Ubuntu image works too

## Requirements

- macOS on Apple Silicon, or Linux with KVM enabled (microsandbox's requirements)
- [microsandbox](https://github.com/superradcompany/microsandbox) 0.7.13, from its installer
  (Homebrew still ships 0.7.11):
  ```sh
  curl -fsSL https://install.microsandbox.dev | sh
  ```
- Rust stable (1.88 or newer) to build

Windows is supported by microsandbox but untested with msbx.

## Install

```sh
cargo install --git https://github.com/finngi/msbx
```

## Usage

### Run an agent

```sh
msbx agent <AGENT> [PATH] [-- ARGS]
```

- `PATH` defaults to the current directory.
- The sandbox is named `<agent>-<folder>`, for example `codex-myapp`.
- If the sandbox exists it is started if needed and reused; otherwise it is created.
- Arguments after `--` go to the agent, as in `sbx run`. When the first starts with `-` they follow the agent's defaults (such as the skip-prompts flag); when it is a word such as a subcommand, the defaults are dropped.

```sh
msbx agent claude                          # Claude Code in the current project
msbx agent pi ~/dev/myapp                  # pi in another project
msbx agent shell                           # bash in the project sandbox
msbx agent claude -- -c                    # claude --dangerously-skip-permissions -c
msbx agent codex -- login --device-auth    # codex login --device-auth
```

| Agent | Runs | Skip-prompts flag | Host-side keys (any one) | Shared sign-in |
|---|---|---|---|---|
| `claude` | `claude` | `--dangerously-skip-permissions` | `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` | `~/.claude/.credentials.json`, `~/.claude.json` |
| `codex` | `codex` | `--dangerously-bypass-approvals-and-sandbox` | `OPENAI_API_KEY` | `~/.codex/auth.json` |
| `copilot` | `copilot` | `--yolo` | `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | `~/.copilot/config.json` |
| `cursor` | `cursor-agent` | `--yolo` | `CURSOR_API_KEY` | `~/.config/cursor/auth.json` |
| `devin` | `devin` | `--permission-mode dangerous` | none | `~/.local/share/devin/credentials.toml` |
| `docker-agent` | `docker-agent run` | `--yolo` | `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GOOGLE_API_KEY` | none |
| `droid` | `droid` | `--auto high` | `FACTORY_API_KEY` | `~/.factory/auth.v2.file`, `auth.v2.key` |
| `gemini` | `gemini` | `--approval-mode=yolo` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | `~/.gemini/oauth_creds.json` |
| `kiro` | `kiro-cli chat` | `--trust-all-tools` | none | none (sign in per sandbox) |
| `opencode` | `opencode` | `--auto` | `OPENCODE_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | `~/.local/share/opencode/auth.json` |
| `pi` | `pi` | none | none (uses local models on the host) | `~/.pi/agent/auth.json` |
| `shell` | `bash` | none | none | none |

Keys are sent only to their vendor's API hosts. Versions, install sources and hosts are on the [Agents](https://finngi.github.io/msbx/reference/agents.html) page. To keep an agent's own permission prompts, or add arguments to every launch:

```toml
# ~/.config/msbx/config.toml
[agents.claude]
auto_approve = false
args = ["--model", "opus"]
```

Sandbox settings are applied only when the sandbox is created. To change them, remove it with `msb rm -f <name>` and run `msbx agent` again.

### Keep an API key in the key store

```sh
msbx secret set OPENAI_API_KEY   # security (or secret-tool) prompts for the value
msbx secret rm OPENAI_API_KEY
```

msbx reads each agent secret that is not set in your shell from the macOS Keychain, or on Linux from the freedesktop Secret Service through `secret-tool` (package `libsecret-tools`), before it touches a sandbox. A variable exported in the shell wins. The value goes only into msbx's own environment, where microsandbox's secret proxy reads it; it is never printed or passed on a command line.

### Sign in once

Agent sign-ins (Codex "Sign in with ChatGPT", Claude Code `/login`, pi `/login` and the others in the table) are kept on a named volume per agent, such as `msbx-login-codex`, mounted at `/home/node/.msbx-login` in every sandbox of that agent. The sign-in file in the VM is a link to that volume, so a login or token refresh in one sandbox is seen by all of them, and it survives `msb rm`.

- Codex: `msbx agent codex -- login --device-auth`, then `msbx agent codex` in any project.
- pi: run `msbx agent pi`, then `/login`.
- Sign out everywhere: stop the agent's sandboxes, then `msbx volume rm msbx-login-codex`.

Sandboxes created before this feature have no login volume; msbx prints a note. Recreate them with `msb rm -f <name>`.

### pi and local models

Each time `msbx agent pi` starts, msbx writes `~/.pi/agent/models.json` in the VM from your `~/.pi/agent/models.json` (or `$PI_CODING_AGENT_DIR/models.json`): only providers whose `baseUrl` is on `127.0.0.1`, `localhost`, `[::1]` or `0.0.0.0` are copied, with the host rewritten to `host.microsandbox.internal`. Providers with real cloud keys stay on the host. With no such file, msbx probes Ollama (`127.0.0.1:11434`) and LM Studio (`127.0.0.1:1234`) and lists the models they serve.

### Control network access

The first `msbx agent` asks for a preset; `msbx policy init <open|balanced|locked-down>` does the same in a script. `balanced` blocks by default and allows the agents' API hosts, installers and sign-ins, npm, Node.js, Debian, PyPI, crates.io, Go and GitHub.

```sh
msbx policy allow pypi.org,files.pythonhosted.org      # everywhere
msbx policy deny "**.example.com" --sandbox codex-api  # one sandbox; deny beats allow
msbx policy check pypi.org:443                         # Allowed or Blocked, exit 1 when blocked
msbx policy log codex-api --blocked                    # what the proxy refused
msbx policy ls                                         # effective rules and their source
msbx policy host-ports 11434 1234 8080                 # Mac ports new sandboxes may reach
msbx proxy status                                      # the proxy on 127.0.0.1:47447
```

Rules live in `~/.config/msbx/policy.toml` and apply to the next connection; nothing restarts.
Host ports are different: traffic to your Mac never passes the proxy, so they are fixed when a
sandbox is created. `msbx policy host-ports` (or `Ctrl-S` in the dashboard) changes them for new
sandboxes and names the existing ones to recreate. The proxy starts itself when msbx needs it and keeps running after msbx exits, like the sandboxes. Use `msbx start NAME` (not `msb start`) for proxied sandboxes: msbx hands `msb` the proxy token. See [Control network access](https://finngi.github.io/msbx/how-to/control-network-access.html).

### Dashboard

```sh
msbx
```

The title marker shows which pane has focus. On narrow terminals, that pane fills the screen;
`Tab` switches to the other pane.

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab`, `←` / `→` (`h` / `l`) | Switch between Cards and Detail |
| `↑` / `↓` (`k` / `j`) | Move within the focused pane |
| `[` / `]` | Change detail tab: Overview / Logs / Network / Rules |
| `Enter` | Cards: attach or create; Network: expand the selected row |
| `c` | Create wizard: choose agent, name and workspace |
| `x`, `s`, `r` | Cards: shell, start/stop, remove (confirm) |
| `n` | Full network view; `/` searches, `o` filters status, `s` filters sandbox |
| `a`, `A`, `b` | Network: allow here, allow everywhere, block here |
| `a`, `r` | Rules: add a rule, remove an explicit rule (confirm) |
| `p` | Choose the network preset |
| `i` | Info for the selected sandbox: identity, resources, security, network, mounts |
| `Ctrl-S` | Settings: default preset and host ports for new sandboxes |
| `?` / `Ctrl-H` | Help for the current view |
| `q` / `Ctrl-C` | Quit |

The create wizard shows credential availability, lets you edit the default `<agent>-<folder>`
name and browse workspace folders. Creation runs in the background; press `Enter` on the new
card when it is ready to attach. Attaching gives the agent the terminal; the dashboard returns
when you exit.

Network rows show time, host, port, hits and status. Expand one for its first/last seen times
and allowed/blocked counts. The Rules view shows preset, global and sandbox rules, and edits
apply to the next connection. Help and footer hints come from the same keymap. Set `NO_COLOR=1`
for a monochrome display. See [Dashboard keys](https://finngi.github.io/msbx/reference/dashboard-keys.html)
for forms, filters and Escape behavior.

### Everything else is msb

Any command msbx does not define is passed to `msb` with its arguments unchanged. msbx replaces itself with `msb`, so the terminal, signals and exit code are exactly `msb`'s.

```sh
msbx ls                          # msb ls
msbx logs codex-myapp -f         # msb logs codex-myapp -f
msbx run alpine -- sh            # msb run alpine -- sh
msbx exec -t shell-myapp -- bash # msb exec -t shell-myapp -- bash
```

`msbx --help` and `msbx --version` are msbx's own; use `msbx <command> --help` for msb's help on a command.

## Security model

msbx adds no isolation of its own. Isolation is provided by microsandbox; read its [security documentation](https://github.com/superradcompany/microsandbox/tree/main/docs/security) and [security policy](https://github.com/superradcompany/microsandbox/blob/main/SECURITY.md).

What microsandbox isolates:

- Each sandbox is a separate microVM with its own kernel.
- Only the project folder is mounted from the host filesystem.
- API keys stay on the host. The VM sees a placeholder, which microsandbox replaces with the real key only on requests to the allowed host.

Agents start with their permission prompts off (`claude --dangerously-skip-permissions` and the like), as in Docker Sandboxes: the VM, the network policy, the git guard and host-side keys are the boundary. Set `auto_approve = false` for an agent in `config.toml` to keep its prompts too.

What msbx adds for git ([RFC 0001](https://finngi.github.io/msbx/rfcs/0001-git-safety.html)): the agent runs as the unprivileged uid 1000 (`node`) under microsandbox's `restricted` profile, `.git/hooks`, `.git/info` and `.git/config` are read-only mounts the agent cannot unmount (even as root), msbx only runs git on your Mac with hooks and fsmonitor off, and it warns when the files git executes from changed during or between sessions. Sandboxes created before this keep running as root; recreate them. See [Keep git safe](https://finngi.github.io/msbx/how-to/guard-git.html).

What it leaves open:

- The project folder is writable from the VM. An agent can modify or delete anything in it, including the history under `.git`. Commit or back up first.
- Network access follows your policy. The proxy decides by the host name in the TLS handshake or HTTP `Host` header, checked against DNS on your machine; traffic with no name is decided by IP. DNS itself is unrestricted, and an allowed host can still receive project contents.
- Host services at `host.microsandbox.internal`: only the host ports in `policy.toml` (by default 11434 for Ollama and 1234 for LM Studio), fixed when the sandbox is created. Any port you add exposes that Mac service to every sandbox created afterwards.
- TLS is intercepted when secrets are used. To substitute keys, microsandbox adds a local CA to the VM's trust store and terminates TLS on the host. Agents that pin certificates may fail.
- Secret values live on the host, in memory and possibly in microsandbox's host-side sandbox configuration (readable only by your user account).

Report msbx vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Relationship to microsandbox

msbx uses an unmodified microsandbox: the Rust SDK (pinned to 0.7.13) and the `msb` CLI (for attaching and pass-through).

msbx keeps no list of `msb` commands. It hands anything it does not define to whichever `msb` is on your `PATH`, so new `msb` commands work through msbx as soon as microsandbox ships them.

## Limitations and roadmap

- Twelve built-in agents, no custom agents. `config.toml` covers the image, resources and agent arguments only; the rest of [RFC 0002](https://finngi.github.io/msbx/rfcs/0002-config.html) (per-project settings, login scope, setup scripts) is not built yet.
- Only pi is set up for local models; other agents need their base URL set by hand.
- Sandbox settings other than the network policy are fixed at creation.
- Host access cannot change while a sandbox runs; host traffic never passes the proxy.
- Later: propose a `msb tui` command or external-subcommand support upstream, so msbx can become part of `msb`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## Licence

Apache-2.0. See [LICENSE](LICENSE).
