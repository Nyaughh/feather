//! Everything drawn on screen. `root` lays out the window; the submodules fill
//! in the sidebar and the content area.

mod content;
mod sidebar;
mod widgets;

use eframe::egui::{
    self, Align, Align2, Area, CentralPanel, Color32, CornerRadius, Frame, Id, Layout, Margin,
    Modal, Order, Panel, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};

use crate::app::{App, Cmd, Repo, Settings, Toast};
use crate::git::ChangeKind;
use crate::theme::{self, icon};
use crate::worker::Action;
use widgets::{ButtonKind, button, elided, paint_galley, paint_icon, paint_row_bg, popup_frame};

pub fn root(app: &mut App, ui: &mut Ui, cmds: &mut Vec<Cmd>) {
    let App {
        repo,
        settings,
        markdown,
        toast,
        ..
    } = app;

    match repo {
        None => welcome(ui, settings, cmds),
        Some(repo) => {
            Panel::left("sidebar")
                .resizable(true)
                .default_size(264.0)
                .size_range(208.0..=440.0)
                .frame(Frame::new().fill(theme::BG_SHELL))
                .show_separator_line(false)
                .show(ui, |ui| sidebar::show(ui, repo, settings, cmds));

            // The content sits in a bordered, rounded panel inset from the shell.
            CentralPanel::default()
                .frame(Frame::new().fill(theme::BG_SHELL).inner_margin(Margin {
                    left: 0,
                    right: 8,
                    top: 8,
                    bottom: 8,
                }))
                .show(ui, |ui| {
                    Frame::new()
                        .fill(theme::BG_PANEL)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(CornerRadius::same(8))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            // Keep content from painting over the rounded corners.
                            ui.shrink_clip_rect(ui.max_rect().shrink(1.0));
                            content::show(ui, repo, markdown, cmds);
                        });
                });

            discard_modal(ui.ctx(), repo, cmds);
        }
    }

    toast_ui(ui.ctx(), toast);
}

/// A scroll area that only lays out the rows currently on screen.
///
/// `add_row` receives each visible row's index and its full-width rectangle.
/// `content_width` is the minimum scrollable width (ignored unless
/// `horizontal`); `pad` is blank space above the first and below the last row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn virtual_rows(
    ui: &mut Ui,
    id: Id,
    horizontal: bool,
    row_height: f32,
    count: usize,
    content_width: f32,
    pad: f32,
    mut add_row: impl FnMut(&mut Ui, usize, Rect),
) {
    ScrollArea::new([horizontal, true])
        .id_salt(id)
        .auto_shrink(false)
        .show_viewport(ui, |ui, viewport| {
            let width = if horizontal {
                content_width.max(ui.available_width())
            } else {
                ui.available_width()
            };
            let height = pad * 2.0 + row_height * count as f32;
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());

            let first = ((viewport.min.y - pad) / row_height).floor().max(0.0) as usize;
            let last = (((viewport.max.y - pad) / row_height).ceil().max(0.0) as usize).min(count);
            for i in first..last {
                let top = rect.top() + pad + row_height * i as f32;
                let row = Rect::from_min_size(pos2(rect.left(), top), vec2(width, row_height));
                add_row(ui, i, row);
            }
        });
}

// ── Welcome screen ─────────────────────────────────────────────────────────

fn welcome(ui: &mut Ui, settings: &Settings, cmds: &mut Vec<Cmd>) {
    CentralPanel::default()
        .frame(Frame::new().fill(theme::BG_SHELL))
        .show(ui, |ui| {
            let width = 360.0_f32.min(ui.available_width() - 48.0);
            let recent: Vec<_> = settings.recent.iter().take(6).collect();
            let height =
                190.0 + recent.len() as f32 * 34.0 + if recent.is_empty() { 0.0 } else { 44.0 };
            let top = ((ui.available_height() - height) / 2.0).max(24.0);
            ui.add_space(top);
            ui.vertical_centered(|ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;

                let (mark, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                ui.painter()
                    .rect_filled(mark, CornerRadius::same(11), theme::ACCENT);
                paint_icon(ui, mark.center(), icon::FEATHER, 24.0, Color32::WHITE);

                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("Feather")
                        .font(egui::FontId::new(20.0, theme::semibold()))
                        .color(theme::TEXT),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Open a repository to browse files and review changes.")
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(22.0);
                if button(ui, ButtonKind::Primary, "Open repository", None, true).clicked() {
                    cmds.push(Cmd::PickRepo);
                }
                ui.add_space(8.0);
                let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::O,
                ));
                ui.label(
                    egui::RichText::new(shortcut)
                        .size(11.5)
                        .color(theme::TEXT_MUTED),
                );

                if !recent.is_empty() {
                    ui.add_space(28.0);
                    let (label, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
                    ui.painter().text(
                        pos2(label.left() + 10.0, label.center().y),
                        Align2::LEFT_CENTER,
                        "Recent",
                        theme::medium_font(11.5),
                        theme::TEXT_MUTED,
                    );
                    for path in recent {
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(width, 34.0), Sense::click());
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        paint_row_bg(ui, rect.shrink2(vec2(0.0, 1.0)), response.hovered(), false);
                        let mid = rect.center().y;
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let name = elided(
                            ui,
                            &name,
                            theme::medium_font(13.0),
                            theme::TEXT,
                            width * 0.5,
                        );
                        let x = rect.left() + 10.0;
                        let w = paint_galley(ui, x, mid, name);
                        let parent = path
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        let parent = elided(
                            ui,
                            &parent,
                            theme::ui_font(12.0),
                            theme::TEXT_MUTED,
                            rect.right() - 10.0 - (x + w + 10.0),
                        );
                        paint_galley(ui, x + w + 10.0, mid, parent);
                        if response.clicked() {
                            cmds.push(Cmd::OpenRepo(path.clone()));
                        }
                    }
                }
            });
        });
}

// ── Overlays ───────────────────────────────────────────────────────────────

fn discard_modal(ctx: &egui::Context, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let Some(change) = repo.confirm_discard.clone() else {
        return;
    };
    let mut close = false;
    let modal = Modal::new(Id::unique("discard_modal"))
        .frame(popup_frame().inner_margin(Margin::same(20)))
        .backdrop_color(Color32::from_black_alpha(140))
        .show(ctx, |ui| {
            ui.set_width(360.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let untracked = change.kind == ChangeKind::Untracked;
            ui.label(
                egui::RichText::new(if untracked {
                    "Delete untracked file?"
                } else {
                    "Discard changes?"
                })
                .font(egui::FontId::new(15.0, theme::semibold()))
                .color(theme::TEXT),
            );
            ui.add_space(8.0);
            let body = if untracked {
                format!(
                    "{} isn't tracked by git, so discarding it deletes the file. This can't be undone.",
                    change.path
                )
            } else {
                format!(
                    "Your unstaged edits to {} will be lost. This can't be undone.",
                    change.path
                )
            };
            ui.label(egui::RichText::new(body).color(theme::TEXT_SECONDARY));
            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if untracked { "Delete file" } else { "Discard" };
                if button(ui, ButtonKind::Danger, label, None, true).clicked() {
                    cmds.push(Cmd::Act(Action::Discard(change.clone())));
                    close = true;
                }
                if button(ui, ButtonKind::Secondary, "Cancel", None, true).clicked() {
                    close = true;
                }
            });
        });
    if close || modal.should_close() {
        repo.confirm_discard = None;
    }
}

fn toast_ui(ctx: &egui::Context, toast: &mut Option<Toast>) {
    let Some(current) = toast else { return };
    let mut dismiss = false;
    Area::new(Id::unique("toast"))
        .order(Order::Tooltip)
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
        .show(ctx, |ui| {
            let response = popup_frame()
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (slot, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                        paint_icon(ui, slot.center(), icon::WARNING, 16.0, theme::RED);
                        // Git errors can run long; show the start of them.
                        let text: String = current.text.chars().take(600).collect();
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).color(theme::TEXT)).wrap(),
                        );
                    });
                })
                .response;
            if response.interact(Sense::click()).clicked() {
                dismiss = true;
            }
        });
    if dismiss {
        *toast = None;
    }
}
