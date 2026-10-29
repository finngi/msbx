# Run Codex or pi in a project

## Codex

1. Set your OpenAI key on the host before the sandbox is first created:

   ```sh
   export OPENAI_API_KEY=sk-...
   ```

2. From the project folder, start Codex:

   ```sh
   msbx agent codex
   ```

   The first run creates `codex-<folder>` and installs Codex in it. Later runs reuse it.

The key never enters the VM. Codex sees a placeholder, and microsandbox swaps in the real key
only on requests to `api.openai.com`. The key is read from your host environment each time the
sandbox starts, so keep `OPENAI_API_KEY` exported in the shell you run msbx from.

If the key was not set when the sandbox was created, msbx prints
`note: OPENAI_API_KEY is not set on the host; not forwarding it to codex-<folder>` and the
sandbox has no key. Secrets are part of the sandbox settings, so
[remove the sandbox](remove-or-reset.md) and run `msbx agent codex` again with the key set.

## pi

```sh
msbx agent pi
```

pi uses models served on your host, so it needs no API key. See
[Reach Ollama or LM Studio on your Mac](reach-host-models.md).

## Another project folder

Pass the folder path as an argument:

```sh
msbx agent codex ~/dev/myapp
```

## Pass arguments to the agent

Put them after `--`:

```sh
msbx agent codex -- --version
```

## Two folders with the same name

Sandboxes are named `<agent>-<folder>`, so `~/work/api` and `~/oss/api` both map to `codex-api`.
msbx refuses to reuse a sandbox created for a different path:

```text
error: sandbox codex-api already exists for /Users/you/work/api; remove it with `msb rm -f codex-api` or run msbx from that folder
```
