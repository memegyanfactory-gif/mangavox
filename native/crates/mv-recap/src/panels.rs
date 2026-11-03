//! Panel frames found on the page image itself. The reader's panel boxes are rough: they spill over
//! the gutters into the next panel or stop short of the frame, so crops showed slivers of other
//! panels. The white gutter is flooded in from the page edges; every patch of the page the flood
//! cannot reach (a framed panel, slanted or not) is a frame, and a reader box that clearly matches
//! one is snapped to it. Black gutters, borderless art and panels joined by a balloon across the
//! gutter find no clean frame: their box stays as read.

use std::collections::VecDeque;

use image::{imageops::FilterType, DynamicImage};

use crate::types::Bbox;

/// Width the page is scanned at.
const WORK_W: u32 = 600;
/// Gutter white: anything at least this light.
const WHITE: u8 = 200;
/// Frames smaller than this share of the page are noise (a page number, a stray mark).
const MIN_AREA: f32 = 0.008;
/// A reader box takes a frame that overlaps it at least this much (intersection over union)...
const MIN_IOU: f32 = 0.5;
/// ... and is not much bigger than it (two panels joined across a broken gutter).
const MAX_GROWTH: f32 = 1.6;

/// The panel frames of a page, as boxes on the whole page.
pub(crate) fn detect(img: &DynamicImage) -> Vec<Bbox> {
    let small = if img.width() > WORK_W { img.resize(WORK_W, u32::MAX, FilterType::Triangle) } else { img.clone() };
    let g = small.to_luma8();
    let (w, h) = (g.width() as usize, g.height() as usize);
    if w < 16 || h < 16 {
        return vec![];
    }
    let white: Vec<bool> = g.pixels().map(|p| p.0[0] >= WHITE).collect();
    let near = |i: usize| {
        let (x, y) = (i % w, i / w);
        [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
    };
    // The gutter: white reached from the page edges.
    let mut seen = vec![false; w * h];
    let mut q: VecDeque<usize> = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1])).filter(|&i| white[i]).collect();
    for &i in &q {
        seen[i] = true;
    }
    while let Some(i) = q.pop_front() {
        for n in near(i) {
            if white[n] && !seen[n] {
                seen[n] = true;
                q.push_back(n);
            }
        }
    }
    // Every patch the gutter does not reach is one frame: its bounding box.
    let mut out = vec![];
    for start in 0..w * h {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        q.push_back(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        while let Some(i) = q.pop_front() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            for n in near(i) {
                if !seen[n] {
                    seen[n] = true;
                    q.push_back(n);
                }
            }
        }
        let b = Bbox([x0 as f32 / w as f32, y0 as f32 / h as f32, (x1 + 1) as f32 / w as f32, (y1 + 1) as f32 / h as f32]);
        if b.area() >= MIN_AREA {
            out.push(b);
        }
    }
    out
}

pub(crate) fn iou(a: Bbox, b: Bbox) -> f32 {
    let [ax0, ay0, ax1, ay1] = a.0;
    let [bx0, by0, bx1, by1] = b.0;
    let inter = (ax1.min(bx1) - ax0.max(bx0)).max(0.0) * (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let union = a.area() + b.area() - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// `b` snapped to the frame in `frames` it clearly is, else `b` as read.
pub(crate) fn snap(frames: &[Bbox], b: Bbox) -> Bbox {
    frames
        .iter()
        .map(|f| (iou(*f, b), *f))
        .filter(|(o, f)| *o >= MIN_IOU && f.area() <= b.area() * MAX_GROWTH)
        .max_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, f)| f)
        .unwrap_or(b)
}
