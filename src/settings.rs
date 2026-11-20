//! Everything the user can change, saved between runs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Accent {
    /// Whatever accent the chosen theme comes with.
    #[default]
    Theme,
    Indigo,
    Blue,
    Green,
    Orange,
    Pink,
}

impl Accent {
    pub const ALL: [Self; 6] = [
        Self::Theme,
        Self::Indigo,
        Self::Blue,
        Self::Green,
        Self::Orange,
        Self::Pink,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Theme => "The theme's own",
            Self::Indigo => "Indigo",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Orange => "Orange",
            Self::Pink => "Pink",
        }
    }
}

/// Where the terminal panel sits in the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TerminalDock {
    /// Under the content, across its full width.
    #[default]
    Bottom,
    /// Beside the content, down its full height.
    Right,
}

/// A theme the user imported, kept as the CSS it came from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    pub css: String,
}

impl CustomTheme {
    /// How settings refer to this theme, distinct from any built-in one.
    pub fn id(&self) -> String {
        format!("custom:{}", self.name)
    }
}

pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 10.0..=20.0;
pub const ZOOM_LEVELS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
pub const TAB_SIZES: [u8; 3] = [2, 4, 8];
/// How many recently opened folders are remembered.
pub const MAX_RECENT: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
    pub sidebar_open: bool,

    /// Light, dark, or follow the system.
    pub theme: ThemeChoice,
    /// Which theme: a built-in one's id, or a custom theme's.
    pub theme_name: String,
    pub custom_themes: Vec<CustomTheme>,
    pub accent: Accent,
    /// Scale of the whole interface.
    pub zoom: f32,

    /// Size of code text, in points.
    pub font_size: f32,
    pub line_wrap: bool,
    pub line_numbers: bool,
    pub tab_size: u8,
    /// Insert spaces when Tab is pressed, rather than a tab character.
    pub insert_spaces: bool,
    /// Save edits automatically a moment after typing stops.
    pub auto_save: bool,

    /// The program new terminals run. Empty means the system's usual shell.
    pub shell: String,
    /// Where the terminal panel sits.
    pub terminal_dock: TerminalDock,

    /// Fetch from the remote every few minutes, so new branches and commits
    /// show up without asking.
    pub auto_fetch: bool,

    /// Show who last changed the line the cursor is on.
    pub blame: bool,
    /// The commit list in the Git tab is expanded.
    pub history_open: bool,

    /// Look for a new version on launch and every few hours.
    pub auto_update: bool,

    /// How many one-time changes to saved settings have been made. See
    /// [`Settings::migrate`].
    pub revision: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            sidebar_open: true,
            theme: ThemeChoice::System,
            theme_name: crate::themes::DEFAULT_THEME.to_owned(),
            custom_themes: Vec::new(),
            accent: Accent::Theme,
            zoom: 1.0,
            font_size: 12.5,
            line_wrap: false,
            line_numbers: true,
            tab_size: 4,
            insert_spaces: true,
            auto_save: false,
            shell: String::new(),
            terminal_dock: TerminalDock::Bottom,
            auto_fetch: true,
            blame: true,
            history_open: false,
            auto_update: true,
            revision: 0,
        }
    }
}

impl Settings {
    /// Applies one-time corrections to settings saved by an older version.
    pub fn migrate(&mut self) {
        // 2: version 0.4.0 made a theme it called Clay the default and moved
        // everyone on Feather over to it. That was a mistake: Feather is the
        // default, and the other theme is now called Claude. Put them back.
        if self.revision < 2 && self.theme_name == "clay" {
            self.theme_name = crate::themes::DEFAULT_THEME.to_owned();
        }
        self.revision = 2;
    }

    /// Brings values loaded from disk back into their valid ranges.
    pub fn sanitize(&mut self) {
        self.font_size = self.font_size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
        if !ZOOM_LEVELS.contains(&self.zoom) {
            self.zoom = 1.0;
        }
        if !TAB_SIZES.contains(&self.tab_size) {
            self.tab_size = 4;
        }
    }
}
