//! Non-zellij fallback: just runs workers as plain background processes.

use anyhow::Result;
use std::process::Command;
use tracing::info;

pub fn run_plain(name: &str, command: &str, args: &[String]) -> Result<()> {
    info!(name, command, "running worker without multiplexer");
    let mut cmd = Command::new(command);
    cmd.args(args);
    let _ = cmd.status();
    Ok(())
}

pub fn tmux_available() -> bool {
    which::which("tmux").is_ok()
}

pub fn run_in_tmux(name: &str, command: &str, args: &[String]) -> Result<()> {
    let joined = std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let status = Command::new("tmux")
        .args(["new-window", "-d", "-t", name, &joined])
        .status()?;
    if !status.success() {
        anyhow::bail!("tmux new-window failed");
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_detached_process(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0)
}

#[cfg(not(unix))]
fn prepare_detached_process(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

/// Detach terminal chat to a background process (when not in zellij).
/// Writes the PID to `~/.orchestrator/tui.pid` and stdout to `~/.orchestrator/tui.log`.
pub fn detach_tui() -> Result<()> {
    let home = home::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let orch_dir = home.join(".orchestrator");
    std::fs::create_dir_all(&orch_dir)?;
    let pid_path = orch_dir.join("tui.pid");
    let log_path = orch_dir.join("tui.log");

    let exe = std::env::current_exe()?;
    let log_file = std::fs::File::create(&log_path)?;

    let mut command = std::process::Command::new(&exe);
    command
        .arg("tui")
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);
    let child = prepare_detached_process(&mut command).spawn()?;

    std::fs::write(&pid_path, child.id().to_string())?;
    println!(
        "terminal chat detached: pid {} (log: {}, pidfile: {})",
        child.id(),
        log_path.display(),
        pid_path.display()
    );
    println!("to stop: kill $(cat {})", pid_path.display());
    Ok(())
}
