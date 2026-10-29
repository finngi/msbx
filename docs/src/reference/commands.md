# Commands

msbx defines three commands of its own. Everything else is passed to `msb`.

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
5. For `codex` and `pi`, the agent's sign-in file is linked to the volume `msbx-login-<agent>`,
   mounted at `/root/.msbx-login` (created when missing, 16 MiB, `noexec`). A plain file the
   agent left at that path is moved onto the volume first. A sandbox created without the volume
   gets a `note:` and no link.
6. For `pi`, `/root/.pi/agent/models.json` is written from the host's local model servers (see
   [Reach Ollama or LM Studio](../how-to/reach-host-models.md)).
7. msbx runs `msb exec -t <name> -- <command> <ARGS>` on your terminal and exits with its exit
   code.

Before step 1, on macOS, each agent secret that is unset or empty in the environment is looked
up in the Keychain (`msbx secret`). If any is found, msbx runs itself again with those values
in its environment and continues from there.

| Agent | Command in the VM | Installed on first use | Host-side secret | Shared sign-in file |
|---|---|---|---|---|
| `codex` | `codex` | `@openai/codex@0.148.0`, `git`, `ca-certificates` | `OPENAI_API_KEY`, sent only to `api.openai.com` | `/root/.codex/auth.json` |
| `pi` | `pi` | `@earendil-works/pi-coding-agent@1.4.0`, `git`, `ca-certificates` | none | `/root/.pi/agent/auth.json` |
| `shell` | `bash` | `git`, `curl`, `ca-certificates` | none | none |

## `msbx secret`

```text
msbx secret set <VAR>
msbx secret rm <VAR>
```

Stores or removes an agent API key in the macOS Keychain, as a generic password with service
`msbx` and account `VAR`. `VAR` must be a secret the agent table forwards; today that is only
`OPENAI_API_KEY`. Any other name is a usage error that lists the valid names.

`set` runs `security add-generic-password -U -s msbx -a VAR -w` on your terminal, so `security`
prompts for the value and msbx never reads it. `rm` runs `security delete-generic-password -s
msbx -a VAR`. Both exit with `security`'s exit code (44 when there is no such item).

When `msbx` or `msbx agent` starts, each secret var that is not set (or is empty) in the shell
is read with `security find-generic-password -s msbx -a VAR -w`. A shell variable always wins.
On Linux, or when `security` is missing, nothing is looked up.

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
