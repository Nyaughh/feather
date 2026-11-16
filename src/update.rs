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
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const REPO: &str = "Nyaughh/feather";
pub const RELEASES_PAGE: &str = "https://github.com/Nyaughh/feather/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often to look again while the app stays open.
const RECHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Nothing we ship is anywhere near this; it only bounds a bad response.
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;

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
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Message::Installed(install(&release, &exe)));
            ctx.request_repaint();
        });
    }

    /// Applies results from the background thread.
    pub fn poll(&mut self) {
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
        std::process::Command::new(exe)
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

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(concat!("feather/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
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
    let body = agent()
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
        _ => "Could not reach GitHub. Check your internet connection.".to_owned(),
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

fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent().get(url).call().map_err(|e| network_error(&e))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The download was interrupted.".to_owned())?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err("The download was larger than expected.".to_owned());
    }
    Ok(bytes)
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

fn install(release: &Release, exe: &Path) -> Result<(), String> {
    let (Some(name), Some(asset_url), Some(sums_url)) =
        (ASSET, &release.asset_url, &release.checksums_url)
    else {
        return Err("This release has no download for this computer.".to_owned());
    };

    let sums = String::from_utf8(download(sums_url)?)
        .map_err(|_| "The checksum list was unreadable.".to_owned())?;
    let expected = expected_checksum(&sums, name)
        .ok_or("The release does not list a checksum for this download.")?
        .to_ascii_lowercase();
    let bytes = download(asset_url)?;
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
}
