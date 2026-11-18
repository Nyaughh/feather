//! The strip across the top of the window. Feather draws it itself so it
//! matches the rest of the app, which means it also has to do the things the
//! system normally would: move the window, resize it from the edges, and
//! provide the minimise / maximise / close buttons.
//!
//! macOS is the exception: its window keeps the native buttons, laid over
//! our title bar, and the system still handles resizing.

use eframe::egui::{
    self, CornerRadius, CursorIcon, Frame, Id, Panel, PointerButton, Pos2, Rect, ResizeDirection,
    Sense, Stroke, StrokeKind, Ui, ViewportCommand, pos2, vec2,
};

use super::widgets::{elided, icon_button, paint_galley};
use crate::app::{Cmd, Repo, View};
use crate::theme::{self, TITLEBAR_HEIGHT, icon};
use crate::update;

/// Whether the system draws the window buttons (and handles resizing).
const NATIVE_BUTTONS: bool = cfg!(target_os = "macos");
/// Room for the macOS traffic lights.
const TRAFFIC_LIGHTS: f32 = 76.0;
const CONTROL_WIDTH: f32 = 46.0;
/// How close to a window edge the pointer has to be to resize.
const EDGE: f32 = 5.0;
const CORNER: f32 = 14.0;

/// Window options for a title bar we draw ourselves.
pub fn configure(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if NATIVE_BUTTONS {
        viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
    } else {
        viewport.with_decorations(false)
    }
}

/// One-off window tweaks once it exists.
pub fn prepare_window(cc: &eframe::CreationContext<'_>) {
    // Without its frame a Windows window also loses its shadow (and, on
    // Windows 11, its rounded corners) unless we ask to keep them.
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowExtWindows;
        if let Some(window) = cc.winit_window() {
            window.set_undecorated_shadow(true);
        }
    }
    let _ = cc;
}

fn window_state(ctx: &egui::Context) -> (bool, bool) {
    ctx.input(|i| {
        let viewport = i.viewport();
        (
            viewport.maximized.unwrap_or(false),
            viewport.fullscreen.unwrap_or(false),
        )
    })
}

pub fn show(
    ui: &mut Ui,
    repo: Option<&Repo>,
    sidebar_open: bool,
    updater: &update::Updater,
    cmds: &mut Vec<Cmd>,
) {
    Panel::top("titlebar")
        .exact_size(TITLEBAR_HEIGHT)
        .frame(Frame::new().fill(theme::bg_shell()))
        .show_separator_line(false)
        .show(ui, |ui| bar(ui, repo, sidebar_open, updater, cmds));
}

fn bar(
    ui: &mut Ui,
    repo: Option<&Repo>,
    sidebar_open: bool,
    updater: &update::Updater,
    cmds: &mut Vec<Cmd>,
) {
    let ctx = ui.ctx().clone();
    let rect = ui.max_rect();
    ui.allocate_rect(rect, Sense::hover());
    let (maximized, fullscreen) = window_state(&ctx);

    // The whole bar moves the window; buttons added after this sit on top.
    let drag = ui.interact(rect, Id::unique("titlebar_drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let mid = rect.center().y;
    let button = |x: f32| Rect::from_center_size(pos2(x, mid), vec2(28.0, 26.0));

    // Left: sidebar toggle.
    let mut left = rect.left() + 10.0;
    if NATIVE_BUTTONS && !fullscreen {
        left += TRAFFIC_LIGHTS;
    }
    if repo.is_some() {
        let tip = format!(
            "{} sidebar ({})",
            if sidebar_open { "Hide" } else { "Show" },
            shortcut_text(&ctx, egui::Key::B)
        );
        if icon_button(
            ui,
            button(left + 14.0),
            Id::unique("toggle_sidebar"),
            icon::SIDEBAR,
            &tip,
        )
        .clicked()
        {
            cmds.push(Cmd::ToggleSidebar);
        }
        left += 36.0;
    }

    // Right: window buttons, then settings and the update marker.
    let mut right = rect.right();
    if !NATIVE_BUTTONS {
        right = window_controls(ui, rect, maximized, cmds);
    }
    right -= 8.0;
    let tip = format!("Settings ({})", shortcut_text(&ctx, egui::Key::Comma));
    if icon_button(
        ui,
        button(right - 14.0),
        Id::unique("open_settings"),
        icon::GEAR,
        &tip,
    )
    .clicked()
    {
        cmds.push(Cmd::ToggleSettings);
    }
    right -= 34.0;
    if repo.is_some() {
        let tip = format!("Terminal ({})", shortcut_text(&ctx, egui::Key::Backtick));
        if icon_button(
            ui,
            button(right - 14.0),
            Id::unique("toggle_terminal"),
            icon::TERMINAL,
            &tip,
        )
        .clicked()
        {
            cmds.push(Cmd::ToggleTerminal);
        }
        right -= 34.0;
    }
    if let update::State::Available(release) | update::State::Ready(release) = &updater.state {
        let tip = format!("Feather {} is available", release.version);
        let slot = button(right - 14.0);
        if icon_button(ui, slot, Id::unique("update_marker"), icon::DOWNLOAD, &tip).clicked() {
            cmds.push(Cmd::ToggleSettings);
        }
        ui.painter()
            .circle_filled(slot.right_top() + vec2(-6.0, 6.0), 3.0, theme::accent());
        right -= 34.0;
    }

    // Centre: what is open.
    if let Some(repo) = repo {
        let file_name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
        let detail = match &repo.view {
            View::File(view) => Some((file_name(&view.rel), view.is_dirty())),
            View::Diff(view) => Some((file_name(&view.change.path), false)),
            View::Commit(view) => Some((view.commit.subject.clone(), false)),
            View::Empty => None,
        };
        let mut title = repo.name.clone();
        if let Some((name, dirty)) = detail {
            title = format!("{name}{} - {title}", if dirty { " •" } else { "" });
        }
        // Centred in the window when there is room, otherwise between the buttons.
        let room = (right - left - 24.0).max(0.0);
        let galley = elided(ui, &title, theme::ui_font(12.5), theme::text_muted(), room);
        let half = galley.size().x / 2.0;
        let centre = rect.center().x.clamp(
            left + 12.0 + half,
            (right - 12.0 - half).max(left + 12.0 + half),
        );
        paint_galley(ui, centre - half, mid, galley);
    }
}

/// Paints minimise, maximise/restore and close at the right edge. Returns the
/// x where they begin.
fn window_controls(ui: &Ui, bar: Rect, maximized: bool, cmds: &mut Vec<Cmd>) -> f32 {
    #[derive(Clone, Copy, PartialEq)]
    enum Control {
        Minimise,
        Maximise,
        Close,
    }
    let ctx = ui.ctx();
    let left = bar.right() - CONTROL_WIDTH * 3.0;
    for (i, control) in [Control::Minimise, Control::Maximise, Control::Close]
        .into_iter()
        .enumerate()
    {
        let rect = Rect::from_min_size(
            pos2(left + CONTROL_WIDTH * i as f32, bar.top()),
            vec2(CONTROL_WIDTH, bar.height()),
        );
        let response = ui.interact(rect, Id::unique(("window_control", i)), Sense::click());
        let hovered = response.hovered();
        let danger = control == Control::Close && hovered;
        if danger {
            ui.painter().rect_filled(
                rect,
                CornerRadius::ZERO,
                egui::Color32::from_rgb(0xe8, 0x11, 0x23),
            );
        } else if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, theme::overlay(20));
        }
        let color = if danger {
            egui::Color32::WHITE
        } else if hovered {
            theme::text()
        } else {
            theme::text_secondary()
        };
        let stroke = Stroke::new(1.0, color);
        let c = rect.center().round() + vec2(0.5, 0.5);
        let painter = ui.painter();
        match control {
            Control::Minimise => {
                painter.hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
            }
            Control::Maximise if maximized => {
                // Two overlapping squares.
                let front = Rect::from_min_size(pos2(c.x - 5.0, c.y - 3.0), vec2(8.0, 8.0));
                let back = front.translate(vec2(2.0, -2.0));
                painter.line(
                    vec![back.left_top(), back.right_top(), back.right_bottom()],
                    stroke,
                );
                painter.rect_stroke(front, CornerRadius::ZERO, stroke, StrokeKind::Middle);
            }
            Control::Maximise => {
                let square = Rect::from_center_size(c, vec2(10.0, 10.0));
                painter.rect_stroke(square, CornerRadius::ZERO, stroke, StrokeKind::Middle);
            }
            Control::Close => {
                let d = 5.0;
                painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
                painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
            }
        }
        if response.clicked() {
            match control {
                Control::Minimise => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                Control::Maximise => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                // Goes through the app so unsaved edits get a say.
                Control::Close => cmds.push(Cmd::Quit),
            }
        }
    }
    left
}

fn shortcut_text(ctx: &egui::Context, key: egui::Key) -> String {
    ctx.format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key))
}

/// Lets the window be resized by its edges and draws its outline. Call last
/// in the frame, so the resize cursor wins over whatever is underneath.
pub fn frame(ctx: &egui::Context) {
    if NATIVE_BUTTONS {
        return;
    }
    let (maximized, fullscreen) = window_state(ctx);
    if maximized || fullscreen {
        return;
    }
    let rect = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        Id::unique("window_outline"),
    ))
    .rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, theme::border_strong()),
        StrokeKind::Inside,
    );

    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    let Some(direction) = edge_at(rect, pos) else {
        return;
    };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Which edge or corner of `rect` the point is on, if any.
fn edge_at(rect: Rect, pos: Pos2) -> Option<ResizeDirection> {
    if !rect.contains(pos) {
        return None;
    }
    let near = |distance: f32, limit: f32| distance <= limit;
    let (left, right) = (pos.x - rect.left(), rect.right() - pos.x);
    let (top, bottom) = (pos.y - rect.top(), rect.bottom() - pos.y);
    let on_edge = near(left, EDGE) || near(right, EDGE) || near(top, EDGE) || near(bottom, EDGE);
    if !on_edge {
        return None;
    }
    // Near an edge: corners take a larger bite than the edges do.
    let (w, e) = (near(left, CORNER), near(right, CORNER));
    let (n, s) = (near(top, CORNER), near(bottom, CORNER));
    Some(match (n, s, w, e) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        _ if near(top, EDGE) => ResizeDirection::North,
        _ if near(bottom, EDGE) => ResizeDirection::South,
        _ if near(left, EDGE) => ResizeDirection::West,
        _ => ResizeDirection::East,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_edges_and_corners() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let at = |x, y| edge_at(rect, pos2(x, y));
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(400.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 598.0), Some(ResizeDirection::South));
        assert_eq!(at(2.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(798.0, 300.0), Some(ResizeDirection::East));
        assert_eq!(at(3.0, 3.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(790.0, 2.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 590.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(797.0, 597.0), Some(ResizeDirection::SouthEast));
        // Inside a corner's reach but not actually on an edge.
        assert_eq!(at(10.0, 10.0), None);
        assert_eq!(at(900.0, 300.0), None);
    }
}
