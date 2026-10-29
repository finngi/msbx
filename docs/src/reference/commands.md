# Commands

msbx defines five commands of its own: `msbx`, `agent`, `secret`, `policy` and `proxy`.
Everything else is passed to `msb`.

## `msbx`

```text
msbx
```

Opens the [dashboard](dashboard-keys.md) of all microsandbox sandboxes. The current directory is
the default project for sandboxes created from the dashboard. When a network policy exists, msbx
makes sure the [proxy](#msbx-proxy) is running first.

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

1. If `policy.toml` does not exist, msbx asks for a preset on a terminal and writes the file;
   without a terminal it stops with `run msbx policy init <open|balanced|locked-down>`. Then it
   makes sure the proxy is running.
2. The sandbox is named `<agent>-<folder>`: the folder name is lowercased, every run of
   characters outside `a-z0-9` becomes one `-`, and the name is cut to 128 bytes.
3. If a sandbox with that name exists for the same folder, it is started if needed and reused.
   If it exists for a different folder, or was not created by msbx, msbx stops with an error.
4. Otherwise it is created with the [sandbox defaults](sandbox-defaults.md), including the proxy.
5. On first use the agent is installed in the VM (a marker file, `/root/.msbx-installed`, records
   that it is done).
6. For `codex` and `pi`, the agent's sign-in file is linked to the volume `msbx-login-<agent>`,
   mounted at `/root/.msbx-login` (created when missing, 16 MiB, `noexec`). A plain file the
   agent left at that path is moved onto the volume first. A sandbox created without the volume
   gets a `note:` and no link.
7. For `pi`, `/root/.pi/agent/models.json` is written from the host's local model servers (see
   [Reach Ollama or LM Studio](../how-to/reach-host-models.md)).
8. msbx runs `msb exec -t <name> -- <command> <ARGS>` on your terminal and exits with its exit
   code.

Before step 1, msbx runs itself again once with the proxy token (`MSBX_PROXY_TOKEN`) and, on
macOS, each agent secret that is unset or empty in the environment and present in the Keychain
(`msbx secret`) added to its environment, and continues from there.

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

## `msbx policy`

```text
msbx policy init <open|balanced|locked-down>
msbx policy ls [--sandbox NAME]
msbx policy allow <TARGETS> [--sandbox NAME]
msbx policy deny <TARGETS> [--sandbox NAME]
msbx policy rm <TARGET> [--sandbox NAME]
msbx policy check <HOST[:PORT]|IP[:PORT]> [--sandbox NAME]
msbx policy log [NAME] [--blocked|--allowed] [--limit N]
msbx policy preset <open|balanced|locked-down>
```

Edits and reads `$XDG_CONFIG_HOME/msbx/policy.toml` (default `~/.config/msbx/policy.toml`),
which the proxy applies to every new connection. See
[Control network access](../how-to/control-network-access.md).

| Command | Effect |
|---|---|
| `init` | Writes the file with a preset and no rules. Fails if it exists. |
| `ls` | The effective rules, denies first, each with its source: `sandbox`, `global` or `preset`. |
| `allow`, `deny` | Add comma-separated targets to the global lists or, with `--sandbox`, to that sandbox's. A target moves from the opposite list of the same scope. Invalid targets are rejected before anything is written. |
| `rm` | Removes a target from both lists of the scope. Errors when it was in neither. |
| `check` | Prints `Allowed` or `Blocked`, the target and the deciding rule, by name alone (no DNS). Exits 1 when blocked. Patterns and CIDRs are not accepted. |
| `log` | The proxy's connection log: a `BLOCKED` section then an `ALLOWED` one, columns `SANDBOX HOST PORT RULE LAST-SEEN COUNT`, newest first. `NAME` limits it to one sandbox; `--limit` caps each section. |
| `preset` | Changes the preset and keeps the rules. |

Targets:

| Form | Matches |
|---|---|
| `example.com` | that host on any port (case-insensitive, trailing dot ignored) |
| `*.example.com` | exactly one extra label |
| `**.example.com` | one or more extra labels; not `example.com` itself |
| `**` | every host and address |
| `TARGET:443` | the same, on one port |
| `10.0.0.1`, `2001:db8::1`, `[2001:db8::1]:443` | one address |
| `10.0.0.0/8`, `2001:db8::/32` | a network (no port) |

Rejected: single-label wildcard suffixes (`*.com`), empty labels, labels over 63 bytes, ports
outside 1-65535, prefix lengths over 32 or 128. Host rules match only when the proxy saw a host
name; address rules match the address.

Decision order for a connection: any matching deny blocks; else any matching allow allows; else
the preset decides (`open` allows, the others block). Rules of the sandbox and the global rules
count alike.

## `msbx proxy`

```text
msbx proxy serve
msbx proxy status
msbx proxy stop
```

The HTTP CONNECT proxy on `127.0.0.1:47447` that every msbx sandbox is created to use.

| Command | Effect |
|---|---|
| `serve` | Runs the proxy in the foreground until `stop`, SIGTERM or Ctrl-C. Fails if the port is taken. |
| `status` | `running: msbx <version> (pid N), policy <path>` and exit 0, or `not running` and exit 1. Errors if the port is held by something else. |
| `stop` | Asks the running proxy to flush its log and exit. |

`msbx agent`, the dashboard and pass-through commands start the proxy themselves when nothing
answers on the port: `msbx proxy serve` in a new process group, output appended to
`$XDG_STATE_HOME/msbx/proxy.log` (default `~/.local/state/msbx/proxy.log`), waiting up to 2 s.
A proxy from another msbx version is left running with a note to run `msbx proxy stop`.

Per connection the proxy checks the Basic credentials (sandbox name and the token in
`$XDG_STATE_HOME/msbx/proxy-token`), answers `407` to anything else, reads the guest's first
bytes for a TLS server name or an HTTP `Host`, checks that the name resolves to the address msb
connected to, decides, logs, and relays or closes. The log is written to
`$XDG_STATE_HOME/msbx/network-log.json` every 2 s when it changed and on exit, and holds at most
5,000 rows.

## Pass-through to `msb`

```text
msbx <COMMAND> [ARGS]...
```

Any command msbx does not define runs `msb <COMMAND> [ARGS]...` with the arguments unchanged.
On Unix msbx replaces itself with `msb`, so the terminal, signals and exit code are exactly
`msb`'s. msbx keeps no list of `msb` commands, so new ones work as soon as microsandbox ships them.

Before handing over, msbx adds the proxy token (and Keychain secrets) to the environment and,
when a policy exists, makes sure the proxy is running, so `msbx start NAME` works for proxied
sandboxes where plain `msb start NAME` does not.

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
