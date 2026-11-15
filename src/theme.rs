//! Colors, fonts and egui style. Everything visual lives here so the rest of
//! the app only talks in terms of named tokens.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Vec2,
};
use std::sync::Arc;

// ── Palette ────────────────────────────────────────────────────────────────

/// Window shell, behind the sidebar.
pub const BG_SHELL: Color32 = Color32::from_rgb(0x09, 0x09, 0x0a);
/// The inset content panel.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x10, 0x11, 0x12);
/// Popups, menus, modals.
pub const BG_ELEVATED: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x1d);
/// Text inputs.
pub const BG_INPUT: Color32 = Color32::from_rgb(0x14, 0x15, 0x17);

pub const BORDER: Color32 = Color32::from_rgb(0x1f, 0x20, 0x23);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x2c, 0x2d, 0x31);

pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xea, 0xec);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9a, 0x9d, 0xa5);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x62, 0x66, 0x6d);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x45, 0x48, 0x4e);

pub const ACCENT: Color32 = Color32::from_rgb(0x5e, 0x6a, 0xd2);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x6c, 0x78, 0xe6);

pub const GREEN: Color32 = Color32::from_rgb(0x4c, 0xb7, 0x82);
pub const RED: Color32 = Color32::from_rgb(0xeb, 0x57, 0x57);
pub const YELLOW: Color32 = Color32::from_rgb(0xe2, 0xb9, 0x4a);
pub const BLUE: Color32 = Color32::from_rgb(0x4e, 0xa7, 0xfc);
pub const ORANGE: Color32 = Color32::from_rgb(0xf2, 0x99, 0x4a);

/// Row hover / selection washes, as white overlays so they work on any surface.
pub fn hover_bg() -> Color32 {
    Color32::from_white_alpha(10)
}
pub fn selected_bg() -> Color32 {
    Color32::from_white_alpha(18)
}

pub fn diff_add_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(0x4c, 0xb7, 0x82, 30)
}
pub fn diff_del_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(0xeb, 0x57, 0x57, 30)
}

// ── Metrics ────────────────────────────────────────────────────────────────

pub const ROW_HEIGHT: f32 = 28.0;
pub const HEADER_HEIGHT: f32 = 44.0;
pub const RADIUS: u8 = 6;
pub const CODE_SIZE: f32 = 12.5;
pub const CODE_ROW_HEIGHT: f32 = 20.0;

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
    FontId::new(CODE_SIZE, FontFamily::Monospace)
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
}

fn install_fonts(ctx: &egui::Context) {
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

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    // Feather is dark-only for now; don't follow a light system theme into
    // egui's stock light style.
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, ui_font(11.5)),
            (TextStyle::Body, ui_font(13.0)),
            (TextStyle::Button, medium_font(13.0)),
            (TextStyle::Heading, FontId::new(22.0, semibold())),
            (TextStyle::Monospace, code_font()),
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

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG_SHELL;
        v.window_fill = BG_ELEVATED;
        v.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = BG_INPUT;
        v.text_edit_bg_color = Some(BG_INPUT);
        v.faint_bg_color = Color32::from_white_alpha(6);
        v.code_bg_color = Color32::from_white_alpha(14);
        v.hyperlink_color = Color32::from_rgb(0x82, 0x8f, 0xff);
        v.warn_fg_color = YELLOW;
        v.error_fg_color = RED;
        v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x5e, 0x6a, 0xd2, 90);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_shadow = egui::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.popup_shadow = egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };

        let radius = CornerRadius::same(RADIUS);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG_PANEL;
        w.noninteractive.weak_bg_fill = BG_PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_SECONDARY);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = Color32::from_white_alpha(14);
        w.inactive.weak_bg_fill = Color32::from_white_alpha(10);
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = radius;
        w.inactive.expansion = 0.0;

        w.hovered.bg_fill = Color32::from_white_alpha(24);
        w.hovered.weak_bg_fill = Color32::from_white_alpha(18);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(40));
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = Color32::from_white_alpha(30);
        w.active.weak_bg_fill = Color32::from_white_alpha(24);
        w.active.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(50));
        w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open = w.hovered;
    });
}
