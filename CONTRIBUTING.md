# Contributing to msbx

Thanks for helping. msbx is small on purpose, so please read the scope rule before starting on a feature.

## Scope

msbx does only what microsandbox's `msb` CLI lacks: the TUI (`msbx`) and `msbx run`. It does not wrap `ls`, `logs`, `stop`, `rm`, `exec` or other commands `msb` already has.

If a feature belongs in the runtime, the SDK or `msb` itself, propose it upstream in [microsandbox](https://github.com/superradcompany/microsandbox) instead. For anything larger than a small fix, open an issue first so we can agree on the approach.

## Where to report bugs

- **msbx bugs** (TUI, `msbx run`, sandbox naming or defaults): [open an issue here](https://github.com/finngi/msbx/issues/new/choose).
- **microsandbox bugs** (VM boot, mounts, networking, secrets, `msb` commands): report them in [microsandbox's issues](https://github.com/superradcompany/microsandbox/issues). If you can reproduce it with `msb` alone, it is upstream.
- **Security issues**: do not open a public issue. See [SECURITY.md](SECURITY.md).

## Development setup

1. Install Rust with [rustup](https://rustup.rs) (directly or through [mise](https://mise.jdx.dev)). rustup reads `rust-toolchain.toml` and installs the pinned channel with `rustfmt` and `clippy`.
2. Install microsandbox 0.7.11, which matches the SDK version msbx is pinned to:
   ```sh
   brew install superradcompany/tap/microsandbox
   msb --version
   ```
   You need macOS on Apple Silicon or Linux with KVM to run sandboxes. Building and the default tests do not need either.
3. Build and run:
   ```sh
   cargo run -- run shell .
   ```

## The gate

Every change must pass these with no warnings. CI runs the same checks on macOS (arm64) and Linux.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests that boot a real microVM are marked `#[ignore]` and do not run in CI. Run them locally before changing sandbox code:

```sh
cargo test -- --ignored
```

### Snapshot tests

The TUI is tested with [insta](https://insta.rs) snapshots rendered through ratatui's `TestBackend`. After changing the UI, review the new snapshots rather than accepting them blindly:

```sh
cargo install --locked cargo-insta   # once
cargo insta review
```

Commit the updated `.snap` files with your change.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`. Imperative subject, 72 characters or fewer.
- Keep pull requests small and focused on one change. Separate refactors from behaviour changes.
- Add or update tests for any logic you change.
- Add a line under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for user-visible changes.

By contributing, you agree that your contributions are licensed under the [Apache-2.0 licence](LICENSE) and that you will follow the [Code of Conduct](CODE_OF_CONDUCT.md).
