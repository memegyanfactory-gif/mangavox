//! Design tokens: one neutral ramp, one accent. Pages must use these, never raw colours.
use gpui_kit::{rgb, rgba, Rgba};

pub struct Tokens;

impl Tokens {
    pub fn base() -> Rgba { rgb(0x0b0d10) }
    pub fn sidebar() -> Rgba { rgb(0x0f1215) }
    pub fn surface() -> Rgba { rgb(0x14181c) }
    pub fn surface_2() -> Rgba { rgb(0x1a1f24) }
    pub fn border() -> Rgba { rgba(0xffffff12) }
    pub fn hover() -> Rgba { rgba(0xffffff0d) }
    pub fn active() -> Rgba { rgba(0xffffff17) }
    pub fn text_1() -> Rgba { rgb(0xf2f4f7) }
    pub fn text_2() -> Rgba { rgb(0xc3c9d1) }
    pub fn text_3() -> Rgba { rgb(0x7d8590) }
    pub fn accent() -> Rgba { rgb(0x2bd49a) }
    pub fn accent_soft() -> Rgba { rgba(0x2bd49a24) }
    pub fn danger() -> Rgba { rgb(0xf87171) }
    pub fn warning() -> Rgba { rgb(0xfbbf24) }
    pub fn scrim() -> Rgba { rgba(0x000000b3) }
}

/// Type scale (px): 11 / 12.5 / 13.5 / 15 / 20 / 28. Spacing uses a 4-pt grid.
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
