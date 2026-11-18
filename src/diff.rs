//! Unified diff parsing. Turns `git diff` output into flat rows the diff view
//! can render (and virtualise) directly.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Context,
    Add,
    Del,
    /// "\ No newline at end of file" and similar.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    /// Widest line in characters, for sizing the horizontal scroll area.
    pub max_chars: usize,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && !self.binary
    }
}

/// One file's part of a patch that covers several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub diff: Diff,
}

/// Splits a patch from `git show` into its files.
pub fn parse_files(raw: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut start = None;
    let mut offset = 0;
    let flush = |chunk: &str, files: &mut Vec<FileDiff>| {
        files.push(FileDiff {
            path: patch_path(chunk),
            diff: parse(chunk),
        });
    };
    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(from) = start {
                flush(&raw[from..offset], &mut files);
            }
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(from) = start {
        flush(&raw[from..], &mut files);
    }
    files
}

/// The path a file's patch is about: where it ends up, or where it was if
/// it was deleted.
fn patch_path(chunk: &str) -> String {
    let mut deleted_from = None;
    for line in chunk.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            return path.to_owned();
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            return path.to_owned();
        }
        if let Some(path) = line.strip_prefix("--- a/") {
            deleted_from = Some(path.to_owned());
        }
        if line.starts_with("@@") {
            break;
        }
    }
    if let Some(path) = deleted_from {
        return path;
    }
    // A file with no content change (a mode change, or binary): the header
    // is "diff --git a/<path> b/<path>".
    let header = chunk.lines().next().unwrap_or("");
    let rest = header.strip_prefix("diff --git a/").unwrap_or(header);
    match rest.rsplit_once(" b/") {
        Some((_, path)) => path.to_owned(),
        None => rest.to_owned(),
    }
}

pub fn parse(raw: &str) -> Diff {
    let mut diff = Diff::default();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_hunk = false;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("@@")
            && let Some((old, new)) = parse_hunk_header(rest)
        {
            old_no = old;
            new_no = new;
            in_hunk = true;
            diff.lines.push(DiffLine {
                kind: LineKind::Hunk,
                old_no: None,
                new_no: None,
                text: line.to_owned(),
            });
            continue;
        }
        if !in_hunk {
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                diff.binary = true;
            }
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Add, &line[1..]),
            Some(b'-') => (LineKind::Del, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'\\') => (LineKind::Note, line),
            // A new file header: the hunk is over.
            Some(b'd') if line.starts_with("diff --git") => {
                in_hunk = false;
                continue;
            }
            // Blank context lines can lose their leading space in transit.
            None => (LineKind::Context, ""),
            _ => continue,
        };
        let (o, n) = match kind {
            LineKind::Add => {
                diff.additions += 1;
                new_no += 1;
                (None, Some(new_no - 1))
            }
            LineKind::Del => {
                diff.deletions += 1;
                old_no += 1;
                (Some(old_no - 1), None)
            }
            LineKind::Context => {
                old_no += 1;
                new_no += 1;
                (Some(old_no - 1), Some(new_no - 1))
            }
            _ => (None, None),
        };
        let text = expand_tabs(text);
        diff.max_chars = diff.max_chars.max(text.chars().count());
        diff.lines.push(DiffLine {
            kind,
            old_no: o,
            new_no: n,
            text,
        });
    }
    diff
}

/// Parses the ` -a,b +c,d @@ …` tail of a hunk header into starting line numbers.
fn parse_hunk_header(rest: &str) -> Option<(u32, u32)> {
    let mut parts = rest.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

pub fn expand_tabs(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_commit_into_its_files() {
        let raw = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\ndiff --git a/logo.png b/logo.png\nnew file mode 100644\nBinary files /dev/null and b/logo.png differ\ndiff --git a/old name.txt b/new name.txt\nsimilarity index 100%\nrename from old name.txt\nrename to new name.txt\n";
        let files = parse_files(raw);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/a.rs", "gone.txt", "logo.png", "new name.txt"]);
        assert_eq!((files[0].diff.additions, files[0].diff.deletions), (1, 1));
        assert_eq!(files[1].diff.deletions, 1);
        assert!(files[2].diff.binary);
        assert!(files[3].diff.is_empty());
        assert_eq!(parse_files(""), []);
    }

    #[test]
    fn parses_hunks_and_numbers() {
        let raw = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n\
@@ -10,4 +10,5 @@ fn main() {\n context\n-removed\n+added one\n+added two\n context\n\\ No newline at end of file\n";
        let d = parse(raw);
        assert_eq!((d.additions, d.deletions), (2, 1));
        assert!(!d.binary);
        let kinds: Vec<_> = d.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Hunk,
                LineKind::Context,
                LineKind::Del,
                LineKind::Add,
                LineKind::Add,
                LineKind::Context,
                LineKind::Note
            ]
        );
        assert_eq!((d.lines[1].old_no, d.lines[1].new_no), (Some(10), Some(10)));
        assert_eq!((d.lines[2].old_no, d.lines[2].new_no), (Some(11), None));
        assert_eq!((d.lines[3].old_no, d.lines[3].new_no), (None, Some(11)));
        assert_eq!((d.lines[5].old_no, d.lines[5].new_no), (Some(12), Some(13)));
    }

    #[test]
    fn lines_that_look_like_headers_stay_in_the_hunk() {
        let raw = "@@ -1,2 +1,2 @@\n---- a markdown rule\n+++ plus plus\n";
        let d = parse(raw);
        assert_eq!(d.lines[1].kind, LineKind::Del);
        assert_eq!(d.lines[1].text, "--- a markdown rule");
        assert_eq!(d.lines[2].kind, LineKind::Add);
        assert_eq!(d.lines[2].text, "++ plus plus");
    }

    #[test]
    fn detects_binary() {
        let d = parse("diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n");
        assert!(d.binary && d.lines.is_empty());
    }

    #[test]
    fn expands_tabs_to_stops() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tx"), "ab  x");
    }
}
