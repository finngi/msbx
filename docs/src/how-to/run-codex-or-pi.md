# Run Codex or pi in a project

## Codex

1. Set your OpenAI key on the host before the sandbox is first created. Either export it:

   ```sh
   export OPENAI_API_KEY=sk-...
   ```

   or, on macOS, store it in the Keychain once:

   ```sh
   msbx secret set OPENAI_API_KEY
   ```

   `security` prompts for the value; msbx never reads it. Later runs of `msbx` take the key
   from the Keychain whenever the variable is not set in the shell. `msbx secret rm
   OPENAI_API_KEY` removes it.

2. From the project folder, start Codex:

   ```sh
   msbx agent codex
   ```

   The first run creates `codex-<folder>` and installs Codex in it. Later runs reuse it.

The key never enters the VM. Codex sees a placeholder, and microsandbox swaps in the real key
only on requests to `api.openai.com`. The key is read from your host environment each time the
sandbox starts, so keep `OPENAI_API_KEY` exported in the shell you run msbx from.

If the key was not set when the sandbox was created, msbx prints
`note: OPENAI_API_KEY is not set on the host; not forwarding it to codex-<folder> (export it,
or store it with `msbx secret set OPENAI_API_KEY`)` and the sandbox has no key. Secrets are
part of the sandbox settings, so [remove the sandbox](remove-or-reset.md) and run
`msbx agent codex` again with the key set.

### Sign in with ChatGPT instead

1. Turn on device code sign-in in your ChatGPT security settings.
2. Run `msbx agent codex -- login --device-auth` and enter the code it shows.
3. Run `msbx agent codex` in any project. It starts signed in.

The sign-in file, `~/.codex/auth.json` in the VM (the agent runs as `node`, so
`/home/node/.codex/auth.json`), is a link to the `msbx-login-codex` volume, which every Codex
sandbox mounts at `~/.msbx-login`. A token refresh in one sandbox
is seen by all of them, and the login survives `msb rm`. To sign out everywhere, stop the Codex
sandboxes and run `msbx volume rm msbx-login-codex`.

## pi

```sh
msbx agent pi
```

pi uses models served on your host, so it needs no API key. msbx writes the list of local
model servers into the sandbox each time; see
[Reach Ollama or LM Studio on your Mac](reach-host-models.md).

To use a subscription instead, type `/login` in pi. The login is kept on the `msbx-login-pi`
volume and shared by every pi sandbox, like Codex's.

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
