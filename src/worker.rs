//! Background work. Nothing that touches the disk for longer than a single
//! directory listing, and nothing that runs `git`, happens on the UI thread.
//!
//! Local git commands run one at a time on a dedicated thread (they share the
//! index lock). That thread also polls for outside changes - a new status, a
//! different diff, an edited file - and only wakes the UI when something
//! actually changed, so an idle window does no drawing at all.
//!
//! Network commands and file loads get their own short-lived threads so a slow
//! push or a huge file never delays a status refresh.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::diff::{self, Diff};
use crate::document::{self, Body, FileStamp, Highlights, LoadedFile};
use crate::git::{
    self, BlameLine, Branch, Change, SearchOptions, SearchResults, Stash, Status, Worktree,
};

/// How often to look for outside changes while the window has focus.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Branches and worktrees change less often than files; check every few polls.
const REFS_EVERY: u32 = 5;
/// How often to fetch from the remote when that is turned on.
const FETCH_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug)]
pub enum Action {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    Discard(Change),
    /// Throw away every unstaged change.
    DiscardAll,
    Commit(String),
    /// Fold what is staged into the last commit; an empty message keeps its own.
    Amend(String),
    /// Take back the last commit, leaving its changes staged.
    UndoCommit,
    /// Set every change aside.
    Stash,
    StashApply(usize),
    StashPop(usize),
    StashDrop(usize),
    Switch(String),
    /// Check out a branch that only exists on a remote (`origin/name`).
    Track(String),
    CreateBranch(String),
    Fetch,
    Pull,
    Push {
        has_upstream: bool,
    },
}

impl Action {
    fn is_network(&self) -> bool {
        matches!(self, Self::Fetch | Self::Pull | Self::Push { .. })
    }

    /// Present-tense label shown while the action runs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stage(_) | Self::StageAll => "Staging…",
            Self::Unstage(_) | Self::UnstageAll => "Unstaging…",
            Self::Discard(_) | Self::DiscardAll => "Discarding…",
            Self::Commit(_) | Self::Amend(_) => "Committing…",
            Self::UndoCommit => "Undoing…",
            Self::Stash => "Stashing…",
            Self::StashApply(_) | Self::StashPop(_) => "Applying…",
            Self::StashDrop(_) => "Deleting…",
            Self::Switch(_) | Self::Track(_) | Self::CreateBranch(_) => "Switching…",
            Self::Fetch => "Fetching…",
            Self::Pull => "Pulling…",
            Self::Push { .. } => "Pushing…",
        }
    }
}

/// What the content area is showing, so the poller knows what to keep fresh.
#[derive(Clone, Debug, Default)]
pub enum Watch {
    #[default]
    Nothing,
    File {
        rel: String,
        stamp: FileStamp,
    },
    Diff {
        change: Change,
        staged: bool,
        token: u64,
    },
}

enum Job {
    Refresh,
    Diff {
        change: Change,
        staged: bool,
        token: u64,
    },
    Act(Action),
}

pub enum Event {
    Status(Result<Status, String>),
    Refs(Refs),
    Diff {
        token: u64,
        result: Result<Diff, String>,
    },
    ActionDone {
        action: Action,
        result: Result<(), String>,
    },
    File {
        token: u64,
        file: LoadedFile,
    },
    Highlights {
        token: u64,
        highlights: Highlights,
    },
    /// The watched file changed on disk.
    FileChanged {
        rel: String,
    },
    /// Results of a search across the repository.
    Search {
        token: u64,
        result: Result<SearchResults, String>,
    },
    /// Every file that quick open can offer.
    Files(Vec<String>),
    /// Who last changed each line of a file.
    Blame {
        rel: String,
        lines: Vec<BlameLine>,
    },
    /// Syntax colors for text being edited, as it stood at `version`.
    EditHighlights {
        rel: String,
        version: u64,
        highlights: Highlights,
    },
}

/// State shared between the UI and the git thread.
#[derive(Default)]
struct Shared {
    /// Poll only while the window is focused.
    active: AtomicBool,
    /// Fetch from the remote every so often.
    auto_fetch: AtomicBool,
    blame: AtomicBool,
    /// A background fetch is running / has just finished.
    fetching: AtomicBool,
    fetched: AtomicBool,
    watch: Mutex<Watch>,
}

/// Sends events to the UI and wakes it.
#[derive(Clone)]
struct Outbox {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Outbox {
    fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }

    fn send_refs(&self, root: &Path) {
        self.send(Event::Refs(read_refs(root)));
    }
}

/// The parts of a repository that change less often than its files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
    pub stashes: Vec<Stash>,
}

fn read_refs(root: &Path) -> Refs {
    Refs {
        branches: git::branches(root).unwrap_or_default(),
        worktrees: git::worktrees(root).unwrap_or_default(),
        stashes: git::stashes(root).unwrap_or_default(),
    }
}

pub struct Worker {
    root: PathBuf,
    jobs: Sender<Job>,
    outbox: Outbox,
    events: Receiver<Event>,
    shared: Arc<Shared>,
    /// Token of the most recent file request; older loads abandon their work.
    file_generation: Arc<AtomicU64>,
    /// Likewise for re-highlighting text as it is edited.
    edit_generation: Arc<AtomicU64>,
    /// And for searches: typing more cancels the search for what came before.
    search_generation: Arc<AtomicU64>,
    is_git: bool,
    next_token: u64,
}

impl Worker {
    pub fn new(root: &Path, is_git: bool, ctx: &egui::Context) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (tx, events) = mpsc::channel::<Event>();
        let outbox = Outbox {
            tx,
            ctx: ctx.clone(),
        };
        let shared = Arc::new(Shared::default());
        shared.active.store(true, Ordering::Relaxed);
        let git_thread = GitThread {
            root: root.to_owned(),
            is_git,
            outbox: outbox.clone(),
            shared: shared.clone(),
            last_status: None,
            last_diff: None,
            last_refs: None,
            polls: 0,
            last_fetch: None,
        };
        thread::Builder::new()
            .name("feather-git".into())
            .spawn(move || git_thread.run(&job_rx))
            .expect("spawn git thread");
        Self {
            root: root.to_owned(),
            jobs,
            outbox,
            events,
            shared,
            file_generation: Arc::new(AtomicU64::new(0)),
            edit_generation: Arc::new(AtomicU64::new(0)),
            search_generation: Arc::new(AtomicU64::new(0)),
            is_git,
            next_token: 1,
        }
    }

    pub fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    /// Turns background polling on or off (it is pointless while unfocused).
    pub fn set_active(&self, active: bool) {
        self.shared.active.store(active, Ordering::Relaxed);
    }

    /// Whether to fetch from the remote every few minutes.
    /// Turns blame lookups on or off. Returns whether that changed anything.
    pub fn set_blame(&self, on: bool) -> bool {
        self.shared.blame.swap(on, Ordering::Relaxed) != on
    }

    pub fn set_auto_fetch(&self, on: bool) {
        self.shared.auto_fetch.store(on, Ordering::Relaxed);
    }

    /// Tells the poller what is on screen.
    pub fn watch(&self, watch: Watch) {
        if let Ok(mut current) = self.shared.watch.lock() {
            *current = watch;
        }
    }

    /// Asks for a fresh status now; the result is always reported.
    pub fn refresh(&self) {
        let _ = self.jobs.send(Job::Refresh);
    }

    /// Requests a diff and keeps watching it for changes.
    pub fn diff(&mut self, change: Change, staged: bool) -> u64 {
        let token = self.token();
        self.watch(Watch::Diff {
            change: change.clone(),
            staged,
            token,
        });
        let _ = self.jobs.send(Job::Diff {
            change,
            staged,
            token,
        });
        token
    }

    pub fn act(&self, action: Action) {
        if action.is_network() {
            let (root, outbox) = (self.root.clone(), self.outbox.clone());
            let jobs = self.jobs.clone();
            thread::spawn(move || {
                let result = run_action(&root, &action);
                outbox.send(Event::ActionDone { action, result });
                // Status is always read on the git thread, so it has one source.
                let _ = jobs.send(Job::Refresh);
                outbox.send_refs(&root);
            });
        } else {
            let _ = self.jobs.send(Job::Act(action));
        }
    }

    /// Searches file contents. A newer search cancels this one.
    pub fn search(&mut self, query: String, options: SearchOptions) -> u64 {
        let token = self.token();
        self.search_generation.store(token, Ordering::Relaxed);
        let generation = self.search_generation.clone();
        let (root, outbox, is_git) = (self.root.clone(), self.outbox.clone(), self.is_git);
        thread::spawn(move || {
            let cancelled = || generation.load(Ordering::Relaxed) != token;
            let result = git::search(&root, is_git, &query, options, &cancelled);
            if !cancelled() {
                outbox.send(Event::Search { token, result });
            }
        });
        token
    }

    /// Lists the repository's files for quick open.
    pub fn list_files(&self) {
        let (root, outbox) = (self.root.clone(), self.outbox.clone());
        thread::spawn(move || {
            if let Ok(files) = git::list_files(&root) {
                outbox.send(Event::Files(files));
            }
        });
    }

    /// Looks up who last changed each line of a file.
    pub fn blame(&self, rel: &str) {
        if !self.is_git || !self.shared.blame.load(Ordering::Relaxed) {
            return;
        }
        let (root, outbox, rel) = (self.root.clone(), self.outbox.clone(), rel.to_owned());
        thread::spawn(move || {
            // Untracked and brand-new files simply have no blame.
            if let Ok(lines) = git::blame(&root, &rel) {
                outbox.send(Event::Blame { rel, lines });
            }
        });
    }

    /// Re-highlights text that is being edited. A newer request cancels this one.
    pub fn highlight_edit(&mut self, rel: &str, text: String, version: u64) {
        let token = self.token();
        self.edit_generation.store(token, Ordering::Relaxed);
        let generation = self.edit_generation.clone();
        let (outbox, rel) = (self.outbox.clone(), rel.to_owned());
        thread::spawn(move || {
            if let Some(highlights) = document::highlight_text(&text, &rel, &generation, token) {
                outbox.send(Event::EditHighlights {
                    rel,
                    version,
                    highlights,
                });
            }
        });
    }

    /// Loads a file for the viewer.
    ///
    /// Normally the text arrives first and syntax colors follow in a second
    /// event, so opening a file is never held up by highlighting. With
    /// `complete` set, a single event carries both - used when reloading a file
    /// that is already on screen, so it does not flash uncolored.
    pub fn load_file(&mut self, rel: &str, complete: bool) -> u64 {
        let token = self.token();
        self.file_generation.store(token, Ordering::Relaxed);
        let generation = self.file_generation.clone();
        let outbox = self.outbox.clone();
        let (path, rel) = (self.root.join(rel), rel.to_owned());
        thread::spawn(move || {
            let mut file = document::load(&path, &rel);
            let size = file.size;
            if complete {
                if let Body::Text(doc) = &mut file.body {
                    doc.highlights =
                        document::highlight(&doc.lines, &rel, size, &generation, token);
                }
                outbox.send(Event::File { token, file });
                return;
            }
            let lines = match &file.body {
                Body::Text(doc) if document::can_highlight(size) => Some(doc.lines.clone()),
                _ => None,
            };
            outbox.send(Event::File { token, file });
            if let Some(lines) = lines
                && let Some(highlights) =
                    document::highlight(&lines, &rel, size, &generation, token)
            {
                outbox.send(Event::Highlights { token, highlights });
            }
        });
        token
    }
}

fn run_action(root: &Path, action: &Action) -> Result<(), String> {
    match action {
        Action::Stage(paths) => git::stage(root, paths),
        Action::Unstage(paths) => git::unstage(root, paths),
        Action::StageAll => git::stage_all(root),
        Action::UnstageAll => git::unstage_all(root),
        Action::Discard(change) => git::discard(root, change),
        Action::DiscardAll => git::discard_all(root),
        Action::Commit(message) => git::commit(root, message),
        Action::Amend(message) => git::amend(root, message),
        Action::UndoCommit => git::undo_commit(root),
        Action::Stash => git::stash_push(root),
        Action::StashApply(index) => git::stash_apply(root, *index),
        Action::StashPop(index) => git::stash_pop(root, *index),
        Action::StashDrop(index) => git::stash_drop(root, *index),
        Action::Switch(name) => git::switch_branch(root, name),
        Action::Track(remote_branch) => git::track_branch(root, remote_branch),
        Action::CreateBranch(name) => git::create_branch(root, name),
        Action::Fetch => git::fetch(root),
        Action::Pull => git::pull(root),
        Action::Push { has_upstream } => git::push(root, *has_upstream),
    }
}

struct GitThread {
    root: PathBuf,
    is_git: bool,
    outbox: Outbox,
    shared: Arc<Shared>,
    /// What the UI was last told, so polling can stay quiet when nothing moved.
    last_status: Option<Result<Status, String>>,
    last_diff: Option<(u64, Result<Diff, String>)>,
    last_refs: Option<Refs>,
    polls: u32,
    last_fetch: Option<Instant>,
}

impl GitThread {
    fn run(mut self, jobs: &Receiver<Job>) {
        // Grammars take a moment to load; do it before anyone is waiting on them.
        thread::spawn(document::warm_up);
        if self.is_git {
            self.send_refs(true);
        }

        let mut next_poll = Instant::now() + POLL_INTERVAL;
        loop {
            let wait = next_poll.saturating_duration_since(Instant::now());
            match jobs.recv_timeout(wait) {
                Ok(job) => self.handle(job),
                Err(RecvTimeoutError::Timeout) => {
                    if self.shared.active.load(Ordering::Relaxed) {
                        self.poll();
                    }
                    next_poll = Instant::now() + POLL_INTERVAL;
                }
                // The UI dropped this repository.
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn handle(&mut self, job: Job) {
        match job {
            Job::Refresh => {
                self.send_status(true);
                self.send_refs(true);
            }
            Job::Diff {
                change,
                staged,
                token,
            } => self.send_diff(&change, staged, token, true),
            Job::Act(action) => {
                let result = run_action(&self.root, &action);
                let refs_changed = matches!(
                    action,
                    Action::Commit(_)
                        | Action::Amend(_)
                        | Action::UndoCommit
                        | Action::Stash
                        | Action::StashApply(_)
                        | Action::StashPop(_)
                        | Action::StashDrop(_)
                        | Action::Switch(_)
                        | Action::Track(_)
                        | Action::CreateBranch(_)
                );
                self.outbox.send(Event::ActionDone { action, result });
                self.send_status(true);
                if refs_changed {
                    self.send_refs(true);
                }
            }
        }
    }

    /// Looks for changes made outside the app and reports only real ones.
    fn poll(&mut self) {
        self.fetch_if_due();
        // A fetch that just landed may have moved branches and ahead/behind.
        let fetched = self.shared.fetched.swap(false, Ordering::Relaxed);
        self.send_status(fetched);
        self.polls += 1;
        if fetched || self.polls.is_multiple_of(REFS_EVERY) {
            self.send_refs(false);
        }

        let watch = match self.shared.watch.lock() {
            Ok(watch) => watch.clone(),
            Err(_) => return,
        };
        match watch {
            Watch::Nothing => {}
            Watch::File { rel, stamp } => {
                let now = document::file_stamp(&self.root.join(&rel));
                if now != stamp {
                    // Remember the new stamp so the change is reported once.
                    if let Ok(mut current) = self.shared.watch.lock()
                        && let Watch::File {
                            rel: current_rel,
                            stamp,
                        } = &mut *current
                        && *current_rel == rel
                    {
                        *stamp = now;
                    }
                    self.outbox.send(Event::FileChanged { rel });
                }
            }
            Watch::Diff {
                change,
                staged,
                token,
            } => self.send_diff(&change, staged, token, false),
        }
    }

    /// Starts a quiet fetch when automatic fetching is on and one is due.
    fn fetch_if_due(&mut self) {
        let shared = &self.shared;
        let due = self
            .last_fetch
            .is_none_or(|at| at.elapsed() >= FETCH_INTERVAL);
        if !self.is_git
            || !due
            || !shared.auto_fetch.load(Ordering::Relaxed)
            || shared.fetching.swap(true, Ordering::Relaxed)
        {
            return;
        }
        self.last_fetch = Some(Instant::now());
        let (root, shared) = (self.root.clone(), self.shared.clone());
        thread::spawn(move || {
            // Failures (offline, no remote, needs a login) are not worth a message.
            let ok = git::fetch_in_background(&root).is_ok();
            shared.fetched.store(ok, Ordering::Relaxed);
            shared.fetching.store(false, Ordering::Relaxed);
        });
    }

    fn send_refs(&mut self, force: bool) {
        if !self.is_git {
            return;
        }
        let refs = read_refs(&self.root);
        if force || self.last_refs.as_ref() != Some(&refs) {
            self.last_refs = Some(refs.clone());
            self.outbox.send(Event::Refs(refs));
        }
    }

    fn send_status(&mut self, force: bool) {
        if !self.is_git {
            return;
        }
        let status = git::status(&self.root);
        if force || self.last_status.as_ref() != Some(&status) {
            self.last_status = Some(status.clone());
            self.outbox.send(Event::Status(status));
        }
    }

    fn send_diff(&mut self, change: &Change, staged: bool, token: u64, force: bool) {
        let result = git::diff(&self.root, change, staged).map(|raw| diff::parse(&raw));
        let unchanged = self
            .last_diff
            .as_ref()
            .is_some_and(|(last_token, last)| *last_token == token && *last == result);
        if force || !unchanged {
            self.last_diff = Some((token, result.clone()));
            self.outbox.send(Event::Diff { token, result });
        }
    }
}
