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
- Network access is open. The VM can reach the public internet, so it can send project
  contents anywhere.
- Host services are reachable at `host.microsandbox.internal`. Ollama is one of them, and
  anything else listening on the host may be reachable from the VM too.
- TLS is intercepted when secrets are used. To substitute keys, microsandbox adds a local CA
  to the VM's trust store and terminates TLS on the host. Agents that pin certificates may fail.
- Secret values live on the host, in memory and possibly in microsandbox's host-side sandbox
  configuration (readable only by your user account).

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

## Known issue: git hooks can run VM-written code on the host

> **Warning.** This is a known, unfixed gap. It is tracked in the
> [Git safety RFC](../rfcs/0001-git-safety.md).

Because `.git` is inside the writable project folder, code in the VM can add or change files
that git on the host later executes, such as scripts in `.git/hooks/` or settings in
`.git/config` that name a program to run (for example `core.hooksPath` or `core.fsmonitor`).
The next time you run `git commit`, `git status` or a similar command in that project **on the
host**, that code runs as your user, outside the sandbox.

Until this is fixed:

- Treat a project an agent has worked on as untrusted on the host.
- Before running git on the host, check `.git/hooks/` and `.git/config` for changes, or run git
  for that project only inside the sandbox.

## Reporting vulnerabilities

Report msbx vulnerabilities as described in
[SECURITY.md](https://github.com/finngi/msbx/blob/main/SECURITY.md). Report microsandbox
vulnerabilities to microsandbox.
