//! Panels the user cropped by hand in Studio's Read step. The boxes live in
//! `studio/manual_panels.json`, the user's own work: Redo from Read keeps it. A page with boxes
//! there gets them as its panels in `read.json` (listed in [`ChapterRead::manual`]) and crops cut
//! exactly to them, and the automatic work (frame detection, snapping, the reader's crops) skips
//! it. The reader reads the page exactly as any other (same prompt): each line and character it
//! finds goes to the user's box it lies in. The reader's own panels are kept ([`ChapterRead::auto_panels`]) so "Reset to
//! automatic" brings them back without a new read.

use std::path::Path;

use anyhow::{Context, Result};
use image::DynamicImage;
use serde::{Deserialize, Serialize};

use crate::store::*;
use crate::types::*;

const FILE: &str = "manual_panels.json";

/// The panels the user drew on one page, normalised to the whole page, in reading order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualPage {
    /// 0-based position in [`crate::list_pages`].
    pub index: usize,
    /// The page file the boxes were drawn on; boxes for another file at that index do not apply.
    pub file: String,
    pub boxes: Vec<Bbox>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManualPanels {
    /// Sorted by `index`.
    pub pages: Vec<ManualPage>,
}

impl ManualPanels {
    /// The user's boxes for a page, when it has any and they were drawn on this file.
    pub fn boxes(&self, index: usize, file: &str) -> Option<&[Bbox]> {
        self.pages.iter().find(|p| p.index == index && (p.file.is_empty() || file.is_empty() || p.file == file)).filter(|p| !p.boxes.is_empty()).map(|p| p.boxes.as_slice())
    }
}

pub fn load_manual(ch: &ChapterRef) -> ManualPanels {
    std::fs::read(chapter_dir(ch).join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_manual(ch: &ChapterRef, m: &ManualPanels) -> Result<()> {
    write_json_atomic(&chapter_dir(ch).join(FILE), m)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
}

/// Share of `b` inside `a`.
fn covered(a: Bbox, b: Bbox) -> f32 {
    let [ax0, ay0, ax1, ay1] = a.0;
    let [bx0, by0, bx1, by1] = b.0;
    let inter = (ax1.min(bx1) - ax0.max(bx0)).max(0.0) * (ay1.min(by1) - ay0.max(by0)).max(0.0);
    if b.area() <= 0.0 {
        0.0
    } else {
        inter / b.area()
    }
}

/// The user's box a reader box belongs to: the one covering most of it, else the nearest centre.
fn owner(boxes: &[Bbox], b: Bbox) -> usize {
    let centre = |x: Bbox| ((x.0[0] + x.0[2]) / 2.0, (x.0[1] + x.0[3]) / 2.0);
    let best = (0..boxes.len()).max_by(|&i, &j| covered(boxes[i], b).total_cmp(&covered(boxes[j], b))).unwrap_or(0);
    if covered(boxes[best], b) > 0.0 {
        return best;
    }
    let (cx, cy) = centre(b);
    let dist = |i: usize| {
        let (x, y) = centre(boxes[i]);
        (x - cx).powi(2) + (y - cy).powi(2)
    };
    (0..boxes.len()).min_by(|&i, &j| dist(i).total_cmp(&dist(j))).unwrap_or(0)
}

/// The page's panels as the user's boxes: what the reader found in each panel (`from`, in reading
/// order) goes to the box it lies in; a panel without a box joins the box before it.
pub(crate) fn fit(from: &[Panel], boxes: &[Bbox]) -> Vec<Panel> {
    let mut out: Vec<Panel> = boxes.iter().map(|b| Panel { bbox: Some(*b), ..Default::default() }).collect();
    if boxes.is_empty() {
        return out;
    }
    let mut last = 0;
    for p in from {
        let n = p.bbox.map(|b| owner(boxes, b)).unwrap_or(last);
        last = n;
        let t = &mut out[n];
        if !p.description.is_empty() {
            t.description = if t.description.is_empty() { p.description.clone() } else { format!("{} {}", t.description, p.description) };
        }
        for c in &p.characters {
            if !t.characters.iter().any(|x| x.name == c.name) {
                t.characters.push(c.clone());
            }
        }
        t.lines.extend(p.lines.iter().cloned());
    }
    out
}

/// Gives a read page the user's panels, keeping the reader's own the first time.
pub(crate) fn apply(read: &mut ChapterRead, index: usize, boxes: &[Bbox]) {
    let Some(page) = read.pages.iter_mut().find(|p| p.index == index) else { return };
    if !read.manual.contains(&index) {
        read.auto_panels.retain(|(i, _)| *i != index);
        read.auto_panels.push((index, page.panels.clone()));
        read.manual.push(index);
        read.manual.sort();
    }
    page.panels = fit(&page.panels, boxes);
}

/// After a read: the hand-cropped pages it read take the user's boxes. A page read afresh
/// (`fresh`) keeps the new reading's own panels for "Reset to automatic".
pub(crate) fn fold(read: &mut ChapterRead, hand: &[(usize, Vec<Bbox>)], fresh: impl Fn(usize) -> bool) {
    for (page, boxes) in hand {
        if fresh(*page) {
            read.manual.retain(|i| i != page);
        }
        apply(read, *page, boxes);
    }
}

/// The crops of the hand-cropped pages a read went over (blocking).
pub(crate) fn write_all(ch: &ChapterRef, pages: &[std::path::PathBuf], hand: &[(usize, Vec<Bbox>)]) -> Result<()> {
    for (page, boxes) in hand {
        let path = pages.get(*page).with_context(|| format!("page {} is missing", page + 1))?;
        write_crops(ch, *page, path, boxes)?;
    }
    Ok(())
}

/// Exact crop of a box drawn by the user.
fn crop_exact(img: &DynamicImage, b: Bbox, out: &Path) -> Result<()> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let [x0, y0, x1, y1] = b.0;
    let (l, t) = ((x0 * w).floor().max(0.0), (y0 * h).floor().max(0.0));
    let (r, bt) = ((x1 * w).ceil().min(w), (y1 * h).ceil().min(h));
    let c = img.crop_imm(l as u32, t as u32, ((r - l) as u32).max(1), ((bt - t) as u32).max(1));
    crate::read::save_jpeg(&c, out, crate::read::KEEP_QUALITY)
}

/// Removes every crop of a page (blocking).
fn clear_crops(ch: &ChapterRef, index: usize) {
    let prefix = format!("p{index:04}_");
    for e in std::fs::read_dir(panels_dir(ch)).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Cuts the crops of a hand-cropped page from its image (blocking).
pub(crate) fn write_crops(ch: &ChapterRef, index: usize, page: &Path, boxes: &[Bbox]) -> Result<()> {
    let img = image::open(page).with_context(|| format!("open {}", page.display()))?;
    clear_crops(ch, index);
    for (n, b) in boxes.iter().enumerate() {
        crop_exact(&img, *b, &panel_path(ch, index, n))?;
    }
    Ok(())
}

/// Saves the user's panels for a page (blocking). An empty list is the same as
/// [`reset_manual_page`]. When the page is already read, its reading takes the new panels and
/// its crops are cut at once, so nothing has to be read again; the story is written from them
/// next time it is written.
pub fn set_manual_page(ch: &ChapterRef, index: usize, boxes: Vec<Bbox>) -> Result<()> {
    let boxes: Vec<Bbox> = boxes.into_iter().filter_map(Bbox::normalized).collect();
    if boxes.is_empty() {
        return reset_manual_page(ch, index).map(|_| ());
    }
    let pages = list_pages(&ch.pages_dir);
    let page = pages.get(index).with_context(|| format!("page {} is not in the chapter", index + 1))?;
    let mut m = load_manual(ch);
    m.pages.retain(|p| p.index != index);
    m.pages.push(ManualPage { index, file: file_name(page), boxes: boxes.clone() });
    m.pages.sort_by_key(|p| p.index);
    save_manual(ch, &m)?;
    if let Some(mut read) = load_read(ch).filter(|r| r.pages.iter().any(|p| p.index == index)) {
        apply(&mut read, index, &boxes);
        save_read(ch, &read)?;
        let _ = crate::transcript::update_transcript(ch, &read);
    }
    write_crops(ch, index, page, &boxes)
}

/// Gives a page back to the automatic panels (blocking): the reader's own panels return, snapped
/// to the frames, with their crops. Returns `true` when the page has to be read again to get them
/// (it was cropped by hand before the reader saw it, and the read kept nothing of its own).
pub fn reset_manual_page(ch: &ChapterRef, index: usize) -> Result<bool> {
    let mut m = load_manual(ch);
    let had = m.pages.len();
    m.pages.retain(|p| p.index != index);
    if m.pages.len() != had {
        save_manual(ch, &m)?;
    }
    let Some(mut read) = load_read(ch) else { return Ok(false) };
    if !read.manual.contains(&index) {
        return Ok(false);
    }
    read.manual.retain(|i| *i != index);
    let auto = read.auto_panels.iter().position(|(i, _)| *i == index).map(|n| read.auto_panels.remove(n).1).unwrap_or_default();
    let again = auto.is_empty();
    clear_crops(ch, index);
    let img = list_pages(&ch.pages_dir).get(index).and_then(|p| image::open(p).ok());
    if let Some(page) = read.pages.iter_mut().find(|p| p.index == index) {
        page.panels = auto;
        if let (Some(img), false) = (&img, again) {
            let frames = crate::panels::detect(img);
            for (n, pn) in page.panels.iter_mut().enumerate() {
                let Some(b) = pn.bbox else { continue };
                let s = crate::panels::snap(&frames, b);
                pn.bbox = Some(s);
                if s.area() >= crate::read::MIN_PANEL_AREA {
                    let _ = crate::read::crop_panel(img, s, &panel_path(ch, index, n));
                }
            }
        }
    }
    save_read(ch, &read)?;
    let _ = crate::transcript::update_transcript(ch, &read);
    Ok(again)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(x0: f32, y0: f32, x1: f32, y1: f32) -> Bbox {
        Bbox([x0, y0, x1, y1])
    }

    fn line(t: &str) -> Line {
        Line { speaker: Some("Kai".into()), text: t.into(), kind: "speech".into() }
    }

    fn panel(b: Option<Bbox>, t: &str) -> Panel {
        Panel { bbox: b, lines: vec![line(t)], characters: vec![CharacterSeen { name: "Kai".into(), ..Default::default() }], description: t.into(), ..Default::default() }
    }

    fn chapter(root: &Path, pages: usize) -> ChapterRef {
        let pages_dir = root.join("pages");
        std::fs::create_dir_all(&pages_dir).unwrap();
        for i in 0..pages {
            image::RgbImage::from_pixel(100, 200, image::Rgb([200, 30, 30])).save(pages_dir.join(format!("{:03}.png", i + 1))).unwrap();
        }
        ChapterRef { chapter_id: "c".into(), manga_id: "m".into(), manga_title: "M".into(), label: "Ch 1".into(), pages_dir, output_dir: root.join("out") }
    }

    #[test]
    fn reader_panels_go_to_the_box_they_lie_in() {
        let boxes = [bx(0.0, 0.0, 1.0, 0.5), bx(0.0, 0.5, 1.0, 1.0)];
        let from = [panel(Some(bx(0.1, 0.6, 0.9, 0.9)), "low"), panel(Some(bx(0.1, 0.1, 0.5, 0.4)), "high"), panel(None, "after high"), panel(Some(bx(0.6, 0.1, 0.9, 0.45)), "high 2")];
        let out = fit(&from, &boxes);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bbox, Some(boxes[0]));
        let texts = |p: &Panel| p.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&out[0]), ["high", "after high", "high 2"]);
        assert_eq!(texts(&out[1]), ["low"]);
        assert_eq!(out[0].characters.len(), 1, "a character is listed once per panel");
        // A box the reader found nothing in is still a panel, and a reader box outside every box goes to the nearest.
        let out = fit(&[panel(Some(bx(0.0, 0.95, 0.1, 1.0)), "edge")], &[bx(0.0, 0.0, 0.5, 0.5), bx(0.2, 0.6, 0.8, 0.9)]);
        assert!(out[0].lines.is_empty());
        assert_eq!(out[1].lines.len(), 1);
    }

    #[test]
    fn manual_crops_replace_the_auto_ones_and_reset_brings_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let ch = chapter(dir.path(), 2);
        let auto = vec![panel(Some(bx(0.0, 0.0, 1.0, 0.3)), "a"), panel(Some(bx(0.0, 0.3, 1.0, 0.6)), "b"), panel(Some(bx(0.0, 0.6, 1.0, 1.0)), "c")];
        let read = ChapterRead { pages: vec![PageRead { index: 0, panels: auto.clone(), ..Default::default() }, PageRead { index: 1, panels: auto.clone(), ..Default::default() }], ..Default::default() };
        save_read(&ch, &read).unwrap();
        for n in 0..3 {
            std::fs::create_dir_all(panels_dir(&ch)).unwrap();
            std::fs::write(panel_path(&ch, 0, n), b"auto").unwrap();
        }
        // A tall box (a long strip panel) and a small one.
        set_manual_page(&ch, 0, vec![bx(0.0, 0.0, 1.0, 0.9), bx(0.2, 0.9, 0.8, 1.0)]).unwrap();
        let r = load_read(&ch).unwrap();
        assert_eq!(r.manual, [0]);
        assert_eq!(r.pages[0].panels.len(), 2);
        assert_eq!(r.pages[0].panels[0].lines.len(), 3);
        assert_eq!(r.pages[1].panels, auto, "other pages stay automatic");
        let (w, h) = image::image_dimensions(panel_path(&ch, 0, 0)).unwrap();
        assert_eq!((w, h), (100, 180));
        assert!(panel_path(&ch, 0, 1).is_file() && !panel_path(&ch, 0, 2).is_file(), "the old third crop goes");
        assert_eq!(load_manual(&ch).boxes(0, "001.png").map(|b| b.len()), Some(2));

        // Cropping again keeps the reader's own panels from the first time.
        set_manual_page(&ch, 0, vec![bx(0.0, 0.0, 1.0, 1.0)]).unwrap();
        assert_eq!(load_read(&ch).unwrap().auto_panels, vec![(0, auto.clone())]);

        assert!(!reset_manual_page(&ch, 0).unwrap());
        let r = load_read(&ch).unwrap();
        assert!(r.manual.is_empty() && r.auto_panels.is_empty());
        assert_eq!(r.pages[0].panels.len(), 3);
        assert_eq!(r.pages[0].panels[0].lines, auto[0].lines);
        assert!(load_manual(&ch).pages.is_empty());
        assert!(panel_path(&ch, 0, 2).is_file());
    }

    /// Answers every page with two panels, top and bottom half.
    struct TwoPanels(std::sync::Mutex<Vec<String>>);

    impl crate::llm::Llm for TwoPanels {
        fn step<'a>(&'a self, req: mv_ai::AiStepRequest, _: mv_ai::CancellationToken) -> futures::future::BoxFuture<'a, Result<String>> {
            self.0.lock().unwrap().push(req.messages.last().map(|m| m.content.clone()).unwrap_or_default());
            Box::pin(async {
                Ok(r#"{"summary":"Kai talks.","scene":{"kind":"dialogue","mood":"calm","intensity":2,"tags":[]},"memory":null,"panels":[
                    {"bbox":[0.0,0.0,1.0,0.5],"description":"top","characters":[{"name":"Kai"}],"lines":[{"speaker":"Kai","kind":"speech","text":"One"}]},
                    {"bbox":[0.0,0.5,1.0,1.0],"description":"bottom","characters":[],"lines":[{"speaker":"Kai","kind":"speech","text":"Two"}]}]}"#
                    .to_string())
            })
        }
    }

    async fn read_it(ch: &ChapterRef, root: &Path) -> (ChapterRead, Vec<String>) {
        let llm = TwoPanels(Default::default());
        let model = ModelChoice { provider: mv_ai::AiProviderId::ClaudeCode, model: "m".into() };
        let brain = std::sync::Mutex::new(crate::Brain::load_in(root.join("brain"), "m", "M"));
        let progress: ProgressFn = std::sync::Arc::new(|_| {});
        let r = crate::read::read_chapter_with(&llm, &model, ch, &brain, &ReadOptions::default(), &progress, &mv_ai::CancellationToken::new()).await.unwrap();
        (r, llm.0.into_inner().unwrap())
    }

    #[tokio::test]
    async fn a_read_keeps_the_hand_cropped_pages_and_redo_from_read_too() {
        let dir = tempfile::tempdir().unwrap();
        let ch = chapter(dir.path(), 2);
        // Page 1 is cropped by hand before anything is read: one tall panel and a small one.
        let boxes = vec![bx(0.0, 0.0, 1.0, 0.8), bx(0.1, 0.8, 0.9, 1.0)];
        set_manual_page(&ch, 0, boxes.clone()).unwrap();
        // The same chapter with no hand crops, for the reading prompts.
        let plain = tempfile::tempdir().unwrap();
        let (_, want) = read_it(&chapter(plain.path(), 2), plain.path()).await;
        for round in 0..2 {
            let (r, prompts) = read_it(&ch, dir.path()).await;
            if round == 0 {
                assert_eq!(prompts, want, "hand crops never change how the pages are read");
            }
            assert_eq!(r.manual, [0]);
            let p0 = &r.pages[0];
            assert_eq!(p0.panels.iter().map(|p| p.bbox.unwrap()).collect::<Vec<_>>(), boxes);
            // Both reader panels lie mostly in the tall box: their text goes there, in order.
            assert_eq!(p0.panels[0].lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
            assert_eq!(r.auto_panels.len(), 1);
            assert_eq!(r.auto_panels[0].1.len(), 2, "the reader's own panels are kept");
            assert_eq!(image::image_dimensions(panel_path(&ch, 0, 0)).unwrap(), (100, 160), "cut exactly to the user's box");
            assert_eq!(image::image_dimensions(panel_path(&ch, 0, 1)).unwrap(), (80, 40));
            assert_eq!(r.pages[1].panels.len(), 2);
            assert!(panel_path(&ch, 1, 0).is_file() && panel_path(&ch, 1, 1).is_file());
            // Redo from Read: the reading and crops go, the hand-cropped panels stay for the next read.
            crate::reset_phase(&ch, crate::Step::Read).unwrap();
            assert!(load_read(&ch).is_none() && !panel_path(&ch, 0, 0).is_file());
            assert_eq!(load_manual(&ch).boxes(0, "001.png").map(<[Bbox]>::to_vec), Some(boxes.clone()));
        }
    }

    #[tokio::test]
    async fn without_hand_crops_a_read_is_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let ch = chapter(dir.path(), 2);
        let (r, prompts) = read_it(&ch, dir.path()).await;
        assert!(prompts.iter().all(|p| !p.contains("PANELS:")), "the reading prompt is unchanged");
        assert!(r.manual.is_empty() && r.auto_panels.is_empty());
        assert!(!chapter_dir(&ch).join(FILE).exists(), "nothing of the cropper is written");
        let json = std::fs::read_to_string(chapter_dir(&ch).join("read.json")).unwrap();
        assert!(!json.contains("\"manual\"") && !json.contains("auto_panels"), "read.json has no new keys");
        // The automatic crops: the reader's boxes with the usual hair of padding.
        assert_eq!(r.pages[0].panels[0].bbox, Some(bx(0.0, 0.0, 1.0, 0.5)));
        assert_eq!(image::image_dimensions(panel_path(&ch, 0, 0)).unwrap(), (100, 101));
    }

    #[test]
    fn snapping_an_old_read_leaves_hand_cropped_pages_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ch = chapter(dir.path(), 1);
        // The page is one flat frame: an automatic box this big would snap to the whole page.
        let mut read = ChapterRead { pages: vec![PageRead { index: 0, panels: vec![panel(Some(bx(0.0, 0.0, 0.9, 0.9)), "x")], ..Default::default() }], ..Default::default() };
        apply(&mut read, 0, &[bx(0.0, 0.0, 0.9, 0.9)]);
        let before = read.pages.clone();
        crate::read::snap_read(&ch, &mut read);
        assert_eq!(read.pages, before);
        assert!(read.panels_snapped);
    }

    #[test]
    fn boxes_belong_to_the_file_they_were_drawn_on() {
        let m = ManualPanels { pages: vec![ManualPage { index: 0, file: "001.png".into(), boxes: vec![bx(0.0, 0.0, 1.0, 1.0)] }] };
        assert!(m.boxes(0, "001.png").is_some());
        assert!(m.boxes(0, "other.png").is_none());
        assert!(m.boxes(1, "001.png").is_none());
    }

    #[test]
    fn a_page_cropped_before_it_is_read_asks_for_a_read_on_reset() {
        let dir = tempfile::tempdir().unwrap();
        let ch = chapter(dir.path(), 1);
        set_manual_page(&ch, 0, vec![bx(0.0, 0.0, 1.0, 1.0)]).unwrap();
        assert!(load_read(&ch).is_none(), "nothing read yet: only the boxes and crops are saved");
        assert!(panel_path(&ch, 0, 0).is_file());
        let mut read = ChapterRead { pages: vec![PageRead { index: 0, ..Default::default() }], ..Default::default() };
        apply(&mut read, 0, &[bx(0.0, 0.0, 1.0, 1.0)]);
        save_read(&ch, &read).unwrap();
        assert!(reset_manual_page(&ch, 0).unwrap());
    }
}
