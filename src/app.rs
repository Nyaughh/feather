//! Application state and the glue between the UI and the background worker.

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::cli::{self, Target};
use crate::diff::Diff;
use crate::document::{self, Body, Highlights, LoadedFile, Preview, Span, TextDoc};
use crate::git::{
    self, BlameLine, Branch, Change, ChangeKind, Commit, CommitInfo, SearchOptions, SearchResults,
    Stash, Status, Worktree,
};
use crate::settings::{Settings, TerminalDock};
use crate::store::{self, Store};
use crate::terminal::Terminals;
use crate::tree::Tree;
use crate::update::{self, Updater};
use crate::worker::{Action, CommitDetail, Event, Watch, Worker};
use crate::{theme, ui};

const SETTINGS_KEY: &str = "feather";
const TOAST_SECONDS: f64 = 7.0;
/// With auto save on, edits are written this long after typing stops.
const AUTO_SAVE_SECONDS: f64 = 1.0;

/// Something the UI asked for this frame. Collected while drawing and applied
/// afterwards, so drawing code only ever needs shared access to state.
pub enum Cmd {
    PickRepo,
    /// Ask for a file to open.
    PickFile,
    /// Open a file chosen in the picker: here if it is in this window's
    /// folder or no folder is open, otherwise in a new window.
    OpenPickedFile(PathBuf),
    OpenRepo(PathBuf),
    /// Open another window, on the home screen.
    NewWindow,
    /// Put this window back on the home screen.
    CloseFolder,
    /// Forget the recently opened folders.
    ClearRecent,
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
    /// Open a file and put the cursor somewhere in it.
    OpenAt {
        rel: String,
        jump: Jump,
    },
    /// Show the Search tab with its field focused.
    FocusSearch,
    ToggleQuickOpen,
    /// Show or hide find-in-file.
    ToggleFind,
    ToggleTerminal,
    NewTerminal,
    /// Close a whole terminal tab, splits and all.
    CloseTerminal(usize),
    SelectTerminal(usize),
    /// Start another terminal beside the active one, in the same tab.
    SplitTerminal,
    /// Close one split of the active tab.
    ClosePane(usize),
    /// Give the keyboard to one split of the active tab.
    FocusPane(usize),
    /// Move the terminal panel to the bottom or the right of the window.
    DockTerminal(TerminalDock),
    AskDiscardAll,
    AskDropStash(Stash),
    /// Show a commit from the history.
    OpenCommit(String),
    ToggleHistory,
    /// Load what the hover card for a commit shows.
    LoadCommitInfo(String),
    /// Fold staged changes into the last commit, using the commit box's
    /// message if it has one.
    Amend,
    /// The same, staging everything first.
    AmendAll,
    /// Stage everything and commit it with the commit box's message.
    CommitAll,
    ToggleSidebar,
    ToggleSettings,
    /// Open the dialog for pasting in a theme.
    ImportTheme,
    CheckForUpdates,
    InstallUpdate,
    RestartToUpdate,
    OpenUrl(String),
    Quit,
    /// Answers to "you have unsaved changes".
    LeaveSaving,
    LeaveDiscarding,
    LeaveCancelled,
    /// A file or folder named on the command line. Kept whole until it is
    /// carried out, as the repository may change before then.
    OpenPath(Target),
    /// Put the `feather` command on the PATH, or take it off.
    InstallCommand,
    RemoveCommand,
}

impl Cmd {
    /// Whether carrying this out would drop what is in the editor.
    fn leaves_file(&self, current: Option<&str>) -> bool {
        match self {
            Self::OpenFile(rel) | Self::OpenAt { rel, .. } => current != Some(rel.as_str()),
            Self::OpenRepo(_)
            | Self::OpenPath(_)
            | Self::CloseFolder
            | Self::OpenDiff { .. }
            | Self::OpenCommit(_)
            | Self::Quit
            | Self::RestartToUpdate => true,
            // These rewrite files on disk, so unsaved edits are settled first.
            Self::Act(Action::Stash | Action::StashApply(_) | Action::StashPop(_)) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Files,
    Search,
    Git,
}

/// Where the editor should put the cursor (and scroll to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Jump {
    /// A 1-based line and byte column, selecting `len` bytes from there.
    Line {
        line: usize,
        column: usize,
        len: usize,
    },
    /// A byte range of the text.
    Bytes(std::ops::Range<usize>),
}

/// Find-in-file: what is being looked for and where it was found.
#[derive(Default)]
pub struct FindBar {
    pub query: String,
    pub match_case: bool,
    /// Byte ranges of every match in the editor's text.
    pub matches: Vec<std::ops::Range<usize>>,
    pub current: usize,
    /// What `matches` was computed for: (hash of the text, query, case).
    pub computed_for: Option<(u64, String, bool)>,
    /// Put the keyboard focus in the field on the next frame.
    pub focus: bool,
}

/// The Search tab: a query, its options, and the matches across the repository.
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub options: SearchOptions,
    pub results: Option<Result<SearchResults, String>>,
    pub searching: bool,
    /// When the query last changed and has not been searched for yet.
    pub changed_at: Option<f64>,
    /// Put the keyboard focus in the field on the next frame.
    pub focus: bool,
    token: u64,
}

/// Quick open: type part of a file name, pick from the matches.
#[derive(Default)]
pub struct QuickOpen {
    pub query: String,
    pub selected: usize,
    /// The best matches, best first.
    pub results: Vec<String>,
    /// The query and file count `results` was worked out for.
    pub ranked_for: Option<(String, usize)>,
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

    /// Whether the file uses Windows line endings.
    pub fn crlf(&self) -> bool {
        self.crlf
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
    /// A pending request to move the cursor, applied once the text is shown.
    pub jump: Option<Jump>,
    pub find: Option<FindBar>,
    /// Who last changed each line, as of the last save.
    pub blame: Option<Vec<BlameLine>>,
    /// The cursor's 1-based line and column, for the status bar.
    pub cursor: Option<(usize, usize)>,
    /// The character index `cursor` was worked out from.
    pub cursor_index: Option<(u64, usize)>,
    /// The character index where the cursor's line ends, which is where the
    /// blame note for it is drawn.
    pub line_end: Option<usize>,
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

/// One commit from the history, and what it changed.
pub struct CommitView {
    pub commit: Commit,
    token: u64,
    pub detail: Option<Result<CommitDetail, String>>,
}

// There is only ever one of these, so the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum View {
    Empty,
    File(FileView),
    Diff(DiffView),
    Commit(CommitView),
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
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
    pub stashes: Vec<Stash>,
    /// Recent history, newest first, and the lines of its graph.
    pub commits: Vec<Commit>,
    pub graph: Vec<crate::graph::Row>,
    /// Hover card details, by commit; `None` while loading.
    pub commit_info: HashMap<String, Option<Result<CommitInfo, String>>>,
    /// `owner/name` when the repository is on GitHub.
    pub github: Option<String>,
    /// The message of a commit being undone, to put back in the commit box.
    undone_message: Option<String>,
    /// The stash waiting on "delete it?".
    pub confirm_drop_stash: Option<Stash>,
    pub tab: Tab,
    pub view: View,
    pub commit_message: String,
    /// Label of the git action currently running, if any.
    pub busy: Option<&'static str>,
    pub confirm_discard: Option<Change>,
    pub confirm_discard_all: bool,
    pub branch_filter: String,
    pub search: SearchState,
    /// Every file in the repository, once quick open has asked for them.
    pub files: Option<Vec<String>>,
    pub quick_open: Option<QuickOpen>,
    pub terminals: Terminals,
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
            stashes: Vec::new(),
            commits: Vec::new(),
            graph: Vec::new(),
            commit_info: HashMap::new(),
            github: None,
            undone_message: None,
            confirm_drop_stash: None,
            tab: Tab::Files,
            view: View::Empty,
            commit_message: String::new(),
            busy: None,
            confirm_discard: None,
            confirm_discard_all: false,
            branch_filter: String::new(),
            search: SearchState::default(),
            files: None,
            quick_open: None,
            terminals: Terminals::default(),
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

    /// The open file changed on disk while it had unsaved edits here.
    fn changed_on_disk(&self) -> bool {
        matches!(&self.view, View::File(FileView { edit: Some(buffer), .. }) if buffer.changed_on_disk)
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
            jump: None,
            find: None,
            blame: None,
            cursor: None,
            cursor_index: None,
            line_end: None,
        });
    }

    /// Opens an empty editor for a file that doesn't exist yet.
    fn open_new_file(&mut self, rel: String) {
        if self.open_rel() == Some(rel.as_str()) {
            return;
        }
        let token = self.worker.token();
        let file = document::new_file(&self.root.join(&rel));
        // Nothing on disk yet, so nothing has changed until something appears.
        self.worker.watch(Watch::File {
            rel: rel.clone(),
            stamp: None,
        });
        let edit = match &file.body {
            Body::Text(doc) => Some(EditBuffer::new(doc, token)),
            _ => None,
        };
        self.view = View::File(FileView {
            preview: false,
            rel,
            token,
            file: Some(file),
            edit,
            jump: None,
            find: None,
            blame: None,
            cursor: None,
            cursor_index: None,
            line_end: None,
        });
    }

    /// Opens a file (if it is not open already) and moves the cursor.
    fn open_at(&mut self, rel: String, jump: Jump) {
        self.open_file(rel);
        if let View::File(view) = &mut self.view {
            view.preview = false;
            view.jump = Some(jump);
        }
    }

    fn run_search(&mut self) {
        self.search.changed_at = None;
        let query = self.search.query.clone();
        if query.trim().is_empty() {
            self.search.results = None;
            self.search.searching = false;
            return;
        }
        self.search.searching = true;
        self.search.token = self.worker.search(query, self.search.options);
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

    fn open_commit(&mut self, hash: &str) {
        let Some(commit) = self.commits.iter().find(|c| c.hash == hash).cloned() else {
            return;
        };
        let token = self.worker.commit(commit.hash.clone());
        if !self.commit_info.contains_key(&commit.hash) {
            self.commit_info.insert(commit.hash.clone(), None);
            self.worker.commit_info(commit.hash.clone());
        }
        self.worker.watch(Watch::Nothing);
        self.view = View::Commit(CommitView {
            commit,
            token,
            detail: None,
        });
    }

    fn run(&mut self, action: Action) {
        if self.busy.is_none() {
            if matches!(action, Action::UndoCommit) {
                self.undone_message = git::last_commit_message(&self.root);
            }
            self.busy = Some(action.label());
            self.worker.act(action);
        }
    }

    /// Whether the last commit can still be rewritten: there is one, and it
    /// has not been pushed.
    pub fn can_rewrite_last_commit(&self) -> bool {
        self.busy.is_none()
            && self.status.head.is_some()
            && (self.status.upstream.is_none() || self.status.ahead > 0)
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
        // Only a file that has never been on disk has no stamp (one that was
        // read and has since gone shows an error, not an editor). It is
        // written even untouched, as saving it would be expected to.
        let new = file.stamp.is_none();
        if !buffer.dirty && !new {
            return Ok(());
        }
        let text = if buffer.crlf {
            std::borrow::Cow::Owned(buffer.text.replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(buffer.text.as_str())
        };
        let result = if new {
            create_new_file(&file.path, text.as_bytes())
        } else {
            std::fs::write(&file.path, text.as_bytes())
        };
        if let Err(e) = result {
            if new && e.kind() == std::io::ErrorKind::AlreadyExists {
                // Something else made the file since it was opened here. Keep
                // the text, and treat it like any file changed on disk:
                // saving again replaces it, reloading shows it.
                buffer.changed_on_disk = true;
                file.stamp = document::file_stamp(&file.path);
                self.worker.watch(Watch::File {
                    rel: view.rel.clone(),
                    stamp: file.stamp,
                });
                return Err(format!(
                    "{} was just created by another program, so it wasn't saved. Your text \
                     is still here: save again to replace that file, or reload to open it.",
                    view.rel
                ));
            }
            return Err(format!("Could not save {}: {e}", view.rel));
        }

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
        self.worker.blame(&view.rel);
        self.worker.refresh();
        if new {
            self.tree.refresh();
        }
        Ok(())
    }

    /// Follows the blame setting: looks it up for the open file when turned
    /// on, forgets it when turned off.
    fn set_blame(&mut self, on: bool) {
        if !self.worker.set_blame(on) {
            return;
        }
        if let View::File(view) = &mut self.view {
            if !on {
                view.blame = None;
            } else if matches!(
                view.file,
                Some(LoadedFile {
                    body: Body::Text(_),
                    ..
                })
            ) {
                self.worker.blame(&view.rel);
            }
        }
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
            Event::Refs(refs) => {
                self.branches = refs.branches;
                self.worktrees = refs.worktrees;
                self.stashes = refs.stashes;
                if self.commits != refs.commits {
                    self.graph = crate::graph::layout(&refs.commits);
                    self.commits = refs.commits;
                    // The open commit may have been amended away or pulled in.
                    if let View::Commit(view) = &mut self.view
                        && let Some(fresh) =
                            self.commits.iter().find(|c| c.hash == view.commit.hash)
                    {
                        view.commit = fresh.clone();
                    }
                }
            }
            Event::CommitInfo { hash, result } => {
                self.commit_info.insert(hash, Some(result));
            }
            Event::Commit { token, result } => {
                if let View::Commit(view) = &mut self.view
                    && view.token == token
                {
                    view.detail = Some(result);
                }
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
                    Ok(()) => match action {
                        Action::Commit(_)
                        | Action::CommitAll(_)
                        | Action::Amend(_)
                        | Action::AmendAll(_) => self.commit_message.clear(),
                        // The message comes back with the changes, ready to edit.
                        Action::UndoCommit => {
                            if let Some(message) = self.undone_message.take()
                                && self.commit_message.trim().is_empty()
                            {
                                self.commit_message = message;
                            }
                        }
                        _ => {}
                    },
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
                    if matches!(file.body, Body::Text(_)) {
                        self.worker.blame(&view.rel);
                    }
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
            Event::Search { token, result } => {
                if token == self.search.token {
                    self.search.searching = false;
                    self.search.results = Some(result);
                }
            }
            Event::Files(files) => self.files = Some(files),
            Event::Blame { rel, lines } => {
                if let View::File(view) = &mut self.view
                    && view.rel == rel
                {
                    view.blame = Some(lines);
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

/// What has been typed into the "import a theme" dialog.
#[derive(Default)]
pub struct ThemeImport {
    pub name: String,
    pub css: String,
    pub error: Option<String>,
}

/// How this window was started.
pub struct Startup {
    /// A file or folder named on the command line.
    pub target: Option<Target>,
    /// Asked for as a new, empty window (`feather --new-window` with no path),
    /// so the last repository isn't reopened.
    pub new_window: bool,
    /// Why the path on the command line couldn't be opened, to show.
    pub error: Option<String>,
}

pub struct App {
    pub settings: Settings,
    /// Shares settings with the other windows.
    store: Store,
    pub theme_import: Option<ThemeImport>,
    pub repo: Option<Repo>,
    pub toast: Option<Toast>,
    pub markdown: CommonMarkCache,
    pub updater: Updater,
    pub settings_open: bool,
    /// The action waiting on "save your changes first?".
    pub confirm_leave: Option<Cmd>,
    images: Arc<crate::images::Loader>,
    /// A folder or file picker that is open, and which of the two it is.
    dialog: Option<(Picking, Receiver<Option<PathBuf>>)>,
    /// Takes paths from `feather` commands run while this window is open.
    instance: Option<crate::instance::Server>,
    /// Installing or removing the `feather` command, which may wait on a
    /// password prompt.
    command_job: Option<Receiver<Result<String, String>>>,
    was_focused: bool,
    /// Set once the user has agreed to close with (or without) saving.
    closing: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        // Settings saved by versions before the shared file came in.
        let older: Option<Settings> = cc.storage.and_then(|s| eframe::get_value(s, SETTINGS_KEY));
        let (store, settings) = Store::open(store::default_path(), older);

        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &settings);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // Added last, so every image comes to it first.
        let images = Arc::new(crate::images::Loader::new());
        cc.egui_ctx.add_image_loader(images.clone());
        ui::titlebar::prepare_window(cc);

        let mut app = Self {
            settings,
            store,
            theme_import: None,
            repo: None,
            toast: None,
            markdown: CommonMarkCache::default(),
            updater: Updater::new(&cc.egui_ctx),
            settings_open: false,
            confirm_leave: None,
            images,
            dialog: None,
            instance: {
                let ctx = cc.egui_ctx.clone();
                crate::instance::Server::start(move || ctx.request_repaint())
            },
            command_job: None,
            was_focused: true,
            closing: false,
        };
        if let Some(error) = startup.error {
            app.toast = Some(Toast::error(error, 0.0));
        }
        // A path on the command line wins; otherwise reopen the last
        // repository, unless this was asked for as a new, empty window.
        match startup.target {
            Some(target) => app.open_target(target, &cc.egui_ctx),
            None if startup.new_window => {}
            None => {
                // Unless another window already has it open.
                if let Some(path) = app.settings.recent.first().cloned().filter(|p| p.is_dir())
                    && !crate::instance::open_elsewhere(&path)
                {
                    app.open_repo(&path, &cc.egui_ctx);
                }
            }
        }
        app
    }

    fn open_repo(&mut self, path: &Path, ctx: &egui::Context) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let path = cli::strip_verbatim(path);
        let repo = Repo::open(&path, ctx);
        let mut repo = repo;
        repo.github = repo.is_git.then(|| git::github_slug(&repo.root)).flatten();
        self.images.set_repository(&repo.root, repo.github.clone());
        store::touch_recent(&mut self.settings.recent, &repo.root);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{} - Feather",
            repo.name
        )));
        if let Some(instance) = &self.instance {
            instance.set_root(Some(repo.root.clone()));
        }
        self.repo = Some(repo);
    }

    /// Opens a path from the command line: a folder as the workspace, a file
    /// in the open repository if it is inside it, otherwise in its own.
    fn open_target(&mut self, target: Target, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let path = match target {
            Target::Folder(path) => {
                // Already showing it: being brought to the front is enough.
                let shown = self.repo.as_ref().is_some_and(|repo| {
                    crate::instance::shows(&repo.root, &Target::Folder(path.clone()))
                });
                if !shown {
                    self.run(Cmd::OpenRepo(path), ctx);
                }
                return;
            }
            Target::File(path) | Target::NewFile(path) => path,
        };
        let inside = |repo: &Repo| cli::relative_to(&repo.root, &path).filter(|r| !r.is_empty());
        if self.repo.as_ref().and_then(inside).is_none() {
            match cli::existing_ancestor(path.parent().unwrap_or(&path)) {
                Some(dir) if dir.is_dir() => self.open_repo(&dir, ctx),
                _ => {
                    let text = format!("Can't open {}", path.display());
                    self.toast = Some(Toast::error(text, now));
                    return;
                }
            }
        }
        let Some(repo) = &mut self.repo else { return };
        let Some(rel) = inside(repo) else {
            let text = format!("Can't open {}", path.display());
            self.toast = Some(Toast::error(text, now));
            return;
        };
        if path.exists() {
            repo.open_file(rel);
        } else {
            repo.open_new_file(rel);
        }
    }

    /// Whether opening `target` would replace what the editor shows.
    fn target_leaves_file(&self, target: &Target) -> bool {
        let Some(repo) = &self.repo else {
            return false;
        };
        match target {
            Target::Folder(_) => !crate::instance::shows(&repo.root, target),
            Target::File(path) | Target::NewFile(path) => {
                let rel = cli::relative_to(&repo.root, path).filter(|r| !r.is_empty());
                rel.is_none() || rel.as_deref() != repo.open_rel()
            }
        }
    }

    fn start_command_job(&mut self, job: fn() -> Result<String, String>, ctx: &egui::Context) {
        if self.command_job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.command_job = Some(rx);
    }

    /// Shows the system's folder or file picker, without blocking the window.
    fn pick(&mut self, picking: Picking, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let root = self.repo.as_ref().map(|r| r.root.clone());
        std::thread::spawn(move || {
            let mut dialog = rfd::AsyncFileDialog::new();
            let picked = match picking {
                Picking::Folder => {
                    dialog = dialog.set_title("Open folder");
                    if let Some(dir) = root.as_deref().and_then(Path::parent) {
                        dialog = dialog.set_directory(dir);
                    }
                    pollster::block_on(dialog.pick_folder())
                }
                Picking::File => {
                    dialog = dialog.set_title("Open file");
                    if let Some(dir) = &root {
                        dialog = dialog.set_directory(dir);
                    }
                    pollster::block_on(dialog.pick_file())
                }
            };
            let _ = tx.send(picked.map(|handle| handle.path().to_owned()));
            ctx.request_repaint();
        });
        self.dialog = Some((picking, rx));
    }

    /// Back to the home screen, with nothing open.
    fn close_folder(&mut self, ctx: &egui::Context) {
        if let Some(repo) = self.repo.take() {
            // Keeps it first in the list, ready to reopen.
            store::touch_recent(&mut self.settings.recent, &repo.root);
        }
        // Now free to take a folder from a `feather` command.
        if let Some(instance) = &self.instance {
            instance.set_root(None);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("Feather".to_owned()));
    }

    /// Drains background results and reacts to focus changes.
    fn tick(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        let (now, focused) = ctx.input(|i| (i.time, i.focused));

        if let Some((picking, rx)) = &self.dialog {
            let picking = *picking;
            match rx.try_recv() {
                Ok(picked) => {
                    self.dialog = None;
                    if let Some(path) = picked {
                        cmds.push(match picking {
                            Picking::Folder => Cmd::OpenRepo(path),
                            Picking::File => Cmd::OpenPickedFile(path),
                        });
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.dialog = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        while let Some(handoff) = self.instance.as_ref().and_then(|i| i.poll()) {
            // Come to the front, as if the user had switched to this window.
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            cmds.push(Cmd::OpenPath(handoff.target));
        }

        if let Some(rx) = &self.command_job {
            match rx.try_recv() {
                Ok(result) => {
                    self.command_job = None;
                    self.toast = Some(match result {
                        Ok(text) => Toast::info(text, now),
                        Err(text) => Toast::error(text, now),
                    });
                }
                Err(mpsc::TryRecvError::Disconnected) => self.command_job = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // Writes settings changes, and picks up the other windows'.
        if let Some(wait) = self.store.tick(&mut self.settings, now) {
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }

        if let Some(repo) = &mut self.repo {
            while let Some(event) = repo.worker.poll() {
                repo.handle(event, &mut self.toast, ctx);
            }
            repo.worker.set_auto_fetch(self.settings.auto_fetch);
            repo.set_blame(self.settings.blame);
            repo.terminals.pump();

            // Search a moment after typing stops, not on every keystroke.
            if let Some(changed) = repo.search.changed_at {
                let wait = changed + 0.25 - now;
                if wait > 0.0 {
                    ctx.request_repaint_after(Duration::from_secs_f64(wait));
                } else {
                    repo.run_search();
                }
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
        let shifted = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);

        // Where Ctrl is the shortcut key, a focused terminal needs most of
        // these combinations for itself (Ctrl+R, Ctrl+P, Ctrl+F and so on).
        let terminal_has_keys = !cfg!(target_os = "macos")
            && self
                .repo
                .as_ref()
                .is_some_and(|r| r.terminals.open && r.terminals.focused);

        ctx.input_mut(|i| {
            // Shift variants first: a plain Ctrl+` pattern also matches Ctrl+Shift+`.
            if i.consume_shortcut(&shifted(Key::Backtick)) {
                cmds.push(Cmd::NewTerminal);
            }
            if i.consume_shortcut(&shortcut(Key::Backtick)) {
                cmds.push(Cmd::ToggleTerminal);
            }
            if self.repo.is_some() && i.consume_shortcut(&split_shortcut()) {
                cmds.push(Cmd::SplitTerminal);
            }
            // Moving between splits, only while one of them has the keyboard.
            // Otherwise these keys stay the shell's (word movement).
            if let Some(repo) = &mut self.repo
                && repo.terminals.open
                && repo.terminals.focused
                && repo
                    .terminals
                    .tabs
                    .get(repo.terminals.active)
                    .is_some_and(|tab| tab.panes.len() > 1)
            {
                for (key, forward) in [(Key::ArrowLeft, false), (Key::ArrowRight, true)] {
                    if i.consume_shortcut(&focus_pane_shortcut(key)) {
                        repo.terminals.cycle_pane(forward);
                    }
                }
            }
            // Shift variants first: a plain Ctrl+F pattern also matches Ctrl+Shift+F.
            if i.consume_shortcut(&shifted(Key::F)) {
                cmds.push(Cmd::FocusSearch);
            }
            if i.consume_shortcut(&shifted(Key::N)) {
                cmds.push(Cmd::NewWindow);
            }
            if terminal_has_keys {
                return;
            }
            for (key, cmd) in [(Key::O, Cmd::PickFile), (Key::W, Cmd::Quit)] {
                if i.consume_shortcut(&shifted(key)) {
                    cmds.push(cmd);
                }
            }
            let table = [
                (shortcut(Key::O), Cmd::PickRepo),
                (shortcut(Key::R), Cmd::Refresh),
                (shortcut(Key::S), Cmd::Save),
                (shortcut(Key::B), Cmd::ToggleSidebar),
                (shortcut(Key::P), Cmd::ToggleQuickOpen),
                (shortcut(Key::F), Cmd::ToggleFind),
                (shortcut(Key::Comma), Cmd::ToggleSettings),
            ];
            for (keys, cmd) in table {
                if i.consume_shortcut(&keys) {
                    cmds.push(cmd);
                }
            }
            if let Some(repo) = &mut self.repo {
                for (key, tab) in [
                    (Key::Num1, Tab::Files),
                    (Key::Num2, Tab::Search),
                    (Key::Num3, Tab::Git),
                ] {
                    if i.consume_shortcut(&shortcut(key)) && (tab != Tab::Git || repo.is_git) {
                        repo.tab = tab;
                        self.settings.sidebar_open = true;
                    }
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
        let leaves = match &cmd {
            Cmd::OpenPath(target) => self.target_leaves_file(target),
            _ => self
                .repo
                .as_ref()
                .is_some_and(|repo| cmd.leaves_file(repo.open_rel())),
        };
        if leaves
            && let Some(repo) = &mut self.repo
            && repo.is_dirty()
        {
            // Auto save never writes over a file something else changed (or
            // created) on disk: that takes an explicit choice.
            if !self.settings.auto_save || repo.changed_on_disk() {
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
            (Cmd::PickRepo, _) => self.pick(Picking::Folder, ctx),
            (Cmd::PickFile, _) => self.pick(Picking::File, ctx),
            (Cmd::NewWindow, _) => {
                if let Err(e) = spawn_window(None) {
                    self.toast = Some(Toast::error(e, now));
                }
            }
            (Cmd::OpenPickedFile(path), _) => {
                let elsewhere = self
                    .repo
                    .as_ref()
                    .is_some_and(|repo| cli::relative_to(&repo.root, &path).is_none());
                if !elsewhere {
                    self.apply(Cmd::OpenPath(Target::File(path)), ctx);
                } else if let Err(e) = spawn_window(Some(&path)) {
                    self.toast = Some(Toast::error(e, now));
                }
            }
            (Cmd::CloseFolder, _) => self.close_folder(ctx),
            (Cmd::ClearRecent, _) => self.settings.recent.clear(),
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
            (Cmd::ImportTheme, _) => self.theme_import = Some(ThemeImport::default()),
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
            (Cmd::OpenPath(target), _) => self.open_target(target, ctx),
            (Cmd::InstallCommand, _) => {
                self.start_command_job(crate::shell_command::install, ctx);
            }
            (Cmd::RemoveCommand, _) => self.start_command_job(crate::shell_command::remove, ctx),
            (Cmd::OpenFile(rel), Some(repo)) => repo.open_file(rel),
            (Cmd::OpenAt { rel, jump }, Some(repo)) => repo.open_at(rel, jump),
            (Cmd::FocusSearch, Some(repo)) => {
                repo.tab = Tab::Search;
                repo.search.focus = true;
                self.settings.sidebar_open = true;
            }
            (Cmd::ToggleQuickOpen, Some(repo)) => {
                if repo.quick_open.take().is_none() {
                    repo.quick_open = Some(QuickOpen::default());
                    // Fetched each time, so new files are there to be found.
                    repo.worker.list_files();
                }
            }
            (Cmd::ToggleFind, Some(repo)) => {
                if let View::File(view) = &mut repo.view
                    && view.edit.is_some()
                {
                    view.preview = false;
                    match &mut view.find {
                        // Pressing it again goes back to the field.
                        Some(find) => find.focus = true,
                        None => {
                            view.find = Some(FindBar {
                                focus: true,
                                ..FindBar::default()
                            });
                        }
                    }
                }
            }
            (Cmd::ToggleTerminal, Some(repo)) => {
                if repo.terminals.tabs.is_empty() {
                    if let Err(e) = repo.terminals.add(&repo.root, &self.settings.shell, ctx) {
                        self.toast = Some(Toast::error(e, now));
                    }
                } else {
                    repo.terminals.open = !repo.terminals.open;
                }
                repo.terminals.grab_focus = repo.terminals.open;
            }
            (Cmd::NewTerminal, Some(repo)) => {
                if let Err(e) = repo.terminals.add(&repo.root, &self.settings.shell, ctx) {
                    self.toast = Some(Toast::error(e, now));
                }
                repo.terminals.grab_focus = true;
            }
            (Cmd::CloseTerminal(index), Some(repo)) => repo.terminals.close(index),
            (Cmd::SelectTerminal(index), Some(repo)) => repo.terminals.select(index, None),
            (Cmd::SplitTerminal, Some(repo)) => {
                if let Err(e) = repo.terminals.split(&repo.root, &self.settings.shell, ctx) {
                    self.toast = Some(Toast::error(e, now));
                }
                repo.terminals.grab_focus = true;
            }
            (Cmd::ClosePane(pane), Some(repo)) => {
                let tab = repo.terminals.active;
                repo.terminals.close_pane(tab, pane);
                repo.terminals.grab_focus = repo.terminals.open;
            }
            (Cmd::FocusPane(pane), Some(repo)) => {
                let tab = repo.terminals.active;
                repo.terminals.select(tab, Some(pane));
            }
            (Cmd::DockTerminal(dock), _) => {
                self.settings.terminal_dock = dock;
                if let Some(repo) = &mut self.repo {
                    repo.terminals.grab_focus = repo.terminals.open;
                }
            }
            (Cmd::AskDiscardAll, Some(repo)) => repo.confirm_discard_all = true,
            (Cmd::AskDropStash(stash), Some(repo)) => repo.confirm_drop_stash = Some(stash),
            (Cmd::OpenCommit(hash), Some(repo)) => repo.open_commit(&hash),
            (Cmd::ToggleHistory, _) => {
                self.settings.history_open = !self.settings.history_open;
            }
            (Cmd::LoadCommitInfo(hash), Some(repo)) => {
                if !repo.commit_info.contains_key(&hash) {
                    repo.commit_info.insert(hash.clone(), None);
                    repo.worker.commit_info(hash);
                }
            }
            (Cmd::Amend, Some(repo)) => {
                let message = repo.commit_message.trim().to_owned();
                repo.run(Action::Amend(message));
            }
            (Cmd::AmendAll, Some(repo)) => {
                let message = repo.commit_message.trim().to_owned();
                repo.run(Action::AmendAll(message));
            }
            (Cmd::CommitAll, Some(repo)) => {
                let message = repo.commit_message.trim().to_owned();
                if !message.is_empty() && repo.status.change_count() > 0 {
                    repo.run(Action::CommitAll(message));
                }
            }
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

/// Splits the terminal: Ctrl+Shift+5 as in VS Code, or Cmd+\ on macOS,
/// where Cmd+Shift+5 belongs to the system's screenshot tool.
pub fn split_shortcut() -> egui::KeyboardShortcut {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if cfg!(target_os = "macos") {
        KeyboardShortcut::new(Modifiers::COMMAND, Key::Backslash)
    } else {
        KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Num5)
    }
}

/// Moves the keyboard to the split on that side: Ctrl+Alt (Cmd+Option on
/// macOS) and an arrow. Ctrl+arrow alone still moves by word in the shell.
pub fn focus_pane_shortcut(key: egui::Key) -> egui::KeyboardShortcut {
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND | egui::Modifiers::ALT, key)
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
        self.store.flush(&mut self.settings);
        // Still kept here too, for going back to an older version.
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The folder in the window closed last is the one a plain launch reopens.
        if let Some(repo) = &self.repo {
            store::touch_recent(&mut self.settings.recent, &repo.root);
        }
        self.store.flush(&mut self.settings);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::bg_shell().to_normalized_gamma_f32()
    }
}

/// Which picker is open.
#[derive(Clone, Copy)]
enum Picking {
    Folder,
    File,
}

/// Starts another Feather window showing `path`, or the home screen. Each
/// window is its own process, so closing one leaves the others running.
fn spawn_window(path: Option<&Path>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Could not open a new window: {e}"))?;
    let mut command = std::process::Command::new(exe);
    command.arg("--new-window").args(path);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Its own process group, so it outlives a terminal this window came from.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not open a new window: {e}"))?;
    // Collected when it ends, so it doesn't linger as a zombie on Unix.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Writes a file that must not exist yet, making its folders first. Fails
/// with `AlreadyExists`, touching nothing, if something is there.
fn create_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_never_replaces_one_that_appeared() {
        let dir = std::env::temp_dir().join(format!("feather-new-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("deep").join("notes.md");

        create_new_file(&path, b"mine").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");

        let again = create_new_file(&path, b"other").unwrap_err();
        assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"mine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
