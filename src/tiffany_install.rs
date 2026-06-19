//! Helpers for the installed tiffany-loop command pair.

use std::path::{Path, PathBuf};

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
}
