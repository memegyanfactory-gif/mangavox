//! Lettering for panel-by-panel pages: speech balloons, shouts, thoughts, whispers and narration
//! boxes drawn over the panels, in reading order (right to left for Japanese order).
use anyhow::{anyhow, Result};
use image::{DynamicImage, RgbImage};

use crate::project::Line;

static FONT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../python/fonts/Inter-Bold.ttf"));

fn face() -> Option<ttf_parser::Face<'static>> {
    ttf_parser::Face::parse(FONT, 0).ok()
}

fn glyph(f: &ttf_parser::Face, c: char) -> Option<ttf_parser::GlyphId> {
    f.glyph_index(c).or_else(|| f.glyph_index('?'))
}

pub(crate) fn text_width(text: &str, size: f32) -> f32 {
    let Some(f) = face() else { return 0. };
    let s = size / f.units_per_em() as f32;
    text.chars().filter_map(|c| glyph(&f, c)).map(|g| f.glyph_hor_advance(g).unwrap_or(0) as f32 * s).sum()
}

/// `text` broken into lines no wider than `max_w` at `size` (a word longer than that keeps its
/// own line).
pub(crate) fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let next = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if text_width(&next, size) <= max_w || cur.is_empty() {
            cur = next;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
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

fn paint(rgba: [u8; 4]) -> tiny_skia::Paint<'static> {
    let mut p = tiny_skia::Paint::default();
    p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    p.anti_alias = true;
    p
}

/// An opaque image being drawn on.
pub(crate) struct Canvas(tiny_skia::Pixmap);

impl Canvas {
    pub(crate) fn from_rgb(img: RgbImage) -> Result<Canvas> {
        let (w, h) = img.dimensions();
        let rgba = DynamicImage::ImageRgb8(img).to_rgba8().into_raw();
        let size = tiny_skia::IntSize::from_wh(w, h).ok_or_else(|| anyhow!("empty image"))?;
        Ok(Canvas(tiny_skia::Pixmap::from_vec(rgba, size).ok_or_else(|| anyhow!("image too large"))?))
    }

    pub(crate) fn into_rgb(self) -> Result<RgbImage> {
        let (w, h) = (self.0.width(), self.0.height());
        let rgba = image::RgbaImage::from_raw(w, h, self.0.take()).ok_or_else(|| anyhow!("bad image buffer"))?;
        Ok(DynamicImage::ImageRgba8(rgba).to_rgb8())
    }

    fn fill(&mut self, path: &tiny_skia::Path, rgba: [u8; 4]) {
        self.0.fill_path(path, &paint(rgba), tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }

    fn stroke_with(&mut self, path: &tiny_skia::Path, stroke: tiny_skia::Stroke, rgba: [u8; 4]) {
        self.0.stroke_path(path, &paint(rgba), &stroke, tiny_skia::Transform::identity(), None);
    }

    fn stroke(&mut self, path: &tiny_skia::Path, width: f32, rgba: [u8; 4]) {
        self.stroke_with(path, tiny_skia::Stroke { width, line_join: tiny_skia::LineJoin::Round, ..Default::default() }, rgba);
    }

    pub(crate) fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, rgba: [u8; 4]) {
        if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            self.stroke(&tiny_skia::PathBuilder::from_rect(r), width, rgba);
        }
    }

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
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Speech,
    Shout,
    Thought,
    Whisper,
    Narration,
}

fn kind(k: &str) -> Kind {
    match k.trim().to_lowercase().as_str() {
        "shout" | "yell" | "scream" => Kind::Shout,
        "thought" | "think" | "thinking" => Kind::Thought,
        "whisper" => Kind::Whisper,
        "narration" | "caption" | "narrator" => Kind::Narration,
        _ => Kind::Speech,
    }
}

/// A closed spiky outline around an ellipse (a shout).
fn burst(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    let n = 22;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let k = if i % 2 == 0 { 1.16 } else { 0.96 };
        let (x, y) = (cx + a.cos() * rx * k, cy + a.sin() * ry * k);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

fn tail(from: (f32, f32), base_w: f32, to: (f32, f32)) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(from.0 - base_w / 2., from.1);
    pb.line_to(to.0, to.1);
    pb.line_to(from.0 + base_w / 2., from.1);
    pb.close();
    pb.finish()
}

/// Draws `lines` inside the panel `(x, y, w, h)`: the first at the top on the side reading
/// starts from, the next ones stepping down and across, so they read in order.
pub(crate) fn balloons(c: &mut Canvas, lines: &[Line], (px, py, pw, ph): (f32, f32, f32, f32), rtl: bool) {
    let ink = [0, 0, 0, 255];
    let paper = [255, 255, 255, 255];
    let page_w = c.0.width() as f32;
    let inset = pw.min(ph) * 0.04;
    let mut cursor = py + inset;
    for (i, l) in lines.iter().filter(|l| !l.text.trim().is_empty()).enumerate() {
        let k = kind(&l.kind);
        let start_side = (i % 2 == 0) == rtl;
        let mut size = page_w * if k == Kind::Shout { 0.021 } else { 0.0175 };
        let max_text = (pw * if k == Kind::Narration { 0.6 } else { 0.42 }).max(page_w * 0.09);
        let text = if k == Kind::Narration { l.text.trim().to_string() } else { l.text.trim().to_uppercase() };
        let mut wrapped = wrap(&text, size, max_text);
        // Shrink long lines into small panels rather than spilling out.
        while wrapped.len() as f32 * size * 1.2 > ph * 0.45 && size > page_w * 0.009 {
            size *= 0.88;
            wrapped = wrap(&text, size, max_text);
        }
        let line_h = size * 1.2;
        let tw = wrapped.iter().map(|t| text_width(t, size)).fold(0., f32::max);
        let th = wrapped.len() as f32 * line_h;
        let (bw, bh) = if k == Kind::Narration { (tw + size * 1.4, th + size * 1.0) } else { (tw * 1.32 + size * 1.6, th * 1.4 + size * 1.2) };
        let bx = if start_side { px + pw - inset - bw } else { px + inset };
        let by = cursor.min(py + ph - inset - bh).max(py + inset);
        if by + bh > py + ph + 1. {
            break;
        }
        let (cx, cy) = (bx + bw / 2., by + bh / 2.);
        let border = (page_w * 0.0022).max(1.5);
        match k {
            Kind::Narration => {
                if let Some(r) = tiny_skia::Rect::from_xywh(bx, by, bw, bh) {
                    let p = tiny_skia::PathBuilder::from_rect(r);
                    c.fill(&p, paper);
                    c.stroke(&p, border, ink);
                }
            }
            Kind::Shout => {
                if let Some(p) = burst(cx, cy, bw / 2., bh / 2.) {
                    c.fill(&p, paper);
                    c.stroke(&p, border * 1.3, ink);
                }
            }
            _ => {
                let to = (cx + if start_side { -bw * 0.12 } else { bw * 0.12 }, by + bh + ph * 0.07);
                if k != Kind::Thought {
                    if let Some(t) = tail((cx, by + bh * 0.9), bw * 0.12, (to.0, to.1.min(py + ph - inset))) {
                        c.fill(&t, paper);
                        c.stroke(&t, border, ink);
                    }
                }
                if let Some(r) = tiny_skia::Rect::from_xywh(bx, by, bw, bh) {
                    if let Some(p) = tiny_skia::PathBuilder::from_oval(r) {
                        c.fill(&p, paper);
                        if k == Kind::Whisper {
                            let dash = tiny_skia::StrokeDash::new(vec![border * 4., border * 3.], 0.);
                            c.stroke_with(&p, tiny_skia::Stroke { width: border, dash, ..Default::default() }, ink);
                        } else {
                            c.stroke(&p, border, ink);
                        }
                    }
                }
                if k == Kind::Thought {
                    // Bubbles trailing toward the thinker.
                    for (j, s) in [0.09f32, 0.06, 0.04].iter().enumerate() {
                        let d = bh * (0.15 + j as f32 * 0.35);
                        let r = bw.min(bh) * s;
                        if let Some(rect) = tiny_skia::Rect::from_xywh(cx + if start_side { -d } else { d } - r, by + bh + d * 0.6 - r, r * 2., r * 2.) {
                            if let Some(p) = tiny_skia::PathBuilder::from_oval(rect) {
                                c.fill(&p, paper);
                                c.stroke(&p, border * 0.8, ink);
                            }
                        }
                    }
                }
            }
        }
        let ascent = face().map(|f| f.ascender() as f32 / f.units_per_em() as f32).unwrap_or(0.9) * size;
        let mut base = cy - th / 2. + ascent * 0.92;
        for t in &wrapped {
            let x = if k == Kind::Narration { bx + size * 0.7 } else { cx - text_width(t, size) / 2. };
            c.text(t, size, x, base, ink);
            base += line_h;
        }
        cursor = by + bh * 0.72;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_to_width() {
        let lines = wrap("THIS IS A LONG LINE OF DIALOGUE THAT HAS TO WRAP", 30., 200.);
        assert!(lines.len() > 2);
        assert!(lines.iter().all(|l| text_width(l, 30.) <= 200. || !l.contains(' ')));
        assert_eq!(wrap("", 30., 200.), Vec::<String>::new());
        assert!(text_width("WWW", 30.) > text_width("iii", 30.));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind("Shout"), Kind::Shout);
        assert_eq!(kind("caption"), Kind::Narration);
        assert_eq!(kind(""), Kind::Speech);
    }
}
