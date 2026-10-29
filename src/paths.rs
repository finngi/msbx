//! Where msbx keeps its own files. XDG base directories on Linux and macOS alike (RFC 0002):
//! settings under `$XDG_CONFIG_HOME/msbx`, runtime state under `$XDG_STATE_HOME/msbx`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The two directories msbx writes to. Resolved once from the environment and passed down, so
/// tests can point everything at temporary folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dirs {
    /// `$XDG_CONFIG_HOME/msbx`, default `~/.config/msbx`.
    pub(crate) config: PathBuf,
    /// `$XDG_STATE_HOME/msbx`, default `~/.local/state/msbx`.
    pub(crate) state: PathBuf,
}

impl Dirs {
    pub(crate) fn from_env() -> Result<Self> {
        Self::resolve(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("XDG_STATE_HOME"),
            std::env::home_dir().as_deref(),
        )
    }

    /// An empty XDG variable counts as unset, as the XDG spec says.
    pub(crate) fn resolve(
        xdg_config: Option<OsString>,
        xdg_state: Option<OsString>,
        home: Option<&Path>,
    ) -> Result<Self> {
        let base = |xdg: Option<OsString>, fallback: &[&str]| -> Result<PathBuf> {
            if let Some(dir) = xdg.filter(|value| !value.is_empty()) {
                return Ok(PathBuf::from(dir));
            }
            let mut dir = home.map(Path::to_path_buf).context(
                "cannot find your home directory; set XDG_CONFIG_HOME and XDG_STATE_HOME",
            )?;
            dir.extend(fallback);
            Ok(dir)
        };
        Ok(Self {
            config: base(xdg_config, &[".config"])?.join("msbx"),
            state: base(xdg_state, &[".local", "state"])?.join("msbx"),
        })
    }

    pub(crate) fn policy_file(&self) -> PathBuf {
        self.config.join("policy.toml")
    }

    pub(crate) fn token_file(&self) -> PathBuf {
        self.state.join("proxy-token")
    }

    pub(crate) fn network_log(&self) -> PathBuf {
        self.state.join("network-log.json")
    }

    pub(crate) fn proxy_log(&self) -> PathBuf {
        self.state.join("proxy.log")
    }

    /// Per-sandbox fingerprints of the project's git files (RFC 0001).
    pub(crate) fn git_guard_dir(&self) -> PathBuf {
        self.state.join("git-guard")
    }
}

/// Writes `bytes` to `path` with mode 600 through a temporary file in the same directory and a
/// rename, so readers never see a partial file. The directory is created (mode 700) if needed.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    create_private_dir(dir)?;
    let temp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options
        .open(&temp)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all())
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        // Best effort: the temp file is useless once the write failed.
        let _ = std::fs::remove_file(&temp);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// Creates `dir` (and parents) with mode 700 unless it exists.
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> &'static Path {
        Path::new("/home/dev")
    }

    #[test]
    fn xdg_variables_override_the_home_defaults() -> Result<()> {
        let dirs = Dirs::resolve(None, None, Some(home()))?;
        assert_eq!(dirs.config, PathBuf::from("/home/dev/.config/msbx"));
        assert_eq!(dirs.state, PathBuf::from("/home/dev/.local/state/msbx"));
        assert_eq!(
            dirs.policy_file(),
            PathBuf::from("/home/dev/.config/msbx/policy.toml")
        );
        assert_eq!(
            dirs.token_file(),
            PathBuf::from("/home/dev/.local/state/msbx/proxy-token")
        );

        let dirs = Dirs::resolve(Some("/etc/x".into()), Some("/var/x".into()), Some(home()))?;
        assert_eq!(dirs.config, PathBuf::from("/etc/x/msbx"));
        assert_eq!(dirs.state, PathBuf::from("/var/x/msbx"));
        assert_eq!(
            dirs.network_log(),
            PathBuf::from("/var/x/msbx/network-log.json")
        );
        assert_eq!(dirs.proxy_log(), PathBuf::from("/var/x/msbx/proxy.log"));
        assert_eq!(dirs.git_guard_dir(), PathBuf::from("/var/x/msbx/git-guard"));
        Ok(())
    }

    #[test]
    fn empty_xdg_variables_count_as_unset_and_no_home_is_an_error() -> Result<()> {
        let dirs = Dirs::resolve(Some(OsString::new()), Some(OsString::new()), Some(home()))?;
        assert_eq!(dirs.config, PathBuf::from("/home/dev/.config/msbx"));
        assert!(Dirs::resolve(None, None, None).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn write_private_creates_the_directory_and_mode_600() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("msbx-paths-{}", std::process::id()));
        let file = root.join("nested").join("token");
        write_private(&file, b"first")?;
        write_private(&file, b"second")?;
        let outcome = (|| -> Result<()> {
            assert_eq!(std::fs::read_to_string(&file)?, "second");
            assert_eq!(
                std::fs::metadata(&file)?.permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(root.join("nested"))?.permissions().mode() & 0o777,
                0o700
            );
            let leftovers: Vec<_> = std::fs::read_dir(root.join("nested"))?
                .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
                .collect();
            assert_eq!(leftovers, ["token"]);
            Ok(())
        })();
        std::fs::remove_dir_all(&root)?;
        outcome
    }
}
