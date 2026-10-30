# Sandbox defaults

`msbx agent` and the dashboard's create wizard create sandboxes with these settings. They are
applied only when the sandbox is created; to change them,
[remove and recreate it](../how-to/remove-or-reset.md). The one exception is the network
policy, which the proxy applies live (see [Control network access](../how-to/control-network-access.md)).

| Setting | Value |
|---|---|
| Name | `<agent>-<folder>` by default; editable in the dashboard wizard. The CLI keeps its generated name; see [Commands](commands.md#msbx-agent). |
| Image | `node:24-bookworm-slim` |
| Mount | The project folder, read-write, at the same absolute path as on the host |
| Working directory | The project folder |
| CPUs | All host CPUs (capped at 255) |
| Memory | 32 GiB limit. The host backs only the pages the guest touches. |
| Network | Deny by default; DNS and TCP ports 11434 and 1234 to the host (`host.microsandbox.internal`); TCP to public addresses through the msbx proxy, which decides per host from `policy.toml` |
| Proxy | HTTP CONNECT to `127.0.0.1:47447`, Basic credentials: the sandbox name and the token from `MSBX_PROXY_TOKEN` (read on the host at each start, like secrets) |
| Secrets | The agent's API keys, if set on the host (see below) |
| Labels | `msbx.agent=<agent>`, `msbx.project=<absolute project path>`, `msbx.network=proxy` |
| Lifecycle | Detached: the VM keeps running after you exit the agent |

Private networks, loopback, link-local and cloud metadata addresses match no rule, so the
default denies them. UDP other than DNS is dropped (an HTTP CONNECT proxy carries only TCP).
Sandboxes created before the proxy existed keep their `public + host` policy and are not
affected by `policy.toml`.

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
msbx shows the last 40 lines of its output. With the `balanced` or `locked-down` preset the
install needs `deb.debian.org`, `security.debian.org` and `registry.npmjs.org`; `balanced`
allows them.
