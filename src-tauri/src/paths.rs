//! Cross-platform path handling.
//!
//! Rules enforced here (and only here):
//! * Application directories come from `directories::ProjectDirs`, never from
//!   hard-coded `~/.config` / `%APPDATA%` strings.
//! * Paths travel as `PathBuf` inside the backend. They become strings only at
//!   the IPC boundary (display) or when handed to a shell-parsed command line
//!   (`GIT_SSH_COMMAND`), each with its own dedicated conversion.
//! * Canonicalization uses `dunce`, so Windows never sees `\\?\C:\...` verbatim
//!   paths, which libgit2, OpenSSH and many editors reject.
//! * Config files are written atomically (temp file + rename) so a crash never
//!   leaves a truncated `config.json`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Linux: `~/.config/octonode`, macOS: `~/Library/Application Support/dev.OctoNode.OctoNode`,
    /// Windows: `%APPDATA%\OctoNode\OctoNode\config`.
    pub config_dir: PathBuf,
    /// Local (non-roaming) data: the encrypted fallback vault lives here.
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> AppResult<Self> {
        let dirs = ProjectDirs::from("dev", "OctoNode", "OctoNode").ok_or_else(|| {
            AppError::Internal("could not determine the user's home directory".into())
        })?;
        Self::from_dirs(
            dirs.config_dir().to_path_buf(),
            dirs.data_local_dir().to_path_buf(),
            dirs.cache_dir().to_path_buf(),
        )
    }

    pub fn from_dirs(
        config_dir: PathBuf,
        data_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> AppResult<Self> {
        for dir in [&config_dir, &data_dir, &cache_dir] {
            fs::create_dir_all(dir)?;
        }
        Ok(Self {
            config_dir,
            data_dir,
            cache_dir,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn vault_file(&self) -> PathBuf {
        self.data_dir.join("secrets.vault")
    }
}

/// Canonicalizes a user-supplied repository path without the Windows verbatim
/// prefix and verifies it exists.
pub fn canonicalize(path: &Path) -> AppResult<PathBuf> {
    dunce::canonicalize(path)
        .map_err(|e| AppError::InvalidInput(format!("cannot resolve path {}: {e}", path.display())))
}

/// Human-readable path for the UI (native separators).
pub fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Converts a repository-relative git path (always `/`-separated, as libgit2
/// reports it) into a native relative `PathBuf`, rejecting traversal.
pub fn git_relative(path: &str) -> AppResult<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                return Err(AppError::InvalidInput(format!(
                    "path escapes the repository: {path:?}"
                )))
            }
            p if p.contains('\\') && cfg!(windows) => {
                return Err(AppError::InvalidInput(format!(
                    "invalid path component: {p:?}"
                )))
            }
            p => out.push(p),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(AppError::InvalidInput("empty path".into()));
    }
    Ok(out)
}

/// Quotes a path for `GIT_SSH_COMMAND` / `core.sshCommand`.
///
/// Git runs that value through `sh` on every platform (Git for Windows ships
/// its own), so: forward slashes, double quotes, escape `\ " $ \``.
pub fn shell_quote_for_git(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Writes `bytes` to `target` atomically: write a sibling temp file, fsync,
/// then rename over the target. On Unix the file is created `0600`.
pub fn atomic_write(target: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = target
        .parent()
        .ok_or_else(|| AppError::Internal(format!("no parent dir for {}", target.display())))?;
    fs::create_dir_all(dir)?;

    let file_name = target
        .file_name()
        .ok_or_else(|| AppError::Internal(format!("no file name in {}", target.display())))?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }

    // `rename` replaces atomically on Unix and (since Rust 1.5, via
    // MoveFileExW + MOVEFILE_REPLACE_EXISTING) on Windows.
    if let Err(e) = fs::rename(&tmp, target) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_relative_rejects_traversal() {
        assert!(git_relative("../etc/passwd").is_err());
        assert!(git_relative("a/../../b").is_err());
        assert!(git_relative("").is_err());
        let p = git_relative("src/main.rs").expect("valid path");
        assert_eq!(p, Path::new("src").join("main.rs"));
    }

    #[test]
    fn shell_quote_escapes_specials() {
        let q = shell_quote_for_git(Path::new("/home/a b/$key\"x"));
        assert_eq!(q, r#""/home/a b/\$key\"x""#);
    }

    #[test]
    fn atomic_write_replaces_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("cfg.json");
        atomic_write(&target, b"one").expect("first write");
        atomic_write(&target, b"two").expect("second write");
        assert_eq!(fs::read(&target).expect("read"), b"two");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
