# Introduction

msbx ("microsandbox extended") is a terminal dashboard and one-command agent launcher for
[microsandbox](https://github.com/superradcompany/microsandbox) microVMs. Every other command
goes straight to `msb`.

> **Status:** early, pre-1.0. Expect breaking changes. microsandbox itself is beta software.
>
> msbx is an independent project. It is not affiliated with or endorsed by superradcompany or
> the microsandbox project.

## What it adds

microsandbox is an open-source (Apache-2.0) microVM runtime. Its `msb` CLI and Rust SDK already
cover mounts, resource limits, networking, host-side secrets and sandbox lifecycle. msbx adds
only what `msb` lacks:

- **`msbx agent`** starts a coding agent (Codex, pi) or a shell in a sandbox for the current
  project. The sandbox is created the first time and reused after that.
- **`msbx`** opens a terminal dashboard of all microsandbox sandboxes, including ones msbx did
  not create, with live CPU and memory, an Overview tab and a Logs tab.
- **Everything else is `msb`.** `msbx ls`, `msbx logs NAME`, `msbx run IMAGE` and any other
  command run `msb` with the same arguments, so `msbx` can stand in for `msb` everywhere.

## How these docs are organised

The pages follow [Diátaxis](https://diataxis.fr/):

- **Tutorials** are lessons: start here if you have never used msbx.
- **How-to guides** are directions for a specific goal.
- **Reference** describes commands, keys and defaults exactly.
- **Explanation** gives background: the security model and how msbx uses microsandbox.
- **Design (RFCs)** holds proposals for changes that are not built yet.

## Requirements

- macOS on Apple Silicon, or Linux with KVM enabled (microsandbox's requirements).
- microsandbox 0.7.11, the version msbx's SDK is pinned to.
- Rust stable (1.88 or newer) to build msbx.

Windows is supported by microsandbox but untested with msbx.
