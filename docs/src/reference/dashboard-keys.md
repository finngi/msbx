# Dashboard keys

Run `msbx` to open the dashboard. Sandboxes are listed as cards on the left; the last card is
`+ new sandbox`. The right pane shows the selected sandbox's Overview, Logs or Network tab.

## Main view

| Key | Action |
|---|---|
| `↑` / `k` | Select previous card |
| `↓` / `j` | Select next card |
| `Tab` | Switch tab (Overview / Logs / Network) |
| `Enter` | Attach: run the sandbox's agent. For a sandbox msbx did not create, open a shell. On `+ new sandbox`, open the create popup. |
| `x` | Open a shell (`bash`) in the sandbox |
| `c` | Open the create popup (the preset popup first, when there is no policy yet) |
| `s` | Start a stopped or crashed sandbox; stop any other |
| `r` | Remove the sandbox (asks to confirm) |
| `p` | Choose the network preset |
| `?` | Show the key help |
| `q` / `Ctrl-C` | Quit |

Attaching hands the terminal to `msb exec -t`; the dashboard returns when you exit and shows the
exit code. Stopping waits up to 30 seconds for a graceful shutdown and never kills the VM.

## Network tab

The header reads `policy <preset> · ↓ rx ↑ tx · N blocked`. Rows are the proxy's log for the
selected sandbox, blocked first, newest first: `✗` or `✓`, `host:port`, count, last seen
(local `HH:MM:SS`). The log is re-read every 2 seconds.

| Key | Action |
|---|---|
| `←` / `h` | Previous row |
| `→` / `l` | Next row |
| `a` | Allow the row's host for this sandbox |
| `A` | Allow the row's host everywhere |
| `d` | Deny the row's host for this sandbox |
| `p` | Choose the network preset |

Each key writes `policy.toml`; the proxy applies it to the next connection. A sandbox created
before network policy shows `created before network policy; recreate with msb rm -f NAME`, and
`a`, `A` and `d` do nothing there.

## Create popup

Creates the selected agent's sandbox for the folder you started `msbx` in, then attaches, like
`msbx agent <agent>`.

| Key | Action |
|---|---|
| `←` / `h` | Previous agent |
| `→` / `l` | Next agent |
| `Enter` | Create and attach |
| `Esc` | Cancel |

## Preset popup

Opened by `p`, or before the first create when `policy.toml` does not exist yet.

| Key | Action |
|---|---|
| `↑` / `↓` / `←` / `→` | Choose Open, Balanced or Locked Down |
| `Enter` | Write the preset (then open the create popup, on first run) |
| `Esc` | Cancel |

## Remove popup

| Key | Action |
|---|---|
| `y` | Remove |
| `n` / `Esc` | Cancel |

## Help popup

`?`, `q` or `Esc` closes it.

`Ctrl-C` quits from any view, including popups.
