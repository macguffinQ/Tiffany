use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

fn main() -> ExitCode {
    let target = resolve_tiffany_loop();
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(&target).args(&args).exec();
        eprintln!("failed to launch tiffany-loop from tiffany compatibility alias: {err}");
        ExitCode::from(127)
    }

    #[cfg(not(unix))]
    {
        match Command::new(&target).args(&args).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(err) => {
                eprintln!("failed to launch tiffany-loop from tiffany compatibility alias: {err}");
                ExitCode::from(127)
            }
        }
    }
}

fn resolve_tiffany_loop() -> PathBuf {
    let exe_name = if cfg!(windows) {
        "tiffany-loop.exe"
    } else {
        "tiffany-loop"
    };

    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let adjacent = parent.join(exe_name);
        if adjacent.exists() {
            return adjacent;
        }
    }

    PathBuf::from(exe_name)
}
