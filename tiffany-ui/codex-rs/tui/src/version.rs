/// The current Tiffany Loop version as embedded at compile time.
///
/// The TUI is still built inside the forked Codex workspace, where some
/// compatibility crates keep their original package versions. User-facing
/// Tiffany surfaces must use this crate's version, not the inner codex-cli
/// compatibility crate version.
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TIFFANY_LOOP_VERSION: &str = CODEX_CLI_VERSION;
