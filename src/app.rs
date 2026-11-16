//! Application state and the glue between the UI and the background worker.

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::diff::Diff;
use crate::document::{self, Body, Highlights, LoadedFile, Preview, Span, TextDoc};
use crate::git::{self, Change, ChangeKind, Status, Worktree};
use crate::settings::Settings;
use crate::tree::Tree;
use crate::update::{self, Updater};
use crate::worker::{Action, Event, Watch, Worker};
use crate::{theme, ui};

const SETTINGS_KEY: &str = "feather";
const MAX_RECENT: usize = 8;
const TOAST_SECONDS: f64 = 7.0;
/// With auto save on, edits are written this long after typing stops.
const AUTO_SAVE_SECONDS: f64 = 1.0;

/// Something the UI asked for this frame. Collected while drawing and applied
/// afterwards, so drawing code only ever needs shared access to state.
pub enum Cmd {
    PickRepo,
    OpenRepo(PathBuf),
    OpenFile(String),
    OpenDiff {
        change: Change,
        staged: bool,
    },
    Act(Action),
    Commit,
    AskDiscard(Change),
    Refresh,
    /// The text in the editor changed.
    Edited,
    Save,
    /// Throw away unsaved edits and read the file from disk again.
    ReloadFile,
    ToggleSidebar,
    ToggleSettings,
    CheckForUpdates,
    InstallUpdate,
    RestartToUpdate,
    OpenUrl(String),
    Quit,
    /// Answers to "you have unsaved changes".
    LeaveSaving,
    LeaveDiscarding,
    LeaveCancelled,
}

impl Cmd {
    /// Whether carrying this out would drop what is in the editor.
    fn leaves_file(&self, current: Option<&str>) -> bool {
        match self {
            Self::OpenFile(rel) => current != Some(rel.as_str()),
            Self::OpenRepo(_) | Self::OpenDiff { .. } | Self::Quit | Self::RestartToUpdate => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Files,
    Git,
}

/// Identifies a laid-out version of the editor text; see [`EditBuffer::layout`].
pub type LayoutKey = (u64, u64, u32, u64);

/// The text of a file while it is open in the editor.
pub struct EditBuffer {
    pub text: String,
    /// There are edits that have not been written to disk.
    pub dirty: bool,
    /// Goes up with every edit.
    pub version: u64,
    pub line_count: usize,
    /// The file was changed by something else while it had unsaved edits here.
    pub changed_on_disk: bool,
    crlf: bool,
    /// Syntax colors, with a hash of the exact text they were computed for.
    pub highlights: Option<(u64, Highlights)>,
    /// Goes up whenever new colors arrive.
    pub highlight_stamp: u64,
    /// Distinguishes one load of the file from the next, so a reload starts
    /// with a clean undo history.
    pub load_id: u64,
    /// Colors by line text from the last result, used for lines that have not
    /// changed while fresh colors are on their way.
    pub known_lines: HashMap<String, Vec<Span>>,
    last_edit: f64,
    /// The styled text most recently handed to egui for layout, reused until
    /// something it depends on changes.
    pub layout: Option<(LayoutKey, egui::text::LayoutJob)>,
}

impl EditBuffer {
    fn new(doc: &TextDoc, load_id: u64) -> Self {
        let mut buffer = Self {
            highlight_stamp: 0,
            load_id,
            text: doc.raw.clone(),
            dirty: false,
            version: 0,
            line_count: 1,
            changed_on_disk: false,
            crlf: doc.crlf,
            highlights: None,
            known_lines: HashMap::new(),
            last_edit: 0.0,
            layout: None,
        };
        buffer.count_lines();
        if let Some(highlights) = &doc.highlights {
            buffer.set_highlights(0, highlights.clone());
        }
        buffer
    }

    fn count_lines(&mut self) {
        self.line_count = self.text.bytes().filter(|b| *b == b'\n').count() + 1;
    }

    fn set_highlights(&mut self, version: u64, highlights: Highlights) {
        if version != self.version {
            return;
        }
        self.known_lines = self
            .text
            .split('\n')
            .zip(&highlights)
            .map(|(line, spans)| (line.to_owned(), spans.clone()))
            .collect();
        self.highlights = Some((egui::util::hash(self.text.as_str()), highlights));
        self.highlight_stamp += 1;
    }
}

pub struct FileView {
    pub rel: String,
    token: u64,
    pub file: Option<LoadedFile>,
    /// Show the rendered preview rather than the source, where one exists.
    pub preview: bool,
    /// Present when the file can be edited.
    pub edit: Option<EditBuffer>,
}

impl FileView {
    pub fn is_dirty(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.dirty)
    }
}

pub struct DiffView {
    pub change: Change,
    pub staged: bool,
    token: u64,
    pub diff: Option<Result<Diff, String>>,
}

// There is only ever one of these, so the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum View {
    Empty,
    File(FileView),
    Diff(DiffView),
}

/// Per-path git state used to annotate the file tree.
#[derive(Default)]
pub struct Decorations {
    files: HashMap<String, ChangeKind>,
    dirty_dirs: HashSet<String>,
    ignored: HashSet<String>,
}

impl Decorations {
    fn build(status: &Status) -> Self {
        let mut d = Self::default();
        for change in status.staged.iter().chain(&status.unstaged) {
            // Unstaged comes last, so the worktree state wins when both exist.
            d.files.insert(change.path.clone(), change.kind);
            let mut path = change.path.as_str();
            while let Some((parent, _)) = path.rsplit_once('/') {
                if !d.dirty_dirs.insert(parent.to_owned()) {
                    break;
                }
                path = parent;
            }
        }
        d.ignored = status
            .ignored
            .iter()
            .map(|p| p.trim_end_matches('/').to_owned())
            .collect();
        d
    }

    pub fn file(&self, rel: &str) -> Option<ChangeKind> {
        self.files.get(rel).copied()
    }

    pub fn dir_is_dirty(&self, rel: &str) -> bool {
        self.dirty_dirs.contains(rel)
    }

    pub fn is_ignored(&self, rel: &str) -> bool {
        if self.ignored.is_empty() {
            return false;
        }
        let mut path = rel;
        loop {
            if self.ignored.contains(path) {
                return true;
            }
            match path.rsplit_once('/') {
                Some((parent, _)) => path = parent,
                None => return false,
            }
        }
    }
}

pub struct Repo {
    pub root: PathBuf,
    pub name: String,
    pub is_git: bool,
    worker: Worker,
    pub tree: Tree,
    pub status: Status,
    /// False until the first status result arrives.
    pub status_loaded: bool,
    pub decorations: Decorations,
    pub branches: Vec<String>,
    pub worktrees: Vec<Worktree>,
    pub tab: Tab,
    pub view: View,
    pub commit_message: String,
    /// Label of the git action currently running, if any.
    pub busy: Option<&'static str>,
    pub confirm_discard: Option<Change>,
    pub branch_filter: String,
}

impl Repo {
    fn open(path: &Path, ctx: &egui::Context) -> Self {
        // Opening a folder inside a repository opens the repository itself.
        let (root, is_git) = match git::find_root(path) {
            Some(root) => (root, true),
            None => (path.to_owned(), false),
        };
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut repo = Self {
            worker: Worker::new(&root, is_git, ctx),
            tree: Tree::new(&root),
            root,
            name,
            is_git,
            status: Status::default(),
            status_loaded: false,
            decorations: Decorations::default(),
            branches: Vec::new(),
            worktrees: Vec::new(),
            tab: Tab::Files,
            view: View::Empty,
            commit_message: String::new(),
            busy: None,
            confirm_discard: None,
            branch_filter: String::new(),
        };
        repo.worker.refresh();
        // Land on the README, the way a repository page would.
        if let Some(readme) = (0..repo.tree.len())
            .map(|i| repo.tree.row(i))
            .find(|n| !n.is_dir && n.name.to_ascii_lowercase().starts_with("readme"))
            .map(|n| n.rel.clone())
        {
            repo.open_file(readme);
        }
        repo
    }

    /// Re-reads the file tree and git status from disk.
    fn refresh(&mut self) {
        self.tree.refresh();
        self.worker.refresh();
    }

    /// The file open in the editor, if any.
    fn open_rel(&self) -> Option<&str> {
        match &self.view {
            View::File(view) => Some(&view.rel),
            _ => None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(&self.view, View::File(view) if view.is_dirty())
    }

    fn open_file(&mut self, rel: String) {
        if self.open_rel() == Some(rel.as_str()) {
            return;
        }
        let token = self.worker.load_file(&rel, false);
        // Watching starts once the file has loaded and its stamp is known.
        self.worker.watch(Watch::Nothing);
        self.view = View::File(FileView {
            preview: document::preview_kind(&rel) != Preview::None,
            rel,
            token,
            file: None,
            edit: None,
        });
    }

    fn open_diff(&mut self, change: Change, staged: bool) {
        let token = self.worker.diff(change.clone(), staged);
        self.view = View::Diff(DiffView {
            change,
            staged,
            token,
            diff: None,
        });
    }

    fn run(&mut self, action: Action) {
        if self.busy.is_none() {
            self.busy = Some(action.label());
            self.worker.act(action);
        }
    }

    pub fn can_commit(&self) -> bool {
        self.busy.is_none()
            && !self.status.staged.is_empty()
            && !self.commit_message.trim().is_empty()
    }

    /// Called after each edit: marks the buffer and asks for fresh colors.
    fn edited(&mut self, now: f64) {
        let View::File(view) = &mut self.view else {
            return;
        };
        let Some(buffer) = &mut view.edit else { return };
        buffer.dirty = true;
        buffer.version += 1;
        buffer.last_edit = now;
        buffer.count_lines();
        self.worker
            .highlight_edit(&view.rel, buffer.text.clone(), buffer.version);
    }

    /// Writes the editor's text to disk.
    fn save(&mut self) -> Result<(), String> {
        let View::File(view) = &mut self.view else {
            return Ok(());
        };
        let (Some(buffer), Some(file)) = (&mut view.edit, &mut view.file) else {
            return Ok(());
        };
        if !buffer.dirty {
            return Ok(());
        }
        let result = if buffer.crlf {
            std::fs::write(&file.path, buffer.text.replace('\n', "\r\n"))
        } else {
            std::fs::write(&file.path, &buffer.text)
        };
        result.map_err(|e| format!("Could not save {}: {e}", view.rel))?;

        buffer.dirty = false;
        buffer.changed_on_disk = false;
        file.stamp = document::file_stamp(&file.path);
        file.size = file.stamp.map_or(0, |(size, _)| size);
        if let Body::Text(doc) = &mut file.body {
            doc.raw.clone_from(&buffer.text);
        }
        // Our own write is not an outside change.
        self.worker.watch(Watch::File {
            rel: view.rel.clone(),
            stamp: file.stamp,
        });
        self.worker.refresh();
        Ok(())
    }

    /// Drops unsaved edits without touching the file.
    fn forget_edits(&mut self) {
        if let View::File(view) = &mut self.view
            && let Some(buffer) = &mut view.edit
        {
            buffer.dirty = false;
        }
    }

    fn reload_file(&mut self) {
        self.forget_edits();
        if let View::File(view) = &mut self.view {
            view.token = self.worker.load_file(&view.rel, true);
        }
    }

    /// After a status change, keeps an open diff pointed at the right entry.
    fn sync_diff(&mut self) {
        let View::Diff(view) = &mut self.view else {
            return;
        };
        let find = |list: &[Change]| list.iter().find(|c| c.path == view.change.path).cloned();
        let (same_side, other_side) = if view.staged {
            (find(&self.status.staged), find(&self.status.unstaged))
        } else {
            (find(&self.status.unstaged), find(&self.status.staged))
        };
        if let Some(change) = same_side {
            view.token = self.worker.diff(change.clone(), view.staged);
            view.change = change;
        } else if let Some(change) = other_side {
            // The file was just staged or unstaged: follow it.
            view.staged = !view.staged;
            view.token = self.worker.diff(change.clone(), view.staged);
            view.change = change;
        } else {
            self.view = View::Empty;
            self.worker.watch(Watch::Nothing);
        }
    }

    fn handle(&mut self, event: Event, toast: &mut Option<Toast>, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        match event {
            Event::Status(result) => match result {
                Ok(status) => {
                    if status != self.status || !self.status_loaded {
                        self.decorations = Decorations::build(&status);
                        self.status = status;
                        self.tree.refresh();
                    }
                    self.status_loaded = true;
                    self.sync_diff();
                }
                Err(e) => {
                    if !self.status_loaded {
                        *toast = Some(Toast::error(e, now));
                    }
                    self.status_loaded = true;
                }
            },
            Event::Refs {
                branches,
                worktrees,
            } => {
                self.branches = branches;
                self.worktrees = worktrees;
            }
            Event::Diff { token, result } => {
                if let View::Diff(view) = &mut self.view
                    && view.token == token
                    && view.diff.as_ref() != Some(&result)
                {
                    view.diff = Some(result);
                }
            }
            Event::ActionDone { action, result } => {
                self.busy = None;
                match result {
                    Ok(()) => {
                        if matches!(action, Action::Commit(_)) {
                            self.commit_message.clear();
                        }
                    }
                    Err(e) => *toast = Some(Toast::error(e, now)),
                }
            }
            Event::File { token, file } => {
                if let View::File(view) = &mut self.view
                    && view.token == token
                {
                    // A reload of something drawn as an image: drop the cached texture.
                    if view.file.is_some() {
                        ctx.forget_image(&file.uri());
                    }
                    self.worker.watch(Watch::File {
                        rel: view.rel.clone(),
                        stamp: file.stamp,
                    });
                    view.edit = match &file.body {
                        Body::Text(doc) if doc.editable => Some(EditBuffer::new(doc, token)),
                        _ => None,
                    };
                    view.file = Some(file);
                }
            }
            Event::Highlights { token, highlights } => {
                if let View::File(view) = &mut self.view
                    && view.token == token
                    && let Some(LoadedFile {
                        body: Body::Text(doc),
                        ..
                    }) = &mut view.file
                    && highlights.len() == doc.lines.len()
                {
                    if let Some(buffer) = &mut view.edit {
                        buffer.set_highlights(0, highlights.clone());
                    }
                    doc.highlights = Some(highlights);
                }
            }
            Event::EditHighlights {
                rel,
                version,
                highlights,
            } => {
                if let View::File(view) = &mut self.view
                    && view.rel == rel
                    && let Some(buffer) = &mut view.edit
                {
                    buffer.set_highlights(version, highlights);
                }
            }
            Event::FileChanged { rel } => {
                if let View::File(view) = &mut self.view
                    && view.rel == rel
                {
                    match &mut view.edit {
                        // Don't replace what the user is in the middle of typing.
                        Some(buffer) if buffer.dirty => buffer.changed_on_disk = true,
                        _ => view.token = self.worker.load_file(&rel, true),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Error,
    Info,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: f64,
}

impl Toast {
    fn error(text: String, now: f64) -> Self {
        Self {
            text,
            kind: ToastKind::Error,
            until: now + TOAST_SECONDS,
        }
    }

    fn info(text: String, now: f64) -> Self {
        Self {
            text,
            kind: ToastKind::Info,
            until: now + TOAST_SECONDS,
        }
    }
}

pub struct App {
    pub settings: Settings,
    pub repo: Option<Repo>,
    pub toast: Option<Toast>,
    pub markdown: CommonMarkCache,
    pub updater: Updater,
    pub settings_open: bool,
    /// The action waiting on "save your changes first?".
    pub confirm_leave: Option<Cmd>,
    folder_dialog: Option<Receiver<Option<PathBuf>>>,
    was_focused: bool,
    /// Set once the user has agreed to close with (or without) saving.
    closing: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        settings.sanitize();

        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &settings);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        ui::titlebar::prepare_window(cc);

        let mut app = Self {
            settings,
            repo: None,
            toast: None,
            markdown: CommonMarkCache::default(),
            updater: Updater::new(&cc.egui_ctx),
            settings_open: false,
            confirm_leave: None,
            folder_dialog: None,
            was_focused: true,
            closing: false,
        };
        // A path on the command line wins; otherwise reopen the last repository.
        let start = initial.or_else(|| app.settings.recent.first().cloned());
        if let Some(path) = start.filter(|p| p.is_dir()) {
            app.open_repo(&path, &cc.egui_ctx);
        }
        app
    }

    fn open_repo(&mut self, path: &Path, ctx: &egui::Context) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let path = strip_verbatim(path);
        let repo = Repo::open(&path, ctx);
        self.settings.recent.retain(|p| *p != repo.root);
        self.settings.recent.insert(0, repo.root.clone());
        self.settings.recent.truncate(MAX_RECENT);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{} — Feather",
            repo.name
        )));
        self.repo = Some(repo);
    }

    fn pick_repo(&mut self, ctx: &egui::Context) {
        if self.folder_dialog.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let start = self.repo.as_ref().map(|r| r.root.clone());
        std::thread::spawn(move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title("Open repository");
            if let Some(dir) = start.as_deref().and_then(Path::parent) {
                dialog = dialog.set_directory(dir);
            }
            let picked = pollster::block_on(dialog.pick_folder());
            let _ = tx.send(picked.map(|handle| handle.path().to_owned()));
            ctx.request_repaint();
        });
        self.folder_dialog = Some(rx);
    }

    /// Drains background results and reacts to focus changes.
    fn tick(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        let (now, focused) = ctx.input(|i| (i.time, i.focused));

        if let Some(rx) = &self.folder_dialog {
            match rx.try_recv() {
                Ok(picked) => {
                    self.folder_dialog = None;
                    if let Some(path) = picked {
                        cmds.push(Cmd::OpenRepo(path));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.folder_dialog = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        if let Some(repo) = &mut self.repo {
            while let Some(event) = repo.worker.poll() {
                repo.handle(event, &mut self.toast, ctx);
            }
            if focused != self.was_focused {
                // The worker only polls for outside changes while we have focus;
                // catch up on anything missed as soon as it comes back.
                repo.worker.set_active(focused);
                if focused {
                    repo.refresh();
                }
            }

            let mut save_now = false;
            if self.settings.auto_save
                && let View::File(view) = &mut repo.view
                && let Some(buffer) = &mut view.edit
                && buffer.dirty
                && !buffer.changed_on_disk
            {
                let wait = buffer.last_edit + AUTO_SAVE_SECONDS - now;
                if wait > 0.0 {
                    ctx.request_repaint_after(Duration::from_secs_f64(wait));
                } else {
                    // Whatever happens, don't try again until the next edit.
                    buffer.last_edit = f64::INFINITY;
                    save_now = true;
                }
            }
            if save_now && let Err(e) = repo.save() {
                self.toast = Some(Toast::error(e, now));
            }
        }
        self.was_focused = focused;

        // Closing the window with unsaved edits asks first.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.closing
            && self.repo.as_ref().is_some_and(Repo::is_dirty)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            cmds.push(Cmd::Quit);
        }

        self.updater.poll();
        if self.settings.auto_update {
            self.updater.check_if_due();
        }
        if std::mem::take(&mut self.updater.manual_result) {
            match &self.updater.state {
                update::State::UpToDate => {
                    self.toast = Some(Toast::info(
                        format!("Feather {} is the latest version.", update::CURRENT_VERSION),
                        now,
                    ));
                }
                update::State::CheckFailed(e) => self.toast = Some(Toast::error(e.clone(), now)),
                _ => {}
            }
        }

        if let Some(toast) = &self.toast {
            if now >= toast.until {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(toast.until - now));
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let shortcut = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let table = [
            (shortcut(Key::O), Cmd::PickRepo),
            (shortcut(Key::R), Cmd::Refresh),
            (shortcut(Key::S), Cmd::Save),
            (shortcut(Key::B), Cmd::ToggleSidebar),
            (shortcut(Key::Comma), Cmd::ToggleSettings),
        ];
        ctx.input_mut(|i| {
            for (keys, cmd) in table {
                if i.consume_shortcut(&keys) {
                    cmds.push(cmd);
                }
            }
            if let Some(repo) = &mut self.repo {
                if i.consume_shortcut(&shortcut(Key::Num1)) {
                    repo.tab = Tab::Files;
                    self.settings.sidebar_open = true;
                }
                if i.consume_shortcut(&shortcut(Key::Num2)) && repo.is_git {
                    repo.tab = Tab::Git;
                    self.settings.sidebar_open = true;
                }
                if repo.tab == Tab::Git && i.consume_shortcut(&shortcut(Key::Enter)) {
                    cmds.push(Cmd::Commit);
                }
            }
        });
    }

    /// Carries out a command, first asking about unsaved edits if it would
    /// lose them.
    fn apply(&mut self, cmd: Cmd, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(repo) = &mut self.repo
            && repo.is_dirty()
            && cmd.leaves_file(repo.open_rel())
        {
            if !self.settings.auto_save {
                self.confirm_leave = Some(cmd);
                return;
            }
            if let Err(e) = repo.save() {
                self.toast = Some(Toast::error(e, now));
                return;
            }
        }
        self.run(cmd, ctx);
    }

    fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        match (cmd, &mut self.repo) {
            (Cmd::PickRepo, _) => self.pick_repo(ctx),
            (Cmd::OpenRepo(path), _) => {
                if path.is_dir() {
                    self.open_repo(&path, ctx);
                } else {
                    self.settings.recent.retain(|p| *p != path);
                    self.toast = Some(Toast::error(
                        format!("{} no longer exists", path.display()),
                        now,
                    ));
                }
            }
            (Cmd::ToggleSidebar, _) => self.settings.sidebar_open = !self.settings.sidebar_open,
            (Cmd::ToggleSettings, _) => self.settings_open = !self.settings_open,
            (Cmd::CheckForUpdates, _) => self.updater.check(true),
            (Cmd::InstallUpdate, _) => self.updater.install(),
            (Cmd::RestartToUpdate, _) => match self.updater.relaunch() {
                Ok(()) => {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(e) => self.toast = Some(Toast::error(e, now)),
            },
            (Cmd::OpenUrl(url), _) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            (Cmd::Quit, _) => {
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            (Cmd::LeaveCancelled, _) => self.confirm_leave = None,
            (Cmd::LeaveSaving, Some(repo)) => match repo.save() {
                Ok(()) => {
                    if let Some(pending) = self.confirm_leave.take() {
                        self.run(pending, ctx);
                    }
                }
                Err(e) => {
                    self.confirm_leave = None;
                    self.toast = Some(Toast::error(e, now));
                }
            },
            (Cmd::LeaveDiscarding, Some(repo)) => {
                repo.forget_edits();
                if let Some(pending) = self.confirm_leave.take() {
                    self.run(pending, ctx);
                }
            }
            (Cmd::OpenFile(rel), Some(repo)) => repo.open_file(rel),
            (Cmd::OpenDiff { change, staged }, Some(repo)) => repo.open_diff(change, staged),
            (Cmd::Act(action), Some(repo)) => repo.run(action),
            (Cmd::Commit, Some(repo)) => {
                if repo.can_commit() {
                    let message = repo.commit_message.trim().to_owned();
                    repo.run(Action::Commit(message));
                }
            }
            (Cmd::AskDiscard(change), Some(repo)) => repo.confirm_discard = Some(change),
            (Cmd::Refresh, Some(repo)) => repo.refresh(),
            (Cmd::Edited, Some(repo)) => repo.edited(now),
            (Cmd::Save, Some(repo)) => {
                if let Err(e) = repo.save() {
                    self.toast = Some(Toast::error(e, now));
                }
            }
            (Cmd::ReloadFile, Some(repo)) => repo.reload_file(),
            (_, None) => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Picks up setting changes, and the system switching light/dark.
        theme::apply(&ctx, &self.settings);

        let mut cmds = Vec::new();
        self.tick(&ctx, &mut cmds);
        self.shortcuts(&ctx, &mut cmds);
        ui::root(self, ui, &mut cmds);
        for cmd in cmds {
            self.apply(cmd, &ctx);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::bg_shell().to_normalized_gamma_f32()
    }
}

/// `canonicalize` on Windows yields `\\?\C:\…` paths, which git and people
/// both dislike. Strip the prefix when the rest is an ordinary drive path.
fn strip_verbatim(path: PathBuf) -> PathBuf {
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
