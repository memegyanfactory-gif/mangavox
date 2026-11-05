//! Pages that repeat the chapter before: the same art again. A chapter that opens by replaying the
//! last one's ending, a prologue told again in full, or the same chapter uploaded twice by two
//! groups (other words, other page cuts, the same pictures). Found by difference hashes of the
//! panel crops, so a new translation or a new way of slicing the strip does not hide it. The
//! writer never tells them again and the video cuts them; recap channels never retell them.

use std::collections::BTreeMap;
use std::path::Path;

/// Bits two crops may differ by and still be the same picture.
const SAME_PICTURE_BITS: u32 = 10;
/// A page repeats when this share of its pictures does.
const PAGE_SHARE: f64 = 0.6;
/// Repeated pages this close together are one stretch (another slicing of the strip leaves a few
/// pages in between unmatched)...
const STRETCH_GAP: usize = 6;
/// ...and a stretch counts when it has this many repeated pages (a single reused picture, like a
/// flashback panel, does not).
const STRETCH_PAGES: usize = 3;
/// A chapter whose pages repeat at this share is the chapter before told again.
const DUPLICATE_SHARE: f64 = 0.5;
/// Brightness spread (0-255) under which a crop is flat (blank, black or white) and says nothing.
const FLAT_SPREAD: f64 = 16.0;

/// What a chapter repeats of the chapter before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Repeats {
    None,
    /// These pages (0-based) retell it.
    Pages(Vec<usize>),
    /// Most of it does: the same chapter again.
    Chapter,
}

/// Difference hash (64 bits) of a picture, `None` for a flat one.
pub(crate) fn dhash(path: &Path) -> Option<u64> {
    let small = image::open(path).ok()?.thumbnail(64, 64).to_luma8();
    let n = small.pixels().len().max(1) as f64;
    let mean = small.pixels().map(|p| p.0[0] as f64).sum::<f64>() / n;
    let spread = (small.pixels().map(|p| (p.0[0] as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
    if spread < FLAT_SPREAD {
        return None;
    }
    let g = image::imageops::resize(&small, 9, 8, image::imageops::FilterType::Triangle);
    let mut h = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            h = (h << 1) | u64::from(g.get_pixel(x, y).0[0] > g.get_pixel(x + 1, y).0[0]);
        }
    }
    Some(h)
}

/// The hashes of the panel crops in `dir` (`pPPPP_NN.jpg`) by page, pages with a picture only.
pub(crate) fn panel_hashes(dir: &Path) -> BTreeMap<usize, Vec<u64>> {
    let mut out: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(page) = name.strip_prefix('p').and_then(|r| r.split_once('_')).filter(|(_, rest)| rest.ends_with(".jpg")).and_then(|(p, _)| p.parse::<usize>().ok()) else { continue };
        if let Some(h) = dhash(&e.path()) {
            out.entry(page).or_default().push(h);
        }
    }
    out
}

/// What the pages of `this` chapter (hashes by page) repeat of the chapter `before` it.
pub(crate) fn repeated_pages(this: &BTreeMap<usize, Vec<u64>>, before: &BTreeMap<usize, Vec<u64>>) -> Repeats {
    let known: Vec<u64> = before.values().flatten().copied().collect();
    if known.is_empty() || this.is_empty() {
        return Repeats::None;
    }
    let seen = |h: &u64| known.iter().any(|k| (h ^ k).count_ones() <= SAME_PICTURE_BITS);
    let matched: Vec<usize> = this.iter().filter(|(_, hs)| !hs.is_empty() && hs.iter().filter(|h| seen(h)).count() as f64 >= hs.len() as f64 * PAGE_SHARE).map(|(p, _)| *p).collect();
    if matched.len() as f64 >= this.len() as f64 * DUPLICATE_SHARE {
        return Repeats::Chapter;
    }
    let mut pages = vec![];
    let mut stretch: Vec<usize> = vec![];
    let mut flush = |stretch: &mut Vec<usize>| {
        if stretch.len() >= STRETCH_PAGES {
            pages.extend(stretch[0]..=stretch[stretch.len() - 1]);
        }
        stretch.clear();
    };
    for &p in &matched {
        if stretch.last().is_some_and(|&l| p - l > STRETCH_GAP) {
            flush(&mut stretch);
        }
        stretch.push(p);
    }
    flush(&mut stretch);
    if pages.is_empty() {
        Repeats::None
    } else {
        Repeats::Pages(pages)
    }
}

/// What `this` chapter's panels repeat of `before`'s (their panel crop folders).
pub(crate) fn repeats_between(this: &Path, before: &Path) -> Repeats {
    let b = panel_hashes(before);
    if b.is_empty() {
        return Repeats::None;
    }
    repeated_pages(&panel_hashes(this), &b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(list: &[(usize, &[u64])]) -> BTreeMap<usize, Vec<u64>> {
        list.iter().map(|(p, hs)| (*p, hs.to_vec())).collect()
    }

    #[test]
    fn a_chapter_opening_on_the_last_ones_ending_leaves_those_pages_out() {
        let before = pages(&[(40, &[0xAAAA_0000_FFFF_1234]), (41, &[0x0F0F_0F0F_0F0F_0F0F]), (42, &[0x1234_5678_9ABC_DEF0]), (43, &[0xFEDC_BA98_7654_3210])]);
        let mut this: Vec<(usize, Vec<u64>)> = vec![
            (0, vec![0xAAAA_0000_FFFF_1235]),                     // one bit off: the same picture
            (1, vec![0x0F0F_0F0F_0F0F_0F0F, 0x0F0F_0F0F_0F0F_0F0F]),
            (2, vec![0x5555_5555_5555_5555]),                     // a new cut of the strip in between
            (3, vec![0x1234_5678_9ABC_DEF0]),
            (4, vec![0xFEDC_BA98_7654_3210]),
        ];
        // The new pages: pictures never seen before.
        this.extend((5..20).map(|p| (p, vec![0x9999_0000_0000_0000u64.rotate_left(p as u32 * 7) ^ 0x00FF_00FF_00FF_00FF])));
        let this: BTreeMap<usize, Vec<u64>> = this.into_iter().collect();
        assert_eq!(repeated_pages(&this, &before), Repeats::Pages(vec![0, 1, 2, 3, 4]));
    }

    #[test]
    fn a_single_reused_picture_is_no_repeat_and_most_pages_again_is_the_chapter_again() {
        let before = pages(&[(10, &[0x1111_2222_3333_4444]), (11, &[0x0F0F_0F0F_0F0F_0F0F])]);
        let new = |p: usize| 0xABCD_EF01_2345_6789u64.rotate_left(p as u32 * 5) ^ (p as u64);
        let mut this: BTreeMap<usize, Vec<u64>> = (0..12).map(|p| (p, vec![new(p)])).collect();
        this.insert(6, vec![0x1111_2222_3333_4444]);
        assert_eq!(repeated_pages(&this, &before), Repeats::None, "a flashback panel shown again is told");
        let again: BTreeMap<usize, Vec<u64>> = (0..10).map(|p| (p, vec![if p % 3 == 2 { new(p) } else { 0x0F0F_0F0F_0F0F_0F0F }])).collect();
        assert_eq!(repeated_pages(&again, &before), Repeats::Chapter);
        assert_eq!(repeated_pages(&again, &BTreeMap::new()), Repeats::None, "nothing to compare with");
    }

    #[test]
    fn crops_are_hashed_by_page_and_flat_ones_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let wave = |x: u32, y: u32| (128.0 + 100.0 * (x as f32 / 15.0).sin() * (y as f32 / 21.0).cos()) as u8;
        let busy = image::RgbImage::from_fn(120, 160, |x, y| image::Rgb([wave(x, y), wave(y, x), wave(x + 30, y)]));
        busy.save(dir.path().join("p0003_00.jpg")).unwrap();
        busy.save(dir.path().join("p0003_01.jpg")).unwrap();
        image::RgbImage::from_pixel(64, 64, image::Rgb([250, 250, 250])).save(dir.path().join("p0004_00.jpg")).unwrap();
        image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(dir.path().join("cover.png")).unwrap();
        let h = panel_hashes(dir.path());
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), vec![3], "the blank page and other files are left out");
        assert_eq!(h[&3].len(), 2);
        assert_eq!(h[&3][0], h[&3][1]);
        // A resized copy of the same picture hashes (nearly) the same.
        let small = image::imageops::resize(&busy, 60, 80, image::imageops::FilterType::Triangle);
        small.save(dir.path().join("small.jpg")).unwrap();
        assert!((dhash(&dir.path().join("small.jpg")).unwrap() ^ h[&3][0]).count_ones() <= SAME_PICTURE_BITS);
    }
}
