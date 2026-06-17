//! zellij integration: detect zellij session, spawn panes/tabs.

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

/// Returns true if we're currently inside a zellij session.
pub fn in_zellij() -> bool {
    std::env::var("ZELLIJ").is_ok() || std::env::var("ZELLIJ_SESSION_NAME").is_ok()
}

/// Returns true if the `zellij` binary is on PATH.
pub fn zellij_available() -> bool {
    which::which("zellij").is_ok()
}

/// Layout description for a single tab with one pane running `cmd`.
#[derive(serde::Serialize)]
pub struct PaneSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// Open a new zellij tab for a worker.
pub fn open_tab(name: &str, command: &str, _args: &[String], _cwd: Option<&PathBuf>) -> Result<()> {
    if !in_zellij() {
        anyhow::bail!("not inside a zellij session");
    }
    let mut cmd = Command::new("zellij");
    cmd.args(["action", "new-tab", "--name", name]);
    let status = cmd.status()?;
    if !status.success() {
        warn!("zellij new-tab returned non-zero");
    }
    info!(name, command, "opened zellij tab");
    Ok(())
}

/// Write a minimal KDL layout for zellij to consume on session start.
pub fn write_kdl_layout(
    path: &PathBuf,
    orchestrator_cmd: &[String],
    workers: &[(String, Vec<String>, PathBuf)],
) -> Result<()> {
    let mut s = String::from("layout {\n  tab name=\"orchestrator\" {\n");
    s.push_str(&format!(
        "    pane name=\"orchestrator\" command=\"{}\"\n",
        orchestrator_cmd.join(" ")
    ));
    for (name, cmd, _cwd) in workers {
        s.push_str(&format!(
            "    pane name=\"{}\" command=\"{}\"\n",
            name,
            cmd.join(" ")
        ));
    }
    s.push_str("  }\n}\n");
    std::fs::write(path, s)?;
    Ok(())
}

/// Spawn a brand-new zellij session from outside.
pub fn spawn_session(name: &str, layout_path: &PathBuf) -> Result<()> {
    if !zellij_available() {
        anyhow::bail!("zellij binary not found on PATH");
    }
    let mut cmd = Command::new("zellij");
    cmd.args(["--session", name, "--layout", "compact"]);
    if layout_path.exists() {
        cmd.arg("--layout-path").arg(layout_path);
    }
    cmd.spawn()?;
    Ok(())
}

/// Current pane name (if known).
pub fn current_pane_name() -> Option<String> {
    std::env::var("ZELLIJ_PANE_NAME").ok()
}

/// Open a new zellij tab and run `orchestrator tui` in it.
/// Returns immediately after the tab is created.
pub fn open_tui_in_new_tab() -> anyhow::Result<()> {
    if !in_zellij() {
        anyhow::bail!("not inside a zellij session");
    }
    let exe = std::env::current_exe()?;
    // We invoke bash -c to keep the tab alive after terminal chat exits,
    // so the user can see the exit message and then press q again.
    let shell_cmd = format!(
        "{} tui; echo; echo '[terminal chat exited - press q to close this tab]'; sleep 999999",
        exe.display()
    );
    let status = std::process::Command::new("zellij")
        .args(["action", "new-tab", "--name", "orchestrator-tui", "--"])
        .arg("bash")
        .arg("-c")
        .arg(&shell_cmd)
        .status()?;
    if !status.success() {
        anyhow::bail!("zellij action new-tab failed (exit {:?})", status.code());
    }
    Ok(())
}
