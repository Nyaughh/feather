//! Gets a Markdown document ready for the viewer.
//!
//! READMEs are full of HTML: a centered logo in an `<img>` tag, badges inside
//! `<p align="center">`, a `<details>` block. The viewer only understands
//! Markdown and would print those tags as text, so the common ones are turned
//! into the Markdown that means the same thing. Tags it does not know are left
//! exactly as they were, and nothing inside code is touched.

use std::borrow::Cow;

use crate::images;

/// Widths given as a percentage are taken of this, the reading column.
const COLUMN: f32 = 780.0;

pub fn prepare(text: &str) -> Cow<'_, str> {
    if !text.contains('<') {
        return Cow::Borrowed(text);
    }
    let mut converter = Converter {
        out: String::with_capacity(text.len()),
        links: Vec::new(),
        depth: 0,
        line_start: 0,
        blank_line: true,
        table_row: false,
        swallow_space: false,
    };
    converter.run(text);
    Cow::Owned(converter.out)
}

struct Converter {
    out: String,
    /// The `href` of each `<a>` that is open, innermost last.
    links: Vec<Option<String>>,
    /// How many block tags (`<p>`, `<div>`, ...) are open. Inside them,
    /// indentation is layout, not a code block.
    depth: usize,
    /// Where the current line starts in `out`.
    line_start: usize,
    /// Nothing but whitespace has been written on the current line.
    blank_line: bool,
    /// The current source line is a row of a Markdown table.
    table_row: bool,
    /// Skip whitespace until the next visible character.
    swallow_space: bool,
}

struct Tag<'a> {
    name: String,
    closing: bool,
    attributes: &'a str,
}

impl Tag<'_> {
    /// The value of an attribute, with quotes removed.
    fn get(&self, wanted: &str) -> Option<&str> {
        let mut rest = self.attributes;
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
            if rest.is_empty() {
                return None;
            }
            let name_end = rest
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
                .unwrap_or(rest.len());
            let name = &rest[..name_end];
            rest = rest[name_end..].trim_start();
            let mut value = "";
            if let Some(after) = rest.strip_prefix('=') {
                let after = after.trim_start();
                let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'');
                if let Some(quote) = quote {
                    let end = after[1..].find(quote).map_or(after.len(), |i| i + 1);
                    value = &after[1..end];
                    rest = after.get(end + 1..).unwrap_or("");
                } else {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    value = &after[..end];
                    rest = &after[end..];
                }
            }
            if name.is_empty() {
                // Something unexpected. Step over it rather than loop forever.
                let mut chars = rest.chars();
                chars.next();
                rest = chars.as_str();
            } else if name.eq_ignore_ascii_case(wanted) {
                return Some(value);
            }
        }
    }

    /// A `width` or `height`, in points.
    fn length(&self, name: &str) -> Option<u32> {
        let value = self.get(name)?.trim();
        if let Some(percent) = value.strip_suffix('%') {
            let percent: f32 = percent.trim().parse().ok()?;
            return (name == "width").then(|| (COLUMN * percent / 100.0).round() as u32);
        }
        let value = value.strip_suffix("px").unwrap_or(value).trim();
        let points: f32 = value.parse().ok()?;
        (points >= 1.0).then(|| points.round() as u32)
    }
}

/// Reads the tag at the start of `text`, which begins with `<`. Returns it
/// and its length in bytes.
fn read_tag(text: &str) -> Option<(Tag<'_>, usize)> {
    let body = &text[1..];
    let (closing, body) = match body.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let name_len = body
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(body.len());
    let name = &body[..name_len];
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    // `<https://...>` and `<me@example.com>` are links, not tags.
    let after = body[name_len..].chars().next()?;
    if !(after.is_whitespace() || after == '>' || after == '/') {
        return None;
    }
    // The tag ends at the first `>` outside a quoted attribute.
    let mut quote = None;
    let mut end = None;
    for (i, c) in body[name_len..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => {
                end = Some(name_len + i);
                break;
            }
            (None, '<') => return None,
            _ => {}
        }
    }
    let end = end?;
    let tag = Tag {
        name: name.to_ascii_lowercase(),
        closing,
        attributes: &body[name_len..end],
    };
    let consumed = text.len() - body.len() + end + 1;
    Some((tag, consumed))
}

/// A fenced code block opening: its marker character and length.
fn fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some((marker, length))
}

impl Converter {
    fn run(&mut self, text: &str) {
        let mut i = 0;
        let mut at_line_start = true;
        while i < text.len() {
            if at_line_start {
                at_line_start = false;
                self.line_start = self.out.len();
                self.blank_line = true;
                let line_end = text[i..].find('\n').map_or(text.len(), |n| i + n);
                let line = &text[i..line_end];
                self.table_row = line.trim_start().starts_with('|');
                if let Some((marker, length)) = fence(line) {
                    // Copy the whole block through untouched.
                    let mut end = (line_end + 1).min(text.len());
                    while end < text.len() {
                        let next = text[end..].find('\n').map_or(text.len(), |n| end + n);
                        let closes = fence(&text[end..next])
                            .is_some_and(|(m, l)| m == marker && l >= length)
                            && text[end..next].trim().chars().all(|c| c == marker);
                        end = (next + 1).min(text.len());
                        if closes {
                            break;
                        }
                    }
                    self.out.push_str(&text[i..end]);
                    i = end;
                    at_line_start = true;
                    continue;
                }
            }
            let rest = &text[i..];
            let c = rest.chars().next().unwrap_or(' ');
            match c {
                '\n' => {
                    if !self.swallow_space {
                        self.out.push('\n');
                    }
                    at_line_start = true;
                    i += 1;
                }
                ' ' | '\t' if self.swallow_space || (self.blank_line && self.depth > 0) => i += 1,
                '`' => {
                    // An inline code span: copy to the matching run of backticks.
                    let run = rest.chars().take_while(|c| *c == '`').count();
                    let ticks = &rest[..run];
                    let end = rest[run..]
                        .find(ticks)
                        .map_or(run, |close| run + close + run);
                    self.text(&rest[..end]);
                    i += end;
                }
                '<' => {
                    if let Some(end) = rest.strip_prefix("<!--").map(|r| r.find("-->")) {
                        // A comment. One left open hides the rest, as in a browser.
                        i += end.map_or(rest.len(), |end| end + 7);
                    } else if let Some((tag, length)) = read_tag(rest) {
                        if let Some(skip) = self.tag(&tag, &rest[length..]) {
                            i += length + skip;
                        } else {
                            self.text(&rest[..length]);
                            i += length;
                        }
                    } else {
                        self.text("<");
                        i += 1;
                    }
                }
                _ => {
                    let length = c.len_utf8();
                    self.text(&rest[..length]);
                    i += length;
                }
            }
        }
    }

    /// Writes ordinary text.
    fn text(&mut self, text: &str) {
        self.swallow_space = false;
        if !text.trim().is_empty() {
            self.blank_line = false;
        }
        self.out.push_str(text);
    }

    /// Writes Markdown that stands in for a tag. Indentation in front of it
    /// was laying out the HTML and would now start a code block, so it goes.
    fn emit(&mut self, markdown: &str) {
        if self.blank_line {
            self.out.truncate(self.line_start);
        }
        self.swallow_space = false;
        if !markdown.trim().is_empty() {
            self.blank_line = false;
        }
        self.out.push_str(markdown);
    }

    /// Starts a new paragraph, unless one has just been started.
    fn paragraph(&mut self) {
        if self.blank_line {
            self.out.truncate(self.line_start);
        }
        let ending = self.out.trim_end_matches([' ', '\t']);
        let needed = if ending.is_empty() || ending.ends_with("\n\n") {
            0
        } else if ending.ends_with('\n') {
            1
        } else {
            2
        };
        self.out.truncate(ending.len());
        for _ in 0..needed {
            self.out.push('\n');
        }
        self.line_start = self.out.len();
        self.blank_line = true;
        self.swallow_space = true;
    }

    /// Handles one tag. `after` is the text following it. Returns how much of
    /// `after` was used up as well, or `None` to leave the tag as it is.
    fn tag(&mut self, tag: &Tag<'_>, after: &str) -> Option<usize> {
        let closing = tag.closing;
        match tag.name.as_str() {
            "img" => {
                if let Some(src) = tag.get("src").map(str::trim).filter(|s| !s.is_empty()) {
                    let alt = tag.get("alt").unwrap_or("").replace(['[', ']', '\n'], " ");
                    let address = images::sized(src, tag.length("width"), tag.length("height"));
                    self.emit(&format!("![{alt}](<{address}>)"));
                }
            }
            "br" => {
                if self.table_row {
                    self.emit(" ");
                } else {
                    self.emit("  \n");
                    self.line_start = self.out.len();
                    self.blank_line = true;
                    self.swallow_space = true;
                }
            }
            "hr" => {
                self.paragraph();
                self.out.push_str("---");
                self.blank_line = false;
                self.paragraph();
            }
            "a" => {
                if closing {
                    match self.links.pop() {
                        Some(Some(href)) => self.emit(&format!("](<{href}>)")),
                        Some(None) => {}
                        None => return None,
                    }
                } else {
                    let href = tag
                        .get("href")
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                        .map(str::to_owned);
                    if href.is_some() {
                        self.emit("[");
                        self.swallow_space = true;
                    }
                    self.links.push(href);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.paragraph();
                if !closing {
                    let level = usize::from(tag.name.as_bytes()[1] - b'0');
                    self.out.push_str(&"#".repeat(level));
                    self.out.push(' ');
                    self.blank_line = false;
                    self.swallow_space = true;
                }
            }
            "p" | "div" | "center" | "section" | "article" | "header" | "footer" | "nav"
            | "details" | "table" | "thead" | "tbody" | "tr" | "td" | "th" | "ul" | "ol"
            | "blockquote" | "figure" | "figcaption" => {
                self.depth = if closing {
                    self.depth.saturating_sub(1)
                } else {
                    self.depth + 1
                };
                self.paragraph();
            }
            "li" => {
                self.paragraph();
                if !closing {
                    self.out.push_str("- ");
                    self.blank_line = false;
                    self.swallow_space = true;
                }
            }
            "summary" => {
                if closing {
                    self.emit("**");
                    self.paragraph();
                } else {
                    self.paragraph();
                    self.out.push_str("**");
                    self.blank_line = false;
                    self.swallow_space = true;
                }
            }
            "b" | "strong" => self.emit("**"),
            "i" | "em" => self.emit("*"),
            "s" | "del" | "strike" => self.emit("~~"),
            "code" | "kbd" | "samp" | "tt" => self.emit("`"),
            "span" | "picture" | "sub" | "sup" | "u" | "small" | "big" | "font" | "ins"
            | "mark" | "abbr" | "source" => self.emit(""),
            "pre" if !closing => {
                // Preformatted text becomes a fenced block, contents untouched.
                let close = after.to_ascii_lowercase().find("</pre>");
                let inner = &after[..close.unwrap_or(after.len())];
                let mut code = String::new();
                let mut rest = inner;
                while let Some(open) = rest.find('<') {
                    code.push_str(&rest[..open]);
                    match read_tag(&rest[open..]) {
                        Some((_, length)) => rest = &rest[open + length..],
                        None => {
                            code.push('<');
                            rest = &rest[open + 1..];
                        }
                    }
                }
                code.push_str(rest);
                let code = code
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&amp;", "&");
                self.paragraph();
                self.out.push_str("````\n");
                self.out.push_str(code.trim_matches('\n'));
                self.out.push_str("\n````");
                self.blank_line = false;
                self.paragraph();
                return Some(close.map_or(after.len(), |c| c + "</pre>".len()));
            }
            "video" if !closing => {
                // Nothing here can play one, but it can be opened.
                if let Some(src) = tag.get("src").map(str::trim).filter(|s| !s.is_empty()) {
                    self.emit(&format!("[Video](<{src}>)"));
                }
            }
            "video" => self.emit(""),
            _ => return None,
        }
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_plain_markdown_alone() {
        let text = "# Title\n\nSome *text* and `code`.\n\n    indented code\n";
        assert!(matches!(prepare(text), Cow::Borrowed(_)));
        // Comparisons, generics and autolinks are not tags.
        for text in [
            "a < b and b > c\n",
            "Returns a Vec<String> or an Option<T>.\n",
            "See <https://example.com> or mail <me@example.com>.\n",
        ] {
            assert_eq!(prepare(text), text);
        }
    }

    #[test]
    fn turns_a_centered_header_into_markdown() {
        let text = "<p align=\"center\">\n  <img src=\"docs/logo.png\" alt=\"Logo\" width=\"120\" />\n</p>\n\n<h1 align=\"center\">\n    Feather\n</h1>\n\n<p align=\"center\">\n    <a href=\"https://example.com/ci\"><img src='https://img.shields.io/badge/ci-ok-green' alt=\"CI\"></a>\n    <b>Fast</b> and <i>small</i>.<br>\n    Really.\n</p>\n\nBody text.\n";
        let expected = "![Logo](<docs/logo.png#feather:w=120>)\n\n# Feather\n\n[![CI](<https://img.shields.io/badge/ci-ok-green>)](<https://example.com/ci>)\n**Fast** and *small*.  \nReally.\n\nBody text.\n";
        assert_eq!(prepare(text), expected);
    }

    #[test]
    fn reads_attributes_however_they_are_written() {
        let (tag, length) =
            read_tag("<IMG  SRC = 'a b.png' width=50% height=\"20px\" alt=\"x > y\" data-x/> tail")
                .unwrap();
        assert_eq!(
            length,
            "<IMG  SRC = 'a b.png' width=50% height=\"20px\" alt=\"x > y\" data-x/>".len()
        );
        assert_eq!(tag.name, "img");
        assert_eq!(tag.get("src"), Some("a b.png"));
        assert_eq!(tag.get("alt"), Some("x > y"));
        assert_eq!(tag.length("width"), Some(390));
        assert_eq!(tag.length("height"), Some(20));
        assert_eq!(tag.get("title"), None);
        // A tag split over several lines.
        let text = "<img\n  src=\"shot.png\"\n  width=\"600\"\n/>\n";
        assert_eq!(prepare(text), "![](<shot.png#feather:w=600>)\n");
    }

    #[test]
    fn does_not_touch_code() {
        let text = "Use `<img src=\"x\">` like so:\n\n```html\n<p align=\"center\">\n  <img src=\"x.png\">\n</p>\n```\n\n~~~\n<br>\n~~~\n\n<br>\n";
        let out = prepare(text);
        assert!(out.contains("`<img src=\"x\">`"));
        assert!(out.contains("```html\n<p align=\"center\">\n  <img src=\"x.png\">\n</p>\n```"));
        assert!(out.contains("~~~\n<br>\n~~~"));
        assert!(!out.trim_end().ends_with("<br>"));
    }

    #[test]
    fn handles_the_other_common_tags() {
        assert_eq!(
            prepare("<details>\n<summary>More</summary>\n\nHidden text.\n\n</details>\n"),
            "**More**\n\nHidden text.\n\n"
        );
        assert_eq!(
            prepare("Text <!-- a\nnote --> here.\n<kbd>Ctrl</kbd>+<kbd>S</kbd>\n"),
            "Text  here.\n`Ctrl`+`S`\n"
        );
        assert_eq!(
            prepare("| A | B |\n|---|---|\n| one<br>two | 3 |\n"),
            "| A | B |\n|---|---|\n| one two | 3 |\n"
        );
        assert_eq!(
            prepare("<ul>\n  <li>One</li>\n  <li>Two</li>\n</ul>\n"),
            "- One\n\n- Two\n\n"
        );
        assert_eq!(
            prepare("<pre><code>let a = 1 &lt; 2;\n  indented</code></pre>\nAfter\n"),
            "````\nlet a = 1 < 2;\n  indented\n````\n\nAfter\n"
        );
        // Unknown tags stay as they were.
        assert_eq!(
            prepare("<custom-thing>x</custom-thing>\n"),
            "<custom-thing>x</custom-thing>\n"
        );
        assert_eq!(
            prepare("<svg width=\"1\"></svg>\n"),
            "<svg width=\"1\"></svg>\n"
        );
    }
}
