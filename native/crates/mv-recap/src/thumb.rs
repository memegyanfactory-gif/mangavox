//! 16:9 thumbnails for the videos Studio makes of a manga.
//!
//! Credits matter (an AI image costs ~75 Magnific credits), so a manga gets exactly ONE AI image,
//! made from its cover and kept in its Studio folder ([`BASE_AI`]); it is only made again when the
//! user asks for it (Regenerate). Every video of the manga, each chapter and the continuous one,
//! uses that same image; what differs is a small badge drawn here, locally, in the top-right
//! corner: "Chapter 5" for a chapter, "Ch. 1–12" for several. A badged copy is cached next to its
//! video and redrawn only when its label or the base image changes.
//!
//! Without an AI image (no Magnific key, or not made yet) the base is a local composition of the
//! cover ([`BASE_LOCAL`]: blurred cover behind, the cover centred, the title), so there is always a
//! thumbnail.
//!
//! A thumbnail made on Studio's Thumbnails page wins over both: the one picked for the manga (else
//! the newest made) is copied in as [`BASE_PICKED`] by [`sync_picked`], and a video can have its
//! own pick ([`Picks`]), drawn instead of the manga's base.
//!
//! The AI image itself is painted by Studio (which owns the Magnific client and key): it registers
//! a [`Painter`] at startup and readers call [`paint_base`]. Nothing here talks to the network.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context as _, Result};
use futures::future::BoxFuture;
use image::{imageops, DynamicImage, RgbImage};
use serde_json::{json, Value};

/// The manga's AI image (one per manga, in its Studio folder).
pub const BASE_AI: &str = "cover_thumb_16x9.jpg";
/// The local stand-in made from the cover when there is no AI image.
pub const BASE_LOCAL: &str = "cover_local_16x9.jpg";
/// The continuous ("Play all" / full recap) video's thumbnail, in the manga's Studio folder.
pub const FULL_THUMBNAIL: &str = "full_recap_thumbnail.jpg";
/// The thumbnail made on Studio's Thumbnails page that the manga's videos use ([`sync_picked`]).
pub const BASE_PICKED: &str = "video_thumb_16x9.jpg";
/// Which made thumbnails the videos use ([`Picks`]), in the manga's Studio folder.
pub const PICKS: &str = "video_thumbs.json";

const BASE_W: u32 = 1920;
const BASE_H: u32 = 1080;
/// YouTube's thumbnail size.
const OUT_W: u32 = 1280;
const OUT_H: u32 = 720;
/// Bumped when the badge or the local composition changes, so cached copies are redrawn.
const LOOK_VERSION: u64 = 1;

static FONT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-ExtraBold.ttf"));

// ------------------------------------------------------------------ the badge text

fn fmt_num(n: f64) -> String {
    if n.fract() == 0. {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The badge for a video made of chapters numbered `numbers` (in any order): "Chapter 5" for one,
/// "Ch. 1–12" for several (first to last, decimals kept), "Special" for one chapter without a
/// number. Chapters without a number do not widen a range.
pub fn badge_label(numbers: &[Option<f64>]) -> String {
    let nums: Vec<f64> = numbers.iter().flatten().copied().filter(|n| n.is_finite()).collect();
    let lo = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    match (nums.is_empty(), numbers.len()) {
        (true, 0) => "Recap".into(),
        (true, 1) => "Special".into(),
        (true, n) => format!("{n} chapters"),
        _ if lo == hi => format!("Chapter {}", fmt_num(lo)),
        _ => format!("Ch. {}\u{2013}{}", fmt_num(lo), fmt_num(hi)),
    }
}

// ------------------------------------------------------------------ base image

/// Which base a manga's thumbnails are drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    /// Made or picked on Studio's Thumbnails page ([`BASE_PICKED`]).
    Picked,
    Ai,
    Local,
}

fn nonempty(p: &Path) -> bool {
    p.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// The base already made for the manga whose Studio folder is `manga_dir`: the thumbnail picked on
/// the Thumbnails page, else its AI image, else the local composition, else `None`.
pub fn pick_base(manga_dir: &Path) -> Option<(PathBuf, BaseKind)> {
    let picked = manga_dir.join(BASE_PICKED);
    let ai = manga_dir.join(BASE_AI);
    let local = manga_dir.join(BASE_LOCAL);
    if nonempty(&picked) {
        Some((picked, BaseKind::Picked))
    } else if nonempty(&ai) {
        Some((ai, BaseKind::Ai))
    } else if nonempty(&local) {
        Some((local, BaseKind::Local))
    } else {
        None
    }
}

/// [`pick_base`], making the local composition from `cover` (or a chapter page) when there is no
/// base yet. Blocking.
pub fn ensure_base(manga_dir: &Path, cover: Option<&Path>, title: &str) -> Result<(PathBuf, BaseKind)> {
    if let Some(b) = pick_base(manga_dir) {
        return Ok(b);
    }
    let cover = cover.ok_or_else(|| anyhow!("this manga has no cover to make a thumbnail from"))?;
    let out = manga_dir.join(BASE_LOCAL);
    make_local_base(cover, title, &out)?;
    Ok((out, BaseKind::Local))
}

/// The local 16:9 composition: the cover blurred and darkened to fill the frame, the cover itself
/// centred on it, and the title along the bottom.
pub fn make_local_base(cover: &Path, title: &str, out: &Path) -> Result<()> {
    let img = image::open(cover).with_context(|| format!("could not read the cover {}", cover.display()))?;
    // A small blur scaled up is as smooth as a big one on the full frame, and much faster.
    let small = img.resize_to_fill(BASE_W / 12, BASE_H / 12, imageops::FilterType::Triangle).to_rgb8();
    let small = imageops::blur(&small, 2.5);
    let mut canvas = imageops::resize(&small, BASE_W, BASE_H, imageops::FilterType::Triangle);
    for p in canvas.pixels_mut() {
        for c in p.0.iter_mut() {
            *c = (*c as f32 * 0.5) as u8;
        }
    }
    let fg = img.resize((BASE_W as f32 * 0.9) as u32, (BASE_H as f32 * 0.74) as u32, imageops::FilterType::Lanczos3).to_rgb8();
    let (fx, fy) = ((BASE_W - fg.width()) / 2, (BASE_H as f32 * 0.07) as u32);
    imageops::overlay(&mut canvas, &fg, fx as i64, fy as i64);
    let mut px = Canvas::from_rgb(canvas)?;
    px.stroke_rect(fx as f32, fy as f32, fg.width() as f32, fg.height() as f32, 3., [255, 255, 255, 70]);
    let title = title.trim();
    if !title.is_empty() {
        let size = BASE_H as f32 * 0.062;
        let text = fit_text(title, size, BASE_W as f32 * 0.9);
        let w = text_width(&text, size);
        let (x, base) = ((BASE_W as f32 - w) / 2., BASE_H as f32 * 0.935);
        px.text(&text, size, x + 4., base + 4., [0, 0, 0, 170]);
        px.text(&text, size, x, base, [255, 255, 255, 255]);
    }
    save_jpeg(&px.into_rgb()?, out, 90)
}

// ------------------------------------------------------------------ badged copies

fn mtime_ns(p: &Path) -> u128 {
    p.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0)
}

/// What a badged copy was drawn from; a match means it is current.
fn badge_key(base: &Path, label: &str) -> Value {
    json!({ "v": LOOK_VERSION, "base": base.to_string_lossy(), "base_mtime": mtime_ns(base).to_string(), "label": label })
}

/// `out` drawn from `base` with `label` in the top-right corner (1280×720 JPEG), reusing `out`
/// when it was drawn from the same base file and label. Its key sits next to it (`.json`).
pub fn badge(base: &Path, label: &str, out: &Path) -> Result<PathBuf> {
    let _one = DRAW.lock().unwrap_or_else(|e| e.into_inner());
    let key = badge_key(base, label);
    let key_file = out.with_extension("json");
    let cached: Option<Value> = std::fs::read(&key_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if cached.as_ref() == Some(&key) && nonempty(out) {
        return Ok(out.to_path_buf());
    }
    let img = image::open(base).with_context(|| format!("could not read {}", base.display()))?;
    let img = img.resize_to_fill(OUT_W, OUT_H, imageops::FilterType::Lanczos3).to_rgb8();
    let mut px = Canvas::from_rgb(img)?;
    px.badge(label);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    save_jpeg(&px.into_rgb()?, out, 90)?;
    std::fs::write(&key_file, serde_json::to_vec(&key)?)?;
    Ok(out.to_path_buf())
}

/// A chapter video's thumbnail: the manga's base (or the video's own pick) with `label`, saved as
/// `thumbnail.jpg` in the chapter's Studio folder `output_dir`. Local and cheap; blocking.
pub fn chapter_thumbnail(manga_dir: &Path, cover: Option<&Path>, title: &str, output_dir: &Path, label: &str) -> Result<PathBuf> {
    let base = video_base(manga_dir, &output_dir.join(crate::media::VIDEO), cover, title)?;
    badge(&base, label, &output_dir.join(crate::media::THUMBNAIL))
}

/// The continuous video's thumbnail ([`FULL_THUMBNAIL`] in the manga's Studio folder): the same
/// base (or the video's own pick) with the range of chapters in it. Local and cheap; blocking.
pub fn full_thumbnail(manga_dir: &Path, cover: Option<&Path>, title: &str, label: &str) -> Result<PathBuf> {
    let base = video_base(manga_dir, &manga_dir.join(crate::media::FULL_RECAP), cover, title)?;
    badge(&base, label, &manga_dir.join(FULL_THUMBNAIL))
}

fn video_base(manga_dir: &Path, video: &Path, cover: Option<&Path>, title: &str) -> Result<PathBuf> {
    match Picks::load(manga_dir).for_video(manga_dir, video) {
        Some(p) => Ok(p),
        None => ensure_base(manga_dir, cover, title).map(|b| b.0),
    }
}

// ------------------------------------------------------------------ thumbnails made in Studio

/// Which of the thumbnails made on Studio's Thumbnails page the manga's videos use ([`PICKS`]).
/// Paths are image files in the manga's gallery; one that is gone counts as no pick.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Picks {
    /// For every video of the manga (else the newest made thumbnail).
    pub manga: Option<PathBuf>,
    /// One video's own: [`video_key`] -> image.
    pub videos: std::collections::BTreeMap<String, PathBuf>,
}

impl Picks {
    pub fn load(manga_dir: &Path) -> Picks {
        std::fs::read(manga_dir.join(PICKS)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, manga_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(manga_dir)?;
        std::fs::write(manga_dir.join(PICKS), serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// The image picked for `video` itself, when it still exists.
    pub fn for_video(&self, manga_dir: &Path, video: &Path) -> Option<PathBuf> {
        self.videos.get(&video_key(manga_dir, video)).filter(|p| nonempty(p)).cloned()
    }
}

/// A video's key in [`Picks::videos`]: its path inside the manga's Studio folder
/// (`<chapter>/video_final.mp4`, `full_recap.mp4`), else the whole path.
pub fn video_key(manga_dir: &Path, video: &Path) -> String {
    video.strip_prefix(manga_dir).unwrap_or(video).to_string_lossy().replace('\\', "/")
}

/// The manga's gallery of thumbnails made on Studio's Thumbnails page (`<studio>/<manga>/thumbnails`;
/// `gallery.json` lists them newest first, each `<id>.png`).
pub fn gallery_dir(manga_id: &str) -> PathBuf {
    crate::Brain::dir(manga_id).join("thumbnails")
}

/// The made thumbnails that still exist, newest first.
pub fn gallery_images(manga_id: &str) -> Vec<PathBuf> {
    let dir = gallery_dir(manga_id);
    let g: Value = std::fs::read(dir.join("gallery.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    g.get("thumbs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| t.get("id")?.as_str()).map(|id| dir.join(format!("{id}.png"))).filter(|p| nonempty(p)).collect())
        .unwrap_or_default()
}

/// A landscape image close to 16:9 (a Shorts or square thumbnail would be cut down to a strip).
pub fn is_wide(p: &Path) -> bool {
    image::image_dimensions(p).is_ok_and(|(w, h)| h > 0 && (1.6..=1.95).contains(&(w as f32 / h as f32)))
}

/// The made thumbnail every video of the manga uses: the picked one, else the newest 16:9 one.
pub fn manga_pick(manga_id: &str) -> Option<PathBuf> {
    let picked = Picks::load(&crate::media::manga_dir(manga_id)).manga.filter(|p| nonempty(p));
    picked.or_else(|| gallery_images(manga_id).into_iter().find(|p| is_wide(p)))
}

/// One drawing at a time: the same base and badge files are written from several places (the
/// Thumbnails page, the manga page, the reader), and Windows lets two writers mix their bytes.
static DRAW: Mutex<()> = Mutex::new(());

/// Copies [`manga_pick`] in as the manga's [`BASE_PICKED`] (or removes it when nothing was made),
/// skipping the work when it is already that image. True when the base changed. Blocking.
pub fn sync_picked(manga_id: &str) -> Result<bool> {
    sync_picked_in(&crate::media::manga_dir(manga_id), manga_pick(manga_id).as_deref())
}

fn sync_picked_in(manga_dir: &Path, src: Option<&Path>) -> Result<bool> {
    let _one = DRAW.lock().unwrap_or_else(|e| e.into_inner());
    let out = manga_dir.join(BASE_PICKED);
    let key_file = out.with_extension("json");
    let Some(src) = src else {
        let had = out.exists();
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&key_file);
        return Ok(had);
    };
    let key = json!({ "src": src.to_string_lossy(), "mtime": mtime_ns(src).to_string() });
    let cached: Option<Value> = std::fs::read(&key_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if cached.as_ref() == Some(&key) && nonempty(&out) {
        return Ok(false);
    }
    let img = image::open(src).with_context(|| format!("could not read {}", src.display()))?;
    save_jpeg(&img.resize_to_fill(BASE_W, BASE_H, imageops::FilterType::Lanczos3).to_rgb8(), &out, 92)?;
    std::fs::write(&key_file, serde_json::to_vec(&key)?)?;
    Ok(true)
}

/// Draws the manga's existing video thumbnails again on their current base (after a pick or a new
/// thumbnail), each with the label it had. Returns the files that changed. Blocking.
pub fn refresh_badges(manga_id: &str) -> Vec<PathBuf> {
    refresh_badges_in(&crate::media::manga_dir(manga_id))
}

fn refresh_badges_in(manga_dir: &Path) -> Vec<PathBuf> {
    let label_of = |thumb: &Path| -> Option<String> {
        let key: Value = serde_json::from_slice(&std::fs::read(thumb.with_extension("json")).ok()?).ok()?;
        key.get("label")?.as_str().map(str::to_string)
    };
    let mut changed = vec![];
    let full = manga_dir.join(FULL_THUMBNAIL);
    if let Some(label) = label_of(&full) {
        let was = mtime_ns(&full);
        if full_thumbnail(manga_dir, None, "", &label).is_ok() && mtime_ns(&full) != was {
            changed.push(full);
        }
    }
    for e in std::fs::read_dir(manga_dir).into_iter().flatten().flatten() {
        let dir = e.path();
        let t = dir.join(crate::media::THUMBNAIL);
        let Some(label) = label_of(&t) else { continue };
        let was = mtime_ns(&t);
        if chapter_thumbnail(manga_dir, None, "", &dir, &label).is_ok() && mtime_ns(&t) != was {
            changed.push(t);
        }
    }
    changed
}

// ------------------------------------------------------------------ the AI image

/// One AI image to paint: from `cover`, written as an image file (any format) at `out`.
pub struct PaintJob {
    pub manga_id: String,
    pub title: String,
    pub cover: PathBuf,
    pub out: PathBuf,
}

/// Studio's image painter, registered at startup (`ui-studio`, Magnific).
pub struct Painter {
    /// A key is set, so painting can work.
    pub ready: fn() -> bool,
    /// Estimated credits for one image with the current defaults (0: unknown).
    pub cost: fn() -> f64,
    /// Paints one image; spends credits.
    pub paint: fn(PaintJob) -> BoxFuture<'static, Result<()>>,
}

static PAINTER: OnceLock<Painter> = OnceLock::new();
/// Manga folders whose AI image was asked for once this session (automatic requests never repeat).
static TRIED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
static IN_FLIGHT: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

pub fn set_painter(p: Painter) {
    let _ = PAINTER.set(p);
}

/// An AI image can be painted (a painter is registered and has a key).
pub fn can_paint() -> bool {
    PAINTER.get().is_some_and(|p| (p.ready)())
}

/// Estimated credits of one AI image, when known.
pub fn paint_cost() -> Option<f64> {
    PAINTER.get().map(|p| (p.cost)()).filter(|c| *c > 0.)
}

fn set_has(set: &Mutex<Option<HashSet<PathBuf>>>, k: &Path) -> bool {
    set.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(k))
}

fn set_put(set: &Mutex<Option<HashSet<PathBuf>>>, k: &Path, on: bool) -> bool {
    let mut g = set.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(HashSet::new);
    if on {
        s.insert(k.to_path_buf())
    } else {
        s.remove(k)
    }
}

/// The automatic request (a chapter video shown for the first time) should paint: no AI image
/// yet, painting works, and it was not tried this session (a failure is never retried in a loop).
pub fn should_auto_paint(manga_dir: &Path) -> bool {
    can_paint() && !nonempty(&manga_dir.join(BASE_AI)) && !nonempty(&manga_dir.join(BASE_PICKED)) && !set_has(&TRIED, manga_dir) && !set_has(&IN_FLIGHT, manga_dir)
}

/// Paints the manga's one AI image from `cover` with the registered painter. Without `force` it
/// reuses an existing image and does nothing for a manga already tried this session; `force`
/// (Regenerate) replaces it. One paint per manga at a time.
pub async fn paint_base(manga_id: &str, title: &str, cover: &Path, force: bool) -> Result<PathBuf> {
    let p = PAINTER.get().ok_or_else(|| anyhow!("Studio's image generator is not available"))?;
    if !(p.ready)() {
        bail!("Add a Magnific API key in Settings › Magnific to make AI thumbnails");
    }
    paint_with(p.paint, &crate::media::manga_dir(manga_id), manga_id, title, cover, force).await
}

/// [`paint_base`] with a given paint function (tests use a fake; nothing here spends credits).
pub async fn paint_with<F>(paint: F, manga_dir: &Path, manga_id: &str, title: &str, cover: &Path, force: bool) -> Result<PathBuf>
where
    F: FnOnce(PaintJob) -> BoxFuture<'static, Result<()>>,
{
    let out = manga_dir.join(BASE_AI);
    if !force && nonempty(&out) {
        return Ok(out);
    }
    if !force && set_has(&TRIED, manga_dir) {
        bail!("The AI thumbnail was already tried this session");
    }
    if !set_put(&IN_FLIGHT, manga_dir, true) {
        bail!("The AI thumbnail is already being made");
    }
    set_put(&TRIED, manga_dir, true);
    let part = manga_dir.join("cover_thumb_16x9.part.png");
    let res = async {
        std::fs::create_dir_all(manga_dir)?;
        paint(PaintJob { manga_id: manga_id.into(), title: title.into(), cover: cover.to_path_buf(), out: part.clone() }).await?;
        let (part, out) = (part.clone(), out.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let img = image::open(&part).context("the painted image could not be read")?;
            save_jpeg(&img.resize_to_fill(BASE_W, BASE_H, imageops::FilterType::Lanczos3).to_rgb8(), &out, 92)
        })
        .await??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let _ = std::fs::remove_file(&part);
    set_put(&IN_FLIGHT, manga_dir, false);
    res.map(|_| out)
}

/// The prompt for the manga's one AI image (the cover is attached as Image 1). The top-right
/// corner stays calm for the chapter badge drawn on it later.
pub fn cover_prompt(title: &str) -> String {
    format!(
        "Turn the attached manga cover (Image 1) into a 16:9 landscape YouTube thumbnail for a recap video of \"{}\". Keep the characters, their faces, hair, outfits, the art style and the colour palette exactly like the cover. Recompose it for a wide frame: the main character large and sharp, off-centre, the scene extended naturally to the sides, cinematic rim light, high contrast, readable at a small size. Keep the top-right corner simple and uncluttered. No text, no title, no letters, no logo, no watermark, no borders.",
        title.trim()
    )
}

// ------------------------------------------------------------------ drawing

fn save_jpeg(img: &RgbImage, out: &Path, quality: u8) -> Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let part = out.with_extension("part.jpg");
    {
        let f = std::fs::File::create(&part)?;
        let mut w = std::io::BufWriter::new(f);
        DynamicImage::ImageRgb8(img.clone()).write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, quality))?;
    }
    std::fs::rename(&part, out)?;
    Ok(())
}

fn face() -> Option<ttf_parser::Face<'static>> {
    ttf_parser::Face::parse(FONT, 0).ok()
}

fn glyph(f: &ttf_parser::Face, c: char) -> Option<ttf_parser::GlyphId> {
    f.glyph_index(c).or_else(|| f.glyph_index(if c == '\u{2013}' { '-' } else { '?' }))
}

/// Width of `text` at `size` px.
fn text_width(text: &str, size: f32) -> f32 {
    let Some(f) = face() else { return 0. };
    let s = size / f.units_per_em() as f32;
    text.chars().filter_map(|c| glyph(&f, c)).map(|g| f.glyph_hor_advance(g).unwrap_or(0) as f32 * s).sum()
}

/// `text` cut with an ellipsis to fit `max_w` px at `size`.
fn fit_text(text: &str, size: f32, max_w: f32) -> String {
    if text_width(text, size) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let t = format!("{}\u{2026}", chars.iter().collect::<String>().trim_end());
        if text_width(&t, size) <= max_w {
            return t;
        }
    }
    String::new()
}

struct Pen<'a> {
    pb: &'a mut tiny_skia::PathBuilder,
    x: f32,
    y: f32,
    s: f32,
}

impl ttf_parser::OutlineBuilder for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(self.x + x1 * self.s, self.y - y1 * self.s, self.x + x * self.s, self.y - y * self.s);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(self.x + x1 * self.s, self.y - y1 * self.s, self.x + x2 * self.s, self.y - y2 * self.s, self.x + x * self.s, self.y - y * self.s);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.).min(h / 2.);
    let k = 0.552_284_8 * r;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn paint(rgba: [u8; 4]) -> tiny_skia::Paint<'static> {
    let mut p = tiny_skia::Paint::default();
    p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    p.anti_alias = true;
    p
}

/// An opaque image being drawn on.
struct Canvas(tiny_skia::Pixmap);

impl Canvas {
    fn from_rgb(img: RgbImage) -> Result<Canvas> {
        let (w, h) = img.dimensions();
        let rgba = DynamicImage::ImageRgb8(img).to_rgba8().into_raw();
        let size = tiny_skia::IntSize::from_wh(w, h).ok_or_else(|| anyhow!("empty image"))?;
        // Opaque pixels are the same premultiplied or not.
        Ok(Canvas(tiny_skia::Pixmap::from_vec(rgba, size).ok_or_else(|| anyhow!("image too large"))?))
    }

    fn into_rgb(self) -> Result<RgbImage> {
        let (w, h) = (self.0.width(), self.0.height());
        let rgba = image::RgbaImage::from_raw(w, h, self.0.take()).ok_or_else(|| anyhow!("bad image buffer"))?;
        Ok(DynamicImage::ImageRgba8(rgba).to_rgb8())
    }

    fn width(&self) -> f32 {
        self.0.width() as f32
    }

    fn fill(&mut self, path: &tiny_skia::Path, rgba: [u8; 4]) {
        self.0.fill_path(path, &paint(rgba), tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }

    fn stroke(&mut self, path: &tiny_skia::Path, width: f32, rgba: [u8; 4]) {
        let stroke = tiny_skia::Stroke { width, ..Default::default() };
        self.0.stroke_path(path, &paint(rgba), &stroke, tiny_skia::Transform::identity(), None);
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, rgba: [u8; 4]) {
        if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            self.stroke(&tiny_skia::PathBuilder::from_rect(r), width, rgba);
        }
    }

    /// `text` with its baseline at `baseline`, starting at `x`.
    fn text(&mut self, text: &str, size: f32, x: f32, baseline: f32, rgba: [u8; 4]) {
        let Some(f) = face() else { return };
        let s = size / f.units_per_em() as f32;
        let mut pb = tiny_skia::PathBuilder::new();
        let mut pen_x = x;
        for c in text.chars() {
            let Some(g) = glyph(&f, c) else { continue };
            f.outline_glyph(g, &mut Pen { pb: &mut pb, x: pen_x, y: baseline, s });
            pen_x += f.glyph_hor_advance(g).unwrap_or(0) as f32 * s;
        }
        if let Some(p) = pb.finish() {
            self.fill(&p, rgba);
        }
    }

    /// The chapter badge: a rounded dark pill with white bold text, in the top-right corner,
    /// sized from the image width so it reads the same at any size.
    fn badge(&mut self, label: &str) {
        let w = self.width();
        let h = (w * 0.068).round();
        let margin = (w * 0.03).round();
        let mut size = h * 0.5;
        let max_text = w * 0.5;
        let mut tw = text_width(label, size);
        if tw > max_text && tw > 0. {
            size *= max_text / tw;
            tw = max_text;
        }
        let pad = h * 0.45;
        let (bw, x, y) = (tw + 2. * pad, w - margin - (tw + 2. * pad), margin);
        if let Some(p) = rounded_rect(x, y, bw, h, h / 2.) {
            self.fill(&p, [10, 10, 14, 205]);
            self.stroke(&p, (w * 0.0016).max(1.), [255, 255, 255, 46]);
        }
        let cap = face().and_then(|f| f.capital_height().map(|c| c as f32 / f.units_per_em() as f32)).unwrap_or(0.72) * size;
        self.text(label, size, x + pad, y + (h + cap) / 2., [255, 255, 255, 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn badge_labels() {
        assert_eq!(badge_label(&[Some(5.)]), "Chapter 5");
        assert_eq!(badge_label(&[Some(12.5)]), "Chapter 12.5");
        // A range is first to last, whatever the order.
        assert_eq!(badge_label(&[Some(12.), Some(1.), Some(4.)]), "Ch. 1\u{2013}12");
        assert_eq!(badge_label(&[Some(1.), Some(12.5)]), "Ch. 1\u{2013}12.5");
        assert_eq!(badge_label(&[Some(0.5), Some(3.)]), "Ch. 0.5\u{2013}3");
        // The same chapter twice (two sources) is one chapter.
        assert_eq!(badge_label(&[Some(3.), Some(3.)]), "Chapter 3");
        // Unnumbered chapters.
        assert_eq!(badge_label(&[None]), "Special");
        assert_eq!(badge_label(&[None, None, None]), "3 chapters");
        assert_eq!(badge_label(&[None, Some(2.), Some(7.), None]), "Ch. 2\u{2013}7");
        assert_eq!(badge_label(&[None, Some(9.)]), "Chapter 9");
        assert_eq!(badge_label(&[]), "Recap");
    }

    fn solid(path: &Path, w: u32, h: u32, c: [u8; 3]) {
        RgbImage::from_pixel(w, h, image::Rgb(c)).save(path).unwrap();
    }

    #[test]
    fn base_falls_back_to_the_local_composition() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("manga");
        assert_eq!(pick_base(&m), None);
        // No base and no cover: no thumbnail can be made.
        assert!(ensure_base(&m, None, "Solo").is_err());
        let cover = t.path().join("cover.png");
        solid(&cover, 300, 450, [200, 40, 40]);
        let (p, k) = ensure_base(&m, Some(&cover), "Solo Leveling").unwrap();
        assert_eq!((p.clone(), k), (m.join(BASE_LOCAL), BaseKind::Local));
        assert_eq!(image::image_dimensions(&p).unwrap(), (BASE_W, BASE_H));
        assert_eq!(pick_base(&m), Some((m.join(BASE_LOCAL), BaseKind::Local)));
        // An empty AI file (a write that failed) does not count; a real one wins.
        std::fs::write(m.join(BASE_AI), "").unwrap();
        assert_eq!(pick_base(&m).unwrap().1, BaseKind::Local);
        solid(&m.join(BASE_AI), 64, 36, [0, 0, 255]);
        assert_eq!(pick_base(&m), Some((m.join(BASE_AI), BaseKind::Ai)));
        assert_eq!(ensure_base(&m, None, "x").unwrap().1, BaseKind::Ai);
    }

    #[test]
    fn only_landscape_thumbnails_become_the_poster_on_their_own() {
        let t = tempfile::tempdir().unwrap();
        let (wide, tall, square) = (t.path().join("w.png"), t.path().join("t.png"), t.path().join("s.png"));
        solid(&wide, 1280, 720, [0, 0, 0]);
        solid(&tall, 720, 1280, [0, 0, 0]);
        solid(&square, 800, 800, [0, 0, 0]);
        assert!(is_wide(&wide) && !is_wide(&tall) && !is_wide(&square) && !is_wide(&t.path().join("missing.png")));
    }

    #[test]
    fn a_thumbnail_made_in_studio_is_what_the_videos_show() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("manga");
        let ch = m.join("c1");
        std::fs::create_dir_all(&ch).unwrap();
        solid(&m.join(BASE_AI), 64, 36, [0, 0, 255]);
        let thumb = chapter_thumbnail(&m, None, "Solo", &ch, "Chapter 1").unwrap();
        assert!(image::open(&thumb).unwrap().to_rgb8().get_pixel(20, OUT_H - 20).0[2] > 200, "drawn on the AI image");

        // A made thumbnail becomes the base, and the existing video thumbnail is drawn again on it.
        let made = t.path().join("gallery").join("a.png");
        std::fs::create_dir_all(made.parent().unwrap()).unwrap();
        solid(&made, 160, 90, [0, 255, 0]);
        assert!(sync_picked_in(&m, Some(&made)).unwrap());
        assert_eq!(pick_base(&m), Some((m.join(BASE_PICKED), BaseKind::Picked)));
        assert!(!sync_picked_in(&m, Some(&made)).unwrap(), "same image: nothing to do");
        assert_eq!(refresh_badges_in(&m), vec![thumb.clone()]);
        let px = image::open(&thumb).unwrap().to_rgb8().get_pixel(20, OUT_H - 20).0;
        assert!(px[1] > 200 && px[2] < 60, "drawn on the made thumbnail: {px:?}");
        assert!(refresh_badges_in(&m).is_empty(), "already current");

        // A video's own pick wins over the manga's.
        let own = t.path().join("gallery").join("b.png");
        solid(&own, 160, 90, [255, 0, 0]);
        let mut picks = Picks::default();
        picks.videos.insert(video_key(&m, &ch.join(crate::media::VIDEO)), own.clone());
        picks.save(&m).unwrap();
        assert_eq!(video_key(&m, &ch.join(crate::media::VIDEO)), "c1/video_final.mp4");
        assert_eq!(refresh_badges_in(&m), vec![thumb.clone()]);
        assert!(image::open(&thumb).unwrap().to_rgb8().get_pixel(20, OUT_H - 20).0[0] > 200);

        // Nothing made any more: back to the AI image.
        assert!(sync_picked_in(&m, None).unwrap());
        assert_eq!(pick_base(&m).unwrap().1, BaseKind::Ai);
    }

    #[test]
    fn badge_is_drawn_top_right_and_cached() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("base.png");
        solid(&base, 1920, 1080, [250, 250, 250]);
        let out = t.path().join("ch").join("thumbnail.jpg");
        badge(&base, "Chapter 5", &out).unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (OUT_W, OUT_H));
        // Inside the pill (top right) is dark, the rest of the picture untouched.
        let (bx, by) = (OUT_W - (OUT_W as f32 * 0.03) as u32 - 6, (OUT_W as f32 * 0.03) as u32 + (OUT_W as f32 * 0.034) as u32);
        assert!(img.get_pixel(bx, by).0[0] < 80, "pill at the top right: {:?}", img.get_pixel(bx, by));
        assert!(img.get_pixel(20, OUT_H - 20).0[0] > 230);
        assert!(img.get_pixel(20, 20).0[0] > 230, "top left stays clear");
        // Same base and label: reused as it is.
        let before = mtime_ns(&out);
        std::thread::sleep(std::time::Duration::from_millis(20));
        badge(&base, "Chapter 5", &out).unwrap();
        assert_eq!(mtime_ns(&out), before);
        // A new label, or a new base image, draws it again.
        badge(&base, "Ch. 1\u{2013}12", &out).unwrap();
        let relabelled = mtime_ns(&out);
        assert_ne!(relabelled, before);
        std::thread::sleep(std::time::Duration::from_millis(20));
        solid(&base, 1920, 1080, [10, 200, 10]);
        badge(&base, "Ch. 1\u{2013}12", &out).unwrap();
        assert_ne!(mtime_ns(&out), relabelled);
        assert!(image::open(&out).unwrap().to_rgb8().get_pixel(20, 20).0[1] > 150);
    }

    #[test]
    fn every_video_of_a_manga_shares_one_base() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("manga");
        let cover = t.path().join("cover.png");
        solid(&cover, 300, 450, [20, 20, 200]);
        let a = chapter_thumbnail(&m, Some(&cover), "T", &m.join("c1"), "Chapter 1").unwrap();
        let b = chapter_thumbnail(&m, Some(&cover), "T", &m.join("c2"), "Chapter 2").unwrap();
        let f = full_thumbnail(&m, Some(&cover), "T", "Ch. 1\u{2013}2").unwrap();
        assert_eq!((a.clone(), b.clone(), f.clone()), (m.join("c1").join("thumbnail.jpg"), m.join("c2").join("thumbnail.jpg"), m.join(FULL_THUMBNAIL)));
        let base_of = |p: &Path| serde_json::from_slice::<Value>(&std::fs::read(p.with_extension("json")).unwrap()).unwrap()["base"].clone();
        assert_eq!(base_of(&a), json!(m.join(BASE_LOCAL).to_string_lossy()));
        assert_eq!(base_of(&a), base_of(&b));
        assert_eq!(base_of(&a), base_of(&f));
        // Only one base image exists for the manga.
        assert!(!m.join(BASE_AI).exists());
        let bases = std::fs::read_dir(&m).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("cover_")).count();
        assert_eq!(bases, 1);
    }

    #[tokio::test]
    async fn paints_once_per_manga_with_a_fake_painter() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("manga");
        let cover = t.path().join("cover.png");
        solid(&cover, 300, 450, [20, 20, 200]);
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn fake(job: PaintJob) -> BoxFuture<'static, Result<()>> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(job.cover.is_file());
                // A square answer is cut to 16:9.
                RgbImage::from_pixel(1024, 1024, image::Rgb([1, 2, 3])).save(&job.out)?;
                Ok(())
            }
            .boxed()
        }
        let p = paint_with(fake, &m, "id", "T", &cover, false).await.unwrap();
        assert_eq!(p, m.join(BASE_AI));
        assert_eq!(image::image_dimensions(&p).unwrap(), (BASE_W, BASE_H));
        assert!(!m.join("cover_thumb_16x9.part.png").exists());
        assert_eq!(pick_base(&m).unwrap().1, BaseKind::Ai);
        // Asked again: the image exists, nothing is painted.
        paint_with(fake, &m, "id", "T", &cover, false).await.unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // Regenerate paints a new one.
        paint_with(fake, &m, "id", "T", &cover, true).await.unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_paint_is_not_retried_automatically() {
        let t = tempfile::tempdir().unwrap();
        let m = t.path().join("manga");
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn failing(_: PaintJob) -> BoxFuture<'static, Result<()>> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            async { bail!("content filter") }.boxed()
        }
        assert!(paint_with(failing, &m, "id", "T", Path::new("c.png"), false).await.is_err());
        assert!(paint_with(failing, &m, "id", "T", Path::new("c.png"), false).await.unwrap_err().to_string().contains("already tried"));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // No painter registered in tests: the automatic request never fires.
        assert!(!should_auto_paint(&t.path().join("other")));
        assert!(!set_has(&IN_FLIGHT, &m));
    }

    #[test]
    fn prompt_keeps_the_cover_and_leaves_room_for_the_badge() {
        let p = cover_prompt(" Solo Leveling ");
        assert!(p.contains("\"Solo Leveling\"") && p.contains("16:9") && p.contains("top-right") && p.contains("No text"));
    }
}
