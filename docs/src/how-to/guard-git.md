# Keep git on your Mac safe from the agent

An agent in the VM shares your project folder, `.git` included. If it could write a git hook or
a setting such as `core.fsmonitor`, git on your Mac would run that code as you. msbx prevents
this in new sandboxes and warns you when something slips through. The design and the
experiments behind it are in [RFC 0001](../rfcs/0001-git-safety.md).

## What a new sandbox does

- The agent runs as the unprivileged user `node`, under microsandbox's `restricted` profile
  (no `sudo`, no mount changes, even for root).
- `.git` is its own mount, so it cannot be renamed or replaced, and `.git/hooks`, `.git/info`,
  `.git/config`, `.git/config.worktree` and `.git/objects/info/alternates` are mounted
  read-only inside it. A `.git` pointer file, a `core.hooksPath` directory such as `.husky/_`
  and `include.path` files are read-only too when they are inside the project. Submodules get
  the same treatment: each `.git/modules/<path>` is its own mount with its `hooks`, `info` and
  `config` read-only, and the submodule's `.git` pointer file is read-only.
- `git commit`, `branch`, `switch`, `tag`, `stash`, `reset` and `log` work. The VM gets your
  Mac's `user.name` and `user.email` as its global identity.

Nothing to set up. Sandboxes created before this change keep running as root without guards;
msbx prints a note when you attach to one. Recreate it:

```sh
msbx rm -f codex-myapp
msbx agent codex
```

Two layouts are not guarded, and msbx says so with a `note:` when it creates the sandbox: a
linked worktree (`git worktree add`) or a submodule checkout used as the project, because their
git data lives outside the project folder (git does not work in the VM there either). Point
msbx at the main checkout instead.

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
git on your Mac runs what these name, even from `git status`. Do not run git in this folder yet; read the files as data from outside it:
  cd / && env -i PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null /usr/bin/git --no-pager config --file '/Users/you/dev/app/.git/config' --no-includes --null --list | od -c
  od -c '/Users/you/dev/app/.git/hooks/<name>'
```

The dashboard shows `git!` on the card and lists the changes in the Overview's `git` row until
the next session starts. In a guarded sandbox this should never happen; in an older one it
means the agent changed them. Either way:

1. Stop every sandbox that shares the project (`msbx ls`, then `msbx stop NAME`), and pause
   editor git integrations: anything that runs git in that folder runs what was planted.
2. Do not run git in the folder, not even `git status` with hooks turned off: `git status`
   refreshes the index and runs a planted clean filter, and `core.hooksPath=/dev/null` does not
   stop hooks defined in config (`hook.<name>.command`). Read the files as data instead:
   - the config, with the `env -i … /usr/bin/git … config --file … --no-includes --null --list`
     command in the warning. It parses only the named file, from `/`, with a cleared
     environment and an absolute git, and `od -c` shows every byte. Repeat it for each included
     file and for `config.worktree`;
   - hooks, `.git` pointer files, `commondir` and attribute files with `od -c FILE`. Check what
     a path is (`ls -l`) before reading it, and never execute a hook to see what it does.
3. Remove or revert what you did not put there. A hook you keep may call another script in the
   project that the agent could change; read that too.
4. If you use a hooks tool such as husky, changes under `.husky/` are your project's files and
   show up like any other edit.

The fingerprint is BLAKE3 hashes and executable bits, stored per sandbox under
`~/.local/state/msbx/git-guard/`; it never holds file contents. It moves on each time a session
starts, so starting the next session accepts the current state. Deleting a file there only
means the next comparison starts fresh.

## Make your own git ignore repository hooks everywhere

Git has no per-repository "untrusted" mode: settings in `.git/config` win over your global
config, so msbx cannot neutralise them from outside. A shell function such as
`git() { command git -c core.hooksPath=/dev/null -c core.fsmonitor=false "$@"; }` turns off
hook directories and fsmonitor, but not filter drivers, config-defined hooks or the other
settings in the RFC's table, so msbx does not recommend it as protection.

## Run the VM test

The guard's VM test is ignored in the normal suite. It needs `msb`, network access and a free
`127.0.0.1:47447` (stop your msbx proxy with `msbx proxy stop` first). From the repository root:

```sh
V=$(mktemp -d)
XDG_CONFIG_HOME=$V/c XDG_STATE_HOME=$V/s MSBX_PROXY_TOKEN=$(openssl rand -hex 16) \
  mise x -- cargo test --locked git_guard_holds_against_the_agent_and_root -- --ignored
```

It creates a temporary repository, sandbox and proxy, checks that neither the agent user nor
root can write hooks or config or unmount the guards, that the agent can commit, and that a hook
planted from the host side is reported, then removes everything it made.
