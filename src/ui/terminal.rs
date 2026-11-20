//! The terminal panel: tabs along the top, and below them the active tab's
//! terminals, side by side when it is split, each drawn cell by cell.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Event, EventFilter, Id, Popup, Rect,
    RectAlign, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::widgets::{
    elided, icon_button, menu_item, menu_separator, paint_galley, paint_icon, paint_row_bg,
    popup_frame,
};
use crate::app::{Cmd, Repo, focus_pane_shortcut, split_shortcut};
use crate::settings::TerminalDock;
use crate::terminal::{Cell, Terminal, key_bytes};
use crate::theme::{self, icon, syntax};
use crate::themes::mix;

const TABS_HEIGHT: f32 = 34.0;
/// The strip naming each terminal of a split tab.
const PANE_HEADER_HEIGHT: f32 = 26.0;
/// Room kept for the split, menu and hide buttons at the end of the tab strip.
const TAB_BUTTONS_WIDTH: f32 = 94.0;

pub fn show(ui: &mut Ui, repo: &mut Repo, dock: TerminalDock, cmds: &mut Vec<Cmd>) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    tabs(ui, repo, dock, cmds);

    let body = ui.available_rect_before_wrap();
    let terminals = &mut repo.terminals;
    let grab_focus = std::mem::take(&mut terminals.grab_focus);
    let Some(group) = terminals.tabs.get_mut(terminals.active) else {
        terminals.focused = false;
        return;
    };

    let split = group.panes.len() > 1;
    let rects = pane_rects(body, group.panes.len(), dock);
    let mut focused_pane = None;
    for (i, (terminal, &rect)) in group.panes.iter_mut().zip(&rects).enumerate() {
        let active_pane = i == group.active;
        let mut screen_rect = rect;
        if split {
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), PANE_HEADER_HEIGHT));
            pane_header(ui, header, terminal, i, active_pane, cmds);
            screen_rect.min.y = header.bottom();
        }
        if screen(ui, screen_rect, terminal, grab_focus && active_pane) {
            focused_pane = Some(i);
        }
    }
    // Lines between the splits.
    let stroke = Stroke::new(1.0, theme::border());
    for rect in rects.iter().skip(1) {
        match dock {
            TerminalDock::Bottom => {
                ui.painter()
                    .vline(rect.left() - 0.5, body.y_range(), stroke);
            }
            TerminalDock::Right => {
                ui.painter().hline(body.x_range(), rect.top() - 0.5, stroke);
            }
        }
    }
    // Clicking into a split makes it the one that gets the keyboard back.
    if let Some(i) = focused_pane {
        group.active = i;
    }
    terminals.focused = focused_pane.is_some();
}

/// Divides `body` into `count` equal panes: side by side under the content,
/// one above the other when the panel is tall and narrow on the right.
fn pane_rects(body: Rect, count: usize, dock: TerminalDock) -> Vec<Rect> {
    let count = count.max(1);
    let n = count as f32;
    let gaps = n - 1.0;
    (0..count)
        .map(|i| {
            let i = i as f32;
            match dock {
                TerminalDock::Bottom => {
                    let w = (body.width() - gaps) / n;
                    Rect::from_min_size(
                        pos2(body.left() + i * (w + 1.0), body.top()),
                        vec2(w, body.height()),
                    )
                }
                TerminalDock::Right => {
                    let h = (body.height() - gaps) / n;
                    Rect::from_min_size(
                        pos2(body.left(), body.top() + i * (h + 1.0)),
                        vec2(body.width(), h),
                    )
                }
            }
        })
        .collect()
}

/// The name of one terminal in a split tab, with a button to close it.
fn pane_header(
    ui: &mut Ui,
    rect: Rect,
    terminal: &Terminal,
    index: usize,
    active: bool,
    cmds: &mut Vec<Cmd>,
) {
    let response = ui
        .interact(
            rect,
            Id::unique(("terminal_pane_header", terminal.id)),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    let hovered = ui.rect_contains_pointer(rect);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::border()),
    );
    let mid = rect.center().y;
    if active {
        // The split that has the keyboard, or gets it back.
        ui.painter()
            .circle_filled(pos2(rect.left() + 12.0, mid), 3.0, theme::accent());
    }
    let label = elided(
        ui,
        &terminal.title,
        theme::ui_font(12.0),
        if active {
            theme::text()
        } else {
            theme::text_muted()
        },
        rect.width() - 56.0,
    );
    paint_galley(ui, rect.left() + 22.0, mid, label);
    let close = Rect::from_center_size(pos2(rect.right() - 14.0, mid), vec2(20.0, 20.0));
    if (hovered || active)
        && icon_button(
            ui,
            close,
            Id::unique(("terminal_pane_close", terminal.id)),
            icon::CLOSE,
            "Close this split",
        )
        .clicked()
    {
        cmds.push(Cmd::ClosePane(index));
    } else if response.clicked() {
        cmds.push(Cmd::FocusPane(index));
    }
}

fn tabs(ui: &mut Ui, repo: &Repo, dock: TerminalDock, cmds: &mut Vec<Cmd>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TABS_HEIGHT), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::border()),
    );
    let mid = rect.center().y;
    let terminals = &repo.terminals;
    let mut x = rect.left() + 8.0;

    // The tabs share what the buttons leave, so a narrow panel shows them all.
    let room = rect.right() - TAB_BUTTONS_WIDTH - 36.0 - x;
    let count = terminals.tabs.len().max(1) as f32;
    let label_width = (room / count - 46.0).clamp(24.0, 140.0);

    for (i, group) in terminals.tabs.iter().enumerate() {
        let label = format!("{}: {}", i + 1, group.title());
        let text = elided(
            ui,
            &label,
            theme::ui_font(12.5),
            if i == terminals.active {
                theme::text()
            } else {
                theme::text_secondary()
            },
            label_width,
        );
        let tab = Rect::from_min_size(
            pos2(x, rect.top() + 5.0),
            vec2(text.size().x + 42.0, rect.height() - 10.0),
        );
        let response = ui
            .interact(tab, Id::unique(("terminal_tab", i)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let hovered = ui.rect_contains_pointer(tab);
        paint_row_bg(ui, tab, hovered, i == terminals.active);
        paint_galley(ui, tab.left() + 10.0, mid, text);
        if group.exited() {
            ui.painter()
                .circle_filled(pos2(tab.right() - 30.0, mid), 2.5, theme::text_muted());
        }
        let close = Rect::from_center_size(pos2(tab.right() - 14.0, mid), vec2(20.0, 20.0));
        let tip = if group.panes.len() > 1 {
            "Close this terminal and its splits"
        } else {
            "Close this terminal"
        };
        if (hovered || i == terminals.active)
            && icon_button(
                ui,
                close,
                Id::unique(("terminal_close", i)),
                icon::CLOSE,
                tip,
            )
            .clicked()
        {
            cmds.push(Cmd::CloseTerminal(i));
        } else if response.clicked() {
            cmds.push(Cmd::SelectTerminal(i));
        }
        x = tab.right() + 4.0;
    }

    let add = Rect::from_center_size(pos2(x + 14.0, mid), vec2(24.0, 24.0));
    if icon_button(
        ui,
        add,
        Id::unique("terminal_add"),
        icon::PLUS,
        "New terminal",
    )
    .clicked()
    {
        cmds.push(Cmd::NewTerminal);
    }

    // The right end: split, more options, hide.
    let slot = |n: f32| {
        Rect::from_center_size(pos2(rect.right() - 20.0 - n * 28.0, mid), vec2(24.0, 24.0))
    };
    let hide_icon = match dock {
        TerminalDock::Bottom => icon::CARET_DOWN,
        TerminalDock::Right => icon::CARET_RIGHT,
    };
    if icon_button(
        ui,
        slot(0.0),
        Id::unique("terminal_hide"),
        hide_icon,
        "Hide the terminal (it keeps running)",
    )
    .clicked()
    {
        cmds.push(Cmd::ToggleTerminal);
    }
    more_menu(ui, slot(1.0), repo, dock, cmds);
    let split_tip = format!(
        "Split the terminal ({})",
        ui.ctx().format_shortcut(&split_shortcut())
    );
    if icon_button(
        ui,
        slot(2.0),
        Id::unique("terminal_split"),
        split_icon(dock),
        &split_tip,
    )
    .clicked()
    {
        cmds.push(Cmd::SplitTerminal);
    }
}

/// Splits sit side by side under the content, one above the other on the right.
fn split_icon(dock: TerminalDock) -> &'static str {
    match dock {
        TerminalDock::Bottom => icon::SPLIT_COLUMNS,
        TerminalDock::Right => icon::SPLIT_ROWS,
    }
}

/// The "..." in the tab strip: splits, and where the panel sits.
fn more_menu(ui: &Ui, rect: Rect, repo: &Repo, dock: TerminalDock, cmds: &mut Vec<Cmd>) {
    let response = icon_button(
        ui,
        rect,
        Id::unique("terminal_more"),
        icon::MORE,
        "More terminal options",
    );
    let ctx = ui.ctx().clone();
    let terminals = &repo.terminals;
    let group = terminals.tabs.get(terminals.active);
    let panes = group.map_or(0, |g| g.panes.len());
    let active = group.map_or(0, |g| g.active);
    Popup::menu(&response)
        .frame(popup_frame())
        .align(RectAlign::BOTTOM_END)
        .align_alternatives(&[RectAlign::BOTTOM_END, RectAlign::TOP_END])
        .gap(4.0)
        .show(|ui| {
            ui.set_width(260.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let shortcut = ctx.format_shortcut(&split_shortcut());
            if menu_item(
                ui,
                Some(split_icon(dock)),
                "Split Terminal",
                Some(&shortcut),
                false,
            )
            .clicked()
            {
                cmds.push(Cmd::SplitTerminal);
            }
            if panes > 1 {
                let next = ctx.format_shortcut(&focus_pane_shortcut(egui::Key::ArrowRight));
                if menu_item(ui, None, "Focus Next Split", Some(&next), false).clicked() {
                    cmds.push(Cmd::FocusPane((active + 1) % panes));
                }
                let previous = ctx.format_shortcut(&focus_pane_shortcut(egui::Key::ArrowLeft));
                if menu_item(ui, None, "Focus Previous Split", Some(&previous), false).clicked() {
                    cmds.push(Cmd::FocusPane((active + panes - 1) % panes));
                }
                if menu_item(ui, Some(icon::CLOSE), "Close Split", None, false).clicked() {
                    cmds.push(Cmd::ClosePane(active));
                }
            }
            menu_separator(ui);
            for (choice, glyph, label) in [
                (TerminalDock::Bottom, icon::DOCK_BOTTOM, "Dock at Bottom"),
                (TerminalDock::Right, icon::DOCK_RIGHT, "Dock on Right"),
            ] {
                if menu_item(ui, Some(glyph), label, None, dock == choice).clicked()
                    && dock != choice
                {
                    cmds.push(Cmd::DockTerminal(choice));
                }
            }
        });
}

/// The sixteen basic terminal colors, taken from the theme.
fn basic_color(index: u8) -> Color32 {
    let bright = |c: Color32| mix(c, theme::text(), 0.25);
    match index {
        0 => mix(theme::bg_panel(), theme::text(), 0.2),
        1 => theme::red(),
        2 => theme::green(),
        3 => theme::yellow(),
        4 => theme::blue(),
        5 => theme::syntax_color(syntax::KEYWORD),
        6 => theme::syntax_color(syntax::ESCAPE),
        7 => theme::text_secondary(),
        8 => theme::text_muted(),
        9 => bright(theme::red()),
        10 => bright(theme::green()),
        11 => bright(theme::yellow()),
        12 => bright(theme::blue()),
        13 => bright(theme::syntax_color(syntax::KEYWORD)),
        14 => bright(theme::syntax_color(syntax::ESCAPE)),
        _ => theme::text(),
    }
}

/// A color from the 256-color palette.
fn indexed_color(index: u8) -> Color32 {
    match index {
        0..=15 => basic_color(index),
        // A 6x6x6 color cube.
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        // A ramp of greys.
        _ => {
            let v = 8 + (index - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

fn resolve(color: vt100::Color) -> Option<Color32> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(indexed_color(index)),
        vt100::Color::Rgb(r, g, b) => Some(Color32::from_rgb(r, g, b)),
    }
}

/// Whether `cell` falls inside a selection, which runs like text does: to the
/// end of the first row, whole rows in between, and into the last.
fn in_selection(selection: Option<(Cell, Cell)>, cell: Cell) -> bool {
    let Some((a, b)) = selection else {
        return false;
    };
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    cell >= start && cell <= end
}

/// Draws the terminal into `rect` and passes it the keyboard. Returns whether
/// it has focus.
fn screen(ui: &mut Ui, rect: Rect, terminal: &mut Terminal, grab_focus: bool) -> bool {
    let id = Id::unique(("terminal_screen", terminal.id));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.clicked() || response.drag_started() || grab_focus {
        ui.memory_mut(|m| m.request_focus(id));
    }
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        // Tab, arrows and Escape belong to the shell, not to moving focus.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    }

    // Fit the grid to the panel.
    let font = theme::code_font();
    let cell_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let cell_h = theme::code_row_height();
    let inner = rect.shrink2(vec2(12.0, 8.0));
    let cols = (inner.width() / cell_w).floor().clamp(8.0, 500.0) as u16;
    let rows = (inner.height() / cell_h).floor().clamp(2.0, 300.0) as u16;
    terminal.resize(rows, cols);

    let cell_at = |pos: egui::Pos2| -> Cell {
        let row = ((pos.y - inner.top()) / cell_h)
            .floor()
            .clamp(0.0, f32::from(rows - 1));
        let col = ((pos.x - inner.left()) / cell_w)
            .floor()
            .clamp(0.0, f32::from(cols - 1));
        (row as u16, col as u16)
    };

    // Mouse: drag selects, a plain click clears, the wheel scrolls history.
    if let Some(pos) = response.interact_pointer_pos() {
        if response.drag_started() {
            terminal.selection = Some((cell_at(pos), cell_at(pos)));
        } else if response.dragged()
            && let Some((start, _)) = terminal.selection
        {
            terminal.selection = Some((start, cell_at(pos)));
        }
    }
    if response.clicked() {
        terminal.selection = None;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() >= 1.0 {
            let lines = (scroll / cell_h).round();
            let lines = if lines == 0.0 { scroll.signum() } else { lines };
            terminal.scroll(lines as i32);
        }
    }

    if focused {
        keyboard(ui, terminal);
    }

    // Paint.
    let painter = ui.painter_at(rect);
    let screen = terminal.screen();
    let default_fg = theme::syntax_color(syntax::TEXT);
    let selection_bg = theme::accent().gamma_multiply(0.45);
    let mut run = String::new();
    for row in 0..rows {
        let y = inner.top() + f32::from(row) * cell_h;
        // Group neighbouring cells that look the same into one piece of text.
        let mut run_start = 0u16;
        let mut run_color = default_fg;
        let flush = |run: &mut String, start: u16, color: Color32| {
            if !run.trim_end().is_empty() {
                painter.text(
                    pos2(inner.left() + f32::from(start) * cell_w, y + cell_h / 2.0),
                    Align2::LEFT_CENTER,
                    run.as_str(),
                    font.clone(),
                    color,
                );
            }
            run.clear();
        };
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (resolve(cell.fgcolor()), resolve(cell.bgcolor()));
            if cell.inverse() {
                (fg, bg) = (
                    Some(bg.unwrap_or(theme::bg_panel())),
                    Some(fg.unwrap_or(default_fg)),
                );
            }
            let mut fg = fg.unwrap_or(default_fg);
            if cell.dim() {
                fg = mix(fg, theme::bg_panel(), 0.4);
            }
            if in_selection(terminal.selection, (row, col)) {
                bg = Some(selection_bg);
            }
            let x = inner.left() + f32::from(col) * cell_w;
            if let Some(bg) = bg {
                let width = if cell.is_wide() { cell_w * 2.0 } else { cell_w };
                painter.rect_filled(
                    Rect::from_min_size(pos2(x, y), vec2(width, cell_h)),
                    CornerRadius::ZERO,
                    bg,
                );
            }
            if cell.underline() && cell.has_contents() {
                painter.hline(x..=x + cell_w, y + cell_h - 2.0, Stroke::new(1.0, fg));
            }
            // Start a new piece of text when the color changes. Wide characters
            // always get their own, so they cannot push later cells out of line.
            if fg != run_color || cell.is_wide() {
                flush(&mut run, run_start, run_color);
                run_start = col;
                run_color = fg;
            }
            if run.is_empty() {
                run_start = col;
            }
            let contents = cell.contents();
            run.push_str(if contents.is_empty() { " " } else { contents });
            if cell.is_wide() {
                flush(&mut run, run_start, run_color);
            }
        }
        flush(&mut run, run_start, run_color);
    }

    // The cursor: solid when the terminal has the keyboard, hollow otherwise.
    if !screen.hide_cursor() && screen.scrollback() == 0 && !terminal.exited {
        let (row, col) = screen.cursor_position();
        let cursor = Rect::from_min_size(
            pos2(
                inner.left() + f32::from(col) * cell_w,
                inner.top() + f32::from(row) * cell_h,
            ),
            vec2(cell_w, cell_h),
        )
        .shrink2(vec2(0.0, 1.5));
        if focused {
            painter.rect_filled(
                cursor,
                CornerRadius::same(1),
                theme::text().gamma_multiply(0.75),
            );
            if let Some(cell) = screen.cell(row, col).filter(|c| c.has_contents()) {
                painter.text(
                    cursor.left_center(),
                    Align2::LEFT_CENTER,
                    cell.contents(),
                    font.clone(),
                    theme::bg_panel(),
                );
            }
        } else {
            painter.rect_stroke(
                cursor,
                CornerRadius::same(1),
                Stroke::new(1.0, theme::text_muted()),
                StrokeKind::Inside,
            );
        }
    }

    if terminal.exited {
        let note = "This terminal's program has ended. Close it or open a new one.";
        let galley =
            painter.layout_no_wrap(note.to_owned(), theme::ui_font(12.0), theme::text_muted());
        let pos = pos2(inner.left(), rect.bottom() - galley.size().y - 8.0);
        painter.rect_filled(
            Rect::from_min_size(pos, galley.size()).expand(3.0),
            CornerRadius::same(3),
            theme::bg_panel(),
        );
        painter.galley(pos, galley, theme::text_muted());
    } else if screen.scrollback() > 0 {
        paint_icon(
            ui,
            pos2(rect.right() - 18.0, rect.bottom() - 16.0),
            icon::ARROW_DOWN,
            12.0,
            theme::text_muted(),
        );
    }
    focused
}

/// Passes this frame's key presses, typed text and pastes to the shell.
fn keyboard(ui: &Ui, terminal: &mut Terminal) {
    let events = ui.input(|i| i.events.clone());
    let application_cursor = terminal.screen().application_cursor();
    let bracketed_paste = terminal.screen().bracketed_paste();
    // On macOS copy and paste are Cmd+C/V, so Ctrl+C is free to mean "interrupt".
    let ctrl_is_command = !cfg!(target_os = "macos");
    // Ctrl+C, X and V arrive as these events; ignore the key press that came with them.
    let clipboard_event = events
        .iter()
        .any(|e| matches!(e, Event::Copy | Event::Cut | Event::Paste(_)));

    for event in events {
        match event {
            Event::Text(text) => terminal.send(text.as_bytes()),
            Event::Paste(text) => {
                let text = text.replace("\r\n", "\r").replace('\n', "\r");
                if bracketed_paste {
                    terminal.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
                } else {
                    terminal.send(text.as_bytes());
                }
            }
            // With text selected this copies it, as in other terminals.
            // Otherwise Ctrl+C interrupts whatever is running.
            Event::Copy => match terminal.selected_text() {
                Some(text) => {
                    ui.ctx().copy_text(text);
                    terminal.selection = None;
                }
                None if ctrl_is_command => terminal.send(&[0x03]),
                None => {}
            },
            Event::Cut if ctrl_is_command => terminal.send(&[0x18]),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let clipboard_key = matches!(key, egui::Key::C | egui::Key::X | egui::Key::V);
                if clipboard_event && clipboard_key && modifiers.command {
                    continue;
                }
                // Cmd shortcuts on macOS are the app's, not the shell's.
                if modifiers.mac_cmd {
                    continue;
                }
                if let Some(bytes) = key_bytes(key, modifiers, application_cursor) {
                    terminal.send(&bytes);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_run_like_text() {
        let selection = Some(((1, 5), (3, 2)));
        assert!(!in_selection(selection, (1, 4)));
        assert!(in_selection(selection, (1, 5)));
        assert!(in_selection(selection, (2, 0)));
        assert!(in_selection(selection, (3, 2)));
        assert!(!in_selection(selection, (3, 3)));
        // Dragging backwards selects the same cells.
        assert!(in_selection(Some(((3, 2), (1, 5))), (2, 79)));
        assert!(!in_selection(None, (0, 0)));
    }

    #[test]
    fn splits_side_by_side_at_the_bottom_and_stacked_on_the_right() {
        let body = Rect::from_min_size(pos2(0.0, 0.0), vec2(601.0, 301.0));
        let columns = pane_rects(body, 2, TerminalDock::Bottom);
        assert_eq!(
            columns[0],
            Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 301.0))
        );
        assert_eq!(
            columns[1],
            Rect::from_min_size(pos2(301.0, 0.0), vec2(300.0, 301.0))
        );
        let rows = pane_rects(body, 2, TerminalDock::Right);
        assert_eq!(
            rows[0],
            Rect::from_min_size(pos2(0.0, 0.0), vec2(601.0, 150.0))
        );
        assert_eq!(
            rows[1],
            Rect::from_min_size(pos2(0.0, 151.0), vec2(601.0, 150.0))
        );
        assert_eq!(pane_rects(body, 1, TerminalDock::Right), [body]);
    }

    #[test]
    fn maps_the_256_color_palette() {
        assert_eq!(indexed_color(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed_color(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(indexed_color(196), Color32::from_rgb(255, 0, 0));
        assert_eq!(indexed_color(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(indexed_color(255), Color32::from_rgb(238, 238, 238));
    }
}
