//! The feed test: a made thumbnail is never seen alone but in a grid of others at about 320x180,
//! for well under a second. This puts it among the competing recaps the YouTube package found (the
//! same manga's first, then other recaps, then the reference channel's), in a cell that changes
//! each run, and asks a model that can see the picture, told nothing about which one is ours, where
//! it looks first, what it would click and what each thumbnail would need to pull harder. The feed
//! picture is shown to the owner too: their own eye is a judge as well. A model is not a viewer;
//! YouTube Studio's "Test & compare" is the real measure.
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logic::Video;
use super::thumbs::Gallery;

/// The feed: 3 rows of 4 thumbnails at YouTube's size, on its dark page.
pub const COLS: usize = 4;
pub const ROWS: usize = 3;
pub const CELLS: usize = COLS * ROWS;
const W: u32 = 320;
const H: u32 = 180;
const GAP: u32 = 16;
/// Fewer competitors than this and the test says nothing.
const MIN_OTHERS: usize = 6;

/// What one feed test found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedResult {
    pub at: u64,
    /// Where it was put, from 0 (left to right, top to bottom).
    pub cell: usize,
    /// The judge looked at it first, and would click it.
    pub first_look: bool,
    pub clicked: bool,
    /// Its score out of 10, and the best competing thumbnail's.
    pub score: u8,
    pub best_other: u8,
    /// What the judge said would make it pull harder.
    pub tip: String,
}

impl FeedResult {
    /// One line for the page: "Looked at first and clicked · 9/10 (best rival 8) · ...".
    pub fn summary(&self) -> String {
        let seen = match (self.first_look, self.clicked) {
            (true, true) => "Looked at first and clicked".to_string(),
            (false, true) => "Clicked (another one caught the eye first)".to_string(),
            (true, false) => "Looked at first, but another one got the click".to_string(),
            (false, false) => "Not looked at first, not clicked".to_string(),
        };
        format!("{seen} · {}/10 against {}/10 for the best competitor · it sat in row {}, place {}", self.score, self.best_other, self.cell / COLS + 1, self.cell % COLS + 1)
    }
}

/// Where a thumbnail's feed picture with it at `cell` is kept (a new run puts it in another cell,
/// so the page never shows a stale copy from the image cache).
pub fn image(manga_id: &str, thumb_id: &str, cell: usize) -> PathBuf {
    Gallery::dir(manga_id).join("feed").join(format!("{thumb_id}-{cell}.jpg"))
}

/// The competing thumbnails' video ids, at most the eleven the feed holds besides ours: the same
/// manga's recaps first, then the other recaps found, then the reference channel's uploads.
pub fn competitor_ids(comparable: &[Video], others: &[Video], reference: &[Video]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for v in comparable.iter().chain(others).chain(reference) {
        if !v.id.is_empty() && !out.contains(&v.id) {
            out.push(v.id.clone());
        }
        if out.len() == CELLS - 1 {
            break;
        }
    }
    out
}

/// The cell ours goes in this time: not the one it had last time, so a good place does not flatter it.
pub fn next_cell(last: Option<usize>, seed: u64) -> usize {
    let c = (seed % CELLS as u64) as usize;
    if Some(c) == last {
        (c + 5) % CELLS
    } else {
        c
    }
}

/// Builds the feed picture at `out`: `ours` at `cell`, the competitors (downloaded once and kept
/// beside the feeds) in the other cells.
pub async fn build(manga_id: &str, ours: &Path, ids: &[String], cell: usize, out: &Path) -> Result<()> {
    let cache = Gallery::dir(manga_id).join("feed").join("cache");
    std::fs::create_dir_all(&cache)?;
    let http = super::magnific::client()?;
    let mut files = vec![];
    for id in ids {
        let f = cache.join(format!("{id}.jpg"));
        if !f.is_file() {
            let Ok(r) = http.get(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")).send().await else { continue };
            if !r.status().is_success() {
                continue;
            }
            let Ok(bytes) = r.bytes().await else { continue };
            let Ok(img) = image::load_from_memory(&bytes) else { continue };
            if img.width() < 200 || img.to_rgb8().save_with_format(&f, image::ImageFormat::Jpeg).is_err() {
                continue;
            }
        }
        files.push(f);
    }
    if files.len() < MIN_OTHERS {
        bail!("Only {} competing thumbnails could be found; open the YouTube package for this manga so it finds the other recaps, then try again.", files.len());
    }
    let (ours, out) = (ours.to_path_buf(), out.to_path_buf());
    tokio::task::spawn_blocking(move || compose(&ours, &files, cell, &out)).await?
}

/// The feed picture: a dark page, `ours` at `cell`, `others` in the remaining cells in order.
pub fn compose(ours: &Path, others: &[PathBuf], cell: usize, out: &Path) -> Result<()> {
    let (cw, ch) = (COLS as u32 * (W + GAP) + GAP, ROWS as u32 * (H + GAP) + GAP);
    let mut canvas = image::RgbImage::from_pixel(cw, ch, image::Rgb([15, 15, 15]));
    let mut rest = others.iter();
    for i in 0..CELLS {
        let file = if i == cell { Some(ours) } else { rest.next().map(PathBuf::as_path) };
        let Some(img) = file.and_then(|f| image::open(f).ok()) else { continue };
        let tile = img.resize_to_fill(W, H, image::imageops::FilterType::Triangle).to_rgb8();
        let (x, y) = (GAP + (i % COLS) as u32 * (W + GAP), GAP + (i / COLS) as u32 * (H + GAP));
        image::imageops::overlay(&mut canvas, &tile, x as i64, y as i64);
    }
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    canvas.save_with_format(out, image::ImageFormat::Jpeg)?;
    Ok(())
}

/// The question for the judge, who sees only the feed picture.
pub fn judge_prompt() -> (String, String) {
    let system = format!(
        "You are an ordinary viewer scrolling YouTube, a fan of manga and manhwa recap videos. The picture shows a feed of {CELLS} video thumbnails in {ROWS} rows of {COLS}, numbered 1 to {CELLS} from left to right and top to bottom (1 is top-left, {CELLS} is bottom-right). Look the way a real viewer does, a glance at each, and answer honestly: none of them is yours.\n\
Reply with JSON only: {{\"first_look\": n, \"click\": n, \"scores\": [{CELLS} whole numbers from 1 to 10, in cell order: how strongly each pulls you to click], \"tips\": [{CELLS} short sentences in cell order: what would make that thumbnail pull harder]}}"
    );
    (system, "Which thumbnail do you look at first, which would you click, and how does each one score?".to_string())
}

/// The judge's answer about the feed in which ours sat at `cell`.
pub fn parse_judgement(raw: &str, cell: usize, at: u64) -> Option<FeedResult> {
    let v = mv_recap::extract_json(raw)?;
    let n = |k: &str| v.get(k).and_then(Value::as_u64).map(|x| x as usize);
    let scores: Vec<u8> = v.get("scores")?.as_array()?.iter().map(|s| s.as_f64().unwrap_or(0.).round().clamp(0., 10.) as u8).collect();
    if scores.len() != CELLS || cell >= CELLS {
        return None;
    }
    let tips: Vec<String> = v.get("tips").and_then(Value::as_array).map(|a| a.iter().map(|t| t.as_str().unwrap_or("").trim().to_string()).collect()).unwrap_or_default();
    let best_other = scores.iter().enumerate().filter(|(i, _)| *i != cell).map(|(_, s)| *s).max().unwrap_or(0);
    Some(FeedResult { at, cell, first_look: n("first_look") == Some(cell + 1), clicked: n("click") == Some(cell + 1), score: scores[cell], best_other, tip: tips.get(cell).cloned().unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid(id: &str) -> Video {
        Video { id: id.into(), ..Default::default() }
    }

    #[test]
    fn the_feed_holds_the_competitors_and_ours() {
        // Same manga first, no repeats, at most eleven.
        let ids = competitor_ids(&[vid("a"), vid("b")], &[vid("b"), vid(""), vid("c")], &(0..20).map(|k| vid(&format!("r{k}"))).collect::<Vec<_>>());
        assert_eq!(ids.len(), CELLS - 1);
        assert_eq!(&ids[..4], &["a", "b", "c", "r0"]);
        // Ours never sits in the same cell twice running.
        assert_eq!(next_cell(None, 27), 3);
        assert_eq!(next_cell(Some(3), 27), 8);

        let d = tempfile::tempdir().unwrap();
        let ours = d.path().join("ours.png");
        image::RgbImage::from_pixel(1280, 720, image::Rgb([250, 200, 0])).save(&ours).unwrap();
        let others: Vec<PathBuf> = (0..11)
            .map(|k| {
                let f = d.path().join(format!("{k}.png"));
                image::RgbImage::from_pixel(320, 180, image::Rgb([30, 60, 200])).save(&f).unwrap();
                f
            })
            .collect();
        let out = d.path().join("feed.jpg");
        compose(&ours, &others, 5, &out).unwrap();
        let f = image::open(&out).unwrap().to_rgb8();
        assert_eq!(f.dimensions(), (4 * 336 + 16, 3 * 196 + 16));
        // Cell 5 is row 2, place 2: ours; cell 0 a competitor; the gaps the dark page.
        let px = |c: usize| *f.get_pixel(16 + (c % COLS) as u32 * 336 + 160, 16 + (c / COLS) as u32 * 196 + 90);
        assert!(px(5).0[0] > 200 && px(5).0[2] < 80);
        assert!(px(0).0[2] > 150 && px(11).0[2] > 150);
        assert!(f.get_pixel(8, 8).0.iter().all(|c| *c < 30));
    }

    #[test]
    fn the_judge_answer_says_how_ours_did() {
        assert!(judge_prompt().0.contains("none of them is yours"));
        let scores = "[6,7,8,5,6,9,7,6,5,8,6,7]";
        let tips = (1..=12).map(|k| format!("\"tip {k}\"")).collect::<Vec<_>>().join(",");
        let r = parse_judgement(&format!(r#"Sure: {{"first_look": 6, "click": 6, "scores": {scores}, "tips": [{tips}]}}"#), 5, 99).unwrap();
        assert_eq!(r, FeedResult { at: 99, cell: 5, first_look: true, clicked: true, score: 9, best_other: 8, tip: "tip 6".into() });
        assert!(r.summary().starts_with("Looked at first and clicked · 9/10 against 8/10") && r.summary().ends_with("row 2, place 2"));
        let lost = parse_judgement(&format!(r#"{{"first_look": 6, "click": 3, "scores": {scores}}}"#), 1, 0).unwrap();
        assert_eq!((lost.first_look, lost.clicked, lost.score, lost.best_other, lost.tip.as_str()), (false, false, 7, 9, ""));
        // Not twelve scores, or no JSON: no result.
        assert!(parse_judgement(r#"{"scores":[1,2,3]}"#, 0, 0).is_none());
        assert!(parse_judgement("no", 0, 0).is_none());
    }
}
