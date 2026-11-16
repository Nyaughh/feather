//! The settings panel: appearance, editor behaviour and updates.

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Id, Layout, Margin, Modal, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::widgets::{ButtonKind, button, icon_button, popup_frame, segmented};
use crate::app::Cmd;
use crate::settings::{Accent, FONT_SIZES, Settings, TAB_SIZES, ThemeChoice, ZOOM_LEVELS};
use crate::theme::{self, icon};
use crate::update::{self, Updater};

const WIDTH: f32 = 540.0;
const CONTROL_COLUMN: f32 = 190.0;

pub fn show(
    ctx: &egui::Context,
    settings: &mut Settings,
    open: &mut bool,
    updater: &Updater,
    cmds: &mut Vec<Cmd>,
) {
    let mut close = false;
    let modal = Modal::new(Id::unique("settings"))
        .frame(popup_frame().inner_margin(Margin::ZERO))
        .backdrop_color(Color32::from_black_alpha(120))
        .show(ctx, |ui| {
            ui.set_width(WIDTH.min(ctx.content_rect().width() - 48.0));
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

            // Header.
            let (header, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
            ui.painter().text(
                pos2(header.left() + 24.0, header.center().y),
                egui::Align2::LEFT_CENTER,
                "Settings",
                egui::FontId::new(15.0, theme::semibold()),
                theme::text(),
            );
            let slot = Rect::from_center_size(
                pos2(header.right() - 26.0, header.center().y),
                vec2(28.0, 28.0),
            );
            if icon_button(ui, slot, Id::unique("close_settings"), icon::CLOSE, "Close").clicked() {
                close = true;
            }
            ui.painter().hline(
                header.x_range(),
                header.bottom(),
                Stroke::new(1.0, theme::border()),
            );

            let max_height = (ctx.content_rect().height() - 160.0).max(200.0);
            ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(Margin {
                            left: 24,
                            right: 24,
                            top: 6,
                            bottom: 20,
                        })
                        .show(ui, |ui| body(ui, settings, updater, cmds));
                });
        });
    if close || modal.should_close() {
        *open = false;
    }
}

fn body(ui: &mut Ui, settings: &mut Settings, updater: &Updater, cmds: &mut Vec<Cmd>) {
    section(ui, "Appearance");
    row(
        ui,
        "Theme",
        "System follows your computer's light or dark mode.",
        |ui| {
            let choices = [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light];
            let selected = choices
                .iter()
                .position(|c| *c == settings.theme)
                .unwrap_or(0);
            if let Some(i) = segmented(
                ui,
                Id::unique("theme_choice"),
                &["System", "Dark", "Light"],
                selected,
            ) {
                settings.theme = choices[i];
            }
        },
    );
    row(
        ui,
        "Accent colour",
        "Used for buttons and selections.",
        |ui| {
            accent_picker(ui, &mut settings.accent);
        },
    );
    row(
        ui,
        "Interface size",
        "Scales everything in the window.",
        |ui| {
            let i = ZOOM_LEVELS
                .iter()
                .position(|z| *z == settings.zoom)
                .unwrap_or(2);
            let label = format!("{:.0}%", settings.zoom * 100.0);
            if let Some(step) = stepper(ui, "zoom", &label, i > 0, i + 1 < ZOOM_LEVELS.len()) {
                settings.zoom = ZOOM_LEVELS[(i as i32 + step) as usize];
            }
        },
    );

    section(ui, "Editor");
    row(
        ui,
        "Font size",
        "Size of code in the editor and in diffs.",
        |ui| {
            let (min, max) = (*FONT_SIZES.start(), *FONT_SIZES.end());
            let label = format!("{}", settings.font_size);
            let size = settings.font_size;
            if let Some(step) = stepper(ui, "font_size", &label, size > min, size < max) {
                settings.font_size = (size + step as f32 * 0.5).clamp(min, max);
            }
        },
    );
    row(
        ui,
        "Wrap long lines",
        "Fold lines at the window edge instead of scrolling sideways.",
        |ui| {
            toggle(ui, "line_wrap", &mut settings.line_wrap);
        },
    );
    row(ui, "Line numbers", "Show them beside the code.", |ui| {
        toggle(ui, "line_numbers", &mut settings.line_numbers);
    });
    row(
        ui,
        "Indent with spaces",
        "What the Tab key inserts. Off inserts a tab character.",
        |ui| {
            toggle(ui, "insert_spaces", &mut settings.insert_spaces);
        },
    );
    row(ui, "Indent width", "Spaces inserted per Tab.", |ui| {
        let selected = TAB_SIZES
            .iter()
            .position(|t| *t == settings.tab_size)
            .unwrap_or(1);
        if let Some(i) = segmented(ui, Id::unique("tab_size"), &["2", "4", "8"], selected) {
            settings.tab_size = TAB_SIZES[i];
        }
    });
    row(
        ui,
        "Auto save",
        "Save edits a moment after you stop typing.",
        |ui| {
            toggle(ui, "auto_save", &mut settings.auto_save);
        },
    );

    section(ui, "Git");
    row(
        ui,
        "Fetch automatically",
        "Check the remote every few minutes for new branches and commits.",
        |ui| toggle(ui, "auto_fetch", &mut settings.auto_fetch),
    );

    section(ui, "Updates");
    row(
        ui,
        "Check automatically",
        "Look for a new version when Feather starts.",
        |ui| {
            toggle(ui, "auto_update", &mut settings.auto_update);
        },
    );
    let (status, action): (String, Option<(&str, Cmd)>) = match &updater.state {
        update::State::Checking => ("Checking for updates…".to_owned(), None),
        update::State::Available(release) if release.installable() => (
            format!("Version {} is available.", release.version),
            Some(("Update", Cmd::InstallUpdate)),
        ),
        update::State::Available(release) => (
            format!("Version {} is available.", release.version),
            Some(("Download", Cmd::OpenUrl(release.page.clone()))),
        ),
        update::State::Installing(release) => {
            (format!("Installing version {}…", release.version), None)
        }
        update::State::Ready(release) => (
            format!(
                "Version {} is installed. Restart to use it.",
                release.version
            ),
            Some(("Restart", Cmd::RestartToUpdate)),
        ),
        update::State::InstallFailed(_, error) => {
            (error.clone(), Some(("Try again", Cmd::InstallUpdate)))
        }
        update::State::CheckFailed(error) => {
            (error.clone(), Some(("Check again", Cmd::CheckForUpdates)))
        }
        update::State::UpToDate => (
            "You have the latest version.".to_owned(),
            Some(("Check again", Cmd::CheckForUpdates)),
        ),
        update::State::Idle => (
            String::new(),
            Some(("Check for updates", Cmd::CheckForUpdates)),
        ),
    };
    let title = format!("Feather {}", update::CURRENT_VERSION);
    row(ui, &title, &status, |ui| {
        if let Some((label, cmd)) = action {
            let kind = match cmd {
                Cmd::InstallUpdate | Cmd::RestartToUpdate => ButtonKind::Primary,
                _ => ButtonKind::Secondary,
            };
            if button(ui, kind, label, None, true).clicked() {
                cmds.push(cmd);
            }
        } else {
            ui.add(egui::Spinner::new().size(16.0).color(theme::text_muted()));
        }
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(
        egui::RichText::new(title)
            .font(theme::medium_font(11.5))
            .color(theme::text_muted()),
    );
    ui.add_space(4.0);
}

/// One setting: its name and a line of explanation on the left, the control
/// on the right.
fn row(ui: &mut Ui, title: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(width - CONTROL_COLUMN);
            ui.label(
                egui::RichText::new(title)
                    .font(theme::ui_font(13.0))
                    .color(theme::text()),
            );
            if !description.is_empty() {
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(description)
                            .size(12.0)
                            .color(theme::text_muted()),
                    )
                    .wrap(),
                );
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.add_space(10.0);
    let line = ui.available_rect_before_wrap();
    ui.painter().hline(
        line.x_range(),
        line.top(),
        Stroke::new(1.0, theme::border()),
    );
}

/// An on/off switch.
fn toggle(ui: &mut Ui, name: &str, on: &mut bool) {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        *on = !*on;
    }
    let hovered = response.hovered();
    let t = ui
        .ctx()
        .animate_bool_with_time(Id::unique(("toggle", name)), *on, 0.1);
    let track = if *on {
        theme::accent()
    } else {
        theme::overlay(if hovered { 44 } else { 30 })
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), track);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 7.0, Color32::WHITE);
}

/// `− value +`. Returns -1 or 1 when a button is pressed.
fn stepper(
    ui: &mut Ui,
    name: &str,
    value: &str,
    can_decrease: bool,
    can_increase: bool,
) -> Option<i32> {
    let (rect, _) = ui.allocate_exact_size(vec2(112.0, 28.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::RADIUS),
        theme::overlay(8),
        Stroke::new(1.0, theme::border_strong()),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        value,
        theme::medium_font(12.5),
        theme::text(),
    );
    let mut step = None;
    for (glyph, x, enabled, delta, tip) in [
        (icon::MINUS, rect.left() + 15.0, can_decrease, -1, "Smaller"),
        (icon::PLUS, rect.right() - 15.0, can_increase, 1, "Larger"),
    ] {
        let slot = Rect::from_center_size(pos2(x, rect.center().y), vec2(24.0, 22.0));
        if enabled {
            if icon_button(ui, slot, Id::unique((name, delta)), glyph, tip).clicked() {
                step = Some(delta);
            }
        } else {
            super::widgets::paint_icon(ui, slot.center(), glyph, 14.0, theme::text_faint());
        }
    }
    step
}

fn accent_picker(ui: &mut Ui, accent: &mut Accent) {
    ui.spacing_mut().item_spacing.x = 6.0;
    // Laid out right to left, so walk the list backwards to keep its order.
    for choice in Accent::ALL.into_iter().rev() {
        let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
        let response = response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(choice.name());
        let color = theme::accent_swatch(choice);
        ui.painter().circle_filled(rect.center(), 8.0, color);
        if *accent == choice {
            ui.painter()
                .circle_stroke(rect.center(), 10.5, Stroke::new(1.5, theme::text()));
        } else if response.hovered() {
            ui.painter()
                .circle_stroke(rect.center(), 10.5, Stroke::new(1.0, theme::text_muted()));
        }
        if response.clicked() {
            *accent = choice;
        }
    }
}
