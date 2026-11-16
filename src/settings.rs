//! Everything the user can change, saved between runs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Accent {
    #[default]
    Indigo,
    Blue,
    Green,
    Orange,
    Pink,
}

impl Accent {
    pub const ALL: [Self; 5] = [
        Self::Indigo,
        Self::Blue,
        Self::Green,
        Self::Orange,
        Self::Pink,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Indigo => "Indigo",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Orange => "Orange",
            Self::Pink => "Pink",
        }
    }
}

pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 10.0..=20.0;
pub const ZOOM_LEVELS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
pub const TAB_SIZES: [u8; 3] = [2, 4, 8];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Most recently opened first.
    pub recent: Vec<PathBuf>,
    pub sidebar_open: bool,

    pub theme: ThemeChoice,
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

    /// Look for a new version on launch and every few hours.
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            sidebar_open: true,
            theme: ThemeChoice::System,
            accent: Accent::Indigo,
            zoom: 1.0,
            font_size: 12.5,
            line_wrap: false,
            line_numbers: true,
            tab_size: 4,
            insert_spaces: true,
            auto_save: false,
            auto_update: true,
        }
    }
}

impl Settings {
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
