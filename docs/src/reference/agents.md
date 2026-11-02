# Agents

`msbx agent <AGENT>` and the dashboard's create wizard know the same agents as Docker Sandboxes
(`sbx run --help`), with the same names, plus pi. Each one is installed inside the VM when the
sandbox is first used, from the vendor's own npm package, release or install script; msbx never
ships an agent binary itself; each agent's own licence applies.

| Agent | Runs | Installed from | Version checked | Skip-prompts flag |
|---|---|---|---|---|
| `claude` | `claude` | npm `@anthropic-ai/claude-code@2.1.314` | 2.1.314 | `--dangerously-skip-permissions` |
| `codex` | `codex` | npm `@openai/codex@0.166.0` | 0.166.0 | `--dangerously-bypass-approvals-and-sandbox` |
| `copilot` | `copilot` | npm `@github/copilot@1.0.105` | 1.0.105 | `--yolo` |
| `cursor` | `cursor-agent` | `https://cursor.com/install`, as the agent user | 2026.10.22-27dcf5f | `--yolo` |
| `devin` | `devin` | `https://cli.devin.ai/install.sh`, as the agent user | 3000.11.234 | `--permission-mode dangerous --respect-workspace-trust=false` |
| `docker-agent` | `docker-agent run` | GitHub release v1.154.0, SHA-256 checked | v1.154.0 | `--yolo` |
| `droid` | `droid` | npm `droid@1.0.0` | 1.0.0 | `--auto high` |
| `gemini` | `gemini` | npm `@google/gemini-cli@0.66.0` | 0.66.0 | `--approval-mode=yolo` |
| `kiro` | `kiro-cli chat` | `https://cli.kiro.dev/install`, as the agent user | 2.37.1 | `--trust-all-tools` |
| `opencode` | `opencode` | npm `opencode-ai@1.19.5` | 1.19.5 | `--auto` |
| `pi` | `pi` | npm `@earendil-works/pi-coding-agent@1.4.0` | 1.4.0 | none (pi does not ask) |
| `shell` | `bash` | nothing | | none |

"Version checked" is what `<command> --version` printed in a `node:24-bookworm` microVM on
2026-10-30 to 2026-11-02, installed by msbx's own install scripts as the agent would run them.
The npm packages and docker-agent are pinned to that version. The three vendor install scripts
always install the vendor's current release, so `cursor`, `devin` and `kiro` may be newer.

## Launching

msbx runs `<command> <defaults> <your arguments>` with `msb exec -t`. Like `sbx run`:

- With no arguments, or arguments that start with `-`, the defaults come first: the fixed
  subcommand (`chat` for kiro, `run` for docker-agent), the skip-prompts flag above, then the
  agent's `args` from `config.toml`. `msbx agent claude -- -c` runs
  `claude --dangerously-skip-permissions -c`.
- Arguments that start with a word, such as a subcommand, replace the defaults:
  `msbx agent codex -- login --device-auth` runs `codex login --device-auth`.

Each agent starts with its permission prompts off by default, as in Docker Sandboxes. The VM,
the network policy, the git guard and host-side secrets are the boundary instead (see the
[security model](../explanation/security-model.md)). To keep an agent's prompts on, set
`auto_approve = false` for it in [`config.toml`](sandbox-defaults.md#config-file):

```toml
[agents.claude]
auto_approve = false
```

`droid` has no flag that skips every prompt in interactive mode; `--auto high` is its most
autonomous level.

## Credentials

An agent gets the host variables in the second column, when they are set, as microsandbox
secrets: the VM sees a placeholder, and the real value is sent only to the hosts listed. Any one
of an agent's variables is enough. `msbx secret set <VAR>` stores any of them in the macOS
Keychain or, on Linux, the Secret Service.

| Agent | Variables | Sent only to | Shared sign-in files (in the agent user's home) |
|---|---|---|---|
| `claude` | `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` | `api.anthropic.com` | `.claude/.credentials.json`, `.claude.json` |
| `codex` | `OPENAI_API_KEY` | `api.openai.com` | `.codex/auth.json` |
| `copilot` | `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` | `api.github.com`, `api.githubcopilot.com`, `api.individual.githubcopilot.com`, `api.business.githubcopilot.com`, `api.enterprise.githubcopilot.com` | `.copilot/config.json` |
| `cursor` | `CURSOR_API_KEY` | `api2.cursor.sh`, `api3.cursor.sh`, `repo42.cursor.sh` | `.config/cursor/auth.json` |
| `devin` | none | | `.local/share/devin/credentials.toml` |
| `docker-agent` | `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GOOGLE_API_KEY` | as for the other agents | none |
| `droid` | `FACTORY_API_KEY` | `api.factory.ai`, `app.factory.ai`, `relay.factory.ai` | `.factory/auth.v2.file`, `.factory/auth.v2.key` |
| `gemini` | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | `generativelanguage.googleapis.com` (and `aiplatform.googleapis.com` for `GOOGLE_API_KEY`) | `.gemini/oauth_creds.json` |
| `kiro` | none | | none: sign in inside each sandbox |
| `opencode` | `OPENCODE_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | `opencode.ai`; the others as above | `.local/share/opencode/auth.json` |
| `pi` | none | | `.pi/agent/auth.json` |
| `shell` | none | | none |

Hosts follow the vendor's documented API host where there is one, otherwise Docker Sandboxes'
[built-in secret list](https://docs.docker.com/ai/sandboxes/configuration/credentials/). Copilot
reads its variables in the order shown and does not accept classic `ghp_` tokens.

Shared sign-in files are links to the `msbx-login-<agent>` volume
([RFC 0004](../rfcs/0004-shared-logins.md)), so you sign in once per agent and every sandbox of
that agent starts signed in. Where they come from:

- claude: Claude Code's setup docs (`~/.claude/.credentials.json` on Linux, account in
  `~/.claude.json`).
- codex, opencode: the vendors' docs; opencode's path also seen in a trace in the VM.
- copilot: GitHub's docs: without a system keychain the token is saved in plain text in
  `~/.copilot/config.json`.
- cursor: traced in the VM (`cursor-agent status` reads `$XDG_CONFIG_HOME/cursor/auth.json`,
  which the CLI writes in place).
- devin: `devin auth status` names the file.
- droid: strings in the droid 1.0.0 binary (`auth.v2.file` needs `auth.v2.key` beside it). Not
  exercised with a real sign-in.
- gemini: gemini-cli's source (`~/.gemini/oauth_creds.json`).
- kiro: keeps its sign-in in a SQLite database with other state
  (`~/.local/share/kiro-cli/data.sqlite3`), which msbx does not share; sign in again in each
  new sandbox (`msbx agent kiro -- login --use-device-flow`).

## Network

The `balanced` preset allows every host above, plus what the installs and sign-ins reach:
`registry.npmjs.org` and `nodejs.org`; Claude sign-in (`claude.ai`, `platform.claude.com`,
`console.anthropic.com`); Codex sign-in (`auth.openai.com`, `chatgpt.com`); Gemini's Google
sign-in (`accounts.google.com`, `oauth2.googleapis.com`, `cloudcode-pa.googleapis.com`);
`**.githubcopilot.com` and GitHub; Cursor (`cursor.com`, `downloads.cursor.com`, `**.cursor.sh`);
Devin (`cli.devin.ai`, `static.devin.ai`, `app.devin.ai`, `api.devin.ai`); Kiro (`cli.kiro.dev`,
`prod.download.cli.kiro.dev`); Droid's sign-in provider (`api.workos.com`, `auth.workos.com`);
and OpenCode's model list (`models.dev`). Kiro's sign-in hosts and Devin's model hosts are not
documented: if a sign-in stalls, `msbx policy log <sandbox> --blocked` shows what to allow.

## Local models

pi is set up for Ollama and LM Studio on your Mac automatically
([Reach Ollama or LM Studio](../how-to/reach-host-models.md)). Other agents can use them too, but
msbx does not configure them yet; point them at `host.microsandbox.internal` yourself.
