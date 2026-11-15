//! The main panel: a header with the current path, then a file viewer, a
//! rendered preview, or a diff.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Label, Layout, Rect, ScrollArea, Sense,
    Stroke, TextFormat, Ui, UiBuilder, pos2, text::LayoutJob, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::virtual_rows;
use super::widgets::{ButtonKind, button, empty_state, paint_galley, segmented};
use crate::app::{Cmd, DiffView, FileView, Repo, View};
use crate::diff::{Diff, DiffLine, LineKind};
use crate::document::{self, Body, LoadedFile, Preview, Span, TextDoc};
use crate::git::ChangeKind;
use crate::theme::{self, CODE_ROW_HEIGHT, HEADER_HEIGHT, icon};
use crate::worker::Action;

const CODE_FG: Color32 = Color32::from_rgb(0xd6, 0xd8, 0xdc);

pub fn show(ui: &mut Ui, repo: &mut Repo, markdown: &mut CommonMarkCache, cmds: &mut Vec<Cmd>) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let idle = repo.busy.is_none();
    match &mut repo.view {
        View::Empty => {
            empty_state(
                ui,
                icon::FEATHER,
                "No file selected",
                Some("Pick a file from the sidebar to view it."),
            );
        }
        View::File(view) => {
            file_header(ui, view);
            file_body(ui, view, markdown);
        }
        View::Diff(view) => {
            diff_header(ui, view, idle, cmds);
            diff_body(ui, view);
        }
    }
}

// ── Header ─────────────────────────────────────────────────────────────────

/// Reserves the header strip and draws its bottom border.
fn header_rect(ui: &mut Ui) -> Rect {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::BORDER),
    );
    rect
}

/// Lays out right-aligned header controls; returns the x where they begin.
fn header_controls(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) -> f32 {
    let area = rect.shrink2(vec2(12.0, 0.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::right_to_left(Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    add(&mut child);
    child.min_rect().left().min(area.right())
}

/// `src / ui / ` in muted text followed by the file name.
fn breadcrumb(ui: &Ui, rel: &str, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let (dir, name) = match rel.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", rel),
    };
    let mut job = LayoutJob::default();
    if !dir.is_empty() {
        let dir = dir.replace('/', "  /  ");
        job.append(
            &format!("{dir}  /  "),
            0.0,
            TextFormat::simple(theme::ui_font(13.0), theme::TEXT_MUTED),
        );
    }
    job.append(
        name,
        0.0,
        TextFormat::simple(theme::medium_font(13.0), theme::TEXT),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

fn file_header(ui: &mut Ui, view: &mut FileView) {
    let rect = header_rect(ui);
    let kind = document::preview_kind(&view.rel);
    let size = view.file.as_ref().map(|f| f.size);
    let preview = &mut view.preview;
    let id = ui.scope_id().with("preview_toggle");

    let controls_left = header_controls(ui, rect, |ui| {
        if kind != Preview::None {
            let selected = usize::from(!*preview);
            if let Some(choice) = segmented(ui, id, &["Preview", "Source"], selected) {
                *preview = choice == 0;
            }
        }
        if let Some(size) = size {
            ui.label(
                egui::RichText::new(document::format_size(size))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
        }
    });
    let x = rect.left() + 16.0;
    let galley = breadcrumb(ui, &view.rel, controls_left - 16.0 - x);
    paint_galley(ui, x, rect.center().y, galley);
}

fn diff_header(ui: &mut Ui, view: &DiffView, idle: bool, cmds: &mut Vec<Cmd>) {
    let rect = header_rect(ui);
    let change = &view.change;
    let stats = match &view.diff {
        Some(Ok(diff)) if !diff.binary => Some((diff.additions, diff.deletions)),
        _ => None,
    };

    let controls_left = header_controls(ui, rect, |ui| {
        if view.staged {
            if button(ui, ButtonKind::Secondary, "Unstage", None, idle).clicked() {
                let mut paths = vec![change.path.clone()];
                paths.extend(change.orig_path.clone());
                cmds.push(Cmd::Act(Action::Unstage(paths)));
            }
        } else if button(ui, ButtonKind::Secondary, "Stage", None, idle).clicked() {
            cmds.push(Cmd::Act(Action::Stage(vec![change.path.clone()])));
        }
        if change.kind != ChangeKind::Deleted
            && button(ui, ButtonKind::Secondary, "Open file", None, true).clicked()
        {
            cmds.push(Cmd::OpenFile(change.path.clone()));
        }
        if let Some((add, del)) = stats {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("−{del}"))
                    .font(theme::medium_font(12.0))
                    .color(theme::RED),
            );
            ui.label(
                egui::RichText::new(format!("+{add}"))
                    .font(theme::medium_font(12.0))
                    .color(theme::GREEN),
            );
        }
    });

    let mid = rect.center().y;
    let mut x = rect.left() + 16.0;
    // Status pill.
    let label = match (view.staged, change.kind) {
        (true, _) => "Staged",
        (false, ChangeKind::Untracked) => "Untracked",
        (false, ChangeKind::Conflicted) => "Conflict",
        (false, _) => "Unstaged",
    };
    let pill_text = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::medium_font(11.5),
        theme::TEXT_SECONDARY,
    );
    let pill_w = pill_text.size().x + 16.0;
    let max_path = controls_left - 16.0 - x - pill_w - 10.0;
    let galley = breadcrumb(ui, &change.path, max_path);
    x += paint_galley(ui, x, mid, galley) + 10.0;
    if x + pill_w < controls_left {
        let pill = Rect::from_min_size(pos2(x, mid - 10.0), vec2(pill_w, 20.0));
        ui.painter().rect_stroke(
            pill,
            CornerRadius::same(10),
            Stroke::new(1.0, theme::BORDER_STRONG),
            egui::StrokeKind::Inside,
        );
        paint_galley(ui, pill.left() + 8.0, mid, pill_text);
    }
}

// ── File body ──────────────────────────────────────────────────────────────

fn file_body(ui: &mut Ui, view: &FileView, markdown: &mut CommonMarkCache) {
    let Some(file) = &view.file else { return };
    let size = document::format_size(file.size);
    match &file.body {
        Body::Text(doc) => match (view.preview, document::preview_kind(&view.rel)) {
            (true, Preview::Markdown) => markdown_view(ui, view, file, doc, markdown),
            (true, Preview::Svg) => image_view(ui, &view.rel, &file.uri()),
            _ if doc.lines.is_empty() => {
                empty_state(ui, icon::FILE, "Empty file", None);
            }
            _ => code_view(ui, &view.rel, doc),
        },
        Body::Image => image_view(ui, &view.rel, &file.uri()),
        Body::Binary => empty_state(
            ui,
            icon::FILE,
            "Binary file",
            Some(&format!("{size} — no preview available.")),
        ),
        Body::TooLarge => empty_state(
            ui,
            icon::FILE,
            "File too large to preview",
            Some(&format!("{size}.")),
        ),
        Body::Error(error) => empty_state(ui, icon::WARNING, "Couldn't open file", Some(error)),
    }
}

fn char_width(ui: &Ui, font: &FontId) -> f32 {
    ui.ctx().fonts_mut(|f| f.glyph_width(font, '0'))
}

/// Builds the colored text for one line of source.
fn line_job(line: &str, spans: Option<&[Span]>, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut start = 0;
    for span in spans.unwrap_or_default() {
        let end = (span.end as usize).min(line.len());
        if end > start && line.is_char_boundary(end) {
            job.append(
                &line[start..end],
                0.0,
                TextFormat::simple(font.clone(), span.color),
            );
            start = end;
        }
    }
    if start < line.len() {
        job.append(
            &line[start..],
            0.0,
            TextFormat::simple(font.clone(), CODE_FG),
        );
    }
    job
}

/// Adds a selectable, non-wrapping line of code at `rect`.
fn code_label(ui: &mut Ui, rect: Rect, job: LayoutJob) {
    if job.text.is_empty() {
        return;
    }
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.add(Label::new(job).selectable(true).extend());
}

fn code_view(ui: &mut Ui, rel: &str, doc: &TextDoc) {
    let font = theme::code_font();
    let char_w = char_width(ui, &font);
    let digits = doc.lines.len().to_string().len().max(2) as f32;
    let gutter = 16.0 + digits * char_w + 16.0;
    let content_width = gutter + doc.max_chars as f32 * char_w + 32.0;
    let highlights = doc.highlights.as_deref();

    virtual_rows(
        ui,
        Id::unique(("code", rel)),
        true,
        CODE_ROW_HEIGHT,
        doc.lines.len(),
        content_width,
        10.0,
        |ui, i, rect| {
            let spans = highlights.and_then(|h| h.get(i)).map(Vec::as_slice);
            let text_rect = Rect::from_min_max(pos2(rect.left() + gutter, rect.top()), rect.max);
            code_label(ui, text_rect, line_job(&doc.lines[i], spans, &font));

            // The gutter stays put while the code scrolls sideways beneath it.
            let left = ui.clip_rect().left();
            let gutter_rect =
                Rect::from_min_size(pos2(left, rect.top()), vec2(gutter - 6.0, rect.height()));
            ui.painter()
                .rect_filled(gutter_rect, CornerRadius::ZERO, theme::BG_PANEL);
            ui.painter().text(
                pos2(left + gutter - 16.0, rect.center().y),
                Align2::RIGHT_CENTER,
                i + 1,
                font.clone(),
                theme::TEXT_FAINT,
            );
        },
    );
}

fn image_view(ui: &mut Ui, rel: &str, uri: &str) {
    ScrollArea::both()
        .id_salt(("image", rel))
        .auto_shrink(false)
        .show(ui, |ui| {
            let available = ui.available_size();
            let max = (available - vec2(64.0, 64.0)).max(vec2(32.0, 32.0));
            ui.allocate_ui_with_layout(
                available,
                Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.add(
                        egui::Image::new(uri.to_owned())
                            .fit_to_original_size(1.0)
                            .max_size(max)
                            .corner_radius(CornerRadius::same(4)),
                    );
                },
            );
        });
}

fn markdown_view(
    ui: &mut Ui,
    view: &FileView,
    file: &LoadedFile,
    doc: &TextDoc,
    cache: &mut CommonMarkCache,
) {
    const COLUMN: f32 = 780.0;
    const MARGIN: f32 = 36.0;
    // Relative image links resolve against the document's own folder.
    let base = file
        .path
        .parent()
        .map(|dir| format!("file://{}/", dir.display()))
        .unwrap_or_else(|| "file://".to_owned());

    ScrollArea::vertical()
        .id_salt(("markdown", &view.rel))
        .auto_shrink(false)
        .show(ui, |ui| {
            let available = ui.available_width();
            let column = (available - MARGIN * 2.0).clamp(120.0, COLUMN);
            let side = ((available - column) / 2.0).max(0.0);
            ui.add_space(28.0);
            ui.horizontal_top(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(column);
                    let style = ui.style_mut();
                    style.interaction.selectable_labels = true;
                    style.spacing.item_spacing = vec2(6.0, 8.0);
                    let widgets = &mut style.visuals.widgets;
                    widgets.noninteractive.fg_stroke.color = Color32::from_rgb(0xd0, 0xd2, 0xd6);
                    widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::BORDER_STRONG);
                    // The copy button on code blocks: icon only, no outline.
                    widgets.inactive.bg_stroke = Stroke::NONE;
                    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                    widgets.inactive.fg_stroke.color = theme::TEXT_MUTED;
                    // Rows no taller than the text, so list bullets sit on the baseline.
                    style.spacing.interact_size.y = 18.0;
                    style
                        .text_styles
                        .insert(egui::TextStyle::Body, theme::ui_font(14.5));
                    style.text_styles.insert(
                        egui::TextStyle::Heading,
                        FontId::new(27.0, theme::semibold()),
                    );
                    style.text_styles.insert(
                        egui::TextStyle::Monospace,
                        FontId::new(13.0, egui::FontFamily::Monospace),
                    );
                    CommonMarkViewer::new()
                        .default_implicit_uri_scheme(base)
                        .max_image_width(Some(column as usize))
                        .show(ui, cache, &doc.raw);
                });
            });
            ui.add_space(56.0);
        });
}

// ── Diff body ──────────────────────────────────────────────────────────────

fn diff_body(ui: &mut Ui, view: &DiffView) {
    match &view.diff {
        None => {}
        Some(Err(error)) => empty_state(ui, icon::WARNING, "Couldn't load diff", Some(error)),
        Some(Ok(diff)) if diff.binary => empty_state(
            ui,
            icon::FILE,
            "Binary file",
            Some("This file changed, but its contents can't be shown as text."),
        ),
        Some(Ok(diff)) if diff.is_empty() => empty_state(
            ui,
            icon::CHECK,
            "No content changes",
            Some("Only the file's mode or name changed."),
        ),
        Some(Ok(diff)) => diff_view(ui, &view.change.path, view.staged, diff),
    }
}

fn diff_view(ui: &mut Ui, path: &str, staged: bool, diff: &Diff) {
    let font = theme::code_font();
    let char_w = char_width(ui, &font);
    let last_no = diff
        .lines
        .iter()
        .rev()
        .find_map(|l| l.new_no.or(l.old_no))
        .unwrap_or(1);
    let digits = last_no.to_string().len().max(2) as f32;
    let number_col = digits * char_w + 14.0;
    let gutter = 10.0 + number_col * 2.0 + char_w + 16.0;
    let content_width = gutter + diff.max_chars as f32 * char_w + 32.0;

    virtual_rows(
        ui,
        Id::unique(("diff", path, staged)),
        true,
        CODE_ROW_HEIGHT,
        diff.lines.len(),
        content_width,
        6.0,
        |ui, i, rect| diff_row(ui, rect, &diff.lines[i], &font, char_w, number_col, gutter),
    );
}

fn diff_row(
    ui: &mut Ui,
    rect: Rect,
    line: &DiffLine,
    font: &FontId,
    char_w: f32,
    number_col: f32,
    gutter: f32,
) {
    let (bg, sign, sign_color, text_color) = match line.kind {
        LineKind::Add => (Some(theme::diff_add_bg()), "+", theme::GREEN, CODE_FG),
        LineKind::Del => (Some(theme::diff_del_bg()), "−", theme::RED, CODE_FG),
        LineKind::Hunk => (
            Some(Color32::from_white_alpha(7)),
            "",
            theme::TEXT_MUTED,
            theme::TEXT_MUTED,
        ),
        LineKind::Note => (None, "", theme::TEXT_MUTED, theme::TEXT_MUTED),
        LineKind::Context => (
            None,
            "",
            theme::TEXT_MUTED,
            Color32::from_rgb(0xa8, 0xab, 0xb2),
        ),
    };
    let visible = Rect::from_x_y_ranges(ui.clip_rect().x_range(), rect.y_range());
    if let Some(bg) = bg {
        ui.painter().rect_filled(visible, CornerRadius::ZERO, bg);
    }

    let text_rect = Rect::from_min_max(pos2(rect.left() + gutter, rect.top()), rect.max);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(
        &line.text,
        0.0,
        TextFormat::simple(font.clone(), text_color),
    );
    code_label(ui, text_rect, job);

    // Pinned gutter: old number, new number, +/- sign.
    let left = visible.left();
    let gutter_rect =
        Rect::from_min_size(pos2(left, rect.top()), vec2(gutter - 4.0, rect.height()));
    let painter = ui.painter();
    painter.rect_filled(gutter_rect, CornerRadius::ZERO, theme::BG_PANEL);
    if let Some(bg) = bg {
        painter.rect_filled(gutter_rect, CornerRadius::ZERO, bg);
    }
    let mid = rect.center().y;
    let number_color = match line.kind {
        LineKind::Add => theme::GREEN.gamma_multiply(0.75),
        LineKind::Del => theme::RED.gamma_multiply(0.75),
        _ => theme::TEXT_FAINT,
    };
    let mut x = left + 10.0 + number_col;
    for number in [line.old_no, line.new_no] {
        if let Some(number) = number {
            painter.text(
                pos2(x - 8.0, mid),
                Align2::RIGHT_CENTER,
                number,
                font.clone(),
                number_color,
            );
        }
        x += number_col;
    }
    if !sign.is_empty() {
        painter.text(
            pos2(left + 10.0 + number_col * 2.0 + char_w / 2.0, mid),
            Align2::CENTER_CENTER,
            sign,
            font.clone(),
            sign_color,
        );
    }
}
