//! Design tokens. Every colour comes from the live [`Palette`] (one per Settings → Appearance
//! preset); pages use `Tokens::*()`, never raw colours. Size scales live here too.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{linear_color_stop, linear_gradient, px, rgb, rgba, App, Background, Hsla, Rgba, Styled};

pub struct Tokens;

impl Tokens {
    // Surfaces, darkest to lightest. In a dark theme elevation reads as lightness.
    /// Behind pages: reader and Studio canvases.
    pub fn canvas() -> Rgba { palette().canvas }
    pub fn base() -> Rgba { palette().base }
    pub fn sidebar() -> Rgba { palette().sidebar }
    pub fn surface() -> Rgba { palette().surface }
    pub fn surface_2() -> Rgba { palette().surface_2 }
    /// Menus, popovers, tooltips: the highest elevation.
    pub fn surface_3() -> Rgba { palette().surface_3 }
    /// Alias of [`Tokens::surface_3`].
    pub fn popover() -> Rgba { palette().surface_3 }
    /// Recessed area (segmented control track, inset wells).
    pub fn well() -> Rgba { palette().well }

    // Lines.
    pub fn border() -> Rgba { palette().border }
    /// Hover / selected borders and input outlines.
    pub fn border_strong() -> Rgba { palette().border_strong }

    // State layers, painted over any surface.
    pub fn hover() -> Rgba { palette().hover }
    /// Selected / current.
    pub fn active() -> Rgba { palette().active }
    /// Hovering something already selected; stronger than `active` so it never looks less selected.
    pub fn active_hover() -> Rgba { palette().active_hover }
    /// Mouse down.
    pub fn pressed() -> Rgba { palette().pressed }
    /// Keyboard focus outline (2px).
    pub fn focus_ring() -> Rgba { palette().focus_ring }
    /// Unfilled part of sliders and progress bars, 3:1 or better against the background.
    pub fn track() -> Rgba { palette().track }

    // Text.
    pub fn text_1() -> Rgba { palette().text_1 }
    pub fn text_2() -> Rgba { palette().text_2 }
    /// Metadata and hints; 4.5:1 or better on every surface.
    pub fn text_3() -> Rgba { palette().text_3 }
    /// Disabled labels and icons.
    pub fn text_disabled() -> Rgba { palette().text_disabled }

    // Brand accent.
    pub fn accent() -> Rgba { palette().accent }
    pub fn accent_hover() -> Rgba { palette().accent_hover }
    pub fn accent_pressed() -> Rgba { palette().accent_pressed }
    /// Tinted fill for selected chips and rows.
    pub fn accent_soft() -> Rgba { palette().accent_soft }
    pub fn accent_border() -> Rgba { palette().accent_border }
    /// Accent at any opacity, e.g. `Tokens::accent_a(0.2)`.
    pub fn accent_a(a: f32) -> Rgba { palette().accent.alpha(a) }
    /// Text and icons on an accent fill.
    pub fn on_accent() -> Rgba { palette().on_accent }
    /// The preset's second colour, for gradients and secondary highlights.
    pub fn accent_2() -> Rgba { palette().accent_2 }

    // Status. Never decorative.
    pub fn success() -> Rgba { palette().success }
    pub fn success_soft() -> Rgba { palette().success_soft }
    pub fn info() -> Rgba { palette().info }
    pub fn info_soft() -> Rgba { palette().info_soft }
    pub fn warning() -> Rgba { palette().warning }
    pub fn warning_soft() -> Rgba { palette().warning_soft }
    pub fn warning_border() -> Rgba { palette().warning_border }
    pub fn danger() -> Rgba { palette().danger }
    pub fn danger_soft() -> Rgba { palette().danger_soft }
    pub fn danger_border() -> Rgba { palette().danger_border }

    /// The one gold for every favourite star, filled.
    pub fn favorite() -> Rgba { palette().favorite }
    /// Rating stars and scores: the same gold as [`Tokens::favorite`].
    pub fn rating() -> Rgba { palette().favorite }

    // Floating layers and dimming.
    /// Translucent black for controls over images and video.
    pub fn glass() -> Rgba { palette().glass }
    /// Near-opaque surface for floating bars and toasts over the UI.
    pub fn glass_strong() -> Rgba { palette().glass_strong }
    pub fn glass_border() -> Rgba { palette().glass_border }
    /// Dialog backdrop.
    pub fn overlay() -> Rgba { palette().overlay }
    /// Heavy dim for text on images (cover badges).
    pub fn scrim() -> Rgba { palette().scrim }
    /// Fade from transparent to the page background, e.g. `180.` under a hero image.
    pub fn overlay_gradient(angle: f32) -> Background {
        let base = palette().base;
        linear_gradient(angle, linear_color_stop(base.alpha(0.), 0.), linear_color_stop(base, 1.))
    }

    /// A categorical colour (icon tiles, storage segments, chart series). The same hue in every
    /// preset: the 400 tier on dark presets, a deeper tier on light ones so it still reads.
    pub fn hue(h: Hue) -> Rgba {
        palette().hue(h)
    }

    /// 14% tint of a hue, for the tile behind a hue-coloured icon.
    pub fn hue_soft(h: Hue) -> Rgba {
        a8(palette().hue(h), SOFT)
    }
}

/// Categorical hues for telling things apart (Settings rail tiles, storage categories, chart
/// series), never for status. All from the Tailwind 400 tier, the lightness the status colours
/// use, so each reaches 3:1 on every surface (WCAG 1.4.11) and dark text reads on a hue fill.
/// Carbon data-vis palette guidance: <https://carbondesignsystem.com/data-visualization/color-palettes/>
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hue {
    Blue,
    Violet,
    Cyan,
    Green,
    Amber,
    Orange,
    Rose,
    Slate,
}

impl Hue {
    /// Every hue, in the order to hand them out.
    pub const ALL: [Hue; 8] = [Hue::Blue, Hue::Violet, Hue::Cyan, Hue::Green, Hue::Amber, Hue::Orange, Hue::Rose, Hue::Slate];

    fn hex(self) -> u32 {
        match self {
            Hue::Blue => 0x60a5fa,
            Hue::Violet => 0xa78bfa,
            Hue::Cyan => 0x22d3ee,
            Hue::Green => 0x4ade80,
            Hue::Amber => 0xfbbf24,
            Hue::Orange => 0xfb923c,
            Hue::Rose => 0xfb7185,
            Hue::Slate => 0x94a3b8,
        }
    }

    /// The same hues for light presets: Tailwind 700/800, so each keeps 3:1 on light surfaces
    /// and a light page colour reads on it as a filled tile's glyph.
    fn hex_light(self) -> u32 {
        match self {
            Hue::Blue => 0x1d4ed8,
            Hue::Violet => 0x6d28d9,
            Hue::Cyan => 0x155e75,
            Hue::Green => 0x166534,
            Hue::Amber => 0x92400e,
            Hue::Orange => 0x9a3412,
            Hue::Rose => 0xbe123c,
            Hue::Slate => 0x475569,
        }
    }
}

/// Where a preset is listed in Settings → Appearance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PresetGroup {
    /// Quiet, neutral looks: light, paper, plain dark.
    Clean,
    /// Expressive or special-purpose looks: high contrast, OLED, community palettes, brand colours.
    Advanced,
}

impl PresetGroup {
    pub const ALL: [PresetGroup; 2] = [PresetGroup::Clean, PresetGroup::Advanced];

    pub fn label(self) -> &'static str {
        match self {
            PresetGroup::Clean => "Clean",
            PresetGroup::Advanced => "Advanced",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PresetGroup::Clean => "Calm, neutral looks that keep the focus on covers and video.",
            PresetGroup::Advanced => "High contrast, true black, community palettes and bolder colour.",
        }
    }
}

/// How a preset's surfaces, lines and text are built.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Light text on dark surfaces; lines and state layers are white at low opacity.
    Dark,
    /// Dark text on light surfaces; lines and state layers are black at low opacity.
    Light,
    /// Dark, with stronger lines, state layers and secondary text (Windows contrast themes:
    /// <https://learn.microsoft.com/en-us/windows/apps/design/accessibility/high-contrast-themes>).
    HighContrast,
}

/// One Settings → Appearance preset: id, copy, the three swatch colours (base, accent,
/// secondary), its group and mode, and optional hand-tuned surfaces and text. Build with
/// [`dark`] / [`light`] and the `const` modifiers below.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub base: u32,
    pub accent: u32,
    pub secondary: u32,
    pub group: PresetGroup,
    pub mode: Mode,
    /// Sidebar, surface, surface_2 and surface_3; derived from `base` when `None`.
    pub ramp: Option<[u32; 4]>,
    /// text_1, text_2 and text_3; the mode's defaults when `None`.
    pub text: Option<[u32; 3]>,
}

/// A dark preset in the Advanced group with derived surfaces and the default text.
pub const fn dark(id: &'static str, name: &'static str, description: &'static str, base: u32, accent: u32, secondary: u32) -> Preset {
    Preset { id, name, description, base, accent, secondary, group: PresetGroup::Advanced, mode: Mode::Dark, ramp: None, text: None }
}

/// A light preset (dark text on light surfaces) in the Advanced group.
pub const fn light(id: &'static str, name: &'static str, description: &'static str, base: u32, accent: u32, secondary: u32) -> Preset {
    Preset { mode: Mode::Light, ..dark(id, name, description, base, accent, secondary) }
}

impl Preset {
    /// Lists the preset under Clean.
    pub const fn clean(self) -> Self {
        Preset { group: PresetGroup::Clean, ..self }
    }

    pub const fn high_contrast(self) -> Self {
        Preset { mode: Mode::HighContrast, ..self }
    }

    /// Hand-tuned sidebar, surface, surface_2, surface_3.
    pub const fn ramp(self, ramp: [u32; 4]) -> Self {
        Preset { ramp: Some(ramp), ..self }
    }

    /// Hand-tuned text_1, text_2, text_3.
    pub const fn text(self, text: [u32; 3]) -> Self {
        Preset { text: Some(text), ..self }
    }

    pub fn is_light(&self) -> bool {
        self.mode == Mode::Light
    }
}

/// The original MangaVox look. Must stay first in [`PRESETS`].
pub const DEFAULT_PRESET: &str = "mangavox-mint";

/// Theme presets (ids shared with the Electron app's `src/shared/appearance.ts`, plus Mint and
/// the newer Clean / Advanced additions). Community palettes follow their published colours:
/// Nord <https://www.nordtheme.com/docs/colors-and-palettes>, Solarized
/// <https://ethanschoonover.com/solarized/>, nudged where a colour missed WCAG AA as text.
pub const PRESETS: &[Preset] = &[
    // ── Clean ──
    dark("mangavox-mint", "MangaVox Mint", "Signature mint and sky on cool graphite", 0x0b0d10, 0x2bd49a, 0x7dd3fc).clean(),
    light("light", "Light", "Crisp white cards on soft grey, classic blue", 0xf5f6f8, 0x2563eb, 0x0ea5e9).clean().ramp([0xeceef2, 0xffffff, 0xe9ecf0, 0xffffff]),
    light("soft-light", "Soft Light", "Low-glare grey with a calm teal accent", 0xe9eaee, 0x0f766e, 0x6366f1).clean().ramp([0xe1e3e8, 0xf4f5f7, 0xdfe2e7, 0xf8f9fa]).text([0x1b1e24, 0x353b46, 0x4b5360]),
    light("paper", "Paper", "Warm off-white like a printed page", 0xf7f3ea, 0xa8471f, 0x4d7c5a).clean().ramp([0xefe9dc, 0xfffdf8, 0xece5d6, 0xfffdf8]).text([0x24201a, 0x4a4337, 0x625a4c]),
    dark("pure-dark", "Pure Dark", "Neutral greys with no tint, blue for actions", 0x0a0a0a, 0x3b82f6, 0xa3a3a3).clean().ramp([0x0f0f0f, 0x161616, 0x1e1e1e, 0x262626]),
    dark("graphite", "Graphite", "Soft charcoal and a quiet periwinkle", 0x17181b, 0x8b93ff, 0x9ca3af).clean().text([0xf2f3f5, 0xc8cbd1, 0x9aa1ab]),
    dark("monochrome", "Monochrome", "Graphite, silver, and quiet motion", 0x090a0b, 0xb8c0cc, 0x7f8ea3).clean(),
    // ── Advanced ──
    dark("high-contrast", "High Contrast", "Black, white and yellow with strong edges", 0x000000, 0xffd60a, 0x00e5ff).high_contrast().ramp([0x000000, 0x0a0a0a, 0x141414, 0x1c1c1c]).text([0xffffff, 0xf2f2f2, 0xd6d6d6]),
    dark("oled-black", "OLED Black", "True black that switches OLED pixels off", 0x000000, 0xa78bfa, 0x22d3ee).ramp([0x000000, 0x0b0b0c, 0x141416, 0x1c1c1f]),
    dark("nord", "Nord", "Arctic polar night with frost blue", 0x2e3440, 0x88c0d0, 0x81a1c1).ramp([0x2a2f3a, 0x343a47, 0x3b4252, 0x434c5e]).text([0xeceff4, 0xd8dee9, 0xb6bfcd]),
    dark("solarized-dark", "Solarized Dark", "Ethan Schoonover's teal-blue night", 0x002b36, 0x2aa198, 0xb58900).ramp([0x00252f, 0x073642, 0x0b3d4a, 0x104552]).text([0xfdf6e3, 0xeee8d5, 0xa3b1b1]),
    light("solarized-light", "Solarized Light", "Solarized's warm cream daylight", 0xfdf6e3, 0x1e6fa8, 0xb58900).ramp([0xf3ecd8, 0xfffbf0, 0xeee8d5, 0xfffcf4]).text([0x073642, 0x34525a, 0x52676d]),
    dark("midnight-blue", "Midnight Blue", "Deep navy with a clear sky-blue accent", 0x0a1020, 0x6ea8fe, 0xa5b4fc),
    light("sakura-light", "Sakura Light", "Blossom pink paper with a deep rose accent", 0xfbf3f5, 0xbe185d, 0xf9a8d4).ramp([0xf5e8ec, 0xfffafb, 0xf3e4e9, 0xfffbfc]).text([0x2a1a20, 0x4e3a42, 0x6b5560]),
    dark("sakura", "Sakura", "Rose highlights and drifting petals", 0x0d090c, 0xfb7185, 0xf9a8d4),
    dark("rust", "Rust", "Oxidized orange on warm charcoal — forged metal", 0x0e0907, 0xc8501a, 0xf0b27a),
    dark("watchfiwn", "Watchfiwn", "Cinematic red on true black — streaming-grade", 0x0b0b0c, 0xe8392e, 0xff6b5f),
    dark("mangavox-emerald", "MangaVox Emerald", "Emerald and cyan studio lighting", 0x07090d, 0x10b981, 0x60a5fa),
    dark("chatgpt-green", "ChatGPT Green", "Charcoal, mint, and connected signals", 0x080b0a, 0x10a37f, 0x9ce6d1),
    dark("claude-warm", "Claude Warm", "Warm terracotta on deep charcoal", 0x0d0b0a, 0xd97757, 0xe8c7a8),
    dark("gemini-prism", "Gemini Prism", "Blue, violet, and cyan refractions", 0x080a12, 0x5b8def, 0xa78bfa),
    dark("amber-terminal", "Amber Terminal", "Black glass and restrained amber data", 0x090806, 0xf59e0b, 0xfde68a),
    dark("cyberpunk", "Cyberpunk", "Cyan and magenta over graphite", 0x07090c, 0x22d3ee, 0xf472b6),
    dark("ocean", "Ocean", "Deep neutral water and clear cyan", 0x060b0e, 0x06b6d4, 0x38bdf8),
    dark("aurora", "Aurora", "Green, blue, and violet circular flow", 0x070a0c, 0x2dd4bf, 0x818cf8),
];

/// Every colour token, resolved for one preset. Read the live one with [`palette`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub canvas: Rgba,
    pub base: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub surface_2: Rgba,
    pub surface_3: Rgba,
    pub well: Rgba,
    pub border: Rgba,
    pub border_strong: Rgba,
    pub hover: Rgba,
    pub active: Rgba,
    pub active_hover: Rgba,
    pub pressed: Rgba,
    pub focus_ring: Rgba,
    pub track: Rgba,
    pub text_1: Rgba,
    pub text_2: Rgba,
    pub text_3: Rgba,
    pub text_disabled: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_pressed: Rgba,
    pub accent_soft: Rgba,
    pub accent_border: Rgba,
    pub on_accent: Rgba,
    pub accent_2: Rgba,
    pub success: Rgba,
    pub success_soft: Rgba,
    pub info: Rgba,
    pub info_soft: Rgba,
    pub warning: Rgba,
    pub warning_soft: Rgba,
    pub warning_border: Rgba,
    pub danger: Rgba,
    pub danger_soft: Rgba,
    pub danger_border: Rgba,
    pub favorite: Rgba,
    pub glass: Rgba,
    pub glass_strong: Rgba,
    pub glass_border: Rgba,
    pub overlay: Rgba,
    pub scrim: Rgba,
    /// Dark text on light surfaces (picks the light hue tier and the light component theme).
    pub light: bool,
}

/// Soft fills are 14% of their colour, borders 35% (the same steps as the original accent_soft).
const SOFT: u8 = 0x24;
const EDGE: u8 = 0x59;

impl Palette {
    /// Builds the palette from a preset's base and accent: surfaces lift the base in even steps
    /// (keeping its tint), accent states shift its lightness, status colours are shared.
    pub fn from_preset(p: &Preset) -> Self {
        let light = p.mode == Mode::Light;
        let hc = p.mode == Mode::HighContrast;
        let base = rgb(p.base);
        let accent = rgb(p.accent);
        let [sidebar, surface, surface_2, surface_3] = neutrals(p);
        let [text_1, text_2, text_3] = p
            .text
            .unwrap_or(match p.mode {
                Mode::Dark => [0xf2f4f7, 0xc3c9d1, 0x8b949e],
                Mode::Light => [0x16181d, 0x3d4450, 0x5b6370],
                Mode::HighContrast => [0xffffff, 0xf2f2f2, 0xd6d6d6],
            })
            .map(rgb);
        // Lines and state layers are "ink" at low opacity: white on dark, black on light.
        // Steps: border, border_strong, hover, active, active_hover, pressed, track, glass_border.
        let steps: [u8; 8] = match p.mode {
            Mode::Dark => [0x12, 0x26, 0x0d, 0x17, 0x1f, 0x24, 0x5c, 0x1a],
            Mode::Light => [0x1a, 0x33, 0x0a, 0x12, 0x1a, 0x21, 0x80, 0x1f],
            Mode::HighContrast => [0x59, 0x99, 0x1a, 0x2b, 0x38, 0x40, 0xa6, 0x66],
        };
        let ink = |a: u8| if light { a8(rgb(0x000000), a) } else { a8(rgb(0xffffff), a) };
        // Tracks need 3:1 on every surface (WCAG 1.4.11); lighter bases (Nord) need more ink.
        let track_bgs = [base, sidebar, surface, surface_2, surface_3, base.blend(ink(steps[3]))];
        let track_a = (steps[6]..=0xff).find(|a| track_bgs.iter().all(|bg| contrast(ink(*a), *bg) >= 3.05)).unwrap_or(0xff);
        // Status colours: the 400 tier reads on dark, the 700 tier on light.
        let (success, info, warning, danger) = if light {
            (rgb(0x15803d), rgb(0x1d4ed8), rgb(0xb45309), rgb(0xb91c1c))
        } else if hc {
            (rgb(0x4ade80), rgb(0x60a5fa), rgb(0xfacc15), rgb(0xff6b6b))
        } else {
            (rgb(0x4ade80), rgb(0x60a5fa), rgb(0xfbbf24), rgb(0xf87171))
        };
        Self {
            canvas: if light { scale(base, 0.94) } else { scale(base, 0.65) },
            base,
            sidebar,
            surface,
            surface_2,
            surface_3,
            well: if light { rgba(0x0000000d) } else if hc { rgba(0xffffff0f) } else { rgba(0x00000038) },
            border: ink(steps[0]),
            border_strong: ink(steps[1]),
            hover: ink(steps[2]),
            active: ink(steps[3]),
            active_hover: ink(steps[4]),
            pressed: ink(steps[5]),
            focus_ring: accent,
            track: ink(track_a),
            text_1,
            text_2,
            text_3,
            text_disabled: a8(text_3, 0x66),
            accent,
            // Hover moves away from the page: lighter on dark, deeper on light.
            accent_hover: shift_l(accent, if light { -0.06 } else { 0.07 }),
            accent_pressed: shift_l(accent, if light { -0.12 } else { -0.06 }),
            accent_soft: a8(accent, SOFT),
            accent_border: a8(accent, EDGE),
            on_accent: readable_on(accent, [base, text_1, rgb(0x000000), rgb(0xffffff)]),
            accent_2: rgb(p.secondary),
            success,
            success_soft: a8(success, SOFT),
            info,
            info_soft: a8(info, SOFT),
            warning,
            warning_soft: a8(warning, SOFT),
            warning_border: a8(warning, EDGE),
            danger,
            danger_soft: a8(danger, SOFT),
            danger_border: a8(danger, EDGE),
            favorite: if light { rgb(0xa16207) } else { rgb(0xfacc15) },
            // Text over glass and scrims is text_1, so on light presets they are light washes
            // (the macOS light material) and dark text keeps reading over images.
            glass: if light { rgba(0xffffffb8) } else { rgba(0x0000008c) },
            glass_strong: a8(surface, 0xf0),
            glass_border: ink(steps[7]),
            overlay: if light { rgba(0x0000002e) } else { rgba(0x0000004d) },
            scrim: if light { rgba(0xffffffc7) } else { rgba(0x000000b3) },
            light,
        }
    }

    /// A categorical hue in this palette's tier (see [`Tokens::hue`]).
    pub fn hue(&self, h: Hue) -> Rgba {
        rgb(if self.light { h.hex_light() } else { h.hex() })
    }
}

/// Sidebar, surface, surface_2 and surface_3: the preset's own ramp, else Mint's original
/// hand-tuned one, else derived from the base (lifted for dark presets, whitened for light).
fn neutrals(p: &Preset) -> [Rgba; 4] {
    if let Some(r) = p.ramp {
        return r.map(rgb);
    }
    if p.id == DEFAULT_PRESET {
        return [rgb(0x0f1215), rgb(0x14181c), rgb(0x1a1f24), rgb(0x21272d)];
    }
    let base = rgb(p.base);
    if p.mode == Mode::Light {
        let white = rgb(0xffffff);
        return [scale(base, 0.965), mix(base, white, 0.7), scale(base, 0.955), mix(base, white, 0.85)];
    }
    [4.7, 10.7, 17.7, 25.7].map(|k| lift(base, k))
}

/// `a` moved `t` of the way to `b`.
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1. }
}

/// Adds `k`/255 to each channel in proportion to the base's own tint, so warm bases stay warm.
fn lift(c: Rgba, k: f32) -> Rgba {
    let mean = (c.r + c.g + c.b) / 3.;
    let up = |v: f32| {
        let tint = if mean > 0. { (v / mean).clamp(0.7, 1.4) } else { 1. };
        (v + k / 255. * tint).min(1.)
    };
    Rgba { r: up(c.r), g: up(c.g), b: up(c.b), a: 1. }
}

/// First candidate that reaches 4.5:1 on `bg` (WCAG AA body text), else the best of them.
fn readable_on(bg: Rgba, candidates: [Rgba; 4]) -> Rgba {
    let best = candidates.into_iter().max_by(|a, b| contrast(*a, bg).total_cmp(&contrast(*b, bg))).unwrap_or(bg);
    candidates.into_iter().find(|c| contrast(*c, bg) >= 4.5).unwrap_or(best)
}

fn scale(c: Rgba, f: f32) -> Rgba {
    Rgba { r: c.r * f, g: c.g * f, b: c.b * f, a: c.a }
}

fn a8(c: Rgba, a: u8) -> Rgba {
    c.alpha(a as f32 / 255.)
}

/// Moves HSL lightness by `d` (hover lightens, pressed darkens).
fn shift_l(c: Rgba, d: f32) -> Rgba {
    let mut h = Hsla::from(c);
    h.l = (h.l + d).clamp(0., 1.);
    h.to_rgb()
}

/// WCAG 2.x relative luminance.
fn luminance(c: Rgba) -> f32 {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio (1 to 21) of `fg` drawn over an opaque `bg`; `fg`'s alpha is composited first.
pub fn contrast(fg: Rgba, bg: Rgba) -> f32 {
    let (a, b) = (luminance(bg.blend(fg)), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn palettes() -> &'static [Palette] {
    static ALL: OnceLock<Vec<Palette>> = OnceLock::new();
    ALL.get_or_init(|| PRESETS.iter().map(Palette::from_preset).collect())
}

/// Index into `PRESETS` of the live palette; 0 is the default.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The live palette. Lock-free, cheap enough to call for every element on every frame.
pub fn palette() -> &'static Palette {
    &palettes()[CURRENT.load(Ordering::Relaxed)]
}

/// Id of the live preset.
pub fn palette_id() -> &'static str {
    PRESETS[CURRENT.load(Ordering::Relaxed)].id
}

/// Looks a preset up by id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Switches every token to preset `id` (unknown ids fall back to the default), re-themes the
/// gpui-component widgets and redraws all windows.
pub fn set_palette(id: &str, cx: &mut App) {
    CURRENT.store(PRESETS.iter().position(|p| p.id == id).unwrap_or(0), Ordering::Relaxed);
    apply_component_theme(cx);
}

/// Startup: applies the saved `appearance.preset` and `appearance.reduceMotion` from config.json
/// (the keys Settings writes). Call after `Theme::change`.
pub fn init(cx: &mut App) {
    let ap = mv_core::config::Config::open_default().get("appearance");
    let preset = ap.as_ref().and_then(|a| a.get("preset")).and_then(|v| v.as_str()).unwrap_or(DEFAULT_PRESET).to_string();
    let reduce = ap.as_ref().and_then(|a| a.get("reduceMotion")).and_then(|v| v.as_bool()).unwrap_or(false);
    crate::motion::set_reduced(reduce, cx);
    set_palette(&preset, cx);
}

/// Maps the live palette onto the gpui-component theme so Switch, Slider, Select, Input, Tooltip,
/// the title bar and scrollbars match the brand. Runs at startup and on every palette change.
pub fn apply_component_theme(cx: &mut App) {
    if !cx.has_global::<Theme>() {
        cx.refresh_windows();
        return;
    }
    let p = *palette();
    let c = Hsla::from;
    // Opaque mix of a state layer over a fill, for widgets that expect solid colours.
    let over = |bg: Rgba, layer: Rgba| Hsla::from(bg.blend(layer));
    // Switch and slider thumbs: white on light presets (text_1 is dark there).
    let thumb = if p.light { p.surface_3 } else { p.text_1 };
    Theme::update(cx, |t| {
        // Widgets that shade by mode (buttons, shimmer, highlighted text) follow the preset.
        t.mode = if p.light { ThemeMode::Light } else { ThemeMode::Dark };
        t.radius = px(radius::MD);
        t.radius_lg = px(radius::LG);

        t.background = c(p.base);
        t.foreground = c(p.text_1);
        t.muted = c(p.surface_2);
        t.muted_foreground = c(p.text_3);
        t.border = c(p.border);
        t.input = c(p.border_strong);
        t.ring = c(p.focus_ring);
        t.caret = c(p.accent);
        // The library caps selection at 30% so selected text stays readable.
        t.selection = c(p.accent.alpha(0.3));
        t.popover = c(p.surface_3);
        t.popover_foreground = c(p.text_1);
        t.overlay = c(p.overlay);
        t.window_border = c(p.border);
        t.drag_border = c(p.accent);
        t.drop_target = c(p.accent_soft);
        t.skeleton = c(p.surface_2);
        t.progress_bar = c(p.accent);
        t.group_box = c(p.surface);
        t.group_box_foreground = c(p.text_1);
        t.accordion = c(p.surface);

        // shadcn's "accent" is the neutral hover / selected fill of menu and list rows.
        t.accent = c(p.active);
        t.accent_foreground = c(p.text_1);

        t.primary = c(p.accent);
        t.primary_hover = c(p.accent_hover);
        t.primary_active = c(p.accent_pressed);
        t.primary_foreground = c(p.on_accent);
        t.button_primary = c(p.accent);
        t.button_primary_hover = c(p.accent_hover);
        t.button_primary_active = c(p.accent_pressed);
        t.button_primary_foreground = c(p.on_accent);

        t.secondary = c(p.surface_2);
        t.secondary_hover = over(p.surface_2, p.hover);
        t.secondary_active = over(p.surface_2, p.pressed);
        t.secondary_foreground = c(p.text_1);
        t.button_secondary = c(p.surface_2);
        t.button_secondary_hover = over(p.surface_2, p.hover);
        t.button_secondary_active = over(p.surface_2, p.pressed);
        t.button_secondary_foreground = c(p.text_1);
        t.button = c(p.hover);
        t.button_hover = c(p.active);
        t.button_active = c(p.pressed);
        t.button_foreground = c(p.text_1);

        // Status fills (solid) and tonal status buttons (soft fill, coloured label).
        let status = |color: Rgba, soft: Rgba| (c(color), c(shift_l(color, 0.06)), c(shift_l(color, -0.08)), c(soft), c(color.alpha(0.22)), c(color.alpha(0.3)));
        (t.danger, t.danger_hover, t.danger_active, t.button_danger, t.button_danger_hover, t.button_danger_active) = status(p.danger, p.danger_soft);
        (t.warning, t.warning_hover, t.warning_active, t.button_warning, t.button_warning_hover, t.button_warning_active) = status(p.warning, p.warning_soft);
        (t.success, t.success_hover, t.success_active, t.button_success, t.button_success_hover, t.button_success_active) = status(p.success, p.success_soft);
        (t.info, t.info_hover, t.info_active, t.button_info, t.button_info_hover, t.button_info_active) = status(p.info, p.info_soft);
        (t.danger_foreground, t.warning_foreground, t.success_foreground, t.info_foreground) = (c(p.base), c(p.base), c(p.base), c(p.base));
        (t.button_danger_foreground, t.button_warning_foreground, t.button_success_foreground, t.button_info_foreground) = (c(p.danger), c(p.warning), c(p.success), c(p.info));
        (t.red, t.green, t.blue, t.yellow) = (c(p.danger), c(p.success), c(p.info), c(p.warning));

        t.link = c(p.accent);
        t.link_hover = c(p.accent_hover);
        t.link_active = c(p.accent_pressed);

        t.switch = c(p.track);
        t.switch_thumb = c(thumb);
        t.slider_bar = c(p.accent);
        t.slider_thumb = c(thumb);

        t.title_bar = c(p.base);
        t.title_bar_border = c(p.border);
        t.status_bar = c(p.base);
        t.status_bar_border = c(p.border);

        t.sidebar = c(p.sidebar);
        t.sidebar_foreground = c(p.text_1);
        t.sidebar_border = c(p.border);
        t.sidebar_accent = c(p.active);
        t.sidebar_accent_foreground = c(p.text_1);
        t.sidebar_primary = c(p.accent);
        t.sidebar_primary_foreground = c(p.on_accent);

        t.tab_bar = c(p.surface);
        t.tab_bar_segmented = c(p.well);
        t.tab = c(p.base.alpha(0.));
        t.tab_foreground = c(p.text_2);
        t.tab_active = c(p.surface_2);
        t.tab_active_foreground = c(p.text_1);

        // `Theme::list` is the list settings; the colour sits on `colors`.
        t.colors.list = c(p.base);
        t.list_even = c(p.base);
        t.list_head = c(p.surface);
        t.list_hover = c(p.hover);
        t.list_active = c(p.accent_soft);
        t.list_active_border = c(p.accent);
        t.table = c(p.base);
        t.table_even = c(p.base);
        t.table_head = c(p.surface);
        t.table_head_foreground = c(p.text_3);
        t.table_foot = c(p.surface);
        t.table_foot_foreground = c(p.text_3);
        t.table_hover = c(p.hover);
        t.table_active = c(p.accent_soft);
        t.table_active_border = c(p.accent);
        t.table_row_border = c(p.border);

        t.scrollbar = c(p.base.alpha(0.));
        t.scrollbar_thumb = c(p.track);
        t.scrollbar_thumb_hover = c(p.text_3);
    });
}

/// Type scale (px): 11 / 12.5 / 13.5 / 15 / 20 / 28. Spacing uses a 4-pt grid.
/// Prefer [`ramp`] for new text: it pairs every size with a line height.
pub mod size {
    pub const XS: f32 = 11.0;
    pub const SM: f32 = 12.5;
    pub const MD: f32 = 13.5;
    pub const LG: f32 = 15.0;
    pub const XL: f32 = 20.0;
    pub const XXL: f32 = 28.0;
    /// Horizontal page padding.
    pub const PAGE_X: f32 = 28.0;
    pub const PAGE_Y: f32 = 24.0;
}

/// Icon sizes (px): XS info-only glyphs, SM inline with text and in buttons, MD icon buttons and
/// nav, LG section headers, XL toolbars, HERO feature glyphs, EMPTY empty states.
pub mod icon {
    pub const XS: f32 = 12.0;
    pub const SM: f32 = 14.0;
    pub const MD: f32 = 16.0;
    pub const LG: f32 = 20.0;
    pub const XL: f32 = 24.0;
    pub const HERO: f32 = 32.0;
    pub const EMPTY: f32 = 40.0;
}

/// Corner radii (px): XS tags, SM chips and badges, MD buttons and inputs, LG cards and panels,
/// XL dialogs and sheets, PILL fully round.
pub mod radius {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const PILL: f32 = 999.0;
}

/// Spacing on a 4-pt grid (px). XXS is the one 2px nudge.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 20.0;
    pub const XXL: f32 = 24.0;
    pub const XXXL: f32 = 32.0;
    pub const HUGE: f32 = 48.0;
}

/// Motion durations, the same tiers gpui-component uses: FAST for hover and press feedback,
/// BASE for small enter/exit (menus, tooltips), SLOW for panels and page-level moves.
pub mod duration {
    use std::time::Duration;
    pub const FAST: Duration = Duration::from_millis(120);
    pub const BASE: Duration = Duration::from_millis(180);
    pub const SLOW: Duration = Duration::from_millis(280);
}

/// Easing curves for `Animation::with_easing`, matching gpui-component's motion tokens.
pub mod easing {
    use gpui_kit::base::Easing;

    fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
        Easing::CubicBezier { x1, y1, x2, y2 }.sample(t)
    }

    /// cubic-bezier(0.2, 0, 0, 1): anything moving or resizing on screen.
    pub fn standard(t: f32) -> f32 {
        bezier(0.2, 0., 0., 1., t)
    }

    /// cubic-bezier(0.16, 1, 0.3, 1): things appearing; fast start, long settle.
    pub fn enter(t: f32) -> f32 {
        bezier(0.16, 1., 0.3, 1., t)
    }

    /// cubic-bezier(0.4, 0, 1, 1): things leaving; accelerates away.
    pub fn exit(t: f32) -> f32 {
        bezier(0.4, 0., 1., 1., t)
    }
}

/// Fluent 2 type ramp: size and line height (px) with weight. Apply with `.ramp(ramp::BODY)`.
pub mod ramp {
    use gpui_kit::FontWeight;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct TypeStyle {
        pub size: f32,
        pub line_height: f32,
        pub weight: FontWeight,
    }

    const fn t(size: f32, line_height: f32, weight: FontWeight) -> TypeStyle {
        TypeStyle { size, line_height, weight }
    }

    /// Smallest text allowed: badges and dense metadata.
    pub const CAPTION_2: TypeStyle = t(11., 14., FontWeight::NORMAL);
    pub const CAPTION: TypeStyle = t(12., 16., FontWeight::NORMAL);
    pub const BODY: TypeStyle = t(14., 20., FontWeight::NORMAL);
    pub const BODY_STRONG: TypeStyle = t(14., 20., FontWeight::SEMIBOLD);
    pub const SUBTITLE_2: TypeStyle = t(16., 22., FontWeight::SEMIBOLD);
    pub const BODY_LARGE: TypeStyle = t(18., 24., FontWeight::NORMAL);
    pub const SUBTITLE: TypeStyle = t(20., 28., FontWeight::SEMIBOLD);
    pub const TITLE_3: TypeStyle = t(24., 32., FontWeight::SEMIBOLD);
    pub const TITLE: TypeStyle = t(28., 36., FontWeight::SEMIBOLD);
    pub const TITLE_LARGE: TypeStyle = t(40., 52., FontWeight::SEMIBOLD);
    pub const DISPLAY: TypeStyle = t(68., 92., FontWeight::SEMIBOLD);
}

/// `.ramp(ramp::BODY)` on any element sets text size, line height and weight together.
pub trait RampExt: Styled + Sized {
    fn ramp(self, s: ramp::TypeStyle) -> Self {
        self.text_size(px(s.size)).line_height(px(s.line_height)).font_weight(s.weight)
    }
}

impl<T: Styled> RampExt for T {}

#[cfg(test)]
mod tests {
    use super::{contrast, Palette, Rgba, DEFAULT_PRESET, PRESETS};

    fn all() -> Vec<(&'static str, Palette)> {
        PRESETS.iter().map(|p| (p.id, Palette::from_preset(p))).collect()
    }

    fn surfaces(p: &Palette) -> [(&'static str, Rgba); 6] {
        [("base", p.base), ("sidebar", p.sidebar), ("surface", p.surface), ("surface_2", p.surface_2), ("surface_3", p.surface_3), ("base+active", p.base.blend(p.active))]
    }

    #[test]
    fn default_preset_is_first_and_ids_unique() {
        assert_eq!(PRESETS[0].id, DEFAULT_PRESET);
        for (i, a) in PRESETS.iter().enumerate() {
            assert!(PRESETS[i + 1..].iter().all(|b| b.id != a.id), "duplicate preset id {}", a.id);
        }
    }

    #[test]
    fn mint_keeps_the_original_look() {
        let p = Palette::from_preset(&PRESETS[0]);
        let hex = |c: Rgba| u32::from(c);
        assert_eq!(hex(p.base), 0x0b0d10ff);
        assert_eq!(hex(p.sidebar), 0x0f1215ff);
        assert_eq!(hex(p.surface), 0x14181cff);
        assert_eq!(hex(p.surface_2), 0x1a1f24ff);
        assert_eq!(hex(p.accent), 0x2bd49aff);
        assert_eq!(hex(p.accent_soft), 0x2bd49a24);
        assert_eq!(hex(p.canvas), 0x07080aff);
    }

    /// WCAG 1.4.3: small text needs 4.5:1. text_1..3 on every surface of every preset.
    #[test]
    fn text_meets_wcag_aa() {
        for (id, p) in all() {
            for (name, bg) in surfaces(&p) {
                for (t, fg) in [("text_1", p.text_1), ("text_2", p.text_2), ("text_3", p.text_3)] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{id}: {t} on {name} is {r:.2}:1");
                }
            }
        }
    }

    /// WCAG 1.4.11: component boundaries and state indicators need 3:1.
    #[test]
    fn non_text_meets_3_to_1() {
        for (id, p) in all() {
            for (name, bg) in surfaces(&p) {
                assert!(contrast(p.track, bg) >= 3., "{id}: track on {name} is {:.2}:1", contrast(p.track, bg));
            }
            for (t, fg) in [("accent / focus ring", p.accent), ("favorite", p.favorite), ("danger", p.danger), ("warning", p.warning), ("success", p.success), ("info", p.info)] {
                let r = contrast(fg, p.base);
                assert!(r >= 3., "{id}: {t} on base is {r:.2}:1");
            }
            let r = contrast(p.on_accent, p.accent);
            assert!(r >= 4.5, "{id}: on_accent on accent is {r:.2}:1");
        }
    }

    /// Hues are non-text indicators (3:1 on every surface) and carry dark text (4.5:1).
    #[test]
    fn hues_read_on_every_surface() {
        for (id, p) in all() {
            for h in super::Hue::ALL {
                let c = p.hue(h);
                for (name, bg) in surfaces(&p) {
                    let r = contrast(c, bg);
                    assert!(r >= 3., "{id}: {h:?} on {name} is {r:.2}:1");
                }
                let r = contrast(p.base, c);
                assert!(r >= 4.5, "{id}: base text on {h:?} is {r:.2}:1");
            }
        }
    }

    /// Body text (text_1, text_2) stays WCAG AA (4.5:1) on the surfaces Settings draws it on,
    /// including a selected (accent-tinted) card and a hovered row, for every preset.
    #[test]
    fn body_text_reads_on_cards_and_selection() {
        for (id, p) in all() {
            let cards = [("surface", p.surface), ("surface+hover", p.surface.blend(p.hover)), ("surface+accent_soft", p.surface.blend(p.accent_soft)), ("glass_strong", p.base.blend(p.glass_strong))];
            for (name, bg) in cards {
                for (t, fg) in [("text_1", p.text_1), ("text_2", p.text_2)] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{id}: {t} on {name} is {r:.2}:1");
                }
            }
            // Primary text gets more than the minimum everywhere.
            assert!(contrast(p.text_1, p.surface) >= 7., "{id}: text_1 on surface is {:.2}:1 (want AAA 7:1)", contrast(p.text_1, p.surface));
        }
    }

    /// Both Appearance groups are filled, light presets are actually light (and dark ones dark),
    /// and text over glass stays readable whichever way round it is.
    #[test]
    fn groups_and_modes_are_coherent() {
        use super::{Mode, PresetGroup};
        for g in PresetGroup::ALL {
            assert!(PRESETS.iter().filter(|p| p.group == g).count() >= 5, "{g:?} has too few presets");
        }
        for p in PRESETS {
            let pal = Palette::from_preset(p);
            let lum_base = contrast(pal.base, super::rgb(0x000000));
            match p.mode {
                Mode::Light => assert!(lum_base > 10., "{}: a light preset needs a light base", p.id),
                Mode::Dark | Mode::HighContrast => assert!(lum_base < 3., "{}: a dark preset needs a dark base", p.id),
            }
            let over_glass = super::rgb(0x808080).blend(pal.glass);
            assert!(contrast(pal.text_1, over_glass) >= 4.5, "{}: text_1 over glass on a mid-grey image is {:.2}:1", p.id, contrast(pal.text_1, over_glass));
        }
    }

    #[test]
    #[ignore = "report only: cargo test -p mv-ui print_contrast_report -- --ignored --nocapture"]
    fn print_contrast_report() {
        let p = Palette::from_preset(&PRESETS[0]);
        let old_text_3 = super::rgb(0x7d8590);
        println!("old text_3 on base {:.2}:1, on surface_2 {:.2}:1", contrast(old_text_3, p.base), contrast(old_text_3, p.surface_2));
        println!("text_3 on base {:.2}:1, on surface_2 {:.2}:1, on surface_3 {:.2}:1", contrast(p.text_3, p.base), contrast(p.text_3, p.surface_2), contrast(p.text_3, p.surface_3));
        println!("track on base {:.2}:1, favorite on base {:.2}:1, accent on base {:.2}:1", contrast(p.track, p.base), contrast(p.favorite, p.base), contrast(p.accent, p.base));
        for (id, p) in all() {
            println!(
                "{id:18} min text_3 {:.2}  min track {:.2}  on_accent {:.2}  accent_hover {:?}",
                surfaces(&p).iter().map(|(_, bg)| contrast(p.text_3, *bg)).fold(f32::MAX, f32::min),
                surfaces(&p).iter().map(|(_, bg)| contrast(p.track, *bg)).fold(f32::MAX, f32::min),
                contrast(p.on_accent, p.accent),
                p.accent_hover
            );
        }
    }
}
