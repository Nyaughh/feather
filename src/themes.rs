//! The themes Feather ships with, and importing new ones.
//!
//! A theme is a pair of palettes, light and dark. Each is described by a small
//! set of seed colors; everything else (hover shades, syntax colors, the
//! terminal's colors) is derived from those, so a theme restyles the whole app.
//!
//! Themes can also be imported from the CSS that <https://tweakcn.com> (and
//! shadcn/ui in general) produces: a `:root { --background: ...; }` block for
//! light and a `.dark { ... }` block for dark.

use eframe::egui::Color32;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::theme::{Palette, syntax};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Blends `t` of the way from `a` to `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// Relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// How far a color is from gray, from 0 to 1.
fn saturation(c: Color32) -> f32 {
    let max = c.r().max(c.g()).max(c.b());
    let min = c.r().min(c.g()).min(c.b());
    if max == 0 {
        0.0
    } else {
        f32::from(max - min) / f32::from(max)
    }
}

pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Nudges `color` toward `toward` until it can be read against `background`.
fn legible(color: Color32, background: Color32, toward: Color32) -> Color32 {
    let mut out = color;
    for step in 1..=8 {
        if contrast(out, background) >= 3.6 {
            break;
        }
        out = mix(color, toward, step as f32 / 8.0);
    }
    out
}

/// The handful of colors a theme is defined by.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub shell: Color32,
    pub panel: Color32,
    pub elevated: Color32,
    pub input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub green: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub orange: Color32,
    pub purple: Color32,
    pub teal: Color32,
    pub radius: u8,
}

impl Seed {
    pub fn palette(&self) -> Palette {
        let dark = luminance(self.panel) < 0.4;
        let pop = if dark { Color32::WHITE } else { Color32::BLACK };
        Palette {
            dark,
            bg_shell: self.shell,
            bg_panel: self.panel,
            bg_elevated: self.elevated,
            bg_input: self.input,
            border: self.border,
            border_strong: self.border_strong,
            text: self.text,
            text_secondary: self.text_secondary,
            text_muted: self.text_muted,
            text_faint: self.text_faint,
            accent: self.accent,
            accent_hover: mix(self.accent, pop, 0.14),
            link: legible(self.accent, self.panel, self.text),
            green: self.green,
            red: self.red,
            yellow: self.yellow,
            blue: self.blue,
            orange: self.orange,
            syntax: {
                let mut colors = [mix(self.text, self.panel, 0.08); syntax::COUNT];
                colors[syntax::COMMENT as usize] = self.text_muted;
                colors[syntax::PUNCTUATION as usize] = self.text_secondary;
                colors[syntax::KEYWORD as usize] = self.purple;
                colors[syntax::STRING as usize] = self.green;
                colors[syntax::NUMBER as usize] = self.orange;
                colors[syntax::ESCAPE as usize] = self.teal;
                colors[syntax::FUNCTION as usize] = self.blue;
                colors[syntax::TYPE as usize] = self.yellow;
                colors[syntax::TAG as usize] = self.red;
                colors
            },
            code_size: 12.5,
            radius: self.radius,
        }
    }
}

/// The id of the theme a new install uses.
pub const DEFAULT_THEME: &str = "feather";

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: Palette,
    pub light: Palette,
}

/// Feather's own look. These two are spelled out in full rather than derived.
pub const FEATHER_DARK: Palette = Palette {
    dark: true,
    bg_shell: rgb(0x09090a),
    bg_panel: rgb(0x101112),
    bg_elevated: rgb(0x1a1b1d),
    bg_input: rgb(0x141517),
    border: rgb(0x1f2023),
    border_strong: rgb(0x2c2d31),
    text: rgb(0xe9eaec),
    text_secondary: rgb(0x9a9da5),
    text_muted: rgb(0x62666d),
    text_faint: rgb(0x45484e),
    // The brand's golden-hour blush (see README, "Brand").
    accent: rgb(0xefbfa8),
    accent_hover: rgb(0xf1c8b4),
    link: rgb(0xedb096),
    green: rgb(0x4cb782),
    red: rgb(0xeb5757),
    yellow: rgb(0xe2b94a),
    blue: rgb(0x4ea7fc),
    orange: rgb(0xf2994a),
    syntax: [
        rgb(0xd6d8dc), // text
        rgb(0x6b6f77), // comment
        rgb(0x9a9da5), // punctuation
        rgb(0xa79df2), // keyword
        rgb(0x8cc8a0), // string
        rgb(0xe0a36a), // number
        rgb(0x76cfc4), // escape
        rgb(0x7db3f5), // function
        rgb(0xe2c07c), // type
        rgb(0xe8868b), // tag
    ],
    code_size: 12.5,
    radius: 6,
};

pub const FEATHER_LIGHT: Palette = Palette {
    dark: false,
    bg_shell: rgb(0xf3f4f6),
    bg_panel: rgb(0xffffff),
    bg_elevated: rgb(0xffffff),
    bg_input: rgb(0xffffff),
    border: rgb(0xe4e5e9),
    border_strong: rgb(0xd3d5db),
    text: rgb(0x1b1c20),
    text_secondary: rgb(0x55585f),
    text_muted: rgb(0x858890),
    text_faint: rgb(0xb3b6bd),
    // The blush is too pale for white, so light mode uses the terracotta it
    // deepens to.
    accent: rgb(0xa9563f),
    accent_hover: rgb(0x914a36),
    link: rgb(0xa04f3a),
    green: rgb(0x23915b),
    red: rgb(0xd13b3b),
    yellow: rgb(0xa9790a),
    blue: rgb(0x2676d0),
    orange: rgb(0xc86f1e),
    syntax: [
        rgb(0x24262b), // text
        rgb(0x8b8e96), // comment
        rgb(0x5d6068), // punctuation
        rgb(0x6a4fd0), // keyword
        rgb(0x2a7d4c), // string
        rgb(0xb05e14), // number
        rgb(0x0e8073), // escape
        rgb(0x1f61c4), // function
        rgb(0x946500), // type
        rgb(0xc0323a), // tag
    ],
    code_size: 12.5,
    radius: 6,
};

#[allow(clippy::too_many_arguments)]
const fn seed(
    surfaces: [u32; 4],
    borders: [u32; 2],
    texts: [u32; 4],
    accent: u32,
    hues: [u32; 7],
) -> Seed {
    Seed {
        shell: rgb(surfaces[0]),
        panel: rgb(surfaces[1]),
        elevated: rgb(surfaces[2]),
        input: rgb(surfaces[3]),
        border: rgb(borders[0]),
        border_strong: rgb(borders[1]),
        text: rgb(texts[0]),
        text_secondary: rgb(texts[1]),
        text_muted: rgb(texts[2]),
        text_faint: rgb(texts[3]),
        accent: rgb(accent),
        green: rgb(hues[0]),
        red: rgb(hues[1]),
        yellow: rgb(hues[2]),
        blue: rgb(hues[3]),
        orange: rgb(hues[4]),
        purple: rgb(hues[5]),
        teal: rgb(hues[6]),
        radius: 6,
    }
}

// Seeds are: surfaces [shell, panel, elevated, input], borders [normal,
// strong], texts [primary, secondary, muted, faint], the accent, and hues
// [green, red, yellow, blue, orange, purple, teal].

const MIDNIGHT: Seed = seed(
    [0x000000, 0x0a0a0a, 0x161616, 0x050505],
    [0x1c1c1c, 0x2a2a2a],
    [0xededed, 0xa1a1a1, 0x6b6b6b, 0x454545],
    0x5e6ad2,
    [
        0x4cb782, 0xeb5757, 0xe2b94a, 0x4ea7fc, 0xf2994a, 0xa79df2, 0x76cfc4,
    ],
);

const CATPPUCCIN_MOCHA: Seed = seed(
    [0x11111b, 0x1e1e2e, 0x313244, 0x181825],
    [0x313244, 0x45475a],
    [0xcdd6f4, 0xa6adc8, 0x7f849c, 0x585b70],
    0xcba6f7,
    [
        0xa6e3a1, 0xf38ba8, 0xf9e2af, 0x89b4fa, 0xfab387, 0xcba6f7, 0x94e2d5,
    ],
);
const CATPPUCCIN_LATTE: Seed = seed(
    [0xe6e9ef, 0xeff1f5, 0xeff1f5, 0xeff1f5],
    [0xccd0da, 0xbcc0cc],
    [0x4c4f69, 0x5c5f77, 0x7c7f93, 0xacb0be],
    0x8839ef,
    [
        0x40a02b, 0xd20f39, 0xdf8e1d, 0x1e66f5, 0xfe640b, 0x8839ef, 0x179299,
    ],
);

const GRUVBOX_DARK: Seed = seed(
    [0x1d2021, 0x282828, 0x3c3836, 0x1d2021],
    [0x3c3836, 0x504945],
    [0xebdbb2, 0xbdae93, 0x928374, 0x665c54],
    0xd79921,
    [
        0xb8bb26, 0xfb4934, 0xfabd2f, 0x83a598, 0xfe8019, 0xd3869b, 0x8ec07c,
    ],
);
const GRUVBOX_LIGHT: Seed = seed(
    [0xf2e5bc, 0xfbf1c7, 0xf9f5d7, 0xf9f5d7],
    [0xd5c4a1, 0xbdae93],
    [0x3c3836, 0x504945, 0x7c6f64, 0xa89984],
    0xb57614,
    [
        0x79740e, 0x9d0006, 0xb57614, 0x076678, 0xaf3a03, 0x8f3f71, 0x427b58,
    ],
);

const NORD_DARK: Seed = seed(
    [0x272c36, 0x2e3440, 0x3b4252, 0x272c36],
    [0x3b4252, 0x4c566a],
    [0xeceff4, 0xd8dee9, 0x7b88a1, 0x4c566a],
    0x88c0d0,
    [
        0xa3be8c, 0xbf616a, 0xebcb8b, 0x81a1c1, 0xd08770, 0xb48ead, 0x8fbcbb,
    ],
);
const NORD_LIGHT: Seed = seed(
    [0xe5e9f0, 0xeceff4, 0xf8f9fb, 0xf8f9fb],
    [0xd8dee9, 0xc2c9d6],
    [0x2e3440, 0x434c5e, 0x6a758c, 0xa3acbd],
    0x5e81ac,
    [
        0x5f8a4b, 0xb0444f, 0xa0782b, 0x5e81ac, 0xb4613f, 0x946e8e, 0x4f8f8c,
    ],
);

const SOLARIZED_DARK: Seed = seed(
    [0x00212b, 0x002b36, 0x073642, 0x00212b],
    [0x073642, 0x0f4a5a],
    [0x93a1a1, 0x839496, 0x657b83, 0x3f5a63],
    0x268bd2,
    [
        0x859900, 0xdc322f, 0xb58900, 0x268bd2, 0xcb4b16, 0x6c71c4, 0x2aa198,
    ],
);
const SOLARIZED_LIGHT: Seed = seed(
    [0xeee8d5, 0xfdf6e3, 0xfffbf0, 0xfffbf0],
    [0xe4ddc8, 0xd3cbb4],
    [0x586e75, 0x657b83, 0x93a1a1, 0xb9c0b6],
    0x268bd2,
    [
        0x859900, 0xdc322f, 0xb58900, 0x268bd2, 0xcb4b16, 0x6c71c4, 0x2aa198,
    ],
);

const TOKYO_NIGHT: Seed = seed(
    [0x16161e, 0x1a1b26, 0x24283b, 0x16161e],
    [0x292e42, 0x3b4261],
    [0xc0caf5, 0xa9b1d6, 0x565f89, 0x414868],
    0x7aa2f7,
    [
        0x9ece6a, 0xf7768e, 0xe0af68, 0x7aa2f7, 0xff9e64, 0xbb9af7, 0x7dcfff,
    ],
);
const TOKYO_DAY: Seed = seed(
    [0xd5d6db, 0xe1e2e7, 0xe9e9ed, 0xe9e9ed],
    [0xc4c8da, 0xa8aecb],
    [0x3760bf, 0x6172b0, 0x848cb5, 0xa1a6c5],
    0x2e7de9,
    [
        0x587539, 0xf52a65, 0x8c6c3e, 0x2e7de9, 0xb15c00, 0x9854f1, 0x007197,
    ],
);

/// tweakcn's "Claude" theme: warm paper and terracotta, from
/// tweakcn.com/themes/cmdght103000n04lh3e2ae93r. Its surfaces come from the
/// CSS; the code colors are chosen by hand, because the theme's chart colors
/// are too close to its backgrounds.
fn claude() -> Preset {
    let theme = import_css(include_str!("../assets/themes/claude.css"));
    let half = |palette: Option<Palette>, fallback: Palette, code: [u32; 7]| {
        let mut palette = palette.unwrap_or(fallback);
        for (class, hex) in [
            syntax::KEYWORD,
            syntax::STRING,
            syntax::NUMBER,
            syntax::ESCAPE,
            syntax::FUNCTION,
            syntax::TYPE,
            syntax::TAG,
        ]
        .into_iter()
        .zip(code)
        {
            palette.syntax[class as usize] = rgb(hex);
        }
        palette
    };
    let (light, dark) = theme.map_or((None, None), |t| (t.light, t.dark));
    Preset {
        id: "claude",
        name: "Claude",
        // Code colors: keyword, string, number, escape, function, type, tag.
        dark: half(
            dark,
            FEATHER_DARK,
            [
                0xd97757, 0xa8b878, 0xd9a657, 0x7db5a8, 0xb0a4e8, 0x8ab4cf, 0xe0806a,
            ],
        ),
        light: half(
            light,
            FEATHER_LIGHT,
            [
                0xb5522e, 0x5b7a2e, 0x946500, 0x1f7a6c, 0x6a4fc9, 0x2f6f98, 0xb03a2e,
            ],
        ),
    }
}

/// The built-in themes. The first is the default.
pub fn presets() -> &'static [Preset] {
    static PRESETS: OnceLock<Vec<Preset>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        let pair = |id, name, dark: Seed, light: Seed| Preset {
            id,
            name,
            dark: dark.palette(),
            light: light.palette(),
        };
        vec![
            Preset {
                id: DEFAULT_THEME,
                name: "Feather",
                dark: FEATHER_DARK,
                light: FEATHER_LIGHT,
            },
            claude(),
            Preset {
                id: "midnight",
                name: "Midnight",
                dark: MIDNIGHT.palette(),
                // Feather's light half, but keeping Midnight's indigo accent.
                light: Palette {
                    accent: rgb(0x5e6ad2),
                    accent_hover: rgb(0x4f5bc4),
                    link: rgb(0x4a56c9),
                    ..FEATHER_LIGHT
                },
            },
            pair(
                "catppuccin",
                "Catppuccin",
                CATPPUCCIN_MOCHA,
                CATPPUCCIN_LATTE,
            ),
            pair("gruvbox", "Gruvbox", GRUVBOX_DARK, GRUVBOX_LIGHT),
            pair("nord", "Nord", NORD_DARK, NORD_LIGHT),
            pair("solarized", "Solarized", SOLARIZED_DARK, SOLARIZED_LIGHT),
            pair("tokyo-night", "Tokyo Night", TOKYO_NIGHT, TOKYO_DAY),
        ]
    })
}

// ── Importing ──────────────────────────────────────────────────────────────

/// A theme read from CSS. Either half may be missing.
#[derive(Clone, Copy, Debug)]
pub struct Imported {
    pub light: Option<Palette>,
    pub dark: Option<Palette>,
}

impl Imported {
    /// The palette for the wanted mode, or the other one if that is all there is.
    pub fn pick(&self, dark: bool) -> Option<Palette> {
        if dark {
            self.dark.or(self.light)
        } else {
            self.light.or(self.dark)
        }
    }
}

/// Reads a theme from shadcn-style CSS variables, as copied from tweakcn.
pub fn import_css(css: &str) -> Result<Imported, String> {
    let (light, dark) = variable_blocks(css);
    let light = light.as_ref().and_then(palette_from_variables);
    let dark = dark.as_ref().and_then(palette_from_variables);
    if light.is_none() && dark.is_none() {
        return Err(
            "No theme found in that text. Paste the CSS from tweakcn's Code button: it starts \
             with \":root {\" and has lines like \"--background: ...\"."
                .to_owned(),
        );
    }
    Ok(Imported { light, dark })
}

type Variables = HashMap<String, String>;

/// Collects `--name: value` pairs from the `:root` and `.dark` blocks.
fn variable_blocks(css: &str) -> (Option<Variables>, Option<Variables>) {
    let mut light: Option<Variables> = None;
    let mut dark: Option<Variables> = None;
    let mut rest = css;
    // Variables pasted without any selector around them count as `:root`.
    if !css.contains('{') {
        return (Some(variables(css)).filter(|v| !v.is_empty()), None);
    }
    while let Some(open) = rest.find('{') {
        let selector = rest[..open].rsplit(['}', ';']).next().unwrap_or("").trim();
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + close];
        let found = variables(body);
        if !found.is_empty() {
            if selector.contains(".dark") {
                dark.get_or_insert_with(Variables::new).extend(found);
            } else if selector.contains(":root") || selector.is_empty() {
                light.get_or_insert_with(Variables::new).extend(found);
            }
        }
        rest = &rest[open + close + 1..];
    }
    (light, dark)
}

fn variables(body: &str) -> Variables {
    body.split(';')
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let name = name.trim().strip_prefix("--")?;
            Some((name.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn palette_from_variables(vars: &Variables) -> Option<Palette> {
    let color = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| vars.get(*name).and_then(|v| parse_color(v)))
    };
    let panel = color(&["background"])?;
    let text = color(&["foreground"])?;
    let dark = luminance(panel) < 0.4;
    let base = if dark { FEATHER_DARK } else { FEATHER_LIGHT };
    let black = Color32::BLACK;

    let secondary = color(&["muted-foreground"]).unwrap_or(mix(text, panel, 0.35));
    let border = color(&["border", "sidebar-border"]).unwrap_or(mix(panel, text, 0.12));
    let accent = color(&["primary", "sidebar-primary"]).unwrap_or(base.accent);
    let readable = |c: Color32| legible(c, panel, text);
    let chart = |n: &str, fallback: Color32| color(&[n]).map_or(fallback, readable);

    let seed = Seed {
        shell: color(&["sidebar", "sidebar-background"]).unwrap_or(if dark {
            mix(panel, black, 0.3)
        } else {
            mix(panel, black, 0.035)
        }),
        panel,
        elevated: color(&["popover", "card"]).unwrap_or(panel),
        input: if dark { mix(panel, black, 0.2) } else { panel },
        border,
        border_strong: mix(border, text, 0.16),
        text,
        text_secondary: secondary,
        text_muted: mix(secondary, panel, 0.3),
        text_faint: mix(secondary, panel, 0.58),
        accent,
        // Status colors keep their meaning, whatever the theme's own hues are.
        green: base.green,
        // Some themes make "destructive" black or white. Only a real color will do.
        red: color(&["destructive"])
            .filter(|c| saturation(*c) > 0.25)
            .map_or(base.red, readable),
        yellow: base.yellow,
        blue: base.blue,
        orange: base.orange,
        purple: base.syntax[syntax::KEYWORD as usize],
        teal: base.syntax[syntax::ESCAPE as usize],
        radius: vars
            .get("radius")
            .and_then(|v| parse_length(v))
            .map_or(6, |px| {
                // Feather's controls are smaller than a web page's, so scale down a little.
                (px * 0.75).round().clamp(0.0, 12.0) as u8
            }),
    };
    let mut palette = seed.palette();
    // The theme's chart colors, where it has them, color the code.
    for (class, name) in [
        (syntax::KEYWORD, "chart-1"),
        (syntax::STRING, "chart-2"),
        (syntax::NUMBER, "chart-3"),
        (syntax::FUNCTION, "chart-4"),
        (syntax::TYPE, "chart-5"),
    ] {
        palette.syntax[class as usize] = chart(name, palette.syntax[class as usize]);
    }
    Some(palette)
}

/// `0.5rem`, `8px` or a bare number of pixels.
fn parse_length(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(rem) = value.strip_suffix("rem") {
        return rem.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    value
        .strip_suffix("px")
        .unwrap_or(value)
        .trim()
        .parse()
        .ok()
}

/// Parses the color notations shadcn themes use: `oklch(...)`, `hsl(...)`, a
/// bare `H S% L%` triple, `rgb(...)` and `#hex`.
pub fn parse_color(value: &str) -> Option<Color32> {
    let value = value.trim().trim_end_matches("!important").trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = value.to_ascii_lowercase();
    let (function, arguments) = match lower.split_once('(') {
        Some((name, rest)) => (name.trim(), rest.trim_end_matches(')')),
        None => ("", lower.as_str()),
    };
    // Drop any "/ alpha" part, then split on spaces or commas.
    let arguments = arguments.split('/').next()?;
    let parts: Vec<&str> = arguments
        .split([' ', ','])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let number = |text: &str, percent_of: f32| -> Option<f32> {
        match text.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok().map(|v| v / 100.0 * percent_of),
            None => text.trim_end_matches("deg").parse::<f32>().ok(),
        }
    };
    match function {
        "oklch" => Some(oklch_to_srgb(
            number(parts[0], 1.0)?,
            number(parts[1], 0.4)?,
            number(parts[2], 360.0)?,
        )),
        "rgb" | "rgba" => Some(Color32::from_rgb(
            number(parts[0], 255.0)?.round().clamp(0.0, 255.0) as u8,
            number(parts[1], 255.0)?.round().clamp(0.0, 255.0) as u8,
            number(parts[2], 255.0)?.round().clamp(0.0, 255.0) as u8,
        )),
        // Older shadcn themes store bare "H S% L%" and wrap it in hsl() later.
        "hsl" | "hsla" | "" => Some(hsl_to_srgb(
            number(parts[0], 360.0)?,
            number(parts[1], 1.0)?,
            number(parts[2], 1.0)?,
        )),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Color32> {
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 | 4 => Some(Color32::from_rgb(
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
        )),
        6 | 8 => Some(Color32::from_rgb(pair(0)?, pair(2)?, pair(4)?)),
        _ => None,
    }
}

fn to_byte(linear: f32) -> u8 {
    let v = linear.clamp(0.0, 1.0);
    let encoded = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

fn oklch_to_srgb(lightness: f32, chroma: f32, hue_degrees: f32) -> Color32 {
    let hue = hue_degrees.to_radians();
    let (a, b) = (chroma * hue.cos(), chroma * hue.sin());
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    Color32::from_rgb(
        to_byte(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s),
        to_byte(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        to_byte(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

fn hsl_to_srgb(hue: f32, saturation: f32, lightness: f32) -> Color32 {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Color32, b: Color32) -> bool {
        let d = |x: u8, y: u8| (i16::from(x) - i16::from(y)).abs() <= 2;
        d(a.r(), b.r()) && d(a.g(), b.g()) && d(a.b(), b.b())
    }

    #[test]
    fn parses_the_color_notations_themes_use() {
        assert_eq!(parse_color("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_color("#5E6AD2"), Some(rgb(0x5e6ad2)));
        assert_eq!(parse_color("rgb(94, 106, 210)"), Some(rgb(0x5e6ad2)));
        assert_eq!(parse_color("rgb(94 106 210 / 50%)"), Some(rgb(0x5e6ad2)));
        assert_eq!(parse_color("oklch(1 0 0)"), Some(Color32::WHITE));
        assert_eq!(parse_color("oklch(0 0 0)"), Some(Color32::BLACK));
        // Reference values from the shadcn/ui default theme.
        assert!(close(
            parse_color("oklch(0.145 0 0)").unwrap(),
            rgb(0x0a0a0a)
        ));
        assert!(close(
            parse_color("oklch(0.985 0 0)").unwrap(),
            rgb(0xfafafa)
        ));
        assert!(close(
            parse_color("oklch(0.577 0.245 27.325)").unwrap(),
            rgb(0xe7000b)
        ));
        assert!(close(
            parse_color("oklch(62.3% 0.214 259.815)").unwrap(),
            rgb(0x2b7fff)
        ));
        assert!(close(
            parse_color("hsl(240 10% 3.9%)").unwrap(),
            rgb(0x09090b)
        ));
        assert!(close(parse_color("240 5.9% 10%").unwrap(), rgb(0x18181b)));
        assert!(close(
            parse_color("hsl(0, 100%, 50%)").unwrap(),
            rgb(0xff0000)
        ));
        assert_eq!(parse_color("0.5rem"), None);
        assert_eq!(parse_color("var(--primary)"), None);
    }

    #[test]
    fn imports_light_and_dark_from_tweakcn_css() {
        let css = r#"
            :root {
              --background: oklch(1 0 0);
              --foreground: oklch(0.145 0 0);
              --primary: oklch(0.6231 0.1880 259.8145);
              --muted-foreground: oklch(0.556 0 0);
              --border: oklch(0.922 0 0);
              --destructive: oklch(0.577 0.245 27.325);
              --sidebar: oklch(0.985 0 0);
              --chart-2: oklch(0.6 0.118 184.704);
              --radius: 0.625rem;
              --font-sans: Inter, sans-serif;
            }
            .dark {
              --background: oklch(0.145 0 0);
              --foreground: oklch(0.985 0 0);
              --primary: oklch(0.6231 0.1880 259.8145);
              --border: oklch(1 0 0 / 10%);
            }
            @theme inline { --color-background: var(--background); }
        "#;
        let theme = import_css(css).unwrap();
        let (light, dark) = (theme.light.unwrap(), theme.dark.unwrap());
        assert!(!light.dark && dark.dark);
        assert_eq!(light.bg_panel, Color32::WHITE);
        assert!(close(light.bg_shell, rgb(0xfafafa)));
        assert!(close(light.accent, rgb(0x3b82f6)));
        assert_eq!(light.radius, 8);
        assert!(close(dark.bg_panel, rgb(0x0a0a0a)));
        assert!(luminance(dark.bg_shell) < luminance(dark.bg_panel));
        // Text colors stay readable against the background in both.
        for palette in [light, dark] {
            assert!(contrast(palette.text, palette.bg_panel) > 10.0);
            assert!(contrast(palette.link, palette.bg_panel) >= 3.0);
            for color in palette.syntax {
                assert!(contrast(color, palette.bg_panel) >= 2.5);
            }
        }
        assert_eq!(theme.pick(true).unwrap().bg_panel, dark.bg_panel);
    }

    #[test]
    fn accepts_partial_themes_and_rejects_other_text() {
        let only_dark = import_css(".dark { --background: #111; --foreground: #eee; }").unwrap();
        assert!(only_dark.light.is_none());
        assert!(only_dark.pick(false).unwrap().dark);
        let bare = import_css("--background: 0 0% 100%; --foreground: 240 10% 3.9%;").unwrap();
        assert!(bare.light.is_some());
        assert!(import_css("body { color: red; }").is_err());
        assert!(import_css("").is_err());
    }

    #[test]
    fn every_preset_is_readable() {
        for preset in presets() {
            for palette in [preset.dark, preset.light] {
                assert!(
                    contrast(palette.text, palette.bg_panel) >= 4.5,
                    "{} text",
                    preset.name
                );
                assert!(
                    contrast(palette.text_secondary, palette.bg_shell) >= 3.0,
                    "{} secondary text",
                    preset.name
                );
            }
            assert!(preset.dark.dark, "{} dark half", preset.name);
            assert!(!preset.light.dark, "{} light half", preset.name);
        }
    }
}
