//! Application state and the glue between the UI and the background worker.

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::diff::Diff;
use crate::document::{self, Body, LoadedFile, Preview};
use crate::git::{self, Change, ChangeKind, Status, Worktree};
use crate::tree::Tree;
use crate::worker::{Action, Event, Watch, Worker};
use crate::{theme, ui};

const SETTINGS_KEY: &str = "feather";
const MAX_RECENT: usize = 8;
const TOAST_SECONDS: f64 = 7.0;

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
}

/// Something the UI asked for this frame. Collected while drawing and applied
/// afterwards, so drawing code only ever needs shared access to state.
pub enum Cmd {
    PickRepo,
    OpenRepo(PathBuf),
    OpenFile(String),
    OpenDiff { change: Change, staged: bool },
    Act(Action),
    Commit,
    AskDiscard(Change),
    Refresh,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Files,
    Git,
}

pub struct FileView {
    pub rel: String,
    token: u64,
    pub file: Option<LoadedFile>,
    /// Show the rendered preview rather than the source, where one exists.
    pub preview: bool,
}

pub struct DiffView {
    pub change: Change,
    pub staged: bool,
    token: u64,
    pub diff: Option<Result<Diff, String>>,
}

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

    fn open_file(&mut self, rel: String) {
        if let View::File(view) = &self.view
            && view.rel == rel
        {
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
                    view.file = Some(file);
                }
            }
            Event::FileChanged { rel } => {
                if let View::File(view) = &mut self.view
                    && view.rel == rel
                {
                    view.token = self.worker.load_file(&rel, true);
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
                    doc.highlights = Some(highlights);
                }
            }
        }
    }
}

pub struct Toast {
    pub text: String,
    pub until: f64,
}

impl Toast {
    fn error(text: String, now: f64) -> Self {
        Self {
            text,
            until: now + TOAST_SECONDS,
        }
    }
}

pub struct App {
    pub settings: Settings,
    pub repo: Option<Repo>,
    pub toast: Option<Toast>,
    pub markdown: CommonMarkCache,
    folder_dialog: Option<Receiver<Option<PathBuf>>>,
    was_focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        let mut app = Self {
            settings,
            repo: None,
            toast: None,
            markdown: CommonMarkCache::default(),
            folder_dialog: None,
            was_focused: true,
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
    fn tick(&mut self, ctx: &egui::Context) {
        let (now, focused) = ctx.input(|i| (i.time, i.focused));

        if let Some(rx) = &self.folder_dialog {
            match rx.try_recv() {
                Ok(picked) => {
                    self.folder_dialog = None;
                    if let Some(path) = picked {
                        self.open_repo(&path, ctx);
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
        }
        self.was_focused = focused;

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
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let files = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1);
        let git = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num2);
        let commit = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
        let refresh = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);
        ctx.input_mut(|i| {
            if i.consume_shortcut(&open) {
                cmds.push(Cmd::PickRepo);
            }
            if i.consume_shortcut(&refresh) {
                cmds.push(Cmd::Refresh);
            }
            if let Some(repo) = &mut self.repo {
                if i.consume_shortcut(&files) {
                    repo.tab = Tab::Files;
                }
                if i.consume_shortcut(&git) && repo.is_git {
                    repo.tab = Tab::Git;
                }
                if repo.tab == Tab::Git && i.consume_shortcut(&commit) {
                    cmds.push(Cmd::Commit);
                }
            }
        });
    }

    fn apply(&mut self, cmd: Cmd, ctx: &egui::Context) {
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
            (_, None) => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(&ctx);

        let mut cmds = Vec::new();
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
        theme::BG_SHELL.to_normalized_gamma_f32()
    }
}

/// `canonicalize` on Windows yields `\\?\C:\…` paths, which git and people
/// both dislike. Strip the prefix when the rest is an ordinary drive path.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if rest.as_bytes().get(1) == Some(&b':') {
                return PathBuf::from(rest.to_owned());
            }
        }
    }
    path
}
