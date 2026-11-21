//! In-app updates.
//!
//! Feather asks GitHub for the latest release, and if it is newer than this
//! build, offers to install it: the right download for this computer is
//! fetched, checked against the release's SHA-256 list, and swapped in for the
//! running program. The new version starts on the next launch (or right away,
//! if the user chooses to restart).
//!
//! All network and disk work happens on a background thread.

use eframe::egui;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const REPO: &str = "Nyaughh/feather";
pub const RELEASES_PAGE: &str = "https://github.com/Nyaughh/feather/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often to look again while the app stays open.
const RECHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Nothing we ship is anywhere near this; it only bounds a bad response.
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Asking GitHub about the latest release is one small reply.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// Setting up a connection, and then waiting for GitHub to start answering.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// A download takes as long as the connection needs (the program is over
/// 16 MB, and GitHub's downloads can run at a few dozen KB/s), but gives up
/// once nothing at all has arrived for this long.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounds a download that never quite stalls but never finishes either.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(60 * 60);

/// The release file for this kind of computer, if we publish one.
const ASSET: Option<&str> = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
    Some("feather-windows-x86_64.exe")
} else if cfg!(target_os = "macos") {
    Some("feather-macos-universal.zip")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("feather-linux-x86_64.tar.gz")
} else {
    None
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    pub page: String,
    /// Download for this computer; `None` if the release has none.
    asset_url: Option<String>,
    checksums_url: Option<String>,
}

impl Release {
    /// Whether Feather can install this release itself.
    pub fn installable(&self) -> bool {
        self.asset_url.is_some() && self.checksums_url.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// Installed; takes effect when Feather next starts.
    Ready(Release),
    CheckFailed(String),
    InstallFailed(Release, String),
}

enum Message {
    Checked(Result<Option<Release>, String>),
    Installed(Result<(), String>),
}

/// How far an install has got, shared with the thread doing it.
#[derive(Default)]
struct Progress {
    /// Bytes of the program downloaded so far.
    received: AtomicU64,
    /// Its size, or 0 while unknown.
    total: AtomicU64,
    /// Downloaded and checked; now being written over the installed copy.
    installing: AtomicBool,
}

impl Progress {
    fn describe(&self) -> String {
        if self.installing.load(Ordering::Relaxed) {
            return "Checking and installing the new version.".to_owned();
        }
        let total = self.total.load(Ordering::Relaxed);
        describe_download(
            self.received.load(Ordering::Relaxed),
            (total > 0).then_some(total),
        )
    }
}

pub struct Updater {
    pub state: State,
    /// The user closed the prompt for this session.
    pub dismissed: bool,
    /// Set when a check the user asked for finishes, until it is reported.
    pub manual_result: bool,
    manual: bool,
    last_check: Option<Instant>,
    /// Where this program lives, recorded before any update replaces it.
    exe: Option<PathBuf>,
    /// The install under way, or the last one.
    progress: Arc<Progress>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let exe = std::env::current_exe().ok();
        if let Some(exe) = &exe {
            clean_up_after_update(exe);
        }
        Self {
            state: State::Idle,
            dismissed: false,
            manual_result: false,
            manual: false,
            last_check: None,
            exe,
            progress: Arc::default(),
            tx,
            rx,
            ctx: ctx.clone(),
        }
    }

    fn busy(&self) -> bool {
        matches!(self.state, State::Checking | State::Installing(_))
    }

    /// Starts a check. `manual` is true when the user asked for it, in which
    /// case the outcome is always reported, including "up to date".
    pub fn check(&mut self, manual: bool) {
        if self.busy() || matches!(self.state, State::Ready(_)) {
            return;
        }
        self.manual = manual;
        if manual {
            self.dismissed = false;
        }
        self.last_check = Some(Instant::now());
        self.state = State::Checking;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Message::Checked(latest_release()));
            ctx.request_repaint();
        });
    }

    /// Runs the periodic check when one is due.
    pub fn check_if_due(&mut self) {
        let due = self
            .last_check
            .is_none_or(|at| at.elapsed() >= RECHECK_EVERY);
        if due {
            self.check(false);
        }
        self.ctx
            .request_repaint_after(RECHECK_EVERY + Duration::from_secs(1));
    }

    pub fn install(&mut self) {
        let (State::Available(release) | State::InstallFailed(release, _)) = &self.state else {
            return;
        };
        let release = release.clone();
        let Some(exe) = self.exe.clone() else {
            self.state = State::InstallFailed(
                release,
                "Feather could not work out where it is installed.".to_owned(),
            );
            return;
        };
        self.dismissed = false;
        self.state = State::Installing(release.clone());
        let progress = Arc::new(Progress::default());
        self.progress = Arc::clone(&progress);
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Message::Installed(install(&release, &exe, &progress)));
            ctx.request_repaint();
        });
    }

    /// What the install under way is doing, such as how much is downloaded.
    pub fn install_status(&self) -> String {
        self.progress.describe()
    }

    /// Applies results from the background thread.
    pub fn poll(&mut self) {
        if matches!(self.state, State::Installing(_)) {
            // Keeps the download's progress moving on screen.
            self.ctx.request_repaint_after(Duration::from_millis(250));
        }
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Checked(result) => {
                    self.manual_result = self.manual;
                    self.state = match result {
                        Ok(Some(release)) => State::Available(release),
                        Ok(None) => State::UpToDate,
                        Err(e) => State::CheckFailed(e),
                    };
                }
                Message::Installed(result) => {
                    let release = match &self.state {
                        State::Installing(release) => release.clone(),
                        _ => continue,
                    };
                    self.state = match result {
                        Ok(()) => State::Ready(release),
                        Err(e) => State::InstallFailed(release, e),
                    };
                }
            }
        }
    }

    /// Starts the newly installed version. The caller then closes this one.
    pub fn relaunch(&self) -> Result<(), String> {
        let exe = self.exe.as_ref().ok_or("unknown install location")?;
        // `--wait` runs the new copy in a window of its own, instead of handing
        // the path to this one as it closes.
        std::process::Command::new(exe)
            .arg("--wait")
            .args(std::env::args_os().skip(1))
            .spawn()
            .map(drop)
            .map_err(|e| format!("Could not restart Feather: {e}"))
    }
}

// ── Checking ───────────────────────────────────────────────────────────────

/// Parses `1.2.3` or `v1.2.3` into numbers that compare correctly.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    let text = text.trim().trim_start_matches('v');
    let core = text.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// For asking GitHub about releases: small replies, so one overall limit.
fn check_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_global(Some(CHECK_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .user_agent(concat!("feather/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// For downloads. A slow connection still gets there, so there is no overall
/// limit short of [`DOWNLOAD_LIMIT`]; a download fails once nothing has
/// arrived for [`STALL_TIMEOUT`].
fn download_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_per_read(Some(STALL_TIMEOUT))
        .timeout_global(Some(DOWNLOAD_LIMIT))
        .user_agent(concat!("feather/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn api_url() -> String {
    // Debug builds can be pointed at a local server to test the update flow.
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("FEATHER_UPDATE_API") {
        return url;
    }
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

fn current_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(version) = std::env::var("FEATHER_FAKE_VERSION") {
        return version;
    }
    CURRENT_VERSION.to_owned()
}

/// The latest release, if it is newer than this build.
fn latest_release() -> Result<Option<Release>, String> {
    let body = ureq::Agent::new_with_config(check_config())
        .get(&api_url())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| network_error(&e))?
        .body_mut()
        .read_to_string()
        .map_err(|e| network_error(&e))?;
    parse_release(&body, &current_version())
}

fn network_error(error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(404) => "No release has been published yet.".to_owned(),
        ureq::Error::StatusCode(403 | 429) => {
            "GitHub is limiting requests from this network. Try again later.".to_owned()
        }
        ureq::Error::StatusCode(code) => format!("GitHub answered with an error ({code})."),
        ureq::Error::Timeout(_) => {
            "GitHub took too long to answer. Check your internet connection and try again."
                .to_owned()
        }
        _ => format!("Could not reach GitHub ({error}). Check your internet connection."),
    }
}

fn parse_release(json: &str, current: &str) -> Result<Option<Release>, String> {
    let unexpected = || "GitHub sent a reply Feather did not understand.".to_owned();
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| unexpected())?;
    let tag = value["tag_name"].as_str().ok_or_else(unexpected)?;
    if !is_newer(tag, current) {
        return Ok(None);
    }
    let asset = |name: &str| {
        value["assets"].as_array()?.iter().find_map(|asset| {
            (asset["name"].as_str()? == name)
                .then(|| asset["browser_download_url"].as_str().map(str::to_owned))?
        })
    };
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        page: value["html_url"]
            .as_str()
            .unwrap_or(RELEASES_PAGE)
            .to_owned(),
        asset_url: ASSET.and_then(asset),
        checksums_url: asset("SHA256SUMS.txt"),
    }))
}

// ── Installing ─────────────────────────────────────────────────────────────

/// Downloads `url`, counting what arrives in `progress` if given.
fn download(url: &str, progress: Option<&Progress>) -> Result<Vec<u8>, String> {
    download_with(download_config(), url, progress)
}

fn download_with(
    config: ureq::config::Config,
    url: &str,
    progress: Option<&Progress>,
) -> Result<Vec<u8>, String> {
    let mut response = ureq::Agent::new_with_config(config)
        .get(url)
        .call()
        .map_err(|e| network_error(&e))?;
    let total = response.body().content_length();
    if total.is_some_and(|total| total > MAX_DOWNLOAD_BYTES) {
        return Err(TOO_LARGE.to_owned());
    }
    let fallback = Progress::default();
    let progress = progress.unwrap_or(&fallback);
    progress.received.store(0, Ordering::Relaxed);
    progress.total.store(total.unwrap_or(0), Ordering::Relaxed);
    read_body(response.body_mut().as_reader(), progress)
}

const TOO_LARGE: &str = "The download was larger than expected.";

/// Reads a whole download, keeping `progress` up to date.
fn read_body(mut reader: impl Read, progress: &Progress) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let total = progress.total.load(Ordering::Relaxed);
                return Err(read_error(
                    &e,
                    bytes.len() as u64,
                    (total > 0).then_some(total),
                ));
            }
        };
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err(TOO_LARGE.to_owned());
        }
        progress
            .received
            .store(bytes.len() as u64, Ordering::Relaxed);
    }
    Ok(bytes)
}

/// Explains a download that failed partway, saying how far it got.
fn read_error(error: &std::io::Error, received: u64, total: Option<u64>) -> String {
    let timeout = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .and_then(|inner| match inner {
            ureq::Error::Timeout(timeout) => Some(*timeout),
            _ => None,
        });
    let got = amount(received, total);
    let advice = "Try again, or download it from the website.";
    match timeout {
        Some(ureq::Timeout::PerRead) => format!(
            "The download stalled: nothing arrived from GitHub for {} seconds, after {got}. {advice}",
            STALL_TIMEOUT.as_secs()
        ),
        Some(_) => format!(
            "The download took longer than {} minutes and was stopped after {got}. {advice}",
            DOWNLOAD_LIMIT.as_secs() / 60
        ),
        None if error.kind() == std::io::ErrorKind::TimedOut => {
            format!("The download stalled after {got}. {advice}")
        }
        None => format!("The download was interrupted after {got} ({error}). {advice}"),
    }
}

/// "3.2 of 16.7 MB", or just "3.2 MB" when the size isn't known.
fn amount(received: u64, total: Option<u64>) -> String {
    let mb = |bytes: u64| format!("{:.1}", bytes as f64 / 1_000_000.0);
    match total {
        Some(total) => format!("{} of {} MB", mb(received), mb(total)),
        None => format!("{} MB", mb(received)),
    }
}

/// Describes a download in progress for the update card and settings.
fn describe_download(received: u64, total: Option<u64>) -> String {
    match total {
        None if received == 0 => "Starting the download.".to_owned(),
        Some(total) => {
            let percent = (received.min(total) * 100) / total;
            format!(
                "Downloading: {} ({percent}%).",
                amount(received, Some(total))
            )
        }
        None => format!("Downloading: {}.", amount(received, None)),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Finds `name` in a `sha256sum`-style list and returns its digest.
fn expected_checksum<'a>(list: &'a str, name: &str) -> Option<&'a str> {
    list.lines().find_map(|line| {
        let (digest, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(digest)
    })
}

fn install(release: &Release, exe: &Path, progress: &Progress) -> Result<(), String> {
    let (Some(name), Some(asset_url), Some(sums_url)) =
        (ASSET, &release.asset_url, &release.checksums_url)
    else {
        return Err("This release has no download for this computer.".to_owned());
    };

    let sums = String::from_utf8(download(sums_url, None)?)
        .map_err(|_| "The checksum list was unreadable.".to_owned())?;
    let expected = expected_checksum(&sums, name)
        .ok_or("The release does not list a checksum for this download.")?
        .to_ascii_lowercase();
    let bytes = download(asset_url, Some(progress))?;
    progress.installing.store(true, Ordering::Relaxed);
    if sha256_hex(&bytes) != expected {
        return Err("The download did not match its checksum, so it was not installed.".to_owned());
    }

    replace_program(&bytes, exe).map_err(|e| {
        format!("Could not replace the installed copy ({e}). You can download it from the website instead.")
    })
}

/// Writes the new program over the running one.
#[cfg(target_os = "windows")]
fn replace_program(download: &[u8], _exe: &Path) -> Result<(), String> {
    let staged = std::env::temp_dir().join(format!("feather-update-{}.exe", std::process::id()));
    std::fs::write(&staged, download).map_err(|e| e.to_string())?;
    let result = self_replace::self_replace(&staged).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&staged);
    result
}

#[cfg(all(unix, not(target_os = "macos")))]
fn replace_program(download: &[u8], exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    // The archive holds a single file, the program itself.
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(download));
    let mut program = Vec::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let is_program = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "feather"))
            .unwrap_or(false);
        if is_program {
            entry.read_to_end(&mut program).map_err(|e| e.to_string())?;
            break;
        }
    }
    if program.is_empty() {
        return Err("the download did not contain the program".to_owned());
    }

    // Stage next to the installed copy so the final step is a same-disk rename.
    let dir = exe.parent().ok_or("no install folder")?;
    let staged = dir.join(format!(".feather-update-{}", std::process::id()));
    std::fs::write(&staged, &program).map_err(|e| e.to_string())?;
    let result = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&staged, exe))
        .map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// On macOS the download is a whole `Feather.app`, which replaces the bundle
/// this program is running from.
#[cfg(target_os = "macos")]
fn replace_program(download: &[u8], exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    // <bundle>.app/Contents/MacOS/feather
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("Feather is not running from an app bundle")?;
    let parent = bundle.parent().ok_or("no install folder")?;
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Err("move Feather into Applications first".to_owned());
    }

    let pid = std::process::id();
    let staging = parent.join(format!(".feather-update-{pid}"));
    let retired = parent.join(format!(".feather-old-{pid}"));
    let _ = std::fs::remove_dir_all(&staging);
    let unpack = || -> Result<PathBuf, String> {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(download)).map_err(|e| e.to_string())?;
        archive.extract(&staging).map_err(|e| e.to_string())?;
        let new_bundle = staging.join("Feather.app");
        let program = new_bundle.join("Contents/MacOS/feather");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        Ok(new_bundle)
    };
    let new_bundle = unpack().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;

    // Swap the bundles, putting the old one back if the second step fails.
    std::fs::rename(bundle, &retired).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new_bundle, bundle) {
        let _ = std::fs::rename(&retired, bundle);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&retired);
    Ok(())
}

/// Removes anything a previous update left behind.
fn clean_up_after_update(exe: &Path) {
    #[cfg(target_os = "macos")]
    if let Some(parent) = exe.ancestors().nth(4) {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".feather-old-") || name.starts_with(".feather-update-") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    let _ = exe;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(is_newer("v1.2.3-beta.1", "1.2.2"));
    }

    #[test]
    fn reads_a_release_and_ignores_older_ones() {
        let json = r#"{
            "tag_name": "v0.3.0",
            "html_url": "https://example.test/release",
            "assets": [
                {"name": "feather-linux-x86_64.tar.gz", "browser_download_url": "https://example.test/linux"},
                {"name": "feather-macos-universal.zip", "browser_download_url": "https://example.test/mac"},
                {"name": "feather-windows-x86_64.exe", "browser_download_url": "https://example.test/win"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/sums"}
            ]
        }"#;
        let release = parse_release(json, "0.2.0").unwrap().expect("newer");
        assert_eq!(release.version, "0.3.0");
        assert_eq!(release.page, "https://example.test/release");
        assert_eq!(
            release.checksums_url.as_deref(),
            Some("https://example.test/sums")
        );
        if ASSET.is_some() {
            assert!(release.installable());
        }
        assert_eq!(parse_release(json, "0.3.0").unwrap(), None);
        assert_eq!(parse_release(json, "1.0.0").unwrap(), None);
        assert!(parse_release("not json", "0.1.0").is_err());
    }

    #[test]
    fn finds_checksums_and_hashes() {
        let list = "aaa111  feather-linux-x86_64.tar.gz\nbbb222 *feather-windows-x86_64.exe\n";
        assert_eq!(
            expected_checksum(list, "feather-linux-x86_64.tar.gz"),
            Some("aaa111")
        );
        assert_eq!(
            expected_checksum(list, "feather-windows-x86_64.exe"),
            Some("bbb222")
        );
        assert_eq!(expected_checksum(list, "missing"), None);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn downloads_only_give_up_when_they_stall() {
        // 0.6.1 gave the whole download two minutes, so a 16 MB program over
        // a slow link to GitHub never arrived.
        let download = download_config();
        assert_eq!(download.timeout_per_read(), Some(STALL_TIMEOUT));
        assert_eq!(download.timeouts().global, Some(DOWNLOAD_LIMIT));
        assert!(download.timeouts().recv_body.is_none());
        assert!(DOWNLOAD_LIMIT >= Duration::from_secs(30 * 60));
        // 17 MB within the limit needs only about 10 KB/s.
        assert!(17_000_000 / DOWNLOAD_LIMIT.as_secs() < 10_000);
        assert_eq!(check_config().timeouts().global, Some(CHECK_TIMEOUT));
    }

    /// Serves one request on a local port: the headers, then `body` a piece
    /// at a time with `pause` before each piece, then `hang` with the
    /// connection open.
    fn serve(length: usize, pieces: Vec<&'static [u8]>, pause: Duration, hang: Duration) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/feather", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK
Content-Length: {length}
Connection: close

"
            );
            for piece in pieces {
                std::thread::sleep(pause);
                let _ = stream.write_all(piece);
                let _ = stream.flush();
            }
            std::thread::sleep(hang);
        });
        url
    }

    fn quick_config(stall: Duration) -> ureq::config::Config {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_recv_response(Some(Duration::from_secs(5)))
            .timeout_per_read(Some(stall))
            .build()
    }

    #[test]
    fn a_slow_download_finishes_and_reports_progress() {
        // Every piece arrives within the stall limit, though the whole takes
        // several times longer than it.
        let pieces: Vec<&'static [u8]> = vec![b"fe", b"at", b"he", b"r!"];
        let url = serve(8, pieces, Duration::from_millis(150), Duration::ZERO);
        let progress = Progress::default();
        let bytes = download_with(
            quick_config(Duration::from_millis(400)),
            &url,
            Some(&progress),
        )
        .expect("a slow but steady download completes");
        assert_eq!(bytes, b"feather!");
        assert_eq!(progress.received.load(Ordering::Relaxed), 8);
        assert_eq!(progress.total.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn a_stalled_download_fails_with_a_clear_error() {
        let url = serve(
            1_000_000,
            vec![b"partial"],
            Duration::ZERO,
            Duration::from_secs(3),
        );
        let progress = Progress::default();
        let started = Instant::now();
        let error = download_with(
            quick_config(Duration::from_millis(300)),
            &url,
            Some(&progress),
        )
        .expect_err("a download that stops arriving fails");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "gave up promptly"
        );
        assert!(error.contains("stalled"), "{error}");
        assert!(error.contains("of 1.0 MB"), "{error}");
        assert!(error.contains("download it from the website"), "{error}");
        assert_eq!(progress.received.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn a_download_cut_short_says_so() {
        // The server closes the connection with most of the body missing.
        let url = serve(100, vec![b"short"], Duration::ZERO, Duration::ZERO);
        let error = download_with(quick_config(Duration::from_secs(2)), &url, None)
            .expect_err("a truncated download fails");
        assert!(error.starts_with("The download was interrupted"), "{error}");
    }

    #[test]
    fn explains_why_a_download_failed() {
        let stalled = std::io::Error::other(ureq::Error::Timeout(ureq::Timeout::PerRead));
        let message = read_error(&stalled, 4_200_000, Some(16_659_968));
        assert!(
            message.starts_with(
                "The download stalled: nothing arrived from GitHub for 30 seconds, after 4.2 of 16.7 MB."
            ),
            "{message}"
        );
        let too_long = std::io::Error::other(ureq::Error::Timeout(ureq::Timeout::Global));
        assert!(read_error(&too_long, 0, None).contains("took longer than 60 minutes"));
        let reset = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        assert!(
            read_error(&reset, 1_000_000, None)
                .starts_with("The download was interrupted after 1.0 MB (")
        );
        assert!(network_error(&ureq::Error::Timeout(ureq::Timeout::Connect)).contains("too long"));
    }

    #[test]
    fn describes_progress() {
        assert_eq!(describe_download(0, None), "Starting the download.");
        assert_eq!(
            describe_download(4_200_000, Some(16_659_968)),
            "Downloading: 4.2 of 16.7 MB (25%)."
        );
        assert_eq!(describe_download(1_500_000, None), "Downloading: 1.5 MB.");
        let progress = Progress::default();
        progress.total.store(10, Ordering::Relaxed);
        progress.received.store(10, Ordering::Relaxed);
        assert_eq!(progress.describe(), "Downloading: 0.0 of 0.0 MB (100%).");
        progress.installing.store(true, Ordering::Relaxed);
        assert_eq!(
            progress.describe(),
            "Checking and installing the new version."
        );
    }

    #[test]
    fn read_body_counts_bytes() {
        let progress = Progress::default();
        let body = vec![7u8; 200_000];
        assert_eq!(read_body(&body[..], &progress).unwrap(), body);
        assert_eq!(progress.received.load(Ordering::Relaxed), 200_000);
    }
}
