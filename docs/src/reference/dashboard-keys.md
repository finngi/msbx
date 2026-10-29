# Dashboard keys

Run `msbx` to open the dashboard. Sandboxes are listed as cards on the left; the last card is
`+ new sandbox`. The right pane shows the selected sandbox's Overview or Logs.

## Main view

| Key | Action |
|---|---|
| `↑` / `k` | Select previous card |
| `↓` / `j` | Select next card |
| `Tab` | Switch tab (Overview / Logs) |
| `Enter` | Attach: run the sandbox's agent. For a sandbox msbx did not create, open a shell. On `+ new sandbox`, open the create popup. |
| `x` | Open a shell (`bash`) in the sandbox |
| `c` | Open the create popup |
| `s` | Start a stopped or crashed sandbox; stop any other |
| `r` | Remove the sandbox (asks to confirm) |
| `?` | Show the key help |
| `q` / `Ctrl-C` | Quit |

Attaching hands the terminal to `msb exec -t`; the dashboard returns when you exit and shows the
exit code. Stopping waits up to 30 seconds for a graceful shutdown and never kills the VM.

## Create popup

Creates the selected agent's sandbox for the folder you started `msbx` in, then attaches, like
`msbx agent <agent>`.

| Key | Action |
|---|---|
| `←` / `h` | Previous agent |
| `→` / `l` | Next agent |
| `Enter` | Create and attach |
| `Esc` | Cancel |

## Remove popup

| Key | Action |
|---|---|
| `y` | Remove |
| `n` / `Esc` | Cancel |

## Help popup

`?`, `q` or `Esc` closes it.

`Ctrl-C` quits from any view, including popups.
