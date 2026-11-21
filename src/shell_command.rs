//! Putting the `feather` command on the PATH from inside the app, like VS
//! Code's "Install 'code' command in PATH".
//!
//! On macOS and Linux it is a symlink to this program. The Windows installer
//! adds the command itself (see `packaging/windows`), so there is nothing to
//! do from here.
//!
//! Nothing that belongs to something else is ever replaced or removed: only
//! a missing entry, or a broken link left by an older Feather, is written
//! over, and removing deletes the link only while it still points here.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The installer looks after it.
    Installer,
    Installed,
    /// Something else called `feather` is already there.
    Taken(PathBuf),
    Missing,
    /// A broken link to a Feather that has moved or gone, holding this target.
    Stale(PathBuf),
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
    let Ok(program) = this_program() else {
        return Status::Taken(link);
    };
    status_of(&link, &program)
}

/// What is at `link`, for a Feather whose program is `program`.
fn status_of(link: &Path, program: &Path) -> Status {
    let Ok(meta) = std::fs::symlink_metadata(link) else {
        return Status::Missing;
    };
    if !meta.file_type().is_symlink() {
        return Status::Taken(link.to_owned());
    }
    let Ok(target) = std::fs::read_link(link) else {
        return Status::Taken(link.to_owned());
    };
    match std::fs::canonicalize(link) {
        Ok(real) if real == program => Status::Installed,
        // It leads somewhere real: someone else's command.
        Ok(_) => Status::Taken(link.to_owned()),
        // Broken. Only a link to something called feather is ours to fix.
        Err(_) if target.file_name().is_some_and(|n| n == "feather") => Status::Stale(target),
        Err(_) => Status::Taken(link.to_owned()),
    }
}

/// Creates the symlink. Returns a line to show the user.
pub fn install() -> Result<String, String> {
    let link = link_path().ok_or("The installer adds the feather command on Windows.")?;
    let exe = this_program()?;
    let stale = match status_of(&link, &exe) {
        Status::Installed => return Ok("The feather command is already installed.".to_owned()),
        Status::Taken(path) => {
            return Err(format!(
                "{} already exists and isn't Feather's.",
                path.display()
            ));
        }
        Status::Stale(old) => Some(old),
        Status::Missing | Status::Installer => None,
    };
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Err("Move Feather into Applications first, then try again.".to_owned());
    }
    match link_here(&exe, &link, stale.as_deref()) {
        Ok(()) => {}
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::PermissionDenied => {
            link_as_admin(&exe, &link, stale.as_deref())?;
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
    let target =
        std::fs::read_link(&link).map_err(|e| format!("Could not read {}: {e}", link.display()))?;
    match remove_if_unchanged(&link, &target) {
        Ok(()) => {}
        Err(e) if cfg!(target_os = "macos") && e.kind() == std::io::ErrorKind::PermissionDenied => {
            // Only while it still points where it did a moment ago.
            run_as_admin(
                "if [ \"$(readlink {1})\" = {2} ]; then rm -f {1}; fi",
                &[&link, &target],
            )?;
        }
        Err(e) => return Err(format!("Could not remove {}: {e}", link.display())),
    }
    Ok("Removed the feather command.".to_owned())
}

/// Deletes `link` if it is still a symlink to `target`.
fn remove_if_unchanged(link: &Path, target: &Path) -> std::io::Result<()> {
    if std::fs::read_link(link).is_ok_and(|now| now == target) {
        std::fs::remove_file(link)
    } else {
        Err(std::io::Error::other("it changed meanwhile"))
    }
}

/// Makes the link, first clearing a broken one that still points at `stale`.
fn link_here(exe: &Path, link: &Path, stale: Option<&Path>) -> std::io::Result<()> {
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(old) = stale
        && !link.exists()
    {
        remove_if_unchanged(link, old)?;
    }
    // Fails rather than replaces if anything is there now.
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
fn link_as_admin(exe: &Path, link: &Path, stale: Option<&Path>) -> Result<(), String> {
    let dir = link.parent().unwrap_or(Path::new("/usr/local/bin"));
    match stale {
        // `ln -s` without -f: it won't replace anything that is there.
        None => run_as_admin("mkdir -p {1} && ln -s {2} {3}", &[dir, exe, link]),
        Some(old) => run_as_admin(
            "mkdir -p {1} && if [ -L {3} ] && [ ! -e {3} ] && [ \"$(readlink {3})\" = {4} ]; \
             then rm -f {3}; fi && ln -s {2} {3}",
            &[dir, exe, link, old],
        ),
    }
}

/// Runs a shell command through an administrator password prompt. `{1}`,
/// `{2}` and so on stand for the paths, which reach the script as arguments,
/// so the paths need no escaping.
fn run_as_admin(command: &str, paths: &[&Path]) -> Result<(), String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg("on run argv")
        .arg("-e")
        .arg(admin_script(command, paths.len()))
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

/// The AppleScript line that runs `command`, with `{n}` replaced by the
/// shell-quoted nth argument.
fn admin_script(command: &str, count: usize) -> String {
    // The command sits inside an AppleScript string: escape it for that first.
    let mut script = command.replace('\\', "\\\\").replace('"', "\\\"");
    for i in 1..=count {
        script = script.replace(
            &format!("{{{i}}}"),
            &format!("\" & quoted form of item {i} of argv & \""),
        );
    }
    format!("do shell script \"{script}\" with administrator privileges")
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

    #[test]
    fn admin_scripts_quote_for_applescript() {
        assert_eq!(
            admin_script("if [ \"$(readlink {1})\" = {2} ]; then rm -f {1}; fi", 2),
            "do shell script \"if [ \\\"$(readlink \" & quoted form of item 1 of argv & \")\\\" = \
             \" & quoted form of item 2 of argv & \" ]; then rm -f \" & quoted form of item 1 of \
             argv & \"; fi\" with administrator privileges"
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_feathers_own_links_are_touched() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("feather-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let program = dir.join("feather");
        let other = dir.join("other-tool");
        std::fs::write(&program, "").unwrap();
        std::fs::write(&other, "").unwrap();
        let link = dir.join("bin").join("feather");

        assert_eq!(status_of(&link, &program), Status::Missing);
        symlink(&program, &link).unwrap();
        assert_eq!(status_of(&link, &program), Status::Installed);

        // A working link to anything else is someone else's.
        std::fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        assert_eq!(status_of(&link, &program), Status::Taken(link.clone()));
        assert!(link_here(&program, &link, None).is_err());
        assert_eq!(std::fs::read_link(&link).unwrap(), other);

        // A broken link to some feather is a leftover, and can be replaced.
        std::fs::remove_file(&link).unwrap();
        let gone = dir.join("old").join("feather");
        symlink(&gone, &link).unwrap();
        assert_eq!(status_of(&link, &program), Status::Stale(gone.clone()));
        link_here(&program, &link, Some(&gone)).unwrap();
        assert_eq!(status_of(&link, &program), Status::Installed);

        // A broken link to anything else is left alone, as is a plain file.
        std::fs::remove_file(&link).unwrap();
        symlink(dir.join("old").join("tool"), &link).unwrap();
        assert_eq!(status_of(&link, &program), Status::Taken(link.clone()));
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "").unwrap();
        assert_eq!(status_of(&link, &program), Status::Taken(link.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
