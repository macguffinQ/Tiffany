//! MCP server pool. Manages a small set of MCP server processes (fs, git, etc.)
//! and provides a config snippet that workers (CC, Codex) can read to discover
//! available MCP servers.

use crate::cc_config::CCConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Clone, Debug)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

pub struct McpPool {
    servers: HashMap<String, McpServerSpec>,
}

impl McpPool {
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
        }
    }

    /// Register a server in the pool. Does not start it yet.
    pub fn register(&mut self, spec: McpServerSpec) {
        info!(name = %spec.name, "registering MCP server");
        self.servers.insert(spec.name.clone(), spec);
    }

    /// Register the canonical default servers (fs, git).
    pub fn register_defaults(&mut self) {
        self.register(McpServerSpec {
            name: "fs".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "~".into()],
            env: HashMap::new(),
        });
        self.register(McpServerSpec {
            name: "git".into(),
            command: "uvx".into(),
            args: vec!["mcp-server-git".into()],
            env: HashMap::new(),
        });
    }

    /// Merge in MCP servers discovered from Claude Code's config files
    /// (`.mcp.json`, `~/.claude.json`). Existing entries with the same name
    /// are NOT overwritten.
    pub fn merge_cc_config(&mut self, cc: &CCConfig) {
        for s in &cc.mcp_servers {
            if !self.servers.contains_key(&s.name) {
                let mut env = HashMap::new();
                for (k, v) in &s.env {
                    env.insert(k.clone(), v.clone());
                }
                self.servers.insert(
                    s.name.clone(),
                    McpServerSpec {
                        name: s.name.clone(),
                        command: s.command.clone(),
                        args: s.args.clone(),
                        env,
                    },
                );
                info!(name = %s.name, "registered MCP server from CC config");
            }
        }
    }

    /// Write a `.mcp.json` config file into `worktree` that CC can discover.
    /// Returns the path to the config file.
    pub fn write_config(&self, worktree: &Path) -> Result<PathBuf> {
        let mut servers = serde_json::Map::new();
        for (name, spec) in &self.servers {
            let mut server = serde_json::Map::new();
            server.insert("command".into(), serde_json::Value::String(spec.command.clone()));
            server.insert(
                "args".into(),
                serde_json::Value::Array(
                    spec.args.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                ),
            );
            if !spec.env.is_empty() {
                let env_map: serde_json::Map<_, _> = spec
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                server.insert("env".into(), serde_json::Value::Object(env_map));
            }
            servers.insert(name.clone(), serde_json::Value::Object(server));
        }
        let body = serde_json::json!({ "mcpServers": servers });
        let path = worktree.join(".mcp.json");
        std::fs::write(&path, serde_json::to_string_pretty(&body)?)?;
        Ok(path)
    }
}

impl Default for McpPool {
    fn default() -> Self {
        Self::new()
    }
}
