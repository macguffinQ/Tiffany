//! Read Claude Code's existing configuration (CLAUDE.md, settings.json,
//! agents/*.md, commands/*.md, .mcp.json, prior sessions).
//!
//! CC stores its config in well-known locations. We merge global + project
//! and expose the result so the orchestrator can:
//! - inject CLAUDE.md into every worker prompt
//! - register CC's MCP servers into the orchestrator's pool
//! - read prior session history as context
//! - treat agents/*.md as named subagents

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CCConfig {
    /// Concatenated CLAUDE.md (global + project)
    pub system_prompt: String,

    pub settings: CCSettings,

    /// Subagents defined in agents/*.md (global + project)
    pub agents: Vec<CCAgent>,

    /// Slash commands in commands/*.md
    pub commands: Vec<CCCommand>,

    /// MCP servers from .mcp.json and ~/.claude.json
    pub mcp_servers: Vec<McpServer>,

    /// Prior session IDs from ~/.claude/projects/<hash>/sessions/*.jsonl
    pub prior_session_ids: Vec<String>,

    /// Debug: where each piece was loaded from
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CCSettings {
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub disabled_mcpjson: bool,
    /// Other keys we don't model
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CCAgent {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub body: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CCCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl CCConfig {
    /// Load and merge CC config from global + current directory.
    pub fn load() -> Self {
        Self::load_at(std::env::current_dir().ok().as_deref())
    }

    /// Load and merge CC config with an explicit CWD (used for tests).
    pub fn load_at(cwd: Option<&Path>) -> Self {
        let mut cfg = Self::default();
        let home = home::home_dir();

        // 1. CLAUDE.md
        for (path, scope) in claude_md_paths(home.as_deref(), cwd) {
            if path.exists() {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if !cfg.system_prompt.is_empty() {
                        cfg.system_prompt.push_str("\n\n---\n\n");
                    }
                    cfg.system_prompt
                        .push_str(&format!("# {} ({})\n\n", path.display(), scope));
                    cfg.system_prompt.push_str(&s);
                    cfg.sources.push(format!("CLAUDE.md ({})", path.display()));
                }
            }
        }

        // 2. settings.json
        for path in settings_paths(home.as_deref(), cwd) {
            if path.exists() {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if let Ok(parsed) = serde_json::from_str::<CCSettings>(&s) {
                        cfg.merge_settings(parsed);
                        cfg.sources
                            .push(format!("settings.json: {}", path.display()));
                    }
                }
            }
        }

        // 3. agents/*.md
        for dir in agents_dirs(home.as_deref(), cwd) {
            if dir.exists() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("md") {
                            if let Ok(agent) = parse_agent_file(&path) {
                                cfg.sources.push(format!(
                                    "agent: {} ({})",
                                    agent.name,
                                    path.display()
                                ));
                                cfg.agents.push(agent);
                            }
                        }
                    }
                }
            }
        }

        // 4. commands/*.md
        for dir in commands_dirs(home.as_deref(), cwd) {
            if dir.exists() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("md") {
                            if let Ok(cmd) = parse_command_file(&path) {
                                cfg.sources.push(format!(
                                    "command: /{} ({})",
                                    cmd.name,
                                    path.display()
                                ));
                                cfg.commands.push(cmd);
                            }
                        }
                    }
                }
            }
        }

        // 5. MCP servers
        for path in mcp_paths(home.as_deref(), cwd) {
            if path.exists() {
                if let Ok(s) = std::fs::read_to_string(&path) {
                    if let Ok(root) = serde_json::from_str::<serde_json::Value>(&s) {
                        let servers = root
                            .get("mcpServers")
                            .and_then(|v| v.as_object())
                            .cloned()
                            .unwrap_or_default();
                        for (name, spec) in servers {
                            if let Some(server) = parse_mcp_server(&name, &spec) {
                                cfg.sources.push(format!(
                                    "MCP: {} ({})",
                                    server.name,
                                    path.display()
                                ));
                                cfg.mcp_servers.push(server);
                            }
                        }
                    }
                }
            }
        }

        // 6. Prior session IDs (best-effort path-slug match against ~/.claude/projects/)
        if let (Some(home), Some(cwd)) = (home.as_ref(), cwd) {
            for slug in project_slugs(cwd) {
                let sessions_dir = home.join(".claude").join("projects").join(&slug);
                if sessions_dir.exists() {
                    if let Ok(rd) = std::fs::read_dir(&sessions_dir) {
                        for entry in rd.flatten() {
                            let p = entry.path();
                            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                                    cfg.prior_session_ids.push(stem.to_string());
                                }
                            }
                        }
                    }
                    if !cfg.prior_session_ids.is_empty() {
                        cfg.sources
                            .push(format!("prior sessions: {}", sessions_dir.display()));
                    }
                }
            }
        }

        cfg
    }

    /// Build a system prompt from CLAUDE.md + injected history.
    pub fn build_system_prompt(&self, history: &str) -> String {
        let mut s = String::new();
        if !self.system_prompt.is_empty() {
            s.push_str("# Project instructions (from CLAUDE.md)\n\n");
            s.push_str(&self.system_prompt);
            s.push_str("\n");
        }
        if !history.is_empty() {
            if !s.is_empty() {
                s.push_str("\n");
            }
            s.push_str("# Prior session context\n\n");
            s.push_str(history);
        }
        s
    }

    fn merge_settings(&mut self, other: CCSettings) {
        if other.model.is_some() && self.settings.model.is_none() {
            self.settings.model = other.model;
        }
        if other.permission_mode.is_some() && self.settings.permission_mode.is_none() {
            self.settings.permission_mode = other.permission_mode;
        }
        for t in other.allowed_tools {
            if !self.settings.allowed_tools.contains(&t) {
                self.settings.allowed_tools.push(t);
            }
        }
        self.settings.disabled_mcpjson |= other.disabled_mcpjson;
    }
}

// ── Path helpers ──────────────────────────────────────────────

fn claude_md_paths(home: Option<&Path>, cwd: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push((home.join(".claude").join("CLAUDE.md"), "global"));
    }
    if let Some(cwd) = cwd {
        out.push((cwd.join(".claude").join("CLAUDE.md"), "project"));
        out.push((cwd.join("CLAUDE.md"), "project-root"));
    }
    out
}

fn settings_paths(home: Option<&Path>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push(home.join(".claude").join("settings.json"));
    }
    if let Some(cwd) = cwd {
        out.push(cwd.join(".claude").join("settings.json"));
        out.push(cwd.join(".claude").join("settings.local.json"));
    }
    out
}

fn agents_dirs(home: Option<&Path>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push(home.join(".claude").join("agents"));
    }
    if let Some(cwd) = cwd {
        out.push(cwd.join(".claude").join("agents"));
    }
    out
}

fn commands_dirs(home: Option<&Path>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push(home.join(".claude").join("commands"));
    }
    if let Some(cwd) = cwd {
        out.push(cwd.join(".claude").join("commands"));
    }
    out
}

fn mcp_paths(home: Option<&Path>, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = home {
        out.push(home.join(".claude.json"));
    }
    if let Some(cwd) = cwd {
        out.push(cwd.join(".mcp.json"));
        out.push(cwd.join(".claude").join(".mcp.json"));
    }
    out
}

/// CC's project directory naming. The exact hash algorithm varies between
/// CC versions; we generate a few candidates and use whichever directory
/// exists. The most common form is absolute path with `/` replaced by `-`.
fn project_slugs(cwd: &Path) -> Vec<String> {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let p = canonical.to_string_lossy();
    vec![
        p.replace('/', "-"),
        p.replace('/', "-").trim_start_matches('-').to_string(),
    ]
}

// ── Parsers ───────────────────────────────────────────────────

fn parse_agent_file(path: &Path) -> Result<CCAgent> {
    let raw = std::fs::read_to_string(path)?;
    let (fm, body) = split_frontmatter(&raw);
    let parsed: serde_yaml::Value = if fm.is_empty() {
        serde_yaml::Value::Null
    } else {
        serde_yaml::from_str(&fm).unwrap_or(serde_yaml::Value::Null)
    };
    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("unnamed")
        .to_string();
    let description = parsed
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tools = parsed
        .get("tools")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let model = parsed
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(CCAgent {
        name,
        description,
        tools,
        model,
        body: body.to_string(),
        source: path.to_path_buf(),
    })
}

fn parse_command_file(path: &Path) -> Result<CCCommand> {
    let raw = std::fs::read_to_string(path)?;
    let (fm, body) = split_frontmatter(&raw);
    let parsed: serde_yaml::Value = if fm.is_empty() {
        serde_yaml::Value::Null
    } else {
        serde_yaml::from_str(&fm).unwrap_or(serde_yaml::Value::Null)
    };
    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("unnamed")
        .to_string();
    let description = parsed
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(CCCommand {
        name,
        description,
        body: body.to_string(),
        source: path.to_path_buf(),
    })
}

fn parse_mcp_server(name: &str, spec: &serde_json::Value) -> Option<McpServer> {
    let command = spec
        .get("command")
        .and_then(|v| v.as_str())
        .map(String::from)?;
    let args = spec
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let env = spec
        .get("env")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(McpServer {
        name: name.to_string(),
        command,
        args,
        env,
    })
}

/// Split a markdown file into YAML frontmatter and body.
/// Returns ("", raw) if no frontmatter.
fn split_frontmatter(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start_matches('\n');
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm = rest[..end].to_string();
            let mut body_start = end + 4;
            if rest[body_start..].starts_with('\n') {
                body_start += 1;
            }
            return (fm, rest[body_start..].to_string());
        }
    }
    (String::new(), raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_split_frontmatter() {
        let raw = "---\nname: foo\ndescription: bar\n---\nbody content\n";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.contains("name: foo"));
        assert!(body.contains("body content"));
    }

    #[test]
    fn test_split_frontmatter_no_fm() {
        let raw = "no frontmatter here";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.is_empty());
        assert_eq!(body, "no frontmatter here");
    }

    #[test]
    fn test_parse_agent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviewer.md");
        fs::write(
            &path,
            "---\nname: reviewer\ndescription: reviews PRs\ntools: [Read, Grep]\nmodel: sonnet\n---\nYou are a careful reviewer.\n",
        )
        .unwrap();
        let agent = parse_agent_file(&path).unwrap();
        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.description, "reviews PRs");
        assert_eq!(agent.tools, vec!["Read", "Grep"]);
        assert_eq!(agent.model.as_deref(), Some("sonnet"));
        assert!(agent.body.contains("careful reviewer"));
    }

    #[test]
    fn test_load_at_finds_claude_md() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("CLAUDE.md"),
            "# Project rules\nAlways write tests.\n",
        )
        .unwrap();
        let cfg = CCConfig::load_at(Some(dir.path()));
        assert!(cfg.system_prompt.contains("Always write tests"));
        assert!(cfg.sources.iter().any(|s| s.contains("CLAUDE.md")));
    }
}
