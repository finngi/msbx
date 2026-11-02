# Troubleshoot

## The agent cannot reach anything after your Mac changed network

After you switch Wi-Fi, or wake your Mac on another network, a sandbox that was already running
may stop resolving host names. The agent then fails with errors such as `Resolving timed out`
or `Could not resolve host`, while the same hosts work on your Mac.

### Why it happens

Each sandbox looks up names through microsandbox's DNS forwarder on the host. The
forwarder reads your Mac's DNS servers once, when the sandbox starts, and keeps using them. On a
new network those servers are usually unreachable (a phone hotspot's `172.20.10.1`, an office
router), so every lookup in the sandbox times out. The msbx proxy is not involved.

### What msbx does about it

Before `msbx agent` or the dashboard's Enter attaches to a running sandbox,
msbx looks up `example.com` inside it, with a 5-second limit. If that fails while your Mac can
resolve the same name, msbx prints

```text
note: codex-myapp cannot look up host names since your Mac changed network; restarting it
```

and restarts the sandbox, which reads your current DNS servers. Restarting ends anything else
still running in that sandbox, such as a shell in another terminal. Files in the project folder
and in the VM are kept. If your Mac is offline too, msbx leaves the sandbox alone.

### Fix it by hand

msbx checks only when it attaches an agent, not when you open a shell (`x` in the dashboard, or
`msb exec`). To restart the sandbox yourself:

```sh
msbx stop codex-myapp
msbx agent -d codex /path/to/myapp
```

Start it again through `msbx`: it passes the proxy token the sandbox needs to reach the internet,
which `msb start` does not.

### Check DNS in a sandbox

From your Mac:

```sh
msb exec codex-myapp -- getent hosts example.com
```

An address means DNS in the sandbox works. A long wait followed by nothing means it is stale.
