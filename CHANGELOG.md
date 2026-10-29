# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Before 1.0.0, minor versions may contain breaking changes.

## [Unreleased]

### Added

- `msbx run <codex|pi|shell> [PATH] [-- ARGS]`: create or reuse a per-project sandbox named `<agent>-<folder>` and attach to it.
- Sandbox defaults: project bind-mounted at the same path, all host CPUs, 32 GiB memory limit, public internet plus host services, agent API keys injected host-side, `node:24-bookworm-slim` image with the agent installed once.
- `msbx` terminal dashboard: sandbox cards, Overview and Logs tabs, and keys to attach, open a shell, create, start/stop and remove sandboxes.

[unreleased]: https://github.com/finngi/msbx/commits/main
