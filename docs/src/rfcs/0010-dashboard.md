# RFC 0010: Dashboard focus, creation and network controls

- Status: Implemented
- Tracking issue: none (local dashboard brief)

## Summary

Give the msbx dashboard one pane focus model, a create wizard with an editable name and workspace picker, and network inspection and rule editing for one or all sandboxes. Creation stays in the background. State and rendering stay pure.

## Motivation

The current dashboard moves cards with vertical arrows and network rows with horizontal arrows. Tab changes tabs while the footer calls it a pane key. Creating a sandbox cannot change its name or workspace, and network rows omit the history needed to understand a decision. These dashboard interactions belong in msbx.

## Guide-level explanation

Tab and Shift-Tab switch between Cards and Detail. Left/h focuses Cards; Right/l focuses Detail. Up/k and Down/j move within the focused pane. Brackets `[` and `]` change detail tabs: Overview, Logs, Network and Rules. A title marker and border identify focus. Below 100 columns, the focused pane uses the full width; Tab still switches panes.

`c` opens agent selection with descriptions and credential availability. Enter moves to configuration: edit the generated `<agent>-<folder>` name, choose a workspace, go Back or Create. The workspace picker starts in the launch folder, includes `..`, filters folders as you type and offers Create here to select the folder. Creating returns to the dashboard while a background job works; Enter on its completed card attaches. The CLI keeps its generated naming.

The Network tab shows time, host, port, hits and status. Enter expands the selected endpoint to show sandbox, complete host/port, first and last seen, and allowed/blocked hit counts. `a` allows a blocked endpoint's host for its sandbox, `b` blocks an allowed host, and `A` allows everywhere. These edit future policy; historical counts remain historical. Status represents the latest recorded outcome (blocked wins an equal-time tie).

`n` opens a full network view with a sandbox column. `/` edits search, `o` cycles all/allowed/blocked, and `s` cycles sandboxes (including names only in history or rules). Escape cancels editing, clears search, collapses details, then returns to the dashboard. Rules lists preset, global and sandbox entries. `a` opens a target input with an allow/block choice; `r` confirms removal of an explicit rule, showing its target and scope. Preset rules are read-only; `p` changes the preset. With All sandboxes selected, new rules are global; selecting a sandbox scopes new rules to it.

`?` opens scrollable help derived from the same binding table as the footer. Empty states explain creation, traffic or clearing filters. Nonempty `NO_COLOR` disables colors while text markers preserve focus and status.

## Reference-level explanation

Implementation plan (written before production edits):

1. `app.rs`: Focus, wizard, folder/search/rule drafts; modal-first input; pure actions and identity reconciliation.
2. `input.rs`, `keymap.rs`: bounded Unicode-safe input and a context binding table shared by dispatch/help/footer.
3. `ui.rs`: pure screen functions, adaptive panes, input cursor, table expansion and snapshots.
4. `model.rs`: endpoint counts/times, owned RuleInfo and shared name validation; `agents.rs`: descriptions.
5. `tui.rs`: async credential/folder results, effective rules and policy edits, paste/resize and terminal cleanup.
6. `sandbox.rs`: `ensure_named(agent, project, name, notify)` and read-only `shared_login`; existing `ensure` retains CLI reuse.
7. `main.rs`, `tui_fixture.rs`: debug-only, explicit JSON fixture bypass before secret/proxy/SDK startup, using the actual event/render loop with mutations disabled.
8. Tests first: focus/input isolation, create defaults/validation/back, stale folder results, row identity/filter/scope, name collision and aggregation/rule IO.
9. Snapshots: each new screen populated/empty where meaningful at 100x30, 80x24 and 60x20; style and long-content checks; read every pending file.
10. Verify: rust-expert gate, tics, mdbook and isolated PTY captures of every changed screen and resize/escape/exit.
11. Dependencies/features: none; use current ratatui/crossterm/serde/tokio/insta and Python pyte driver.
12. Exclusions: VM boot, mouse, settings, cloud/local choice, filesystem rules, extra Info popup, user keymap configuration, commits/pushes.

Name validation accepts the same canonical lowercase ASCII letters, digits and single interior hyphens as generated names, with a 128-byte limit. A shared pure validator is called by the wizard and sandbox boundary. This intentionally keeps the narrower msbx naming convention even though microsandbox accepts dots, underscores and uppercase too. The dashboard rejects listed existing names and calls the SDK's atomic `create()` operation, so concurrent creators cannot attach to a winner with different settings. The CLI retains `connect_or_create` and verifies both project and agent labels before reuse. SDK source checked: `microsandbox-0.7.11/lib/sandbox/builder.rs:1669` preserves existing configuration on `connect_or_create`; `microsandbox-types-0.7.11/lib/validation.rs:20` defines the outer name limits.

Credential summaries are produced in IO. Startup already resolves shell/Keychain values into the process environment; report availability from that environment without displaying values. Shared sign-in checks use `Volume::get` and `handle.fs().exists("auth.json")`, without creating a volume or booting a VM (`microsandbox-0.7.11/lib/volume/mod.rs:111,458`, `lib/volume/fs.rs:248`). Distinguish missing from unknown/error. Existence does not prove a token is valid.

The raw log remains unchanged: it keys rows by sandbox, host, port, outcome and deciding rule. The dashboard deliberately aggregates into endpoint rows `(sandbox, host, port)`, summing counts with saturation and keeping earliest/latest timestamps. Selection and expansion track endpoint identity through refreshes. Policy rows come from `Policy::rules`; edits use `allow`, `deny`, `remove`, atomic save and `proxy::reload`, preserving existing precedence.

Folder results carry the requested path so late responses cannot overwrite a newer picker. Filesystem work runs outside the render/update path. Draft names update with agent/folder defaults only until edited. Invalid names/targets stay in their form with an error. The IO loop supplies pure renderer measurements before keys and redraws so scrolling is bounded by wrapped display rows. Native key Release/Repeat events cannot submit; paste inserts filtered text only into an active input.

The fixture path exists only in debug builds. It bypasses real SDK refresh, credentials and proxy startup, refuses lifecycle/policy writes, confines folder reads to its temporary workspace and feeds the normal state/render path. It is limited to verification.

## Drawbacks

Forms, filters and focused panes add state and more screens to test. A full-width focused pane on smaller terminals hides the other pane until navigation. Aggregating history means one row can contain both allowed and blocked hits; expansion exposes those counts and status is explicitly the latest outcome.

## Security

No new VM-to-host channel is introduced. The chosen workspace remains the writable host mount already provided by msbx, so choosing a broader folder broadens what the agent can edit. Credentials are only presence summaries; no token contents enter App, UI, snapshots or reports. Rule dialogs state scope, and removal captures the exact rule identity. Existing deny precedence still applies even after adding an allow. Debug fixture checks cannot create, start, attach to, stop or remove a sandbox, or edit the user's policy.

## Rationale and alternatives

Use brackets for tabs, following [lazygit's separate pane and tab bindings](https://github.com/jesseduffield/lazygit/blob/4e1eebe278790f257a875da6633234391a5a85b3/docs/Config.md#L692-L757). Numbered tabs would also work, as [gitui demonstrates](https://github.com/gitui-org/gitui/blob/d7214eccb3cac52ce3f8d9ca18f218671821eeee/src/keys/key_list.rs#L138-L142), but brackets work consistently for the four detail tabs and two full-network tabs.

Keep the [Ratatui TEA split](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/) and its existing concrete functions. Help and footer share binding data, following [gitui's common commands](https://github.com/gitui-org/gitui/blob/d7214eccb3cac52ce3f8d9ca18f218671821eeee/src/app.rs#L661-L666) and [television's input map](https://github.com/alexpasmantier/television/blob/67ddd3046f125cb8229af3de70c5b1e89af0a694/television/keymap.rs#L12-L90). Keep an explicit named-create entry point so CLI defaults remain unchanged.

Skip `i`: Overview already contains status, uptime, CPU, memory, network policy, traffic, secrets, mounts and image. Add identity/path there if needed.

## Prior art

The supplied Docker sbx v0.50.1 traversal (2026-10-29) motivates the wizard, endpoint expansion, all-sandbox network filters and rule actions. The brief supplies this field evidence; public API behavior was not independently verified. [bottom's table state](https://github.com/ClementTsang/bottom/blob/dbf91d4bfce5b2553870ae984284bdd25729ab4a/src/canvas/components/data_table/state.rs#L16-L100) separates selection from viewport. [Television's PTY harness](https://github.com/alexpasmantier/television/blob/67ddd3046f125cb8229af3de70c5b1e89af0a694/tests/pty/common.rs#L24-L211) complements snapshots with real input. [NO_COLOR](https://no-color.org/) defines nonempty-value behavior.

## Unresolved questions

Native Windows input and real microVM creation, shared login contents and live proxy behavior cannot be exercised in this task. Synthetic event tests and boundary tests cover the planned behavior; the report must keep those limits explicit.

## Future possibilities

Mouse hit testing, richer credential setup and filesystem policy views can be considered separately if msbx gains the corresponding runtime support and a concrete use case.
