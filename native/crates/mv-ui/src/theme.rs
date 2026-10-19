//! Design tokens. Every colour comes from the live [`Palette`] (one per Settings → Appearance
//! preset); pages use `Tokens::*()`, never raw colours. Size scales live here too.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use gpui_kit::component::Theme;
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
}

/// One Settings → Appearance preset: id, copy and the three swatch colours (base, accent, secondary).
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub base: u32,
    pub accent: u32,
    pub secondary: u32,
}

/// The original MangaVox look. Must stay first in [`PRESETS`].
pub const DEFAULT_PRESET: &str = "mangavox-mint";

/// Theme presets (ids shared with the Electron app's `src/shared/appearance.ts`, plus Mint).
pub const PRESETS: &[Preset] = &[
    Preset { id: "mangavox-mint", name: "MangaVox Mint", description: "Signature mint and sky on cool graphite", base: 0x0b0d10, accent: 0x2bd49a, secondary: 0x7dd3fc },
    Preset { id: "rust", name: "Rust", description: "Oxidized orange on warm charcoal — forged metal", base: 0x0e0907, accent: 0xc8501a, secondary: 0xf0b27a },
    Preset { id: "watchfiwn", name: "Watchfiwn", description: "Cinematic red on true black — streaming-grade", base: 0x0b0b0c, accent: 0xe8392e, secondary: 0xff6b5f },
    Preset { id: "mangavox-emerald", name: "MangaVox Emerald", description: "Emerald and cyan studio lighting", base: 0x07090d, accent: 0x10b981, secondary: 0x60a5fa },
    Preset { id: "chatgpt-green", name: "ChatGPT Green", description: "Charcoal, mint, and connected signals", base: 0x080b0a, accent: 0x10a37f, secondary: 0x9ce6d1 },
    Preset { id: "claude-warm", name: "Claude Warm", description: "Warm terracotta on deep charcoal", base: 0x0d0b0a, accent: 0xd97757, secondary: 0xe8c7a8 },
    Preset { id: "gemini-prism", name: "Gemini Prism", description: "Blue, violet, and cyan refractions", base: 0x080a12, accent: 0x5b8def, secondary: 0xa78bfa },
    Preset { id: "amber-terminal", name: "Amber Terminal", description: "Black glass and restrained amber data", base: 0x090806, accent: 0xf59e0b, secondary: 0xfde68a },
    Preset { id: "cyberpunk", name: "Cyberpunk", description: "Cyan and magenta over graphite", base: 0x07090c, accent: 0x22d3ee, secondary: 0xf472b6 },
    Preset { id: "sakura", name: "Sakura", description: "Rose highlights and drifting petals", base: 0x0d090c, accent: 0xfb7185, secondary: 0xf9a8d4 },
    Preset { id: "ocean", name: "Ocean", description: "Deep neutral water and clear cyan", base: 0x060b0e, accent: 0x06b6d4, secondary: 0x38bdf8 },
    Preset { id: "monochrome", name: "Monochrome", description: "Graphite, silver, and quiet motion", base: 0x090a0b, accent: 0xb8c0cc, secondary: 0x7f8ea3 },
    Preset { id: "aurora", name: "Aurora", description: "Green, blue, and violet circular flow", base: 0x070a0c, accent: 0x2dd4bf, secondary: 0x818cf8 },
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
}

/// Soft fills are 14% of their colour, borders 35% (the same steps as the original accent_soft).
const SOFT: u8 = 0x24;
const EDGE: u8 = 0x59;

impl Palette {
    /// Builds the palette from a preset's base and accent: surfaces lift the base in even steps
    /// (keeping its tint), accent states shift its lightness, status colours are shared.
    pub fn from_preset(p: &Preset) -> Self {
        let base = rgb(p.base);
        let accent = rgb(p.accent);
        let [sidebar, surface, surface_2, surface_3] = neutrals(p);
        let text_1 = rgb(0xf2f4f7);
        let text_3 = rgb(0x8b949e);
        let (success, info, warning, danger) = (rgb(0x4ade80), rgb(0x60a5fa), rgb(0xfbbf24), rgb(0xf87171));
        Self {
            canvas: scale(base, 0.65),
            base,
            sidebar,
            surface,
            surface_2,
            surface_3,
            well: rgba(0x00000038),
            border: rgba(0xffffff12),
            border_strong: rgba(0xffffff26),
            hover: rgba(0xffffff0d),
            active: rgba(0xffffff17),
            active_hover: rgba(0xffffff1f),
            pressed: rgba(0xffffff24),
            focus_ring: accent,
            track: rgba(0xffffff5c),
            text_1,
            text_2: rgb(0xc3c9d1),
            text_3,
            text_disabled: a8(text_3, 0x66),
            accent,
            accent_hover: shift_l(accent, 0.07),
            accent_pressed: shift_l(accent, -0.06),
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
            favorite: rgb(0xfacc15),
            glass: rgba(0x0000008c),
            glass_strong: a8(surface, 0xf0),
            glass_border: rgba(0xffffff1a),
            overlay: rgba(0x0000004d),
            scrim: rgba(0x000000b3),
        }
    }
}

/// Sidebar, surface, surface_2 and surface_3. Mint keeps its original hand-tuned ramp.
fn neutrals(p: &Preset) -> [Rgba; 4] {
    if p.id == DEFAULT_PRESET {
        return [rgb(0x0f1215), rgb(0x14181c), rgb(0x1a1f24), rgb(0x21272d)];
    }
    let base = rgb(p.base);
    [4.7, 10.7, 17.7, 25.7].map(|k| lift(base, k))
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
    Theme::update(cx, |t| {
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
        t.switch_thumb = c(p.text_1);
        t.slider_bar = c(p.accent);
        t.slider_thumb = c(p.text_1);

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
