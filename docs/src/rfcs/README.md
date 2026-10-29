# RFCs

RFCs ("requests for comments") are design proposals for changes to msbx that are larger than a
small fix: new commands, new defaults, or anything that changes the
[security model](../explanation/security-model.md). Nothing described in an RFC exists until it
is implemented and released; the other sections of these docs describe what msbx does today.

## Process

1. Copy [`0000-template.md`](0000-template.md) to `docs/src/rfcs/NNNN-short-name.md`, using the
   next free number, and add it to `docs/src/SUMMARY.md`.
2. Open a pull request with the RFC alone, linking a tracking issue in its metadata
   (`- Tracking issue: #N`).
3. Discussion happens on the pull request. Update the RFC as it changes.
4. When accepted, the RFC is merged with `- Status: Accepted` and implementation follows in
   separate pull requests. A declined RFC may be merged with `- Status: Declined` to record the
   decision.

Each RFC starts with a short metadata list:

```markdown
- Status: Proposed
- Tracking issue: #N
```

## Index

| RFC | Title | Status |
|---|---|---|
| 0001 | [Git safety](0001-git-safety.md) | Proposed |
| 0002 | [Config](0002-config.md) | Proposed |
| 0003 | [GitHub access](0003-github-access.md) | Proposed |
| 0004 | [Shared logins](0004-shared-logins.md) | Proposed |
| 0005 | [Detachable sessions](0005-detachable-sessions.md) | Proposed |
| 0006 | [Setup script](0006-setup-script.md) | Proposed |
| 0007 | [Worktrees](0007-worktrees.md) | Proposed |
| 0008 | [Network](0008-network.md) | Proposed |
| 0009 | [Ports](0009-ports.md) | Proposed |
