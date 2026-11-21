//! Lets `feather <path>` use a window that is already open, the way `code`
//! does with VS Code.
//!
//! Each running Feather listens on a loopback port and writes the port, its
//! process id and a random secret to a file only this user can read. The
//! command reads those files, newest first, and asks each window in turn
//! whether it will take the path. A window says yes when it already shows
//! that folder (or the repository a file is in), or when it shows nothing.
//! Without a yes, the command opens a new window.
//!
//! Loopback ports are open to every account on the machine, and a window that
//! crashed leaves its file behind, so the port it names may since belong to
//! anything. Each side therefore proves it knows the secret (an HMAC over
//! fresh nonces from both ends) before the command sends a path, and the
//! window acts only on a request carrying the command's proof. A port that
//! can't prove it is forgotten.

use crate::cli::{self, Target, WindowChoice};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bumped whenever the messages change, so old and new copies leave each other be.
const PROTOCOL: u32 = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// For a whole exchange, start to finish, on either side.
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(2);
/// Far more than any message needs.
const MAX_LINE: usize = 64 * 1024;
/// Connections handled at once; any beyond this are turned away.
const MAX_CONNECTIONS: usize = 8;

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
    /// Only asking whether the window is there.
    Ping,
}

/// The command's first line: a nonce for the window to prove itself with.
#[derive(Serialize, Deserialize)]
struct Hello {
    nonce: String,
}

/// The window's answer: its own nonce, and proof that it knows the secret.
#[derive(Serialize, Deserialize)]
struct Challenge {
    nonce: String,
    proof: String,
}

/// What the command wants, as JSON in `body`, with the command's proof over
/// the window's nonce and that exact text.
#[derive(Serialize, Deserialize)]
struct Ask {
    body: String,
    proof: String,
}

#[derive(Serialize, Deserialize)]
struct Request {
    want: Want,
    target: Option<Target>,
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
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
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
        let shared = Shared {
            token,
            root: root.clone(),
            tx: Mutex::new(tx),
            wake: Box::new(wake),
        };
        std::thread::Builder::new()
            .name("feather-instance".to_owned())
            .spawn(move || listen(listener, Arc::new(shared)))
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

/// What every connection handler needs.
struct Shared {
    token: String,
    root: Arc<Mutex<Option<PathBuf>>>,
    tx: Mutex<Sender<Handoff>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

fn listen(listener: TcpListener, shared: Arc<Shared>) {
    let busy = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // Each connection gets its own thread and deadline, so one that
        // stalls holds up nobody else.
        if busy.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            busy.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (shared, done) = (shared.clone(), busy.clone());
        let spawned = std::thread::Builder::new()
            .name("feather-instance-request".to_owned())
            .spawn(move || {
                serve(stream, &shared);
                done.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            busy.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// One exchange with a command. Says nothing it shouldn't to anyone who
/// can't prove they know the secret.
fn serve(stream: TcpStream, shared: &Shared) -> Option<()> {
    let deadline = Instant::now() + EXCHANGE_DEADLINE;
    let mut stream = Line::new(stream, deadline);
    let hello: Hello = serde_json::from_str(&stream.read()?).ok()?;
    let nonce = new_token();
    let proof = hmac_hex(&shared.token, &["server", &hello.nonce, &nonce]);
    stream.write(
        &serde_json::to_string(&Challenge {
            nonce: nonce.clone(),
            proof,
        })
        .ok()?,
    )?;

    let ask: Ask = serde_json::from_str(&stream.read()?).ok()?;
    let expected = hmac_hex(&shared.token, &["client", &nonce, &ask.body]);
    if !same(&expected, &ask.proof) {
        return None;
    }
    let request: Request = serde_json::from_str(&ask.body).ok()?;
    let current = shared.root.lock().ok().and_then(|r| r.clone());
    let accepted = match &request.target {
        None => request.want == Want::Ping,
        Some(target) => accepts(current.as_deref(), request.want, target),
    };
    if accepted
        && !matches!(request.want, Want::Peek | Want::Ping)
        && let Some(target) = request.target
        && shared
            .tx
            .lock()
            .is_ok_and(|tx| tx.send(Handoff { target }).is_ok())
    {
        (shared.wake)();
    }
    stream.write(&serde_json::to_string(&Reply { accepted }).ok()?)
}

/// Whether a window showing `root` should take `target`.
fn accepts(root: Option<&Path>, want: Want, target: &Target) -> bool {
    match (want, root) {
        (Want::Any | Want::Ping, _) | (Want::Empty, None) => true,
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
    offer(passes, Some(target))
}

/// Whether another window of this program already shows `folder` (or the
/// repository it is in). Nothing changes in that window.
// For the multi-window work, which builds on this registry.
#[allow(dead_code)]
pub fn open_elsewhere(folder: &Path) -> bool {
    offer(&[Want::Peek], Some(&Target::Folder(folder.to_owned())))
}

/// Whether any other window of this program is open.
// For the multi-window work, which builds on this registry.
#[allow(dead_code)]
pub fn others_running() -> bool {
    offer(&[Want::Ping], None)
}

/// How asking one window went.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Declined,
    /// Nothing there, or something that isn't that window.
    Gone,
}

/// Asks the other windows, a pass at a time, until one says yes.
fn offer(passes: &[Want], target: Option<&Target>) -> bool {
    let (Some(dir), Some(exe)) = (registry_dir(), own_exe()) else {
        return false;
    };
    let mut entries = read_entries(&dir, &exe);
    for want in passes {
        let mut alive = Vec::new();
        for (file, entry) in entries {
            match ask(&entry, *want, target) {
                Outcome::Accepted => return true,
                Outcome::Declined => alive.push((file, entry)),
                Outcome::Gone => {
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

fn ask(entry: &Entry, want: Want, target: Option<&Target>) -> Outcome {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, entry.port));
    // A live window's port takes connections in the kernel, however busy
    // the window is, so failing to connect on loopback means nothing listens.
    let Ok(stream) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        return Outcome::Gone;
    };
    let mut stream = Line::new(stream, Instant::now() + EXCHANGE_DEADLINE);

    // First the window proves itself; only then does it hear the path.
    let nonce = new_token();
    let Some(hello) = serde_json::to_string(&Hello {
        nonce: nonce.clone(),
    })
    .ok() else {
        return Outcome::Gone;
    };
    // No answer (a busy window turns connections away, a slow one runs out
    // of time) says nothing about whether the window is there: keep it.
    let Some(line) = stream.write(&hello).and_then(|()| stream.read()) else {
        return Outcome::Declined;
    };
    // An answer that isn't a proof of the secret is something else on the
    // port: the window that wrote this entry has gone.
    let Ok(challenge) = serde_json::from_str::<Challenge>(&line) else {
        return Outcome::Gone;
    };
    let expected = hmac_hex(&entry.token, &["server", &nonce, &challenge.nonce]);
    if !same(&expected, &challenge.proof) {
        return Outcome::Gone;
    }

    // If it takes the path it raises its window, which Windows only allows
    // when we say so.
    #[cfg(windows)]
    cli::windows::allow_foreground(entry.pid);
    let request = Request {
        want,
        target: target.cloned(),
    };
    let Ok(body) = serde_json::to_string(&request) else {
        return Outcome::Declined;
    };
    let proof = hmac_hex(&entry.token, &["client", &challenge.nonce, &body]);
    let reply = serde_json::to_string(&Ask { body, proof })
        .ok()
        .and_then(|line| stream.write(&line))
        .and_then(|()| stream.read())
        .and_then(|line| serde_json::from_str::<Reply>(&line).ok());
    // It proved itself but then didn't answer: busy, so count it as a no.
    match reply {
        Some(Reply { accepted: true }) => Outcome::Accepted,
        _ => Outcome::Declined,
    }
}

/// Newline-separated messages over a stream, all within one deadline.
struct Line {
    stream: TcpStream,
    deadline: Instant,
}

impl Line {
    fn new(stream: TcpStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    fn remaining(&self) -> Option<Duration> {
        let left = self.deadline.checked_duration_since(Instant::now())?;
        (!left.is_zero()).then_some(left)
    }

    /// One message, or `None` at the deadline, at the size limit, on a
    /// closed connection or on anything that isn't text.
    fn read(&mut self) -> Option<String> {
        let mut bytes = Vec::new();
        let mut byte = [0u8];
        loop {
            self.stream.set_read_timeout(Some(self.remaining()?)).ok()?;
            match self.stream.read(&mut byte) {
                Ok(0) => return None,
                Ok(_) if byte[0] == b'\n' => return String::from_utf8(bytes).ok(),
                Ok(_) if bytes.len() < MAX_LINE => bytes.push(byte[0]),
                Ok(_) => return None,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }

    fn write(&mut self, message: &str) -> Option<()> {
        self.stream
            .set_write_timeout(Some(self.remaining()?))
            .ok()?;
        let mut line = message.as_bytes().to_vec();
        line.push(b'\n');
        self.stream.write_all(&line).ok()
    }
}

/// HMAC-SHA256 (RFC 2104) of `parts`, each ending in a newline, as hex.
fn hmac_hex(key: &str, parts: &[&str]) -> String {
    let mut message = Vec::new();
    for part in parts {
        message.extend_from_slice(part.as_bytes());
        message.push(b'\n');
    }
    hmac(key.as_bytes(), &message)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// Compares two proofs without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
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
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        let mac = hmac(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }

    /// A window with secret "secret", and what it was handed.
    fn window() -> (u16, Arc<Mutex<Option<PathBuf>>>, Receiver<Handoff>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let root = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::channel();
        let shared = Shared {
            token: "secret".to_owned(),
            root: root.clone(),
            tx: Mutex::new(tx),
            wake: Box::new(|| {}),
        };
        std::thread::spawn(move || listen(listener, Arc::new(shared)));
        (port, root, rx)
    }

    fn entry(port: u16, token: &str) -> Entry {
        Entry {
            protocol: PROTOCOL,
            pid: 0,
            port,
            token: token.to_owned(),
            exe: PathBuf::new(),
        }
    }

    #[test]
    fn a_window_takes_paths_over_the_socket() {
        let (port, root, rx) = window();
        let target = Target::NewFile(std::env::temp_dir().join("x.txt"));
        let some = Some(&target);

        // Without the secret, the window can't prove itself and is forgotten.
        assert_eq!(ask(&entry(port, "guess"), Want::Any, some), Outcome::Gone);
        assert!(rx.try_recv().is_err());

        // An empty window takes it; one showing something else does not.
        let good = entry(port, "secret");
        assert_eq!(ask(&good, Want::Empty, some), Outcome::Accepted);
        assert_eq!(rx.recv().unwrap().target, target);
        *root.lock().unwrap() = Some(PathBuf::from("/somewhere/else"));
        assert_eq!(ask(&good, Want::Empty, some), Outcome::Declined);
        assert!(rx.try_recv().is_err());

        // Peeking and pinging get answers without the window doing anything.
        *root.lock().unwrap() = Some(std::env::temp_dir());
        assert_eq!(ask(&good, Want::Peek, some), Outcome::Accepted);
        assert_eq!(ask(&good, Want::Ping, None), Outcome::Accepted);
        assert!(rx.try_recv().is_err());
        assert_eq!(ask(&good, Want::Showing, some), Outcome::Accepted);
        assert_eq!(rx.recv().unwrap().target, target);
    }

    #[test]
    fn the_path_is_never_sent_to_an_impostor() {
        // Something else on a port a crashed window left behind.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = Line::new(stream, Instant::now() + EXCHANGE_DEADLINE);
            let hello = line.read().unwrap();
            let fake = Challenge {
                nonce: "n".to_owned(),
                proof: "0".repeat(64),
            };
            line.write(&serde_json::to_string(&fake).unwrap()).unwrap();
            let mut rest = String::new();
            while let Some(more) = line.read() {
                rest.push_str(&more);
            }
            (hello, rest)
        });
        let target = Target::File(PathBuf::from("/private/plans.txt"));
        assert_eq!(
            ask(&entry(port, "secret"), Want::Any, Some(&target)),
            Outcome::Gone
        );
        let (hello, rest) = heard.join().unwrap();
        assert!(!hello.contains("plans") && !hello.contains("secret"));
        assert!(rest.is_empty(), "sent after a bad proof: {rest}");
    }

    #[test]
    fn a_stalled_connection_holds_up_nobody() {
        let (port, _root, _rx) = window();
        // Connects and says nothing.
        let _stalled = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let started = Instant::now();
        assert_eq!(
            ask(&entry(port, "secret"), Want::Ping, None),
            Outcome::Accepted
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_window_that_is_too_busy_is_kept() {
        // Accepts, then hangs up without a word, as a window at its limit does.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });
        assert_eq!(
            ask(&entry(port, "secret"), Want::Ping, None),
            Outcome::Declined
        );
    }

    #[test]
    fn an_unreachable_window_is_noticed() {
        // Bind and drop to find a port nothing listens on.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let target = Target::Folder(std::env::temp_dir());
        assert_eq!(
            ask(&entry(port, ""), Want::Any, Some(&target)),
            Outcome::Gone
        );
    }
}
