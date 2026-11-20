//! Everything drawn on screen. `root` lays out the window; the submodules fill
//! in the title bar, the sidebar and the content area.

mod content;
mod search;
mod settings;
mod sidebar;
mod terminal;
pub mod titlebar;
mod widgets;

use eframe::egui::{
    self, Align, Align2, Area, CentralPanel, Color32, CornerRadius, Frame, Id, Layout, Margin,
    Modal, Order, Panel, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};

use crate::app::{App, Cmd, Repo, Toast, ToastKind};
use crate::git::ChangeKind;
use crate::settings::{Settings, TerminalDock};
use crate::theme::{self, icon};
use crate::update::{self, Updater};
use crate::worker::Action;
use widgets::{ButtonKind, button, elided, paint_galley, paint_icon, paint_row_bg, popup_frame};

pub fn root(app: &mut App, ui: &mut Ui, cmds: &mut Vec<Cmd>) {
    let ctx = ui.ctx().clone();
    titlebar::show(
        ui,
        app.repo.as_ref(),
        app.settings.sidebar_open,
        &app.updater,
        cmds,
    );

    match &mut app.repo {
        None => welcome(ui, &app.settings, cmds),
        Some(repo) => {
            let sidebar_open = app.settings.sidebar_open;
            if sidebar_open {
                Panel::left("sidebar")
                    .resizable(true)
                    .default_size(264.0)
                    .size_range(208.0..=440.0)
                    .frame(Frame::new().fill(theme::bg_shell()))
                    .show_separator_line(false)
                    .show(ui, |ui| sidebar::show(ui, repo, &app.settings, cmds));
            }

            // The terminal goes under the content, or beside it on the right.
            // Each position remembers its own size.
            if repo.terminals.open {
                let dock = app.settings.terminal_dock;
                let panel = match dock {
                    TerminalDock::Bottom => Panel::bottom("terminal_panel")
                        .default_size(260.0)
                        .size_range(110.0..=640.0),
                    TerminalDock::Right => Panel::right("terminal_panel_right")
                        .default_size(440.0)
                        .size_range(260.0..=1000.0),
                };
                panel
                    .resizable(true)
                    .frame(Frame::new().fill(theme::bg_shell()).inner_margin(Margin {
                        left: if sidebar_open || dock == TerminalDock::Right {
                            0
                        } else {
                            8
                        },
                        right: 8,
                        top: 0,
                        bottom: 8,
                    }))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        Frame::new()
                            .fill(theme::bg_panel())
                            .stroke(Stroke::new(1.0, theme::border()))
                            .corner_radius(CornerRadius::same(theme::panel_radius()))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                ui.shrink_clip_rect(ui.max_rect().shrink(1.0));
                                terminal::show(ui, repo, dock, cmds);
                            });
                    });
            } else {
                repo.terminals.focused = false;
            }

            // The content sits in a bordered, rounded panel inset from the shell.
            CentralPanel::default()
                .frame(Frame::new().fill(theme::bg_shell()).inner_margin(Margin {
                    left: if sidebar_open { 0 } else { 8 },
                    right: 8,
                    top: 0,
                    bottom: 8,
                }))
                .show(ui, |ui| {
                    Frame::new()
                        .fill(theme::bg_panel())
                        .stroke(Stroke::new(1.0, theme::border()))
                        .corner_radius(CornerRadius::same(theme::panel_radius()))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            // Keep content from painting over the rounded corners.
                            ui.shrink_clip_rect(ui.max_rect().shrink(1.0));
                            content::show(ui, repo, &app.settings, &mut app.markdown, cmds);
                        });
                });

            discard_modal(&ctx, repo, cmds);
            discard_all_modal(&ctx, repo, cmds);
            drop_stash_modal(&ctx, repo, cmds);
            search::quick_open(&ctx, repo, cmds);
        }
    }

    if app.settings_open {
        settings::show(
            &ctx,
            &mut app.settings,
            &mut app.settings_open,
            &app.updater,
            cmds,
        );
    }
    theme_import_modal(&ctx, app);
    leave_modal(&ctx, app, cmds);
    update_prompt(&ctx, &mut app.updater, cmds);
    toast_ui(&ctx, &mut app.toast);
    titlebar::frame(&ctx);
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
    add_row: impl FnMut(&mut Ui, usize, Rect),
) {
    virtual_rows_from(
        ui,
        id,
        horizontal,
        row_height,
        count,
        content_width,
        pad,
        None,
        add_row,
    );
}

/// [`virtual_rows`], optionally scrolled to a vertical offset first.
#[allow(clippy::too_many_arguments)]
pub(crate) fn virtual_rows_from(
    ui: &mut Ui,
    id: Id,
    horizontal: bool,
    row_height: f32,
    count: usize,
    content_width: f32,
    pad: f32,
    scroll_to: Option<f32>,
    mut add_row: impl FnMut(&mut Ui, usize, Rect),
) {
    let mut area = ScrollArea::new([horizontal, true])
        .id_salt(id)
        .auto_shrink(false);
    if let Some(offset) = scroll_to {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_viewport(ui, |ui, viewport| {
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
        .frame(Frame::new().fill(theme::bg_shell()))
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
                    .rect_filled(mark, CornerRadius::same(11), theme::accent());
                paint_icon(ui, mark.center(), icon::FEATHER, 24.0, theme::on_accent());

                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("Feather")
                        .font(egui::FontId::new(20.0, theme::semibold()))
                        .color(theme::text()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Open a repository to browse, edit and commit.")
                        .color(theme::text_secondary()),
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
                        .color(theme::text_muted()),
                );

                if !recent.is_empty() {
                    ui.add_space(28.0);
                    let (label, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
                    ui.painter().text(
                        pos2(label.left() + 10.0, label.center().y),
                        Align2::LEFT_CENTER,
                        "Recent",
                        theme::medium_font(11.5),
                        theme::text_muted(),
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
                            theme::text(),
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
                            theme::text_muted(),
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

// ── Dialogs ────────────────────────────────────────────────────────────────

fn dialog_title(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(egui::FontId::new(15.0, theme::semibold()))
            .color(theme::text()),
    );
    ui.add_space(8.0);
}

fn dialog_body(ui: &mut Ui, text: String) {
    ui.label(egui::RichText::new(text).color(theme::text_secondary()));
    ui.add_space(20.0);
}

fn dialog(ctx: &egui::Context, name: &str, add: impl FnOnce(&mut Ui)) -> bool {
    Modal::new(Id::unique(name))
        .frame(popup_frame().inner_margin(Margin::same(20)))
        .backdrop_color(Color32::from_black_alpha(140))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        })
        .should_close()
}

fn discard_modal(ctx: &egui::Context, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let Some(change) = repo.confirm_discard.clone() else {
        return;
    };
    let mut close = false;
    let dismissed = dialog(ctx, "discard_modal", |ui| {
        let untracked = change.kind == ChangeKind::Untracked;
        dialog_title(
            ui,
            if untracked {
                "Delete untracked file?"
            } else {
                "Discard changes?"
            },
        );
        dialog_body(
            ui,
            if untracked {
                format!(
                    "{} isn't tracked by git, so discarding it deletes the file. This can't be undone.",
                    change.path
                )
            } else {
                format!(
                    "Your unstaged edits to {} will be lost. This can't be undone.",
                    change.path
                )
            },
        );
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
    if close || dismissed {
        repo.confirm_discard = None;
    }
}

fn discard_all_modal(ctx: &egui::Context, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    if !repo.confirm_discard_all {
        return;
    }
    let changes = &repo.status.unstaged;
    let untracked = changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Untracked)
        .count();
    let edited = changes.len() - untracked;
    let files = |n: usize| if n == 1 { "file" } else { "files" };
    let mut parts = Vec::new();
    if edited > 0 {
        parts.push(format!("Edits to {edited} {} will be lost", files(edited)));
    }
    if untracked > 0 {
        parts.push(format!(
            "{untracked} untracked {} will be deleted",
            files(untracked)
        ));
    }
    let mut close = false;
    let dismissed = dialog(ctx, "discard_all_modal", |ui| {
        dialog_title(ui, "Discard all changes?");
        dialog_body(
            ui,
            format!(
                "{}. Staged changes are kept. This can't be undone.",
                parts.join(", and ")
            ),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if button(ui, ButtonKind::Danger, "Discard all", None, true).clicked() {
                cmds.push(Cmd::Act(Action::DiscardAll));
                close = true;
            }
            if button(ui, ButtonKind::Secondary, "Cancel", None, true).clicked() {
                close = true;
            }
        });
    });
    if close || dismissed || changes.is_empty() {
        repo.confirm_discard_all = false;
    }
}

fn drop_stash_modal(ctx: &egui::Context, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let Some(stash) = &repo.confirm_drop_stash else {
        return;
    };
    let mut close = false;
    let dismissed = dialog(ctx, "drop_stash_modal", |ui| {
        dialog_title(ui, "Delete this stash?");
        dialog_body(
            ui,
            format!(
                "\"{}\" will be deleted without being applied. This can't be undone.",
                stash.label
            ),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if button(ui, ButtonKind::Danger, "Delete", None, true).clicked() {
                cmds.push(Cmd::Act(Action::StashDrop(stash.index)));
                close = true;
            }
            if button(ui, ButtonKind::Secondary, "Cancel", None, true).clicked() {
                close = true;
            }
        });
    });
    // The list moved under the dialog: what it names may no longer be there.
    let gone = !repo.stashes.contains(stash);
    if close || dismissed || gone {
        repo.confirm_drop_stash = None;
    }
}

/// "Save your changes first?", shown before anything that would drop them.
fn leave_modal(ctx: &egui::Context, app: &App, cmds: &mut Vec<Cmd>) {
    if app.confirm_leave.is_none() {
        return;
    }
    let file = match app.repo.as_ref().map(|r| &r.view) {
        Some(crate::app::View::File(view)) => view.rel.clone(),
        _ => "this file".to_owned(),
    };
    let dismissed = dialog(ctx, "leave_modal", |ui| {
        dialog_title(ui, "Save your changes?");
        dialog_body(
            ui,
            format!(
                "{file} has edits that haven't been saved. They will be lost if you don't save them."
            ),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if button(ui, ButtonKind::Primary, "Save", None, true).clicked() {
                cmds.push(Cmd::LeaveSaving);
            }
            if button(ui, ButtonKind::Secondary, "Don't save", None, true).clicked() {
                cmds.push(Cmd::LeaveDiscarding);
            }
            if button(ui, ButtonKind::Secondary, "Cancel", None, true).clicked() {
                cmds.push(Cmd::LeaveCancelled);
            }
        });
    });
    if dismissed {
        cmds.push(Cmd::LeaveCancelled);
    }
}

/// Paste-in dialog for a theme from tweakcn (or any shadcn/ui theme).
fn theme_import_modal(ctx: &egui::Context, app: &mut App) {
    let Some(draft) = &mut app.theme_import else {
        return;
    };
    let mut import = false;
    let mut cancel = false;
    let dismissed = Modal::new(Id::unique("theme_import"))
        .frame(popup_frame().inner_margin(Margin::same(20)))
        .backdrop_color(Color32::from_black_alpha(140))
        .show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ctx.content_rect().width() - 80.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            dialog_title(ui, "Import a theme");
            ui.label(
                egui::RichText::new(
                    "On tweakcn.com, pick or build a theme, press Code and copy the CSS. Paste it \
                     below. Any shadcn/ui theme works the same way.",
                )
                .color(theme::text_secondary()),
            );
            ui.add_space(14.0);
            let field = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
                Frame::new()
                    .fill(theme::bg_input())
                    .stroke(Stroke::new(1.0, theme::border_strong()))
                    .corner_radius(CornerRadius::same(theme::radius()))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| add(ui));
            };
            field(ui, &mut |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .align(egui::Align2::LEFT_CENTER)
                        .hint_text("Name, for example Sunset")
                        .desired_width(f32::INFINITY)
                        .frame(Frame::new()),
                );
            });
            ui.add_space(8.0);
            field(ui, &mut |ui| {
                ScrollArea::vertical()
                    .id_salt("theme_css")
                    .max_height(220.0)
                    .min_scrolled_height(220.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut draft.css)
                                .hint_text(":root {\n  --background: oklch(1 0 0);\n  ...\n}")
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(10)
                                .desired_width(f32::INFINITY)
                                .frame(Frame::new()),
                        );
                    });
            });
            if let Some(error) = &draft.error {
                ui.add_space(10.0);
                ui.add(egui::Label::new(egui::RichText::new(error).color(theme::red())).wrap());
            }
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ready = !draft.css.trim().is_empty();
                if button(ui, ButtonKind::Primary, "Import", None, ready).clicked() {
                    import = true;
                }
                if button(ui, ButtonKind::Secondary, "Cancel", None, true).clicked() {
                    cancel = true;
                }
            });
        })
        .should_close();

    if import {
        match crate::themes::import_css(&draft.css) {
            Ok(_) => {
                let name = match draft.name.trim() {
                    "" => format!("Imported {}", app.settings.custom_themes.len() + 1),
                    name => name.to_owned(),
                };
                let imported = crate::settings::CustomTheme {
                    name,
                    css: draft.css.clone(),
                };
                // Importing under an existing name replaces that theme.
                app.settings
                    .custom_themes
                    .retain(|t| t.name != imported.name);
                app.settings.theme_name = imported.id();
                app.settings.custom_themes.push(imported);
                app.theme_import = None;
            }
            Err(error) => draft.error = Some(error),
        }
    } else if cancel || dismissed {
        app.theme_import = None;
    }
}

// ── Floating cards ─────────────────────────────────────────────────────────

/// The update card in the bottom-right corner.
fn update_prompt(ctx: &egui::Context, updater: &mut Updater, cmds: &mut Vec<Cmd>) {
    use update::State;
    if updater.dismissed {
        return;
    }
    let (title, detail): (String, String) = match &updater.state {
        State::Available(release) => (
            format!("Feather {} is available", release.version),
            format!("You have {}.", update::CURRENT_VERSION),
        ),
        State::Installing(release) => (
            format!("Updating to Feather {}…", release.version),
            "Downloading and checking the new version.".to_owned(),
        ),
        State::Ready(release) => (
            format!("Feather {} is installed", release.version),
            "Restart to start using it.".to_owned(),
        ),
        State::InstallFailed(_, error) => ("The update didn't install".to_owned(), error.clone()),
        _ => return,
    };

    let mut dismiss = false;
    Area::new(Id::unique("update_prompt"))
        .order(Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0))
        .show(ctx, |ui| {
            popup_frame().inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(300.0);
                ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                ui.label(
                    egui::RichText::new(title)
                        .font(theme::medium_font(13.5))
                        .color(theme::text()),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(detail).color(theme::text_secondary()))
                        .wrap(),
                );
                if matches!(updater.state, State::Installing(_)) {
                    ui.add_space(12.0);
                    ui.add(egui::Spinner::new().size(16.0).color(theme::text_muted()));
                    return;
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| match &updater.state {
                    State::Available(release) => {
                        if release.installable() {
                            if button(ui, ButtonKind::Primary, "Update", None, true).clicked() {
                                cmds.push(Cmd::InstallUpdate);
                            }
                        } else if button(ui, ButtonKind::Primary, "Download", None, true).clicked()
                        {
                            cmds.push(Cmd::OpenUrl(release.page.clone()));
                        }
                        if button(ui, ButtonKind::Secondary, "Later", None, true).clicked() {
                            dismiss = true;
                        }
                    }
                    State::Ready(_) => {
                        if button(ui, ButtonKind::Primary, "Restart now", None, true).clicked() {
                            cmds.push(Cmd::RestartToUpdate);
                        }
                        if button(ui, ButtonKind::Secondary, "Later", None, true).clicked() {
                            dismiss = true;
                        }
                    }
                    State::InstallFailed(release, _) => {
                        if button(ui, ButtonKind::Primary, "Try again", None, true).clicked() {
                            cmds.push(Cmd::InstallUpdate);
                        }
                        if button(ui, ButtonKind::Secondary, "Download", None, true).clicked() {
                            cmds.push(Cmd::OpenUrl(release.page.clone()));
                        }
                        if button(ui, ButtonKind::Secondary, "Close", None, true).clicked() {
                            dismiss = true;
                        }
                    }
                    _ => {}
                });
            });
        });
    if dismiss {
        updater.dismissed = true;
    }
}

fn toast_ui(ctx: &egui::Context, toast: &mut Option<Toast>) {
    let Some(current) = toast else { return };
    let mut dismiss = false;
    Area::new(Id::unique("toast"))
        .order(Order::Tooltip)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -24.0))
        .show(ctx, |ui| {
            let response = popup_frame()
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (slot, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                        let (glyph, color) = match current.kind {
                            ToastKind::Error => (icon::WARNING, theme::red()),
                            ToastKind::Info => (icon::INFO, theme::text_secondary()),
                        };
                        paint_icon(ui, slot.center(), glyph, 16.0, color);
                        // Git errors can run long; show the start of them.
                        let text: String = current.text.chars().take(600).collect();
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).color(theme::text())).wrap(),
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
