//! Integrated terminals: a shell running in a pseudo-terminal, and the
//! screen state its output adds up to.
//!
//! Each terminal has a thread reading the shell's output and one waiting for
//! it to exit; both hand their news to the UI through a channel.

use eframe::egui;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

/// Lines kept above the visible screen.
const SCROLLBACK: usize = 5000;

enum Message {
    Output(Vec<u8>),
    Exited,
}

/// Collects the answers to questions the running program asks the terminal.
///
/// Programs can ask where the cursor is or what kind of terminal this is, and
/// then wait for the reply. Windows does exactly that when a console starts:
/// until it is told the cursor position it shows nothing and accepts nothing.
#[derive(Default)]
struct Replies {
    pending: Vec<u8>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        prefix: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        action: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (prefix, action, first) {
            // "Report cursor position" (and its DEC variant).
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            (Some(b'?'), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[?{};{}R", row + 1, col + 1)
            }
            // "Are you there?"
            (None, 'n', 5) => "\x1b[0n".to_owned(),
            // "What are you?": a VT100 with the advanced video option.
            (None, 'c', 0) => "\x1b[?1;2c".to_owned(),
            (Some(b'>'), 'c', 0) => "\x1b[>0;10;1c".to_owned(),
            _ => return,
        };
        self.pending.extend_from_slice(reply.as_bytes());
    }
}

/// A cell position on the visible screen: (row, column).
pub type Cell = (u16, u16);

pub struct Terminal {
    /// Short name for its tab, e.g. `bash`.
    pub title: String,
    parser: vt100::Parser<Replies>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    messages: Receiver<Message>,
    pub exited: bool,
    size: (u16, u16),
    /// Text selected with the mouse: where the drag began and where it is now.
    pub selection: Option<(Cell, Cell)>,
}

/// The program a new terminal runs: the user's choice, or the usual shell.
fn shell_command(custom: &str) -> CommandBuilder {
    let custom = custom.trim();
    if !custom.is_empty() {
        let mut parts = custom.split_whitespace();
        let mut cmd = CommandBuilder::new(parts.next().unwrap_or(custom));
        cmd.args(parts);
        return cmd;
    }
    if cfg!(windows) {
        let mut cmd = CommandBuilder::new("powershell.exe");
        cmd.arg("-NoLogo");
        cmd
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let mut cmd = CommandBuilder::new(shell);
        // A login shell, so PATH is what the user's own terminal would have
        // even when Feather was started from a desktop icon.
        cmd.arg("-l");
        cmd
    }
}

impl Terminal {
    /// Starts a shell in `cwd`. `shell` overrides the default program.
    pub fn spawn(cwd: &Path, shell: &str, ctx: &egui::Context) -> Result<Self, String> {
        let (rows, cols) = (24, 80);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Could not open a terminal: {e}"))?;

        let mut cmd = shell_command(shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let title = cmd
            .get_argv()
            .first()
            .map(|program| {
                Path::new(program)
                    .file_stem()
                    .unwrap_or(program)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "shell".to_owned());

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Could not start {title}: {e}"))?;
        // Only the child should hold the other end, or its exit goes unnoticed.
        drop(pair.slave);

        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let (tx, messages) = mpsc::channel();
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(Message::Output(buffer[..n].to_vec())).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }
        {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                let _ = tx.send(Message::Exited);
                ctx.request_repaint();
            });
        }

        Ok(Self {
            title,
            parser: vt100::Parser::new_with_callbacks(
                nz(rows),
                nz(cols),
                SCROLLBACK,
                Replies::default(),
            ),
            writer,
            master: pair.master,
            killer,
            messages,
            exited: false,
            size: (rows, cols),
            selection: None,
        })
    }

    /// Feeds any output that has arrived into the screen.
    pub fn pump(&mut self) {
        while let Ok(message) = self.messages.try_recv() {
            match message {
                Message::Output(bytes) => {
                    self.parser.process(&bytes);
                    // Answer anything the program asked while printing that.
                    let replies = std::mem::take(&mut self.parser.callbacks_mut().pending);
                    if !replies.is_empty() {
                        let _ = self.writer.write_all(&replies);
                        let _ = self.writer.flush();
                    }
                }
                Message::Exited => self.exited = true,
            }
        }
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Sends keystrokes (or pasted text) to the shell.
    pub fn send(&mut self, bytes: &[u8]) {
        if self.exited || bytes.is_empty() {
            return;
        }
        // Typing always returns to the live screen.
        self.parser.screen_mut().set_scrollback(0);
        self.selection = None;
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Tells the shell how many rows and columns it has to draw in.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(8));
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        self.parser.set_size(nz(rows), nz(cols));
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    #[cfg(test)]
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Scrolls the view by `lines` (positive is back into history).
    pub fn scroll(&mut self, lines: i32) {
        let screen = self.parser.screen_mut();
        let current = screen.scrollback() as i32;
        screen.set_scrollback((current + lines).max(0) as usize);
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let text = self
            .screen()
            .contents_between(start.0, start.1, end.0, end.1 + 1);
        (!text.trim().is_empty()).then_some(text)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Closing a terminal ends whatever is running in it.
        let _ = self.killer.kill();
    }
}

fn nz(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value.max(1)).unwrap_or(NonZeroU16::MIN)
}

/// The terminals belonging to one repository, shown as tabs in a panel.
#[derive(Default)]
pub struct Terminals {
    pub list: Vec<Terminal>,
    pub active: usize,
    pub open: bool,
    /// Whether the terminal had the keyboard on the last frame.
    pub focused: bool,
    /// Give the terminal the keyboard on the next frame.
    pub grab_focus: bool,
}

impl Terminals {
    /// Opens a new terminal tab and shows the panel.
    pub fn add(&mut self, cwd: &Path, shell: &str, ctx: &egui::Context) -> Result<(), String> {
        let terminal = Terminal::spawn(cwd, shell, ctx)?;
        self.list.push(terminal);
        self.active = self.list.len() - 1;
        self.open = true;
        Ok(())
    }

    pub fn close(&mut self, index: usize) {
        if index < self.list.len() {
            self.list.remove(index);
            self.active = self.active.min(self.list.len().saturating_sub(1));
        }
        if self.list.is_empty() {
            self.open = false;
        }
    }

    pub fn pump(&mut self) {
        for terminal in &mut self.list {
            terminal.pump();
        }
    }
}

/// The bytes a key press sends to the shell, if it sends any.
pub fn key_bytes(
    key: egui::Key,
    modifiers: egui::Modifiers,
    application_cursor: bool,
) -> Option<Vec<u8>> {
    use egui::Key;

    // Ctrl+letter is the matching control character.
    if modifiers.ctrl && !modifiers.alt {
        let letter = key.name();
        if let [byte @ b'A'..=b'Z'] = letter.as_bytes() {
            return Some(vec![byte - b'A' + 1]);
        }
        match key {
            Key::OpenBracket => return Some(vec![0x1b]),
            Key::Backslash => return Some(vec![0x1c]),
            Key::CloseBracket => return Some(vec![0x1d]),
            Key::Space => return Some(vec![0]),
            _ => {}
        }
    }
    let arrow = |letter: u8| {
        if application_cursor {
            vec![0x1b, b'O', letter]
        } else {
            vec![0x1b, b'[', letter]
        }
    };
    let sequence: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Tab if modifiers.shift => b"\x1b[Z",
        Key::Tab => b"\t",
        Key::Backspace if modifiers.ctrl || modifiers.alt => b"\x17",
        Key::Backspace => b"\x7f",
        Key::Escape => b"\x1b",
        Key::ArrowUp => return Some(arrow(b'A')),
        Key::ArrowDown => return Some(arrow(b'B')),
        Key::ArrowRight if modifiers.ctrl || modifiers.alt => b"\x1bf",
        Key::ArrowLeft if modifiers.ctrl || modifiers.alt => b"\x1bb",
        Key::ArrowRight => return Some(arrow(b'C')),
        Key::ArrowLeft => return Some(arrow(b'D')),
        Key::Home => return Some(arrow(b'H')),
        Key::End => return Some(arrow(b'F')),
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        Key::F1 => b"\x1bOP",
        Key::F2 => b"\x1bOQ",
        Key::F3 => b"\x1bOR",
        Key::F4 => b"\x1bOS",
        Key::F5 => b"\x1b[15~",
        Key::F6 => b"\x1b[17~",
        Key::F7 => b"\x1b[18~",
        Key::F8 => b"\x1b[19~",
        Key::F9 => b"\x1b[20~",
        Key::F10 => b"\x1b[21~",
        Key::F11 => b"\x1b[23~",
        Key::F12 => b"\x1b[24~",
        _ => return None,
    };
    Some(sequence.to_vec())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(terminal: &mut Terminal, what: impl Fn(&Terminal) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            terminal.pump();
            if what(terminal) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn runs_a_shell_and_shows_its_output() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let mut terminal = Terminal::spawn(&dir, "/bin/sh", &ctx).expect("spawn sh");
        assert_eq!(terminal.title, "sh");
        terminal.resize(10, 40);
        assert_eq!(terminal.size(), (10, 40));

        // A command's output lands on the screen, colors and all.
        terminal.send(b"printf 'fea%s \\033[31mred\\033[0m\\n' ther\r");
        assert!(wait_for(&mut terminal, |t| t
            .screen()
            .contents()
            .contains("feather red")));
        let screen = terminal.screen();
        // The shell's prompt may share the row, so find where the text starts.
        let rows: Vec<String> = screen.rows(0, 40).collect();
        let (row, col) = rows
            .iter()
            .enumerate()
            .find_map(|(r, text)| {
                let col = text.find("feather red")?;
                (!text.contains("printf")).then_some((r as u16, col as u16))
            })
            .unwrap_or_else(|| panic!("no output row in {rows:?}"));
        assert_eq!(
            screen.cell(row, col + 8).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(
            screen.cell(row, col).unwrap().fgcolor(),
            vt100::Color::Default
        );

        // It runs in the folder it was given.
        terminal.send(b"pwd\r");
        let here = dir.canonicalize().unwrap().display().to_string();
        assert!(wait_for(&mut terminal, |t| t
            .screen()
            .contents()
            .contains(&here)));

        // Selecting text on that row reads it back.
        terminal.selection = Some(((row, col), (row, col + 6)));
        assert_eq!(terminal.selected_text().as_deref(), Some("feather"));

        terminal.send(b"exit\r");
        assert!(wait_for(&mut terminal, |t| t.exited));
    }

    #[test]
    fn answers_when_a_program_asks_where_the_cursor_is() {
        // Windows asks this before it will show a prompt; here a script does.
        let ctx = egui::Context::default();
        let mut terminal =
            Terminal::spawn(&std::env::temp_dir(), "/bin/bash --norc --noprofile", &ctx)
                .expect("spawn bash");
        terminal.send(
            b"clear; printf 'abc\\033[6n'; IFS='[;' read -rs -d R _ row col; echo \" at=$row,$col\"\r",
        );
        assert!(
            wait_for(&mut terminal, |t| t
                .screen()
                .contents()
                .contains("abc at=1,4")),
            "screen was: {:?}",
            terminal.screen().contents()
        );
    }

    #[test]
    fn translates_keys() {
        use egui::{Key, Modifiers};
        let none = Modifiers::NONE;
        assert_eq!(key_bytes(Key::Enter, none, false).unwrap(), b"\r");
        assert_eq!(key_bytes(Key::C, Modifiers::CTRL, false).unwrap(), [3]);
        assert_eq!(key_bytes(Key::D, Modifiers::CTRL, false).unwrap(), [4]);
        assert_eq!(key_bytes(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes(Key::Backspace, none, false).unwrap(), b"\x7f");
        assert_eq!(
            key_bytes(Key::A, none, false),
            None,
            "plain letters arrive as text"
        );
    }
}
