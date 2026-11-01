//! Hand the finished story to the existing Python audio/video pipeline.
//!
//! * `script_edited.json` - `[{"page": i, "text": "..."}]` for EVERY page 0..N (`run_tts` writes a short
//!   silence for empty pages, so the page numbering and timing stay intact).
//! * `narration.studio.json` - `{"beats": [...]}` read by `pipeline_worker.load_beats`. `sync_map` aligns
//!   beats to audio by `min(audio page) - min(beat page)`, so there is a beat for every page (page 0
//!   included) and numbering is 0-based like the audio files. A page the story leaves out gets one
//!   `"skip": true` beat, which `sync_map` cuts from the video. A passage the user gave a delivery
//!   carries it as the beat's `kind` / `intensity`, which the narration follows. A passage the writer
//!   cued a sound effect on carries `sfx` / `sfx_strength` (1-3), which the renderer places.
//!
//! A passage is shown over its panel's crop. When it names no panel, or that crop is blank (an empty or
//! nearly black / white panel), the page's most telling panel is used instead: one with characters in
//! it, the largest first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use crate::store::*;
use crate::types::*;

/// Brightness spread (0-255) under which a crop counts as blank.
const BLANK_SPREAD: f64 = 16.0;

fn abs(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().trim_start_matches(r"\?\").to_string()
}

/// True when the image is (nearly) one flat tone: an empty, black or white panel.
fn is_blank(path: &Path) -> bool {
    let Ok(img) = image::open(path) else { return false };
    let g = img.thumbnail(64, 64).to_luma8();
    let n = g.pixels().len().max(1) as f64;
    let mean = g.pixels().map(|p| p.0[0] as f64).sum::<f64>() / n;
    let var = g.pixels().map(|p| (p.0[0] as f64 - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() < BLANK_SPREAD
}

/// The crop shown for a passage on `page` that asked for `wanted` (see the module docs).
fn pick_panel(ch: &ChapterRef, read: Option<&ChapterRead>, page: usize, wanted: Option<usize>, blank: &mut HashMap<PathBuf, bool>) -> Option<(usize, PathBuf)> {
    let mut usable = |n: usize| {
        let p = panel_path(ch, page, n);
        let ok = p.is_file() && !*blank.entry(p.clone()).or_insert_with(|| is_blank(&p));
        ok.then_some(p)
    };
    if let Some(p) = wanted.and_then(&mut usable) {
        return Some((wanted.unwrap(), p));
    }
    let panels = &read?.pages.iter().find(|p| p.index == page)?.panels;
    let mut order: Vec<usize> = (0..panels.len()).collect();
    let rank = |n: &usize| (!panels[*n].characters.is_empty(), panels[*n].bbox.map(|b| b.area()).unwrap_or(0.0));
    order.sort_by(|a, b| rank(b).partial_cmp(&rank(a)).unwrap_or(std::cmp::Ordering::Equal));
    order.into_iter().find_map(|n| usable(n).map(|p| (n, p)))
}

pub fn export_for_pipeline(ch: &ChapterRef, story: &Story) -> Result<()> {
    let pages = list_pages(&ch.pages_dir);
    let read = load_read(ch);
    let mut blank = HashMap::new();
    let page_count = pages.len().max(story.segments.iter().map(|s| s.page + 1).max().unwrap_or(0));
    let mut script: Vec<Value> = vec![];
    let mut beats: Vec<Value> = vec![];

    // The in-medias-res opening is voiced first, on the first narrated page, over its own panels.
    let said = |s: &&StorySegment| !s.text.trim().is_empty();
    let opening = story.segments.iter().filter(said).filter(|s| !s.hook).map(|s| s.page).min().or_else(|| story.segments.iter().filter(said).map(|s| s.page).min());

    for page in 0..page_count {
        let page_img = pages.get(page).map(|p| abs(p));
        let hook = story.segments.iter().filter(said).filter(|s| s.hook && Some(page) == opening);
        let segs: Vec<&StorySegment> = hook.chain(story.segments.iter().filter(said).filter(|s| !s.hook && s.page == page)).collect();
        let text = segs.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
        script.push(json!({ "page": page, "text": text }));

        if segs.is_empty() {
            beats.push(json!({ "page": page, "panel": Value::Null, "panel_image": page_img, "text": "", "skip": true }));
            continue;
        }
        for s in segs {
            // A hook line shows its own page's panel while it is voiced on this page.
            let (panel, image) = match pick_panel(ch, read.as_ref(), s.page, s.panel, &mut blank) {
                Some((n, p)) => (json!(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| n.to_string())), Some(abs(&p))),
                None => (Value::Null, pages.get(s.page).map(|p| abs(p))),
            };
            let mut beat = json!({ "page": page, "panel": panel, "panel_image": image, "text": s.text.trim() });
            // The user's direction for this passage (narration_tts.build_plan: a beat's own
            // kind / intensity win over the page's scene).
            if let Some(d) = s.delivery.as_ref().filter(|d| !d.kind.is_empty()) {
                beat["kind"] = json!(d.kind);
                beat["intensity"] = json!(d.intensity.min(10));
            }
            // The writer's sound-effect cue (sync_map carries it; the renderer places it).
            if let Some(x) = s.sfx.as_ref().filter(|x| SFX_NAMES.contains(&x.name.as_str())) {
                beat["sfx"] = json!(x.name);
                beat["sfx_strength"] = json!(x.strength.clamp(1, 3));
            }
            beats.push(beat);
        }
    }

    std::fs::create_dir_all(&ch.output_dir)?;
    write_json_atomic(&ch.output_dir.join("script_edited.json"), &script)?;
    write_json_atomic(&ch.output_dir.join("narration.studio.json"), &json!({ "beats": beats }))?;
    Ok(())
}
