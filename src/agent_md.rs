//! Orchestrator's own AGENTS.md — the platform's "personality" file.
//!
//! Read from:
//!   - `~/.orchestrator/AGENTS.md` (global)
//!   - `<project>/.orchestrator/AGENTS.md` (project)
//!   - `<project>/AGENTS.md` (project root)
//!
//! Injected as the highest-priority system prompt section for every
//! worker. Use this to set default model preferences, project conventions,
//! and "how I want this orchestrator to behave" rules.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct AgentMd {
    pub content: String,
    pub sources: Vec<PathBuf>,
}

impl AgentMd {
    pub fn load() -> Self {
        Self::load_at(std::env::current_dir().ok().as_deref())
    }

    pub fn load_at(cwd: Option<&Path>) -> Self {
        let mut content = String::new();
        let mut sources = vec![];
        let home = home::home_dir();

        for path in agent_md_paths(home.as_deref(), cwd) {
            if path.exists() {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if !content.is_empty() {
                        content.push_str("\n\n---\n\n");
                    }
                    content.push_str(&format!(
                        "# {} (orchestrator AGENTS.md)\n\n",
                        path.display()
                    ));
                    content.push_str(&s);
                    sources.push(path);
                }
            }
        }

        Self { content, sources }
    }
}

fn agent_md_paths(home: Option<&Path>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push(home.join(".orchestrator").join("AGENTS.md"));
    }
    if let Some(cwd) = cwd {
        out.push(cwd.join(".orchestrator").join("AGENTS.md"));
        out.push(cwd.join("AGENTS.md"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_finds_global() {
        // Just exercise the loader; the real .orchestrator/ dir may or may not exist
        let _ = AgentMd::load();
    }

    #[test]
    fn load_at_finds_project() {
        let dir = tempfile::tempdir().unwrap();
        let orch_dir = dir.path().join(".orchestrator");
        fs::create_dir_all(&orch_dir).unwrap();
        fs::write(
            orch_dir.join("AGENTS.md"),
            "# My orchestrator rules\nAlways use sonnet.\n",
        )
        .unwrap();
        let md = AgentMd::load_at(Some(dir.path()));
        assert!(md.content.contains("Always use sonnet"));
        assert!(!md.sources.is_empty());
    }
}
