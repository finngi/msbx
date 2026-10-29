# Security policy

## Supported versions

msbx is pre-1.0. Only the latest release (or `main`, before the first release) receives security fixes.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's private vulnerability reporting:

1. Go to the [Security tab](https://github.com/finngi/msbx/security) of `finngi/msbx`.
2. Click **Report a vulnerability**.

Do not open a public issue, pull request or discussion for a security problem.

Include:

- what the issue is and its impact
- steps or a minimal reproduction
- the msbx version or commit, `msb --version`, and your OS and architecture

You will get an acknowledgement as soon as possible. msbx is maintained by one person in their spare time, so please allow reasonable time for a fix before disclosing publicly.

## Scope

In scope: msbx's own code, for example:

- the sandbox configuration msbx creates (mounts, network profile, secret bindings)
- handling of API keys before they are passed to microsandbox
- the TUI and `msbx agent` acting on the wrong sandbox or the wrong path

Out of scope here: the isolation boundary itself. Sandbox escape, isolation bypass between sandboxes, network-policy bypass and secret leakage inside the runtime are microsandbox vulnerabilities. Report them to microsandbox as described in its [security policy](https://github.com/superradcompany/microsandbox/blob/main/SECURITY.md).

If you are unsure which project an issue belongs to, report it here and we will help route it.
