# Sandbox defaults

`msbx agent` and the dashboard's create popup create sandboxes with these settings. They are
applied only when the sandbox is created; to change them,
[remove and recreate it](../how-to/remove-or-reset.md). There is no config file.

| Setting | Value |
|---|---|
| Name | `<agent>-<folder>`, see [Commands](commands.md#msbx-agent) |
| Image | `node:24-bookworm-slim` |
| Mount | The project folder, read-write, at the same absolute path as on the host |
| Working directory | The project folder |
| CPUs | All host CPUs (capped at 255) |
| Memory | 32 GiB limit. The host backs only the pages the guest touches. |
| Network | microsandbox profiles `public` and `host`: the public internet plus services on the host at `host.microsandbox.internal` |
| Secrets | The agent's API keys, if set on the host (see below) |
| Labels | `msbx.agent=<agent>`, `msbx.project=<absolute project path>` |
| Lifecycle | Detached: the VM keeps running after you exit the agent |

## Secrets

| Agent | Variable | Allowed host |
|---|---|---|
| `codex` | `OPENAI_API_KEY` | `api.openai.com` |
| `pi`, `shell` | none | |

The sandbox stores only a reference to the host variable. Each time the sandbox
starts, microsandbox reads the variable from the host environment. Inside the VM the variable
holds a placeholder; microsandbox's proxy replaces it with the real value only on requests to
the allowed host. A variable that is unset or empty when the sandbox is created is not
forwarded, and msbx prints a note.

## First-use install

On first use, msbx runs the agent's install script as root in the VM (see
[Commands](commands.md#msbx-agent)), then writes `/root/.msbx-installed`. If the install fails,
msbx shows the last 40 lines of its output.
