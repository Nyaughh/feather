//! Keeps settings in one file that every Feather window shares, and decides
//! which window looks after eframe's own saved state.
//!
//! Each window is its own process with its own copy of the settings. When a
//! window writes, it re-reads the file under a lock and changes only the
//! settings it changed itself, so two windows never undo each other's changes.
//! Settings it doesn't know (saved by a newer version) are left as they are.
//! A window also picks up changes made by the others (a new theme, a folder
//! another window opened) when it next looks at the file.

use serde_json::{Map, Value};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::settings::{MAX_RECENT, Settings};

const FILE_NAME: &str = "settings.json";
/// How long settings sit unwritten after they change, so dragging a slider
/// doesn't write the file on every frame. Also the wait before trying again
/// when a write fails.
const WRITE_DELAY: f64 = 0.5;
/// How often to check whether another window has written the file.
const CHECK_EVERY: f64 = 1.0;

/// Settings that describe this window rather than the user's preferences.
/// Each window keeps its own. A change here is still written, so the next
/// new window starts the same way.
const WINDOW_KEYS: [&str; 3] = ["sidebar_open", "history_open", "terminal_dock"];

pub struct Store {
    /// `None` when there is nowhere to keep the file, which leaves settings
    /// for this run only.
    path: Option<PathBuf>,
    /// The settings as of the last successful read or write.
    base: Settings,
    /// The file's text as of then (`None`: there was no file), to notice
    /// when another window writes it.
    seen: Option<String>,
    /// When the settings first differed from `base`.
    changed_at: Option<f64>,
    last_check: f64,
}

/// Where the settings file lives: beside eframe's own saved state.
pub fn default_path() -> Option<PathBuf> {
    eframe::storage_dir("feather").map(|dir| dir.join(FILE_NAME))
}

impl Store {
    /// Reads the settings file. Without one, `legacy` (what an older version
    /// saved) is moved into a new file; with neither, the defaults are used
    /// and nothing is written until something changes.
    pub fn open(path: Option<PathBuf>, legacy: Option<Settings>) -> (Self, Settings) {
        let mut fallback = legacy.clone().unwrap_or_default();
        prepare(&mut fallback);
        let mut store = Self {
            path: path.clone(),
            base: fallback.clone(),
            seen: None,
            changed_at: None,
            last_check: f64::NEG_INFINITY,
        };
        let Some(path) = path else {
            return (store, fallback);
        };
        // Read and, if need be, created under the lock, so a window starting
        // at the same moment sees either nothing or the whole migration.
        let lock = lock(&path);
        match read_text(&path) {
            Ok(Some(text)) => {
                if let Some((_, settings)) = decode(&text, &fallback) {
                    store.base = settings;
                }
                store.seen = Some(text);
            }
            Ok(None) => {
                if let (Some(_), Some(_)) = (&legacy, &lock)
                    && let Some(text) = encode(&to_map(&fallback).unwrap_or_default())
                    && write_text(&path, &text).is_ok()
                {
                    store.seen = Some(text);
                }
            }
            // Unreadable for now: start from the fallback, and take the file
            // in once it can be read.
            Err(_) => {}
        }
        drop(lock);
        let settings = store.base.clone();
        (store, settings)
    }

    /// Call once a frame. Writes changes once they have settled and picks up
    /// changes from other windows. Returns how long until it next needs a
    /// frame, if it is waiting to write.
    pub fn tick(&mut self, settings: &mut Settings, now: f64) -> Option<f64> {
        if *settings != self.base {
            let since = *self.changed_at.get_or_insert(now);
            let wait = since + WRITE_DELAY - now;
            if wait > 0.0 {
                return Some(wait);
            }
            if !self.sync(settings) {
                // Kept, and tried again shortly.
                self.changed_at = Some(now);
                return Some(WRITE_DELAY);
            }
        } else if now - self.last_check >= CHECK_EVERY {
            self.last_check = now;
            if self.changed_on_disk() {
                self.sync(settings);
            }
        }
        None
    }

    /// Writes any changes now, for when the window is closing.
    pub fn flush(&mut self, settings: &mut Settings) {
        for attempt in 0..5 {
            if *settings == self.base && !self.changed_on_disk() {
                return;
            }
            if self.sync(settings) {
                return;
            }
            // Another window may have the file open for a moment.
            std::thread::sleep(std::time::Duration::from_millis(20 << attempt));
        }
    }

    fn changed_on_disk(&self) -> bool {
        let Some(path) = &self.path else { return false };
        // The text, not the modification time: two writes close together can
        // share a time stamp.
        read_text(path).is_ok_and(|text| text != self.seen)
    }

    /// Merges this window's changes into the file and writes the result.
    /// On success `settings` holds the merged settings; on failure nothing
    /// is changed, so the same changes can be tried again.
    fn sync(&mut self, settings: &mut Settings) -> bool {
        let Some(path) = self.path.clone() else {
            self.base = settings.clone();
            self.changed_at = None;
            return true;
        };
        // Held from reading to replacing, so two windows saving at the same
        // moment take turns.
        let Some(_lock) = lock(&path) else {
            return false;
        };
        let Ok(disk_text) = read_text(&path) else {
            return false;
        };
        let decoded = disk_text.as_deref().and_then(|t| decode(t, &self.base));
        let Some((text, merged)) = merge(&self.base, settings, decoded) else {
            return false;
        };
        if disk_text.as_ref() != Some(&text) && write_text(&path, &text).is_err() {
            return false;
        }
        self.seen = Some(text);
        self.base = merged.clone();
        self.changed_at = None;
        *settings = merged;
        true
    }
}

fn prepare(settings: &mut Settings) {
    settings.migrate();
    settings.sanitize();
}

/// The file's text, or `None` if there is no file.
fn read_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Replaces the file in one step, so a reader never sees half of it.
fn write_text(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, text)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Takes the lock that guards the settings file, waiting for it if another
/// window has it. `None` if it can't be taken.
fn lock(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .ok()?;
    match file.lock() {
        Ok(()) => Some(file),
        // Nothing to take turns with on such a system.
        Err(e) if e.kind() == ErrorKind::Unsupported => Some(file),
        Err(_) => None,
    }
}

fn encode(map: &Map<String, Value>) -> Option<String> {
    serde_json::to_string_pretty(map).ok()
}

/// The settings in a file's text, as its raw keys and as [`Settings`].
/// Values this version can't read (from a newer version, say) keep their
/// `fallback` value in the settings, and their raw value in the map.
/// `None` if the text isn't a settings file at all.
fn decode(text: &str, fallback: &Settings) -> Option<(Map<String, Value>, Settings)> {
    let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let mut settings = match serde_json::from_value::<Settings>(Value::Object(raw.clone())) {
        Ok(settings) => settings,
        Err(_) => {
            // Take what can be read, one setting at a time.
            let mut map = to_map(fallback)?;
            for (key, value) in &raw {
                if !map.contains_key(key) {
                    continue;
                }
                let mut trial = map.clone();
                trial.insert(key.clone(), value.clone());
                if serde_json::from_value::<Settings>(Value::Object(trial.clone())).is_ok() {
                    map = trial;
                }
            }
            serde_json::from_value(Value::Object(map)).ok()?
        }
    };
    prepare(&mut settings);
    Some((raw, settings))
}

fn to_map(settings: &Settings) -> Option<Map<String, Value>> {
    match serde_json::to_value(settings) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Combines the settings this window changed since `base` with the file's
/// (`None` when there is no readable file). Returns the text to write and
/// the settings this window should use: what was written, except that the
/// window keeps its own [`WINDOW_KEYS`].
fn merge(
    base: &Settings,
    local: &Settings,
    disk: Option<(Map<String, Value>, Settings)>,
) -> Option<(String, Settings)> {
    let base_map = to_map(base)?;
    let local_map = to_map(local)?;
    // With no file to merge into, this window's settings are written whole.
    let (mut out, disk) = disk.unwrap_or_else(|| (base_map.clone(), base.clone()));
    for (key, value) in &local_map {
        if base_map.get(key) != Some(value) && key != "recent" {
            out.insert(key.clone(), value.clone());
        }
    }
    if local.recent != base.recent {
        let recent = merge_recent(&base.recent, &local.recent, &disk.recent);
        out.insert("recent".to_owned(), serde_json::to_value(recent).ok()?);
    }
    let text = encode(&out)?;
    let (_, mut merged) = decode(&text, local)?;
    merged.sidebar_open = local.sidebar_open;
    merged.history_open = local.history_open;
    merged.terminal_dock = local.terminal_dock;
    debug_assert_eq!(WINDOW_KEYS.len(), 3, "keep the fields above in step");
    Some((text, merged))
}

/// Recent folders after this window changed its list from `base` to
/// `local` while another window may have changed the file's list to `disk`.
///
/// Folders this window opened (new, or moved ahead of one they used to
/// follow) go first, in this window's order; then the file's list, without
/// anything this window removed.
fn merge_recent(base: &[PathBuf], local: &[PathBuf], disk: &[PathBuf]) -> Vec<PathBuf> {
    if disk == base {
        return local.to_vec();
    }
    let position = |list: &[PathBuf], path: &PathBuf| list.iter().position(|p| p == path);
    let moved_up = |at: usize, path: &PathBuf| {
        let Some(old) = position(base, path) else {
            return true;
        };
        local[at + 1..]
            .iter()
            .any(|later| position(base, later).is_some_and(|o| o < old))
    };
    let mut result: Vec<PathBuf> = local
        .iter()
        .enumerate()
        .filter(|(at, path)| moved_up(*at, path))
        .map(|(_, path)| path.clone())
        .collect();
    let removed = |p: &PathBuf| base.contains(p) && !local.contains(p);
    for path in disk {
        if !removed(path) && !result.contains(path) {
            result.push(path.clone());
        }
    }
    result.truncate(MAX_RECENT);
    result
}

/// Puts `path` at the top of the recent folders.
pub fn touch_recent(recent: &mut Vec<PathBuf>, path: &Path) {
    recent.retain(|p| p != path);
    recent.insert(0, path.to_owned());
    recent.truncate(MAX_RECENT);
}

/// eframe keeps the window's size and place, and egui's own state, in one
/// file (`app.ron`) that it rewrites whole from what it read at startup. So
/// only one window may use it: the first to take this lock, which it holds
/// until it closes. Returns whether this window is that one.
pub fn claim_window_state() -> bool {
    static HELD: OnceLock<File> = OnceLock::new();
    let Some(dir) = eframe::storage_dir("feather") else {
        return true;
    };
    match claim(&dir.join("app.ron.lock")) {
        Claim::Owner(file) => {
            let _ = HELD.set(file);
            true
        }
        Claim::Taken => false,
        // Without a lock, behave as a single window always has.
        Claim::Unavailable => true,
    }
}

/// For the other windows: a file in place of `app.ron` that they never
/// write, so they start without its saved window place.
pub fn spare_window_state() -> Option<PathBuf> {
    eframe::storage_dir("feather").map(|dir| dir.join("window.ron"))
}

enum Claim {
    Owner(File),
    Taken,
    Unavailable,
}

fn claim(path: &Path) -> Claim {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(file) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    else {
        return Claim::Unavailable;
    };
    match file.try_lock() {
        Ok(()) => Claim::Owner(file),
        Err(TryLockError::WouldBlock) => Claim::Taken,
        Err(TryLockError::Error(_)) => Claim::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{TerminalDock, ThemeChoice};

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    /// A settings file in a fresh temporary folder.
    fn temp_file(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "feather-store-{name}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join(FILE_NAME)
    }

    fn read(path: &Path) -> Settings {
        let text = read_text(path).unwrap().unwrap();
        decode(&text, &Settings::default()).unwrap().1
    }

    fn clean_up(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn touching_moves_a_folder_to_the_top() {
        let mut recent = paths(&["a", "b", "c"]);
        touch_recent(&mut recent, Path::new("c"));
        assert_eq!(recent, paths(&["c", "a", "b"]));
        touch_recent(&mut recent, Path::new("d"));
        assert_eq!(recent, paths(&["d", "c", "a", "b"]));
        for i in 0..20 {
            touch_recent(&mut recent, Path::new(&format!("n{i}")));
        }
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], PathBuf::from("n19"));
    }

    #[test]
    fn recents_from_two_windows_are_combined() {
        let base = paths(&["a", "b", "c"]);
        // This window opened x; another opened y meanwhile.
        let local = paths(&["x", "a", "b", "c"]);
        let disk = paths(&["y", "a", "b", "c"]);
        assert_eq!(
            merge_recent(&base, &local, &disk),
            paths(&["x", "y", "a", "b", "c"])
        );
    }

    #[test]
    fn reopening_older_folders_moves_them_up_in_order() {
        let base = paths(&["a", "b", "c"]);
        let local = paths(&["c", "a", "b"]);
        let disk = paths(&["y", "a", "b", "c"]);
        assert_eq!(
            merge_recent(&base, &local, &disk),
            paths(&["c", "y", "a", "b"])
        );
        // Opened b, then c, before the list was saved.
        let local = paths(&["c", "b", "a"]);
        assert_eq!(merge_recent(&base, &local, &base), local);
        assert_eq!(
            merge_recent(&base, &local, &disk),
            paths(&["c", "b", "y", "a"])
        );
    }

    #[test]
    fn removing_a_recent_folder_sticks() {
        let base = paths(&["a", "b", "c"]);
        let local = paths(&["b", "c"]);
        let disk = paths(&["y", "a", "b", "c"]);
        assert_eq!(merge_recent(&base, &local, &disk), paths(&["y", "b", "c"]));
        // Clearing the list keeps only what other windows added since.
        assert_eq!(merge_recent(&base, &[], &disk), paths(&["y"]));
    }

    #[test]
    fn merging_keeps_both_windows_changes() {
        let base = Settings::default();
        let local = Settings {
            font_size: 15.0,
            sidebar_open: false,
            terminal_dock: TerminalDock::Right,
            ..base.clone()
        };
        let disk = Settings {
            theme: ThemeChoice::Dark,
            zoom: 1.25,
            ..base.clone()
        };
        let disk_map = to_map(&disk).unwrap();

        let (text, merged) = merge(&base, &local, Some((disk_map, disk))).unwrap();
        let written = decode(&text, &base).unwrap().1;
        assert_eq!(written.font_size, 15.0);
        assert_eq!(written.theme, ThemeChoice::Dark);
        assert_eq!(written.zoom, 1.25);
        // The window's own layout is saved for new windows...
        assert!(!written.sidebar_open);
        assert_eq!(written.terminal_dock, TerminalDock::Right);
        assert_eq!(merged, written);

        // ...but another window's change to it doesn't reach into this one.
        let local_map = to_map(&local).unwrap();
        let (text, merged) = merge(&base, &base, Some((local_map, local))).unwrap();
        let written = decode(&text, &base).unwrap().1;
        assert_eq!(written.terminal_dock, TerminalDock::Right);
        assert!(merged.sidebar_open);
        assert_eq!(merged.terminal_dock, TerminalDock::Bottom);
        assert_eq!(merged.font_size, 15.0);
    }

    #[test]
    fn settings_from_a_newer_version_survive() {
        let base = Settings::default();
        let mut raw = to_map(&base).unwrap();
        raw.insert("from_the_future".to_owned(), Value::Bool(true));
        raw.insert("theme".to_owned(), Value::String("Sepia".to_owned()));
        raw.insert("zoom".to_owned(), serde_json::json!(1.25));
        let text = encode(&raw).unwrap();

        // What can be read is; what can't keeps this version's value.
        let (raw, disk) = decode(&text, &base).unwrap();
        assert_eq!(disk.zoom, 1.25);
        assert_eq!(disk.theme, base.theme);

        let local = Settings {
            font_size: 16.0,
            ..base.clone()
        };
        let (text, merged) = merge(&base, &local, Some((raw, disk))).unwrap();
        let Value::Object(written) = serde_json::from_str(&text).unwrap() else {
            panic!("not an object");
        };
        assert_eq!(written["from_the_future"], Value::Bool(true));
        assert_eq!(written["theme"], Value::String("Sepia".to_owned()));
        assert_eq!(merged.font_size, 16.0);
        assert_eq!(merged.zoom, 1.25);
    }

    #[test]
    fn two_windows_share_one_file() {
        let path = temp_file("share");
        let (mut one, mut a) = Store::open(Some(path.clone()), Some(Settings::default()));
        let (mut two, mut b) = Store::open(Some(path.clone()), None);
        assert_eq!(a, b);

        a.theme = ThemeChoice::Light;
        touch_recent(&mut a.recent, Path::new("/one"));
        one.flush(&mut a);

        b.font_size = 16.0;
        touch_recent(&mut b.recent, Path::new("/two"));
        two.flush(&mut b);
        assert_eq!(b.theme, ThemeChoice::Light, "picked up the other window's");
        assert_eq!(b.font_size, 16.0);
        assert_eq!(b.recent, paths(&["/two", "/one"]));

        // The first window catches up once it looks at the file again, even
        // if both writes landed within one tick of the file's clock.
        one.flush(&mut a);
        assert_eq!(a, b);
        let (_, fresh) = Store::open(Some(path.clone()), None);
        assert_eq!(fresh, b);
        clean_up(&path);
    }

    #[test]
    fn changes_are_written_once_they_settle() {
        let path = temp_file("settle");
        let (mut store, mut settings) = Store::open(Some(path.clone()), Some(Settings::default()));
        settings.font_size = 14.0;
        assert!(store.tick(&mut settings, 10.0).is_some());
        assert_eq!(read(&path).font_size, 12.5, "not yet");
        assert!(store.tick(&mut settings, 10.2).is_some());
        assert_eq!(store.tick(&mut settings, 10.6), None);
        assert_eq!(read(&path).font_size, 14.0);
        clean_up(&path);
    }

    #[test]
    fn a_failed_write_is_kept_and_tried_again() {
        let path = temp_file("retry");
        let (mut store, mut settings) = Store::open(Some(path.clone()), Some(Settings::default()));
        // A folder where the file should be can't be read, as when another
        // program has it open: nothing is overwritten, and nothing is lost.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        settings.font_size = 18.0;
        assert!(store.tick(&mut settings, 0.0).is_some());
        assert!(
            store.tick(&mut settings, 1.0).is_some(),
            "failed: try again"
        );
        assert!(path.is_dir());
        assert_eq!(settings.font_size, 18.0);
        assert_ne!(store.base, settings);

        fs::remove_dir(&path).unwrap();
        fs::write(&path, "{}").unwrap();
        assert!(store.tick(&mut settings, 1.6).is_none());
        assert_eq!(settings.font_size, 18.0);
        assert_eq!(read(&path).font_size, 18.0);
        clean_up(&path);
    }

    #[test]
    fn an_old_save_moves_into_the_file() {
        let path = temp_file("fallback");
        let old = Settings {
            recent: paths(&["/old"]),
            ..Settings::default()
        };
        let (_, settings) = Store::open(Some(path.clone()), Some(old));
        assert_eq!(settings.recent, paths(&["/old"]));
        assert_eq!(read(&path).recent, paths(&["/old"]));
        // Once the file exists it wins over the old save.
        let (_, settings) = Store::open(Some(path.clone()), Some(Settings::default()));
        assert_eq!(settings.recent, paths(&["/old"]));
        clean_up(&path);
    }

    #[test]
    fn nothing_is_written_until_there_is_something_to_keep() {
        let path = temp_file("empty");
        let (mut store, mut settings) = Store::open(Some(path.clone()), None);
        assert_eq!(read_text(&path).unwrap(), None);
        store.flush(&mut settings);
        assert_eq!(read_text(&path).unwrap(), None);
        settings.zoom = 1.1;
        store.flush(&mut settings);
        assert_eq!(read(&path).zoom, 1.1);
        clean_up(&path);
    }

    #[test]
    fn works_without_a_file() {
        let (mut store, mut settings) = Store::open(None, None);
        settings.zoom = 1.1;
        store.flush(&mut settings);
        assert_eq!(settings.zoom, 1.1);
    }

    #[test]
    fn only_one_window_owns_eframes_state() {
        let path = temp_file("claim").with_file_name("app.ron.lock");
        let Claim::Owner(first) = claim(&path) else {
            panic!("the first claim should win");
        };
        assert!(matches!(claim(&path), Claim::Taken));
        drop(first);
        assert!(matches!(claim(&path), Claim::Owner(_)));
        clean_up(&path);
    }
}
