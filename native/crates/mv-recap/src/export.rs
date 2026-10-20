//! Hand the finished story to the existing Python audio/video pipeline.
//!
//! * `script_edited.json` - `[{"page": i, "text": "..."}]` for EVERY page 0..N (`run_tts` writes a short
//!   silence for empty pages, so the page numbering and timing stay intact).
//! * `narration.studio.json` - `{"beats": [...]}` read by `pipeline_worker.load_beats`. `sync_map` aligns
//!   beats to audio by `min(audio page) - min(beat page)`, so there is a beat for every page (page 0
//!   included) and numbering is 0-based like the audio files. A passage the user gave a delivery
//!   carries it as the beat's `kind` / `intensity`, which the narration follows.

use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::store::*;
use crate::types::*;

fn abs(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().trim_start_matches(r"\\?\").to_string()
}

pub fn export_for_pipeline(ch: &ChapterRef, story: &Story) -> Result<()> {
    let pages = list_pages(&ch.pages_dir);
    let page_count = pages.len().max(story.segments.iter().map(|s| s.page + 1).max().unwrap_or(0));
    let mut script: Vec<Value> = vec![];
    let mut beats: Vec<Value> = vec![];

    for page in 0..page_count {
        let page_img = pages.get(page).map(|p| abs(p));
        let segs: Vec<&StorySegment> = story.segments.iter().filter(|s| s.page == page && !s.text.trim().is_empty()).collect();
        let text = segs.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
        script.push(json!({ "page": page, "text": text }));

        if segs.is_empty() {
            beats.push(json!({ "page": page, "panel": Value::Null, "panel_image": page_img, "text": "" }));
            continue;
        }
        for s in segs {
            let crop = s.panel.map(|n| (n, panel_path(ch, page, n))).filter(|(_, p)| p.is_file());
            let (panel, image) = match crop {
                Some((n, p)) => (json!(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| n.to_string())), Some(abs(&p))),
                None => (Value::Null, page_img.clone()),
            };
            let mut beat = json!({ "page": page, "panel": panel, "panel_image": image, "text": s.text.trim() });
            // The user's direction for this passage (narration_tts.build_plan: a beat's own
            // kind / intensity win over the page's scene).
            if let Some(d) = s.delivery.as_ref().filter(|d| !d.kind.is_empty()) {
                beat["kind"] = json!(d.kind);
                beat["intensity"] = json!(d.intensity.min(10));
            }
            beats.push(beat);
        }
    }

    std::fs::create_dir_all(&ch.output_dir)?;
    write_json_atomic(&ch.output_dir.join("script_edited.json"), &script)?;
    write_json_atomic(&ch.output_dir.join("narration.studio.json"), &json!({ "beats": beats }))?;
    Ok(())
}
