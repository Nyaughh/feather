//! Colors, fonts and egui style. Everything visual lives here so the rest of
//! the app only talks in terms of named tokens.
//!
//! The palette is chosen at run time (dark or light, plus an accent), so the
//! tokens are functions rather than constants.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Vec2,
};
use std::cell::Cell;
use std::sync::Arc;

use crate::settings::{Accent, Settings, ThemeChoice};
use crate::themes;

// ── Palette ────────────────────────────────────────────────────────────────

/// Syntax token classes. Highlighting stores these, not colors, so a theme
/// change recolors open files without re-highlighting them.
pub mod syntax {
    pub const TEXT: u8 = 0;
    pub const COMMENT: u8 = 1;
    pub const PUNCTUATION: u8 = 2;
    pub const KEYWORD: u8 = 3;
    pub const STRING: u8 = 4;
    pub const NUMBER: u8 = 5;
    pub const ESCAPE: u8 = 6;
    pub const FUNCTION: u8 = 7;
    pub const TYPE: u8 = 8;
    pub const TAG: u8 = 9;
    pub const COUNT: usize = 10;
}

/// Every color the app draws with, plus the two metrics a theme can set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg_shell: Color32,
    pub bg_panel: Color32,
    pub bg_elevated: Color32,
    pub bg_input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub link: Color32,
    pub green: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub orange: Color32,
    pub syntax: [Color32; syntax::COUNT],
    pub code_size: f32,
    /// Corner radius of buttons, rows and inputs.
    pub radius: u8,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

thread_local! {
    // Only the UI thread reads colors.
    static CURRENT: Cell<Palette> = const { Cell::new(themes::FEATHER_DARK) };
    static STAMP: Cell<u64> = const { Cell::new(0) };
}

/// Changes whenever the palette or code font does; cached text layouts
/// compare it to know when they are stale.
pub fn stamp() -> u64 {
    STAMP.with(Cell::get)
}

fn palette() -> Palette {
    CURRENT.with(Cell::get)
}

/// The color of each accent choice; `None` means "whatever the theme uses".
pub fn accent_swatch(accent: Accent) -> Option<Color32> {
    Some(rgb(match accent {
        Accent::Theme => return None,
        Accent::Indigo => 0x5e6ad2,
        Accent::Blue => 0x2f80ed,
        Accent::Green => 0x2f9e6b,
        Accent::Orange => 0xd9772b,
        Accent::Pink => 0xd1548e,
    }))
}

/// The palette the settings ask for, given whether the system is in dark mode.
pub fn resolve(settings: &Settings, system_dark: bool) -> Palette {
    let dark = match settings.theme {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => system_dark,
    };
    let custom = settings
        .custom_themes
        .iter()
        .find(|t| t.id() == settings.theme_name)
        .and_then(|t| themes::import_css(&t.css).ok())
        .and_then(|t| t.pick(dark));
    let mut palette = custom.unwrap_or_else(|| {
        let presets = themes::presets();
        let preset = presets
            .iter()
            .find(|p| p.id == settings.theme_name)
            .unwrap_or(&presets[0]);
        if dark { preset.dark } else { preset.light }
    });
    if let Some(accent) = accent_swatch(settings.accent) {
        let pop = if palette.dark {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        palette.accent = accent;
        palette.accent_hover = themes::mix(accent, pop, 0.14);
    }
    palette.code_size = settings.font_size;
    palette
}

pub fn is_dark() -> bool {
    palette().dark
}

/// Window shell, behind the sidebar.
pub fn bg_shell() -> Color32 {
    palette().bg_shell
}
/// The inset content panel.
pub fn bg_panel() -> Color32 {
    palette().bg_panel
}
/// Popups, menus, modals.
pub fn bg_elevated() -> Color32 {
    palette().bg_elevated
}
/// Text inputs.
pub fn bg_input() -> Color32 {
    palette().bg_input
}
pub fn border() -> Color32 {
    palette().border
}
pub fn border_strong() -> Color32 {
    palette().border_strong
}
pub fn text() -> Color32 {
    palette().text
}
pub fn text_secondary() -> Color32 {
    palette().text_secondary
}
pub fn text_muted() -> Color32 {
    palette().text_muted
}
pub fn text_faint() -> Color32 {
    palette().text_faint
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn accent_hover() -> Color32 {
    palette().accent_hover
}
/// Text and icons drawn on top of the accent color.
pub fn on_accent() -> Color32 {
    if themes::luminance(palette().accent) > 0.45 {
        Color32::from_rgb(0x14, 0x14, 0x16)
    } else {
        Color32::WHITE
    }
}
pub fn green() -> Color32 {
    palette().green
}
pub fn red() -> Color32 {
    palette().red
}
pub fn yellow() -> Color32 {
    palette().yellow
}
pub fn blue() -> Color32 {
    palette().blue
}
pub fn orange() -> Color32 {
    palette().orange
}

/// The color for a syntax token class.
pub fn syntax_color(class: u8) -> Color32 {
    let p = palette();
    p.syntax.get(class as usize).copied().unwrap_or(p.syntax[0])
}

/// A translucent wash that reads on the current background: white over dark,
/// black over light. `alpha` is tuned for the dark theme.
pub fn overlay(alpha: u8) -> Color32 {
    if palette().dark {
        Color32::from_white_alpha(alpha)
    } else {
        Color32::from_black_alpha((u16::from(alpha) * 3 / 4) as u8)
    }
}

/// Row hover / selection washes.
pub fn hover_bg() -> Color32 {
    overlay(10)
}
pub fn selected_bg() -> Color32 {
    overlay(18)
}

pub fn diff_add_bg() -> Color32 {
    let c = green();
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), if is_dark() { 30 } else { 38 })
}
pub fn diff_del_bg() -> Color32 {
    let c = red();
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), if is_dark() { 30 } else { 34 })
}

// ── Metrics ────────────────────────────────────────────────────────────────

pub const ROW_HEIGHT: f32 = 28.0;
pub const HEADER_HEIGHT: f32 = 44.0;
pub const TITLEBAR_HEIGHT: f32 = 38.0;

/// Corner radius of buttons, rows and inputs, set by the theme.
pub fn radius() -> u8 {
    palette().radius
}
/// Corner radius of panels, popups and dialogs.
pub fn panel_radius() -> u8 {
    (palette().radius + 2).min(14)
}

pub fn code_size() -> f32 {
    palette().code_size
}
pub fn code_row_height() -> f32 {
    (code_size() * 1.6).round()
}

// ── Fonts ──────────────────────────────────────────────────────────────────

pub fn medium() -> FontFamily {
    FontFamily::Name("inter-medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("inter-semibold".into())
}
pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn ui_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn medium_font(size: f32) -> FontId {
    FontId::new(size, medium())
}
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, icons())
}
pub fn code_font() -> FontId {
    FontId::new(code_size(), FontFamily::Monospace)
}

/// Phosphor icon codepoints (regular weight).
pub mod icon {
    pub const CARET_RIGHT: &str = "\u{E13A}";
    pub const CARET_DOWN: &str = "\u{E136}";
    pub const CARET_UP_DOWN: &str = "\u{E140}";
    pub const FOLDER: &str = "\u{E24A}";
    pub const FOLDER_OPEN: &str = "\u{E256}";
    pub const FILE: &str = "\u{E230}";
    pub const FILE_TEXT: &str = "\u{E23A}";
    pub const FILE_CODE: &str = "\u{E914}";
    pub const FILE_IMAGE: &str = "\u{EA24}";
    pub const GIT_BRANCH: &str = "\u{E278}";
    pub const TREE_STRUCTURE: &str = "\u{E67C}";
    pub const PLUS: &str = "\u{E3D4}";
    pub const MINUS: &str = "\u{E32A}";
    pub const DISCARD: &str = "\u{E038}";
    pub const CHECK: &str = "\u{E182}";
    pub const FEATHER: &str = "\u{E9C0}";
    pub const ARROW_UP: &str = "\u{E08E}";
    pub const ARROW_DOWN: &str = "\u{E03E}";
    pub const REFRESH: &str = "\u{E094}";
    pub const WARNING: &str = "\u{E4E2}";
    pub const SIDEBAR: &str = "\u{EC24}";
    pub const GEAR: &str = "\u{E270}";
    pub const CLOSE: &str = "\u{E4F6}";
    pub const DOWNLOAD: &str = "\u{E20C}";
    pub const INFO: &str = "\u{E2CE}";
    pub const SEARCH: &str = "\u{E30C}";
    pub const TERMINAL: &str = "\u{EAE8}";
    pub const MORE: &str = "\u{E1FE}";
    pub const CLOUD: &str = "\u{E1AA}";
    pub const TAG: &str = "\u{E478}";
    pub const CLOCK: &str = "\u{E19A}";
    pub const COPY: &str = "\u{E1CA}";
    pub const GITHUB: &str = "\u{E576}";
    pub const USER: &str = "\u{E4C2}";
}

/// Loads the fonts. Call once, at startup.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add(
        "inter-medium",
        include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    );
    add(
        "inter-semibold",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add("phosphor", include_bytes!("../assets/fonts/Phosphor.ttf"));

    // egui's bundled fonts stay as fallbacks for emoji and unusual glyphs.
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let with = |first: &str| {
        let mut v = vec![first.to_owned(), "phosphor".to_owned()];
        v.extend(fallback.iter().cloned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with("inter"));
    fonts.families.insert(medium(), with("inter-medium"));
    fonts.families.insert(semibold(), with("inter-semibold"));
    fonts.families.insert(icons(), with("phosphor"));
    // Let code fall back to the UI fonts for glyphs the mono font lacks.
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        mono.push("inter".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Applies the user's theme, accent, zoom and font size. Cheap to call again;
/// returns whether anything visible changed.
pub fn apply(ctx: &egui::Context, settings: &Settings) -> bool {
    let zoom_changed = (ctx.zoom_factor() - settings.zoom).abs() > 0.001;
    if zoom_changed {
        ctx.set_zoom_factor(settings.zoom);
    }
    // Resolving a theme can mean parsing CSS, so only do it when an input changed.
    let system_dark = ctx.system_theme() != Some(egui::Theme::Light);
    let signature = egui::util::hash((
        &settings.theme_name,
        settings.theme,
        settings.accent,
        settings.font_size.to_bits(),
        system_dark,
        &settings.custom_themes,
    ));
    let styled = egui::Id::unique(STYLED);
    if ctx.data(|d| d.get_temp::<u64>(styled)) == Some(signature) && !zoom_changed {
        return false;
    }
    ctx.data_mut(|d| d.insert_temp(styled, signature));

    let next = resolve(settings, system_dark);
    CURRENT.with(|current| current.set(next));
    STAMP.with(|stamp| stamp.set(stamp.get() + 1));
    ctx.set_theme(if next.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.all_styles_mut(|style| style_egui(style, &next));
    true
}

const STYLED: &str = "feather_styled";

fn style_egui(style: &mut egui::Style, p: &Palette) {
    style.text_styles = [
        (TextStyle::Small, ui_font(11.5)),
        (TextStyle::Body, ui_font(13.0)),
        (TextStyle::Button, medium_font(13.0)),
        (TextStyle::Heading, FontId::new(22.0, semibold())),
        (
            TextStyle::Monospace,
            FontId::new(p.code_size, FontFamily::Monospace),
        ),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 4.0);
    s.button_padding = Vec2::new(10.0, 5.0);
    s.interact_size = Vec2::new(28.0, 28.0);
    s.menu_margin = Margin::same(4);
    s.window_margin = Margin::same(16);
    s.scroll.floating = true;
    s.scroll.bar_width = 8.0;
    s.scroll.floating_allocated_width = 0.0;
    s.scroll.foreground_color = false;

    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.4;
    style.animation_time = 0.08;

    let wash = |alpha: u8| {
        if p.dark {
            Color32::from_white_alpha(alpha)
        } else {
            Color32::from_black_alpha((u16::from(alpha) * 3 / 4) as u8)
        }
    };
    let strong = if p.dark {
        Color32::WHITE
    } else {
        Color32::BLACK
    };

    let v = &mut style.visuals;
    v.dark_mode = p.dark;
    v.override_text_color = None;
    v.panel_fill = p.bg_shell;
    v.window_fill = p.bg_elevated;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same((p.radius + 4).min(16));
    v.menu_corner_radius = CornerRadius::same((p.radius + 2).min(14));
    v.extreme_bg_color = p.bg_input;
    v.text_edit_bg_color = Some(p.bg_input);
    v.faint_bg_color = wash(6);
    v.code_bg_color = wash(14);
    v.hyperlink_color = p.link;
    v.warn_fg_color = p.yellow;
    v.error_fg_color = p.red;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(
        p.accent.r(),
        p.accent.g(),
        p.accent.b(),
        if p.dark { 90 } else { 60 },
    );
    v.selection.stroke = Stroke::new(1.0, p.accent);
    let shadow = |alpha: u8, blur: u8, y: i8| egui::Shadow {
        offset: [0, y],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { alpha } else { alpha / 3 }),
    };
    v.window_shadow = shadow(120, 28, 8);
    v.popup_shadow = shadow(110, 20, 6);

    let radius = CornerRadius::same(p.radius);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg_panel;
    w.noninteractive.weak_bg_fill = p.bg_panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_secondary);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = wash(14);
    w.inactive.weak_bg_fill = wash(10);
    w.inactive.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = wash(24);
    w.hovered.weak_bg_fill = wash(18);
    w.hovered.bg_stroke = Stroke::new(1.0, wash(40));
    w.hovered.fg_stroke = Stroke::new(1.0, strong);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = wash(30);
    w.active.weak_bg_fill = wash(24);
    w.active.bg_stroke = Stroke::new(1.0, wash(50));
    w.active.fg_stroke = Stroke::new(1.0, strong);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open = w.hovered;
}

/// The shadow under popups and floating cards.
pub fn popup_shadow() -> egui::Shadow {
    egui::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(if is_dark() { 110 } else { 40 }),
    }
}
