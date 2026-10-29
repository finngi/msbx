## Summary

<!-- What does this change do? -->

## Why

<!-- The problem it solves. Link the issue: "Closes #123". -->

## How tested

<!-- Paste results. Mention manual checks against a real sandbox if relevant. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo test -- --ignored` (real VM; needed for sandbox changes)

## Checklist

- [ ] In scope: adds something `msb` lacks, rather than wrapping an existing `msb` command
- [ ] Tests added or updated for changed logic; snapshots reviewed with `cargo insta review`
- [ ] `CHANGELOG.md` updated under `[Unreleased]` for user-visible changes
- [ ] Commits follow Conventional Commits
