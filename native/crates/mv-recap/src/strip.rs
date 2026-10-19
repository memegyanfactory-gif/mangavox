//! Vertical-strip chapters (webtoon / manhwa) are often delivered as fixed-height slices that cut straight
//! through panels and speech balloons, so a slice read on its own comes back "cut off at the bottom".
//! Such a chapter is stitched back into one strip and re-cut at the gaps between panels; every part is read
//! whole and its panels are then mapped back onto the original page files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{GrayImage, RgbImage};

use crate::types::Bbox;

/// Rows of one page file that belong to a [`Unit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Span {
    pub page: usize,
    pub y0: u32,
    pub y1: u32,
    /// Full height of that page.
    pub h: u32,
}

/// What the model reads in one call: one whole page, or a stretch of a strip stitched from several pages.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Unit {
    pub spans: Vec<Span>,
    /// `false` = an ordinary page, sent as it is.
    pub stitched: bool,
}

impl Unit {
    pub fn page(i: usize) -> Unit {
        Unit { spans: vec![Span { page: i, y0: 0, y1: 0, h: 0 }], stitched: false }
    }
    pub fn first(&self) -> usize {
        self.spans[0].page
    }
    pub fn pages(&self) -> Vec<usize> {
        self.spans.iter().map(|s| s.page).collect()
    }
    /// Stable id stored in `read.json` so a cancelled read resumes where it stopped.
    pub fn key(&self) -> (usize, u32) {
        (self.spans[0].page, self.spans[0].y0)
    }
    fn height(&self) -> u32 {
        self.spans.iter().map(|s| s.y1 - s.y0).sum()
    }

    /// Map a box on the unit image to `(span index, box on that span's page)`. The panel goes to the page it
    /// overlaps most; the page box is clamped to that page.
    pub fn to_page(&self, b: Bbox) -> Option<(usize, Option<Bbox>)> {
        if !self.stitched {
            return Some((0, Some(b)));
        }
        let hh = self.height() as f32;
        let [x0, y0, x1, y1] = b.0;
        let (by0, by1) = (y0 * hh, y1 * hh);
        let mut off = 0.0;
        let mut best: Option<(usize, f32, f32)> = None;
        for (n, s) in self.spans.iter().enumerate() {
            let len = (s.y1 - s.y0) as f32;
            let overlap = (by1.min(off + len) - by0.max(off)).max(0.0);
            if best.is_none_or(|(_, o, _)| overlap > o) {
                best = Some((n, overlap, off));
            }
            off += len;
        }
        let (n, _, off) = best?;
        let s = self.spans[n];
        let to_page = |y: f32| (s.y0 as f32 + (y - off)) / s.h.max(1) as f32;
        Some((n, Bbox([x0, to_page(by0), x1, to_page(by1)]).normalized()))
    }

    /// Re-express a box on the unit image as a box on page span `n` (for face boxes).
    pub fn box_on_span(&self, n: usize, b: Bbox) -> Option<Bbox> {
        if !self.stitched {
            return Some(b);
        }
        let hh = self.height() as f32;
        let off: u32 = self.spans[..n].iter().map(|s| s.y1 - s.y0).sum();
        let s = self.spans[n];
        let to_page = |y: f32| (s.y0 as f32 + (y * hh - off as f32)) / s.h.max(1) as f32;
        Bbox([b.0[0], to_page(b.0[1]), b.0[2], to_page(b.0[3])]).normalized()
    }
}

struct Scan {
    w: u32,
    /// Per row: brightest minus darkest pixel. A low spread is a flat gap between panels.
    spread: Vec<u8>,
    top: Vec<u8>,
    bottom: Vec<u8>,
}

/// Rows at most this uneven count as blank (JPEG noise on a flat background stays well below it).
const FLAT: u8 = 32;

fn row_spread(img: &GrayImage, y: u32) -> u8 {
    let (mut lo, mut hi) = (255u8, 0u8);
    for x in 0..img.width() {
        let v = img.get_pixel(x, y).0[0];
        lo = lo.min(v);
        hi = hi.max(v);
    }
    hi.saturating_sub(lo)
}

fn row(img: &GrayImage, y: u32) -> Vec<u8> {
    (0..img.width()).map(|x| img.get_pixel(x, y).0[0]).collect()
}

fn scan(path: &Path) -> Option<Scan> {
    let img = image::open(path).ok()?.to_luma8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    Some(Scan { w, spread: (0..h).map(|y| row_spread(&img, y)).collect(), top: row(&img, 0), bottom: row(&img, h - 1) })
}

/// The art runs on from the bottom edge of `a` into the top edge of `b` (the slice cut through a panel).
fn continues(a: &Scan, b: &Scan) -> bool {
    if a.spread.last().is_some_and(|s| *s <= FLAT) || b.spread.first().is_some_and(|s| *s <= FLAT) {
        return false;
    }
    let diff: u64 = a.bottom.iter().zip(&b.top).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    diff / a.w.max(1) as u64 <= 20
}

/// Global rows (all pages stacked) where to cut, each part between `min` and `max` rows tall: in the
/// middle of the longest gap between panels, or on the flattest row when a stretch has no gap at all.
pub(crate) fn cut_points(spread: &[u8], min: usize, max: usize) -> Vec<usize> {
    let total = spread.len();
    let mut cuts = vec![];
    let mut start = 0;
    while total - start > max {
        let (lo, hi) = (start + min, (start + max).min(total));
        let mut best = (0, (lo..hi).min_by_key(|&y| spread[y]).unwrap_or(hi)); // (gap length, cut row)
        let mut y = lo;
        while y < hi {
            if spread[y] > FLAT {
                y += 1;
                continue;
            }
            let run_start = y;
            while y < hi && spread[y] <= FLAT {
                y += 1;
            }
            if y - run_start > best.0 {
                best = (y - run_start, (run_start + y) / 2);
            }
        }
        cuts.push(best.1);
        start = best.1;
    }
    cuts
}

/// Plan how a chapter is read. Ordinary pages are one unit each; a sliced strip is re-cut at panel gaps.
pub(crate) fn plan(pages: &[PathBuf]) -> Vec<Unit> {
    let ordinary = || (0..pages.len()).map(Unit::page).collect();
    if pages.len() < 2 {
        return ordinary();
    }
    let Some(scans) = pages.iter().map(|p| scan(p)).collect::<Option<Vec<_>>>() else { return ordinary() };
    let w = scans[0].w;
    if scans.iter().any(|s| s.w != w) {
        return ordinary();
    }
    let joins = scans.windows(2).filter(|p| continues(&p[0], &p[1])).count();
    if joins == 0 || joins * 4 < scans.len() - 1 {
        return ordinary();
    }

    let spread: Vec<u8> = scans.iter().flat_map(|s| s.spread.iter().copied()).collect();
    let mut bounds = vec![0];
    bounds.extend(cut_points(&spread, w as usize, w as usize * 5 / 2));
    bounds.push(spread.len());
    let mut offsets = vec![0usize];
    for s in &scans {
        offsets.push(offsets.last().unwrap() + s.spread.len());
    }
    bounds
        .windows(2)
        .filter(|b| b[1] > b[0])
        .map(|b| {
            let spans = scans
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let (p0, p1) = (offsets[i], offsets[i + 1]);
                    let (a, z) = (b[0].max(p0), b[1].min(p1));
                    (z > a).then(|| Span { page: i, y0: (a - p0) as u32, y1: (z - p0) as u32, h: s.spread.len() as u32 })
                })
                .collect();
            Unit { spans, stitched: true }
        })
        .collect()
}

/// Stitch the unit's rows into one full-resolution image at `out`.
pub(crate) fn compose(unit: &Unit, pages: &[PathBuf], out: &Path) -> Result<()> {
    let mut canvas: Option<RgbImage> = None;
    let mut y = 0;
    for s in &unit.spans {
        let img = image::open(&pages[s.page]).with_context(|| format!("open {}", pages[s.page].display()))?.to_rgb8();
        let c = canvas.get_or_insert_with(|| RgbImage::from_pixel(img.width(), unit.height(), image::Rgb([255, 255, 255])));
        let part = image::imageops::crop_imm(&img, 0, s.y0, img.width().min(c.width()), s.y1 - s.y0).to_image();
        image::imageops::replace(c, &part, 0, y as i64);
        y += s.y1 - s.y0;
    }
    let canvas = canvas.context("empty strip part")?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    canvas.save_with_format(out, image::ImageFormat::Jpeg).with_context(|| format!("write {}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100px-wide strip of `rows` rows: art (noise) everywhere except white gaps at `gaps`.
    fn strip_rows(rows: usize, gaps: &[std::ops::Range<usize>]) -> RgbImage {
        RgbImage::from_fn(100, rows as u32, |x, y| {
            if gaps.iter().any(|g| g.contains(&(y as usize))) {
                image::Rgb([255, 255, 255])
            } else {
                let v = ((x * 7 + y * 3) % 200) as u8;
                image::Rgb([v, v, v])
            }
        })
    }

    #[test]
    fn sliced_strip_is_recut_at_panel_gaps() {
        let dir = tempfile::tempdir().unwrap();
        // 600 rows sliced into 4 files of 150; the only real gap is rows 180-200 (inside file 2).
        let full = strip_rows(600, &[180..200]);
        let pages: Vec<PathBuf> = (0..4)
            .map(|n| {
                let p = dir.path().join(format!("{n:03}.png"));
                image::imageops::crop_imm(&full, 0, n * 150, 100, 150).to_image().save(&p).unwrap();
                p
            })
            .collect();
        let units = plan(&pages);
        assert!(units.iter().all(|u| u.stitched));
        // min 100 / max 250 rows: first cut lands in the gap (row 190), then on the flattest row (no gap), then the rest.
        assert_eq!(units[0].spans, vec![Span { page: 0, y0: 0, y1: 150, h: 150 }, Span { page: 1, y0: 0, y1: 40, h: 150 }]);
        assert_eq!(units[1].spans[0], Span { page: 1, y0: 40, y1: 140, h: 150 });
        assert_eq!(units.iter().map(|u| u.height()).sum::<u32>(), 600);

        // A panel straddling the file boundary maps to the page it mostly lies on, clamped to it.
        let (n, b) = units[0].to_page(Bbox([0.0, 100.0 / 190.0, 1.0, 170.0 / 190.0])).unwrap();
        assert_eq!(n, 0);
        let b = b.unwrap().0;
        assert!((b[1] - 100.0 / 150.0).abs() < 1e-4 && b[3] == 1.0);

        let out = dir.path().join("part.jpg");
        compose(&units[0], &pages, &out).unwrap();
        assert_eq!(image::image_dimensions(&out).unwrap(), (100, 190));
    }

    #[test]
    fn ordinary_pages_stay_separate() {
        let dir = tempfile::tempdir().unwrap();
        // Separate pages with white margins top and bottom never "continue" into each other.
        let pages: Vec<PathBuf> = (0..3)
            .map(|n| {
                let p = dir.path().join(format!("{n}.png"));
                strip_rows(150, &[0..10, 140..150]).save(&p).unwrap();
                p
            })
            .collect();
        assert_eq!(plan(&pages), vec![Unit::page(0), Unit::page(1), Unit::page(2)]);
    }

    #[test]
    #[ignore = "reads a local library chapter: set MV_STRIP_DIR"]
    fn plan_real_chapter() {
        let dir = std::env::var("MV_STRIP_DIR").unwrap();
        let pages = crate::store::list_pages(Path::new(&dir));
        let units = plan(&pages);
        println!("{} pages -> {} units", pages.len(), units.len());
        for u in &units {
            println!("{:?} h={}", u.spans.iter().map(|s| (s.page + 1, s.y0, s.y1)).collect::<Vec<_>>(), u.height());
        }
    }
}
