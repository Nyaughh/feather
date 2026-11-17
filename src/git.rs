//! Thin wrapper around the `git` command line.
//!
//! Shelling out (rather than linking libgit2) means commits honour the user's
//! hooks, signing, credential helpers and config exactly as their terminal does.
//! Every call here blocks, so apart from the one-off [`find_root`] when a
//! repository is opened they are only ever made from background threads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

fn command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root)
        // Never block on a credential prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        // We poll status; don't fight the user's own git commands for the index lock.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

struct Output {
    ok: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

fn exec(mut cmd: Command) -> Result<Output> {
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "git was not found on your PATH".to_owned(),
        _ => format!("could not run git: {e}"),
    })?;
    Ok(Output {
        ok: out.status.success(),
        code: out.status.code(),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    })
}

fn run(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = command(root);
    cmd.args(args);
    let out = exec(cmd)?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(error_text(&out, args))
    }
}

fn run_text(root: &Path, args: &[&str]) -> Result<String> {
    run(root, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn error_text(out: &Output, args: &[&str]) -> String {
    if !out.stderr.is_empty() {
        return out.stderr.clone();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_owned();
    }
    format!("git {} failed", args.first().copied().unwrap_or_default())
}

/// The top-level directory of the work tree containing `path`, if any.
pub fn find_root(path: &Path) -> Option<PathBuf> {
    let out = run_text(path, &["rev-parse", "--show-toplevel"]).ok()?;
    let line = out.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return None;
    }
    Some(PathBuf::from(line))
}

/// `owner/name` when the repository's `origin` is on GitHub.
pub fn github_slug(root: &Path) -> Option<String> {
    let url = run_text(root, &["config", "--get", "remote.origin.url"]).ok()?;
    parse_github_slug(url.trim())
}

fn parse_github_slug(url: &str) -> Option<String> {
    // https://github.com/owner/name.git, git@github.com:owner/name.git,
    // ssh://git@github.com/owner/name
    let (_, rest) = url.split_once("github.com")?;
    let rest = rest.strip_prefix([':', '/'])?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| format!("{owner}/{name}"))
}

// ── Status ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' | b'C' => Self::Renamed,
            _ => Self::Modified,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Path relative to the repository root, always with `/` separators.
    pub path: String,
    /// Previous path, for renames.
    pub orig_path: Option<String>,
    pub kind: ChangeKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Branch name, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated commit HEAD points at; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    /// Ignored paths (directories end with `/`).
    pub ignored: Vec<String>,
}

impl Status {
    pub fn change_count(&self) -> usize {
        self.staged.len() + self.unstaged.len()
    }

    pub fn branch_label(&self) -> String {
        match (&self.branch, &self.head) {
            (Some(b), _) => b.clone(),
            (None, Some(h)) => format!("{h} (detached)"),
            (None, None) => "no branch".to_owned(),
        }
    }
}

pub fn status(root: &Path) -> Result<Status> {
    let out = run(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    Ok(parse_status(&out))
}

fn parse_status(out: &[u8]) -> Status {
    let mut st = Status::default();
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let text = String::from_utf8_lossy(rec);
        if let Some(header) = text.strip_prefix("# ") {
            parse_header(header, &mut st);
            continue;
        }
        match rec[0] {
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let mut f = text.splitn(9, ' ');
                let xy = f.nth(1).unwrap_or("..").as_bytes().to_vec();
                if let Some(path) = f.nth(6) {
                    push_xy(&mut st, &xy, path, None);
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path NUL origPath
                let mut f = text.splitn(10, ' ');
                let xy = f.nth(1).unwrap_or("..").as_bytes().to_vec();
                let path = f.nth(7).map(str::to_owned);
                let orig = records
                    .next()
                    .map(|o| String::from_utf8_lossy(o).into_owned());
                if let Some(path) = path {
                    push_xy(&mut st, &xy, &path, orig);
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                if let Some(path) = text.splitn(11, ' ').nth(10) {
                    st.unstaged.push(Change {
                        path: path.to_owned(),
                        orig_path: None,
                        kind: ChangeKind::Conflicted,
                    });
                }
            }
            b'?' => st.unstaged.push(Change {
                path: text[2..].to_owned(),
                orig_path: None,
                kind: ChangeKind::Untracked,
            }),
            b'!' => st.ignored.push(text[2..].to_owned()),
            _ => {}
        }
    }
    st.staged.sort_by(|a, b| a.path.cmp(&b.path));
    st.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    st
}

fn parse_header(header: &str, st: &mut Status) {
    if let Some(oid) = header.strip_prefix("branch.oid ") {
        if oid != "(initial)" {
            st.head = Some(oid.chars().take(7).collect());
        }
    } else if let Some(name) = header.strip_prefix("branch.head ") {
        if name != "(detached)" {
            st.branch = Some(name.to_owned());
        }
    } else if let Some(up) = header.strip_prefix("branch.upstream ") {
        st.upstream = Some(up.to_owned());
    } else if let Some(ab) = header.strip_prefix("branch.ab ") {
        for part in ab.split(' ') {
            if let Some(n) = part.strip_prefix('+') {
                st.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix('-') {
                st.behind = n.parse().unwrap_or(0);
            }
        }
    }
}

fn push_xy(st: &mut Status, xy: &[u8], path: &str, orig: Option<String>) {
    let (x, y) = (
        xy.first().copied().unwrap_or(b'.'),
        xy.get(1).copied().unwrap_or(b'.'),
    );
    if x != b'.' {
        st.staged.push(Change {
            path: path.to_owned(),
            orig_path: orig.clone(),
            kind: ChangeKind::from_code(x),
        });
    }
    if y != b'.' {
        st.unstaged.push(Change {
            path: path.to_owned(),
            // A rename is recorded in the index; the worktree side is a plain edit.
            orig_path: None,
            kind: ChangeKind::from_code(y),
        });
    }
}

// ── Diff ───────────────────────────────────────────────────────────────────

/// Raw unified diff for one file.
pub fn diff(root: &Path, change: &Change, staged: bool) -> Result<String> {
    if change.kind == ChangeKind::Untracked {
        // `--no-index` exits 1 when the files differ, which is always.
        let mut cmd = command(root);
        cmd.args(["diff", "--no-color", "--no-index", "--", "/dev/null"])
            .arg(&change.path);
        let out = exec(cmd)?;
        return match out.code {
            Some(0) | Some(1) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            _ => Err(error_text(&out, &["diff"])),
        };
    }
    let mut args = vec!["diff", "--no-color", "-M"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    if let Some(orig) = &change.orig_path {
        args.push(orig);
    }
    args.push(&change.path);
    run_text(root, &args)
}

// ── Index operations ───────────────────────────────────────────────────────

pub fn stage(root: &Path, paths: &[String]) -> Result<()> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(root, &args).map(drop)
}

pub fn stage_all(root: &Path) -> Result<()> {
    run(root, &["add", "-A"]).map(drop)
}

fn has_head(root: &Path) -> bool {
    run(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<()> {
    // Before the first commit there is no HEAD to restore from.
    let mut args = if has_head(root) {
        vec!["restore", "--staged", "--"]
    } else {
        vec!["rm", "--cached", "-q", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    run(root, &args).map(drop)
}

pub fn unstage_all(root: &Path) -> Result<()> {
    if has_head(root) {
        run(root, &["reset", "-q"]).map(drop)
    } else {
        run(root, &["rm", "-r", "--cached", "-q", "."]).map(drop)
    }
}

/// Throw away worktree changes to one file. Untracked files are deleted.
pub fn discard(root: &Path, change: &Change) -> Result<()> {
    if change.kind == ChangeKind::Untracked {
        run(root, &["clean", "-f", "-q", "--", &change.path]).map(drop)
    } else {
        run(root, &["restore", "--worktree", "--", &change.path]).map(drop)
    }
}

/// Throws away every unstaged change: tracked files go back to what is
/// staged, and untracked files and folders are deleted.
pub fn discard_all(root: &Path) -> Result<()> {
    if has_head(root) || !run(root, &["ls-files"])?.is_empty() {
        run(root, &["restore", "--worktree", "--", "."])?;
    }
    run(root, &["clean", "-fd", "-q"]).map(drop)
}

pub fn commit(root: &Path, message: &str) -> Result<()> {
    let mut cmd = command(root);
    cmd.args(["commit", "-q", "-F", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(message.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Err(if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "git commit failed".to_owned()
    })
}

// ── Branches, worktrees, remotes ───────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The branch's own name, e.g. `feature/login`.
    pub name: String,
    /// For a branch that so far only exists on a remote: its remote-tracking
    /// name, e.g. `origin/feature/login`. `None` for local branches.
    pub remote: Option<String>,
}

/// Local branches, most recently committed first, followed by branches that
/// exist on a remote but have not been checked out here yet.
pub fn branches(root: &Path) -> Result<Vec<Branch>> {
    let out = run_text(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse_branches(&out))
}

fn parse_branches(refs: &str) -> Vec<Branch> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in refs.lines() {
        if let Some(name) = line.strip_prefix("refs/heads/") {
            local.push(Branch {
                name: name.to_owned(),
                remote: None,
            });
        } else if let Some(tracking) = line.strip_prefix("refs/remotes/")
            && let Some((_, name)) = tracking.split_once('/')
            && name != "HEAD"
        {
            remote.push(Branch {
                name: name.to_owned(),
                remote: Some(tracking.to_owned()),
            });
        }
    }
    // A remote branch that is already checked out locally is listed once, and
    // so is one that several remotes share.
    let mut seen: std::collections::HashSet<String> =
        local.iter().map(|b| b.name.clone()).collect();
    remote.retain(|b| seen.insert(b.name.clone()));
    local.extend(remote);
    local
}

/// Checks out a branch that only exists on a remote, creating the local
/// branch that tracks it.
pub fn track_branch(root: &Path, remote_branch: &str) -> Result<()> {
    run(root, &["switch", "-q", "--track", remote_branch]).map(drop)
}

pub fn switch_branch(root: &Path, name: &str) -> Result<()> {
    run(root, &["switch", "-q", name]).map(drop)
}

pub fn create_branch(root: &Path, name: &str) -> Result<()> {
    run(root, &["switch", "-q", "-c", name]).map(drop)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: String,
}

impl Worktree {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

pub fn worktrees(root: &Path) -> Result<Vec<Worktree>> {
    let out = run_text(root, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut bare = false;
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take().filter(|_| !bare) {
                list.push(wt);
            }
            bare = false;
            current = Some(Worktree {
                path: PathBuf::from(path),
                branch: None,
                head: String::new(),
            });
        } else if let Some(wt) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                wt.head = head.chars().take(7).collect();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                wt.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_owned(),
                );
            } else if line == "bare" {
                bare = true;
            }
        }
    }
    if let Some(wt) = current.filter(|_| !bare) {
        list.push(wt);
    }
    Ok(list)
}

pub fn fetch(root: &Path) -> Result<()> {
    run(root, &["fetch", "--prune", "-q"]).map(drop)
}

/// A fetch nobody asked for: it must never put a login prompt on screen.
pub fn fetch_in_background(root: &Path) -> Result<()> {
    let mut cmd = command(root);
    cmd.env("GCM_INTERACTIVE", "never").args([
        "-c",
        "credential.interactive=false",
        "fetch",
        "--prune",
        "-q",
    ]);
    let out = exec(cmd)?;
    if out.ok {
        Ok(())
    } else {
        Err(error_text(&out, &["fetch"]))
    }
}

pub fn pull(root: &Path) -> Result<()> {
    run(root, &["pull", "--ff-only", "-q"]).map(drop)
}

pub fn push(root: &Path, has_upstream: bool) -> Result<()> {
    if has_upstream {
        run(root, &["push", "-q"]).map(drop)
    } else {
        run(root, &["push", "-q", "-u", "origin", "HEAD"]).map(drop)
    }
}

// ── Search ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, in bytes, of the first match on the line.
    pub column: u32,
    /// The line, trimmed to a sensible length.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub files: Vec<FileMatches>,
    pub total: usize,
    /// More matches exist than were collected.
    pub truncated: bool,
}

/// Stop collecting after this many matches; nobody reads further than that.
const MAX_MATCHES: usize = 2000;
const MAX_MATCH_LINE: usize = 400;

/// Searches file contents with `git grep`, which honours `.gitignore`, skips
/// binary files, and is fast. `cancelled` is checked as results stream in.
pub fn search(
    root: &Path,
    is_git: bool,
    query: &str,
    options: SearchOptions,
    cancelled: &dyn Fn() -> bool,
) -> Result<SearchResults> {
    use std::io::{BufRead, BufReader};

    let mut cmd = command(root);
    cmd.args(["grep", "-n", "--column", "-I", "-z", "--no-color"]);
    if is_git {
        cmd.arg("--untracked");
    } else {
        cmd.arg("--no-index");
    }
    if !options.match_case {
        cmd.arg("-i");
    }
    if options.whole_word {
        cmd.arg("-w");
    }
    cmd.arg(if options.regex { "-E" } else { "-F" });
    cmd.arg("-e").arg(query).args(["--", "."]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run git: {e}"))?;
    let stdout = child.stdout.take().ok_or("no output from git")?;

    let mut results = SearchResults::default();
    let mut reader = BufReader::new(stdout);
    let mut record = Vec::new();
    loop {
        record.clear();
        // One match per line: path NUL line NUL column NUL text.
        if reader.read_until(b'\n', &mut record).unwrap_or(0) == 0 {
            break;
        }
        if cancelled() || results.total >= MAX_MATCHES {
            results.truncated = results.total >= MAX_MATCHES;
            let _ = child.kill();
            break;
        }
        let Some(found) = parse_grep_record(&record) else {
            continue;
        };
        results.total += 1;
        match results.files.last_mut() {
            Some(file) if file.path == found.0 => file.matches.push(found.1),
            _ => results.files.push(FileMatches {
                path: found.0,
                matches: vec![found.1],
            }),
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    // 1 means "no matches"; anything higher is a real error (a bad pattern).
    if results.total == 0 && status.code().is_some_and(|c| c > 1) {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        let message = stderr.trim().trim_start_matches("fatal: ");
        return Err(if message.is_empty() {
            "The search failed.".to_owned()
        } else {
            message.to_owned()
        });
    }
    Ok(results)
}

fn parse_grep_record(record: &[u8]) -> Option<(String, SearchMatch)> {
    let record = record.strip_suffix(b"\n").unwrap_or(record);
    let mut fields = record.splitn(4, |b| *b == 0);
    let path = String::from_utf8_lossy(fields.next()?).into_owned();
    let line = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
    let column = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
    let text = String::from_utf8_lossy(fields.next()?);
    let text = text.trim_end_matches('\r');
    let text = match text.char_indices().nth(MAX_MATCH_LINE) {
        Some((cut, _)) => &text[..cut],
        None => text,
    };
    Some((
        path,
        SearchMatch {
            line,
            column,
            text: text.to_owned(),
        },
    ))
}

/// Every file a quick-open should offer: tracked files plus untracked ones
/// that are not ignored.
pub fn list_files(root: &Path) -> Result<Vec<String>> {
    let out = run(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut files: Vec<String> = out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    files.sort_unstable();
    files.dedup();
    Ok(files)
}

// ── Blame ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// Abbreviated commit, or `None` for a line that is not committed yet.
    pub commit: Option<String>,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
}

/// Who last changed each line of a file, one entry per line.
pub fn blame(root: &Path, path: &str) -> Result<Vec<BlameLine>> {
    let out = run_text(root, &["blame", "--porcelain", "--", path])?;
    Ok(parse_blame(&out))
}

fn parse_blame(porcelain: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;

    // Commit details appear once, the first time a commit is mentioned.
    let mut commits: HashMap<String, BlameLine> = HashMap::new();
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in porcelain.lines() {
        if line.starts_with('\t') {
            // The source line itself: the header for it is complete.
            if let Some(info) = commits.get(&current) {
                lines.push(info.clone());
            }
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit()) {
            current = key.to_owned();
            let uncommitted = key.bytes().all(|b| b == b'0');
            commits.entry(current.clone()).or_insert_with(|| BlameLine {
                commit: (!uncommitted).then(|| key[..7].to_owned()),
                author: String::new(),
                time: 0,
                summary: String::new(),
            });
        } else if let Some(info) = commits.get_mut(&current) {
            match key {
                "author" => info.author = value.to_owned(),
                "author-time" => info.time = value.parse().unwrap_or(0),
                "summary" => info.summary = value.to_owned(),
                _ => {}
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_github_repository_from_a_remote_address() {
        for url in [
            "https://github.com/Nyaughh/feather.git",
            "https://github.com/Nyaughh/feather",
            "https://user@github.com/Nyaughh/feather/",
            "git@github.com:Nyaughh/feather.git",
            "ssh://git@github.com/Nyaughh/feather.git",
        ] {
            assert_eq!(
                parse_github_slug(url).as_deref(),
                Some("Nyaughh/feather"),
                "{url}"
            );
        }
        assert_eq!(parse_github_slug("https://gitlab.com/a/b.git"), None);
        assert_eq!(parse_github_slug("https://github.com/only-owner"), None);
        assert_eq!(parse_github_slug("/srv/git/feather.git"), None);
    }

    #[test]
    fn parses_porcelain_v2() {
        let raw = b"# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/main.rs\0\
1 A. N... 000000 100644 100644 aaa bbb new file.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs\0old name.rs\0\
1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? notes.md\0! target/\0";
        let st = parse_status(raw);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head.as_deref(), Some("1234567"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));

        let staged: Vec<_> = st
            .staged
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            staged,
            [
                ("both.rs", ChangeKind::Modified),
                ("new file.txt", ChangeKind::Added),
                ("new name.rs", ChangeKind::Renamed),
            ]
        );
        assert_eq!(st.staged[2].orig_path.as_deref(), Some("old name.rs"));

        let unstaged: Vec<_> = st
            .unstaged
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            unstaged,
            [
                ("both.rs", ChangeKind::Modified),
                ("conflict.rs", ChangeKind::Conflicted),
                ("notes.md", ChangeKind::Untracked),
                ("src/main.rs", ChangeKind::Modified),
            ]
        );
        assert_eq!(st.ignored, ["target/"]);
    }

    /// A scratch repository that is removed when dropped.
    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("feather-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = Self(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            // Keep file contents byte-identical on Windows too.
            repo.git(&["config", "core.autocrlf", "false"]);
            repo
        }

        fn git(&self, args: &[&str]) {
            run(&self.0, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn status(&self) -> Status {
            status(&self.0).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn paths(list: &[Change]) -> Vec<&str> {
        list.iter().map(|c| c.path.as_str()).collect()
    }

    #[test]
    fn stages_unstages_and_commits_from_an_empty_repository() {
        let repo = TempRepo::new("unborn");
        let root = &repo.0;
        repo.write("a.txt", "one\n");
        repo.write("dir/b.txt", "two\n");

        let st = repo.status();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head, None);
        assert_eq!(paths(&st.unstaged), ["a.txt", "dir/b.txt"]);
        assert!(st.unstaged.iter().all(|c| c.kind == ChangeKind::Untracked));

        // Staging and unstaging work before there is any commit to restore from.
        stage(root, &["a.txt".to_owned()]).unwrap();
        assert_eq!(paths(&repo.status().staged), ["a.txt"]);
        unstage(root, &["a.txt".to_owned()]).unwrap();
        assert!(repo.status().staged.is_empty());
        stage_all(root).unwrap();
        assert_eq!(paths(&repo.status().staged), ["a.txt", "dir/b.txt"]);
        unstage_all(root).unwrap();
        assert!(repo.status().staged.is_empty());

        stage_all(root).unwrap();
        commit(root, "First commit\n\nWith a body.").unwrap();
        let st = repo.status();
        assert!(st.head.is_some());
        assert_eq!(st.change_count(), 0);
        assert_eq!(
            run_text(root, &["log", "-1", "--format=%s|%b"])
                .unwrap()
                .trim(),
            "First commit|With a body."
        );
    }

    #[test]
    fn diffs_discards_and_switches_branches() {
        let repo = TempRepo::new("flow");
        let root = &repo.0;
        repo.write("a.txt", "one\ntwo\n");
        repo.write("old name.txt", "same content\nline two\nline three\n");
        stage_all(root).unwrap();
        commit(root, "init").unwrap();

        repo.write("a.txt", "one\nTWO\n");
        repo.write("new.txt", "fresh\n");
        repo.git(&["mv", "old name.txt", "new name.txt"]);
        let st = repo.status();
        assert_eq!(paths(&st.unstaged), ["a.txt", "new.txt"]);
        let rename = &st.staged[0];
        assert_eq!(rename.kind, ChangeKind::Renamed);
        assert_eq!(rename.orig_path.as_deref(), Some("old name.txt"));

        // Diff of a tracked edit, of an untracked file, and of a staged rename.
        let d = crate::diff::parse(&diff(root, &st.unstaged[0], false).unwrap());
        assert_eq!((d.additions, d.deletions), (1, 1));
        let d = crate::diff::parse(&diff(root, &st.unstaged[1], false).unwrap());
        assert_eq!((d.additions, d.deletions), (1, 0));
        let d = crate::diff::parse(&diff(root, rename, true).unwrap());
        assert!(d.is_empty(), "a pure rename has no content changes");

        // Unstaging a rename needs both of its paths.
        unstage(
            root,
            &["new name.txt".to_owned(), "old name.txt".to_owned()],
        )
        .unwrap();
        assert!(repo.status().staged.is_empty());
        repo.git(&["add", "-A", "--", "new name.txt", "old name.txt"]);

        // Discard restores tracked files and deletes untracked ones.
        discard(root, &st.unstaged[0]).unwrap();
        discard(root, &st.unstaged[1]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert!(!root.join("new.txt").exists());
        assert!(repo.status().unstaged.is_empty());
        commit(root, "rename").unwrap();

        create_branch(root, "feature/x").unwrap();
        assert_eq!(repo.status().branch.as_deref(), Some("feature/x"));
        let mut all: Vec<String> = branches(root)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        all.sort();
        assert_eq!(all, ["feature/x", "main"]);

        // A branch that only exists on a remote is listed, and can be checked out.
        repo.git(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ]);
        repo.git(&["update-ref", "refs/remotes/origin/from-remote", "HEAD"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let listed = branches(root).unwrap();
        let remote_only: Vec<_> = listed.iter().filter(|b| b.remote.is_some()).collect();
        assert_eq!(remote_only.len(), 1, "origin/main is already local");
        assert_eq!(remote_only[0].name, "from-remote");
        assert_eq!(remote_only[0].remote.as_deref(), Some("origin/from-remote"));
        track_branch(root, "origin/from-remote").unwrap();
        let st = repo.status();
        assert_eq!(st.branch.as_deref(), Some("from-remote"));
        assert_eq!(st.upstream.as_deref(), Some("origin/from-remote"));
        switch_branch(root, "main").unwrap();
        switch_branch(root, "main").unwrap();
        assert_eq!(repo.status().branch.as_deref(), Some("main"));
        assert!(switch_branch(root, "does-not-exist").is_err());

        let trees = worktrees(root).unwrap();
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].branch.as_deref(), Some("main"));
        assert_eq!(
            find_root(&root.join(".")).map(|p| p.canonicalize().unwrap()),
            Some(root.canonicalize().unwrap())
        );
    }

    #[test]
    fn searches_blames_lists_and_discards_everything() {
        let repo = TempRepo::new("search");
        let root = &repo.0;
        repo.write("src/a.rs", "fn main() {\n    let Needle = 1;\n}\n");
        repo.write(
            "notes.txt",
            "a needle in a haystack\nno match here\nneedles\n",
        );
        repo.write(".gitignore", "ignored/\n");
        repo.write("ignored/x.txt", "needle\n");
        stage_all(root).unwrap();
        commit(root, "first").unwrap();
        repo.write("new.txt", "untracked needle\n");

        let never = || false;
        let find = |query: &str, options: SearchOptions| {
            search(root, true, query, options, &never).unwrap()
        };
        // Case-insensitive by default; untracked files count, ignored ones don't.
        let all = find("needle", SearchOptions::default());
        let paths: Vec<_> = all.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["new.txt", "notes.txt", "src/a.rs"]);
        assert_eq!(all.total, 4);
        let code = &all.files[2].matches[0];
        assert_eq!((code.line, code.column), (2, 9));
        assert_eq!(code.text, "    let Needle = 1;");

        let exact = find(
            "Needle",
            SearchOptions {
                match_case: true,
                ..Default::default()
            },
        );
        assert_eq!(exact.total, 1);
        let words = find(
            "needle",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
        );
        assert_eq!(words.total, 3, "\"needles\" is not the whole word");
        let pattern = find(
            "ne+dles?$",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        assert_eq!(pattern.total, 2);
        assert_eq!(find("nothing-like-this", SearchOptions::default()).total, 0);
        assert!(
            search(
                root,
                true,
                "(",
                SearchOptions {
                    regex: true,
                    ..Default::default()
                },
                &never
            )
            .is_err()
        );

        let files = list_files(root).unwrap();
        assert_eq!(files, [".gitignore", "new.txt", "notes.txt", "src/a.rs"]);

        let who = blame(root, "notes.txt").unwrap();
        assert_eq!(who.len(), 3);
        assert_eq!(who[0].author, "Test");
        assert_eq!(who[0].summary, "first");
        assert!(who[0].commit.is_some() && who[0].time > 0);

        // Discard all: edits are reverted and untracked files removed.
        repo.write("notes.txt", "changed\n");
        repo.write("dir/extra.txt", "x\n");
        assert_eq!(repo.status().unstaged.len(), 3);
        discard_all(root).unwrap();
        assert!(repo.status().unstaged.is_empty());
        assert!(!root.join("new.txt").exists() && !root.join("dir").exists());
        assert!(
            root.join("ignored/x.txt").exists(),
            "ignored files are left alone"
        );
        assert!(
            std::fs::read_to_string(root.join("notes.txt"))
                .unwrap()
                .starts_with("a needle")
        );
    }

    #[test]
    fn lists_local_branches_before_remote_only_ones() {
        let refs = "refs/heads/main\nrefs/remotes/origin/HEAD\nrefs/remotes/origin/aarin/tn-case-type\n\
                    refs/remotes/origin/main\nrefs/heads/topic\nrefs/remotes/fork/aarin/tn-case-type\n";
        let names: Vec<_> = parse_branches(refs)
            .into_iter()
            .map(|b| (b.name, b.remote))
            .collect();
        assert_eq!(
            names,
            [
                ("main".to_owned(), None),
                ("topic".to_owned(), None),
                (
                    "aarin/tn-case-type".to_owned(),
                    Some("origin/aarin/tn-case-type".to_owned())
                ),
            ]
        );
    }

    #[test]
    fn parses_unborn_and_detached() {
        let st = parse_status(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(st.head, None);
        assert_eq!(st.branch_label(), "main");

        let st = parse_status(b"# branch.oid abcdef1234\0# branch.head (detached)\0");
        assert_eq!(st.branch, None);
        assert_eq!(st.branch_label(), "abcdef1 (detached)");
    }
}
