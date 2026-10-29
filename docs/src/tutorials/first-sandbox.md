# Your first sandbox

In this tutorial you install microsandbox and msbx, open a shell in a sandbox for a project, and
then manage that sandbox from the dashboard. It takes about ten minutes, most of it the first
image download.

You need macOS on Apple Silicon or Linux with KVM, Homebrew, and Rust (1.88 or newer).

## 1. Install microsandbox and msbx

```sh
brew install superradcompany/tap/microsandbox
msb --version          # msb 0.7.11
cargo install --git https://github.com/finngi/msbx
msbx --version
```

## 2. Make a practice project

Use a throwaway folder, because the sandbox can write to everything in it.

```sh
mkdir -p ~/msbx-tutorial
cd ~/msbx-tutorial
echo "hello from the host" > note.txt
```

## 3. Open a shell in a sandbox

```sh
msbx agent shell
```

The very first run asks which network policy new sandboxes get; press Enter for Balanced
(block by default, with package registries and AI APIs allowed). Then msbx creates a sandbox
named `shell-msbx-tutorial`, boots it, and installs a few packages (`ca-certificates`, `git`,
`curl`). You will see:

```text
Preset [1-3, default 2]:
installing shell in shell-msbx-tutorial (first run only)...
```

You are now in `bash` inside the microVM, in the same folder path as on the host:

```sh
pwd                    # /Users/you/msbx-tutorial (or /home/you/... on Linux)
cat note.txt           # hello from the host
echo "hello from the VM" > vm.txt
uname -a               # a Linux kernel, separate from your host's
exit
```

Back on the host, `cat vm.txt` prints `hello from the VM`. The project folder is shared with the
VM, and it is the only part of the host the VM can see.

The sandbox keeps running after you exit. Run `msbx agent shell` again and it reconnects in a
second, with no install step.

## 4. Use the dashboard

From the same folder, run:

```sh
msbx
```

1. `shell-msbx-tutorial` is on the left with live CPU and memory. Press `Tab` to see its Logs,
   `Tab` again for the Network tab (what the sandbox reached and what was blocked), and once
   more for the Overview.
2. Press `Enter` to attach. You are back in `bash` in the VM. Type `exit` and the dashboard
   returns, showing `exited shell-msbx-tutorial (code 0)`.
3. Press `s` to stop the sandbox. Its status changes to stopped. Press `s` again to start it.
4. Press `r`, then `y`, to remove it.
5. Press `q` to quit.

## What you learned

- `msbx agent <agent>` creates a sandbox for the current folder the first time and reuses it
  after that.
- Only the project folder is shared with the VM.
- The dashboard attaches, starts, stops and removes sandboxes.

Next: [run Codex or pi in a project](../how-to/run-codex-or-pi.md), and read the
[security model](../explanation/security-model.md) before you point an agent at real work.
