//! Keeps settings in one file that every Feather window shares, and decides
//! which window looks after eframe's own saved state.
//!
//! Each window is its own process with its own copy of the settings. When a
//! window writes, it re-reads the file under a lock and changes only the
//! settings it changed itself, so two windows never undo each other's changes.
//! Settings it doesn't know (saved by a newer version) are left as they are,
//! and a file it can't read is left alone rather than replaced. A window also
//! picks up changes made by the others (a new theme, a folder another window
//! opened) when it next looks at the file.

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::settings::{MAX_RECENT, Settings};

const FILE_NAME: &str = "settings.json";
/// eframe's own saved state, where versions before this file kept settings.
const LEGACY_FILE_NAME: &str = "app.ron";
/// The key the settings had in it.
const LEGACY_KEY: &str = "feather";
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

/// A change to the list of recent folders. Recorded as what was done rather
/// than as the resulting list, so it can be replayed on top of changes other
/// windows made meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecentOp {
    /// A folder was opened: it goes to the top.
    Open(PathBuf),
    /// A folder was taken off the list.
    Forget(PathBuf),
    Clear,
}

impl RecentOp {
    fn apply(&self, recent: &mut Vec<PathBuf>) {
        match self {
            Self::Open(path) => {
                recent.retain(|p| p != path);
                recent.insert(0, path.clone());
                recent.truncate(MAX_RECENT);
            }
            Self::Forget(path) => recent.retain(|p| p != path),
            Self::Clear => recent.clear(),
        }
    }
}

pub struct Store {
    /// `None` when there is nowhere to keep the file, which leaves settings
    /// for this run only.
    path: Option<PathBuf>,
    /// Where an older version kept settings, to move them over from.
    legacy: Option<PathBuf>,
    /// The settings as of the last successful read or write.
    base: Settings,
    /// The file's text as of then (`None`: there was no file), to notice
    /// when another window writes it.
    seen: Option<String>,
    /// Changes to the recent folders since then.
    recent_ops: Vec<RecentOp>,
    /// When the settings first differed from `base`.
    changed_at: Option<f64>,
    last_check: f64,
    /// The settings came from a file this window could read (or there was
    /// nothing to read). Until then they are only a stand-in, and are never
    /// written over the copy older versions read.
    settled: bool,
}

/// What an older version saved.
enum Legacy {
    /// Nothing: a fresh install.
    Nothing,
    Found(Settings),
    /// There is something, but it can't be read right now.
    Unreadable,
}

impl Store {
    /// The settings file in its usual place, with the older place to move
    /// settings over from.
    pub fn open_default() -> (Self, Settings) {
        let dir = eframe::storage_dir("feather");
        Self::open(
            dir.as_ref().map(|d| d.join(FILE_NAME)),
            dir.as_ref().map(|d| d.join(LEGACY_FILE_NAME)),
        )
    }

    /// Reads the settings file. Without one, the settings an older version
    /// left in `legacy` are moved into a new file (by whichever window gets
    /// there first); with neither, the defaults are used and nothing is
    /// written until something changes.
    pub fn open(path: Option<PathBuf>, legacy: Option<PathBuf>) -> (Self, Settings) {
        let mut store = Self {
            path: path.clone(),
            legacy,
            base: defaults(),
            seen: None,
            recent_ops: Vec::new(),
            changed_at: None,
            last_check: f64::NEG_INFINITY,
            settled: false,
        };
        let Some(path) = path else {
            store.settled = true;
            return (store, defaults());
        };
        // Read and, if need be, created under the lock, so a window starting
        // at the same moment sees either nothing or the whole migration.
        let lock = lock(&path);
        match read_text(&path) {
            Ok(Some(text)) => {
                if let Some((_, settings)) = decode(&text, &store.base) {
                    store.base = settings;
                    store.settled = true;
                }
                store.seen = Some(text);
            }
            Ok(None) => match store.read_legacy() {
                Legacy::Found(settings) => {
                    // Without the lock this stays read-only; a later write
                    // moves it over.
                    if lock.is_some()
                        && let Some(text) = to_map(&settings).as_ref().and_then(encode)
                        && write_text(&path, &text).is_ok()
                    {
                        store.seen = Some(text);
                    }
                    store.base = settings;
                    store.settled = true;
                }
                Legacy::Nothing => store.settled = true,
                // Tried again until it can be read.
                Legacy::Unreadable => {}
            },
            // Unreadable for now: start from the defaults, and take the file
            // in once it can be read.
            Err(_) => {}
        }
        drop(lock);
        let settings = store.base.clone();
        (store, settings)
    }

    /// Records a change to the recent folders and makes it in `settings`.
    pub fn recent(&mut self, settings: &mut Settings, op: RecentOp) {
        op.apply(&mut settings.recent);
        self.recent_ops.push(op);
    }

    fn has_changes(&self, settings: &Settings) -> bool {
        *settings != self.base || !self.recent_ops.is_empty()
    }

    /// Call once a frame. Writes changes once they have settled and picks up
    /// changes from other windows. Returns how long until it next needs a
    /// frame, if it is waiting to write.
    pub fn tick(&mut self, settings: &mut Settings, now: f64) -> Option<f64> {
        if self.has_changes(settings) {
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
            // Also keeps trying to read settings it couldn't at first.
            if !self.settled || self.changed_on_disk() {
                self.sync(settings);
            }
        }
        None
    }

    /// Writes any changes now, for when the window is closing.
    pub fn flush(&mut self, settings: &mut Settings) {
        for attempt in 0..5 {
            let pending = self.has_changes(settings);
            if !pending && self.settled && !self.changed_on_disk() {
                return;
            }
            // Retried only for changes that would otherwise be lost.
            if self.sync(settings) || !pending {
                return;
            }
            // Another window may have the file open for a moment.
            std::thread::sleep(std::time::Duration::from_millis(20 << attempt));
        }
    }

    /// For the window that keeps eframe's state: writes any changes, then
    /// keeps a copy of the settings where older versions look for them. Only
    /// settings that were actually read go there, never a stand-in for ones
    /// that couldn't be, so a good copy is never replaced by defaults.
    pub fn save_with_legacy(&mut self, settings: &mut Settings, storage: &mut dyn eframe::Storage) {
        self.flush(settings);
        if self.settled {
            eframe::set_value(storage, LEGACY_KEY, settings);
        }
    }

    fn changed_on_disk(&self) -> bool {
        let Some(path) = &self.path else { return false };
        // The text, not the modification time: two writes close together can
        // share a time stamp.
        read_text(path).is_ok_and(|text| text != self.seen)
    }

    fn read_legacy(&self) -> Legacy {
        let Some(path) = &self.legacy else {
            return Legacy::Nothing;
        };
        match read_text(path) {
            Ok(None) => Legacy::Nothing,
            Ok(Some(text)) => match parse_legacy(&text) {
                Some(Some(mut settings)) => {
                    prepare(&mut settings);
                    Legacy::Found(settings)
                }
                Some(None) => Legacy::Nothing,
                None => Legacy::Unreadable,
            },
            Err(_) => Legacy::Unreadable,
        }
    }

    /// Merges this window's changes into the file and writes the result.
    /// On success `settings` holds the merged settings; on failure nothing
    /// is changed, so the same changes can be tried again.
    fn sync(&mut self, settings: &mut Settings) -> bool {
        let Some(path) = self.path.clone() else {
            self.base = settings.clone();
            self.recent_ops.clear();
            self.changed_at = None;
            return true;
        };
        // Held from reading to replacing, so two windows saving at the same
        // moment take turns. Without it, settings stay as they are on disk.
        let Some(_lock) = lock(&path) else {
            return false;
        };
        let disk = match read_text(&path) {
            Ok(Some(text)) => match decode(&text, &self.base) {
                Some(decoded) => Some((Some(text), decoded)),
                // Present but not readable as settings: left for whoever
                // can read it, not replaced.
                None => return false,
            },
            Ok(None) => match self.read_legacy() {
                // Not moved over yet: these changes go on top of it.
                Legacy::Found(old) => match to_map(&old) {
                    Some(map) => Some((None, (map, old))),
                    None => return false,
                },
                Legacy::Nothing => None,
                // Moving it over failed for now; a fresh file would lose it.
                Legacy::Unreadable => return false,
            },
            Err(_) => return false,
        };
        let (disk_text, decoded) = match disk {
            Some((text, decoded)) => (text, Some(decoded)),
            None => (None, None),
        };
        let Some((text, merged)) = merge(&self.base, settings, &self.recent_ops, decoded) else {
            return false;
        };
        if disk_text.as_ref() != Some(&text) && write_text(&path, &text).is_err() {
            return false;
        }
        self.seen = Some(text);
        self.settled = true;
        self.base = merged.clone();
        self.recent_ops.clear();
        self.changed_at = None;
        *settings = merged;
        true
    }
}

fn defaults() -> Settings {
    let mut settings = Settings::default();
    prepare(&mut settings);
    settings
}

fn prepare(settings: &mut Settings) {
    settings.migrate();
    settings.sanitize();
}

/// The settings in an older version's `app.ron`: `Some(None)` if it has
/// none, `None` if it can't be read.
fn parse_legacy(text: &str) -> Option<Option<Settings>> {
    let values: HashMap<String, String> = ron::from_str(text).ok()?;
    match values.get(LEGACY_KEY) {
        Some(value) => ron::from_str(value).ok().map(Some),
        None => Some(None),
    }
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
/// window has it. `None` if it can't be taken, in which case nothing may be
/// written.
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
    file.lock().ok()?;
    Some(file)
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

/// Combines the settings this window changed since `base`, and its changes
/// to the recent folders, with the file's (`None` when there is no file
/// yet). Returns the text to write and the settings this window should use:
/// what was written, except that the window keeps its own [`WINDOW_KEYS`].
fn merge(
    base: &Settings,
    local: &Settings,
    recent_ops: &[RecentOp],
    disk: Option<(Map<String, Value>, Settings)>,
) -> Option<(String, Settings)> {
    let base_map = to_map(base)?;
    let local_map = to_map(local)?;
    // With no file to merge into, this window's settings are written whole.
    let (mut out, disk) = disk.unwrap_or_else(|| (base_map.clone(), base.clone()));
    for (key, value) in &local_map {
        if base_map.get(key) != Some(value) {
            out.insert(key.clone(), value.clone());
        }
    }
    if !recent_ops.is_empty() {
        let mut recent = disk.recent;
        for op in recent_ops {
            op.apply(&mut recent);
        }
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

/// eframe keeps the window's size and place, and egui's own state, in one
/// file (`app.ron`) that it rewrites whole from what it read at startup. So
/// only one window may use it: the first to take this lock, which it holds
/// until it closes. Returns whether this window is that one. If the lock
/// can't be taken at all, no window is: they all leave the file alone.
pub fn claim_window_state() -> bool {
    static HELD: OnceLock<File> = OnceLock::new();
    let Some(dir) = eframe::storage_dir("feather") else {
        return false;
    };
    match claim(&dir.join("app.ron.lock")) {
        Claim::Owner(file) => HELD.set(file).is_ok(),
        Claim::Taken | Claim::Unavailable => false,
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

    fn legacy_of(path: &Path) -> PathBuf {
        path.with_file_name(LEGACY_FILE_NAME)
    }

    /// Writes an `app.ron` the way eframe does.
    fn write_legacy(path: &Path, settings: &Settings) {
        let mut values = HashMap::new();
        values.insert(
            LEGACY_KEY.to_owned(),
            ron::ser::to_string(settings).unwrap(),
        );
        values.insert("egui".to_owned(), "()".to_owned());
        fs::write(path, ron::ser::to_string(&values).unwrap()).unwrap();
    }

    /// Opens the store the way a window does, with an `app.ron` beside it.
    fn open(path: &Path) -> (Store, Settings) {
        Store::open(Some(path.to_owned()), Some(legacy_of(path)))
    }

    fn read(path: &Path) -> Settings {
        let text = read_text(path).unwrap().unwrap();
        decode(&text, &Settings::default()).unwrap().1
    }

    fn clean_up(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn opening_moves_a_folder_to_the_top() {
        let mut recent = paths(&["a", "b", "c"]);
        RecentOp::Open("c".into()).apply(&mut recent);
        assert_eq!(recent, paths(&["c", "a", "b"]));
        RecentOp::Open("d".into()).apply(&mut recent);
        assert_eq!(recent, paths(&["d", "c", "a", "b"]));
        for i in 0..20 {
            RecentOp::Open(format!("n{i}").into()).apply(&mut recent);
        }
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], PathBuf::from("n19"));
        RecentOp::Forget("n19".into()).apply(&mut recent);
        assert_eq!(recent[0], PathBuf::from("n18"));
        RecentOp::Clear.apply(&mut recent);
        assert!(recent.is_empty());
    }

    /// Two windows starting from `start`; the first opens `first` and saves,
    /// then the second, not having looked since, opens `second` and saves.
    fn two_windows_open(start: &[&str], first: &[RecentOp], second: &[RecentOp]) -> Vec<PathBuf> {
        let path = temp_file("recent");
        let initial = Settings {
            recent: paths(start),
            ..Settings::default()
        };
        write_text(&path, &encode(&to_map(&initial).unwrap()).unwrap()).unwrap();
        let (mut one, mut a) = open(&path);
        let (mut two, mut b) = open(&path);
        for op in first {
            one.recent(&mut a, op.clone());
        }
        one.flush(&mut a);
        for op in second {
            two.recent(&mut b, op.clone());
        }
        two.flush(&mut b);
        let result = read(&path).recent;
        assert_eq!(b.recent, result);
        clean_up(&path);
        result
    }

    #[test]
    fn recents_from_two_windows_are_combined() {
        let open = |p: &str| RecentOp::Open(p.into());
        assert_eq!(
            two_windows_open(&["a", "b", "c"], &[open("y")], &[open("x")]),
            paths(&["x", "y", "a", "b", "c"])
        );
        // Opened b, then c, before saving: both kept, in order.
        assert_eq!(
            two_windows_open(&["a", "b", "c"], &[open("y")], &[open("b"), open("c")]),
            paths(&["c", "b", "y", "a"])
        );
        // A full list: what falls off the end isn't a removal.
        let full = ["a", "b", "c", "d", "e", "f", "g", "h"];
        assert_eq!(
            two_windows_open(&full, &[open("h")], &[open("i")]),
            paths(&["i", "h", "a", "b", "c", "d", "e", "f"])
        );
        // Opening what is already at the top here still counts.
        assert_eq!(
            two_windows_open(&["a", "b"], &[open("b")], &[open("a")]),
            paths(&["a", "b"])
        );
    }

    #[test]
    fn removing_recent_folders_sticks() {
        let forget = RecentOp::Forget("a".into());
        let open = RecentOp::Open("y".into());
        assert_eq!(
            two_windows_open(&["a", "b", "c"], std::slice::from_ref(&open), &[forget]),
            paths(&["y", "b", "c"])
        );
        // Clearing keeps only what other windows open afterwards.
        assert_eq!(
            two_windows_open(&["a", "b", "c"], &[RecentOp::Clear], &[open]),
            paths(&["y"])
        );
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

        let (text, merged) = merge(&base, &local, &[], Some((disk_map, disk))).unwrap();
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
        let (text, merged) = merge(&base, &base, &[], Some((local_map, local))).unwrap();
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
        let (text, merged) = merge(&base, &local, &[], Some((raw, disk))).unwrap();
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
        let (mut one, mut a) = open(&path);
        let (mut two, mut b) = open(&path);
        assert_eq!(a, b);

        a.theme = ThemeChoice::Light;
        one.recent(&mut a, RecentOp::Open("/one".into()));
        one.flush(&mut a);

        b.font_size = 16.0;
        two.recent(&mut b, RecentOp::Open("/two".into()));
        two.flush(&mut b);
        assert_eq!(b.theme, ThemeChoice::Light, "picked up the other window's");
        assert_eq!(b.font_size, 16.0);
        assert_eq!(b.recent, paths(&["/two", "/one"]));

        // The first window catches up once it looks at the file again, even
        // if both writes landed within one tick of the file's clock.
        one.flush(&mut a);
        assert_eq!(a, b);
        let (_, fresh) = open(&path);
        assert_eq!(fresh, b);
        clean_up(&path);
    }

    #[test]
    fn changes_are_written_once_they_settle() {
        let path = temp_file("settle");
        let (mut store, mut settings) = open(&path);
        settings.font_size = 14.0;
        assert!(store.tick(&mut settings, 10.0).is_some());
        assert_eq!(read_text(&path).unwrap(), None, "not yet");
        assert!(store.tick(&mut settings, 10.2).is_some());
        assert_eq!(store.tick(&mut settings, 10.6), None);
        assert_eq!(read(&path).font_size, 14.0);
        clean_up(&path);
    }

    #[test]
    fn a_failed_write_is_kept_and_tried_again() {
        let path = temp_file("retry");
        let (mut store, mut settings) = open(&path);
        // A folder where the file should be can't be read, as when another
        // program has it open: nothing is overwritten, and nothing is lost.
        fs::create_dir(&path).unwrap();
        settings.font_size = 18.0;
        store.recent(&mut settings, RecentOp::Open("/kept".into()));
        assert!(store.tick(&mut settings, 0.0).is_some());
        assert!(
            store.tick(&mut settings, 1.0).is_some(),
            "failed: try again"
        );
        assert!(path.is_dir());
        assert_eq!(settings.font_size, 18.0);

        fs::remove_dir(&path).unwrap();
        assert!(store.tick(&mut settings, 1.6).is_none());
        let written = read(&path);
        assert_eq!(written.font_size, 18.0);
        assert_eq!(written.recent, paths(&["/kept"]));
        clean_up(&path);
    }

    #[test]
    fn a_file_that_cant_be_read_is_left_alone() {
        let path = temp_file("broken");
        let (mut store, mut settings) = open(&path);
        settings.font_size = 14.0;
        store.flush(&mut settings);
        // Half a file, as a crash in some other program might leave it.
        let broken = "{\n  \"font_size\": 16.0,\n  \"rec";
        fs::write(&path, broken).unwrap();

        // Neither looking for changes nor saving its own replaces it.
        assert_eq!(store.tick(&mut settings, 100.0), None);
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        settings.zoom = 1.1;
        store.flush(&mut settings);
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        assert_eq!(settings.zoom, 1.1, "kept for when it can be read");

        // Once it can be read again, the change goes in.
        fs::write(&path, "{}").unwrap();
        store.flush(&mut settings);
        assert_eq!(read(&path).zoom, 1.1);
        clean_up(&path);
    }

    #[test]
    fn an_old_save_moves_into_the_file() {
        let path = temp_file("legacy");
        let old = Settings {
            font_size: 19.0,
            recent: paths(&["/old"]),
            ..Settings::default()
        };
        write_legacy(&legacy_of(&path), &old);
        let (_, settings) = open(&path);
        assert_eq!(settings.font_size, 19.0);
        assert_eq!(read(&path).recent, paths(&["/old"]));
        // Once the file exists it wins over the old save.
        write_legacy(&legacy_of(&path), &Settings::default());
        let (_, settings) = open(&path);
        assert_eq!(settings.font_size, 19.0);
        clean_up(&path);
    }

    #[test]
    fn any_window_can_move_the_old_save_over() {
        // The second window gets there before the first.
        let path = temp_file("race");
        let old = Settings {
            font_size: 19.0,
            recent: paths(&["/old"]),
            ..Settings::default()
        };
        write_legacy(&legacy_of(&path), &old);
        let (mut second, mut b) = open(&path);
        assert_eq!(b.font_size, 19.0);
        second.recent(&mut b, RecentOp::Open("/second".into()));
        second.flush(&mut b);

        let (_, a) = open(&path);
        assert_eq!(a.font_size, 19.0);
        assert_eq!(a.recent, paths(&["/second", "/old"]));
        clean_up(&path);
    }

    #[test]
    fn an_old_save_that_cant_be_read_yet_isnt_replaced() {
        let path = temp_file("legacy-broken");
        fs::write(legacy_of(&path), "{\"feather\": \"(font_si").unwrap();
        let (mut store, mut settings) = open(&path);
        assert_eq!(read_text(&path).unwrap(), None);
        settings.zoom = 1.1;
        store.flush(&mut settings);
        assert_eq!(
            read_text(&path).unwrap(),
            None,
            "not taken for a fresh install"
        );

        let old = Settings {
            font_size: 19.0,
            ..Settings::default()
        };
        write_legacy(&legacy_of(&path), &old);
        store.flush(&mut settings);
        let written = read(&path);
        assert_eq!(written.font_size, 19.0);
        assert_eq!(written.zoom, 1.1);
        clean_up(&path);
    }

    /// eframe's storage, as the window that owns it sees it.
    #[derive(Default)]
    struct Storage(HashMap<String, String>);

    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    impl Storage {
        /// What eframe loads from an `app.ron`, if it can read it.
        fn load(path: &Path) -> Self {
            let text = fs::read_to_string(path).unwrap();
            Self(ron::from_str(&text).unwrap_or_default())
        }

        fn legacy(&self) -> Option<Settings> {
            eframe::get_value(self, LEGACY_KEY)
        }
    }

    #[test]
    fn a_good_old_copy_isnt_replaced_by_defaults() {
        // settings.json is broken, but app.ron beside it is fine.
        let path = temp_file("downgrade");
        let old = Settings {
            font_size: 19.0,
            recent: paths(&["/legacy"]),
            ..Settings::default()
        };
        write_legacy(&legacy_of(&path), &old);
        let broken = "{\n  \"font_size\": 16.0,\n  \"rec";
        fs::write(&path, broken).unwrap();
        let mut storage = Storage::load(&legacy_of(&path));

        let (mut store, mut settings) = open(&path);
        assert_eq!(settings, defaults(), "only a stand-in");
        store.save_with_legacy(&mut settings, &mut storage);
        assert_eq!(storage.legacy(), Some(old.clone()), "the old copy is kept");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);

        // With settings.json gone, the old copy is moved over again.
        fs::remove_file(&path).unwrap();
        store.save_with_legacy(&mut settings, &mut storage);
        assert_eq!(settings.font_size, 19.0);
        assert_eq!(read(&path).recent, paths(&["/legacy"]));
        assert_eq!(storage.legacy().unwrap().font_size, 19.0);
        clean_up(&path);
    }

    #[test]
    fn an_old_copy_read_late_is_still_moved_over() {
        // app.ron couldn't be read when the window opened.
        let path = temp_file("late");
        fs::write(legacy_of(&path), "{\"feather\": \"(font_si").unwrap();
        let (mut store, mut settings) = open(&path);
        let mut storage = Storage::load(&legacy_of(&path));
        store.save_with_legacy(&mut settings, &mut storage);
        assert_eq!(
            storage.legacy(),
            None,
            "defaults aren't saved as the old copy"
        );

        let old = Settings {
            font_size: 19.0,
            ..Settings::default()
        };
        write_legacy(&legacy_of(&path), &old);
        // Picked up while the window is open...
        assert_eq!(store.tick(&mut settings, 5.0), None);
        assert_eq!(settings.font_size, 19.0);
        // ...and so what it saves on closing is the real thing.
        store.save_with_legacy(&mut settings, &mut storage);
        assert_eq!(storage.legacy().unwrap().font_size, 19.0);
        assert_eq!(read(&path).font_size, 19.0);
        clean_up(&path);
    }

    #[test]
    fn nothing_is_written_until_there_is_something_to_keep() {
        let path = temp_file("empty");
        let (mut store, mut settings) = open(&path);
        assert_eq!(settings, defaults());
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
        // Somewhere the lock can't even be made: nobody owns it.
        let blocked = path.with_file_name("not-a-folder");
        fs::write(&blocked, "").unwrap();
        assert!(matches!(
            claim(&blocked.join("app.ron.lock")),
            Claim::Unavailable
        ));
        clean_up(&path);
    }
}
