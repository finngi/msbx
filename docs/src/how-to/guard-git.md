# Keep git on your Mac safe from the agent

An agent in the VM shares your project folder, `.git` included. If it could write a git hook or
a setting such as `core.fsmonitor`, git on your Mac would run that code as you. msbx prevents
this in new sandboxes and warns you when something slips through. The design and the
experiments behind it are in [RFC 0001](../rfcs/0001-git-safety.md).

## What a new sandbox does

- The agent runs as the unprivileged user `node`, under microsandbox's `restricted` profile
  (no `sudo`, no mount changes, even for root).
- `.git` is its own mount, so it cannot be renamed or replaced, and `.git/hooks`, `.git/info`,
  `.git/config` and `.git/config.worktree` are mounted read-only inside it. A `.git` pointer
  file, a `core.hooksPath` directory such as `.husky/_` and `include.path` files are read-only
  too when they are inside the project.
- `git commit`, `branch`, `switch`, `tag`, `stash`, `reset` and `log` work. The VM gets your
  Mac's `user.name` and `user.email` as its global identity.

Nothing to set up. Sandboxes created before this change keep running as root without guards;
msbx prints a note when you attach to one. Recreate it:

```sh
msbx rm -f codex-myapp
msbx agent codex
```

## What the agent cannot do, and what to do instead

| The agent runs | Result | Do this instead |
|---|---|---|
| `apt-get install …`, `sudo …` | Fails: not root, and `sudo` is blocked | On your Mac: `msbx exec -t -u root codex-myapp -- apt-get install -y PACKAGE` |
| `git config user.email …` (repository scope) | `could not write config file .git/config: Device or resource busy` | `git config --global user.email …` in the VM; msbx seeds it from your Mac |
| `git remote add …`, `git branch --set-upstream-to …` | The same error | Run them on your Mac |
| Writing `.git/hooks/*` or `.git/info/*` | `Read-only file system` | Edit hooks on your Mac |

`npm`, `pip`, `cargo` and the like still work for the agent user: they install into its home.

## When msbx warns

At the end of a session, and again when the next one starts, msbx compares the files git
executes from with a fingerprint it took before:

```text
warning: git files changed during this session in /Users/you/dev/app:
  .git/hooks/pre-commit (new)
  .git/config: core.fsmonitor = ./tools/watch.sh (new)
git runs these on your Mac. Review them before running git in this folder:
  git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /Users/you/dev/app status
```

The dashboard shows `git!` on the card and lists the changes in the Overview's `git` row until
the next session starts. In a guarded sandbox this should never happen; in an older one it
means the agent changed them. Either way:

1. Do not run plain `git` in that folder yet. The command in the warning runs git with hooks
   and fsmonitor off; `cat .git/config` and `ls -l .git/hooks` need no git at all.
2. Remove or revert what you did not put there.
3. If you use a hooks tool such as husky, changes under `.husky/` are your project's files and
   show up like any other edit.

The fingerprint is stored per sandbox under `~/.local/state/msbx/git-guard/`. Deleting it
only means the next comparison starts fresh.

## Make your own git ignore repository hooks everywhere

Git has no per-repository "untrusted" mode: settings in `.git/config` win over your global
config, so msbx cannot neutralise them from outside. If you want your shell's `git` to never
run repository hooks or fsmonitor, add a function to your shell profile:

```sh
git() { command git -c core.hooksPath=/dev/null -c core.fsmonitor=false "$@"; }
```

This also turns off hooks you rely on, so msbx leaves that choice to you.
