# RFC 0002: msbx config file

- Status: Proposed
- Tracking issue: #TBD

## Summary

msbx supports a single optional settings file, `~/.config/msbx/config.toml`. It stores settings you would otherwise have to pass on every launch. If you don't have the file, msbx works like it does now.

## Motivation

Currently, msbx's defaults are hard-coded. If you want to change the number of CPUs, the amount of memory, network access, login sharing or the setup script, you need a flag on every launch, or a code change. Several planned features each need a setting, and those settings should live in one place.

People also work on different projects with different requirements, such as a client repo with its own GitHub token, or a heavier build that needs more memory. One small user-level file covers all of this, and it never holds a secret value.

## Guide-level explanation

- Set CPUs, memory, network access, login sharing and a setup script once. You can set them for all agents, for one agent, or for one project folder.
- Name the environment variable that holds your GitHub token. The file never contains secret values, only variable names.
- If the file has a typo or an unknown key, msbx stops before it starts anything and shows the line and column.

## Reference-level explanation

msbx looks for the file in one place:

- `$XDG_CONFIG_HOME/msbx/config.toml` if `XDG_CONFIG_HOME` is set
- otherwise `~/.config/msbx/config.toml`

It's the same path on macOS and Linux. If the file doesn't exist, msbx uses its built-in defaults.

When several places set the same key, the more specific one wins:

```mermaid
flowchart LR
  A[Built-in defaults] --> B["[defaults]"] --> C["[agents.&lt;id&gt;]"] --> D["[projects.&quot;&lt;path&gt;&quot;]"] --> E[Command-line flags]
```

Read it from left to right: each step takes precedence over the previous step.

Project tables use the absolute path of the project folder, as msbx sees it (with symlinks resolved). For example, `[projects."/Users/you/dev/api"]`.

Settings are applied when msbx creates the sandbox. An existing sandbox keeps the settings it was created with. To make a change take effect, delete the sandbox with `msb rm -f <name>` and launch again.

### Example

```toml
# ~/.config/msbx/config.toml. Every key is optional.

[defaults]                     # all agents, all projects
cpus = 8
memory_mib = 16384
login = "agent"                # "agent", "project" or "off": share agent logins per agent, per project, or not at all
network = "balanced"           # "open" or "balanced"
network_allow = ["pypi.org"]   # extra hosts the sandbox may reach
host = true                    # allow access to services on your Mac (for example Ollama)
setup = ".msbx/setup.sh"       # script run once in a new sandbox, relative to the project; false to skip

[agents.codex]                 # only when running codex
login = "off"

[github]
token_env = "MSBX_GITHUB_TOKEN"  # name of the variable that holds the token, never the token itself

[git]                          # by default msbx copies your git identity from the host
name = "Your Name"
email = "you@example.com"

[projects."/Users/you/dev/acme-api"]
cpus = 4
network = "open"
github = { token_env = "MSBX_GITHUB_TOKEN_ACME" }
```

### Errors

msbx checks the file before it does anything else. It rejects:

- unknown keys (for example `cpu` instead of `cpus`), and lists the valid ones
- `cpus` below 1 or above the number of CPU cores on your machine
- `memory_mib` of 0
- project paths that are not absolute
- a `token_env` that looks like a token (`ghp_…`, `gho_…`, `github_pat_…`) rather than a variable name

Example:

```
error: reading config /Users/you/.config/msbx/config.toml: TOML parse error at line 3, column 1
  |
3 | cpu = 4
  | ^^^
unknown field `cpu`, expected one of `cpus`, `memory_mib`, `login`, ...
```

### Implementation notes

- New module `src/config.rs`:
  - `Config` holds `defaults: Settings`, `agents: BTreeMap<String, Settings>`, `projects: BTreeMap<PathBuf, Settings>`, `github` and `git`.
  - `Settings` holds only `Option` fields, plus `Settings::overlay(self, top) -> Settings` for merging.
- Functions:
  - `path(xdg: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf>`
  - `load(path: Option<&Path>) -> anyhow::Result<Config>`: a missing file means defaults; an invalid file is an error
  - `Config::resolve(&self, agent: &str, project: &Path) -> Settings`
- Use serde derive with `deny_unknown_fields`. New dependencies are `serde` and `toml`. Do not add `etcetera`: the XDG lookup is five lines with one caller.
- `main` loads the config once, before the TUI starts. The resolved settings go into a plain `model::Launch` that is passed to `sandbox::ensure`. Only `sandbox.rs` talks to the SDK.
- Tests:
  - parse errors: unknown key, token given as `token_env`, relative project path, empty file
  - precedence table for `resolve`
  - `path` with and without `XDG_CONFIG_HOME`

## Drawbacks

This design has no separate list of limits. Its costs and trade-offs are covered under [Rationale and alternatives](#rationale-and-alternatives).

## Security

- The file holds no secrets. Keys that refer to secrets take an environment variable name. msbx reads the value from your environment when the sandbox starts, and microsandbox never stores it.
- There is no config file inside the project folder. The agent can write to the project folder, so a config file there could widen the agent's own access the next time its sandbox is created.

## Rationale and alternatives

- **User-level only, not a config file in the repo:** the agent can write to the project folder. A `.msbx/config.toml` there could widen the agent's own network or token access the next time its sandbox is created. Per-project settings go in `[projects."<path>"]` tables in your own file instead. The cost is that a project cannot ship shared settings for its contributors; that can come later with a check that it only narrows access.
- **TOML:** what Rust tools and their users expect (Cargo, uv, rustfmt). It allows comments, and serde gives typed parsing with line and column errors. JSON has no comments, YAML has surprising implicit typing, and a custom format would need its own docs and parser.
- **No `msbx config` command:** msbx passes unknown commands to msb, so a `config` command would shadow a future msb command of the same name, and editing a small file needs no command.
- **No environment variable layer:** command-line flags already override the file. A parallel `MSBX_*` variable for every key doubles what we must document and test. The exception is variables that name secrets, which are references by design.
- **XDG path on macOS too (`~/.config`), not `~/Library/Application Support`:** matches git, gh and uv, and gives users one path to remember. It is a five-line lookup, so no extra crate is needed.
- **Not doing it:** each feature would grow its own flags or environment variables, and users would retype them on every launch.

## Prior art

- [XDG Base Directory spec](https://specifications.freedesktop.org/basedir-spec/latest/): `XDG_CONFIG_HOME`, falling back to `~/.config`.
- [git config scopes](https://git-scm.com/docs/git-config#SCOPES): system, global and local precedence. Git also refuses some settings from repo-local config (`safe.directory`), for the same reason we skip in-repo config.
- [git `includeIf`](https://git-scm.com/docs/git-config#_conditional_includes): per-folder settings kept in the user's own file.
- [Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html): hierarchical, including per-directory `.cargo/config.toml`. That is the in-repo model we chose not to copy.
- [uv configuration files](https://docs.astral.sh/uv/concepts/configuration-files/): `~/.config/uv/uv.toml` on macOS too, with command-line flags taking precedence.
- [GitHub CLI](https://cli.github.com/manual/gh_help_environment): keeps its config in `~/.config/gh/config.yml`.

## Unresolved questions

- Should project tables also match by git remote (for example `github.com/owner/repo`) as well as by path, so a moved checkout keeps its settings?
- Should there be a way to see the merged result, such as `msbx agent codex --print-config`, without adding a `config` command?
- When the file changes, should msbx warn that an existing sandbox was created with different settings, or stay silent until the user recreates it?
- Should a later version allow a narrowing-only project file checked into the repo?
- Should `setup` paths be resolved relative to the project or to the config file?

## Future possibilities

- A config file inside each repository (see Security).
- An `msbx config` command. Edit the file directly. A new command could also clash with a future `msb config` command, and msbx passes unknown commands to msb.
- Overriding settings with environment variables.
- Changing running sandboxes when the file changes.
