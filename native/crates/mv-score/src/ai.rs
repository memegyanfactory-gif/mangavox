//! Optional AI spotting: the user's chosen model reads the page-by-page scene notes and marks the
//! musical arc (where a fight starts, which page is the reveal, where to hold a breath). The answer is
//! saved in `music.json` and reused until the user asks again.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use mv_ai::{emit_noop, AiMessage, AiStepRequest, CancellationToken, Registry};
use mv_recap::{extract_json, load_read, ChapterRef, ModelChoice};
use serde_json::Value;

use crate::model::{Direction, Style};

const SYSTEM: &str = "You are a film composer spotting a manga recap video for its music score. \
For each page you get its scene kind, mood, intensity (0-10) and a summary. Decide the musical arc across the whole chapter: \
build-ups before big moments, a held breath (drop) right before a reveal, a hit where a fight or shock lands, \
quiet beds for dialogue. Styles (the genres recap channels use): phonk (fights, cocky or badass moments), \
epic (orchestral + choir: power-ups, climaxes, big reveals), tension (dark cinematic bed: danger, schemes), horror (drones: monsters, dread), \
sad (piano + strings: loss, romance, flashbacks), lofi (comedy, daily life), mystery (secrets, investigation, neutral narration). \
Reply with JSON only: {\"pages\":[{\"page\":0,\"style\":\"mystery\",\"energy\":3,\"hit\":false,\"drop\":false}, ...]} with one entry per page given. \
Use \"hit\" sparingly (a few per chapter) and \"drop\" only on the page of a reveal.";

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Parses the model's answer; unknown styles and out-of-range values are dropped or clamped.
pub fn parse_directions(v: &Value) -> Vec<Direction> {
    v.get("pages")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some(Direction {
                        page: p.get("page")?.as_u64()? as usize,
                        style: p.get("style").and_then(Value::as_str).and_then(Style::parse),
                        energy: p.get("energy").and_then(Value::as_f64).map(|e| e.clamp(0.0, 10.0) as f32),
                        hit: p.get("hit").and_then(Value::as_bool).unwrap_or(false),
                        drop: p.get("drop").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn spot(reg: Arc<Registry>, model: ModelChoice, ch: ChapterRef, cancel: CancellationToken) -> Result<Vec<Direction>> {
    let read = load_read(&ch).filter(|r| !r.pages.is_empty()).ok_or_else(|| anyhow!("Read the chapter first."))?;
    let pages: Vec<String> = read.pages.iter().map(|p| format!("page {}: [{}; {}; intensity {}] {}", p.index, p.scene.kind, p.scene.mood, p.scene.intensity, truncate(&p.summary, 180))).collect();
    static N: AtomicU64 = AtomicU64::new(0);
    let req = AiStepRequest {
        request_id: format!("score-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: model.provider,
        model: model.model.clone(),
        messages: vec![AiMessage::system(SYSTEM), AiMessage::user(format!("{} · {}\n\n{}", ch.manga_title, ch.label, pages.join("\n")))],
        tools: None,
        effort: None,
        access: None,
        images: vec![],
        workspace: None,
        lean: true,
        mcp: None,
    };
    let r = reg.run_step_cancellable(req, emit_noop(), cancel).await;
    if r.stopped {
        bail!("cancelled");
    }
    if let Some(e) = r.error {
        bail!(e);
    }
    let v = extract_json(&r.text).ok_or_else(|| anyhow!("The model's cue sheet was not JSON."))?;
    let d = parse_directions(&v);
    if d.is_empty() {
        bail!("The model returned an empty cue sheet.");
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_cleans_the_cue_sheet() {
        let v: Value = serde_json::from_str(r#"{"pages":[{"page":0,"style":"Calm","energy":2},{"page":1,"style":"weird","energy":14,"hit":true},{"style":"epic"}]}"#).unwrap();
        let d = parse_directions(&v);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].style, Some(Style::Lofi), "old style names map onto the genres");
        assert_eq!(d[1].style, None);
        assert_eq!(d[1].energy, Some(10.0));
        assert!(d[1].hit);
    }
}
