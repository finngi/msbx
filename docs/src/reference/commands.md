# Commands

msbx defines two commands of its own. Everything else is passed to `msb`.

## `msbx`

```text
msbx
```

Opens the [dashboard](dashboard-keys.md) of all microsandbox sandboxes. The current directory is
the default project for sandboxes created from the dashboard.

`msbx --help` (`-h`) and `msbx --version` (`-V`) are msbx's own. `msbx help [COMMAND]` prints
msbx's help for its own commands.

## `msbx agent`

```text
msbx agent <AGENT> [PATH] [-- <ARGS>...]
```

Starts (creating if needed) the agent's sandbox for a folder and attaches to the agent.

| Argument | Meaning |
|---|---|
| `AGENT` | `codex`, `pi` or `shell` |
| `PATH` | Project folder to mount at the same path inside the sandbox. Default: the current directory. |
| `ARGS` | Arguments passed to the agent, after `--`. |

Behaviour:

1. The sandbox is named `<agent>-<folder>`: the folder name is lowercased, every run of
   characters outside `a-z0-9` becomes one `-`, and the name is cut to 128 bytes.
2. If a sandbox with that name exists for the same folder, it is started if needed and reused.
   If it exists for a different folder, or was not created by msbx, msbx stops with an error.
3. Otherwise it is created with the [sandbox defaults](sandbox-defaults.md).
4. On first use the agent is installed in the VM (a marker file, `/root/.msbx-installed`, records
   that it is done).
5. msbx runs `msb exec -t <name> -- <command> <ARGS>` on your terminal and exits with its exit
   code.

| Agent | Command in the VM | Installed on first use | Host-side secret |
|---|---|---|---|
| `codex` | `codex` | `@openai/codex@0.148.0`, `git`, `ca-certificates` | `OPENAI_API_KEY`, sent only to `api.openai.com` |
| `pi` | `pi` | `@earendil-works/pi-coding-agent@1.4.0`, `git`, `ca-certificates` | none |
| `shell` | `bash` | `git`, `curl`, `ca-certificates` | none |

## Pass-through to `msb`

```text
msbx <COMMAND> [ARGS]...
```

Any command msbx does not define runs `msb <COMMAND> [ARGS]...` with the arguments unchanged.
On Unix msbx replaces itself with `msb`, so the terminal, signals and exit code are exactly
`msb`'s. msbx keeps no list of `msb` commands, so new ones work as soon as microsandbox ships them.

```sh
msbx ls                          # msb ls
msbx logs codex-myapp -f         # msb logs codex-myapp -f
msbx run alpine -- sh            # msb run alpine -- sh
msbx exec -t shell-myapp -- bash # msb exec -t shell-myapp -- bash
```

Use `msbx <COMMAND> --help` for `msb`'s help on a command.

## Finding `msb`

msbx uses the first `msb` on your `PATH`. If there is none, it exits with
`msb not found — install microsandbox: brew install superradcompany/tap/microsandbox`.
