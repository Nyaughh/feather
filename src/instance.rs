//! Lets `feather <path>` use a window that is already open, the way `code`
//! does with VS Code.
//!
//! Each running Feather listens on a loopback port and writes the port, its
//! process id and a random token to a small file in a per-user folder. The
//! command reads those files, newest first, and asks each window in turn
//! whether it will take the path. A window says yes when it already shows
//! that folder (or the repository a file is in), or when it shows nothing.
//! Without a yes, the command opens a new window. Files whose window has gone
//! away are deleted as they are found.

use crate::cli::{self, Target, WindowChoice};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Bumped whenever the messages change, so old and new copies leave each other be.
const PROTOCOL: u32 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// Far more than any request needs.
const MAX_REQUEST: u64 = 64 * 1024;

/// What a running window writes about itself.
#[derive(Serialize, Deserialize)]
struct Entry {
    protocol: u32,
    pid: u32,
    port: u16,
    token: String,
    /// Only the same program answers, so a development build and an
    /// installed Feather each keep to their own windows.
    exe: PathBuf,
}

/// Which windows may take a path, from most to least particular.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Want {
    /// One that already shows the folder, or the repository the file is in.
    Showing,
    /// One that has no folder open.
    Empty,
    /// Any window at all.
    Any,
    /// Only asking: answers as `Showing` would, but the window does nothing.
    Peek,
}

#[derive(Serialize, Deserialize)]
struct Ask {
    token: String,
    want: Want,
    target: Target,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    accepted: bool,
}

/// A path another `feather` command handed to this window.
pub struct Handoff {
    pub target: Target,
}

/// This window's end: listens for paths and says what it is showing.
pub struct Server {
    root: Arc<Mutex<Option<PathBuf>>>,
    handoffs: Receiver<Handoff>,
    entry_file: PathBuf,
}

impl Server {
    /// Starts listening, or returns `None` if there is nowhere to say so.
    pub fn start(wake: impl Fn() + Send + 'static) -> Option<Self> {
        let dir = registry_dir()?;
        let exe = own_exe()?;
        create_private_dir(&dir).ok()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = new_token();
        let entry = Entry {
            protocol: PROTOCOL,
            pid: std::process::id(),
            port,
            token: token.clone(),
            exe,
        };
        let entry_file = dir.join(format!("{}.json", entry.pid));
        write_private(&entry_file, &serde_json::to_vec(&entry).ok()?).ok()?;

        let root = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let shared = root.clone();
        std::thread::Builder::new()
            .name("feather-instance".to_owned())
            .spawn(move || listen(listener, &token, &shared, &tx, &wake))
            .ok()?;
        Some(Self {
            root,
            handoffs: rx,
            entry_file,
        })
    }

    /// Records the folder this window now shows.
    pub fn set_root(&self, root: Option<PathBuf>) {
        if let Ok(mut current) = self.root.lock() {
            *current = root;
        }
    }

    pub fn poll(&self) -> Option<Handoff> {
        self.handoffs.try_recv().ok()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.entry_file);
    }
}

fn listen(
    listener: TcpListener,
    token: &str,
    root: &Mutex<Option<PathBuf>>,
    tx: &Sender<Handoff>,
    wake: &dyn Fn(),
) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
        let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
        let mut line = String::new();
        let mut reader = BufReader::new((&stream).take(MAX_REQUEST));
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let Ok(ask) = serde_json::from_str::<Ask>(&line) else {
            continue;
        };
        if ask.token != token {
            continue;
        }
        let current = root.lock().ok().and_then(|r| r.clone());
        let accepted = accepts(current.as_deref(), ask.want, &ask.target);
        if accepted && ask.want != Want::Peek && tx.send(Handoff { target: ask.target }).is_ok() {
            wake();
        }
        let reply = serde_json::to_string(&Reply { accepted }).unwrap_or_default();
        let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
    }
}

/// Whether a window showing `root` should take `target`.
fn accepts(root: Option<&Path>, want: Want, target: &Target) -> bool {
    match (want, root) {
        (Want::Any, _) | (Want::Empty, None) => true,
        (Want::Empty, Some(_)) | (Want::Showing | Want::Peek, None) => false,
        (Want::Showing | Want::Peek, Some(root)) => shows(root, target),
    }
}

/// Whether a window showing `root` already shows `target`.
pub fn shows(root: &Path, target: &Target) -> bool {
    match target {
        // Opening a folder inside a repository opens the repository.
        Target::Folder(path) => {
            let opens = crate::git::find_root(path).unwrap_or_else(|| path.clone());
            cli::relative_to(root, &opens).is_some_and(|rel| rel.is_empty())
        }
        Target::File(path) | Target::NewFile(path) => {
            cli::relative_to(root, path).is_some_and(|rel| !rel.is_empty())
        }
    }
}

/// Offers `target` to the windows already open. True when one took it.
pub fn hand_off(target: &Target, window: WindowChoice) -> bool {
    let passes: &[Want] = match window {
        WindowChoice::New => return false,
        WindowChoice::Auto => &[Want::Showing, Want::Empty],
        WindowChoice::Reuse => &[Want::Showing, Want::Any],
    };
    offer(passes, target)
}

/// Whether another window of this program already shows `folder` (or the
/// repository it is in). Nothing changes in that window.
// For the multi-window work, which builds on this registry.
#[allow(dead_code)]
pub fn open_elsewhere(folder: &Path) -> bool {
    offer(&[Want::Peek], &Target::Folder(folder.to_owned()))
}

/// Whether any other window of this program is open.
// For the multi-window work, which builds on this registry.
#[allow(dead_code)]
pub fn others_running() -> bool {
    let (Some(dir), Some(exe)) = (registry_dir(), own_exe()) else {
        return false;
    };
    read_entries(&dir, &exe).into_iter().any(|(file, entry)| {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, entry.port));
        let alive = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok();
        if !alive {
            let _ = std::fs::remove_file(&file);
        }
        alive
    })
}

/// Asks the other windows, a pass at a time, until one says yes.
fn offer(passes: &[Want], target: &Target) -> bool {
    let (Some(dir), Some(exe)) = (registry_dir(), own_exe()) else {
        return false;
    };
    let mut entries = read_entries(&dir, &exe);
    for want in passes {
        let mut alive = Vec::new();
        for (file, entry) in entries {
            match ask(&entry, *want, target) {
                Some(true) => return true,
                Some(false) => alive.push((file, entry)),
                // Nothing is listening: that window is gone.
                None => {
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
        entries = alive;
    }
    false
}

/// The other windows' entries, newest first.
fn read_entries(dir: &Path, exe: &Path) -> Vec<(PathBuf, Entry)> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<_> = listing
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            if path.extension().is_none_or(|e| e != "json") {
                return None;
            }
            let modified = item.metadata().and_then(|m| m.modified()).ok();
            let entry: Entry = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            if entry.exe != exe {
                // Other copies tidy up after themselves, unless they are gone
                // altogether (a deleted build, say).
                if !entry.exe.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                return None;
            }
            (entry.protocol == PROTOCOL && entry.pid != std::process::id())
                .then_some((modified, path, entry))
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found
        .into_iter()
        .map(|(_, path, entry)| (path, entry))
        .collect()
}

/// `None` when the window can't be reached at all.
fn ask(entry: &Entry, want: Want, target: &Target) -> Option<bool> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, entry.port));
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok()?;
    // If it takes the path it raises its window, which Windows only allows
    // when we say so.
    #[cfg(windows)]
    cli::windows::allow_foreground(entry.pid);
    let ask = Ask {
        token: entry.token.clone(),
        want,
        target: target.clone(),
    };
    let reply = (|| {
        stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(REPLY_TIMEOUT)).ok()?;
        let mut line = serde_json::to_string(&ask).ok()?;
        line.push('\n');
        stream.write_all(line.as_bytes()).ok()?;
        let mut answer = String::new();
        BufReader::new(stream.take(MAX_REQUEST))
            .read_line(&mut answer)
            .ok()?;
        serde_json::from_str::<Reply>(&answer).ok()
    })();
    // Something answered the port but not sensibly: count it as a no.
    Some(reply.is_some_and(|r| r.accepted))
}

fn own_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.canonicalize().unwrap_or(exe))
}

/// Where the windows' entries live. Only this user can read it.
fn registry_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("Feather").join("instances"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support/Feather/instances"))
    } else {
        var("XDG_RUNTIME_DIR")
            .map(|d| d.join("feather"))
            .or_else(|| var("HOME").map(|h| h.join(".cache/feather/instances")))
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// 128 unguessable bits. The standard library seeds its hash keys from the
/// operating system's random source, which is all this needs.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..2u8)
        .map(|i| {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u8(i);
            hasher.write_u128(nanos);
            hasher.write_u32(std::process::id());
            format!("{:016x}", hasher.finish())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_windows_take_a_path() {
        let root = std::env::temp_dir().join("feather-instance-root");
        let file = Target::File(root.join("src").join("main.rs"));
        let new_file = Target::NewFile(root.join("todo.md"));
        let outside = Target::File(std::env::temp_dir().join("elsewhere.txt"));

        assert!(accepts(Some(&root), Want::Showing, &file));
        assert!(accepts(Some(&root), Want::Showing, &new_file));
        assert!(!accepts(Some(&root), Want::Showing, &outside));
        assert!(!accepts(None, Want::Showing, &file));

        assert!(accepts(None, Want::Empty, &outside));
        assert!(!accepts(Some(&root), Want::Empty, &outside));

        assert!(accepts(Some(&root), Want::Any, &outside));
        assert!(accepts(None, Want::Any, &outside));
    }

    #[test]
    fn a_folder_is_shown_only_by_its_own_window() {
        let dir = std::env::temp_dir().join(format!("feather-instance-{}", std::process::id()));
        let inner = dir.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let dir = cli::strip_verbatim(dir.canonicalize().unwrap());
        let inner = cli::strip_verbatim(inner.canonicalize().unwrap());
        // Neither is in a repository (the temp folder isn't), so each opens as itself.
        if crate::git::find_root(&dir).is_none() {
            assert!(shows(&dir, &Target::Folder(dir.clone())));
            assert!(!shows(&dir, &Target::Folder(inner.clone())));
            assert!(!shows(&inner, &Target::Folder(dir.clone())));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_differ() {
        let a = new_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, new_token());
    }

    #[test]
    fn a_window_takes_paths_over_the_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let root = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let shared = root.clone();
        std::thread::spawn(move || listen(listener, "secret", &shared, &tx, &|| {}));

        let entry = |token: &str| Entry {
            protocol: PROTOCOL,
            pid: 0,
            port,
            token: token.to_owned(),
            exe: PathBuf::new(),
        };
        let target = Target::NewFile(std::env::temp_dir().join("x.txt"));

        // The wrong token gets no answer.
        assert_eq!(ask(&entry("guess"), Want::Any, &target), Some(false));
        assert!(rx.try_recv().is_err());

        // An empty window takes it; one showing something else does not.
        assert_eq!(ask(&entry("secret"), Want::Empty, &target), Some(true));
        assert_eq!(rx.recv().unwrap().target, target);
        *root.lock().unwrap() = Some(PathBuf::from("/somewhere/else"));
        assert_eq!(ask(&entry("secret"), Want::Empty, &target), Some(false));
        assert!(rx.try_recv().is_err());

        // Peeking gets the answer without the window doing anything.
        *root.lock().unwrap() = Some(std::env::temp_dir());
        assert_eq!(ask(&entry("secret"), Want::Peek, &target), Some(true));
        assert!(rx.try_recv().is_err());
        assert_eq!(ask(&entry("secret"), Want::Showing, &target), Some(true));
        assert_eq!(rx.recv().unwrap().target, target);
    }

    #[test]
    fn an_unreachable_window_is_noticed() {
        // Bind and drop to find a port nothing listens on.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let entry = Entry {
            protocol: PROTOCOL,
            pid: 0,
            port,
            token: String::new(),
            exe: PathBuf::new(),
        };
        let target = Target::Folder(std::env::temp_dir());
        assert_eq!(ask(&entry, Want::Any, &target), None);
    }
}
