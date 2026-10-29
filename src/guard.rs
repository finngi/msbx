//! Git guard (RFC 0001): the files in a project that git on the host executes from, how new
//! sandboxes mount them read-only, and a fingerprint of them that msbx compares before and after
//! each agent session and against a stored baseline.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::hostgit;
use crate::paths::{self, Dirs};

/// Where a project's git data lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repo {
    pub(crate) git_dir: PathBuf,
    /// The `.git` file (`gitdir: <path>`) when the project is a linked worktree, a submodule or
    /// uses a separate git dir.
    pub(crate) gitfile: Option<PathBuf>,
}

/// A host path mounted at the same place in the VM so git data cannot be planted: the git dir
/// itself read-write (so it cannot be renamed or replaced), the rest read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Guard {
    pub(crate) path: PathBuf,
    pub(crate) readonly: bool,
}

/// Fingerprint of everything git executes from in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    /// Project-relative path (absolute when outside the project) to its fingerprint.
    files: BTreeMap<String, FileState>,
    /// The repository config file, as shown in warnings.
    config: Option<String>,
    /// Risky settings in that file (see `hostgit::is_risky`), key to its values in file order.
    settings: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    /// BLAKE3 of the contents, hex.
    hash: String,
    /// Git ignores hooks without the executable bit, so flipping it is a change.
    executable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum What {
    New,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    File {
        path: String,
        what: What,
    },
    Setting {
        config: String,
        key: String,
        value: String,
        what: What,
    },
}

impl fmt::Display for What {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::New => "new",
            Self::Removed => "removed",
            Self::Changed => "changed",
        })
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { path, what } => write!(f, "{path} ({what})"),
            Self::Setting {
                config,
                key,
                value,
                what,
            } => write!(f, "{config}: {key} = {value} ({what})"),
        }
    }
}

/// The project's repository, if `.git` is a directory or a `gitdir:` file.
pub(crate) fn repo(project: &Path) -> Option<Repo> {
    let dot_git = project.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;
    if metadata.is_dir() {
        return Some(Repo {
            git_dir: dot_git,
            gitfile: None,
        });
    }
    if !metadata.is_file() {
        return None;
    }
    let target = gitdir_target(&std::fs::read_to_string(&dot_git).ok()?)?;
    Some(Repo {
        git_dir: if target.is_absolute() {
            target
        } else {
            project.join(target)
        },
        gitfile: Some(dot_git),
    })
}

/// The path in a `gitdir: <path>` file; git accepts a trailing newline, CRLF included.
fn gitdir_target(text: &str) -> Option<PathBuf> {
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    (!target.is_empty()).then(|| PathBuf::from(target))
}

/// The mounts that guard a project's git data, parent first. `hooks` and `info` are created when
/// missing so they can be mounted. Empty when there is no repository, or its git dir is outside
/// the project (nothing to guard; git does not work in the VM there either).
pub(crate) fn mounts(project: &Path) -> Result<Vec<Guard>> {
    let Some(repo) = repo(project) else {
        return Ok(Vec::new());
    };
    if !inside(project, &repo.git_dir) {
        return Ok(Vec::new());
    }
    for name in ["hooks", "info"] {
        let dir = repo.git_dir.join(name);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut guards = vec![Guard {
        path: repo.git_dir.clone(),
        readonly: false,
    }];
    let readonly = ["hooks", "info", "config", "config.worktree"]
        .into_iter()
        .map(|name| repo.git_dir.join(name))
        .chain(repo.gitfile.clone())
        .chain(config_targets(project, &repo))
        .filter(|path| path.exists() && inside(project, path));
    for path in readonly {
        if !guards.iter().any(|guard| same_path(&guard.path, &path)) {
            guards.push(Guard {
                path,
                readonly: true,
            });
        }
    }
    Ok(guards)
}

/// Paths the repository config points git at: the hooks directory and included config files.
/// A relative `core.hooksPath` is relative to the working tree, a relative `include.path` to the
/// config file. `~` paths are the user's own, outside the project.
fn config_targets(project: &Path, repo: &Repo) -> Vec<PathBuf> {
    let Ok(settings) = hostgit::risky_settings(&repo.git_dir.join("config")) else {
        return Vec::new();
    };
    settings
        .iter()
        .flat_map(|(key, values)| values.iter().map(move |value| (key, value)))
        .filter_map(|(key, value)| {
            let base = if key == "core.hookspath" {
                project
            } else if key == "include.path" || key.starts_with("includeif.") {
                &repo.git_dir
            } else {
                return None;
            };
            (!value.starts_with('~')).then(|| normalize(&base.join(value)))
        })
        .collect()
}

/// Resolves `.` and `..` by name, so `.git/../x` reads as `x`; symlinks are left alone.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn inside(project: &Path, path: &Path) -> bool {
    match (project.canonicalize(), path.canonicalize()) {
        (Ok(project), Ok(path)) => path.starts_with(project),
        _ => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Fingerprints the project's git files. `previous` saves re-reading the config with git when
/// the file has not changed.
pub(crate) fn snapshot(project: &Path, previous: Option<&Snapshot>) -> Result<Snapshot> {
    let Some(repo) = repo(project) else {
        return Ok(Snapshot::default());
    };
    let mut files = BTreeMap::new();
    let mut hooks_dirs = vec![repo.git_dir.join("hooks")];
    let mut singles = vec![
        repo.git_dir.join("config"),
        repo.git_dir.join("config.worktree"),
        repo.git_dir.join("commondir"),
        repo.git_dir.join("info").join("attributes"),
    ];
    singles.extend(repo.gitfile.clone());
    for target in config_targets(project, &repo) {
        if target.is_dir() {
            hooks_dirs.push(target);
        } else {
            singles.push(target);
        }
    }
    for dir in hooks_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            singles.push(
                entry
                    .with_context(|| format!("reading {}", dir.display()))?
                    .path(),
            );
        }
    }
    for path in singles {
        if let Some(state) = file_state(&path)? {
            files.insert(relative(project, &path), state);
        }
    }
    let config_path = repo.git_dir.join("config");
    let config = relative(project, &config_path);
    let has_config = files.contains_key(&config);
    let settings = match previous {
        Some(previous) if previous.files.get(&config) == files.get(&config) => {
            previous.settings.clone()
        }
        _ if has_config => hostgit::risky_settings(&config_path)
            .ok()
            .unwrap_or_default(),
        _ => BTreeMap::new(),
    };
    Ok(Snapshot {
        files,
        config: has_config.then_some(config),
        settings,
    })
}

/// `None` for anything that is not a regular file (a missing path, a directory, a symlink).
fn file_state(path: &Path) -> Result<Option<FileState>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(FileState {
        hash: blake3::hash(&bytes).to_hex().to_string(),
        executable: executable(&metadata),
    }))
}

#[cfg(unix)]
fn executable(metadata: &std::fs::Metadata) -> bool {
    std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

fn relative(project: &Path, path: &Path) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// What changed between two fingerprints: files in path order, then settings in key order.
pub(crate) fn changes(before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();
    let paths: BTreeSet<&String> = before.files.keys().chain(after.files.keys()).collect();
    for path in paths {
        let what = match (before.files.get(path), after.files.get(path)) {
            (None, Some(_)) => What::New,
            (Some(_), None) => What::Removed,
            (Some(old), Some(new)) if old != new => What::Changed,
            _ => continue,
        };
        changes.push(Change::File {
            path: path.clone(),
            what,
        });
    }
    let config = after
        .config
        .clone()
        .or_else(|| before.config.clone())
        .unwrap_or_else(|| ".git/config".to_owned());
    let keys: BTreeSet<&String> = before
        .settings
        .keys()
        .chain(after.settings.keys())
        .collect();
    for key in keys {
        let (values, what) = match (before.settings.get(key), after.settings.get(key)) {
            (None, Some(new)) => (new, What::New),
            (Some(old), None) => (old, What::Removed),
            (Some(old), Some(new)) if old != new => (new, What::Changed),
            _ => continue,
        };
        changes.push(Change::Setting {
            config: config.clone(),
            key: key.clone(),
            value: values.join(", "),
            what,
        });
    }
    changes
}

/// The warning for a list of changes, with the command that inspects the repository without
/// running anything planted in it.
pub(crate) fn report(project: &Path, when: &str, changes: &[Change]) -> String {
    let listed = changes
        .iter()
        .map(|change| format!("  {change}\n"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        "warning: git files changed {when} in {project}:\n{listed}\
         git runs these on your Mac. Review them before running git in this folder:\n  \
         git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C {project} status",
        project = project.display()
    )
}

fn baseline_file(dirs: &Dirs, sandbox: &str) -> PathBuf {
    dirs.git_guard_dir().join(format!("{sandbox}.json"))
}

fn load_baseline(file: &Path) -> Result<Option<Snapshot>> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("reading {}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", file.display())),
    }
}

fn save_baseline(file: &Path, snapshot: &Snapshot) -> Result<()> {
    paths::write_private(file, &serde_json::to_vec(snapshot)?)
}

/// One agent session: the state when it started, so `finish` can say what it changed.
pub(crate) struct Session {
    project: PathBuf,
    start: Snapshot,
}

impl Session {
    /// Compares the project with the stored baseline (the state when the last session started)
    /// and makes the current state the new baseline. The warning, if any, is for the caller to
    /// print.
    pub(crate) fn start(
        dirs: &Dirs,
        sandbox: &str,
        project: &Path,
    ) -> Result<(Self, Option<String>)> {
        let file = baseline_file(dirs, sandbox);
        let baseline = load_baseline(&file)?;
        let start = snapshot(project, baseline.as_ref())?;
        let warning = baseline
            .map(|baseline| changes(&baseline, &start))
            .filter(|changes| !changes.is_empty())
            .map(|changes| report(project, "since the last session started", &changes));
        save_baseline(&file, &start)?;
        Ok((
            Self {
                project: project.to_path_buf(),
                start,
            },
            warning,
        ))
    }

    /// The warning about changes made during the session, if any.
    pub(crate) fn finish(&self) -> Result<Option<String>> {
        let now = snapshot(&self.project, Some(&self.start))?;
        let changes = changes(&self.start, &now);
        Ok((!changes.is_empty()).then(|| report(&self.project, "during this session", &changes)))
    }
}

/// Changes since the last session started, for the dashboard; none before the first session.
pub(crate) fn check(dirs: &Dirs, sandbox: &str, project: &Path) -> Result<Vec<Change>> {
    let Some(baseline) = load_baseline(&baseline_file(dirs, sandbox))? else {
        return Ok(Vec::new());
    };
    let now = snapshot(project, Some(&baseline))?;
    Ok(changes(&baseline, &now))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A temporary folder removed when dropped, so a failing assertion leaves nothing behind.
    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Result<Self> {
            let root =
                std::env::temp_dir().join(format!("msbx-guard-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&root)?;
            Ok(Self(root.canonicalize()?))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> Result<()> {
        let output = hostgit::command().current_dir(dir).args(args).output()?;
        anyhow::ensure!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    fn init_repo(tag: &str) -> Result<Temp> {
        let temp = Temp::new(tag)?;
        git(temp.path(), &["init", "-q"])?;
        Ok(temp)
    }

    fn write_hook(project: &Path, name: &str, body: &str, executable: bool) -> Result<()> {
        let hook = project.join(".git").join("hooks").join(name);
        std::fs::write(&hook, body)?;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(mode))?;
        Ok(())
    }

    fn rendered(changes: &[Change]) -> Vec<String> {
        changes.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn repo_finds_a_git_directory_or_a_gitdir_file() -> Result<()> {
        let temp = init_repo("repo")?;
        let project = temp.path();
        assert_eq!(
            repo(project),
            Some(Repo {
                git_dir: project.join(".git"),
                gitfile: None,
            })
        );
        let plain = Temp::new("plain")?;
        assert_eq!(repo(plain.path()), None);

        let linked = Temp::new("linked")?;
        let gitfile = linked.path().join(".git");
        std::fs::write(&gitfile, "gitdir: ../elsewhere/.git/worktrees/x\r\n")?;
        assert_eq!(
            repo(linked.path()),
            Some(Repo {
                git_dir: linked.path().join("../elsewhere/.git/worktrees/x"),
                gitfile: Some(gitfile.clone()),
            })
        );
        std::fs::write(&gitfile, "gitdir: /abs/repo\n")?;
        assert_eq!(
            repo(linked.path()).map(|repo| repo.git_dir),
            Some(PathBuf::from("/abs/repo"))
        );
        std::fs::write(&gitfile, "not a pointer\n")?;
        assert_eq!(repo(linked.path()), None);
        Ok(())
    }

    #[test]
    fn mounts_guard_the_git_dir_and_what_its_config_points_at() -> Result<()> {
        let temp = init_repo("mounts")?;
        let project = temp.path();
        let git_dir = project.join(".git");
        std::fs::remove_dir_all(git_dir.join("info"))?;
        let husky = project.join(".husky").join("_");
        std::fs::create_dir_all(&husky)?;
        std::fs::write(git_dir.join("local.inc"), "[user]\n\tname = x\n")?;
        std::fs::write(project.join("outside.inc"), "")?;
        git(project, &["config", "core.hooksPath", ".husky/_"])?;
        git(project, &["config", "include.path", "local.inc"])?;
        git(
            project,
            &["config", "--add", "include.path", "~/.gitconfig-elsewhere"],
        )?;
        git(
            project,
            &["config", "--add", "include.path", "/nonexistent/x"],
        )?;
        git(
            project,
            &["config", "--add", "include.path", "../outside.inc"],
        )?;

        let guards = mounts(project)?;
        let listed: Vec<(String, bool)> = guards
            .iter()
            .map(|guard| (relative(project, &guard.path), guard.readonly))
            .collect();
        assert_eq!(
            listed,
            [
                (".git".to_owned(), false),
                (".git/hooks".to_owned(), true),
                (".git/info".to_owned(), true),
                (".git/config".to_owned(), true),
                (".husky/_".to_owned(), true),
                (".git/local.inc".to_owned(), true),
                ("outside.inc".to_owned(), true),
            ]
        );
        assert!(
            git_dir.join("info").is_dir(),
            "info is created for the mount"
        );
        Ok(())
    }

    #[test]
    fn mounts_are_empty_without_a_repo_or_with_git_data_elsewhere() -> Result<()> {
        let plain = Temp::new("nomounts")?;
        assert_eq!(mounts(plain.path())?, []);
        let linked = Temp::new("linkedmounts")?;
        std::fs::write(linked.path().join(".git"), "gitdir: /tmp\n")?;
        assert_eq!(mounts(linked.path())?, []);
        Ok(())
    }

    #[test]
    fn changes_list_hooks_settings_and_the_git_pointer() -> Result<()> {
        let temp = init_repo("changes")?;
        let project = temp.path();
        write_hook(project, "pre-push", "#!/bin/sh\n", false)?;
        let before = snapshot(project, None)?;
        assert_eq!(before.config.as_deref(), Some(".git/config"));
        assert!(before.settings.is_empty(), "{:?}", before.settings);
        assert_eq!(changes(&before, &snapshot(project, Some(&before))?), []);

        write_hook(project, "pre-commit", "#!/bin/sh\nevil\n", true)?;
        write_hook(project, "pre-push", "#!/bin/sh\n", true)?;
        std::fs::remove_file(project.join(".git/hooks/pre-rebase.sample"))?;
        git(project, &["config", "core.fsmonitor", "./tools/watch.sh"])?;
        std::fs::write(project.join(".git/info/attributes"), "* filter=x\n")?;
        let after = snapshot(project, Some(&before))?;
        assert_eq!(
            rendered(&changes(&before, &after)),
            [
                ".git/config (changed)",
                ".git/hooks/pre-commit (new)",
                ".git/hooks/pre-push (changed)",
                ".git/hooks/pre-rebase.sample (removed)",
                ".git/info/attributes (new)",
                ".git/config: core.fsmonitor = ./tools/watch.sh (new)",
            ]
        );

        git(project, &["config", "core.fsmonitor", "false"])?;
        let later = snapshot(project, Some(&after))?;
        assert_eq!(
            rendered(&changes(&after, &later)),
            [
                ".git/config (changed)",
                ".git/config: core.fsmonitor = false (changed)",
            ]
        );
        git(project, &["config", "--unset", "core.fsmonitor"])?;
        let unset = snapshot(project, Some(&later))?;
        assert_eq!(
            rendered(&changes(&later, &unset)),
            [
                ".git/config (changed)",
                ".git/config: core.fsmonitor = false (removed)",
            ]
        );

        // Replacing the directory by a pointer file is the Pillar Security bypass.
        std::fs::rename(project.join(".git"), project.join(".git-alt"))?;
        std::fs::write(project.join(".git"), "gitdir: .git-alt\n")?;
        let pointed = snapshot(project, Some(&unset))?;
        let listed = rendered(&changes(&unset, &pointed));
        assert!(listed.contains(&".git (new)".to_owned()), "{listed:?}");
        assert!(
            listed.contains(&".git/hooks/pre-commit (removed)".to_owned()),
            "{listed:?}"
        );
        assert!(
            listed.contains(&".git-alt/hooks/pre-commit (new)".to_owned()),
            "{listed:?}"
        );
        Ok(())
    }

    #[test]
    fn snapshot_follows_hooks_path_and_includes_inside_the_project() -> Result<()> {
        let temp = init_repo("hookspath")?;
        let project = temp.path();
        let husky = project.join(".husky").join("_");
        std::fs::create_dir_all(&husky)?;
        git(project, &["config", "core.hooksPath", ".husky/_"])?;
        git(project, &["config", "include.path", "../.gitconfig.local"])?;
        std::fs::write(project.join(".gitconfig.local"), "")?;
        let before = snapshot(project, None)?;
        std::fs::write(husky.join("pre-commit"), "evil")?;
        std::fs::write(project.join(".gitconfig.local"), "[core]\n\tpager = evil\n")?;
        let after = snapshot(project, Some(&before))?;
        assert_eq!(
            rendered(&changes(&before, &after)),
            [".gitconfig.local (changed)", ".husky/_/pre-commit (new)"]
        );
        Ok(())
    }

    #[test]
    fn no_repo_means_an_empty_snapshot() -> Result<()> {
        let plain = Temp::new("empty")?;
        assert_eq!(snapshot(plain.path(), None)?, Snapshot::default());
        Ok(())
    }

    #[test]
    fn report_lists_changes_and_the_safe_command() {
        let changes = [
            Change::File {
                path: ".git/hooks/pre-commit".to_owned(),
                what: What::New,
            },
            Change::Setting {
                config: ".git/config".to_owned(),
                key: "core.fsmonitor".to_owned(),
                value: "./tools/watch.sh".to_owned(),
                what: What::New,
            },
        ];
        assert_eq!(
            report(
                Path::new("/Users/you/dev/app"),
                "during this session",
                &changes
            ),
            "warning: git files changed during this session in /Users/you/dev/app:\n  \
             .git/hooks/pre-commit (new)\n  \
             .git/config: core.fsmonitor = ./tools/watch.sh (new)\n\
             git runs these on your Mac. Review them before running git in this folder:\n  \
             git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /Users/you/dev/app status"
        );
    }

    #[test]
    fn sessions_warn_at_start_against_the_baseline_and_at_finish() -> Result<()> {
        let temp = init_repo("session")?;
        let project = temp.path();
        let state = Temp::new("state")?;
        let dirs = Dirs {
            config: state.path().join("config"),
            state: state.path().join("state"),
        };
        assert_eq!(check(&dirs, "box", project)?, [], "no baseline yet");

        let (first, warning) = Session::start(&dirs, "box", project)?;
        assert_eq!(
            warning, None,
            "the first session has nothing to compare with"
        );
        assert_eq!(first.finish()?, None);
        write_hook(project, "pre-commit", "evil", true)?;
        let during = first.finish()?.context("a planted hook is reported")?;
        assert!(during.starts_with("warning: git files changed during this session in"));
        assert!(
            during.contains("\n  .git/hooks/pre-commit (new)\n"),
            "{during}"
        );

        // The dashboard sees the same change until the next session starts.
        assert_eq!(
            rendered(&check(&dirs, "box", project)?),
            [".git/hooks/pre-commit (new)"]
        );
        let (second, warning) = Session::start(&dirs, "box", project)?;
        let warning = warning.context("the next launch warns too")?;
        assert!(
            warning.contains("since the last session started"),
            "{warning}"
        );
        assert_eq!(check(&dirs, "box", project)?, []);
        assert_eq!(second.finish()?, None);

        let stored = std::fs::read_to_string(dirs.git_guard_dir().join("box.json"))?;
        assert!(stored.contains("pre-commit"), "{stored}");
        Ok(())
    }
}
