//! Page layouts: the panel grids a chapter script picks from, where each panel sits on the page
//! (mirrored for right-to-left reading), the image shape to ask a model for, and the composer
//! that lays generated panels out on a page with borders, gutters and lettering.
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use image::{imageops, DynamicImage, RgbImage};

use crate::letter::{self, Canvas};
use crate::project::Line;

/// A page is 2:3 (close to a tankōbon's 1:1.42 once trimmed), painted at 1600 × 2400.
pub const PAGE_W: u32 = 1600;
pub const PAGE_H: u32 = 2400;
pub const PAGE_ASPECT: &str = "2:3";

/// x, y, w, h as fractions of the page's live area, in left-to-right reading order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

pub struct Template {
    pub id: &'static str,
    /// What the AI reads when it picks one.
    pub use_for: &'static str,
    pub panels: &'static [Rect],
}

pub static TEMPLATES: &[Template] = &[
    Template { id: "splash", use_for: "one full-page splash: a reveal, a title page, a huge moment", panels: &[r(0., 0., 1., 1.)] },
    Template { id: "two-tier", use_for: "two wide panels stacked: a cause and its effect", panels: &[r(0., 0., 1., 0.5), r(0., 0.5, 1., 0.5)] },
    Template { id: "three-tier", use_for: "three wide panels: steady pacing, a walk, a conversation", panels: &[r(0., 0., 1., 0.33), r(0., 0.33, 1., 0.34), r(0., 0.67, 1., 0.33)] },
    Template { id: "big-top", use_for: "a big establishing panel then two reaction panels", panels: &[r(0., 0., 1., 0.55), r(0., 0.55, 0.5, 0.45), r(0.5, 0.55, 0.5, 0.45)] },
    Template { id: "big-bottom", use_for: "two setup panels then a big payoff panel", panels: &[r(0., 0., 0.5, 0.42), r(0.5, 0., 0.5, 0.42), r(0., 0.42, 1., 0.58)] },
    Template { id: "four-grid", use_for: "four even beats: a back-and-forth, a montage", panels: &[r(0., 0., 0.5, 0.5), r(0.5, 0., 0.5, 0.5), r(0., 0.5, 0.5, 0.5), r(0.5, 0.5, 0.5, 0.5)] },
    Template { id: "tall-side", use_for: "a tall full-body panel beside three small beats", panels: &[r(0., 0., 0.45, 1.), r(0.45, 0., 0.55, 0.33), r(0.45, 0.33, 0.55, 0.34), r(0.45, 0.67, 0.55, 0.33)] },
    Template { id: "five-action", use_for: "fast action: a wide opener, then quick hits", panels: &[r(0., 0., 1., 0.3), r(0., 0.3, 0.4, 0.35), r(0.4, 0.3, 0.6, 0.35), r(0., 0.65, 0.6, 0.35), r(0.6, 0.65, 0.4, 0.35)] },
    Template { id: "six-classic", use_for: "dialogue-heavy pages: six even panels", panels: &[r(0., 0., 0.5, 0.33), r(0.5, 0., 0.5, 0.33), r(0., 0.33, 0.5, 0.34), r(0.5, 0.33, 0.5, 0.34), r(0., 0.67, 0.5, 0.33), r(0.5, 0.67, 0.5, 0.33)] },
];

/// The template `id` names, else the first one with `n` panels (a script may name a layout with
/// a different count); an even grid when none has that many.
pub fn rects(id: &str, n: usize, rtl: bool) -> Vec<Rect> {
    let n = n.max(1);
    let base: Vec<Rect> = match TEMPLATES.iter().find(|t| t.id == id && t.panels.len() == n).or_else(|| TEMPLATES.iter().find(|t| t.panels.len() == n)) {
        Some(t) => t.panels.to_vec(),
        None => {
            let rows = n.div_ceil(2);
            (0..n)
                .map(|i| {
                    let (row, col) = (i / 2, i % 2);
                    let last_alone = n % 2 == 1 && i == n - 1;
                    r(if last_alone { 0. } else { col as f32 * 0.5 }, row as f32 / rows as f32, if last_alone { 1. } else { 0.5 }, 1. / rows as f32)
                })
                .collect()
        }
    };
    if rtl {
        base.into_iter().map(|p| r(1. - p.x - p.w, p.y, p.w, p.h)).collect()
    } else {
        base
    }
}

/// Where panel rects land on a `w` × `h` page: a margin around the live area, a gutter between.
pub fn place(rect: Rect, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let (w, h) = (w as f32, h as f32);
    let margin = w * 0.055;
    let (gx, gy) = (w * 0.014, w * 0.022);
    let (lw, lh) = (w - 2. * margin + gx, h - 2. * margin + gy);
    let x0 = margin - gx / 2. + rect.x * lw + gx / 2.;
    let y0 = margin - gy / 2. + rect.y * lh + gy / 2.;
    let pw = rect.w * lw - gx;
    let ph = rect.h * lh - gy;
    (x0.round() as u32, y0.round() as u32, pw.max(1.).round() as u32, ph.max(1.).round() as u32)
}

/// Width / height of an "a:b" aspect.
pub fn ratio(aspect: &str) -> Option<f32> {
    let (a, b) = aspect.split_once(':')?;
    let (a, b) = (a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?);
    (a > 0. && b > 0.).then_some(a / b)
}

/// The option closest in shape to `want` (w / h).
pub fn nearest<'a>(want: f32, options: &[&'a str]) -> &'a str {
    options.iter().copied().min_by(|a, b| {
        let d = |s: &str| ratio(s).map(|r| (r / want).ln().abs()).unwrap_or(f32::MAX);
        d(a).total_cmp(&d(b))
    }).unwrap_or("1:1")
}

/// One generated panel and its lettering, ready to lay out.
pub struct PanelArt {
    pub image: Option<PathBuf>,
    pub lines: Vec<Line>,
}

fn cover(img: &DynamicImage, w: u32, h: u32) -> RgbImage {
    img.resize_to_fill(w, h, imageops::FilterType::Lanczos3).to_rgb8()
}

/// Lays `panels` out on a white page with the layout's rects, black borders and the balloons,
/// saved as PNG at `out`. A panel without an image stays blank (so a partial page still shows).
pub fn compose(layout: &str, panels: &[PanelArt], rtl: bool, out: &Path) -> Result<()> {
    let mut page = RgbImage::from_pixel(PAGE_W, PAGE_H, image::Rgb([255, 255, 255]));
    let rects = rects(layout, panels.len(), rtl);
    let placed: Vec<(u32, u32, u32, u32)> = rects.iter().map(|r| place(*r, PAGE_W, PAGE_H)).collect();
    for (art, (x, y, w, h)) in panels.iter().zip(&placed) {
        if let Some(f) = &art.image {
            let img = image::open(f).with_context(|| format!("could not read {}", f.display()))?;
            imageops::replace(&mut page, &cover(&img, *w, *h), *x as i64, *y as i64);
        }
    }
    let mut c = Canvas::from_rgb(page)?;
    let border = PAGE_W as f32 * 0.0035;
    for (art, (x, y, w, h)) in panels.iter().zip(&placed) {
        c.stroke_rect(*x as f32, *y as f32, *w as f32, *h as f32, border, [0, 0, 0, 255]);
        letter::balloons(&mut c, &art.lines, (*x as f32, *y as f32, *w as f32, *h as f32), rtl);
    }
    save_png(c.into_rgb()?, out)
}

/// A model's whole-page image fitted to the page size (it may come back a little off 2:3).
pub fn fit_page(src: &Path, out: &Path) -> Result<()> {
    let img = image::open(src).with_context(|| format!("could not read {}", src.display()))?;
    save_png(cover(&img, PAGE_W, PAGE_H), out)
}

/// The part of a page image under panel `i` (for animating a whole-page image's panels).
pub fn crop_panel(page: &Path, layout: &str, n: usize, i: usize, rtl: bool, out: &Path) -> Result<()> {
    let img = image::open(page).with_context(|| format!("could not read {}", page.display()))?;
    let rect = *rects(layout, n, rtl).get(i).context("no such panel")?;
    let (x, y, w, h) = place(rect, img.width(), img.height());
    save_png(img.crop_imm(x, y, w, h).to_rgb8(), out)
}

fn save_png(img: RgbImage, out: &Path) -> Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let part = out.with_extension("part.png");
    img.save_with_format(&part, image::ImageFormat::Png)?;
    std::fs::rename(&part, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_tiles_the_page() {
        for t in TEMPLATES {
            let area: f32 = t.panels.iter().map(|p| p.w * p.h).sum();
            assert!((area - 1.).abs() < 0.02, "{} covers {area}", t.id);
            for p in t.panels {
                assert!(p.x >= 0. && p.y >= 0. && p.x + p.w <= 1.001 && p.y + p.h <= 1.001, "{}", t.id);
            }
        }
    }

    #[test]
    fn counts_mirroring_and_fallbacks() {
        // The named layout when the count matches, another with that count when not.
        assert_eq!(rects("four-grid", 4, false).len(), 4);
        assert_eq!(rects("splash", 3, false), rects("three-tier", 3, false));
        // Seven panels: an even grid, the last one full width.
        let seven = rects("whatever", 7, false);
        assert_eq!(seven.len(), 7);
        assert_eq!(seven[6].w, 1.);
        // Right to left: the first panel of a row is on the right.
        let ltr = rects("four-grid", 4, false);
        let rtl = rects("four-grid", 4, true);
        assert!(ltr[0].x < ltr[1].x && rtl[0].x > rtl[1].x);
    }

    #[test]
    fn placed_panels_stay_apart_and_inside() {
        let ps: Vec<_> = rects("six-classic", 6, false).into_iter().map(|r| place(r, PAGE_W, PAGE_H)).collect();
        for (x, y, w, h) in &ps {
            assert!(*x > 0 && *y > 0 && x + w < PAGE_W && y + h < PAGE_H);
        }
        // A gutter between neighbours.
        assert!(ps[0].0 + ps[0].2 < ps[1].0);
        assert!(ps[0].1 + ps[0].3 < ps[2].1);
    }

    #[test]
    fn nearest_shape() {
        assert_eq!(nearest(2. / 3., &["16:9", "9:16", "1:1", "4:3"]), "9:16");
        assert_eq!(nearest(2. / 3., &["1:1", "2:3", "3:4", "16:9"]), "2:3");
        assert_eq!(nearest(2.4, &["16:9", "1:1"]), "16:9");
    }

    #[test]
    fn composes_a_lettered_page() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("p.png");
        RgbImage::from_pixel(300, 200, image::Rgb([90, 90, 90])).save(&src).unwrap();
        let line = |t: &str, k: &str| Line { speaker: "Kai".into(), text: t.into(), kind: k.into() };
        let panels = vec![
            PanelArt { image: Some(src.clone()), lines: vec![line("Ten billion percent, this will work!", "speech"), line("Two days later.", "narration")] },
            PanelArt { image: Some(src), lines: vec![line("WHAT?!", "shout")] },
            PanelArt { image: None, lines: vec![] },
        ];
        let out = t.path().join("page.png");
        compose("big-top", &panels, true, &out).unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (PAGE_W, PAGE_H));
        // The margin is paper white, and the first panel (top) is grey under its balloons.
        assert_eq!(img.get_pixel(5, 5).0, [255, 255, 255]);
        let (x, y, w, h) = place(rects("big-top", 3, true)[0], PAGE_W, PAGE_H);
        assert_eq!(img.get_pixel(x + w / 2, y + h - 20).0, [90, 90, 90]);
    }
}
