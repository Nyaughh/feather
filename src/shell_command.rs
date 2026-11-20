//! Putting the `feather` command on the PATH from inside the app, like VS
//! Code's "Install 'code' command in PATH".
//!
//! On macOS and Linux it is a symlink to this program. The Windows installer
//! adds the command itself (see `packaging/windows`), so there is nothing to
//! do from here.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The installer looks after it.
    Installer,
    Installed,
    /// Something else called `feather` is already there.
    Taken(PathBuf),
    Missing,
}

/// Where the symlink goes.
pub fn link_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        // On the PATH of every macOS account (see /etc/paths).
        Some(PathBuf::from("/usr/local/bin/feather"))
    } else if cfg!(windows) {
        None
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/feather"))
    }
}

fn this_program() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Can't find Feather's program: {e}"))?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

pub fn status() -> Status {
    let Some(link) = link_path() else {
        return Status::Installer;
    };
    match std::fs::symlink_metadata(&link) {
        Err(_) => Status::Missing,
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::canonicalize(&link).ok();
            if target.is_some() && target == this_program().ok() {
                Status::Installed
            } else {
                // A leftover link, say from a copy that has moved: ours to replace.
                Status::Missing
            }
        }
        Ok(_) => Status::Taken(link),
    }
}

/// Creates (or repoints) the symlink. Returns a line to show the user.
pub fn install() -> Result<String, String> {
    let link = link_path().ok_or("The installer adds the feather command on Windows.")?;
    if let Status::Taken(path) = status() {
        return Err(format!(
            "{} already exists and isn't Feather's.",
            path.display()
        ));
    }
    let exe = this_program()?;
    let text = exe.to_string_lossy();
    if text.contains("/AppTranslocation/") {
        return Err("Move Feather into Applications first, then try again.".to_owned());
    }
    match link_here(&exe, &link) {
        Ok(()) => {}
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::PermissionDenied => {
            link_as_admin(&exe, &link)?;
        }
        Err(e) => return Err(format!("Could not create {}: {e}", link.display())),
    }
    let dir = link.parent().unwrap_or(Path::new("/"));
    Ok(if on_path(dir) {
        "Installed. Open a new terminal and type feather.".to_owned()
    } else {
        format!(
            "Installed in {}, which isn't on your PATH yet. Add it, then open a new terminal.",
            dir.display()
        )
    })
}

/// Removes the symlink, if it is ours.
pub fn remove() -> Result<String, String> {
    let link = link_path().ok_or("The uninstaller removes the feather command on Windows.")?;
    if status() != Status::Installed {
        return Ok("The feather command isn't installed.".to_owned());
    }
    match std::fs::remove_file(&link) {
        Ok(()) => {}
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::PermissionDenied => {
            run_as_admin("rm -f {1}", &[&link])?;
        }
        Err(e) => return Err(format!("Could not remove {}: {e}", link.display())),
    }
    Ok("Removed the feather command.".to_owned())
}

fn link_here(exe: &Path, link: &Path) -> std::io::Result<()> {
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::symlink_metadata(link).is_ok() {
        std::fs::remove_file(link)?;
    }
    #[cfg(unix)]
    return std::os::unix::fs::symlink(exe, link);
    #[cfg(not(unix))]
    {
        let _ = exe;
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// /usr/local/bin belongs to root on most Macs: ask for a password the way
/// VS Code does.
fn link_as_admin(exe: &Path, link: &Path) -> Result<(), String> {
    let dir = link.parent().unwrap_or(Path::new("/usr/local/bin"));
    run_as_admin("mkdir -p {1} && ln -sf {2} {3}", &[dir, exe, link])
}

/// Runs a shell command through an administrator password prompt. `{1}`,
/// `{2}` and so on stand for the paths, which reach the script as arguments,
/// so nothing needs escaping.
fn run_as_admin(command: &str, paths: &[&Path]) -> Result<(), String> {
    let mut script = command.to_owned();
    for i in 1..=paths.len() {
        script = script.replace(
            &format!("{{{i}}}"),
            &format!("\" & quoted form of item {i} of argv & \""),
        );
    }
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg("on run argv")
        .arg("-e")
        .arg(format!(
            "do shell script \"{script}\" with administrator privileges"
        ))
        .arg("-e")
        .arg("end run")
        .args(paths)
        .output()
        .map_err(|e| format!("Could not ask for permission: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("-128") {
            Err("Cancelled.".to_owned())
        } else {
            Err(format!("Could not install the command: {}", error.trim()))
        }
    }
}

fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|p| p == dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_has_a_place_for_the_command() {
        let link = link_path();
        if cfg!(windows) {
            assert_eq!(link, None);
            assert_eq!(status(), Status::Installer);
        } else {
            assert!(link.is_some_and(|l| l.ends_with("bin/feather")));
        }
    }
}
