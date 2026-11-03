//! The text AI behind Creative: the model picked in the assistant's model picker answers in JSON
//! (one retry with a nudge when it does not).
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use mv_ai::{AiMessage, AiProviderId, Registry};
use mv_create::prompts::{self, ScriptAnswer};
use mv_create::{Page, Project};
use serde_json::Value;

/// Who answers: the shared registry and the picked provider and model.
#[derive(Clone)]
pub(crate) struct Brainbox {
    pub reg: Arc<Registry>,
    pub provider: AiProviderId,
    pub model: String,
    /// The page's Stop for the AI.
    pub cancel: mv_ai::CancellationToken,
}

impl Brainbox {
    /// The assistant's current model, or why there is none.
    pub(crate) fn current(cancel: mv_ai::CancellationToken, cx: &mut gpui_kit::App) -> std::result::Result<Brainbox, String> {
        let (provider, model) = ui_assistant::assistant_model(cx);
        let provider = serde_json::from_value::<AiProviderId>(Value::String(provider)).map_err(|_| "Pick an AI model first (the model button under the chat box).".to_string())?;
        Ok(Brainbox { reg: ui_assistant::registry(cx), provider, model, cancel })
    }

    pub(crate) async fn json(&self, mut messages: Vec<AiMessage>) -> Result<Value> {
        static N: AtomicU64 = AtomicU64::new(0);
        for attempt in 0..2 {
            let req = mv_ai::AiStepRequest {
                request_id: format!("creative-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
                provider_id: self.provider,
                model: self.model.clone(),
                messages: messages.clone(),
                tools: None,
                effort: None,
                access: None,
                images: Vec::new(),
                workspace: None,
                lean: true,
                mcp: None,
            };
            let r = self.reg.run_step_cancellable(req, mv_ai::emit_noop(), self.cancel.clone()).await;
            if r.stopped || self.cancel.is_cancelled() {
                bail!("stopped");
            }
            if let Some(e) = r.error {
                bail!(e);
            }
            if let Some(v) = mv_recap::extract_json(&r.text) {
                return Ok(v);
            }
            if attempt == 0 {
                messages.push(AiMessage::assistant(r.text));
                messages.push(AiMessage::user("Your previous answer was not valid JSON. Reply with the JSON object only."));
            }
        }
        Err(anyhow!("the AI's answer was not valid JSON; try again or pick another model"))
    }

    pub(crate) async fn ask(&self, (system, user): (String, String)) -> Result<Value> {
        self.json(vec![AiMessage::system(system), AiMessage::user(user)]).await
    }
}

/// Pages per script call: small enough that no answer gets cut off.
const BATCH: u32 = 6;

/// Writes chapter `n`'s whole script a few pages at a time. Returns its title, summary and pages.
pub(crate) async fn write_chapter(ai: Brainbox, mut p: Project, n: u32, note: String, status: Arc<Mutex<String>>) -> Result<(String, String, Vec<Page>)> {
    let total = p.settings.pages_per_chapter.clamp(4, 60);
    let mut pages: Vec<Page> = vec![];
    let (mut title, mut summary) = (String::new(), String::new());
    let mut from = 1;
    while from <= total {
        let to = (from + BATCH - 1).min(total);
        *status.lock().unwrap_or_else(|e| e.into_inner()) = format!("Writing chapter {n}, pages {from}–{to} of {total}");
        let v = ai.ask(prompts::chapter_script(&p, n, &note, from, to, &prompts::page_digest(&pages))).await?;
        let a: ScriptAnswer = serde_json::from_value(v)?;
        if title.is_empty() {
            title = a.title;
            summary = a.summary;
        }
        let got: Vec<Page> = a.pages.into_iter().filter(|pg| !pg.panels.is_empty()).collect();
        if got.is_empty() {
            bail!("the AI wrote no panels for pages {from}–{to}");
        }
        for mut pg in got.into_iter().take((to - from + 1) as usize) {
            pg.number = pages.len() as u32 + 1;
            tidy(&p, &mut pg);
            pages.push(pg);
        }
        from = pages.len() as u32 + 1;
        // Later batches see the title the first one chose.
        let ch = p.chapter_mut(n);
        if ch.title.is_empty() {
            ch.title = title.clone();
        }
    }
    Ok((title, summary, pages))
}

/// One page rewritten.
pub(crate) async fn edit_page(ai: Brainbox, p: Project, n: u32, page: Page, instruction: String) -> Result<Page> {
    let v = ai.ask(prompts::page_edit(&p, n, &page, &instruction)).await?;
    let a: ScriptAnswer = serde_json::from_value(v)?;
    let mut pg = a.pages.into_iter().find(|x| !x.panels.is_empty()).ok_or_else(|| anyhow!("the AI wrote no panels"))?;
    pg.number = page.number;
    tidy(&p, &mut pg);
    Ok(pg)
}

/// Cast names spelled as in the cast list, at most six panels, and none of the file fields: a
/// rewritten page is painted and animated afresh, and the AI never chooses which files we read.
fn tidy(p: &Project, pg: &mut Page) {
    pg.panels.truncate(6);
    pg.image = None;
    pg.model.clear();
    for panel in &mut pg.panels {
        panel.image = None;
        panel.clip = None;
        for c in &mut panel.characters {
            if let Some(m) = p.character(&c.name) {
                c.name = m.name.clone();
            }
        }
        for l in &mut panel.lines {
            if let Some(m) = p.character(&l.speaker) {
                l.speaker = m.name.clone();
            }
        }
    }
}
