//! The two Studio switches around the AI steps, both off by default (kept in [`Prefs`]):
//!
//! * **Light read**: pages are read by a cheaper model that can see pictures (a free OpenCode
//!   vision model first), asked only for the text and the basics (see `LIGHT_READ_SYSTEM`). The
//!   story is still written by the picked model.
//! * **Auto switch**: when Read or Story fails (an error, a usage limit, a model that cannot see
//!   pictures), other models are tested on one page of the chapter: the same CLI's other models
//!   first, then the other signed-in CLIs. Read only tries models that can see pictures. The first
//!   that passes the test carries on (a read keeps every page already read) and is remembered for
//!   the rest of the session, so the next chapter starts with it.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use mv_ai::{AiEffort, AiMessage, AiProviderId, AiProviderState, CancellationToken, Registry};

use crate::brain::Brain;
use crate::llm::{ask_story_json, is_cancelled, request};
use crate::read::read_chapter_with;
use crate::store::{chapter_dir, list_pages, load_read};
use crate::types::*;

/// Light read is on (the Studio switch).
pub fn light_on() -> bool {
    Prefs::load().light_read
}

pub fn set_light(on: bool) {
    Prefs::update(|p| p.light_read = on);
}

/// The Auto switch is on.
pub fn auto_on() -> bool {
    Prefs::load().auto_switch
}

pub fn set_auto(on: bool) {
    Prefs::update(|p| p.auto_switch = on);
}

/// What a model is needed for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Role {
    /// Reading pages (needs a model that can see pictures).
    Read,
    /// Reading pages in Light read.
    Light,
    /// Writing the story (text only).
    Story,
}

/// Most models tested after a failure before giving up.
const MAX_TESTS: usize = 6;
/// Most times one step switches model.
const MAX_SWITCHES: usize = 3;
/// How long a test may take (a CLI agent starts slowly; a free model can be busy).
const TEST_TIMEOUT: Duration = Duration::from_secs(240);

pub fn label(m: &ModelChoice) -> String {
    if m.model.is_empty() || m.model == "default" {
        m.provider.as_str().to_string()
    } else {
        format!("{} · {}", m.provider.as_str(), m.model)
    }
}

/// Models that took over this session: (role, the model picked) to the model that works.
fn taken_over() -> &'static Mutex<HashMap<(Role, ModelChoice), ModelChoice>> {
    static M: OnceLock<Mutex<HashMap<(Role, ModelChoice), ModelChoice>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn remembered(role: Role, picked: &ModelChoice) -> Option<ModelChoice> {
    taken_over().lock().unwrap().get(&(role, picked.clone())).cloned()
}

fn remember(role: Role, picked: &ModelChoice, works: &ModelChoice) {
    taken_over().lock().unwrap().insert((role, picked.clone()), works.clone());
}

fn forget(role: Role, picked: &ModelChoice) {
    taken_over().lock().unwrap().remove(&(role, picked.clone()));
}

/// A failure no other model fixes: nothing to read, nothing read yet, the same chapter twice.
fn model_cannot_fix(e: &anyhow::Error) -> bool {
    let m = format!("{e:#}");
    is_cancelled(e) || m.contains("No page images found") || m.contains("Read the chapters first") || m.contains(crate::story::SAME_CHAPTER_NOTE)
}

// ── which models to try ──────────────────────────────────────────────────────────────────────────

/// Models of one CLI worth trying, best first. `cheap`: the light ones first (Light read).
fn models_of(id: AiProviderId, vision: bool, cheap: bool, opencode_vision: &[String], opencode_all: &[String], agy: &[String]) -> Vec<String> {
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match id {
        AiProviderId::ClaudeCode if cheap => v(&["claude-haiku-5-5", "claude-sonnet-5-5"]),
        AiProviderId::ClaudeCode => v(&["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5"]),
        AiProviderId::Antigravity => {
            let order: &[&str] = if cheap {
                &["gemini-3.8-flash-low", "gemini-3.8-flash-medium", "gemini-3.8-flash-high"]
            } else {
                &["gemini-3.8-flash-high", "claude-sonnet-4-6", "claude-opus-4-6-thinking", "gemini-3.8-flash-medium"]
            };
            // Its live list, when there is one, says what this account has; GPT-OSS cannot see.
            let mut out: Vec<String> = order.iter().filter(|m| agy.is_empty() || agy.iter().any(|a| a == *m)).map(|s| s.to_string()).collect();
            if !vision {
                let more: Vec<String> = agy.iter().filter(|a| a.starts_with("gpt-oss") && !out.contains(a)).cloned().collect();
                out.extend(more);
            }
            out
        }
        AiProviderId::Opencode if vision => opencode_vision.iter().take(4).cloned().collect(),
        AiProviderId::Opencode => {
            // Text: its own free models, the ones that can also see first (they are the stronger ones).
            let mut out: Vec<String> = opencode_vision.iter().take(2).cloned().collect();
            let more: Vec<String> = opencode_all.iter().filter(|m| m.starts_with("opencode/") && !out.contains(m)).take(2).cloned().collect();
            out.extend(more);
            out
        }
        AiProviderId::Codex | AiProviderId::GeminiCli => v(&["default"]),
        AiProviderId::GrokCli if !vision => v(&["default"]),
        _ => vec![],
    }
}

/// Every model to test for `role`, in order: the failed model's own CLI first, then the others.
/// Only CLIs that are installed and signed in (ready) count. Light read starts with the free ones.
async fn candidates(reg: &Registry, role: Role, from: &ModelChoice, tried: &[ModelChoice], ws: &std::path::Path) -> Vec<ModelChoice> {
    let ready: Vec<AiProviderId> = reg.list_providers(false).await.into_iter().filter(|p| p.enabled && p.state == AiProviderState::Ready).map(|p| p.id).collect();
    let vision = role != Role::Story;
    let cheap = role == Role::Light;
    let order: &[AiProviderId] = if cheap {
        &[AiProviderId::Opencode, AiProviderId::Antigravity, AiProviderId::ClaudeCode, AiProviderId::GeminiCli, AiProviderId::Codex, AiProviderId::GrokCli]
    } else {
        &[AiProviderId::ClaudeCode, AiProviderId::Antigravity, AiProviderId::Opencode, AiProviderId::Codex, AiProviderId::GeminiCli, AiProviderId::GrokCli]
    };
    let mut providers: Vec<AiProviderId> = vec![];
    if !cheap {
        providers.push(from.provider);
    }
    let rest: Vec<AiProviderId> = order.iter().copied().filter(|p| !providers.contains(p)).collect();
    providers.extend(rest);
    providers.retain(|p| ready.contains(p));
    let opencode_vision = if providers.contains(&AiProviderId::Opencode) { mv_ai::providers::cli::opencode_vision_models(ws).await } else { vec![] };
    let opencode_all: Vec<String> = if providers.contains(&AiProviderId::Opencode) && !vision {
        reg.list_models(AiProviderId::Opencode, false).await.into_iter().map(|m| m.id).collect()
    } else {
        vec![]
    };
    let agy: Vec<String> = if providers.contains(&AiProviderId::Antigravity) {
        reg.list_models(AiProviderId::Antigravity, false).await.into_iter().map(|m| m.id).filter(|m| m != "default").collect()
    } else {
        vec![]
    };
    providers
        .into_iter()
        .flat_map(|p| models_of(p, vision, cheap, &opencode_vision, &opencode_all, &agy).into_iter().map(move |model| ModelChoice { provider: p, model }))
        .filter(|m| m != from && !tried.contains(m))
        .collect()
}

// ── the test ─────────────────────────────────────────────────────────────────────────────────────

/// The page a test reads: the chapter's page with the most lines in its reading so far (a page
/// with dialogue proves more than a title page), else the third page. And how many lines it has.
fn test_page(ch: &ChapterRef) -> Option<(PathBuf, usize)> {
    let pages = list_pages(&ch.pages_dir);
    let lines_on = |p: &PageRead| p.panels.iter().map(|pn| pn.lines.len()).sum::<usize>();
    if let Some(best) = load_read(ch).and_then(|r| r.pages.iter().filter(|p| p.index < pages.len()).max_by_key(|p| lines_on(p)).filter(|p| lines_on(p) > 0).map(|p| (p.index, lines_on(p)))) {
        return Some((pages[best.0].clone(), best.1));
    }
    pages.get(2.min(pages.len().saturating_sub(1))).map(|p| (p.clone(), 0))
}

/// Read one page of the chapter with `m` in a folder of its own. `Ok` when a real reading came back
/// (a scene, a summary, and lines where the page has them).
async fn test_read(reg: &Arc<Registry>, m: &ModelChoice, ch: &ChapterRef, light: bool, n: usize, cancel: &CancellationToken) -> Result<String, String> {
    let (page, had_lines) = test_page(ch).ok_or("the chapter has no pages")?;
    let root = chapter_dir(ch).join("work").join(format!("auto_test_{n}"));
    let _ = std::fs::remove_dir_all(&root);
    let pages_dir = root.join("pages");
    std::fs::create_dir_all(&pages_dir).map_err(|e| e.to_string())?;
    let ext = page.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "jpg".into());
    std::fs::copy(&page, pages_dir.join(format!("001.{ext}"))).map_err(|e| e.to_string())?;
    let test = ChapterRef { chapter_id: format!("{}-test", ch.chapter_id), output_dir: root.join("out"), pages_dir, ..ch.clone() };
    let brain = Mutex::new(Brain::load_in(root.join("brain"), &ch.manga_id, &ch.manga_title));
    let silent: ProgressFn = Arc::new(|_| {});
    let opts = ReadOptions { light, ..Default::default() };
    // No waiting out a usage limit here: a model that is limited fails the test.
    let llm: Arc<Registry> = reg.clone();
    let run = read_chapter_with(&llm, m, &test, &brain, &opts, &silent, cancel);
    let out = match tokio::time::timeout(TEST_TIMEOUT, run).await {
        Err(_) => Err("it took too long".to_string()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Ok(Ok(read)) => match read.pages.first() {
            None => Err("no page came back".into()),
            Some(p) if p.scene.kind.is_empty() => Err("its answer was not a page reading".into()),
            Some(p) if p.summary.trim().is_empty() => Err("its reading has no summary".into()),
            Some(p) => {
                let lines: usize = p.panels.iter().map(|pn| pn.lines.len()).sum();
                if had_lines > 0 && lines == 0 {
                    Err(format!("it found no lines on a page with {had_lines}"))
                } else {
                    Ok(format!("{lines} lines"))
                }
            }
        },
    };
    let _ = std::fs::remove_dir_all(&root);
    out
}

/// A small story call (built like the Story step's): `Ok` when the JSON has segments.
async fn test_story(reg: &Arc<Registry>, m: &ModelChoice, ch: &ChapterRef, cancel: &CancellationToken) -> Result<String, String> {
    let work = chapter_dir(ch).join("work").join("auto_test_story");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let system = "You are a narrator. Reply with STRICT JSON only: one object, no markdown fences, no commentary. Do not run commands or open files.";
    let prompt = r#"Page 1: a boy finds a glowing sword in a cave. Page 2: he lifts it and the cave shakes.
Retell this in one short passage per page. Reply with exactly: {"title": "...", "segments": [{"page": 0, "text": "..."}, {"page": 1, "text": "..."}]}"#;
    let mut req = request(m, vec![AiMessage::system(system), AiMessage::user(prompt)], vec![], Some(work.clone()));
    req.effort = Some(AiEffort::Medium);
    let llm: Arc<Registry> = reg.clone();
    let out = match tokio::time::timeout(TEST_TIMEOUT, ask_story_json(&llm, req, cancel)).await {
        Err(_) => Err("it took too long".to_string()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Ok(Ok(Err(_))) => Err("its answer was not JSON".into()),
        Ok(Ok(Ok(v))) => match v.get("segments").and_then(|s| s.as_array()).map(|s| s.len()).unwrap_or(0) {
            0 => Err("its answer has no story".into()),
            n => Ok(format!("{n} passages")),
        },
    };
    let _ = std::fs::remove_dir_all(&work);
    out
}

/// Tests the models for `role` one by one; the first that passes.
async fn find_working(reg: &Arc<Registry>, role: Role, from: &ModelChoice, tried: &[ModelChoice], ch: &ChapterRef, progress: &ProgressFn, cancel: &CancellationToken) -> Option<ModelChoice> {
    let ws = chapter_dir(ch).join("work");
    let _ = std::fs::create_dir_all(&ws);
    let list = candidates(reg, role, from, tried, &ws).await;
    if list.is_empty() {
        progress(Progress::Log("Auto: no other signed-in AI to try.".into()));
        return None;
    }
    for (n, m) in list.into_iter().take(MAX_TESTS).enumerate() {
        if cancel.is_cancelled() {
            return None;
        }
        progress(Progress::Log(format!("Auto: testing {} on one page…", label(&m))));
        let result = match role {
            Role::Story => test_story(reg, &m, ch, cancel).await,
            Role::Read => test_read(reg, &m, ch, false, n, cancel).await,
            Role::Light => test_read(reg, &m, ch, true, n, cancel).await,
        };
        match result {
            Ok(how) => {
                progress(Progress::Log(format!("Auto: {} passed ({how}).", label(&m))));
                return Some(m);
            }
            Err(why) => progress(Progress::Log(format!("Auto: {} failed the test: {}", label(&m), crate::llm::truncate(&why, 160)))),
        }
    }
    None
}

// ── running a step ───────────────────────────────────────────────────────────────────────────────

/// Runs a step with `picked`; with Auto on, a failure makes another model that passes the test
/// carry on (up to [`MAX_SWITCHES`] times), and a model that took over earlier is used at once.
#[allow(clippy::too_many_arguments)]
async fn with_auto<T, F, Fut>(reg: &Arc<Registry>, auto: bool, role: Role, picked: ModelChoice, ch: &ChapterRef, progress: &ProgressFn, cancel: &CancellationToken, mut run: F) -> Result<T>
where
    F: FnMut(ModelChoice) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut current = picked.clone();
    if auto {
        if let Some(m) = remembered(role, &picked) {
            progress(Progress::Log(format!("Auto: using {}, which took over from {} earlier.", label(&m), label(&picked))));
            current = m;
        }
    }
    let mut tried: Vec<ModelChoice> = vec![];
    loop {
        let err = match run(current.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        if !auto || cancel.is_cancelled() || model_cannot_fix(&err) || tried.len() >= MAX_SWITCHES {
            return Err(err);
        }
        tried.push(current.clone());
        progress(Progress::Log(format!("Auto: {} failed ({}). Looking for another model…", label(&current), crate::llm::truncate(&format!("{err:#}"), 200))));
        match find_working(reg, role, &current, &tried, ch, progress, cancel).await {
            Some(m) => {
                progress(Progress::Log(format!("Auto: switching to {} and carrying on.", label(&m))));
                remember(role, &picked, &m);
                current = m;
            }
            None => {
                forget(role, &picked);
                return Err(anyhow!("{err:#} (Auto tested the other signed-in models and none passed)"));
            }
        }
    }
}

/// The model Light read uses: one that took over before, else the first cheaper model that
/// passes the test (a free OpenCode vision model first). `None`: none passed.
async fn light_reader(reg: &Arc<Registry>, picked: &ModelChoice, ch: &ChapterRef, progress: &ProgressFn, cancel: &CancellationToken) -> Option<ModelChoice> {
    if let Some(m) = remembered(Role::Light, picked) {
        return Some(m);
    }
    progress(Progress::Log("Light read: finding a cheaper model that can see pictures…".into()));
    let m = find_working(reg, Role::Light, picked, &[], ch, progress, cancel).await?;
    remember(Role::Light, picked, &m);
    Some(m)
}

/// Whether `m` can see pictures, as far as the CLI says: OpenCode lists which models take images
/// (unknown when it lists nothing), Antigravity's GPT-OSS is text only; the others all can.
async fn can_see(m: &ModelChoice, ch: &ChapterRef) -> bool {
    match m.provider {
        AiProviderId::Opencode => {
            let ws = chapter_dir(ch).join("work");
            let _ = std::fs::create_dir_all(&ws);
            let seeing = mv_ai::providers::cli::opencode_vision_models(&ws).await;
            seeing.is_empty() || seeing.contains(&m.model) || !m.model.starts_with("opencode/")
        }
        AiProviderId::Antigravity => !m.model.starts_with("gpt-oss"),
        _ => true,
    }
}

/// [`crate::read_chapter`] with the Studio switches: Light read reads with a cheaper vision model
/// and the light prompt; Auto replaces a model that fails.
pub async fn read_chapter_smart(reg: Arc<Registry>, model: ModelChoice, ch: ChapterRef, brain: Arc<Mutex<Brain>>, opts: ReadOptions, progress: ProgressFn, cancel: CancellationToken) -> Result<ChapterRead> {
    read_smart(reg, model, ch, brain, opts, progress, cancel, light_on(), auto_on()).await
}

/// [`read_chapter_smart`] with the switches given.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn read_smart(reg: Arc<Registry>, model: ModelChoice, ch: ChapterRef, brain: Arc<Mutex<Brain>>, opts: ReadOptions, progress: ProgressFn, cancel: CancellationToken, light: bool, auto: bool) -> Result<ChapterRead> {
    let mut opts = opts;
    opts.light = light;
    let (role, start) = if light {
        match light_reader(&reg, &model, &ch, &progress, &cancel).await {
            Some(m) => {
                progress(Progress::Log(format!("Light read: reading with {}; the story stays on {}.", label(&m), label(&model))));
                (Role::Light, m)
            }
            None => {
                progress(Progress::Log(format!("Light read: no cheaper model passed the test, so {} reads (light).", label(&model))));
                (Role::Read, model)
            }
        }
    } else {
        (Role::Read, model)
    };
    // A model that cannot see pictures never gets the pages: Auto picks one that can at once,
    // otherwise the step stops with what to do (instead of a long wait for a useless answer).
    let mut start = start;
    if role == Role::Read && !can_see(&start, &ch).await {
        let why = format!("{} can't see pictures, so it can't read pages", label(&start));
        if !auto {
            let msg = format!("{why}. Pick a model that can (Claude, Antigravity's Gemini, or an OpenCode vision model), or turn on Auto switch.");
            progress(Progress::Failed(msg.clone()));
            return Err(anyhow!(msg));
        }
        progress(Progress::Log(format!("Auto: {why}; looking for one that can…")));
        match find_working(&reg, Role::Read, &start, std::slice::from_ref(&start), &ch, &progress, &cancel).await {
            Some(m) => {
                progress(Progress::Log(format!("Auto: switching to {} and carrying on.", label(&m))));
                remember(Role::Read, &start, &m);
                start = m;
            }
            None => return Err(anyhow!("{why}, and Auto found no other signed-in model that passed the test")),
        }
    }
    with_auto(&reg, auto, role, start, &ch, &progress, &cancel, |m| crate::read::read_chapter(reg.clone(), m, ch.clone(), brain.clone(), opts.clone(), progress.clone(), cancel.clone())).await
}

/// [`crate::write_story_for_manga`] with the Auto switch.
#[allow(clippy::too_many_arguments)]
pub async fn write_story_for_manga_smart(
    reg: Arc<Registry>,
    model: ModelChoice,
    chapters: Vec<ChapterRef>,
    manga: Vec<ChapterRef>,
    styles: Vec<String>,
    brain: Arc<Mutex<Brain>>,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<Vec<(String, Story)>> {
    let Some(ch) = chapters.first().cloned() else { return crate::story::write_story_for_manga(reg, model, chapters, manga, styles, brain, progress, cancel).await };
    with_auto(&reg, auto_on(), Role::Story, model, &ch, &progress, &cancel, |m| {
        crate::story::write_story_for_manga(reg.clone(), m, chapters.clone(), manga.clone(), styles.clone(), brain.clone(), progress.clone(), cancel.clone())
    })
    .await
}

/// [`crate::write_story_anyway`] with the Auto switch.
pub async fn write_story_anyway_smart(reg: Arc<Registry>, model: ModelChoice, ch: ChapterRef, styles: Vec<String>, brain: Arc<Mutex<Brain>>, progress: ProgressFn, cancel: CancellationToken) -> Result<Vec<(String, Story)>> {
    let c = ch.clone();
    with_auto(&reg, auto_on(), Role::Story, model, &c, &progress, &cancel, |m| crate::story::write_story_anyway(reg.clone(), m, ch.clone(), styles.clone(), brain.clone(), progress.clone(), cancel.clone())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_tries_models_that_can_see_and_light_read_starts_cheap() {
        let oc_vision = vec!["opencode/crouton-alpha-free".to_string()];
        let oc_all = vec!["opencode/big-pickle".to_string(), "opencode/crouton-alpha-free".to_string()];
        let agy = vec!["gemini-3.8-flash-high".to_string(), "gemini-3.8-flash-low".to_string(), "claude-sonnet-4-6".to_string(), "gpt-oss-120b-medium".to_string()];
        let read = models_of(AiProviderId::Antigravity, true, false, &oc_vision, &oc_all, &agy);
        assert_eq!(read, ["gemini-3.8-flash-high", "claude-sonnet-4-6"], "GPT-OSS cannot see; only models this account lists");
        assert!(models_of(AiProviderId::Antigravity, false, false, &oc_vision, &oc_all, &agy).contains(&"gpt-oss-120b-medium".to_string()), "fine for the story");
        assert_eq!(models_of(AiProviderId::Antigravity, true, true, &oc_vision, &oc_all, &agy)[0], "gemini-3.8-flash-low");
        assert_eq!(models_of(AiProviderId::Opencode, true, false, &oc_vision, &oc_all, &agy), oc_vision, "only its vision models read");
        assert_eq!(models_of(AiProviderId::Opencode, false, false, &oc_vision, &oc_all, &agy), ["opencode/crouton-alpha-free", "opencode/big-pickle"]);
        assert!(models_of(AiProviderId::GrokCli, true, false, &oc_vision, &oc_all, &agy).is_empty());
        assert_eq!(models_of(AiProviderId::ClaudeCode, true, true, &oc_vision, &oc_all, &agy)[0], "claude-haiku-5-5");
    }

    #[test]
    fn content_problems_are_not_fixed_by_another_model() {
        assert!(model_cannot_fix(&anyhow!("No page images found in X")));
        assert!(model_cannot_fix(&anyhow!("cancelled")));
        assert!(model_cannot_fix(&anyhow!("Ch 1: No story could be written. Read the chapters first, then try again.")));
        assert!(!model_cannot_fix(&anyhow!("Page 8: the AI sent an empty answer")));
    }

    #[test]
    fn a_model_that_took_over_is_remembered_per_role() {
        let picked = ModelChoice { provider: AiProviderId::Antigravity, model: "gemini-3.8-flash-high".into() };
        let works = ModelChoice { provider: AiProviderId::Opencode, model: "opencode/crouton-alpha-free".into() };
        remember(Role::Read, &picked, &works);
        assert_eq!(remembered(Role::Read, &picked), Some(works));
        assert_eq!(remembered(Role::Story, &picked), None);
        forget(Role::Read, &picked);
        assert_eq!(remembered(Role::Read, &picked), None);
    }
}
