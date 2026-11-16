//! Finding things: the Search tab (text across the whole repository) and
//! quick open (a file by part of its name).

use eframe::egui::{
    self, Align2, Area, CornerRadius, CursorIcon, Frame, Id, Key, Margin, Modal, Sense, Stroke,
    TextEdit, TextFormat, Ui, pos2,
    text::{LayoutJob, TextWrapping},
    vec2,
};

use super::sidebar::file_icon;
use super::virtual_rows;
use super::widgets::{elided, empty_state, paint_galley, paint_icon, paint_row_bg, popup_frame};
use crate::app::{Cmd, Jump, Repo};
use crate::git::{SearchMatch, SearchResults};
use crate::theme::{self, ROW_HEIGHT, icon};

const PAD: f32 = 8.0;
/// How many quick-open matches to list.
const QUICK_OPEN_ROWS: usize = 60;

// ── Search tab ─────────────────────────────────────────────────────────────

/// A small on/off button for a search option.
fn option_chip(ui: &mut Ui, label: &str, on: &mut bool, tooltip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 22.0), Sense::click());
    let response = response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    let fill = if *on {
        theme::accent().gamma_multiply(0.35)
    } else if response.hovered() {
        theme::overlay(18)
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme::radius().min(5)), fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::medium_font(11.5),
        if *on {
            theme::text()
        } else {
            theme::text_secondary()
        },
    );
    if response.clicked() {
        *on = !*on;
        return true;
    }
    false
}

pub fn show(ui: &mut Ui, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let now = ui.input(|i| i.time);
    let search = &mut repo.search;
    let mut changed = false;
    let mut submit = false;

    Frame::new()
        .inner_margin(Margin {
            left: PAD as i8,
            right: PAD as i8,
            top: 2,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
            let id = Id::unique("search_field");
            let focused = ui.memory(|m| m.has_focus(id));
            Frame::new()
                .fill(theme::bg_input())
                .stroke(Stroke::new(
                    1.0,
                    if focused {
                        theme::accent().gamma_multiply(0.8)
                    } else {
                        theme::border_strong()
                    },
                ))
                .corner_radius(CornerRadius::same(theme::radius()))
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    let response = ui.add(
                        TextEdit::singleline(&mut search.query)
                            .id(id)
                            .hint_text("Search in files")
                            .desired_width(f32::INFINITY)
                            .frame(Frame::new()),
                    );
                    if std::mem::take(&mut search.focus) {
                        response.request_focus();
                    }
                    changed |= response.changed();
                    submit |= response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                });
            ui.horizontal(|ui| {
                let options = &mut search.options;
                changed |= option_chip(ui, "Aa", &mut options.match_case, "Match case");
                changed |= option_chip(ui, "ab", &mut options.whole_word, "Match whole words");
                changed |= option_chip(ui, ".*", &mut options.regex, "Use a regular expression");
                let summary = match &search.results {
                    _ if search.searching => "Searching…".to_owned(),
                    Some(Ok(results)) if results.total > 0 => format!(
                        "{}{} in {} {}",
                        results.total,
                        if results.truncated { "+" } else { "" },
                        results.files.len(),
                        if results.files.len() == 1 {
                            "file"
                        } else {
                            "files"
                        }
                    ),
                    _ => String::new(),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(summary)
                            .size(11.5)
                            .color(theme::text_muted()),
                    );
                });
            });
        });

    if submit {
        search.changed_at = Some(now - 1.0);
    } else if changed {
        search.changed_at = Some(now);
    }

    match &search.results {
        None if search.query.trim().is_empty() => empty_state(
            ui,
            icon::SEARCH,
            "Search the repository",
            Some("Finds text in every file that isn't ignored."),
        ),
        None => {}
        Some(Err(error)) => empty_state(ui, icon::WARNING, "That search didn't work", Some(error)),
        Some(Ok(results)) if results.total == 0 => {
            if !search.searching {
                empty_state(ui, icon::SEARCH, "No results", None);
            }
        }
        Some(Ok(results)) => results_list(ui, results, &search.query, search.options.regex, cmds),
    }
}

#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Match(usize, usize),
}

fn results_list(
    ui: &mut Ui,
    results: &SearchResults,
    query: &str,
    regex: bool,
    cmds: &mut Vec<Cmd>,
) {
    let mut rows = Vec::with_capacity(results.total + results.files.len());
    for (f, file) in results.files.iter().enumerate() {
        rows.push(Row::File(f));
        rows.extend((0..file.matches.len()).map(|m| Row::Match(f, m)));
    }
    // A regular expression can match any length, so only plain text is marked.
    let mark = if regex { 0 } else { query.len() };

    virtual_rows(
        ui,
        Id::unique("search_results"),
        false,
        ROW_HEIGHT - 2.0,
        rows.len(),
        0.0,
        2.0,
        |ui, i, rect| {
            let rect = rect.shrink2(vec2(PAD, 0.5));
            let response = ui
                .interact(rect, ui.scope_id().with(("result", i)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            paint_row_bg(ui, rect, response.hovered(), false);
            let mid = rect.center().y;
            match rows[i] {
                Row::File(f) => {
                    let file = &results.files[f];
                    let (dir, name) = match file.path.rsplit_once('/') {
                        Some((dir, name)) => (dir, name),
                        None => ("", file.path.as_str()),
                    };
                    paint_icon(
                        ui,
                        pos2(rect.left() + 14.0, mid),
                        file_icon(name),
                        15.0,
                        theme::text_muted(),
                    );
                    let count = ui.painter().layout_no_wrap(
                        file.matches.len().to_string(),
                        theme::ui_font(11.0),
                        theme::text_muted(),
                    );
                    let right = rect.right() - 8.0 - count.size().x;
                    paint_galley(ui, right, mid, count);
                    let x = rect.left() + 28.0;
                    let name = elided(
                        ui,
                        name,
                        theme::medium_font(12.5),
                        theme::text(),
                        right - 8.0 - x,
                    );
                    let w = paint_galley(ui, x, mid, name);
                    let dir_x = x + w + 8.0;
                    if !dir.is_empty() && right - 8.0 - dir_x > 24.0 {
                        let dir = elided(
                            ui,
                            dir,
                            theme::ui_font(11.5),
                            theme::text_muted(),
                            right - 8.0 - dir_x,
                        );
                        paint_galley(ui, dir_x, mid + 0.5, dir);
                    }
                    if response.clicked() {
                        cmds.push(Cmd::OpenFile(file.path.clone()));
                    }
                }
                Row::Match(f, m) => {
                    let file = &results.files[f];
                    let found = &file.matches[m];
                    let x = rect.left() + 28.0;
                    let job = snippet(found, mark, rect.right() - 8.0 - x);
                    let galley = ui.painter().layout_job(job);
                    paint_galley(ui, x, mid, galley);
                    if response.clicked() {
                        cmds.push(Cmd::OpenAt {
                            rel: file.path.clone(),
                            jump: Jump::Line {
                                line: found.line as usize,
                                column: found.column as usize,
                                len: mark,
                            },
                        });
                    }
                }
            }
        },
    );
}

/// One matching line, trimmed so the match is in view, with the match marked.
fn snippet(found: &SearchMatch, mark: usize, max_width: f32) -> LayoutJob {
    let text = found.text.as_str();
    let lead = text.len() - text.trim_start().len();
    let start = (found.column as usize)
        .saturating_sub(1)
        .clamp(lead, text.len());
    // Keep a little context before the match rather than a long run-up to it.
    let mut from = lead;
    if start - lead > 24 {
        from = start - 16;
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
    let mut end = (start + mark).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let start = start.min(end);

    let plain = TextFormat::simple(theme::ui_font(12.5), theme::text_secondary());
    let marked = TextFormat {
        background: theme::accent().gamma_multiply(0.35),
        ..TextFormat::simple(theme::ui_font(12.5), theme::text())
    };
    let mut job = LayoutJob::default();
    if from > lead {
        job.append("…", 0.0, plain.clone());
    }
    if text.is_char_boundary(start) {
        job.append(&text[from..start], 0.0, plain.clone());
        job.append(&text[start..end], 0.0, marked);
        job.append(&text[end..], 0.0, plain);
    } else {
        job.append(&text[from..], 0.0, plain);
    }
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

// ── Quick open ─────────────────────────────────────────────────────────────

/// How well `path` matches what was typed; `None` if it does not at all.
///
/// The typed letters have to appear in order. Matches in the file name beat
/// matches in the folders, and runs of adjacent letters and letters at the
/// start of a word score higher.
pub fn fuzzy_score(path: &str, query: &str) -> Option<i32> {
    let query: Vec<u8> = query
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let bytes = path.as_bytes();
    let name_start = path.rfind('/').map_or(0, |i| i + 1);

    let subsequence = |from: usize| -> Option<i32> {
        let mut score = 0;
        let mut next = 0;
        let mut previous: Option<usize> = None;
        for (i, byte) in bytes.iter().enumerate().skip(from) {
            if next < query.len() && byte.to_ascii_lowercase() == query[next] {
                score += 1;
                if previous == Some(i.wrapping_sub(1)) {
                    score += 6;
                }
                let at_word_start =
                    i == from || matches!(bytes[i - 1], b'/' | b'_' | b'-' | b'.' | b' ');
                if at_word_start {
                    score += 8;
                }
                previous = Some(i);
                next += 1;
            }
        }
        (next == query.len()).then_some(score)
    };

    let in_name = subsequence(name_start).map(|s| s + 100);
    let score = in_name.or_else(|| subsequence(0))?;
    // Among equals, the shorter path is the likelier one.
    Some(score * 4 - (bytes.len() as i32).min(200) / 8)
}

fn rank(files: &[String], query: &str) -> Vec<String> {
    let mut scored: Vec<(i32, &String)> = files
        .iter()
        .filter_map(|path| fuzzy_score(path, query).map(|score| (score, path)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    });
    scored
        .into_iter()
        .take(QUICK_OPEN_ROWS)
        .map(|(_, path)| path.clone())
        .collect()
}

/// The quick-open box near the top of the window.
pub fn quick_open(ctx: &egui::Context, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let Repo {
        quick_open: slot,
        files,
        ..
    } = repo;
    let Some(state) = slot else { return };
    let files: &[String] = files.as_deref().unwrap_or_default();

    // Re-rank only when the query or the file list changed.
    let wanted = (state.query.clone(), files.len());
    if state.ranked_for.as_ref() != Some(&wanted) {
        state.results = rank(files, &state.query);
        state.ranked_for = Some(wanted);
        state.selected = 0;
    }

    // Arrow keys move through the list while the field keeps the focus.
    let (down, up, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, Key::Enter),
        )
    });
    let count = state.results.len();
    if count > 0 {
        if down {
            state.selected = (state.selected + 1) % count;
        }
        if up {
            state.selected = (state.selected + count - 1) % count;
        }
    }
    let mut chosen: Option<String> = enter
        .then(|| state.results.get(state.selected).cloned())
        .flatten();
    let moved = down || up;

    let id = Id::unique("quick_open");
    let area: Area = Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 64.0));
    let modal = Modal::new(id)
        .area(area)
        .frame(popup_frame().inner_margin(Margin::same(6)))
        .backdrop_color(egui::Color32::from_black_alpha(90))
        .show(ctx, |ui| {
            ui.set_width(560.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            Frame::new()
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    let field = ui.add(
                        TextEdit::singleline(&mut state.query)
                            .hint_text("Go to file…")
                            .desired_width(f32::INFINITY)
                            .frame(Frame::new()),
                    );
                    field.request_focus();
                });
            let line = ui.available_rect_before_wrap();
            ui.painter().hline(
                line.x_range(),
                line.top(),
                Stroke::new(1.0, theme::border_strong()),
            );
            ui.add_space(4.0);

            if state.results.is_empty() {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
                ui.painter().text(
                    pos2(rect.left() + 10.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    if files.is_empty() {
                        "Listing files…"
                    } else {
                        "No matching files"
                    },
                    theme::ui_font(13.0),
                    theme::text_muted(),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(30.0 * 12.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, path) in state.results.iter().enumerate() {
                        let (rect, response) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                        let response = response.on_hover_cursor(CursorIcon::PointingHand);
                        let selected = i == state.selected;
                        paint_row_bg(ui, rect, response.hovered(), selected);
                        if selected && moved {
                            ui.scroll_to_rect(rect, None);
                        }
                        let (dir, name) = match path.rsplit_once('/') {
                            Some((dir, name)) => (dir, name),
                            None => ("", path.as_str()),
                        };
                        let mid = rect.center().y;
                        paint_icon(
                            ui,
                            pos2(rect.left() + 16.0, mid),
                            file_icon(name),
                            15.0,
                            theme::text_muted(),
                        );
                        let x = rect.left() + 32.0;
                        let right = rect.right() - 10.0;
                        let name = elided(ui, name, theme::ui_font(13.0), theme::text(), right - x);
                        let w = paint_galley(ui, x, mid, name);
                        if !dir.is_empty() && right - (x + w + 10.0) > 30.0 {
                            let dir = elided(
                                ui,
                                dir,
                                theme::ui_font(12.0),
                                theme::text_muted(),
                                right - (x + w + 10.0),
                            );
                            paint_galley(ui, x + w + 10.0, mid + 0.5, dir);
                        }
                        if response.clicked() {
                            chosen = Some(path.clone());
                        }
                    }
                });
        });

    if let Some(path) = chosen {
        cmds.push(Cmd::OpenFile(path));
        *slot = None;
    } else if modal.should_close() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_file_name_matches_first() {
        let files: Vec<String> = [
            "src/ui/sidebar.rs",
            "src/sidebar_old/readme.md",
            "docs/side-notes/bar.txt",
            "src/main.rs",
            "README.md",
        ]
        .map(str::to_owned)
        .into();
        let ranked = rank(&files, "sidebar");
        assert_eq!(ranked[0], "src/ui/sidebar.rs");
        assert_eq!(ranked.len(), 3);
        assert!(!ranked.contains(&"src/main.rs".to_owned()));

        // Letters in order, not necessarily adjacent, and case-insensitive.
        assert_eq!(rank(&files, "mnrs"), ["src/main.rs"]);
        assert_eq!(rank(&files, "READ")[0], "README.md");
        // A folder can be part of the query.
        assert_eq!(rank(&files, "ui/side")[0], "src/ui/sidebar.rs");
        // Nothing typed lists everything, shortest first.
        assert_eq!(rank(&files, "").len(), files.len());
        assert_eq!(rank(&files, "")[0], "README.md");
        assert!(rank(&files, "zzz").is_empty());
    }

    #[test]
    fn snippets_mark_the_match_and_keep_it_in_view() {
        let found = SearchMatch {
            line: 3,
            column: 9,
            text: "    let needle = 1;".to_owned(),
        };
        let job = snippet(&found, 6, 400.0);
        assert_eq!(job.text, "let needle = 1;");
        let marked: Vec<_> = job
            .sections
            .iter()
            .filter(|s| s.format.background != egui::Color32::TRANSPARENT)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(marked, ["needle"]);

        // A match far along a long line is brought into view.
        let long = SearchMatch {
            line: 1,
            column: 61,
            text: format!("{}needle after", "x".repeat(60)),
        };
        let job = snippet(&long, 6, 400.0);
        assert!(job.text.starts_with('…'));
        assert!(job.text.contains("needle after"));
        assert!(job.text.len() < 50);
    }
}
