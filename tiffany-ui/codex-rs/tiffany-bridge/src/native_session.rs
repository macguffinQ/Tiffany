use sha2::Digest as _;
use sha2::Sha256;
use std::path::Path;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeSessionRuntime {
    Claude,
    Codex,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
pub struct NativeSessionCommand<'a> {
    pub role: &'a str,
    pub runtime: Option<&'a str>,
    pub native_session: &'a str,
    pub command: &'a str,
    pub worktree: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionPath {
    pub runtime: NativeSessionRuntime,
    pub path: PathBuf,
    pub message_count: usize,
}

pub fn native_session_path_in_home(
    home: &Path,
    command: &NativeSessionCommand<'_>,
) -> Option<NativeSessionPath> {
    if native_session_command_is_claude(command) {
        return Some(NativeSessionPath {
            runtime: NativeSessionRuntime::Claude,
            path: claude_session_jsonl_path(home, command.worktree?, command.native_session)?,
            message_count: 0,
        });
    }
    if native_session_command_is_codex(command) {
        return Some(NativeSessionPath {
            runtime: NativeSessionRuntime::Codex,
            path: codex_session_jsonl_path(home, command.native_session)?,
            message_count: 0,
        });
    }
    if native_session_command_is_gemini(command) {
        let path = gemini_session_json_path_in_home(home, command)?;
        let message_count = gemini_session_message_count(&path).unwrap_or(0);
        return Some(NativeSessionPath {
            runtime: NativeSessionRuntime::Gemini,
            path,
            message_count,
        });
    }
    None
}

pub fn native_session_command_is_claude(command: &NativeSessionCommand<'_>) -> bool {
    native_session_command_runtime_matches(command, &["claude-code", "claude", "cc"])
        || native_session_command_role_matches(command, &["worker-cc", "claude"])
        || command.command.contains("claude --resume")
        || command.command.contains("claude resume")
}

pub fn native_session_command_is_codex(command: &NativeSessionCommand<'_>) -> bool {
    native_session_command_runtime_matches(command, &["codex"])
        || native_session_command_role_matches(command, &["codex"])
        || command.command.contains("codex resume")
        || command.command.contains("codex exec resume")
}

pub fn native_session_command_is_gemini(command: &NativeSessionCommand<'_>) -> bool {
    native_session_command_runtime_matches(command, &["gemini", "gemini-cli"])
        || native_session_command_role_matches(command, &["gemini"])
        || command.command.contains("gemini --resume")
        || command.command.contains("gemini -r")
}

pub fn claude_session_jsonl_path(
    home: &Path,
    worktree: &str,
    native_session: &str,
) -> Option<PathBuf> {
    let worktree = nonempty_trimmed(worktree)?;
    let canonical = Path::new(worktree)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(worktree));
    let project_slug = canonical
        .to_string_lossy()
        .replace('/', "-")
        .trim_start_matches('-')
        .to_string();
    Some(
        home.join(".claude")
            .join("projects")
            .join(project_slug)
            .join(format!("{native_session}.jsonl")),
    )
}

pub fn codex_session_jsonl_path(home: &Path, native_session: &str) -> Option<PathBuf> {
    find_codex_rollout_path_by_id(&home.join(".codex").join("sessions"), native_session)
}

pub fn find_codex_rollout_path_by_id(root: &Path, id: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name.contains(id) && file_name.ends_with(".jsonl") {
                return Some(path);
            }
        }
    }
    None
}

pub fn gemini_session_json_path_in_home(
    home: &Path,
    command: &NativeSessionCommand<'_>,
) -> Option<PathBuf> {
    let root = home.join(".gemini").join("tmp");
    let id = command.native_session.trim();
    if !id.is_empty() && id != "latest" {
        if let Some(path) = find_gemini_chat_path_by_id(&root, id) {
            return Some(path);
        }
    }
    if let Some(project_path) = command
        .worktree
        .and_then(gemini_project_hash)
        .and_then(|hash| latest_gemini_chat_path_for_project(&root, &hash))
    {
        return Some(project_path);
    }
    if command.worktree.and_then(nonempty_trimmed).is_some() {
        return None;
    }
    latest_gemini_chat_path(&root)
}

pub fn gemini_project_hash(worktree: &str) -> Option<String> {
    let worktree = nonempty_trimmed(worktree)?;
    let canonical = Path::new(worktree)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(worktree));
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

pub fn find_gemini_chat_path_by_id(root: &Path, id: &str) -> Option<PathBuf> {
    gemini_chat_paths(root)
        .into_iter()
        .find(|path| gemini_chat_path_matches_id(path, id))
}

pub fn gemini_session_message_count(path: &Path) -> Option<usize> {
    let body = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&body).ok()?;
    value
        .get("messages")
        .and_then(|value| value.as_array())
        .map(Vec::len)
}

fn native_session_command_runtime_matches(
    command: &NativeSessionCommand<'_>,
    runtimes: &[&str],
) -> bool {
    let Some(runtime) = command
        .runtime
        .and_then(nonempty_trimmed)
        .map(|runtime| runtime.to_ascii_lowercase())
    else {
        return false;
    };
    runtimes.iter().any(|candidate| runtime == *candidate)
}

fn native_session_command_role_matches(
    command: &NativeSessionCommand<'_>,
    markers: &[&str],
) -> bool {
    let role = command.role.to_ascii_lowercase();
    markers
        .iter()
        .any(|marker| role == *marker || role.contains(marker))
}

fn gemini_chat_path_matches_id(path: &Path, id: &str) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if file_name.contains(id) {
        return true;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|value| {
            value
                .get("sessionId")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .is_some_and(|session_id| session_id == id || session_id.starts_with(id))
}

fn latest_gemini_chat_path_for_project(root: &Path, project_hash: &str) -> Option<PathBuf> {
    newest_gemini_chat_path(gemini_chat_paths(&root.join(project_hash).join("chats")))
}

fn latest_gemini_chat_path(root: &Path) -> Option<PathBuf> {
    newest_gemini_chat_path(gemini_chat_paths(root))
}

fn newest_gemini_chat_path(paths: Vec<PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, path)
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn gemini_chat_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if file_name.starts_with("session-") && file_name.ends_with(".json") {
                paths.push(path);
            }
        }
    }
    paths
}

fn nonempty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_native_session_runtime_from_runtime_role_or_command() {
        assert!(native_session_command_is_claude(&NativeSessionCommand {
            role: "executor",
            runtime: Some("claude-code"),
            native_session: "s1",
            command: "worker resume s1",
            worktree: Some("/tmp/repo"),
        }));
        assert!(native_session_command_is_codex(&NativeSessionCommand {
            role: "worker-codex",
            runtime: None,
            native_session: "s2",
            command: "worker-codex resume s2",
            worktree: Some("/tmp/repo"),
        }));
        assert!(native_session_command_is_gemini(&NativeSessionCommand {
            role: "worker",
            runtime: None,
            native_session: "latest",
            command: "gemini --resume latest",
            worktree: Some("/tmp/repo"),
        }));
    }

    #[test]
    fn claude_path_matches_project_slug_layout() {
        let path = claude_session_jsonl_path(
            Path::new("/home/alice"),
            "/Users/alice/code/project",
            "claude-session",
        )
        .expect("claude path");

        assert_eq!(
            path,
            PathBuf::from(
                "/home/alice/.claude/projects/Users-alice-code-project/claude-session.jsonl"
            )
        );
    }

    #[test]
    fn codex_rollout_path_finds_nested_jsonl_by_id() -> std::io::Result<()> {
        let home = tempfile::tempdir()?;
        let dir = home.path().join(".codex/sessions/2026/06/28");
        std::fs::create_dir_all(&dir)?;
        let rollout = dir.join("rollout-abc123.jsonl");
        std::fs::write(&rollout, "{}\n")?;

        assert_eq!(
            codex_session_jsonl_path(home.path(), "abc123"),
            Some(rollout)
        );
        Ok(())
    }

    #[test]
    fn gemini_project_hash_matches_cli_storage_layout() {
        assert_eq!(
            gemini_project_hash("/tiffany-loop/nonexistent-gemini-project").as_deref(),
            Some("beefa779e279137845bc9e354c1678846c66d0641b01693571f0bb86bae3fdb6")
        );
    }

    #[test]
    fn gemini_chat_path_uses_current_worktree_project_hash() -> std::io::Result<()> {
        let home = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        let project_hash =
            gemini_project_hash(&project.path().display().to_string()).expect("project hash");
        let chat_dir = home
            .path()
            .join(".gemini")
            .join("tmp")
            .join(project_hash)
            .join("chats");
        std::fs::create_dir_all(&chat_dir)?;
        let chat_path = chat_dir.join("session-2026-06-26T00-00-abcdef12.json");
        std::fs::write(
            &chat_path,
            r#"{"sessionId":"abcdef12-0000-0000-0000-000000000000","messages":[{},{}]}"#,
        )?;
        let command = NativeSessionCommand {
            role: "worker-gemini",
            runtime: Some("gemini"),
            native_session: "latest",
            command: "gemini --resume latest",
            worktree: Some(&project.path().display().to_string()),
        };

        let session_path = native_session_path_in_home(home.path(), &command).expect("path");

        assert_eq!(session_path.runtime, NativeSessionRuntime::Gemini);
        assert_eq!(session_path.path, chat_path);
        assert_eq!(session_path.message_count, 2);
        Ok(())
    }
}
