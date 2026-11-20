//! The `feather` command: what its arguments ask for, where their paths point,
//! and starting the window without holding up the shell that ran it.

use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

pub const USAGE: &str = "\
Usage: feather [options] [path]

Opens a file or folder in Feather. A folder becomes the workspace; a file opens
in its repository (or, outside one, in the folder it is in). A file that does
not exist yet opens empty and is created when you save it. With no path,
Feather opens the last repository it showed.

If a Feather window already shows that folder, or the repository the file is
in, it is brought to the front instead of opening another.

Options:
  -n, --new-window     Always open a new window
  -r, --reuse-window   Open in the most recent window, replacing its folder
  -w, --wait           Open a new window and return when it closes
  -h, --help           Show this help
  -v, --version        Show the version
";

/// What a run of the command asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Help,
    Version,
    Open(Options),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub path: Option<OsString>,
    pub window: WindowChoice,
    /// Run the window in this process rather than in the background.
    pub wait: bool,
}

/// Which window a path should open in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WindowChoice {
    /// One already showing it, or an empty one, else a new one.
    #[default]
    Auto,
    New,
    /// The most recent window, even if it shows something else.
    Reuse,
}

/// A path from the command line, made absolute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Folder(PathBuf),
    File(PathBuf),
    /// A file that doesn't exist yet: it opens empty and is created on save.
    NewFile(PathBuf),
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Request, String> {
    let mut options = Options::default();
    let mut only_paths = false;
    for arg in args {
        let text = arg.to_str();
        let is_option = !only_paths && text.is_some_and(|t| t.starts_with('-') && t.len() > 1);
        if is_option {
            match text.unwrap_or_default() {
                "--" => only_paths = true,
                "-h" | "--help" => return Ok(Request::Help),
                "-v" | "--version" => return Ok(Request::Version),
                "-n" | "--new-window" => options.window = WindowChoice::New,
                "-r" | "--reuse-window" => options.window = WindowChoice::Reuse,
                "-w" | "--wait" => options.wait = true,
                // Older macOS adds a process serial number when Finder starts an app.
                t if t.starts_with("-psn_") => {}
                t => return Err(format!("unknown option '{t}'")),
            }
            continue;
        }
        if text == Some("-") && !only_paths {
            return Err("reading from standard input isn't supported".to_owned());
        }
        if options.path.is_some() {
            return Err("Feather opens one file or folder at a time".to_owned());
        }
        options.path = Some(arg);
    }
    Ok(Request::Open(options))
}

/// Works out what `arg`, typed in `cwd`, refers to.
pub fn resolve(cwd: &Path, arg: &OsStr) -> Result<Target, String> {
    if arg.is_empty() {
        return Err("the path is empty".to_owned());
    }
    let path = cwd.join(arg);
    let shown = Path::new(arg).display();
    let names_folder = ends_with_separator(arg);
    if let Ok(meta) = std::fs::metadata(&path) {
        let real = canonical(&path);
        return if meta.is_dir() {
            Ok(Target::Folder(real))
        } else if names_folder {
            Err(format!("{shown} is a file, not a folder"))
        } else {
            Ok(Target::File(real))
        };
    }
    if names_folder {
        return Err(format!("there is no folder {shown}"));
    }
    let path = normalize(&path);
    let Some(ancestor) = existing_ancestor(&path) else {
        return Err(format!("can't open {shown}"));
    };
    if !ancestor.is_dir() {
        return Err(format!("{} is a file, not a folder", ancestor.display()));
    }
    let rest = path.strip_prefix(&ancestor).unwrap_or(Path::new(""));
    Ok(Target::NewFile(canonical(&ancestor).join(rest)))
}

/// The nearest folder or file at or above `path` that exists.
pub fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .map(Path::to_owned)
}

/// `path` relative to `root` with `/` between names, if it is inside it
/// (an empty string when it is `root` itself). On Windows the comparison
/// ignores case and accepts either kind of slash, since git reports
/// `C:/repo` where Windows says `C:\repo`.
pub fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let key = |c: Component| {
        let text = c.as_os_str().to_string_lossy().into_owned();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    let mut inner = path.components();
    for part in root.components() {
        if key(part) != key(inner.next()?) {
            return None;
        }
    }
    let names: Vec<String> = inner
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(names.join("/"))
}

/// `canonicalize` on Windows yields `\\?\C:\…` paths, which git and people
/// both dislike. Strip the prefix when the rest is an ordinary drive path.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return PathBuf::from(rest.to_owned());
        }
    }
    path
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .map(strip_verbatim)
        .unwrap_or_else(|_| normalize(path))
}

/// Drops `.` and folds `..` into the name before it, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(part);
                }
            }
            _ => out.push(part),
        }
    }
    out
}

fn ends_with_separator(arg: &OsStr) -> bool {
    let text = arg.to_string_lossy();
    text.ends_with('/') || (cfg!(windows) && text.ends_with('\\'))
}

// ── Launching ──────────────────────────────────────────────────────────────

/// Whether a terminal (or a script) ran us, as opposed to a desktop shortcut
/// or the file manager.
pub fn from_terminal() -> bool {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
            || std::io::stdout().is_terminal()
            || std::io::stderr().is_terminal()
    }
    #[cfg(windows)]
    {
        windows::has_std_handles() || windows::attach_parent_console()
    }
}

/// Starts another copy of Feather that outlives this one and doesn't hold on
/// to the terminal, so the shell gets its prompt back straight away.
pub fn spawn_detached(args: &[OsString]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    // Through a symlink such as /usr/local/bin/feather, run the real file so
    // macOS finds the app bundle around it.
    let exe = exe.canonicalize().map(strip_verbatim).unwrap_or(exe);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        Command::new(&exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own process group, so Ctrl+C and closing the terminal leave it be.
            .process_group(0)
            .spawn()?;
    }
    #[cfg(windows)]
    windows::spawn_detached(&exe, args)?;
    Ok(())
}

/// Prints to the terminal that ran us, even from a Windows GUI program.
pub fn print(text: &str, error: bool) {
    use std::io::Write;
    #[cfg(windows)]
    if !windows::has_std_handles() {
        if windows::attach_parent_console()
            && let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$")
        {
            let _ = console.write_all(text.as_bytes());
        }
        return;
    }
    let _ = if error {
        std::io::stderr().write_all(text.as_bytes())
    } else {
        std::io::stdout().write_all(text.as_bytes())
    };
}

#[cfg(windows)]
pub mod windows {
    use std::ffi::{OsStr, OsString, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;

    #[repr(C)]
    struct StartupInfo {
        cb: u32,
        reserved: *mut u16,
        desktop: *mut u16,
        title: *mut u16,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        columns: u32,
        rows: u32,
        fill_attribute: u32,
        flags: u32,
        show_window: u16,
        reserved2_size: u16,
        reserved2: *mut u8,
        std_input: RawHandle,
        std_output: RawHandle,
        std_error: RawHandle,
    }

    #[repr(C)]
    struct ProcessInformation {
        process: RawHandle,
        thread: RawHandle,
        process_id: u32,
        thread_id: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfo,
            process_information: *mut ProcessInformation,
        ) -> i32;
        fn CloseHandle(handle: RawHandle) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }

    /// Starts `exe` with no console, in its own process group, and without
    /// any of our handles. `std::process::Command` always passes on every
    /// inheritable handle, and a pipe that came down from the shell (or from
    /// a script further up) would then stay open as long as the window does,
    /// leaving whatever reads it waiting.
    pub fn spawn_detached(exe: &Path, args: &[OsString]) -> std::io::Result<()> {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let application: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
        let mut line = command_line(exe.as_os_str(), args);
        line.push(0);
        let startup = StartupInfo {
            cb: size_of::<StartupInfo>() as u32,
            reserved: null_mut(),
            desktop: null_mut(),
            title: null_mut(),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            columns: 0,
            rows: 0,
            fill_attribute: 0,
            flags: 0,
            show_window: 0,
            reserved2_size: 0,
            reserved2: null_mut(),
            std_input: null_mut(),
            std_output: null_mut(),
            std_error: null_mut(),
        };
        let mut info = ProcessInformation {
            process: null_mut(),
            thread: null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Some terminals put what they run in a job that is killed with the
        // tab; leave it when allowed to.
        let mut started = false;
        for flags in [flags | CREATE_BREAKAWAY_FROM_JOB, flags] {
            // SAFETY: every pointer is to a live, null-terminated buffer or
            // struct of the layout Windows expects; `line` may be written to,
            // as CreateProcessW requires.
            started = unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    0,
                    flags,
                    null(),
                    null(),
                    &startup,
                    &mut info,
                )
            } != 0;
            if started {
                break;
            }
        }
        if !started {
            return Err(std::io::Error::last_os_error());
        }
        allow_foreground(info.process_id);
        // SAFETY: handles CreateProcessW just gave us, closed once.
        unsafe {
            CloseHandle(info.thread);
            CloseHandle(info.process);
        }
        Ok(())
    }

    /// The program and its arguments as one command line, quoted the way
    /// programs split it back apart (the Microsoft C runtime's rules).
    pub fn command_line(exe: &OsStr, args: &[OsString]) -> Vec<u16> {
        let mut line = Vec::new();
        // A program path can't contain quotes, so plain quoting is enough.
        line.push(u16::from(b'"'));
        line.extend(exe.encode_wide());
        line.push(u16::from(b'"'));
        for arg in args {
            line.push(u16::from(b' '));
            quote(&arg.encode_wide().collect::<Vec<_>>(), &mut line);
        }
        line
    }

    fn quote(arg: &[u16], out: &mut Vec<u16>) {
        let [quote, backslash] = [u16::from(b'"'), u16::from(b'\\')];
        let special = b" \t\n\"".map(u16::from);
        let plain = !arg.is_empty() && !arg.iter().any(|c| special.contains(c));
        if plain {
            out.extend_from_slice(arg);
            return;
        }
        out.push(quote);
        let mut backslashes = 0;
        for &c in arg {
            if c == backslash {
                backslashes += 1;
                continue;
            }
            // Backslashes count only before a quote: double them there.
            let doubled = if c == quote {
                backslashes * 2 + 1
            } else {
                backslashes
            };
            out.extend(std::iter::repeat_n(backslash, doubled));
            out.push(c);
            backslashes = 0;
        }
        // And before the closing quote.
        out.extend(std::iter::repeat_n(backslash, backslashes * 2));
        out.push(quote);
    }

    /// Standard handles exist when the parent passed them on: pipes from Git
    /// Bash, a redirect, or a console.
    pub fn has_std_handles() -> bool {
        std_handles().iter().any(|h| usable(*h))
    }

    fn std_handles() -> [RawHandle; 3] {
        [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ]
    }

    fn usable(handle: RawHandle) -> bool {
        !handle.is_null() && handle as isize != -1
    }

    /// Joins the console of the program that started us, if it has one.
    pub fn attach_parent_console() -> bool {
        static ATTACHED: OnceLock<bool> = OnceLock::new();
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: a plain Win32 call with no pointers.
        *ATTACHED.get_or_init(|| unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0)
    }

    /// Lets another process bring its window to the front. Windows only lets
    /// whoever the user is interacting with do that, which is us, not it.
    pub fn allow_foreground(process_id: u32) {
        // SAFETY: a plain Win32 call with no pointers; failure is harmless.
        unsafe { AllowSetForegroundWindow(process_id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn open(list: &[&str]) -> Options {
        match parse(args(list)) {
            Ok(Request::Open(options)) => options,
            other => panic!("expected options for {list:?}, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_just_launches() {
        assert_eq!(open(&[]), Options::default());
    }

    #[test]
    fn one_path_with_window_options() {
        let o = open(&["main.rs"]);
        assert_eq!(o.path.as_deref(), Some(OsStr::new("main.rs")));
        assert_eq!(o.window, WindowChoice::Auto);
        assert!(!o.wait);

        let o = open(&["-n", "."]);
        assert_eq!(o.window, WindowChoice::New);
        assert_eq!(o.path.as_deref(), Some(OsStr::new(".")));

        let o = open(&["t2chat", "--reuse-window"]);
        assert_eq!(o.window, WindowChoice::Reuse);

        let o = open(&["--wait", "--wait", "--", "x"]);
        assert!(o.wait);
        assert_eq!(o.path.as_deref(), Some(OsStr::new("x")));
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(parse(args(&["--help"])), Ok(Request::Help));
        assert_eq!(parse(args(&["-h", "--bogus"])), Ok(Request::Help));
        assert_eq!(parse(args(&["x", "-v"])), Ok(Request::Version));
        assert_eq!(parse(args(&["--version"])), Ok(Request::Version));
    }

    #[test]
    fn after_double_dash_everything_is_a_path() {
        assert_eq!(
            open(&["--", "--help"]).path.as_deref(),
            Some(OsStr::new("--help"))
        );
        assert_eq!(open(&["--", "-"]).path.as_deref(), Some(OsStr::new("-")));
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(parse(args(&["--bogus"])).unwrap_err().contains("--bogus"));
        assert!(parse(args(&["a", "b"])).is_err());
        assert!(parse(args(&["-"])).is_err());
    }

    #[test]
    fn finder_serial_number_is_ignored() {
        assert_eq!(open(&["-psn_0_12345"]), Options::default());
    }

    /// A scratch folder with `src/main.rs` and `notes.txt` in it.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("feather-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "hi\n").unwrap();
        canonical(&dir)
    }

    #[test]
    fn folders_and_files_resolve_from_the_working_directory() {
        let dir = scratch("resolve");
        let os = |s: &str| OsString::from(s);
        assert_eq!(resolve(&dir, &os(".")), Ok(Target::Folder(dir.clone())));
        assert_eq!(
            resolve(&dir, &os("src")),
            Ok(Target::Folder(dir.join("src")))
        );
        assert_eq!(
            resolve(&dir.join("src"), &os("..")),
            Ok(Target::Folder(dir.clone()))
        );
        assert_eq!(
            resolve(&dir.join("src"), &os("main.rs")),
            Ok(Target::File(dir.join("src").join("main.rs")))
        );
        assert_eq!(
            resolve(&dir.join("src"), &os("../notes.txt")),
            Ok(Target::File(dir.join("notes.txt")))
        );
        // An absolute path ignores the working directory.
        let elsewhere = std::env::temp_dir();
        assert_eq!(
            resolve(&elsewhere, dir.join("notes.txt").as_os_str()),
            Ok(Target::File(dir.join("notes.txt")))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_paths_become_new_files() {
        let dir = scratch("missing");
        let os = |s: &str| OsString::from(s);
        assert_eq!(
            resolve(&dir, &os("todo.md")),
            Ok(Target::NewFile(dir.join("todo.md")))
        );
        assert_eq!(
            resolve(&dir, &os("src/./new/deep.rs")),
            Ok(Target::NewFile(dir.join("src").join("new").join("deep.rs")))
        );
        assert_eq!(
            resolve(&dir.join("src"), &os("../other.rs")),
            Ok(Target::NewFile(dir.join("other.rs")))
        );
        // Not under a file, and not something that has to be a folder.
        assert!(resolve(&dir, &os("notes.txt/inside")).is_err());
        assert!(resolve(&dir, &os("nowhere/")).is_err());
        assert!(resolve(&dir, &os("notes.txt/")).is_err());
        assert!(resolve(&dir, &os("")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_paths_inside_a_root() {
        let root = Path::new("/work/repo");
        assert_eq!(
            relative_to(root, Path::new("/work/repo/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            relative_to(root, Path::new("/work/repo")).as_deref(),
            Some("")
        );
        assert_eq!(relative_to(root, Path::new("/work/repo2/a")), None);
        assert_eq!(relative_to(root, Path::new("/work")), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_lines_quote_like_the_c_runtime() {
        let line = |args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            String::from_utf16(&windows::command_line(
                OsStr::new(r"C:\F\feather.exe"),
                &args,
            ))
            .unwrap()
        };
        assert_eq!(line(&[]), r#""C:\F\feather.exe""#);
        assert_eq!(
            line(&["--wait", "--", r"C:\repo\src"]),
            r#""C:\F\feather.exe" --wait -- C:\repo\src"#
        );
        assert_eq!(line(&[""]), r#""C:\F\feather.exe" """#);
        assert_eq!(
            line(&[r"C:\my repo\"]),
            r#""C:\F\feather.exe" "C:\my repo\\""#
        );
        assert_eq!(line(&[r#"say "hi""#]), r#""C:\F\feather.exe" "say \"hi\"""#);
        assert_eq!(line(&[r#"a\"b"#]), r#""C:\F\feather.exe" "a\\\"b""#);
        assert_eq!(line(&[r"a\\b c"]), r#""C:\F\feather.exe" "a\\b c""#);
    }

    #[cfg(windows)]
    #[test]
    fn windows_roots_match_whatever_the_slashes_and_case() {
        // git reports roots with forward slashes.
        let root = Path::new("C:/Users/Me/Repo");
        assert_eq!(
            relative_to(root, Path::new(r"c:\users\me\repo\src\main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            relative_to(root, Path::new(r"C:\Users\Me\Repo")).as_deref(),
            Some("")
        );
        assert_eq!(relative_to(root, Path::new(r"D:\Users\Me\Repo\x")), None);
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\a\b")),
            PathBuf::from(r"C:\a\b")
        );
    }
}
