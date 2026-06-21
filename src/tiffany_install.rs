//! Helpers for the installed tiffany-loop command pair.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffanyBinarySource {
    Env,
    Adjacent,
    Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiffanyBinary {
    pub path: PathBuf,
    pub source: TiffanyBinarySource,
    pub verified: bool,
}

impl TiffanyBinary {
    pub fn source_label(&self) -> &'static str {
        match self.source {
            TiffanyBinarySource::Env => "TIFFANY_BIN",
            TiffanyBinarySource::Adjacent => "next to orchestrator",
            TiffanyBinarySource::Path => "PATH",
        }
    }
}

pub fn legacy_tui_forced() -> bool {
    std::env::var("ORCHESTRATOR_LEGACY_TUI")
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

pub fn resolve_tiffany_binary() -> Option<TiffanyBinary> {
    if let Some(path) = std::env::var_os("TIFFANY_BIN").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        let verified = path.exists() || which::which(&path).is_ok();
        return Some(TiffanyBinary {
            path,
            source: TiffanyBinarySource::Env,
            verified,
        });
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            for exe_name in tiffany_exe_names() {
                let adjacent = parent.join(exe_name);
                if adjacent.exists() {
                    return Some(TiffanyBinary {
                        path: adjacent,
                        source: TiffanyBinarySource::Adjacent,
                        verified: true,
                    });
                }
            }
        }
    }

    for exe_name in tiffany_exe_names() {
        if let Ok(path) = which::which(exe_name) {
            return Some(TiffanyBinary {
                path,
                source: TiffanyBinarySource::Path,
                verified: true,
            });
        }
    }
    None
}

pub fn find_tiffany_binary() -> Option<PathBuf> {
    resolve_tiffany_binary().map(|binary| binary.path)
}

pub fn tiffany_exe_name() -> &'static str {
    tiffany_exe_names()[0]
}

pub fn tiffany_exe_names() -> [&'static str; 2] {
    if cfg!(windows) {
        ["tiffany-loop.exe", "tiffany.exe"]
    } else {
        ["tiffany-loop", "tiffany"]
    }
}

pub fn current_orchestrator_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

pub fn resolved_tiffany_home() -> Option<(PathBuf, TiffanyHomeSource)> {
    if let Some(path) = std::env::var_os("TIFFANY_HOME").filter(|value| !value.is_empty()) {
        return Some((PathBuf::from(path), TiffanyHomeSource::Env));
    }
    home::home_dir().map(|home| (home.join(".tiffany"), TiffanyHomeSource::Default))
}

pub fn resolved_tiffany_sqlite_home(tiffany_home: &Path) -> (PathBuf, TiffanyHomeSource) {
    if let Some(path) = std::env::var_os("TIFFANY_SQLITE_HOME").filter(|value| !value.is_empty()) {
        return (PathBuf::from(path), TiffanyHomeSource::Env);
    }
    (tiffany_home.to_path_buf(), TiffanyHomeSource::Default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffanyHomeSource {
    Env,
    Default,
}

impl TiffanyHomeSource {
    pub fn label(self) -> &'static str {
        match self {
            TiffanyHomeSource::Env => "env",
            TiffanyHomeSource::Default => "default",
        }
    }
}

pub fn launch_command_preview(config_path: &Path) -> String {
    let tiffany = resolve_tiffany_binary()
        .map(|binary| binary.path)
        .unwrap_or_else(|| PathBuf::from(tiffany_exe_name()));
    let orchestrator = current_orchestrator_exe().unwrap_or_else(|| PathBuf::from("orchestrator"));
    let config = crate::config::expand_home(config_path);
    shell_join([
        tiffany,
        PathBuf::from("orchestrator"),
        PathBuf::from("--bin"),
        orchestrator,
        PathBuf::from("--orchestrator-config"),
        config,
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCheckout {
    pub root: PathBuf,
    pub commit: Option<String>,
    pub dirty: bool,
}

impl SourceCheckout {
    pub fn summary(&self) -> String {
        let commit = self.commit.as_deref().unwrap_or("unknown");
        let dirty = if self.dirty { " +dirty" } else { "" };
        format!("{} ({commit}{dirty})", self.root.display())
    }
}

pub fn source_checkout() -> Option<SourceCheckout> {
    let root = source_checkout_root_for_exe(std::env::current_exe().ok()?.as_path())?;
    Some(SourceCheckout {
        commit: git_output(&root, &["rev-parse", "--short", "HEAD"]),
        dirty: git_dirty(&root).unwrap_or(false),
        root,
    })
}

fn source_checkout_root_for_exe(exe_path: &Path) -> Option<PathBuf> {
    exe_path
        .ancestors()
        .find(|path| is_source_checkout_dir(path))
        .map(Path::to_path_buf)
}

fn is_source_checkout_dir(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join("scripts/tiffany-clean-targets").is_file()
}

fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_dirty(repo: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

fn shell_join(args: impl IntoIterator<Item = PathBuf>) -> String {
    args.into_iter()
        .map(|arg| shell_arg(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_arg_keeps_simple_paths_readable() {
        assert_eq!(
            shell_arg("/usr/local/bin/tiffany"),
            "/usr/local/bin/tiffany"
        );
        assert_eq!(
            shell_arg("/Applications/Tiffany Loop/tiffany"),
            "'/Applications/Tiffany Loop/tiffany'"
        );
    }

    #[test]
    fn source_labels_are_user_facing() {
        let binary = TiffanyBinary {
            path: PathBuf::from("tiffany-loop"),
            source: TiffanyBinarySource::Path,
            verified: true,
        };

        assert_eq!(binary.source_label(), "PATH");
    }

    #[test]
    fn source_checkout_summary_marks_dirty_state() {
        let checkout = SourceCheckout {
            root: PathBuf::from("/repo"),
            commit: Some("abc1234".into()),
            dirty: true,
        };

        assert_eq!(checkout.summary(), "/repo (abc1234 +dirty)");
    }

    #[test]
    fn source_checkout_summary_handles_unknown_commit() {
        let checkout = SourceCheckout {
            root: PathBuf::from("/repo"),
            commit: None,
            dirty: false,
        };

        assert_eq!(checkout.summary(), "/repo (unknown)");
    }

    #[test]
    fn source_checkout_root_is_based_on_executable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let bin_dir = repo.join("target/debug");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
        std::fs::write(repo.join("scripts/tiffany-clean-targets"), "").unwrap();
        let exe = bin_dir.join("orchestrator");
        std::fs::write(&exe, "").unwrap();

        assert_eq!(source_checkout_root_for_exe(&exe), Some(repo));
        assert_eq!(
            source_checkout_root_for_exe(Path::new("/usr/local/bin/orchestrator")),
            None
        );
    }
}
