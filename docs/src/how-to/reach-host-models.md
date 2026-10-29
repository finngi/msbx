# Reach Ollama or LM Studio on your Mac

msbx sandboxes can reach services on the host at `host.microsandbox.internal`. microsandbox
routes that name to your host's `localhost`, so a model server listening only on `127.0.0.1`
is reachable.

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

- The sandbox must have the `host` network profile. Sandboxes created by msbx do; check with
  `msbx inspect <name>` or the dashboard's Overview tab (`network  public + host`).
- `localhost` inside the VM is the VM itself, not your Mac.

Any service listening on your host is reachable this way, not only model servers. See the
[security model](../explanation/security-model.md).
