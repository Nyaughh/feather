//! The settings panel: appearance, editor behaviour and updates.

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Id, Layout, Margin, Modal, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::widgets::{ButtonKind, button, icon_button, popup_frame, segmented};
use crate::app::Cmd;
use crate::settings::{Accent, FONT_SIZES, Settings, TAB_SIZES, ThemeChoice, ZOOM_LEVELS};
use crate::theme::{self, icon};
use crate::themes;
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
    theme_grid(ui, settings, cmds);
    row(
        ui,
        "Mode",
        "Each theme has a light and a dark side. System follows your computer.",
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
    accent_row(ui, &mut settings.accent);
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

    section(ui, "Terminal");
    row(
        ui,
        "Shell",
        "The program new terminals run. Leave empty for the usual one.",
        |ui| {
            egui::Frame::new()
                .fill(theme::bg_input())
                .stroke(Stroke::new(1.0, theme::border_strong()))
                .corner_radius(CornerRadius::same(theme::radius()))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut settings.shell)
                            .align(egui::Align2::LEFT_CENTER)
                            .hint_text(if cfg!(windows) {
                                "powershell.exe"
                            } else {
                                "Your login shell"
                            })
                            .desired_width(150.0)
                            .frame(egui::Frame::new()),
                    );
                });
        },
    );

    section(ui, "Git");
    row(
        ui,
        "Fetch automatically",
        "Check the remote every few minutes for new branches and commits.",
        |ui| toggle(ui, "auto_fetch", &mut settings.auto_fetch),
    );
    row(
        ui,
        "Show blame",
        "Beside the line the cursor is on: who last changed it, when, and why.",
        |ui| toggle(ui, "blame", &mut settings.blame),
    );

    command_line(ui, cmds);

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

/// The `feather` terminal command, and putting it on the PATH.
fn command_line(ui: &mut Ui, cmds: &mut Vec<Cmd>) {
    use crate::shell_command::{self, Status};
    section(ui, "Command line");
    let link = shell_command::link_path().map(|p| p.display().to_string());
    let (description, action) = match shell_command::status() {
        Status::Installer => (
            "Type feather . or feather main.rs in a terminal to open it here. The \
             installer puts the command on your PATH."
                .to_owned(),
            None,
        ),
        Status::Installed => (
            format!(
                "Installed as {}. Type feather . or feather main.rs in a terminal to open it here.",
                link.unwrap_or_default()
            ),
            Some(("Remove", Cmd::RemoveCommand)),
        ),
        Status::Missing | Status::Stale(_) => (
            format!(
                "Open files and folders from a terminal with feather . or feather main.rs. \
                 Installing links {} to Feather.",
                link.unwrap_or_default()
            ),
            Some(("Install", Cmd::InstallCommand)),
        ),
        Status::Taken(path) => (
            format!("{} belongs to another program.", path.display()),
            None,
        ),
    };
    row(ui, "The feather command", &description, |ui| {
        if let Some((label, cmd)) = action
            && button(ui, ButtonKind::Secondary, label, None, true).clicked()
        {
            cmds.push(cmd);
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
    // On a pale accent (the blush, monochrome) a white knob would vanish.
    let knob = if *on {
        theme::on_accent()
    } else {
        Color32::WHITE
    };
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 7.0, knob);
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
        CornerRadius::same(theme::radius()),
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

/// Like [`row`], but with the swatches on their own line under the text: there
/// are too many to fit beside it.
fn accent_row(ui: &mut Ui, accent: &mut Accent) {
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("Accent colour")
            .font(theme::ui_font(13.0))
            .color(theme::text()),
    );
    ui.add_space(2.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Used for buttons and selections. The first keeps the theme's own, which \
                 for Feather is the golden-hour blush of its icon.",
            )
            .size(12.0)
            .color(theme::text_muted()),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| accent_picker(ui, accent));
    ui.add_space(10.0);
    let line = ui.available_rect_before_wrap();
    ui.painter().hline(
        line.x_range(),
        line.top(),
        Stroke::new(1.0, theme::border()),
    );
}

fn accent_picker(ui: &mut Ui, accent: &mut Accent) {
    ui.spacing_mut().item_spacing.x = 6.0;
    let dark = theme::is_dark();
    for choice in Accent::ALL {
        let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
        let response = response
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(choice.name());
        match theme::accent_swatch(choice, dark) {
            Some(color) => {
                ui.painter().circle_filled(rect.center(), 8.0, color);
            }
            // "The theme's own": an outlined dot rather than a fixed color.
            None => {
                ui.painter().circle_stroke(
                    rect.center(),
                    7.25,
                    Stroke::new(1.5, theme::text_secondary()),
                );
            }
        }
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
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(accent.name())
            .size(12.0)
            .color(theme::text_secondary()),
    );
}

/// The themes, two to a row, each drawn in its own colors.
fn theme_grid(ui: &mut Ui, settings: &mut Settings, cmds: &mut Vec<Cmd>) {
    const HEIGHT: f32 = 44.0;
    const GAP: f32 = 8.0;
    let dark = theme::is_dark();

    // (id, name, palette, is custom)
    let mut cards: Vec<(String, String, theme::Palette, bool)> = themes::presets()
        .iter()
        .map(|p| {
            let palette = if dark { p.dark } else { p.light };
            (p.id.to_owned(), p.name.to_owned(), palette, false)
        })
        .collect();
    for custom in &settings.custom_themes {
        if let Some(palette) = themes::import_css(&custom.css)
            .ok()
            .and_then(|t| t.pick(dark))
        {
            cards.push((custom.id(), custom.name.clone(), palette, true));
        }
    }

    ui.add_space(8.0);
    let width = (ui.available_width() - GAP) / 2.0;
    let mut remove = None;
    for pair in cards.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (id, name, palette, custom) in pair {
                let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
                let response = response.on_hover_cursor(CursorIcon::PointingHand);
                let selected = settings.theme_name == *id;
                let outline = if selected {
                    Stroke::new(2.0, theme::accent())
                } else if response.hovered() {
                    Stroke::new(1.0, theme::text_muted())
                } else {
                    Stroke::new(1.0, theme::border_strong())
                };
                let painter = ui.painter();
                painter.rect(
                    rect,
                    CornerRadius::same(theme::panel_radius()),
                    palette.bg_panel,
                    outline,
                    StrokeKind::Inside,
                );
                painter.text(
                    pos2(rect.left() + 14.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    theme::medium_font(13.0),
                    palette.text,
                );
                let mut x = rect.right() - if *custom { 40.0 } else { 16.0 };
                for color in [palette.red, palette.green, palette.blue, palette.accent] {
                    painter.circle_filled(pos2(x, rect.center().y), 5.0, color);
                    x -= 14.0;
                }
                if response.clicked() {
                    settings.theme_name.clone_from(id);
                }
                if *custom {
                    let slot = Rect::from_center_size(
                        pos2(rect.right() - 16.0, rect.center().y),
                        vec2(22.0, 22.0),
                    );
                    let close = ui
                        .interact(slot, Id::unique(("remove_theme", id)), Sense::click())
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Remove this theme");
                    let color = if close.hovered() {
                        palette.text
                    } else {
                        palette.text_muted
                    };
                    super::widgets::paint_icon(ui, slot.center(), icon::CLOSE, 12.0, color);
                    if close.clicked() {
                        remove = Some(id.clone());
                    }
                }
            }
        });
        ui.add_space(GAP);
    }
    if let Some(id) = remove {
        settings.custom_themes.retain(|t| t.id() != id);
        if settings.theme_name == id {
            settings.theme_name = themes::DEFAULT_THEME.to_owned();
        }
    }
    ui.horizontal(|ui| {
        if button(ui, ButtonKind::Secondary, "Import a theme", None, true)
            .on_hover_text("Paste a theme from tweakcn.com or any shadcn/ui theme")
            .clicked()
        {
            cmds.push(Cmd::ImportTheme);
        }
    });
    ui.add_space(6.0);
}
