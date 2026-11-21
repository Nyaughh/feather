//! The `feather` command on Windows. The installer copies this to
//! `bin\feather.exe` beside Feather and puts that folder on the user's PATH.
//!
//! Feather itself is a windowed program, which Command Prompt and PowerShell
//! don't wait for, so its `--help` would print after the next prompt and
//! `--wait` wouldn't wait. This small console program runs it with the
//! arguments exactly as given, waits, and passes on its exit code. Feather
//! starts its window in the background and returns at once, so the prompt
//! still comes straight back. (A batch file would do the waiting too, but
//! cmd.exe re-reads its arguments as text, so a file named `a&b.txt` or
//! `%PATH%.md` would turn into something else.)

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::process::{Command, ExitCode};
    let feather = std::env::current_exe().ok().and_then(|exe| {
        let app = exe.parent()?.parent()?;
        Some(app.join("feather.exe"))
    });
    let Some(feather) = feather else {
        eprintln!("feather: can't find Feather next to this command");
        return ExitCode::from(1);
    };
    let status = Command::new(&feather)
        // Tells Feather it is the shell command, even when every stream has
        // been redirected.
        .arg("--from-shell")
        .args(std::env::args_os().skip(1))
        .status();
    match status {
        Ok(status) => ExitCode::from(status.code().map_or(1, |c| u8::try_from(c).unwrap_or(1))),
        Err(e) => {
            eprintln!("feather: can't start {}: {e}", feather.display());
            ExitCode::from(1)
        }
    }
}

/// Elsewhere the `feather` command is a link to Feather itself.
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("feather-launcher is only used on Windows; run feather itself.");
    std::process::ExitCode::from(1)
}
