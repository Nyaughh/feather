//! Small hand-painted building blocks shared by the sidebar and content area.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Galley, Id, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, Vec2, pos2, text::LayoutJob, text::TextWrapping, vec2,
};
use std::sync::Arc;

use crate::theme;

/// Lays out one line of text, ending in "…" if it does not fit `max_width`.
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// Paints `galley` vertically centered on `y`, starting at `x`. Returns its width.
pub fn paint_galley(ui: &Ui, x: f32, y: f32, galley: Arc<Galley>) -> f32 {
    let size = galley.size();
    let pos = pos2(x, y - size.y / 2.0).round();
    ui.painter().galley(pos, galley, theme::text());
    size.x
}

pub fn paint_icon(ui: &Ui, center: Pos2, icon: &str, size: f32, color: Color32) {
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        icon,
        theme::icon_font(size),
        color,
    );
}

/// Rounded hover/selection wash behind a list row.
pub fn paint_row_bg(ui: &Ui, rect: Rect, hovered: bool, selected: bool) {
    let fill = if selected {
        theme::selected_bg()
    } else if hovered {
        theme::hover_bg()
    } else {
        return;
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(theme::radius()), fill);
}

/// A square icon-only button painted into `rect`.
pub fn icon_button(ui: &Ui, rect: Rect, id: Id, icon: &str, tooltip: &str) -> Response {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let color = if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), theme::overlay(22));
        theme::text()
    } else {
        theme::text_secondary()
    };
    paint_icon(ui, rect.center(), icon, 14.0, color);
    response.on_hover_text(tooltip)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent color.
    Primary,
    /// Subtle outline.
    Secondary,
    /// Filled red, for destructive confirmation.
    Danger,
}

/// A text button. `width` of `None` sizes to the label.
pub fn button(
    ui: &mut Ui,
    kind: ButtonKind,
    label: &str,
    width: Option<f32>,
    enabled: bool,
) -> Response {
    let height = 30.0;
    let font = theme::medium_font(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let width = width.unwrap_or(galley.size().x + 24.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), sense);
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    let hovered = enabled && response.hovered();
    let (fill, stroke, text) = match kind {
        ButtonKind::Primary if !enabled => (theme::overlay(10), Stroke::NONE, theme::text_muted()),
        ButtonKind::Primary => (
            if hovered {
                theme::accent_hover()
            } else {
                theme::accent()
            },
            Stroke::NONE,
            theme::on_accent(),
        ),
        ButtonKind::Danger => (
            if hovered {
                theme::red().gamma_multiply(0.88)
            } else {
                theme::red()
            },
            Stroke::NONE,
            Color32::WHITE,
        ),
        ButtonKind::Secondary => (
            if hovered {
                theme::overlay(20)
            } else {
                theme::overlay(10)
            },
            Stroke::new(1.0, theme::border_strong()),
            if enabled {
                theme::text()
            } else {
                theme::text_muted()
            },
        ),
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(theme::radius()),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    let pos = rect.center() - galley.size() / 2.0;
    painter.galley_with_override_text_color(pos.round(), galley, text);
    response
}

/// One row in a popup menu: optional icon, label, optional muted detail.
pub fn menu_item(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    detail: Option<&str>,
    checked: bool,
) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    paint_row_bg(ui, rect, response.hovered(), false);

    let mid = rect.center().y;
    let mut x = rect.left() + 8.0;
    if let Some(icon) = icon {
        paint_icon(ui, pos2(x + 8.0, mid), icon, 14.0, theme::text_secondary());
        x += 24.0;
    }
    let mut right = rect.right() - 8.0;
    if checked {
        paint_icon(
            ui,
            pos2(right - 7.0, mid),
            theme::icon::CHECK,
            13.0,
            theme::text(),
        );
        right -= 22.0;
    }
    let label = elided(ui, label, theme::ui_font(13.0), theme::text(), right - x);
    x += paint_galley(ui, x, mid, label) + 8.0;
    if let Some(detail) = detail {
        let detail = elided(
            ui,
            detail,
            theme::ui_font(12.0),
            theme::text_muted(),
            right - x,
        );
        // Right-align the detail text.
        let w = detail.size().x;
        paint_galley(ui, right - w, mid, detail);
    }
    response
}

/// A two-option segmented control. Returns the newly chosen index, if any.
pub fn segmented(ui: &mut Ui, id: Id, labels: &[&str], selected: usize) -> Option<usize> {
    let font = theme::medium_font(12.0);
    let galleys: Vec<_> = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap((*l).to_owned(), font.clone(), Color32::PLACEHOLDER)
        })
        .collect();
    let seg_w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 20.0;
    let size = Vec2::new(seg_w * labels.len() as f32 + 4.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(7), theme::overlay(10));

    let mut chosen = None;
    for (i, galley) in galleys.into_iter().enumerate() {
        let seg = Rect::from_min_size(
            pos2(rect.left() + 2.0 + seg_w * i as f32, rect.top() + 2.0),
            vec2(seg_w, rect.height() - 4.0),
        );
        let response = ui
            .interact(seg, id.with(i), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let active = i == selected;
        if active {
            ui.painter()
                .rect_filled(seg, CornerRadius::same(5), theme::overlay(26));
        }
        let color = if active || response.hovered() {
            theme::text()
        } else {
            theme::text_secondary()
        };
        let pos = seg.center() - galley.size() / 2.0;
        ui.painter()
            .galley_with_override_text_color(pos.round(), galley, color);
        if response.clicked() && !active {
            chosen = Some(i);
        }
    }
    chosen
}

/// Centered icon + message for empty and placeholder states.
pub fn empty_state(ui: &mut Ui, icon: &str, title: &str, detail: Option<&str>) {
    let rect = ui.available_rect_before_wrap();
    let center = rect.center();
    paint_icon(
        ui,
        pos2(center.x, center.y - 26.0),
        icon,
        28.0,
        theme::text_faint(),
    );
    ui.painter().text(
        pos2(center.x, center.y + 6.0),
        Align2::CENTER_CENTER,
        title,
        theme::medium_font(13.0),
        theme::text_secondary(),
    );
    if let Some(detail) = detail {
        let galley = ui.painter().layout(
            detail.to_owned(),
            theme::ui_font(12.5),
            theme::text_muted(),
            (rect.width() - 48.0).clamp(80.0, 460.0),
        );
        let pos = pos2(center.x - galley.size().x / 2.0, center.y + 22.0);
        ui.painter()
            .galley(pos.round(), galley, theme::text_muted());
    }
}

/// Styling shared by popups and menus.
pub fn popup_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::bg_elevated())
        .stroke(Stroke::new(1.0, theme::border_strong()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(4))
        .shadow(theme::popup_shadow())
}
