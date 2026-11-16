//! Loading files for the viewer and syntax-highlighting them. All of this runs
//! off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Files larger than this are not loaded into the viewer.
const MAX_TEXT_BYTES: u64 = 5 * 1024 * 1024;
/// Highlighting is skipped above this size; plain text is still shown.
const MAX_HIGHLIGHT_BYTES: u64 = 1024 * 1024;
/// Larger files open read-only: a text field holding megabytes gets sluggish.
const MAX_EDIT_BYTES: u64 = 1024 * 1024;
/// Lines longer than this are truncated on screen (minified bundles etc).
const MAX_LINE_CHARS: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    None,
    Markdown,
    Svg,
}

pub fn preview_kind(path: &str) -> Preview {
    match extension(path).as_str() {
        "md" | "markdown" | "mdown" | "mkd" => Preview::Markdown,
        "svg" => Preview::Svg,
        _ => Preview::None,
    }
}

pub fn is_image(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
    )
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// One run within a line: bytes up to `end` belong to a syntax token class
/// (see [`crate::theme::syntax`]). The theme turns the class into a color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub end: u32,
    pub class: u8,
}

pub type Highlights = Vec<Vec<Span>>;

pub struct TextDoc {
    /// The file's text with line endings normalised to `\n`.
    pub raw: String,
    /// One entry per line. Over-long lines are cut short in files that are
    /// not [`editable`](Self::editable).
    pub lines: Vec<String>,
    /// Widest line in columns, for sizing the horizontal scroll area.
    pub max_chars: usize,
    pub highlights: Option<Highlights>,
    /// The file used Windows line endings, to be restored on save.
    pub crlf: bool,
    /// False for files that are too big to edit comfortably, or that are not
    /// valid UTF-8 (saving those would damage them).
    pub editable: bool,
}

pub enum Body {
    Text(TextDoc),
    Image,
    Binary,
    TooLarge,
    Error(String),
}

/// Size and modification time; `None` when the file cannot be read at all.
pub type FileStamp = Option<(u64, Option<SystemTime>)>;

pub fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

pub struct LoadedFile {
    pub path: PathBuf,
    pub size: u64,
    /// The file's stamp when it was read, for noticing later edits.
    pub stamp: FileStamp,
    pub body: Body,
}

impl LoadedFile {
    /// The URI egui's image loaders know this file by.
    pub fn uri(&self) -> String {
        format!("file://{}", self.path.display())
    }
}

pub fn load(path: &Path, rel: &str) -> LoadedFile {
    let stamp = file_stamp(path);
    let size = stamp.map_or(0, |(size, _)| size);
    let body = if stamp.is_none() {
        Body::Error("This file no longer exists.".to_owned())
    } else if is_image(rel) {
        Body::Image
    } else if size > MAX_TEXT_BYTES {
        Body::TooLarge
    } else {
        match std::fs::read(path) {
            Err(e) => Body::Error(e.to_string()),
            Ok(bytes) if looks_binary(&bytes) => Body::Binary,
            Ok(bytes) => Body::Text(text_doc(bytes)),
        }
    };
    LoadedFile {
        path: path.to_owned(),
        size,
        stamp,
        body,
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

fn text_doc(bytes: Vec<u8>) -> TextDoc {
    let size = bytes.len() as u64;
    let (mut raw, lossy) = match String::from_utf8(bytes) {
        Ok(s) => (s, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let crlf = raw.contains("\r\n");
    if crlf {
        raw = raw.replace("\r\n", "\n");
    }
    let too_long = raw.lines().any(|line| line.len() > MAX_LINE_CHARS * 4);
    let editable = !lossy && size <= MAX_EDIT_BYTES && !too_long;

    let mut max_chars = 0;
    let lines: Vec<String> = raw
        .lines()
        .map(|line| {
            let mut line = line.to_owned();
            let mut chars = columns(&line);
            if !editable && chars > MAX_LINE_CHARS {
                let cut = line
                    .char_indices()
                    .nth(MAX_LINE_CHARS)
                    .map_or(line.len(), |(i, _)| i);
                line.truncate(cut);
                line.push('…');
                chars = MAX_LINE_CHARS + 1;
            }
            max_chars = max_chars.max(chars);
            line
        })
        .collect();
    TextDoc {
        raw,
        lines,
        max_chars,
        highlights: None,
        crlf,
        editable,
    }
}

/// Width of a line in columns, counting a tab as four.
pub fn columns(line: &str) -> usize {
    line.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

// ── Syntax highlighting ────────────────────────────────────────────────────

struct Syntax {
    set: SyntaxSet,
    theme: Theme,
}

fn syntax() -> &'static Syntax {
    static SYNTAX: OnceLock<Syntax> = OnceLock::new();
    SYNTAX.get_or_init(|| Syntax {
        set: two_face::syntax::extra_newlines(),
        theme: build_theme(),
    })
}

/// Loads the grammar set ahead of the first file open.
pub fn warm_up() {
    syntax();
}

fn find_syntax<'a>(set: &'a SyntaxSet, rel: &str, first_line: &str) -> Option<&'a SyntaxReference> {
    let path = Path::new(rel);
    let name = path.file_name()?.to_string_lossy();
    let ext = extension(rel);
    // A few common extensions the bundled grammars don't claim themselves.
    let ext = match ext.as_str() {
        "mjs" | "cjs" => "js",
        "mts" | "cts" => "ts",
        "jsonc" | "json5" | "webmanifest" => "json",
        "mdx" => "md",
        "zsh" | "bash" | "fish" => "sh",
        "lock" if name == "Cargo.lock" => "toml",
        other => other,
    };
    set.find_syntax_by_extension(&name)
        .or_else(|| set.find_syntax_by_extension(ext))
        .or_else(|| set.find_syntax_by_first_line(first_line))
}

/// Whether a file of this size gets syntax colors at all.
pub fn can_highlight(size: u64) -> bool {
    size <= MAX_HIGHLIGHT_BYTES
}

/// Highlights `lines`, giving up early (returning `None`) if `generation`
/// moves on from `token` - i.e. the user has opened something else.
pub fn highlight(
    lines: &[String],
    rel: &str,
    size: u64,
    generation: &AtomicU64,
    token: u64,
) -> Option<Highlights> {
    if !can_highlight(size) || lines.is_empty() {
        return None;
    }
    let syn = syntax();
    let reference = find_syntax(&syn.set, rel, &lines[0])?;
    if reference.name == "Plain Text" {
        return None;
    }
    let mut hl = HighlightLines::new(reference, &syn.theme);
    let mut out = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i % 256 == 0 && generation.load(Ordering::Relaxed) != token {
            return None;
        }
        buf.clear();
        buf.push_str(line);
        buf.push('\n');
        let Ok(ranges) = hl.highlight_line(&buf, &syn.set) else {
            return None;
        };
        let mut spans: Vec<Span> = Vec::with_capacity(ranges.len());
        let mut end = 0u32;
        for (style, piece) in ranges {
            end += piece.len() as u32;
            // The theme below stores the token class in the red channel.
            let class = style.foreground.r;
            match spans.last_mut() {
                Some(last) if last.class == class => last.end = end,
                _ => spans.push(Span { end, class }),
            }
        }
        // Drop the newline we added.
        if let Some(last) = spans.last_mut() {
            last.end = last.end.min(line.len() as u32);
        }
        out.push(spans);
    }
    Some(out)
}

/// A syntect theme whose "colors" are token classes: the red channel carries
/// the class number, which [`highlight`] reads back out.
fn build_theme() -> Theme {
    use crate::theme::syntax as class;

    let rules: &[(&str, u8)] = &[
        ("comment, punctuation.definition.comment", class::COMMENT),
        (
            "punctuation, meta.brace, keyword.operator",
            class::PUNCTUATION,
        ),
        ("keyword, storage, keyword.operator.word", class::KEYWORD),
        (
            "string, punctuation.definition.string, markup.raw, markup.inserted, markup.quote",
            class::STRING,
        ),
        (
            "constant.numeric, constant.language, constant.other, support.constant, \
             variable.other.constant, string.regexp, entity.other.attribute-name, \
             variable.language, variable.parameter.option",
            class::NUMBER,
        ),
        (
            "constant.character.escape, constant.other.placeholder, markup.bold, markup.italic, \
             markup.underline.link, variable.annotation, meta.annotation, entity.name.label",
            class::ESCAPE,
        ),
        (
            "entity.name.function, support.function, variable.function, meta.function-call, \
             markup.heading, entity.name.section, meta.mapping.key string, \
             meta.mapping.key punctuation.definition.string, support.type.property-name, \
             meta.property-name, entity.name.tag.yaml, entity.name.tag.toml, meta.tag.key",
            class::FUNCTION,
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
             entity.name.trait, entity.name.namespace, support.type, support.class, \
             entity.other.inherited-class",
            class::TYPE,
        ),
        ("entity.name.tag, markup.deleted, invalid", class::TAG),
    ];

    let marker = |class: u8| Color {
        r: class,
        g: 0,
        b: 0,
        a: 0xff,
    };
    let scopes = rules
        .iter()
        .filter_map(|(selector, class)| {
            Some(ThemeItem {
                scope: selector.parse::<ScopeSelectors>().ok()?,
                style: StyleModifier {
                    foreground: Some(marker(*class)),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect();

    Theme {
        name: Some("Feather".to_owned()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(marker(class::TEXT)),
            ..ThemeSettings::default()
        },
        scopes,
    }
}

/// Highlights a whole text; `None` if the language is unknown, the text is too
/// big, or `generation` moved past `token` first.
pub fn highlight_text(
    text: &str,
    rel: &str,
    generation: &AtomicU64,
    token: u64,
) -> Option<Highlights> {
    let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    highlight(&lines, rel, text.len() as u64, generation, token)
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_and_keeps_spans_within_lines() {
        let lines = vec![
            "fn main() {".to_owned(),
            "    let x = \"hi\"; // note".to_owned(),
            "}".to_owned(),
        ];
        let generation = AtomicU64::new(1);
        let hl = highlight(&lines, "src/main.rs", 40, &generation, 1).expect("rust is known");
        assert_eq!(hl.len(), lines.len());
        for (spans, line) in hl.iter().zip(&lines) {
            assert_eq!(spans.last().unwrap().end as usize, line.len());
            assert!(spans.windows(2).all(|w| w[0].end <= w[1].end));
        }
        // More than one color on a line with a keyword, string and comment.
        assert!(hl[1].len() >= 3);
    }

    #[test]
    fn highlight_is_cancelled_by_a_newer_generation() {
        let lines = vec!["fn main() {}".to_owned()];
        let generation = AtomicU64::new(2);
        assert!(highlight(&lines, "a.rs", 10, &generation, 1).is_none());
    }

    #[test]
    fn knows_common_extensions() {
        let set = &syntax().set;
        for name in [
            "a.rs",
            "a.ts",
            "a.tsx",
            "a.toml",
            "a.py",
            "a.go",
            "a.json",
            "a.yml",
            "a.css",
            "a.html",
            "a.sh",
            "Dockerfile",
            "Makefile",
            "a.md",
            "a.js",
            "a.c",
            "a.cpp",
            "a.java",
            "a.kt",
            "a.swift",
            "a.rb",
            "a.sql",
            "a.lua",
        ] {
            assert!(find_syntax(set, name, "").is_some(), "no syntax for {name}");
        }
    }

    #[test]
    fn classifies_content() {
        assert!(looks_binary(b"\x89PNG\0\0"));
        assert!(!looks_binary(b"plain text"));
        assert_eq!(preview_kind("docs/README.MD"), Preview::Markdown);
        assert_eq!(preview_kind("logo.svg"), Preview::Svg);
        assert!(is_image("a/b.PNG"));
        let doc = text_doc(b"a\tb\r\nsecond".to_vec());
        assert_eq!(doc.lines, ["a\tb", "second"]);
        assert_eq!(doc.raw, "a\tb\nsecond");
        assert_eq!(doc.max_chars, 6);
        assert!(doc.crlf && doc.editable);
        assert!(!text_doc(vec![b'a', 0xff, b'b']).editable);
    }
}
