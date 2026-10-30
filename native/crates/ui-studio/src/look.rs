//! The Studio-wide **Look & branding** setting: how a chapter video is rendered (a preset plus
//! effect switches, passed to the renderer as `video_config["look"]`) and the logo / text
//! watermark the export adds to every video.
//!
//! File: `<data>/studio/video_look.json`. The export reads the same file on its own, so the keys
//! are a fixed contract:
//! `{"preset", "grade"?, "vignette"?, "grain"?, "glow"?, "impact"?, "logo": {on, path, pos, size,
//! opacity}, "text": {on, text, pos, size, opacity}}`. A missing effect key means "the preset's
//! own default". A picked logo is copied into `<data>/studio/brand/logo.<ext>`: it is the user's
//! own data and lives with the app data, never in the repo.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_ph, muted, ph, segmented, selected_badge, size, switch, tip, ButtonKind, Interactive as _, Ph, Tokens};
use serde_json::{json, Map, Value};

use crate::widgets::card;

// ------------------------------------------------------------------------------------ settings

/// How a chapter video is cut together (mirrors the renderer's presets).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    Clean,
    Cinematic,
    Recap,
    #[default]
    Dynamic,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Clean, Preset::Cinematic, Preset::Recap, Preset::Dynamic];

    /// The JSON value.
    pub fn key(self) -> &'static str {
        match self {
            Preset::Clean => "clean",
            Preset::Cinematic => "cinematic",
            Preset::Recap => "recap",
            Preset::Dynamic => "dynamic",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.key() == s.trim().to_ascii_lowercase())
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Clean => "Clean",
            Preset::Cinematic => "Cinematic",
            Preset::Recap => "Recap duo",
            Preset::Dynamic => "Dynamic",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Preset::Clean => "Single panel, gentle Ken Burns.",
            Preset::Cinematic => "Graded, glow, grain, soft fades.",
            Preset::Recap => "Tall panels side by side like the big manhwa recap channels: each new panel slides in.",
            Preset::Dynamic => "Duo panels plus punch zooms, shakes and white flashes on action beats, varied transitions.",
        }
    }

    /// The effects this preset turns on by itself (mirrors the Python renderer).
    pub fn effects(self) -> Effects {
        match self {
            Preset::Clean => Effects::default(),
            Preset::Cinematic => Effects { grade: true, vignette: true, grain: true, glow: true, impact: false },
            Preset::Recap => Effects { grade: true, vignette: true, ..Effects::default() },
            Preset::Dynamic => Effects { grade: true, vignette: true, grain: true, glow: true, impact: true },
        }
    }
}

/// One effect switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Grade,
    Glow,
    Vignette,
    Grain,
    Impact,
}

impl Effect {
    pub const ALL: [Effect; 5] = [Effect::Grade, Effect::Glow, Effect::Vignette, Effect::Grain, Effect::Impact];

    /// The JSON key.
    pub fn key(self) -> &'static str {
        match self {
            Effect::Grade => "grade",
            Effect::Glow => "glow",
            Effect::Vignette => "vignette",
            Effect::Grain => "grain",
            Effect::Impact => "impact",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Effect::Grade => "Colour grade",
            Effect::Glow => "Glow",
            Effect::Vignette => "Vignette",
            Effect::Grain => "Film grain",
            Effect::Impact => "Action punch (zoom + shake + flash)",
        }
    }

    fn icon(self) -> Ph {
        match self {
            Effect::Grade => ph::PALETTE,
            Effect::Glow => ph::SUN,
            Effect::Vignette => ph::CIRCLE_DASHED,
            Effect::Grain => ph::FILM_STRIP,
            Effect::Impact => ph::LIGHTNING,
        }
    }

    fn index(self) -> usize {
        Effect::ALL.iter().position(|e| *e == self).unwrap_or(0)
    }
}

/// The effective effect booleans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Effects {
    pub grade: bool,
    pub vignette: bool,
    pub grain: bool,
    pub glow: bool,
    pub impact: bool,
}

impl Effects {
    pub fn get(&self, e: Effect) -> bool {
        match e {
            Effect::Grade => self.grade,
            Effect::Glow => self.glow,
            Effect::Vignette => self.vignette,
            Effect::Grain => self.grain,
            Effect::Impact => self.impact,
        }
    }
}

/// Where an overlay sits: the nine anchors of a 3x3 grid, row by row.
pub const POSITIONS: [&str; 9] = ["tl", "tc", "tr", "ml", "mc", "mr", "bl", "bc", "br"];
const POSITION_NAMES: [&str; 9] = ["Top left", "Top", "Top right", "Left", "Centre", "Right", "Bottom left", "Bottom", "Bottom right"];

fn position_of(v: &Value, d: usize) -> usize {
    v.get("pos").and_then(Value::as_str).and_then(|s| POSITIONS.iter().position(|p| *p == s)).unwrap_or(d)
}

fn f32_in(v: &Value, key: &str, d: f32, lo: f32, hi: f32) -> f32 {
    v.get(key).and_then(Value::as_f64).map(|x| (x as f32).clamp(lo, hi)).unwrap_or(d)
}

fn round3(x: f32) -> f64 {
    (x as f64 * 1000.).round() / 1000.
}

/// The logo mark.
#[derive(Clone, Debug, PartialEq)]
pub struct Logo {
    pub on: bool,
    /// The copy in `<data>/studio/brand/`.
    pub path: Option<PathBuf>,
    /// Index into [`POSITIONS`].
    pub pos: usize,
    /// Width as a fraction of the frame width (0.04..0.4).
    pub size: f32,
    pub opacity: f32,
}

impl Default for Logo {
    fn default() -> Self {
        Self { on: false, path: None, pos: 2, size: 0.12, opacity: 0.85 }
    }
}

impl Logo {
    fn to_json(&self) -> Value {
        json!({
            "on": self.on,
            "path": self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            "pos": POSITIONS[self.pos.min(8)],
            "size": round3(self.size),
            "opacity": round3(self.opacity),
        })
    }

    fn from_json(v: &Value) -> Self {
        let d = Self::default();
        Self {
            on: v.get("on").and_then(Value::as_bool).unwrap_or(d.on),
            path: v.get("path").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(PathBuf::from),
            pos: position_of(v, d.pos),
            size: f32_in(v, "size", d.size, 0.04, 0.4),
            opacity: f32_in(v, "opacity", d.opacity, 0., 1.),
        }
    }
}

/// The text watermark.
#[derive(Clone, Debug, PartialEq)]
pub struct Watermark {
    pub on: bool,
    pub text: String,
    /// Index into [`POSITIONS`].
    pub pos: usize,
    /// Font size as a fraction of the frame height (0.02..0.12).
    pub size: f32,
    pub opacity: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self { on: false, text: String::new(), pos: 0, size: 0.05, opacity: 0.6 }
    }
}

impl Watermark {
    fn to_json(&self) -> Value {
        json!({
            "on": self.on,
            "text": self.text,
            "pos": POSITIONS[self.pos.min(8)],
            "size": round3(self.size),
            "opacity": round3(self.opacity),
        })
    }

    fn from_json(v: &Value) -> Self {
        let d = Self::default();
        Self {
            on: v.get("on").and_then(Value::as_bool).unwrap_or(d.on),
            text: v.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
            pos: position_of(v, d.pos),
            size: f32_in(v, "size", d.size, 0.02, 0.12),
            opacity: f32_in(v, "opacity", d.opacity, 0., 1.),
        }
    }
}

/// The whole Look & branding setting.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Look {
    pub preset: Preset,
    /// Per effect (in [`Effect::ALL`] order): `None` = the preset's own default.
    pub overrides: [Option<bool>; 5],
    pub logo: Logo,
    pub text: Watermark,
}

impl Look {
    /// The effects a render uses: the preset's, with the user's switches on top.
    pub fn effects(&self) -> Effects {
        let base = self.preset.effects();
        let pick = |e: Effect| self.overrides[e.index()].unwrap_or(base.get(e));
        Effects { grade: pick(Effect::Grade), vignette: pick(Effect::Vignette), grain: pick(Effect::Grain), glow: pick(Effect::Glow), impact: pick(Effect::Impact) }
    }

    /// Turns one effect on or off (an override only when it differs from the preset).
    pub fn set_effect(&mut self, e: Effect, on: bool) {
        self.overrides[e.index()] = (self.preset.effects().get(e) != on).then_some(on);
    }

    /// Picks a preset; its effects start from the preset's own defaults.
    pub fn set_preset(&mut self, p: Preset) {
        self.preset = p;
        self.overrides = [None; 5];
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("preset".into(), json!(self.preset.key()));
        for e in Effect::ALL {
            if let Some(on) = self.overrides[e.index()] {
                m.insert(e.key().into(), json!(on));
            }
        }
        m.insert("logo".into(), self.logo.to_json());
        m.insert("text".into(), self.text.to_json());
        Value::Object(m)
    }

    pub fn from_json(v: &Value) -> Self {
        let mut overrides = [None; 5];
        for e in Effect::ALL {
            overrides[e.index()] = v.get(e.key()).and_then(Value::as_bool);
        }
        Self {
            preset: v.get("preset").and_then(Value::as_str).and_then(Preset::parse).unwrap_or_default(),
            overrides,
            logo: v.get("logo").map(Logo::from_json).unwrap_or_default(),
            text: v.get("text").map(Watermark::from_json).unwrap_or_default(),
        }
    }

    /// `video_config["look"]`: the preset and the effective effect booleans.
    pub fn render_config(&self) -> Value {
        let fx = self.effects();
        json!({
            "preset": self.preset.key(),
            "grade": fx.grade,
            "vignette": fx.vignette,
            "grain": fx.grain,
            "glow": fx.glow,
            "impact": fx.impact,
        })
    }

    pub fn load_from(file: &Path) -> Self {
        std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object).map(|v| Self::from_json(&v)).unwrap_or_default()
    }

    pub fn save_to(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.to_json()).map_err(std::io::Error::other)?;
        std::fs::write(file, text)
    }

    /// The saved setting (defaults when there is none).
    pub fn load() -> Self {
        Self::load_from(&look_file())
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&look_file())
    }
}

/// `<data>/studio/video_look.json`.
pub fn look_file() -> PathBuf {
    mv_core::paths::studio().join("video_look.json")
}

/// `<data>/studio/brand`: where the picked logo is kept.
pub fn brand_dir() -> PathBuf {
    mv_core::paths::studio().join("brand")
}

/// `video_config["look"]` for a render, from the saved setting.
pub fn video_config() -> Value {
    Look::load().render_config()
}

/// Image types a logo can be.
pub const LOGO_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn image_ext(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).filter(|e| LOGO_EXTS.contains(&e.as_str()))
}

/// Copies a picked image into `dir` as `logo.<ext>` (replacing an older logo) and returns the copy.
pub fn import_logo_into(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let ext = image_ext(src).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "pick a PNG, JPG or WEBP image"))?;
    let dest = dir.join(format!("logo.{ext}"));
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y);
    if same(src, &dest) {
        return Ok(dest);
    }
    std::fs::create_dir_all(dir)?;
    let bytes = std::fs::read(src)?;
    for e in LOGO_EXTS {
        let old = dir.join(format!("logo.{e}"));
        if old != dest {
            let _ = std::fs::remove_file(old);
        }
    }
    std::fs::write(&dest, bytes)?;
    Ok(dest)
}

/// [`import_logo_into`] the Studio brand folder.
pub fn import_logo(src: &Path) -> std::io::Result<PathBuf> {
    import_logo_into(src, &brand_dir())
}

// ------------------------------------------------------------------------------------ the panel

const LOGO_SIZES: [f32; 4] = [0.08, 0.12, 0.16, 0.22];
const LOGO_OPACITY: [f32; 3] = [0.5, 0.75, 1.0];
const TEXT_SIZES: [f32; 3] = [0.035, 0.05, 0.07];
const TEXT_OPACITY: [f32; 3] = [0.4, 0.6, 0.8];

/// The option nearest `v`.
fn nearest(options: &[f32], v: f32) -> usize {
    options.iter().enumerate().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs())).map(|(i, _)| i).unwrap_or(0)
}

fn fade(c: Rgba, a: f32) -> Hsla {
    let mut h: Hsla = c.into();
    h.a *= a;
    h
}

/// The loaded logo for the thumbnail and preview: the image (from bytes, so a replaced
/// `logo.png` never shows the old cached picture) and its width / height.
struct LogoImage {
    image: Arc<Image>,
    aspect: f32,
}

fn load_logo_image(path: &Path) -> Option<LogoImage> {
    let format = match image_ext(path)?.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        _ => ImageFormat::Webp,
    };
    let bytes = std::fs::read(path).ok()?;
    let aspect = image::load_from_memory(&bytes).ok().map(|i| i.width() as f32 / i.height().max(1) as f32).unwrap_or(1.).clamp(0.2, 5.);
    Some(LogoImage { image: Arc::new(Image::from_bytes(format, bytes)), aspect })
}

/// The Look & branding card of the Video studio.
pub(crate) struct LookPanel {
    look: Look,
    text: Entity<InputState>,
    logo: Option<LogoImage>,
    _subs: Vec<Subscription>,
}

impl LookPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let look = Look::load();
        let initial = look.text.text.clone();
        let text = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("e.g. @yourchannel");
            s.set_value(initial, window, cx);
            s
        });
        let subs = vec![cx.subscribe_in(&text, window, |this: &mut Self, st, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                let value = st.read(cx).value().to_string();
                if value != this.look.text.text {
                    this.edit(|l| l.text.text = value, cx);
                }
            }
        })];
        let logo = look.logo.path.as_deref().and_then(load_logo_image);
        Self { look, text, logo, _subs: subs }
    }

    fn edit(&mut self, f: impl FnOnce(&mut Look), cx: &mut Context<Self>) {
        f(&mut self.look);
        if let Err(e) = self.look.save() {
            toast::show(cx, Toast::error(format!("Could not save the look: {e}")));
        }
        cx.notify();
    }

    fn choose_logo(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this image".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(src) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| match import_logo(&src) {
                Ok(dest) => {
                    this.logo = load_logo_image(&dest);
                    this.edit(
                        |l| {
                            l.logo.path = Some(dest);
                            l.logo.on = true;
                        },
                        cx,
                    );
                }
                Err(e) => toast::show(cx, Toast::warning(format!("Could not use that logo: {e}"))),
            });
        })
        .detach();
    }

    fn remove_logo(&mut self, cx: &mut Context<Self>) {
        self.logo = None;
        self.edit(
            |l| {
                l.logo.path = None;
                l.logo.on = false;
            },
            cx,
        );
    }

    // -------------------------------------------------------------- pieces

    fn tiles(&self, cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().gap(px(space::SM)).children(Preset::ALL.into_iter().map(|p| {
            let on = self.look.preset == p;
            div()
                .id(("look-preset", p as usize))
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(206.))
                .p(px(6.))
                .rounded(px(radius::LG))
                .border_2()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface() })
                .interactive()
                .tooltip(tip(p.blurb()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(|l| l.set_preset(p), cx)))
                .child(div().relative().child(schematic(p, 192., 108.)).when(on, |d| d.child(div().absolute().top(px(6.)).left(px(6.)).child(selected_badge()))))
                .child(div().px(px(2.)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(p.label()))
                .child(div().px(px(2.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(p.blurb()))
        }))
    }

    fn effects(&self, cx: &mut Context<Self>) -> Div {
        let fx = self.look.effects();
        let base = self.look.preset.effects();
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .w(px(280.))
            .flex_none()
            .child(section(ph::SLIDERS_HORIZONTAL, "Effects"))
            .children(Effect::ALL.into_iter().map(|e| {
                let changed = fx.get(e) != base.get(e);
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_size(px(size::SM))
                            .text_color(Tokens::text_2())
                            .child(icon_ph(e.icon(), Tokens::text_3(), icon_px::SM))
                            .child(e.label())
                            .when(changed, |d| d.child(div().size(px(6.)).rounded_full().bg(Tokens::accent()))),
                    )
                    .child(switch(("look-fx", e.index()), fx.get(e)).on_click(cx.listener(move |this, on: &bool, _, cx| this.edit(|l| l.set_effect(e, *on), cx))))
            }))
            .child(muted("A dot marks a switch changed from the look's own. Picking a look resets them."))
    }

    fn logo_controls(&self, cx: &mut Context<Self>) -> Div {
        let l = self.look.logo.clone();
        let thumb = div()
            .size(px(56.))
            .flex_none()
            .rounded(px(radius::MD))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .map(|d| match &self.logo {
                Some(li) => d.child(img(li.image.clone()).size_full().object_fit(ObjectFit::Contain)),
                None => d.child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD)),
            });
        let has = l.path.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .w(px(300.))
            .flex_none()
            .child(div().flex().items_center().justify_between().child(section(ph::IMAGE, "Logo")).child(switch("look-logo-on", l.on).on_click(cx.listener(|this, on: &bool, _, cx| this.edit(|l| l.logo.on = *on, cx)))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(thumb)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(btn("look-logo-pick", if has { "Replace image…" } else { "Choose image…" }, ButtonKind::Secondary).small().icon(ph::FOLDER_OPEN).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.choose_logo(cx))))
                            .when(has, |d| d.child(btn("look-logo-remove", "Remove", ButtonKind::Ghost).small().icon(ph::X).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.remove_logo(cx)))))
                            .when(!has, |d| d.child(muted("PNG with transparency works best."))),
                    ),
            )
            .child(field("Position", position_grid("look-logo-pos", l.pos, cx, |l, ix| l.logo.pos = ix).into_any_element()))
            .child(field(
                "Size",
                segmented("look-logo-size", ["S", "M", "L", "XL"], nearest(&LOGO_SIZES, l.size))
                    .small()
                    .on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|l| l.logo.size = LOGO_SIZES[(*ix).min(3)], cx)))
                    .into_any_element(),
            ))
            .child(field(
                "Opacity",
                segmented("look-logo-opacity", ["50%", "75%", "100%"], nearest(&LOGO_OPACITY, l.opacity))
                    .small()
                    .on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|l| l.logo.opacity = LOGO_OPACITY[(*ix).min(2)], cx)))
                    .into_any_element(),
            ))
    }

    fn text_controls(&self, cx: &mut Context<Self>) -> Div {
        let t = self.look.text.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .w(px(300.))
            .flex_none()
            .child(div().flex().items_center().justify_between().child(section(ph::TEXT_T, "Text watermark")).child(switch("look-text-on", t.on).on_click(cx.listener(|this, on: &bool, _, cx| this.edit(|l| l.text.on = *on, cx)))))
            .child(Input::new(&self.text).small())
            .child(field("Position", position_grid("look-text-pos", t.pos, cx, |l, ix| l.text.pos = ix).into_any_element()))
            .child(field(
                "Size",
                segmented("look-text-size", ["S", "M", "L"], nearest(&TEXT_SIZES, t.size))
                    .small()
                    .on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|l| l.text.size = TEXT_SIZES[(*ix).min(2)], cx)))
                    .into_any_element(),
            ))
            .child(field(
                "Opacity",
                segmented("look-text-opacity", ["40%", "60%", "80%"], nearest(&TEXT_OPACITY, t.opacity))
                    .small()
                    .on_change(cx.listener(|this, ix: &usize, _, cx| this.edit(|l| l.text.opacity = TEXT_OPACITY[(*ix).min(2)], cx)))
                    .into_any_element(),
            ))
    }

    /// A small 16:9 frame: the look's layout with the logo and text where the export puts them.
    fn preview(&self) -> Div {
        let (fw, fh) = (288., 162.);
        let margin = fw * 0.03;
        let l = &self.look.logo;
        let t = &self.look.text;
        let logo_w = fw * l.size;
        let logo = l.on.then(|| match &self.logo {
            Some(li) => img(li.image.clone()).w(px(logo_w)).h(px(logo_w / li.aspect)).object_fit(ObjectFit::Contain).opacity(l.opacity).into_any_element(),
            None => div()
                .w(px(logo_w))
                .h(px(logo_w * 0.5))
                .rounded(px(3.))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(Tokens::text_2())
                .text_size(px((logo_w * 0.18).max(7.)))
                .text_color(Tokens::text_2())
                .opacity(l.opacity)
                .child("LOGO")
                .into_any_element(),
        });
        let words = if t.text.trim().is_empty() { "Your text".to_string() } else { t.text.clone() };
        let text = t.on.then(|| div().whitespace_nowrap().text_size(px((fh * t.size).max(6.))).font_weight(FontWeight::BOLD).text_color(fade(Tokens::text_1(), t.opacity)).child(words).into_any_element());
        let anchored = |pos: usize, el: AnyElement| {
            let (col, row) = (pos % 3, pos / 3);
            let d = div().absolute().inset_0().p(px(margin)).flex();
            let d = match col {
                0 => d.justify_start(),
                1 => d.justify_center(),
                _ => d.justify_end(),
            };
            let d = match row {
                0 => d.items_start(),
                1 => d.items_center(),
                _ => d.items_end(),
            };
            d.child(el)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(section(ph::MONITOR_PLAY, "Preview"))
            .child(
                div()
                    .relative()
                    .w(px(fw))
                    .h(px(fh))
                    .rounded(px(radius::MD))
                    .overflow_hidden()
                    .border_1()
                    .border_color(Tokens::border_strong())
                    .child(schematic(self.look.preset, fw, fh))
                    .when_some(logo, |d, el| d.child(anchored(l.pos, el)))
                    .when_some(text, |d, el| d.child(anchored(t.pos, el))),
            )
            .child(muted("Where the logo and text sit on a 16:9 export."))
    }
}

/// A small section heading.
fn section(icon: Ph, text: &'static str) -> Div {
    div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(text)
}

/// A labelled control.
fn field(label: &'static str, control: AnyElement) -> Div {
    div().flex().flex_col().gap(px(4.)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(label)).child(control)
}

/// The 3x3 anchor grid (like the caption anchors in `video`).
fn position_grid(id: &'static str, current: usize, cx: &mut Context<LookPanel>, set: fn(&mut Look, usize)) -> Div {
    let mut grid = div().flex().flex_col().gap(px(4.));
    for r in 0..3 {
        let mut line = div().flex().gap(px(4.));
        for c in 0..3 {
            let ix = r * 3 + c;
            let on = ix == current;
            line = line.child(
                div()
                    .id((id, ix))
                    .size(px(24.))
                    .rounded(px(radius::SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if on { Tokens::accent() } else { Tokens::border() })
                    .bg(if on { Tokens::accent_soft() } else { Tokens::surface_2() })
                    .interactive()
                    .tooltip(tip(POSITION_NAMES[ix]))
                    .child(div().size(px(6.)).rounded_full().bg(if on { Tokens::accent() } else { Tokens::text_3() }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.edit(|l| set(l, ix), cx))),
            );
        }
        grid = grid.child(line);
    }
    grid
}

/// A panel block in a schematic.
fn panel(x: f32, y: f32, w: f32, h: f32, fill: Hsla) -> Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded(px(2.)).bg(fill).border_1().border_color(Tokens::border_strong())
}

/// The preset drawn with divs on a `w` x `h` frame.
fn schematic(p: Preset, w: f32, h: f32) -> Div {
    let frame = div().relative().w(px(w)).h(px(h)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::well());
    let page = fade(Tokens::surface_3(), 1.);
    let hint = |icon: Ph, x: f32, y: f32| div().absolute().left(px(x)).top(px(y)).child(icon_ph(icon, Tokens::accent(), icon_px::SM));
    match p {
        Preset::Clean => {
            let (pw, phh) = (w * 0.34, h * 0.8);
            frame.child(panel((w - pw) / 2., (h - phh) / 2., pw, phh, page)).child(
                // The Ken Burns crop: a slightly smaller frame inside the panel.
                div().absolute().left(px((w - pw) / 2. + pw * 0.08)).top(px((h - phh) / 2. + phh * 0.08)).w(px(pw * 0.84)).h(px(phh * 0.84)).rounded(px(2.)).border_1().border_color(Tokens::accent()),
            )
        }
        Preset::Cinematic => {
            let bar = h * 0.12;
            frame
                .child(panel(w * 0.18, bar + h * 0.06, w * 0.64, h - 2. * bar - h * 0.12, fade(Tokens::accent(), 0.28)))
                .child(div().absolute().left_0().top_0().w(px(w)).h(px(bar)).bg(Tokens::canvas()))
                .child(div().absolute().left_0().bottom_0().w(px(w)).h(px(bar)).bg(Tokens::canvas()))
                .child(div().absolute().inset_0().border_4().border_color(fade(Tokens::canvas(), 0.55)))
                .child(hint(ph::SPARKLE, w - 24., bar + 4.))
        }
        Preset::Recap => {
            let (pw, phh) = (w * 0.27, h * 0.86);
            let y = (h - phh) / 2.;
            frame
                .child(panel(w / 2. - pw - 4., y, pw, phh, page))
                .child(panel(w / 2. + 4., y, pw, phh, fade(Tokens::accent(), 0.28)))
                .child(hint(ph::ARROW_LEFT, w / 2. + pw + 8., h / 2. - 8.))
        }
        Preset::Dynamic => {
            let (pw, phh) = (w * 0.27, h * 0.86);
            let y = (h - phh) / 2.;
            frame
                .child(panel(w / 2. - pw - 4., y, pw, phh, page))
                .child(panel(w / 2. + 4., y - h * 0.04, pw * 1.08, phh * 1.08, fade(Tokens::accent(), 0.28)))
                .child(div().absolute().inset_0().bg(fade(Tokens::text_1(), 0.12)))
                .child(hint(ph::LIGHTNING, w / 2. + pw * 1.08 + 10., h * 0.18))
                .child(hint(ph::ARROWS_OUT, w / 2. + pw * 1.08 + 10., h * 0.62))
        }
    }
}

impl Render for LookPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let note = |text: &'static str| div().flex().items_center().gap(px(6.)).child(icon_ph(ph::INFO, Tokens::text_3(), icon_px::SM)).child(muted(text));
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(size::LG)).text_color(Tokens::text_1()).child("Look & branding"))
                    .child(muted("Studio-wide · every chapter")),
            )
            .child(self.tiles(cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::XL))
                    .child(self.effects(cx))
                    .child(self.logo_controls(cx))
                    .child(self.text_controls(cx))
                    .child(self.preview()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(note("The look applies when a chapter video is made (use Redo → Video on made chapters)."))
                    .child(note("Logo and text are added to every export.")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{import_logo_into, nearest, Effect, Effects, Look, Preset, LOGO_SIZES};
    use serde_json::json;

    #[test]
    fn defaults_match_the_contract() {
        let l = Look::default();
        assert_eq!(l.preset, Preset::Dynamic);
        assert_eq!(l.effects(), Effects { grade: true, vignette: true, grain: true, glow: true, impact: true });
        assert_eq!((l.logo.pos, l.logo.size, l.logo.opacity), (2, 0.12, 0.85));
        assert_eq!((l.text.pos, l.text.size, l.text.opacity), (0, 0.05, 0.6));
        let v = l.to_json();
        assert_eq!(v["preset"], "dynamic");
        assert_eq!(v["logo"]["pos"], "tr");
        assert_eq!(v["text"]["pos"], "tl");
        // No overrides: the effect keys are absent.
        for e in Effect::ALL {
            assert!(v.get(e.key()).is_none(), "{} written without an override", e.key());
        }
    }

    #[test]
    fn preset_effects_mirror_the_renderer() {
        assert_eq!(Preset::Clean.effects(), Effects::default());
        assert_eq!(Preset::Cinematic.effects(), Effects { grade: true, vignette: true, grain: true, glow: true, impact: false });
        assert_eq!(Preset::Recap.effects(), Effects { grade: true, vignette: true, grain: false, glow: false, impact: false });
        assert_eq!(Preset::Dynamic.effects(), Effects { grade: true, vignette: true, grain: true, glow: true, impact: true });
    }

    #[test]
    fn video_config_has_the_effective_booleans() {
        let mut l = Look::default();
        l.set_preset(Preset::Recap);
        l.set_effect(Effect::Grain, true);
        l.set_effect(Effect::Grade, false);
        assert_eq!(l.render_config(), json!({"preset": "recap", "grade": false, "vignette": true, "grain": true, "glow": false, "impact": false}));
        // Setting a switch back to the preset's own value drops the override.
        l.set_effect(Effect::Grade, true);
        assert_eq!(l.overrides, [None, None, None, Some(true), None]);
        // A new preset starts from its own defaults.
        l.set_preset(Preset::Clean);
        assert_eq!(l.render_config(), json!({"preset": "clean", "grade": false, "vignette": false, "grain": false, "glow": false, "impact": false}));
    }

    #[test]
    fn absent_keys_fall_back_and_junk_is_clamped() {
        let l = Look::from_json(&json!({"preset": "cinematic", "impact": true, "logo": {"pos": "zz", "size": 9}, "text": {"pos": "bc", "size": 0.001, "opacity": 3}}));
        assert_eq!(l.preset, Preset::Cinematic);
        assert!(l.effects().impact && l.effects().grain);
        assert_eq!((l.logo.pos, l.logo.size), (2, 0.4));
        assert_eq!((l.text.pos, l.text.size, l.text.opacity), (7, 0.02, 1.0));
        assert_eq!(Look::from_json(&json!({"preset": "nope"})).preset, Preset::Dynamic);
    }

    #[test]
    fn load_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("studio").join("video_look.json");
        assert_eq!(Look::load_from(&file), Look::default(), "no file: defaults");
        let mut l = Look::default();
        l.set_preset(Preset::Cinematic);
        l.set_effect(Effect::Impact, true);
        l.logo.on = true;
        l.logo.path = Some(dir.path().join("brand").join("logo.png"));
        l.logo.pos = 8;
        l.logo.size = 0.22;
        l.logo.opacity = 0.5;
        l.text.on = true;
        l.text.text = "@mangavox".into();
        l.text.pos = 7;
        l.text.size = 0.035;
        l.text.opacity = 0.8;
        l.save_to(&file).unwrap();
        assert_eq!(Look::load_from(&file), l);
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(raw["preset"], "cinematic");
        assert_eq!(raw["impact"], true);
        assert_eq!(raw["logo"]["pos"], "br");
        assert_eq!(raw["text"]["pos"], "bc");
        assert_eq!(raw["text"]["text"], "@mangavox");
        // A broken file reads as defaults.
        std::fs::write(&file, "{nope").unwrap();
        assert_eq!(Look::load_from(&file), Look::default());
    }

    #[test]
    fn logo_is_copied_into_the_brand_folder() {
        let dir = tempfile::tempdir().unwrap();
        let brand = dir.path().join("brand");
        let src = dir.path().join("My Logo.PNG");
        std::fs::write(&src, b"png-bytes").unwrap();
        let dest = import_logo_into(&src, &brand).unwrap();
        assert_eq!(dest, brand.join("logo.png"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"png-bytes");
        // A new logo of another type replaces the old one.
        let jpg = dir.path().join("b.jpg");
        std::fs::write(&jpg, b"jpg-bytes").unwrap();
        assert_eq!(import_logo_into(&jpg, &brand).unwrap(), brand.join("logo.jpg"));
        assert!(!brand.join("logo.png").exists());
        // Picking the copy itself again keeps it.
        assert_eq!(import_logo_into(&brand.join("logo.jpg"), &brand).unwrap(), brand.join("logo.jpg"));
        assert_eq!(std::fs::read(brand.join("logo.jpg")).unwrap(), b"jpg-bytes");
        assert!(import_logo_into(&dir.path().join("notes.txt"), &brand).is_err());
    }

    #[test]
    fn size_options_pick_the_nearest() {
        assert_eq!(nearest(&LOGO_SIZES, 0.12), 1);
        assert_eq!(nearest(&LOGO_SIZES, 0.3), 3);
    }
}
