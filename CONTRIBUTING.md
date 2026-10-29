# Contributing to msbx

Thanks for helping. msbx is small on purpose, so please read the scope rule before starting on a feature.

## Scope

msbx does only what microsandbox's `msb` CLI lacks: the TUI (`msbx`) and `msbx agent`. Every other command (`ls`, `logs`, `stop`, `rm`, `exec`, `run` and so on) is passed to `msb` unchanged, and msbx has no code or list for them.

If a feature belongs in the runtime, the SDK or `msb` itself, propose it upstream in [microsandbox](https://github.com/superradcompany/microsandbox) instead. For anything larger than a small fix, open an issue first so we can agree on the approach.

## Where to report bugs

- msbx bugs (TUI, `msbx agent`, sandbox naming or defaults): [open an issue here](https://github.com/finngi/msbx/issues/new/choose).
- microsandbox bugs (VM boot, mounts, networking, secrets, `msb` commands): report them in [microsandbox's issues](https://github.com/superradcompany/microsandbox/issues). If you can reproduce it with `msb` alone, it is upstream.
- Security issues: do not open a public issue. See [SECURITY.md](SECURITY.md).

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
   cargo run -- agent shell .
   ```

## The gate

Every change must pass these with no warnings. CI runs the same checks on macOS (arm64) and Linux.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests that boot a real microVM are marked `#[ignore]` and do not run in CI. Run them locally before changing sandbox code, with every msbx path pointed at temporary folders and a one-off proxy token, one test at a time (they share the proxy port):

```sh
XDG_CONFIG_HOME=$(mktemp -d) XDG_STATE_HOME=$(mktemp -d) \
MSBX_PROXY_TOKEN=$(openssl rand -hex 16) MSBX_TEST_SECRET=msbx-test-secret-$RANDOM$RANDOM \
cargo test -- --ignored --test-threads=1
```

Stop a running `msbx proxy` first: the tests bind its port.

### Snapshot tests

The TUI is tested with [insta](https://insta.rs) snapshots rendered through ratatui's `TestBackend`. After changing the UI, review each new snapshot before you accept it:

```sh
cargo install --locked cargo-insta   # once
cargo insta review
```

Commit the updated `.snap` files with your change.

## Docs

The documentation site is an [mdBook](https://rust-lang.github.io/mdBook/) in `docs/`, published to <https://finngi.github.io/msbx/> from `main`. Install the versions CI uses, then serve it with live reload:

```sh
cargo install --locked mdbook --version 0.5.0
cargo install --locked mdbook-mermaid --version 0.17.1
mdbook-mermaid install docs   # once: writes the git-ignored mermaid JS files
mdbook serve docs --open
```

Pages live in `docs/src/` and must be listed in `docs/src/SUMMARY.md`. They follow [Diátaxis](https://diataxis.fr/), so put each page in one category:

- `tutorials/`: lessons for a first-time user.
- `how-to/`: steps for one specific goal.
- `reference/`: exact descriptions of commands, keys and defaults.
- `explanation/`: background and reasoning, such as the security model.

Document only what the code does today. Proposals go in an RFC: copy `docs/src/rfcs/0000-template.md` to the next free number, add it to `SUMMARY.md`, and open a pull request with the RFC alone. Discussion happens on that pull request; implementation follows once it is accepted. Use ```` ```mermaid ```` fences for diagrams.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/): `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`. Imperative subject, 72 characters or fewer.
- Keep pull requests small and focused on one change. Separate refactors from behaviour changes.
- Add or update tests for any logic you change.
- Add a line under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for user-visible changes.

By contributing, you agree that your contributions are licensed under the [Apache-2.0 licence](LICENSE) and that you will follow the [Code of Conduct](CODE_OF_CONDUCT.md).
