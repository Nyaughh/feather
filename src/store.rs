//! Keeps settings in one file that every Feather window shares.
//!
//! Each window is its own process with its own copy of the settings. When a
//! window writes, it re-reads the file and changes only the settings it
//! changed itself, so two windows never undo each other's changes. A window
//! also picks up changes made by the others (a new theme, a folder another
//! window opened) when it next looks at the file.

use serde_json::{Map, Value};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::settings::{MAX_RECENT, Settings};

const FILE_NAME: &str = "settings.json";
/// How long settings sit unwritten after they change, so dragging a slider
/// doesn't write the file on every frame.
const WRITE_DELAY: f64 = 0.5;
/// How often to check whether another window has written the file.
const CHECK_EVERY: f64 = 1.0;

/// Settings that describe this window rather than the user's preferences.
/// Each window keeps its own. A change here is still written, so the next
/// new window starts the same way.
const WINDOW_KEYS: [&str; 2] = ["sidebar_open", "history_open"];

pub struct Store {
    /// `None` when there is nowhere to keep the file, which leaves settings
    /// for this run only.
    path: Option<PathBuf>,
    /// The settings as of the last read or write.
    base: Settings,
    /// The file's modification time as of the last read or write.
    stamp: Option<SystemTime>,
    /// When the settings first differed from `base`.
    changed_at: Option<f64>,
    last_check: f64,
}

/// Where the settings file lives: beside eframe's own saved state.
pub fn default_path() -> Option<PathBuf> {
    eframe::storage_dir("feather").map(|dir| dir.join(FILE_NAME))
}

impl Store {
    /// Reads the settings file. Without one, `fallback` (what an older
    /// version saved) is used and written out as the new file.
    pub fn open(path: Option<PathBuf>, fallback: Option<Settings>) -> (Self, Settings) {
        let from_disk = path.as_deref().and_then(read);
        let missing = from_disk.is_none();
        let mut settings = from_disk.or(fallback).unwrap_or_default();
        prepare(&mut settings);
        let mut store = Self {
            stamp: path.as_deref().and_then(modified),
            path,
            base: settings.clone(),
            changed_at: None,
            last_check: f64::NEG_INFINITY,
        };
        if missing {
            store.write(&settings);
        }
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
            self.sync(settings);
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
        if *settings != self.base || self.changed_on_disk() {
            self.sync(settings);
        }
    }

    fn changed_on_disk(&self) -> bool {
        self.path
            .as_deref()
            .is_some_and(|path| modified(path) != self.stamp)
    }

    /// Merges this window's changes with the file and writes the result.
    /// `settings` ends up holding the merged settings.
    fn sync(&mut self, settings: &mut Settings) {
        self.changed_at = None;
        let Some(path) = self.path.clone() else {
            self.base = settings.clone();
            return;
        };
        // Held while reading and writing, so two windows saving at the same
        // moment take turns.
        let _lock = lock(&path);
        let (to_disk, merged) = match read(&path) {
            Some(mut disk) => {
                prepare(&mut disk);
                let (to_disk, merged) = merge(&self.base, settings, &disk);
                if to_disk != disk {
                    self.write(&to_disk);
                }
                (to_disk, merged)
            }
            None => {
                self.write(settings);
                (settings.clone(), settings.clone())
            }
        };
        drop(to_disk);
        self.stamp = modified(&path);
        self.base = merged.clone();
        *settings = merged;
    }

    fn write(&mut self, settings: &Settings) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(settings) else {
            return;
        };
        // Written beside the file and moved over it, so a reader never sees
        // half of it.
        let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if fs::write(&temp, text).is_ok() && fs::rename(&temp, path).is_err() {
            let _ = fs::remove_file(&temp);
        }
        self.stamp = modified(path);
    }
}

fn prepare(settings: &mut Settings) {
    settings.migrate();
    settings.sanitize();
}

fn read(path: &Path) -> Option<Settings> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Takes the lock that guards the settings file, waiting for it if another
/// window has it. `None` if locking isn't possible, in which case the write
/// goes ahead unguarded.
fn lock(path: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

/// Combines settings this window has changed since `base` with what is on
/// `disk`. Returns what to write, and what this window should use: the same,
/// except that the window keeps its own [`WINDOW_KEYS`].
fn merge(base: &Settings, local: &Settings, disk: &Settings) -> (Settings, Settings) {
    let (Some(base_map), Some(local_map), Some(mut out)) =
        (to_map(base), to_map(local), to_map(disk))
    else {
        return (local.clone(), local.clone());
    };
    for (key, value) in &local_map {
        if base_map.get(key) != Some(value) && key != "recent" {
            out.insert(key.clone(), value.clone());
        }
    }
    let Ok(mut to_disk) = serde_json::from_value::<Settings>(Value::Object(out)) else {
        return (local.clone(), local.clone());
    };
    if local.recent != base.recent {
        to_disk.recent = merge_recent(&base.recent, &local.recent, &disk.recent);
    }
    let mut merged = to_disk.clone();
    merged.sidebar_open = local.sidebar_open;
    merged.history_open = local.history_open;
    debug_assert_eq!(WINDOW_KEYS.len(), 2, "keep the fields above in step");
    (to_disk, merged)
}

fn to_map(settings: &Settings) -> Option<Map<String, Value>> {
    match serde_json::to_value(settings) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Recent folders after this window changed its list from `base` to
/// `local` while another window may have changed the file's list to `disk`.
///
/// Folders this window opened (new, or moved up the list) go first, then
/// the file's list without anything this window removed.
fn merge_recent(base: &[PathBuf], local: &[PathBuf], disk: &[PathBuf]) -> Vec<PathBuf> {
    // Positions in the old list among the folders that are still there, so
    // removing one doesn't look like moving up everything after it.
    let kept: Vec<&PathBuf> = base.iter().filter(|p| local.contains(p)).collect();
    let opened: Vec<&PathBuf> = local
        .iter()
        .enumerate()
        .filter(|(i, p)| kept.iter().position(|k| k == p).is_none_or(|old| *i < old))
        .map(|(_, p)| p)
        .collect();
    let removed = |p: &PathBuf| base.contains(p) && !local.contains(p);
    let mut result: Vec<PathBuf> = opened.iter().map(|p| (*p).clone()).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ThemeChoice;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    /// A settings file in a fresh temporary folder.
    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "feather-store-{name}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
        ));
        let dir = PathBuf::from(dir.to_string_lossy().replace([':', ' '], "_"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join(FILE_NAME)
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
    fn reopening_an_older_folder_moves_it_up() {
        let base = paths(&["a", "b", "c"]);
        let local = paths(&["c", "a", "b"]);
        let disk = paths(&["y", "a", "b", "c"]);
        assert_eq!(
            merge_recent(&base, &local, &disk),
            paths(&["c", "y", "a", "b"])
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
        let mut local = base.clone();
        local.font_size = 15.0;
        local.sidebar_open = false;
        let mut disk = base.clone();
        disk.theme = ThemeChoice::Dark;
        disk.sidebar_open = true;
        disk.zoom = 1.25;

        let (to_disk, merged) = merge(&base, &local, &disk);
        assert_eq!(to_disk.font_size, 15.0);
        assert_eq!(to_disk.theme, ThemeChoice::Dark);
        assert_eq!(to_disk.zoom, 1.25);
        // The window's own change is saved for new windows...
        assert!(!to_disk.sidebar_open);
        assert_eq!(merged, to_disk);

        // ...but another window's change to it doesn't reach into this one.
        let (to_disk, merged) = merge(&base, &base, &local);
        assert!(!to_disk.sidebar_open);
        assert!(merged.sidebar_open);
        assert_eq!(merged.font_size, 15.0);
    }

    #[test]
    fn two_windows_share_one_file() {
        let path = temp_file("share");
        let (mut one, mut a) = Store::open(Some(path.clone()), None);
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

        // The first window catches up once it looks at the file again.
        one.flush(&mut a);
        assert_eq!(a, b);
        let (_, fresh) = Store::open(Some(path.clone()), None);
        assert_eq!(fresh, b);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn changes_are_written_once_they_settle() {
        let path = temp_file("settle");
        let (mut store, mut settings) = Store::open(Some(path.clone()), None);
        settings.font_size = 14.0;
        assert!(store.tick(&mut settings, 10.0).is_some());
        assert_eq!(read(&path).unwrap().font_size, 12.5, "not yet");
        assert!(store.tick(&mut settings, 10.2).is_some());
        assert_eq!(store.tick(&mut settings, 10.6), None);
        assert_eq!(read(&path).unwrap().font_size, 14.0);
        let _ = fs::remove_dir_all(path.parent().unwrap());
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
        assert_eq!(read(&path).unwrap().recent, paths(&["/old"]));
        // Once the file exists it wins over the old save.
        let (_, settings) = Store::open(Some(path.clone()), Some(Settings::default()));
        assert_eq!(settings.recent, paths(&["/old"]));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn works_without_a_file() {
        let (mut store, mut settings) = Store::open(None, None);
        settings.zoom = 1.1;
        store.flush(&mut settings);
        assert_eq!(settings.zoom, 1.1);
    }
}
