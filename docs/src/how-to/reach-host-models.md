# Reach Ollama or LM Studio on your Mac

msbx sandboxes can reach services on the host at `host.microsandbox.internal`. microsandbox
routes that name to your host's `localhost`, so a model server listening only on `127.0.0.1`
is reachable.

## pi: no setup needed

Each time `msbx agent pi` starts, msbx writes `/root/.pi/agent/models.json` in the VM:

1. If you have `~/.pi/agent/models.json` on the host (or `$PI_CODING_AGENT_DIR/models.json`),
   msbx copies the providers whose `baseUrl` host is `127.0.0.1`, `localhost`, `[::1]` or
   `0.0.0.0`, with that host replaced by `host.microsandbox.internal`. Every other provider is
   left out, so a cloud API key in that file never enters the VM.
2. Otherwise msbx asks `127.0.0.1:11434` (Ollama) and `127.0.0.1:1234` (LM Studio) for their
   models and writes a provider for each server that answers.
3. If neither finds a server, msbx writes nothing and prints a `note:`.

pi reads the file when you open `/model`. Start the model server before `msbx agent pi`, or
run `msbx agent pi` again after starting it.

## Ollama

1. On the host, start Ollama (the app, or `ollama serve`). It listens on port 11434.
2. In the sandbox, check it answers:

   ```sh
   msbx agent shell
   curl http://host.microsandbox.internal:11434/api/tags
   ```

3. Point your agent at `http://host.microsandbox.internal:11434` wherever it would otherwise use
   `http://localhost:11434`.

## LM Studio

1. On the host, start the LM Studio server (in the app, or `lms server start`). Its examples use
   port 1234; use the port LM Studio shows if you changed it.
2. In the sandbox:

   ```sh
   curl http://host.microsandbox.internal:1234/v1/models
   ```

3. Use `http://host.microsandbox.internal:1234/v1` as the agent's OpenAI-compatible base URL.

## If it does not connect

- The sandbox must be allowed to reach the host port. Sandboxes created by msbx may reach the
  host ports in `policy.toml` (by default 11434 and 1234) and nothing else there. For a server
  on another port, add it with `msbx policy host-ports 11434 1234 8080` (or `Ctrl-S` in the
  dashboard), then recreate the sandbox: `i` on its card shows the ports it was created with.
  Sandboxes created before the network policy existed have the whole `host` profile
  (`network  public + host` on the Overview tab).
- `localhost` inside the VM is the VM itself. Use `host.microsandbox.internal` to reach your Mac.

Host access is fixed when the sandbox is created; the network policy does not change it. See
[Control network access](control-network-access.md) and the
[security model](../explanation/security-model.md).
