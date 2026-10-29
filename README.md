# msbx — microsandbox extended

A terminal dashboard and one-command agent launcher for [microsandbox](https://github.com/superradcompany/microsandbox) microVMs. Every other command goes straight to `msb`.

<!-- Screenshot: docs/screenshot.png (to be added) -->

> **Status:** early, pre-1.0. Expect breaking changes. microsandbox itself is beta software.

**Documentation:** <https://finngi.github.io/msbx/>

msbx is an independent project. It is not affiliated with or endorsed by superradcompany or the microsandbox project.

## Why

Docker Sandboxes (`sbx`) runs coding agents in microVMs, but it is a proprietary, binary-only product. microsandbox is an open-source (Apache-2.0) microVM runtime whose `msb` CLI and Rust SDK already cover mounts, resource limits, networking, host-side secrets and sandbox lifecycle.

msbx is a thin open layer on top. It adds only what `msb` lacks: a TUI and a single command that creates and attaches to a per-project agent sandbox. Anything else you type is passed to `msb`, so `msbx` can stand in for `msb` everywhere.

## Features

- **`msbx agent`**: one command to start a coding agent (Codex, pi) or a shell in a sandbox for the current project. The sandbox is created the first time and reused after that.
- **`msbx`**: a terminal dashboard of all microsandbox sandboxes, including ones msbx did not create, with live CPU and memory, an Overview tab and a Logs tab.
- **Everything else is `msb`**: `msbx ls`, `msbx logs NAME`, `msbx run IMAGE` and any other command run `msb` with the same arguments.
- **Sensible sandbox defaults**:
  - the project folder is mounted at the same path as on the host
  - all host CPUs, with a 32 GiB memory limit
  - public internet plus host services (for example Ollama at `host.microsandbox.internal:11434`)
  - agent API keys stay on the host: the VM sees only a placeholder, and microsandbox's secret proxy substitutes the real key on the way out
  - `node:24-bookworm-slim` image, with the agent installed once at creation

## Requirements

- macOS on Apple Silicon, or Linux with KVM enabled (microsandbox's requirements)
- [microsandbox](https://github.com/superradcompany/microsandbox) 0.7.11:
  ```sh
  brew install superradcompany/tap/microsandbox
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
msbx agent <codex|pi|shell> [PATH] [-- ARGS]
```

- `PATH` defaults to the current directory.
- The sandbox is named `<agent>-<folder>`, for example `codex-myapp`.
- If the sandbox exists it is started if needed and reused; otherwise it is created.
- Arguments after `--` are passed to the agent.

```sh
msbx agent codex                # Codex in the current project
msbx agent pi ~/dev/myapp       # pi in another project
msbx agent shell                # bash in the project sandbox
msbx agent codex -- --version   # pass arguments through to the agent
```

| Agent | Command in the VM | Host-side secret |
|---|---|---|
| `codex` | `codex` | `OPENAI_API_KEY`, sent only to `api.openai.com` |
| `pi` | `pi` | none (uses local models on the host) |
| `shell` | `bash` | none |

Sandbox settings are applied only when the sandbox is created. To change them, remove it with `msb rm -f <name>` and run `msbx agent` again.

### Dashboard

```sh
msbx
```

| Key | Action |
|---|---|
| `↑` / `↓` | Select sandbox |
| `Tab` | Switch tab (Overview / Logs) |
| `Enter` | Attach the sandbox's agent |
| `x` | Open a shell in the sandbox |
| `c` | Create a sandbox |
| `s` | Start / stop |
| `r` | Remove (asks to confirm) |
| `?` | Help |
| `q` | Quit |

Attaching hands the terminal to `msb exec -t`; the dashboard returns when you exit.

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

**Isolated:**

- Each sandbox is a separate microVM with its own kernel.
- Only the project folder is mounted. The rest of the host filesystem is not visible.
- API keys are not placed in the VM. The VM sees a placeholder, which microsandbox replaces with the real key only on requests to the allowed host.

**Not isolated:**

- **The project folder is writable from the VM.** An agent can modify or delete anything in it, including `.git`. Commit or back up first.
- **Network access is open.** The VM can reach the public internet, so it can send project contents anywhere.
- **Host services are reachable** at `host.microsandbox.internal`, not only Ollama. Anything listening on the host may be reachable from the VM.
- **TLS is intercepted when secrets are used.** To substitute keys, microsandbox adds a local CA to the VM's trust store and terminates TLS on the host. Agents that pin certificates may fail.
- **Secret values live on the host**, in memory and possibly in microsandbox's host-side sandbox configuration (readable only by your user account).

Report msbx vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Relationship to microsandbox

msbx uses the microsandbox Rust SDK (pinned to 0.7.11) and the `msb` CLI (for attaching and pass-through). It does not fork or patch microsandbox.

msbx keeps no list of `msb` commands: anything it does not define is handed to whichever `msb` is on your `PATH`, so new `msb` commands work through msbx as soon as microsandbox ships them.

## Limitations and roadmap

- Three built-in agents (`codex`, `pi`, `shell`); no config file or custom images.
- Sandbox settings are fixed at creation.
- No network-log view: the microsandbox SDK has no API for it yet.
- Later: propose a `msb tui` command or external-subcommand support upstream, so msbx can become part of `msb`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). Changes are recorded in [CHANGELOG.md](CHANGELOG.md).

## Licence

Apache-2.0. See [LICENSE](LICENSE).
