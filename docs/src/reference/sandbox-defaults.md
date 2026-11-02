# Sandbox defaults

`msbx agent` and the dashboard's create wizard create sandboxes with these settings, after
[`config.toml`](#config-file) overrides. They are applied only when the sandbox is created; to
change them,
[remove and recreate it](../how-to/remove-or-reset.md). The one exception is the network
policy, which the proxy applies live (see [Control network access](../how-to/control-network-access.md)).

| Setting | Value |
|---|---|
| Name | `<agent>-<folder>` by default; editable in the dashboard wizard. The CLI keeps its generated name; see [Commands](commands.md#msbx-agent). |
| Image | `node:24-bookworm` (full Debian 12 with Node.js 24, git, curl, Python 3, gcc and make), or the image in `config.toml` |
| Mount | The project folder, read-write, at the same absolute path as on the host |
| Git guard | When the project has a `.git` directory: `.git` mounted read-write at the same path, and `.git/hooks`, `.git/info`, `.git/config`, `.git/config.worktree` and `.git/objects/info/alternates` (if present), a `.git` pointer file, and `core.hooksPath` or `include.path` targets inside the project mounted read-only; the same for each submodule git dir under `.git/modules` and its `.git` pointer file ([RFC 0001](../rfcs/0001-git-safety.md)). `hooks` and `info` are created when missing. A linked worktree or submodule checkout gets no guard and a `note:`. |
| User | uid 1000 (`node` in the default image; created as `node` by the install when an image has no uid 1000 account); the first-use install runs as root |
| Security profile | `restricted`: `no_new_privs`, no `CAP_SYS_ADMIN`, so nobody in the VM can unmount the guards or use `sudo` |
| Working directory | The project folder |
| CPUs | All host CPUs (capped at 255), or `cpus` in `config.toml` |
| Memory | Half of the host's RAM, at least 512 MiB and at most 32 GiB (Docker Sandboxes' rule), or `memory_mib` in `config.toml`. msbx reads the RAM with `sysctl -n hw.memsize` on macOS and `/proc/meminfo` on Linux, and uses 4 GiB if it cannot. The host backs only the pages the guest touches. |
| Network | Deny by default; DNS and the TCP host ports from `policy.toml` (default 11434 and 1234) to the host (`host.microsandbox.internal`); TCP to public addresses through the msbx proxy, which decides per host from `policy.toml` |
| Proxy | HTTP CONNECT to `127.0.0.1:47447`, Basic credentials: the sandbox name and the token from `MSBX_PROXY_TOKEN` (read on the host at each start, like secrets) |
| Secrets | The agent's API keys, if set on the host (see below) |
| Labels | `msbx.agent=<agent>`, `msbx.project=<absolute project path>`, `msbx.network=proxy`, `msbx.git=guarded` (or `none` when the project has no repository) |
| Lifecycle | Detached: the VM keeps running after you exit the agent |

Private networks, loopback, link-local and cloud metadata addresses match no rule, so the
default denies them. UDP other than DNS is dropped (an HTTP CONNECT proxy carries only TCP).
Sandboxes created before the proxy existed keep their `public + host` policy and are not
affected by `policy.toml`. Sandboxes created before the git guard run the agent as root with
no read-only mounts; the dashboard's `git` row says so.

Each time the sandbox is prepared for an agent, msbx also sets the VM's global `user.name` and
`user.email` from your Mac's git config, when the VM has none, so the agent can commit.

## Secrets

Each agent's API key variables and the hosts they are sent to are listed on the
[Agents](agents.md#credentials) page.

The sandbox stores only a reference to the host variable. Each time the sandbox
starts, microsandbox reads the variable from the host environment. Inside the VM the variable
holds a placeholder; microsandbox's proxy replaces it with the real value only on requests to
the allowed hosts. A variable that is unset or empty when the sandbox is created is not
forwarded; when none of the agent's variables is set, msbx prints a note.

## First-use install

On first use, msbx installs the agent in the VM, then writes `/root/.msbx-installed`:

1. As root, it makes sure the image has `ca-certificates`, `curl`, `git` and `bash` (plus
   `unzip` for kiro), installing any that are missing with `apt-get`; creates a `node` account
   with uid 1000 when the image has none; and, for an agent installed from npm when the image
   has no `npm`, installs Node.js 24.22.2 from `nodejs.org`, checked against its `SHASUMS256.txt`.
   Then it runs `npm install -g <package>@<version>` or the agent's own root script.
2. For cursor, devin and kiro it runs the vendor's install script as the agent user, which puts
   the agent in `~/.local/bin`; root then links those commands into `/usr/local/bin`, where
   `msb exec` finds them.

The [Agents](agents.md) page lists what each agent installs. If a step fails, msbx shows the
last 40 lines of its output. The agent itself runs as uid 1000, so the install is the place for
system packages. With the `balanced` preset the install's hosts are allowed; with
`locked-down`, allow them first (`msbx policy log <sandbox> --blocked` lists them).

### Other images

`config.toml` can name another image. It must be Debian- or Ubuntu-based (or already have the
packages above), for `x86_64` or `aarch64`, with glibc. On Ubuntu images uid 1000 is the
`ubuntu` user, which the agent then runs as. microsandbox refuses to boot a sandbox whose user
is missing from the image, which is why msbx sets the user by uid (1000).

## Config file

`~/.config/msbx/config.toml` (or `$XDG_CONFIG_HOME/msbx/config.toml`) is optional. Without it,
msbx uses the defaults above. It holds the part of [RFC 0002](../rfcs/0002-config.md) that
[RFC 0012](../rfcs/0012-agents-and-defaults.md) needed:

```toml
[defaults]               # every agent
image = "node:24-bookworm"
cpus = 4                 # 1 to 255
memory_mib = 8192        # at least 512

[agents.claude]          # one agent; the names are those of `msbx agent`
image = "ubuntu:24.04"   # wins over [defaults]
auto_approve = false     # keep Claude Code's permission prompts
args = ["--model", "opus"]  # added after the defaults on every launch
```

`image`, `cpus` and `memory_mib` apply when a sandbox is created; an existing sandbox keeps
its own. `auto_approve` and `args` apply at every launch, from the CLI or the dashboard. msbx
reads the file before it does anything else and stops on an unknown key, an unknown agent or a
bad value, naming the line:

```text
error: reading config /Users/you/.config/msbx/config.toml: TOML parse error at line 2, column 1
  |
2 | cpu = 4
  | ^^^
unknown field `cpu`, expected one of `image`, `cpus`, `memory_mib`
```
