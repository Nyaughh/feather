//! Background work. Nothing that touches the disk for longer than a single
//! directory listing, and nothing that runs `git`, happens on the UI thread.
//!
//! Local git commands run one at a time on a dedicated thread (they share the
//! index lock). That thread also polls for outside changes — a new status, a
//! different diff, an edited file — and only wakes the UI when something
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
use crate::git::{self, Change, Status, Worktree};

/// How often to look for outside changes while the window has focus.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub enum Action {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    Discard(Change),
    Commit(String),
    Switch(String),
    CreateBranch(String),
    Fetch,
    Pull,
    Push { has_upstream: bool },
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
            Self::Discard(_) => "Discarding…",
            Self::Commit(_) => "Committing…",
            Self::Switch(_) | Self::CreateBranch(_) => "Switching…",
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
    Refs {
        branches: Vec<String>,
        worktrees: Vec<Worktree>,
    },
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
        self.send(Event::Refs {
            branches: git::branches(root).unwrap_or_default(),
            worktrees: git::worktrees(root).unwrap_or_default(),
        });
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
    /// `complete` set, a single event carries both — used when reloading a file
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
        Action::Commit(message) => git::commit(root, message),
        Action::Switch(name) => git::switch_branch(root, name),
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
}

impl GitThread {
    fn run(mut self, jobs: &Receiver<Job>) {
        // Grammars take a moment to load; do it before anyone is waiting on them.
        thread::spawn(document::warm_up);
        if self.is_git {
            self.outbox.send_refs(&self.root);
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
            Job::Refresh => self.send_status(true),
            Job::Diff {
                change,
                staged,
                token,
            } => self.send_diff(&change, staged, token, true),
            Job::Act(action) => {
                let result = run_action(&self.root, &action);
                let refs_changed = matches!(
                    action,
                    Action::Commit(_) | Action::Switch(_) | Action::CreateBranch(_)
                );
                self.outbox.send(Event::ActionDone { action, result });
                self.send_status(true);
                if refs_changed {
                    self.outbox.send_refs(&self.root);
                }
            }
        }
    }

    /// Looks for changes made outside the app and reports only real ones.
    fn poll(&mut self) {
        self.send_status(false);

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
