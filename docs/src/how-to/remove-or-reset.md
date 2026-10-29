# Remove or reset a sandbox

msbx has no remove command of its own: `msbx rm`, `msbx stop` and the rest are passed straight
to `msb`. Removing a sandbox deletes the VM and anything installed in it. Your project folder
is on the host and is not touched.

## Remove one sandbox

```sh
msbx ls                       # find the name, e.g. codex-myapp
msbx rm -f codex-myapp        # -f stops it first if it is running
```

Or in the dashboard: select it, press `r`, then `y`.

## Reset a sandbox

Sandbox settings (image, CPUs, memory, mounts, network, secrets) are fixed when the sandbox is
created. To pick up new defaults, a newly set API key, or a clean install of the agent, remove
the sandbox and create it again:

```sh
msbx rm -f codex-myapp
msbx agent codex              # from the project folder
```

## Remove every sandbox for one agent

msbx labels its sandboxes with `msbx.agent` and `msbx.project`, so you can select them by label:

```sh
msbx rm -f --label msbx.agent=codex
```

## Stop without removing

```sh
msbx stop codex-myapp
```

The sandbox keeps its installed agent; `msbx agent codex` starts it again.
