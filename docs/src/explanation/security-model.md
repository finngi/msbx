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
