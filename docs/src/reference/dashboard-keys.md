# Dashboard keys

Run `msbx` to open the dashboard. Cards lists sandboxes and a final `+ new sandbox` card.
Detail shows Overview, Logs, Network or Rules for the selected sandbox. A title marker and
border identify the focused pane. Below 100 columns, the focused pane fills the screen;
switching focus still works. `NO_COLOR=1` disables colors while markers and status words remain.

## Navigation

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | Switch between Cards and Detail |
| `←` / `h` | Focus Cards |
| `→` / `l` | Focus Detail |
| `↑` / `k`, `↓` / `j` | Move within the focused pane |
| `[` / `]` | Previous / next detail tab |
| `c` | Open the create wizard; choose a preset first if no policy exists |
| `n` | Open the full network view |
| `p` | Choose the network preset |
| `i` | Show the selected sandbox's info (outside the full network view) |
| `Ctrl-S` | Open settings: default preset and host ports |
| `?` / `Ctrl-H` | Show contextual key help outside a form or search input |
| `q` / `Ctrl-C` | Quit |

Help and footer hints use the same keymap. Text inputs take ownership of typed characters;
for example, `q` in a name or search field inserts a letter. `Ctrl-C` quits from every view.

## Cards

These actions apply while Cards has focus.

| Key | Action |
|---|---|
| `Enter` | Attach the sandbox's agent; open a shell for a sandbox msbx did not create. On `+ new sandbox`, open the create wizard. |
| `x` | Open a shell (`bash`) in the sandbox |
| `s` | Start a stopped or crashed sandbox; stop any other |
| `r` | Ask to remove the sandbox |

Attaching gives the agent the terminal. The dashboard returns when you exit and shows the exit
code. Stopping waits up to 30 seconds for a graceful shutdown and never kills the VM.

A card shows `git!` in red when the project's git hooks, config or pointer files changed since
the sandbox's last session started; the Overview's `git` row lists the changes, or reads
`guarded` / `not guarded (created before git guards; …)`. The check runs every 2 seconds for
every sandbox msbx created, against the baseline stored by its last session
([Keep git safe](../how-to/guard-git.md)).

## Network

Rows show time, host, port, hits and status. Each row combines the history for one sandbox,
host and port. Status is the latest recorded outcome; expansion shows both allowed and
blocked counts. The log refreshes every 2 seconds.

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Previous / next row |
| `Enter` | Expand or collapse the selected row |
| `PageUp` / `PageDown` | Scroll the expanded row's details |
| `a` | Allow a blocked row's host for that sandbox |
| `A` | Allow the selected host everywhere |
| `b` | Block an allowed row's host for that sandbox |
| `p` | Choose the network preset |

Expanded rows show status, complete host and port, sandbox, first/last seen and total,
allowed and blocked hits. Policy edits affect the next connection; they do not rewrite
history. A matching deny still overrides an allow.

Sandboxes created before network policy and sandboxes msbx did not create are marked as
unproxied. Their current traffic bypasses this log and policy; `a`, `A` and `b` do nothing for
those sandboxes.

## Full network view

`n` opens traffic across all sandboxes and adds a Sandbox column. Brackets switch between
Network and Rules. Network row actions work here too.

| Key | Action |
|---|---|
| `/` | Edit search text; `Enter` applies it, `Esc` restores the previous query |
| `o` | Cycle All, Allowed and Blocked status |
| `s` | Cycle All sandboxes and individual sandbox names |
| `Esc` | Cancel search editing, clear the query, collapse the expanded row, then return to the dashboard |
| `n` | Return to the dashboard outside text input |

Empty results explain how to clear filters. Sandbox choices include names present only in
saved traffic or rules.

## Rules

Rules lists the effective preset, global and sandbox entries with their source and scope.
The detail tab uses the selected sandbox. The full view uses its sandbox filter; All sandboxes
shows all scopes.

| Key | Action |
|---|---|
| `↑` / `k`, `↓` / `j` | Previous / next rule |
| `a` | Add a target, choosing Allow or Block |
| `r` | Confirm removal of an explicit rule |
| `p` | Change the preset |

In the add dialog, type the target, press `Tab` to switch Allow / Block, and `Enter` to save.
`Esc` cancels. The dialog shows its scope. In the full view, All sandboxes adds a global rule;
selecting a sandbox scopes the rule to it. Preset rules are read-only. Removing a global rule affects
every sandbox, even when viewed from one sandbox's detail tab. The confirmation names the
exact target and scope. Adding from a detail tab requires a proxied sandbox; use `n` for global
rules.

Targets use the same syntax as [`msbx policy`](commands.md#msbx-policy): hosts, wildcards,
ports, IPs and CIDRs. Invalid targets stay in the dialog with an error. Saving or removing a
rule reloads the proxy policy for subsequent connections.

## Create wizard

Choose Codex, pi or shell from the agent list. Each has a description and credential summary:
Codex reports `OPENAI_API_KEY` availability from the shell or macOS Keychain and shared login
presence; pi reports shared login presence; shell needs no credentials. Presence does not
prove a credential is valid, and values are never displayed.

Continue to configure the name and workspace. The name defaults to `<agent>-<folder>`; editing
it preserves your choice when the folder changes. Names use lowercase ASCII letters, digits
and single interior hyphens, up to 128 bytes. Invalid names stay in the form with an error.
A name already in use is rejected; attach to that card or choose another name. The `msbx agent`
command keeps its generated name.

| Stage | Keys |
|---|---|
| Agent | `↑` / `↓` (`k` / `j`) selects; `Enter` continues; `Esc` cancels |
| Configure | `Tab` / `Shift-Tab` moves between Name, Workspace, Back and Create; `Enter` chooses |
| Name | Type or paste; `←` / `→`, `Home`, `End`, `Backspace`, `Delete` edit; `Ctrl-U` clears |
| Folder | Type to filter; `↑` / `↓` selects; `Enter` opens a folder or chooses Create here; `Esc` returns to Configure |

The folder picker starts in the launch folder. Open `..` to go up, or choose Create here to use
the current folder. Letters including `j` and `k` filter folder names; use arrows to move the
selection. The chosen folder is the sandbox's writable workspace. Back returns to agent
selection. Escape from Configure returns to agent selection; Escape there cancels the wizard.

Creating returns to the dashboard while a background job runs. Progress appears on the status
line and the new card is selected when ready. Press `Enter` on that card to attach.

## Preset popup

Opened by `p`, or before the first create when `policy.toml` does not exist.

| Key | Action |
|---|---|
| `↑` / `↓` / `←` / `→` | Choose Open, Balanced or Locked Down |
| `Enter` | Save the preset, then open the create wizard on first run |
| `Esc` | Cancel |

## Info popup

`i` opens everything msbx knows about the selected card, including sandboxes msbx did not
create. Fields the runtime does not report show `—`.

| Section | Fields |
|---|---|
| Identity | Name, agent, project, status, creation time, image |
| Resources | vCPUs and current load, memory used / limit, root disk used and free, uptime, traffic in and out |
| Security | Guest user (`node` for msbx sandboxes), security profile (`restricted`), git guard |
| Network | Whether the msbx proxy applies, the current preset, the host ports set at create and the ones configured now, and the number of rules scoped to this sandbox |
| Mounts and secrets | Mounts, and secret variable names only; values never leave your Mac |

When the configured host ports differ from the sandbox's, the row is yellow and names the
`msb rm -f NAME` that starts the recreate. `↑` / `↓` (`k` / `j`) scroll; `Esc`, `i` or `q`
close.

## Settings

`Ctrl-S` opens two settings, saved together in `policy.toml`:

- Default network preset: Open, Balanced or Locked Down, the same choice as `p`. Saving
  reloads the proxy, so the next connection follows it.
- Host ports: the ports on your Mac (`host.microsandbox.internal`) new sandboxes may reach,
  separated by commas or spaces. Empty allows none; DNS still works. They are checked like
  `msbx policy host-ports` and an invalid list stays in the form with an error. They apply only
  to sandboxes created afterwards; recreate a sandbox to change its ports.

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | Switch between the preset and the host ports |
| `↑` / `↓` / `←` / `→` | Choose the preset (preset field) |
| Type, `Ctrl-U` | Edit or clear the host ports (ports field) |
| `Enter` | Save both |
| `Esc` | Cancel |

Without a `policy.toml`, saving creates it, as the first preset choice does.

## Confirmations and help

Removal dialogs require `y`; `n` or `Esc` cancels. Merely opening a dialog never removes a
sandbox or rule. Help scrolls with `↑` / `↓` (`k` / `j`) and closes with `Esc`, `?` or `q`.
