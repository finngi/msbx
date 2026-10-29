# Security model

msbx adds no isolation of its own. Isolation is provided by microsandbox; read its
[security documentation](https://github.com/superradcompany/microsandbox/tree/main/docs/security)
and [security policy](https://github.com/superradcompany/microsandbox/blob/main/SECURITY.md).

## Isolated

- Each sandbox is a separate microVM with its own kernel.
- Only the project folder is mounted from the host filesystem.
- API keys stay on the host. The VM sees a placeholder, which microsandbox replaces with
  the real key only on requests to the allowed host.

## Not isolated

- The project folder is writable from the VM. An agent can modify or delete anything in it,
  including `.git`. Commit or back up first.
- Network access follows your policy (see below). With the `open` preset the VM can reach the
  public internet and send project contents anywhere. With any preset, an allowed host can
  still receive data.
- Host services: only ports 11434 (Ollama) and 1234 (LM Studio) at
  `host.microsandbox.internal`, fixed when the sandbox is created.
- TLS is intercepted when secrets are used. To substitute keys, microsandbox adds a local CA
  to the VM's trust store and terminates TLS on the host. Agents that pin certificates may fail.
- Secret values live on the host, in memory and possibly in microsandbox's host-side sandbox
  configuration (readable only by your user account).

## Network policy and the proxy

Every msbx sandbox sends its public TCP traffic through the msbx proxy on `127.0.0.1:47447`,
which applies `~/.config/msbx/policy.toml` per connection
([how-to](../how-to/control-network-access.md), [RFC 0008](../rfcs/0008-network.md)).

What it stops:

- Connections to hosts your policy does not allow, by the name in the TLS handshake or the HTTP
  `Host` header. The name only counts when it resolves on your machine to the address the VM
  connected to, so a guest cannot borrow an allowed name for another server.
- Spoofed sandbox names: the proxy accepts only the token in
  `~/.local/state/msbx/proxy-token` (mode 600), so other local processes cannot pass as a
  sandbox.
- Private networks, loopback, link-local and cloud metadata addresses, denied by the static
  policy in the sandbox before the proxy is involved.

What it does not stop:

- DNS: the sandbox can resolve any name through microsandbox's gateway.
- Data leaving through an allowed host, such as `github.com` under `balanced`.
- Host access beyond the two model ports being fixed at create; the proxy cannot change it,
  because traffic to your machine bypasses the proxy.
- Traffic with no name in its first bytes (plain TCP, or a server-first protocol such as
  SSH) is decided by the address alone.
- A legitimate connection blocked as `name-mismatch` when DNS answers differ between the VM and
  your machine.

The proxy binds loopback only, never logs payloads or credentials, and its log holds host
names, ports, counts and times in `~/.local/state/msbx/network-log.json`. The unauthenticated
status request reveals the msbx version, pid and policy path to local processes.

## Keychain keys

`msbx secret set` stores an API key in the macOS Keychain through Apple's `security` tool,
which prompts for the value; msbx never reads it on that path. At startup msbx reads each
unset secret with `security find-generic-password` and runs itself again with the value in its
environment, where microsandbox's secret proxy picks it up. The value is not printed, logged,
put in an error message or passed on a command line. The first time, macOS may ask whether
`security` may read the item.

## Shared sign-ins

Codex and pi sign-ins (the `auth.json` each writes) are kept on one named volume per agent,
`msbx-login-<agent>`, and linked into every sandbox of that agent. See
[RFC 0004](../rfcs/0004-shared-logins.md) for the design and its risk table. In short:

- Any sandbox of that agent can read the token, and could swap in another account's token.
  Use separate agents or remove the volume for repos you do not trust.
- The volume is mounted `noexec`, `nosuid` and `nodev`, and msbx removes a link planted on it
  before linking.
- The token is plain JSON under `~/.microsandbox/volumes/msbx-login-<agent>/` on the host,
  readable only by your user account.
- Only the sign-in file is shared. Config, sessions and history stay in each sandbox.

## Local models for pi

msbx copies only loopback providers from your pi `models.json` into the VM, so a cloud API key
in that file stays on the host. The copied file is written with mode 600.

## Git: hooks and settings the VM cannot plant

`.git` is inside the shared project folder, and git on your Mac runs programs named in it:
scripts in `.git/hooks/` (or the `core.hooksPath` directory), and settings such as
`core.fsmonitor`, `core.sshCommand`, filter and diff drivers, `credential.helper` and `!`
aliases in `.git/config`. Cursor shipped this as CVE-2026-26268. msbx closes it in three layers
([RFC 0001](../rfcs/0001-git-safety.md), [how-to](../how-to/guard-git.md)):

- In new sandboxes the agent runs as the unprivileged user `node` under microsandbox's
  `restricted` profile (`no_new_privs`, no `CAP_SYS_ADMIN` even for root). `.git` is its own
  read-write mount, and `.git/hooks`, `.git/info`, `.git/config`, `.git/config.worktree`, a
  `.git` pointer file, and any `core.hooksPath` or `include.path` target inside the project
  are read-only mounts. Measured in a VM: the agent and root both get `Read-only file system`
  or `Device or resource busy`, `umount` fails, and `git commit` works.
- msbx runs git on your Mac only through one helper with `core.hooksPath=/dev/null`,
  `core.fsmonitor=false` and no config includes, and only to read a repository's config and
  your identity.
- msbx fingerprints those files before each session and warns, on the terminal and with a
  `git!` badge in the dashboard, when they changed during a session or since the last one.

What stays open:

- Sandboxes created before this change run as root without guards. msbx says so when you
  attach; recreate them with `msbx rm -f NAME`.
- The agent can still rewrite history or corrupt the repository (`objects`, `refs`, `index`
  are writable). That costs you work; no code runs on your Mac.
- `.gitattributes` and `.lfsconfig` are writable; they only matter with a filter driver in your
  global config (git-lfs reads only a safe subset of `.lfsconfig`). A nested repository the
  agent creates under the project is only used if you `cd` into it. Submodules and linked
  worktrees are not guarded yet.
- Anything else on your Mac that reacts to project files (`package.json` scripts, `mise.toml`,
  editor tasks) is out of scope.

## Reporting vulnerabilities

Report msbx vulnerabilities as described in
[SECURITY.md](https://github.com/finngi/msbx/blob/main/SECURITY.md). Report microsandbox
vulnerabilities to microsandbox.
