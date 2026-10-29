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

/// Submodules nest (`.git/modules/a/modules/b`); deeper than this is not a layout git makes.
const MAX_MODULE_DEPTH: usize = 32;

/// Where a project's git data lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repo {
    pub(crate) git_dir: PathBuf,
    /// The `.git` file (`gitdir: <path>`) when the project is a linked worktree, a submodule or
    /// uses a separate git dir.
    pub(crate) gitfile: Option<PathBuf>,
}

/// A git directory whose control files git on the host reads: the project's own, or a
/// submodule's under `.git/modules`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlDir {
    git_dir: PathBuf,
    /// The working tree a relative `core.hooksPath` is resolved against.
    worktree: PathBuf,
    /// The `gitdir:` pointer file of that working tree, when it has one.
    gitfile: Option<PathBuf>,
}

/// A host path mounted at the same place in the VM so git data cannot be planted: a git dir
/// itself read-write (so it cannot be renamed or replaced), the rest read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Guard {
    pub(crate) path: PathBuf,
    pub(crate) readonly: bool,
}

/// The guard mounts for a project, and notes about what could not be guarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) guards: Vec<Guard>,
    pub(crate) notes: Vec<String>,
}

/// Fingerprint of everything git executes from in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    /// Project-relative path (absolute when outside the project) to its fingerprint.
    files: BTreeMap<String, FileState>,
    /// Per config file (same keys as `files`), its risky settings (see `hostgit::is_risky`),
    /// key to values in file order.
    settings: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    /// BLAKE3 of the contents, hex. Contents themselves are never stored: a config can hold
    /// credentials.
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

/// Paths, keys and values come from the project, so they are shown with control characters
/// escaped: a planted name must not redraw the terminal.
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { path, what } => write!(f, "{} ({what})", escape(path)),
            Self::Setting {
                config,
                key,
                value,
                what,
            } => write!(
                f,
                "{}: {} = {} ({what})",
                escape(config),
                escape(key),
                escape(value)
            ),
        }
    }
}

/// `text` with control characters (newlines, escapes) written out as `\n`, `\u{1b}` and so on.
pub(crate) fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// The project's repository, if `.git` is a directory or a `gitdir:` file.
pub(crate) fn repo(project: &Path) -> Option<Repo> {
    let dot_git = project.join(".git");
    let git_dir = resolve_marker(&dot_git)?;
    Some(Repo {
        gitfile: (git_dir != dot_git).then_some(dot_git),
        git_dir,
    })
}

/// The git dir a `.git` entry stands for: itself when a directory, the `gitdir:` target when a
/// pointer file (relative to the file's folder).
fn resolve_marker(dot_git: &Path) -> Option<PathBuf> {
    let metadata = std::fs::symlink_metadata(dot_git).ok()?;
    if metadata.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    if !metadata.is_file() {
        return None;
    }
    let target = gitdir_target(&std::fs::read_to_string(dot_git).ok()?)?;
    Some(if target.is_absolute() {
        target
    } else {
        normalize(&dot_git.parent()?.join(target))
    })
}

/// The path in a `gitdir: <path>` file; git accepts a trailing newline, CRLF included.
fn gitdir_target(text: &str) -> Option<PathBuf> {
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    (!target.is_empty()).then(|| PathBuf::from(target))
}

/// The project's git dir and every submodule git dir under its `modules/`.
fn control_dirs(project: &Path, repo: &Repo) -> Vec<ControlDir> {
    let mut dirs = vec![ControlDir {
        git_dir: repo.git_dir.clone(),
        worktree: project.to_path_buf(),
        gitfile: repo.gitfile.clone(),
    }];
    collect_modules(&repo.git_dir.join("modules"), 0, &mut dirs);
    dirs
}

/// Walks `.git/modules`: a folder holding `HEAD` and `config` is a submodule's git dir (its
/// path mirrors the submodule path, so intermediate folders are plain), which may nest its own.
fn collect_modules(dir: &Path, depth: usize, out: &mut Vec<ControlDir>) {
    if depth > MAX_MODULE_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    for child in children {
        if child.join("HEAD").is_file() && child.join("config").is_file() {
            let worktree = module_worktree(&child);
            let gitfile = worktree
                .as_deref()
                .map(|worktree| worktree.join(".git"))
                .filter(|gitfile| gitfile.is_file());
            out.push(ControlDir {
                worktree: worktree.unwrap_or_else(|| child.clone()),
                git_dir: child.clone(),
                gitfile,
            });
            collect_modules(&child.join("modules"), depth + 1, out);
        } else {
            collect_modules(&child, depth + 1, out);
        }
    }
}

/// A submodule git dir records its working tree in `core.worktree`, relative to itself.
fn module_worktree(git_dir: &Path) -> Option<PathBuf> {
    let settings = hostgit::risky_settings(&git_dir.join("config")).ok()?;
    let value = settings.get("core.worktree")?.last()?;
    let path = Path::new(value);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        normalize(&git_dir.join(path))
    })
}

/// The mounts that guard a project's git data, parent first. `hooks` and `info` are created when
/// missing so they can be mounted. Empty, with a note, when the git dir is outside the project
/// (a linked worktree: nothing to guard, and git does not work in the VM there either); empty
/// without a note when there is no repository.
pub(crate) fn mounts(project: &Path) -> Result<Plan> {
    let Some(repo) = repo(project) else {
        return Ok(Plan::default());
    };
    if !inside(project, &repo.git_dir) {
        return Ok(Plan {
            guards: Vec::new(),
            notes: vec![format!(
                "note: the git data of {} is at {}, outside the project (a linked worktree or \
                 submodule checkout): it is not guarded, and git does not work in the VM there",
                project.display(),
                repo.git_dir.display()
            )],
        });
    }
    let mut guards = Vec::new();
    let mut add = |path: PathBuf, readonly: bool| {
        if !guards
            .iter()
            .any(|guard: &Guard| same_path(&guard.path, &path))
        {
            guards.push(Guard { path, readonly });
        }
    };
    for control in control_dirs(project, &repo) {
        for name in ["hooks", "info"] {
            let dir = control.git_dir.join(name);
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        add(control.git_dir.clone(), false);
        let readonly = [
            "hooks",
            "info",
            "config",
            "config.worktree",
            "objects/info/alternates",
        ]
        .into_iter()
        .map(|name| control.git_dir.join(name))
        .chain(control.gitfile.clone())
        .chain(config_targets(&control))
        .filter(|path| path.exists() && inside(project, path));
        for path in readonly {
            add(path, true);
        }
    }
    Ok(Plan {
        guards,
        notes: Vec::new(),
    })
}

/// Paths the config of a git dir points git at: the hooks directory and included config files.
/// A relative `core.hooksPath` is relative to the working tree, a relative `include.path` to the
/// config file. `~` paths are the user's own, outside the project.
fn config_targets(control: &ControlDir) -> Vec<PathBuf> {
    let Ok(settings) = hostgit::risky_settings(&control.git_dir.join("config")) else {
        return Vec::new();
    };
    settings
        .iter()
        .flat_map(|(key, values)| values.iter().map(move |value| (key, value)))
        .filter_map(|(key, value)| {
            let base = if key == "core.hookspath" {
                &control.worktree
            } else if key == "include.path" || key.starts_with("includeif.") {
                &control.git_dir
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

/// Fingerprints the project's git files. `previous` saves re-reading a config with git when
/// the file has not changed.
pub(crate) fn snapshot(project: &Path, previous: Option<&Snapshot>) -> Result<Snapshot> {
    let Some(repo) = repo(project) else {
        return Ok(Snapshot::default());
    };
    let mut files = BTreeMap::new();
    let mut settings = BTreeMap::new();
    for control in control_dirs(project, &repo) {
        let mut dirs = vec![control.git_dir.join("hooks")];
        let mut singles = vec![
            control.git_dir.join("config"),
            control.git_dir.join("config.worktree"),
            control.git_dir.join("commondir"),
            control.git_dir.join("info").join("attributes"),
            control
                .git_dir
                .join("objects")
                .join("info")
                .join("alternates"),
        ];
        singles.extend(control.gitfile.clone());
        for target in config_targets(&control) {
            if target.is_dir() {
                dirs.push(target);
            } else {
                singles.push(target);
            }
        }
        for dir in dirs {
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
        let config_path = control.git_dir.join("config");
        let config = relative(project, &config_path);
        if !files.contains_key(&config) {
            continue;
        }
        let unchanged =
            previous.filter(|previous| previous.files.get(&config) == files.get(&config));
        let risky = match unchanged.and_then(|previous| previous.settings.get(&config)) {
            Some(known) => known.clone(),
            None => hostgit::risky_settings(&config_path)
                .ok()
                .unwrap_or_default(),
        };
        settings.insert(config, risky);
    }
    Ok(Snapshot { files, settings })
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

/// What changed between two fingerprints: files in path order, then settings per config file
/// in key order.
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
    let empty = BTreeMap::new();
    let configs: BTreeSet<&String> = before
        .settings
        .keys()
        .chain(after.settings.keys())
        .collect();
    for config in configs {
        let old = before.settings.get(config).unwrap_or(&empty);
        let new = after.settings.get(config).unwrap_or(&empty);
        let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        for key in keys {
            let (values, what) = match (old.get(key), new.get(key)) {
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
    }
    changes
}

/// The warning for a list of changes. It never suggests running git in the project: even
/// `git status` runs a planted clean filter and `core.hooksPath=/dev/null` does not stop
/// config-defined hooks (RFC 0001, experiments 6 and 7). Config is read as data from `/` with
/// an absolute git and a cleared environment; hooks with `od`.
pub(crate) fn report(project: &Path, when: &str, changes: &[Change]) -> String {
    let listed = changes
        .iter()
        .map(|change| format!("  {change}\n"))
        .collect::<Vec<_>>()
        .concat();
    let config = changes
        .iter()
        .find_map(|change| match change {
            Change::Setting { config, .. } => Some(config.clone()),
            Change::File { .. } => None,
        })
        .unwrap_or_else(|| ".git/config".to_owned());
    let project = escape(&project.display().to_string());
    format!(
        "warning: git files changed {when} in {project}:\n{listed}\
         git on your Mac runs what these name, even from `git status`. Do not run git in this \
         folder yet; read the files as data from outside it:\n  \
         cd / && env -i PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
         /usr/bin/git --no-pager config --file '{project}/{}' --no-includes --null --list | od -c\n  \
         od -c '{project}/.git/hooks/<name>'\n\
         See https://finngi.github.io/msbx/how-to/guard-git.html",
        escape(&config)
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

    /// Real git on a fixture: the hardened command, plus a home for `~` includes and the file
    /// transport for submodules, which the hardened command turns off.
    fn git(dir: &Path, args: &[&str]) -> Result<()> {
        let output = hostgit::command()
            .current_dir(dir)
            .env("HOME", std::env::temp_dir())
            .env("GIT_ALLOW_PROTOCOL", "file")
            .args(["-c", "user.name=t", "-c", "user.email=t@t.test"])
            .args(args)
            .output()?;
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

    fn write_hook(dir: &Path, name: &str, body: &str, executable: bool) -> Result<()> {
        let hook = dir.join("hooks").join(name);
        std::fs::write(&hook, body)?;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(mode))?;
        Ok(())
    }

    fn rendered(changes: &[Change]) -> Vec<String> {
        changes.iter().map(ToString::to_string).collect()
    }

    fn listed(project: &Path, plan: &Plan) -> Vec<(String, bool)> {
        plan.guards
            .iter()
            .map(|guard| (relative(project, &guard.path), guard.readonly))
            .collect()
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
                git_dir: linked
                    .path()
                    .parent()
                    .context("temp dir has a parent")?
                    .join("elsewhere/.git/worktrees/x"),
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
        std::fs::create_dir_all(git_dir.join("objects/info"))?;
        std::fs::write(
            git_dir.join("objects/info/alternates"),
            "/elsewhere/objects\n",
        )?;
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

        let plan = mounts(project)?;
        assert_eq!(plan.notes, [] as [String; 0]);
        assert_eq!(
            listed(project, &plan),
            [
                (".git".to_owned(), false),
                (".git/hooks".to_owned(), true),
                (".git/info".to_owned(), true),
                (".git/config".to_owned(), true),
                (".git/objects/info/alternates".to_owned(), true),
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
    fn mounts_guard_submodule_git_dirs_and_their_pointer_files() -> Result<()> {
        let temp = Temp::new("submodule")?;
        let upstream = temp.path().join("upstream");
        std::fs::create_dir(&upstream)?;
        git(&upstream, &["init", "-q"])?;
        git(&upstream, &["commit", "-q", "--allow-empty", "-m", "init"])?;
        let project = temp.path().join("project");
        std::fs::create_dir(&project)?;
        git(&project, &["init", "-q"])?;
        git(
            &project,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                upstream.to_str().context("utf-8 path")?,
                "lib/dep",
            ],
        )?;
        let module = project.join(".git/modules/lib/dep");
        assert!(
            module.join("HEAD").is_file(),
            "git put the module under modules/"
        );

        let plan = mounts(&project)?;
        let guards = listed(&project, &plan);
        for expected in [
            (".git".to_owned(), false),
            (".git/modules/lib/dep".to_owned(), false),
            (".git/modules/lib/dep/hooks".to_owned(), true),
            (".git/modules/lib/dep/info".to_owned(), true),
            (".git/modules/lib/dep/config".to_owned(), true),
            ("lib/dep/.git".to_owned(), true),
        ] {
            assert!(guards.contains(&expected), "{expected:?} in {guards:?}");
        }

        let before = snapshot(&project, None)?;
        assert!(before.settings.contains_key(".git/modules/lib/dep/config"));
        write_hook(&module, "pre-commit", "evil", true)?;
        std::fs::write(project.join("lib/dep/.git"), "gitdir: /tmp/evil\n")?;
        git(
            &module,
            &["config", "--file", "config", "core.fsmonitor", "evil"],
        )?;
        let after = snapshot(&project, Some(&before))?;
        assert_eq!(
            rendered(&changes(&before, &after)),
            [
                ".git/modules/lib/dep/config (changed)",
                ".git/modules/lib/dep/hooks/pre-commit (new)",
                "lib/dep/.git (changed)",
                ".git/modules/lib/dep/config: core.fsmonitor = evil (new)",
            ]
        );
        Ok(())
    }

    #[test]
    fn mounts_are_empty_without_a_repo_and_noted_with_git_data_elsewhere() -> Result<()> {
        let no_repo = Temp::new("nomounts")?;
        assert_eq!(mounts(no_repo.path())?, Plan::default());
        let linked = Temp::new("linkedmounts")?;
        std::fs::write(linked.path().join(".git"), "gitdir: /tmp\n")?;
        let plan = mounts(linked.path())?;
        assert_eq!(plan.guards, []);
        assert_eq!(plan.notes.len(), 1);
        assert!(
            plan.notes[0].starts_with("note: the git data of ") && plan.notes[0].contains("/tmp"),
            "{}",
            plan.notes[0]
        );
        Ok(())
    }

    #[test]
    fn changes_list_hooks_settings_and_the_git_pointer() -> Result<()> {
        let temp = init_repo("changes")?;
        let project = temp.path();
        let git_dir = project.join(".git");
        write_hook(&git_dir, "pre-push", "#!/bin/sh\n", false)?;
        let before = snapshot(project, None)?;
        assert_eq!(before.settings.keys().collect::<Vec<_>>(), [".git/config"]);
        assert!(before.settings[".git/config"].is_empty());
        assert_eq!(changes(&before, &snapshot(project, Some(&before))?), []);

        write_hook(&git_dir, "pre-commit", "#!/bin/sh\nevil\n", true)?;
        write_hook(&git_dir, "pre-push", "#!/bin/sh\n", true)?;
        std::fs::remove_file(git_dir.join("hooks/pre-rebase.sample"))?;
        git(project, &["config", "core.fsmonitor", "./tools/watch.sh"])?;
        std::fs::write(git_dir.join("info/attributes"), "* filter=x\n")?;
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
    fn report_reads_files_as_data_and_never_suggests_git_status() {
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
        let text = report(
            Path::new("/Users/you/dev/app"),
            "during this session",
            &changes,
        );
        assert!(
            text.starts_with(
                "warning: git files changed during this session in /Users/you/dev/app:\n  \
                 .git/hooks/pre-commit (new)\n  \
                 .git/config: core.fsmonitor = ./tools/watch.sh (new)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "cd / && env -i PATH=/usr/bin:/bin GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
                 /usr/bin/git --no-pager config --file '/Users/you/dev/app/.git/config' \
                 --no-includes --null --list | od -c"
            ),
            "{text}"
        );
        assert!(
            text.contains("od -c '/Users/you/dev/app/.git/hooks/<name>'"),
            "{text}"
        );
        // No runnable git command on the project: not even the old `-c ... status` form.
        for forbidden in ["-C /Users", "git -c", " status\n", "diff", "checkout"] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
    }

    #[test]
    fn control_characters_in_names_are_escaped() {
        let change = Change::Setting {
            config: ".git/config".to_owned(),
            key: "alias.x".to_owned(),
            value: "!evil\n\u{1b}[2J".to_owned(),
            what: What::New,
        };
        assert_eq!(
            change.to_string(),
            ".git/config: alias.x = !evil\\n\\u{1b}[2J (new)"
        );
        let text = report(Path::new("/tmp/p\u{7}"), "now", &[change]);
        assert!(text.lines().all(|line| !line.chars().any(char::is_control)));
        assert!(text.contains("/tmp/p\\u{7}"), "{text}");
        assert_eq!(escape("plain/path"), "plain/path");
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
        write_hook(&project.join(".git"), "pre-commit", "evil", true)?;
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
        assert!(
            !stored.contains("repositoryformatversion"),
            "no file contents are stored"
        );
        Ok(())
    }
}
