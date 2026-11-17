//! The left column: repository switcher, Files/Git tabs, and the
//! worktree + branch pickers pinned to the bottom.

use eframe::egui::{
    self, Align2, CentralPanel, Color32, CornerRadius, CursorIcon, Frame, Id, Margin, Panel, Popup,
    PopupCloseBehavior, Rect, RectAlign, Response, ScrollArea, Sense, Stroke, TextEdit, Ui, pos2,
    vec2,
};

use super::virtual_rows;
use super::widgets::{
    ButtonKind, button, elided, empty_state, icon_button, menu_item, paint_galley, paint_icon,
    paint_row_bg, popup_frame,
};
use crate::app::{Cmd, Repo, Tab, View};
use crate::git::{Branch, Change, ChangeKind};
use crate::settings::Settings;
use crate::theme::{self, ROW_HEIGHT, icon};
use crate::worker::Action;

const PAD: f32 = 8.0;
const TABS_HEIGHT: f32 = 36.0;
const FOOTER_ROW: f32 = 28.0;
const SWITCHER_HEIGHT: f32 = 40.0;

pub fn show(ui: &mut Ui, repo: &mut Repo, settings: &Settings, cmds: &mut Vec<Cmd>) {
    Panel::top("sidebar_header")
        .exact_size(SWITCHER_HEIGHT + TABS_HEIGHT)
        .frame(Frame::new())
        .show_separator_line(false)
        .show(ui, |ui| {
            repo_switcher(ui, repo, settings, cmds);
            tabs(ui, repo);
        });

    if repo.is_git {
        Panel::bottom("sidebar_footer")
            .exact_size(FOOTER_ROW + PAD * 2.0)
            .frame(Frame::new())
            .show_separator_line(false)
            .show(ui, |ui| footer(ui, repo, cmds));
    }

    match repo.tab {
        Tab::Files => {
            CentralPanel::default()
                .frame(Frame::new())
                .show(ui, |ui| files(ui, repo, cmds));
        }
        Tab::Search => {
            CentralPanel::default()
                .frame(Frame::new())
                .show(ui, |ui| super::search::show(ui, repo, cmds));
        }
        Tab::Git => {
            Panel::bottom("commit_box")
                .frame(Frame::new().inner_margin(Margin::same(PAD as i8)))
                .show_separator_line(false)
                .show(ui, |ui| commit_box(ui, repo, cmds));
            CentralPanel::default()
                .frame(Frame::new())
                .show(ui, |ui| changes(ui, repo, cmds));
        }
    }
}

// ── Header ─────────────────────────────────────────────────────────────────

fn repo_switcher(ui: &mut Ui, repo: &Repo, settings: &Settings, cmds: &mut Vec<Cmd>) {
    let (outer, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), SWITCHER_HEIGHT), Sense::hover());
    let rect = Rect::from_min_max(
        pos2(outer.left() + PAD, outer.top() + 2.0),
        pos2(outer.right() - PAD, outer.bottom() - 6.0),
    );
    let response = ui
        .interact(rect, ui.scope_id().with("repo_switcher"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let open = Popup::is_id_open(ui.ctx(), Popup::default_response_id(&response));
    paint_row_bg(ui, rect, response.hovered() || open, false);

    let mid = rect.center().y;
    let x = rect.left() + 10.0;
    let name = elided(
        ui,
        &repo.name,
        theme::medium_font(13.5),
        theme::text(),
        rect.width() - 44.0,
    );
    let w = paint_galley(ui, x, mid, name);
    paint_icon(
        ui,
        pos2(x + w + 12.0, mid),
        icon::CARET_UP_DOWN,
        12.0,
        theme::text_muted(),
    );

    Popup::menu(&response)
        .frame(popup_frame())
        .gap(4.0)
        .show(|ui| {
            ui.set_width(280.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::O,
            ));
            if menu_item(
                ui,
                Some(icon::FOLDER_OPEN),
                "Open repository…",
                Some(&shortcut),
                false,
            )
            .clicked()
            {
                cmds.push(Cmd::PickRepo);
            }
            let others: Vec<_> = settings
                .recent
                .iter()
                .filter(|p| **p != repo.root)
                .collect();
            if !others.is_empty() {
                section_label(ui, "Recent");
                for path in others {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let parent = path
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    if menu_item(ui, None, &name, Some(&parent), false).clicked() {
                        cmds.push(Cmd::OpenRepo(path.clone()));
                    }
                }
            }
        });
}

fn section_label(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(
        pos2(rect.left() + 8.0, rect.center().y + 2.0),
        Align2::LEFT_CENTER,
        text,
        theme::medium_font(11.5),
        theme::text_muted(),
    );
}

fn tabs(ui: &mut Ui, repo: &mut Repo) {
    let (outer, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), TABS_HEIGHT), Sense::hover());
    let track = Rect::from_min_max(
        pos2(outer.left() + PAD, outer.top() + 2.0),
        pos2(outer.right() - PAD, outer.bottom() - 6.0),
    );
    ui.painter()
        .rect_filled(track, CornerRadius::same(7), theme::overlay(8));

    let count = repo.status.change_count();
    let all = [
        (Tab::Files, "Files"),
        (Tab::Search, "Search"),
        (Tab::Git, "Git"),
    ];
    let half = (track.width() - 4.0) / all.len() as f32;
    for (i, (tab, label)) in all.into_iter().enumerate() {
        let seg = Rect::from_min_size(
            pos2(track.left() + 2.0 + half * i as f32, track.top() + 2.0),
            vec2(half, track.height() - 4.0),
        );
        let enabled = tab != Tab::Git || repo.is_git;
        let response = ui.interact(
            seg,
            ui.scope_id().with(("tab", i)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let active = repo.tab == tab;
        if active {
            ui.painter()
                .rect_filled(seg, CornerRadius::same(5), theme::overlay(24));
        }
        let color = if !enabled {
            theme::text_faint()
        } else if active || response.hovered() {
            theme::text()
        } else {
            theme::text_secondary()
        };
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), theme::medium_font(12.5), color);
        let badge = (tab == Tab::Git && count > 0).then(|| {
            ui.painter().layout_no_wrap(
                count.to_string(),
                theme::medium_font(11.0),
                theme::text_secondary(),
            )
        });
        let badge_w = badge.as_ref().map_or(0.0, |b| b.size().x + 10.0 + 6.0);
        let x = seg.center().x - (galley.size().x + badge_w) / 2.0;
        let w = paint_galley(ui, x, seg.center().y, galley);
        if let Some(badge) = badge {
            let pill = Rect::from_min_size(
                pos2(x + w + 6.0, seg.center().y - 8.0),
                vec2(badge.size().x + 10.0, 16.0),
            );
            ui.painter()
                .rect_filled(pill, CornerRadius::same(8), theme::overlay(22));
            paint_galley(ui, pill.left() + 5.0, pill.center().y, badge);
        }
        if response.clicked() {
            repo.tab = tab;
        }
        if !enabled {
            response.on_hover_text("Not a git repository");
        }
    }
}

// ── Files tab ──────────────────────────────────────────────────────────────

fn kind_color(kind: ChangeKind) -> Color32 {
    match kind {
        ChangeKind::Modified => theme::yellow(),
        ChangeKind::Added | ChangeKind::Untracked => theme::green(),
        ChangeKind::Deleted => theme::red(),
        ChangeKind::Renamed => theme::blue(),
        ChangeKind::Conflicted => theme::orange(),
    }
}

pub(super) fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => icon::FILE_IMAGE,
        "md" | "markdown" | "txt" | "rst" | "adoc" => icon::FILE_TEXT,
        "" => icon::FILE,
        _ => icon::FILE_CODE,
    }
}

fn files(ui: &mut Ui, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    if repo.tree.len() == 0 {
        empty_state(ui, icon::FOLDER, "Empty folder", None);
        return;
    }
    let selected = match &repo.view {
        View::File(view) => Some(view.rel.as_str()),
        _ => None,
    };
    let mut toggle = None;
    let (tree, decorations) = (&repo.tree, &repo.decorations);

    virtual_rows(
        ui,
        Id::unique("file_tree"),
        false,
        ROW_HEIGHT,
        tree.len(),
        0.0,
        2.0,
        |ui, i, rect| {
            let node = tree.row(i);
            let rect = rect.shrink2(vec2(PAD, 0.5));
            let response = ui
                .interact(rect, ui.scope_id().with(("node", i)), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            let is_selected = selected == Some(node.rel.as_str());
            paint_row_bg(ui, rect, response.hovered(), is_selected);

            let ignored = decorations.is_ignored(&node.rel);
            let mid = rect.center().y;
            let mut x = rect.left() + 6.0 + f32::from(node.depth) * 14.0;
            if node.is_dir {
                let caret = if node.expanded {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                };
                paint_icon(ui, pos2(x + 5.0, mid), caret, 10.0, theme::text_muted());
            }
            x += 14.0;
            let glyph = match (node.is_dir, node.expanded) {
                (true, true) => icon::FOLDER_OPEN,
                (true, false) => icon::FOLDER,
                (false, _) => file_icon(&node.name),
            };
            let icon_color = if ignored {
                theme::text_faint()
            } else {
                theme::text_muted()
            };
            paint_icon(ui, pos2(x + 8.0, mid), glyph, 15.0, icon_color);
            x += 22.0;

            // Right edge: change marker.
            let mut right = rect.right() - 8.0;
            let status = if node.is_dir {
                None
            } else {
                decorations.file(&node.rel)
            };
            if let Some(kind) = status {
                ui.painter().text(
                    pos2(right - 5.0, mid),
                    Align2::CENTER_CENTER,
                    kind.letter(),
                    theme::medium_font(11.0),
                    kind_color(kind),
                );
                right -= 18.0;
            } else if node.is_dir && decorations.dir_is_dirty(&node.rel) {
                ui.painter().circle_filled(
                    pos2(right - 5.0, mid),
                    2.5,
                    theme::yellow().gamma_multiply(0.7),
                );
                right -= 18.0;
            }

            let color = if ignored {
                theme::text_muted()
            } else if is_selected {
                theme::text()
            } else {
                theme::text().gamma_multiply(0.88)
            };
            let name = elided(ui, &node.name, theme::ui_font(13.0), color, right - x);
            paint_galley(ui, x, mid, name);

            if response.clicked() {
                if node.is_dir {
                    toggle = Some(i);
                } else {
                    cmds.push(Cmd::OpenFile(node.rel.clone()));
                }
            }
        },
    );

    if let Some(i) = toggle {
        repo.tree.toggle(i);
    }
}

// ── Git tab ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum GitRow {
    Header { staged: bool },
    Change { staged: bool, index: usize },
}

fn changes(ui: &mut Ui, repo: &Repo, cmds: &mut Vec<Cmd>) {
    if !repo.status_loaded {
        return;
    }
    let status = &repo.status;
    if status.change_count() == 0 {
        empty_state(
            ui,
            icon::CHECK,
            "No changes",
            Some("Your working tree is clean."),
        );
        return;
    }

    let mut rows = Vec::with_capacity(status.change_count() + 2);
    for (staged, list) in [(true, &status.staged), (false, &status.unstaged)] {
        if !list.is_empty() {
            rows.push(GitRow::Header { staged });
            rows.extend((0..list.len()).map(|index| GitRow::Change { staged, index }));
        }
    }
    let selected = match &repo.view {
        View::Diff(view) => Some((view.change.path.as_str(), view.staged)),
        _ => None,
    };
    let idle = repo.busy.is_none();

    virtual_rows(
        ui,
        Id::unique("git_changes"),
        false,
        ROW_HEIGHT,
        rows.len(),
        0.0,
        2.0,
        |ui, i, rect| {
            let rect = rect.shrink2(vec2(PAD, 0.5));
            match rows[i] {
                GitRow::Header { staged } => {
                    let (label, count) = if staged {
                        ("Staged", status.staged.len())
                    } else {
                        ("Changes", status.unstaged.len())
                    };
                    let mid = rect.center().y + 2.0;
                    let galley = ui.painter().layout_no_wrap(
                        label.to_owned(),
                        theme::medium_font(11.5),
                        theme::text_muted(),
                    );
                    let w = paint_galley(ui, rect.left() + 8.0, mid, galley);
                    ui.painter().text(
                        pos2(rect.left() + 8.0 + w + 6.0, mid),
                        Align2::LEFT_CENTER,
                        count.to_string(),
                        theme::ui_font(11.5),
                        theme::text_faint(),
                    );
                    let slot =
                        Rect::from_center_size(pos2(rect.right() - 14.0, mid), vec2(22.0, 22.0));
                    let id = ui.scope_id().with(("all", staged));
                    if staged {
                        if icon_button(ui, slot, id, icon::MINUS, "Unstage all").clicked() && idle {
                            cmds.push(Cmd::Act(Action::UnstageAll));
                        }
                    } else {
                        if icon_button(ui, slot, id, icon::PLUS, "Stage all").clicked() && idle {
                            cmds.push(Cmd::Act(Action::StageAll));
                        }
                        let discard = slot.translate(vec2(-24.0, 0.0));
                        if icon_button(
                            ui,
                            discard,
                            id.with("discard"),
                            icon::DISCARD,
                            "Discard all changes",
                        )
                        .clicked()
                            && idle
                        {
                            cmds.push(Cmd::AskDiscardAll);
                        }
                    }
                }
                GitRow::Change { staged, index } => {
                    let list = if staged {
                        &status.staged
                    } else {
                        &status.unstaged
                    };
                    let change = &list[index];
                    let is_selected = selected == Some((change.path.as_str(), staged));
                    change_row(ui, rect, i, change, staged, is_selected, idle, cmds);
                }
            }
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn change_row(
    ui: &mut Ui,
    rect: Rect,
    i: usize,
    change: &Change,
    staged: bool,
    selected: bool,
    idle: bool,
    cmds: &mut Vec<Cmd>,
) {
    let response = ui
        .interact(rect, ui.scope_id().with(("change", i)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hovered = ui.rect_contains_pointer(rect);
    paint_row_bg(ui, rect, hovered, selected);

    let mid = rect.center().y;
    let mut right = rect.right() - 8.0;
    ui.painter().text(
        pos2(right - 5.0, mid),
        Align2::CENTER_CENTER,
        change.kind.letter(),
        theme::medium_font(11.0),
        kind_color(change.kind),
    );
    right -= 18.0;

    // Actions appear on hover, where the path would otherwise run.
    if hovered {
        let mut slot = |ui: &Ui, name: &str, glyph: &str, tip: &str| {
            let r = Rect::from_center_size(pos2(right - 11.0, mid), vec2(22.0, 22.0));
            right -= 24.0;
            icon_button(ui, r, ui.scope_id().with((name, i)), glyph, tip).clicked() && idle
        };
        if staged {
            if slot(ui, "unstage", icon::MINUS, "Unstage") {
                cmds.push(Cmd::Act(Action::Unstage(paths_of(change))));
            }
        } else {
            if slot(ui, "stage", icon::PLUS, "Stage") {
                cmds.push(Cmd::Act(Action::Stage(vec![change.path.clone()])));
            }
            if change.kind != ChangeKind::Conflicted
                && slot(ui, "discard", icon::DISCARD, "Discard changes")
            {
                cmds.push(Cmd::AskDiscard(change.clone()));
            }
        }
        right -= 2.0;
    }

    let (dir, name) = match change.path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", change.path.as_str()),
    };
    let x = rect.left() + 8.0;
    let name_color = if change.kind == ChangeKind::Deleted {
        theme::text_secondary()
    } else {
        theme::text()
    };
    let name = elided(ui, name, theme::ui_font(13.0), name_color, right - x);
    let w = paint_galley(ui, x, mid, name);
    let dir_x = x + w + 8.0;
    if !dir.is_empty() && right - dir_x > 24.0 {
        let dir = elided(
            ui,
            dir,
            theme::ui_font(12.0),
            theme::text_muted(),
            right - dir_x,
        );
        paint_galley(ui, dir_x, mid + 0.5, dir);
    }

    if response.clicked() {
        cmds.push(Cmd::OpenDiff {
            change: change.clone(),
            staged,
        });
    }
}

/// Paths to hand to git for a staged change; a rename needs both sides.
fn paths_of(change: &Change) -> Vec<String> {
    let mut paths = vec![change.path.clone()];
    paths.extend(change.orig_path.clone());
    paths
}

fn commit_box(ui: &mut Ui, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    let width = ui.available_width();

    let edit_id = ui.scope_id().with("commit_message");
    let focused = ui.memory(|m| m.has_focus(edit_id));
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
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("commit_scroll")
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.add(
                        TextEdit::multiline(&mut repo.commit_message)
                            .id(edit_id)
                            .hint_text("Commit message")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .frame(Frame::new()),
                    );
                });
        });

    let staged = repo.status.staged.len();
    let label = match (repo.busy, staged) {
        (Some(busy), _) => busy.to_owned(),
        (None, 0) => "Commit".to_owned(),
        (None, 1) => "Commit 1 file".to_owned(),
        (None, n) => format!("Commit {n} files"),
    };
    let shortcut = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND,
        egui::Key::Enter,
    ));
    let commit = button(
        ui,
        ButtonKind::Primary,
        &label,
        Some(width),
        repo.can_commit(),
    );
    let hint = if staged == 0 {
        "Stage changes to commit them".to_owned()
    } else if repo.commit_message.trim().is_empty() {
        "Write a commit message first".to_owned()
    } else {
        format!("Commit staged changes ({shortcut})")
    };
    if commit.on_hover_text(hint).clicked() {
        cmds.push(Cmd::Commit);
    }

    // Remote actions.
    let idle = repo.busy.is_none();
    let (ahead, behind) = (repo.status.ahead, repo.status.behind);
    let has_upstream = repo.status.upstream.is_some();
    let third = (width - 8.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let pull = count_label("Pull", behind);
        if ghost_button(ui, icon::ARROW_DOWN, &pull, third)
            .on_hover_text("Pull (fast-forward only)")
            .clicked()
            && idle
        {
            cmds.push(Cmd::Act(Action::Pull));
        }
        let push = count_label("Push", ahead);
        let tip = if has_upstream {
            "Push"
        } else {
            "Publish this branch to origin"
        };
        if ghost_button(ui, icon::ARROW_UP, &push, third)
            .on_hover_text(tip)
            .clicked()
            && idle
        {
            cmds.push(Cmd::Act(Action::Push { has_upstream }));
        }
        if ghost_button(ui, icon::REFRESH, "Fetch", third)
            .on_hover_text("Fetch from remote")
            .clicked()
            && idle
        {
            cmds.push(Cmd::Act(Action::Fetch));
        }
    });
}

fn count_label(label: &str, count: u32) -> String {
    if count > 0 {
        format!("{label} {count}")
    } else {
        label.to_owned()
    }
}

/// A quiet icon + label button with no resting background.
fn ghost_button(ui: &mut Ui, glyph: &str, label: &str, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let color = if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::radius()), theme::hover_bg());
        theme::text()
    } else {
        theme::text_secondary()
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::ui_font(12.0), color);
    let total = 13.0 + 5.0 + galley.size().x;
    let x = rect.center().x - total / 2.0;
    paint_icon(ui, pos2(x + 6.5, rect.center().y), glyph, 13.0, color);
    paint_galley(ui, x + 18.0, rect.center().y, galley);
    response
}

// ── Footer: worktree + branch ──────────────────────────────────────────────

fn footer(ui: &mut Ui, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let outer = ui.available_rect_before_wrap();
    ui.painter().hline(
        outer.x_range(),
        outer.top(),
        Stroke::new(1.0, theme::border()),
    );
    // One row, two columns: worktree on the left, branch on the right.
    let gap = 4.0;
    let width = (outer.width() - PAD * 2.0 - gap) / 2.0;
    let column = |n: f32| {
        Rect::from_min_size(
            pos2(outer.left() + PAD + (width + gap) * n, outer.top() + PAD),
            vec2(width, FOOTER_ROW),
        )
    };
    worktree_picker(ui, column(0.0), repo, cmds);
    branch_picker(ui, column(1.0), repo, cmds);
}

/// Paints a footer row and returns its click response.
fn footer_row(ui: &Ui, rect: Rect, id: &str, glyph: &str, label: &str, detail: &str) -> Response {
    let response = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let open = Popup::is_id_open(ui.ctx(), Popup::default_response_id(&response));
    let active = response.hovered() || open;
    paint_row_bg(ui, rect, active, false);

    let mid = rect.center().y;
    paint_icon(
        ui,
        pos2(rect.left() + 15.0, mid),
        glyph,
        14.0,
        theme::text_muted(),
    );
    let mut right = rect.right() - 8.0;
    if active {
        paint_icon(
            ui,
            pos2(right - 6.0, mid),
            icon::CARET_UP_DOWN,
            12.0,
            theme::text_muted(),
        );
        right -= 18.0;
    } else if !detail.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            detail.to_owned(),
            theme::ui_font(11.5),
            theme::text_muted(),
        );
        right -= galley.size().x;
        paint_galley(ui, right, mid, galley);
        right -= 8.0;
    }
    let x = rect.left() + 30.0;
    let color = if active {
        theme::text()
    } else {
        theme::text_secondary()
    };
    let galley = elided(ui, label, theme::ui_font(12.5), color, right - x);
    paint_galley(ui, x, mid, galley);
    response
}

fn worktree_picker(ui: &mut Ui, rect: Rect, repo: &Repo, cmds: &mut Vec<Cmd>) {
    let detail = match repo.worktrees.len() {
        0 | 1 => String::new(),
        n => n.to_string(),
    };
    let response = footer_row(
        ui,
        rect,
        "worktree",
        icon::TREE_STRUCTURE,
        &repo.name,
        &detail,
    )
    .on_hover_text("Switch worktree");

    Popup::menu(&response)
        .frame(popup_frame())
        .align(RectAlign::TOP_START)
        // Stay put above the row even as the list changes size.
        .align_alternatives(&[RectAlign::TOP_START])
        .gap(4.0)
        .show(|ui| {
            ui.set_width(rect.width().max(260.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            section_label(ui, "Worktrees");
            if repo.worktrees.is_empty() {
                menu_item(ui, None, &repo.name, None, true);
            }
            for worktree in &repo.worktrees {
                let branch = worktree
                    .branch
                    .clone()
                    .unwrap_or_else(|| format!("{} (detached)", worktree.head));
                let current = worktree.path == repo.root;
                if menu_item(ui, None, &worktree.name(), Some(&branch), current).clicked()
                    && !current
                {
                    cmds.push(Cmd::OpenRepo(worktree.path.clone()));
                }
            }
        });
}

fn branch_picker(ui: &mut Ui, rect: Rect, repo: &mut Repo, cmds: &mut Vec<Cmd>) {
    let mut detail = String::new();
    if repo.status.ahead > 0 {
        detail.push_str(&format!("↑{}", repo.status.ahead));
    }
    if repo.status.behind > 0 {
        if !detail.is_empty() {
            detail.push(' ');
        }
        detail.push_str(&format!("↓{}", repo.status.behind));
    }
    let label = repo.status.branch_label();
    let response = footer_row(ui, rect, "branch", icon::GIT_BRANCH, &label, &detail)
        .on_hover_text("Switch branch");
    if response.clicked() {
        repo.branch_filter.clear();
        // Make sure the list is current as it opens.
        cmds.push(Cmd::Refresh);
    }
    let idle = repo.busy.is_none();

    Popup::menu(&response)
        .frame(popup_frame())
        .align(RectAlign::TOP_START)
        // Stay put above the row even as the list changes size.
        .align_alternatives(&[RectAlign::TOP_START])
        .gap(4.0)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(rect.width().max(260.0));
            ui.spacing_mut().item_spacing.y = 0.0;

            let edit = Frame::new()
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.add(
                        TextEdit::singleline(&mut repo.branch_filter)
                            .align(egui::Align2::LEFT_CENTER)
                            .hint_text("Switch or create branch…")
                            .desired_width(f32::INFINITY)
                            .frame(Frame::new()),
                    )
                })
                .inner;
            // Typing should go straight to the filter while the picker is open.
            if ui.memory(|m| m.focused().is_none()) {
                edit.request_focus();
            }
            let line = ui.available_rect_before_wrap();
            ui.painter().hline(
                line.x_range(),
                line.top(),
                Stroke::new(1.0, theme::border_strong()),
            );
            ui.add_space(4.0);

            let filter = repo.branch_filter.trim().to_owned();
            let needle = filter.to_lowercase();
            let current = repo.status.branch.as_deref();
            let matches: Vec<&Branch> = repo
                .branches
                .iter()
                .filter(|b| {
                    needle.is_empty()
                        || b.name.to_lowercase().contains(&needle)
                        || b.remote
                            .as_ref()
                            .is_some_and(|r| r.to_lowercase().contains(&needle))
                })
                .collect();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let exact = repo
                .branches
                .iter()
                .any(|b| b.name == filter || b.remote.as_deref() == Some(filter.as_str()));
            let mut chosen: Option<Action> = None;

            ScrollArea::vertical()
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, branch) in matches.iter().enumerate() {
                        let is_current = branch.remote.is_none() && current == Some(&branch.name);
                        // Branches not checked out here yet say which remote has them.
                        let origin = branch
                            .remote
                            .as_deref()
                            .and_then(|r| r.strip_suffix(branch.name.as_str()))
                            .map(|r| r.trim_end_matches('/'));
                        let clicked =
                            menu_item(ui, Some(icon::GIT_BRANCH), &branch.name, origin, is_current)
                                .clicked();
                        // Enter picks the top match.
                        let via_enter = enter && i == 0 && !filter.is_empty();
                        if (clicked || via_enter) && !is_current && chosen.is_none() {
                            chosen = Some(match &branch.remote {
                                Some(remote) => Action::Track(remote.clone()),
                                None => Action::Switch(branch.name.clone()),
                            });
                        }
                    }
                });

            if !filter.is_empty() && !exact {
                let label = format!("Create branch “{filter}”");
                let clicked = menu_item(ui, Some(icon::PLUS), &label, None, false).clicked();
                if (clicked || (enter && matches.is_empty())) && chosen.is_none() {
                    chosen = Some(Action::CreateBranch(filter.clone()));
                }
            } else if matches.is_empty() {
                section_label(ui, "No branches");
            }

            if let Some(action) = chosen {
                if idle {
                    cmds.push(Cmd::Act(action));
                }
                repo.branch_filter.clear();
                ui.close();
            }
        });
}
