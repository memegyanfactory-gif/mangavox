//! Studio › Hook: a 1-2 minute hook for any manga Studio has written. The AI finds the story's
//! peak moments and the one question to plant, writes three hooks in different styles (a cold
//! open, a contrast, stakes first...), judges them like a recap channel's editor and the winner is
//! voiced and rendered over the chapters' own panels (`hook_maker.py`). Another draft can be
//! rendered instead with one click.
//!
//! The video then goes on like a one-video export with its hook on: the manga's character intro
//! (the cast on cinematic cards, see `export::intro`) and the Netflix-style ribbon transition into
//! chapter 1. A full-recap export with the Hook on plays this hook (without the intro, which the
//! export adds itself) instead of writing another ([`ready_hook`]).
//!
//! What was made is kept in `<studio>/<manga>/hook/` (`hook.json`, `hook.mp4`, the hook alone, and
//! `hook_intro.mp4`, the hook and the intro), so the page shows it again later. The run is
//! app-wide ([`RUN`]): leaving the page does not stop it.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::hook::{fallback_hook, pick_scenes, story_context, BeatInfo, MAX_CANDIDATES};
use mv_recap::hook_ai::{HookDraft, HookResult};
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{badge, btn, muted, page_title, ph, segmented, size, ButtonKind, Tokens, Tone};
use serde::{Deserialize, Serialize};

use crate::data::{ChapterItem, MangaGroup};
use crate::export::{hook_beats, hook_spec, intro, Beat, HookJob, Reporter};
use crate::page::Load;

/// The length choices: the AI's own pick, then 1, 1.5 and 2 minutes.
const LENGTHS: [(Option<u32>, &str); 4] = [(None, "AI decides"), (Some(60), "1 min"), (Some(90), "1.5 min"), (Some(120), "2 min")];

/// What was made for a manga, kept in its `hook/hook.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Saved {
    pub result: HookResult,
    /// The rendered video of `drafts[shown]`, when it was made.
    pub video: Option<PathBuf>,
    /// Which draft the video is.
    pub shown: usize,
    /// Its length in seconds.
    pub seconds: f64,
    /// Chapters it was made from.
    pub chapters: usize,
    /// Their ids (empty in hooks made before they were kept).
    pub chapter_ids: Vec<String>,
    /// The hook alone (`video` is the hook and then the intro when `with_intro`).
    pub hook_only: Option<PathBuf>,
    pub with_intro: bool,
    /// Why the video has no intro, when it could not be made.
    pub intro_note: String,
}

fn hook_dir(manga_id: &str) -> PathBuf {
    mv_core::paths::studio_manga(manga_id).join("hook")
}

fn load_saved(manga_id: &str) -> Option<Saved> {
    let s: Saved = serde_json::from_str(&std::fs::read_to_string(hook_dir(manga_id).join("hook.json")).ok()?).ok()?;
    Some(Saved { video: s.video.clone().filter(|v| v.is_file()), hook_only: s.hook_only.clone().filter(|v| v.is_file()), ..s })
}

/// The hook made on this page for manga `manga_id`, alone (without the intro), when it was made
/// only from chapters among `chapter_ids` (those of the video it would open): a full-recap export
/// plays it. Blocking.
pub(crate) fn ready_hook(manga_id: &str, chapter_ids: &[String]) -> Option<PathBuf> {
    let s = load_saved(manga_id)?;
    hook_for(&s, chapter_ids)
}

fn hook_for(s: &Saved, chapter_ids: &[String]) -> Option<PathBuf> {
    // A hook made before the intro was added: its video is the hook alone.
    let file = s.hook_only.clone().or_else(|| (!s.with_intro).then(|| s.video.clone()).flatten())?;
    let fits = if s.chapter_ids.is_empty() { s.chapters > 0 && s.chapters <= chapter_ids.len() } else { s.chapter_ids.iter().all(|id| chapter_ids.contains(id)) };
    fits.then_some(file)
}

fn save(manga_id: &str, s: &Saved) -> Result<()> {
    let dir = hook_dir(manga_id);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::write(dir.join("hook.json"), serde_json::to_string_pretty(s)?)?;
    Ok(())
}

/// The hook being made, app-wide (one at a time).
struct Running {
    manga_id: String,
    /// Says the stage (the intro's own steps come through it too) and stops the intro.
    reporter: Reporter,
    cancel: mv_ai::CancellationToken,
}

static RUN: Mutex<Option<Running>> = Mutex::new(None);

fn running() -> Option<(String, String)> {
    RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| (r.manga_id.clone(), r.reporter.now().1))
}

fn set_stage(stage: &str) {
    if let Some(r) = RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        r.reporter.set(0., stage);
    }
}

fn cancel_run() {
    if let Some(r) = RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        r.cancel.cancel();
        r.reporter.cancel();
        // A stage that is not a model call (voicing, rendering) sees the cancel at its next check.
    }
}

fn cancelled() -> bool {
    RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_none_or(|r| r.cancel.is_cancelled() || r.reporter.cancelled())
}

/// The written chapters of `g` with the beats a hook can use, in story order.
fn written(g: &MangaGroup) -> Vec<(ChapterItem, Vec<Beat>)> {
    g.chapters.iter().map(|c| (c.clone(), hook_beats(&c.output_dir))).filter(|(_, b)| b.iter().any(|b| b.image.is_some() && !b.text.is_empty())).collect()
}

/// How many chapters of `g` a hook can be made from (cheap: a file check per chapter).
fn written_count(g: &MangaGroup) -> usize {
    g.chapters.iter().filter(|c| c.output_dir.join("narration.studio.json").is_file() || c.output_dir.join("sync_map.json").is_file()).count()
}

pub(crate) struct HookPage {
    library: Load<Vec<MangaGroup>>,
    selected: Option<String>,
    length: usize,
    saved: HashMap<String, Option<Saved>>,
    counts: HashMap<String, usize>,
    error: Option<String>,
    scroll: ScrollHandle,
}

impl HookPage {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(|| -> Result<_> {
                let groups = crate::data::load()?;
                let counts: HashMap<String, usize> = groups.iter().map(|g| (g.id.clone(), written_count(g))).collect();
                let saved: HashMap<String, Option<Saved>> = groups.iter().map(|g| (g.id.clone(), load_saved(&g.id))).collect();
                Ok((groups, counts, saved))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((mut groups, counts, saved)) => {
                        // The mangas a hook can be made from first, the most written first.
                        groups.sort_by(|a, b| counts.get(&b.id).cmp(&counts.get(&a.id)).then(a.title.cmp(&b.title)));
                        let last = std::fs::read_to_string(mv_core::paths::studio().join("hook_last.txt")).unwrap_or_default();
                        this.selected = running().map(|r| r.0).or_else(|| groups.iter().find(|g| g.id == last.trim()).map(|g| g.id.clone())).or_else(|| groups.first().map(|g| g.id.clone()));
                        this.counts = counts;
                        this.saved = saved;
                        this.library = Load::Ready(groups);
                    }
                    Err(e) => this.library = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        let mut page = HookPage { library: Load::Loading, selected: None, length: 0, saved: HashMap::new(), counts: HashMap::new(), error: None, scroll: ScrollHandle::new() };
        if running().is_some() {
            page.tick(cx);
        }
        page
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        let _ = std::fs::write(mv_core::paths::studio().join("hook_last.txt"), &id);
        self.selected = Some(id);
        self.error = None;
        cx.notify();
    }

    /// Redraws every half second while a hook is being made (the stage line), then loads what it
    /// made.
    fn tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let busy = running();
            let Ok(()) = this.update(cx, |this, cx| {
                if busy.is_none() {
                    if let Some(id) = this.selected.clone() {
                        this.saved.insert(id.clone(), load_saved(&id));
                    }
                }
                cx.notify();
            }) else {
                return;
            };
            if busy.is_none() {
                return;
            }
        })
        .detach();
    }

    /// Makes the hook of `g`: writes it (unless `draft` names one already written to render
    /// instead), then voices and renders it.
    fn make(&mut self, g: MangaGroup, draft: Option<usize>, cx: &mut Context<Self>) {
        if running().is_some() {
            return;
        }
        let env = crate::setup::latest().map(|c| c.env);
        let Some((python, layout)) = env.and_then(|e| Some((e.python.ok()?, e.scripts.ok()?))) else {
            self.error = Some("The hook is voiced by Studio's Python, which is not set up yet (or Studio has not checked it since the app started). Open Studio once, finish Setup, then come back.".into());
            cx.notify();
            return;
        };
        let (provider, model) = ui_assistant::assistant_model(cx);
        let model = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)).ok().map(|provider| mv_recap::ModelChoice { provider, model });
        if model.is_none() && draft.is_none() {
            self.error = Some("Pick an AI model in the assistant first: it writes and judges the hook.".into());
            cx.notify();
            return;
        }
        let reg = ui_assistant::registry(cx);
        let seconds = LENGTHS[self.length].0;
        let earlier = self.saved.get(&g.id).cloned().flatten();
        let cancel = mv_ai::CancellationToken::new();
        // The intro after the hook, as a one-video export makes it (its cast profiles written with
        // the same model).
        let intro_job = intro::IntroJob {
            manga_id: g.id.clone(),
            manga_title: g.title.clone(),
            chapters: g.chapters.clone(),
            numbers: intro::chapter_numbers(&g.chapters),
            model_name: model.as_ref().map(|m| m.model.clone()).unwrap_or_default(),
            model: model.clone().filter(|m| !m.model.trim().is_empty()),
            reg: reg.clone(),
            python: python.clone(),
            layout: layout.clone(),
        };
        let reporter = Reporter::standalone(Arc::new(AtomicBool::new(false)));
        reporter.set(0., "Reading the written chapters");
        // In Activity too (its Cancel stops the run), so it is seen from every page.
        reporter.track(&g.id, &format!("Hook · {}", g.title));
        *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running { manga_id: g.id.clone(), reporter: reporter.clone(), cancel: cancel.clone() });
        self.error = None;
        self.tick(cx);
        cx.notify();
        let ex = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let id = g.id.clone();
            let res = run(g, draft, earlier, seconds, model, reg, python, layout, cancel, &ex).await;
            let res = match res {
                Ok(s) => with_intro(s, intro_job, &reporter, &ex).await,
                Err(e) => Err(e),
            };
            drop(reporter);
            *RUN.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(s) => {
                        let score = s.result.drafts.get(s.shown).map(|d| d.score).unwrap_or(0);
                        let how = if s.with_intro { "with the intro and the ribbon" } else { "hook only, see the note on the page" };
                        toast::show(cx, Toast::success(format!("Hook ready · {score}/100 · {} · {how}", fmt_secs(s.seconds))));
                        this.saved.insert(id, Some(s));
                    }
                    Err(e) if e.to_string() == "cancelled" => {
                        this.saved.insert(id.clone(), load_saved(&id));
                    }
                    Err(e) => {
                        this.saved.insert(id.clone(), load_saved(&id));
                        this.error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn manga_strip(&self, groups: &[MangaGroup], cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().gap(px(space::SM)).children(groups.iter().enumerate().map(|(i, g)| {
            let on = self.selected.as_deref() == Some(g.id.as_str());
            let id = g.id.clone();
            let written = self.counts.get(&g.id).copied().unwrap_or(0);
            let made = self.saved.get(&g.id).and_then(|s| s.as_ref()).and_then(|s| s.result.drafts.get(s.shown).map(|d| d.score));
            let note = match (written, made) {
                (0, _) => "No chapter written yet".to_string(),
                (n, Some(score)) => format!("Hook {score}/100 · {n} chapter{}", if n == 1 { "" } else { "s" }),
                (n, None) => format!("{n} chapter{} written", if n == 1 { "" } else { "s" }),
            };
            div()
                .id(("hk-manga", i))
                .flex()
                .items_center()
                .gap(px(space::SM))
                .w(px(250.))
                .p(px(6.))
                .pr(px(space::MD))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if on { Tokens::accent() } else { Tokens::border() })
                .bg(if on { Tokens::accent_soft() } else { Tokens::surface() })
                .when(written == 0, |d| d.opacity(0.6))
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select(id.clone(), cx)))
                .child(
                    div()
                        .flex_none()
                        .w(px(40.))
                        .h(px(56.))
                        .rounded(px(radius::MD))
                        .overflow_hidden()
                        .bg(Tokens::surface_2())
                        .when_some(g.cover.clone().filter(|c| c.is_file()), |d, c| d.child(img(c).size_full().object_fit(ObjectFit::Cover))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).line_clamp(2).child(g.title.clone()))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(note)),
                )
        }))
    }

    fn controls(&self, g: &MangaGroup, cx: &mut Context<Self>) -> Div {
        let busy = running();
        let here = busy.as_ref().is_some_and(|b| b.0 == g.id);
        let written = self.counts.get(&g.id).copied().unwrap_or(0);
        let has = self.saved.get(&g.id).is_some_and(|s| s.is_some());
        let lengths = segmented("hk-length", LENGTHS.iter().map(|l| (None, l.1)), self.length).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            this.length = *ix;
            cx.notify();
        }));
        let go = {
            let g = g.clone();
            let b = btn("hk-make", if has { "Make a new hook" } else { "Make hook" }, ButtonKind::Primary).icon(ph::LIGHTNING).loading(here);
            let b = if written == 0 {
                b.disabled("Write at least one chapter of this manga in Studio first: the hook is made from its narration and panels.")
            } else if busy.is_some() && !here {
                b.disabled("Another hook is being made.")
            } else {
                b
            };
            b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.make(g.clone(), None, cx)))
        };
        let mut row = div().flex().flex_wrap().items_center().gap(px(space::MD)).child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Length")).child(lengths).child(go);
        if here {
            row = row.child(btn("hk-cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).on_click(|_, _, _| cancel_run()));
        }
        let mut col = div().flex().flex_col().gap(px(space::SM)).child(row);
        if let Some((_, stage)) = busy.filter(|b| b.0 == g.id) {
            col = col.child(div().flex().items_center().gap(px(space::SM)).child(mv_ui::progress_bar("hk-bar", 0., true)).child(div().flex_none().text_size(px(size::SM)).text_color(Tokens::text_2()).child(stage)));
        }
        if let Some(e) = &self.error {
            col = col.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e.clone()));
        }
        col
    }

    fn result(&self, g: &MangaGroup, s: &Saved, cx: &mut Context<Self>) -> Div {
        let plan = &s.result.plan;
        let busy = running().is_some();
        let mut top = div().flex().flex_col().gap(px(space::SM)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border());
        let shown = s.result.drafts.get(s.shown);
        let mut head = div().flex().flex_wrap().items_center().gap(px(space::SM)).child(div().text_size(px(size::LG)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Your hook"));
        if let Some(d) = shown {
            head = head.child(badge(format!("{}/100", d.score), score_tone(d.score))).child(badge(title_case(&d.angle.kind), Tone::Neutral));
        }
        if s.seconds > 0. {
            head = head.child(badge(fmt_secs(s.seconds), Tone::Neutral));
        }
        top = top.child(head);
        if !plan.question.is_empty() {
            top = top.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("The question it plants: {}", plan.question)));
        }
        if !plan.genre.is_empty() {
            top = top.child(muted(format!("Genre: {} · made from {} chapter{}", plan.genre, s.chapters, if s.chapters == 1 { "" } else { "s" })));
        }
        let mut actions = div().flex().flex_wrap().gap(px(space::SM));
        if let Some(v) = s.video.clone() {
            let (title, v2) = (format!("{} · Hook", g.title), v.clone());
            actions = actions
                .child(btn("hk-play", "Play", ButtonKind::Primary).icon(ph::PLAY).on_click(move |_, window, cx| crate::phases::play_video(&title, &v, window, cx)))
                .child(btn("hk-folder", "Show in folder", ButtonKind::Secondary).icon(ph::FOLDER_OPEN).on_click(move |_, _, cx| cx.reveal_path(&v2)));
        } else if !busy {
            let g2 = g.clone();
            let shown_ix = s.shown;
            actions = actions.child(btn("hk-render", "Render this hook", ButtonKind::Primary).icon(ph::FILM_STRIP).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.make(g2.clone(), Some(shown_ix), cx))));
        }
        top = top.child(actions);
        if s.with_intro {
            top = top.child(muted("Plays the hook, then the character intro and the Netflix-style ribbon transition into chapter 1. A full-recap export with the Hook on uses this hook."));
        } else if !s.intro_note.is_empty() {
            top = top.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child(s.intro_note.clone()));
        }
        for n in &s.result.notes {
            top = top.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Note: {n}")));
        }
        let drafts = s.result.drafts.iter().enumerate().map(|(i, d)| self.draft_card(g, i, d, i == s.shown, busy, cx));
        div().flex().flex_col().gap(px(space::MD)).child(top).child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Drafts, judged")).children(drafts)
    }

    fn draft_card(&self, g: &MangaGroup, i: usize, d: &HookDraft, shown: bool, busy: bool, cx: &mut Context<Self>) -> Div {
        let mut head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{} · {}", if i == 0 { "Winner" } else { "Draft" }, title_case(&d.angle.kind))))
            .child(badge(format!("{}/100", d.score), score_tone(d.score)));
        if shown {
            head = head.child(badge("In the video", Tone::Accent));
        } else if !busy {
            let g = g.clone();
            head = head.child(btn(("hk-use", i), "Render this one instead", ButtonKind::Ghost).small().icon(ph::ARROWS_CLOCKWISE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.make(g.clone(), Some(i), cx))));
        }
        let points = div().flex().flex_wrap().gap(px(space::XS)).children(d.points.iter().map(|(name, p)| badge(format!("{name} {p}"), Tone::Neutral)));
        let lines = div().flex().flex_col().gap(px(4.)).children(d.script.lines.iter().map(|l| div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(l.text.clone()))).child(div().text_size(px(size::SM)).text_color(Tokens::accent()).child(d.script.outro.clone()));
        let mut card = div().flex().flex_col().gap(px(space::SM)).p(px(space::LG)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(if shown { Tokens::accent() } else { Tokens::border() }).child(head);
        if !d.angle.pitch.is_empty() {
            card = card.child(muted(d.angle.pitch.clone()));
        }
        if !d.points.is_empty() {
            card = card.child(points);
        }
        if !d.verdict.is_empty() {
            card = card.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("Editor: {}", d.verdict)));
        }
        card.child(lines)
    }
}

fn score_tone(score: u32) -> Tone {
    match score {
        85.. => Tone::Success,
        65..85 => Tone::Info,
        45..65 => Tone::Warning,
        _ => Tone::Danger,
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn fmt_secs(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Writes (unless `draft` names one of `earlier`'s drafts to render), voices and renders the
/// hook of `g`. Saves what it made as it goes.
#[allow(clippy::too_many_arguments)]
async fn run(
    g: MangaGroup,
    draft: Option<usize>,
    earlier: Option<Saved>,
    seconds: Option<u32>,
    model: Option<mv_recap::ModelChoice>,
    reg: Arc<mv_ai::Registry>,
    python: mv_studio::PythonInterpreter,
    layout: mv_studio::PythonLayout,
    cancel: mv_ai::CancellationToken,
    ex: &BackgroundExecutor,
) -> Result<Saved> {
    let job_reg = reg.clone();
    let gg = g.clone();
    let chapters = mv_ui::bg::blocking(move || written(&gg)).await?;
    if chapters.is_empty() {
        bail!("No chapter of this manga is written yet. Write a few chapters in Studio first: the hook is made from their narration and panels.");
    }
    let infos: Vec<(String, Vec<BeatInfo>)> = chapters.iter().map(|(c, b)| (c.label(), b.iter().map(Beat::info).collect())).collect();
    let candidates = pick_scenes(&infos, MAX_CANDIDATES);
    if candidates.len() < 2 {
        bail!("The written chapters have too little narration with pictures for a hook.");
    }
    // The script: written now, or a draft written before.
    let mut saved = match (draft, earlier) {
        (Some(i), Some(mut s)) if i < s.result.drafts.len() => {
            s.shown = i;
            s.video = None;
            s.hook_only = None;
            s.with_intro = false;
            s
        }
        _ => {
            let Some(model) = model else { bail!("Pick an AI model in the assistant first.") };
            let (id, title, last) = (g.id.clone(), g.title.clone(), chapters.last().map(|(c, _)| c.label()).unwrap_or_default());
            let cands = candidates.clone();
            let token = cancel.clone();
            let task = mv_ui::bg::runtime().spawn(async move {
                let story = story_context(&mv_recap::Brain::load(&id, &title), &last);
                mv_recap::hook::best_hook(reg, model, &title, &story, &cands, seconds, token, Arc::new(set_stage)).await
            });
            let res = wait(task, ex).await?;
            if cancelled() {
                bail!("cancelled");
            }
            let result = match res {
                Ok(r) => r,
                Err(e) => {
                    let secs = seconds.unwrap_or(90);
                    let script = fallback_hook(&candidates, secs);
                    HookResult {
                        plan: mv_recap::hook_ai::HookPlan { seconds: secs, ..Default::default() },
                        drafts: vec![HookDraft { angle: mv_recap::hook_ai::Angle { kind: "the narration's own lines".into(), pitch: String::new() }, script, ..Default::default() }],
                        notes: vec![format!("The AI could not write the hook ({e:#}), so it uses the narration's own lines.")],
                    }
                }
            };
            Saved { result, chapters: chapters.len(), chapter_ids: chapters.iter().map(|(c, _)| c.id.clone()).collect(), ..Default::default() }
        }
    };
    let id = g.id.clone();
    let s2 = saved.clone();
    mv_ui::bg::blocking(move || save(&id, &s2)).await??;
    // Voice and render it.
    set_stage("Voicing the hook");
    let script = saved.result.drafts.get(saved.shown).map(|d| d.script.clone()).ok_or_else(|| anyhow!("the hook has no draft"))?;
    let job = HookJob {
        manga_id: g.id.clone(),
        manga_title: g.title.clone(),
        cover: g.cover.clone().filter(|p| p.is_file()),
        chapters: chapters.iter().map(|(c, _)| c.clone()).collect(),
        seconds: saved.result.plan.seconds.max(15),
        model: None,
        reg: job_reg,
        python: python.clone(),
        layout: layout.clone(),
    };
    let work = hook_dir(&g.id);
    let spec = hook_spec(&job, &chapters, &script, "1080p", &work).ok_or_else(|| anyhow!("the hook's lines have no pictures"))?;
    let out = PathBuf::from(spec["out"].as_str().unwrap_or_default());
    let render = work.join("render");
    let args_file = work.join("args.json");
    let spec_text = serde_json::to_string(&spec)?;
    let (r2, a2) = (render.clone(), args_file.clone());
    mv_ui::bg::blocking(move || -> Result<()> {
        let _ = std::fs::remove_dir_all(&r2);
        std::fs::write(&a2, spec_text)?;
        Ok(())
    })
    .await??;
    let script_path = layout.script("hook_maker.py").map_err(|e| anyhow!("{e}"))?;
    let (py, dir, arg) = (python, layout.dir.clone(), args_file.to_string_lossy().into_owned());
    let task = mv_ui::bg::runtime().spawn(async move { mv_studio::bridge::run_capture(&py, &script_path, &[arg], &dir).await });
    let voiced = render.join("sync_map.json");
    let mut rendering = false;
    let res = loop {
        if task.is_finished() {
            break task.await.map_err(|e| anyhow!("{e}"))?;
        }
        if cancelled() {
            task.abort();
            bail!("cancelled");
        }
        if !rendering && voiced.is_file() {
            rendering = true;
            set_stage("Rendering the hook over the panels");
        }
        ex.timer(Duration::from_millis(300)).await;
    };
    let (code, stdout, stderr) = res.map_err(|e| anyhow!("{e}"))?;
    let last = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let v: serde_json::Value = serde_json::from_str(last).unwrap_or(serde_json::Value::Null);
    if v["ok"].as_bool() != Some(true) {
        let why = v["message"].as_str().map(str::to_string).or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string)).unwrap_or_else(|| format!("hook_maker exited {code:?}"));
        bail!("The hook was written but could not be rendered: {why}");
    }
    saved.video = Some(out).filter(|p| p.is_file());
    saved.hook_only = saved.video.clone();
    saved.with_intro = false;
    saved.seconds = v["seconds"].as_f64().unwrap_or(0.);
    let (id, s2) = (g.id.clone(), saved.clone());
    mv_ui::bg::blocking(move || {
        let _ = std::fs::remove_dir_all(&render);
        save(&id, &s2)
    })
    .await??;
    Ok(saved)
}

/// Goes on from the rendered hook of `saved` like a one-video export with its hook on: the manga's
/// chapter videos joined (as Play and Export join them, kept), its character intro made or the
/// kept one reused (`job`, each step through `r`), then the hook and the intro joined into
/// `hook_intro.mp4`, which becomes the page's video. Without a chapter video, or when the intro
/// cannot be made, the hook stays alone and the page says why.
async fn with_intro(mut saved: Saved, job: intro::IntroJob, r: &Reporter, ex: &BackgroundExecutor) -> Result<Saved> {
    let Some(hook) = saved.hook_only.clone() else { return Ok(saved) };
    let id = job.manga_id.clone();
    let files: Vec<PathBuf> = job.chapters.iter().map(|c| c.output_dir.join(crate::data::VIDEO_FILE)).filter(|p| p.is_file()).collect();
    let made = if files.is_empty() {
        Err(anyhow!("no chapter of this manga has a video yet (the intro opens onto chapter 1's first frame)"))
    } else {
        r.set(0., "Joining the chapter videos for the intro");
        let (rr, out) = (r.clone(), crate::export::joined_path(&id));
        let joined = mv_ui::bg::blocking(move || -> Result<_> {
            rr.take_turn()?;
            crate::export::join(&files, &out, &rr)
        })
        .await
        .and_then(|x| x);
        match joined {
            Ok(j) => intro::make_intro(job, &j, r, ex).await.map(|m| m.path),
            Err(e) => Err(e),
        }
    };
    if r.cancelled() || cancelled() {
        bail!("cancelled");
    }
    match made {
        Ok(intro_file) => {
            r.set(0.99, "Joining the hook, the intro and the transition");
            let (rr, out, h) = (r.clone(), hook_dir(&id).join("hook_intro.mp4"), hook.clone());
            let res = mv_ui::bg::blocking(move || crate::export::hook_then_intro(&h, &intro_file, &out, &rr).map(|secs| (out, secs))).await.and_then(|x| x);
            match res {
                Ok((out, secs)) => {
                    saved.video = Some(out);
                    saved.with_intro = true;
                    saved.seconds = secs;
                    saved.intro_note.clear();
                }
                Err(_) if r.cancelled() => bail!("cancelled"),
                Err(e) => saved.intro_note = format!("The hook and the intro could not be joined ({e:#}), so the video is the hook alone."),
            }
        }
        Err(e) => saved.intro_note = format!("The video is the hook alone: the character intro could not be made ({e:#})."),
    }
    let s2 = saved.clone();
    mv_ui::bg::blocking(move || save(&id, &s2)).await??;
    Ok(saved)
}

/// Awaits `task`, giving up when the run is cancelled.
async fn wait<T: Send + 'static>(task: tokio::task::JoinHandle<T>, ex: &BackgroundExecutor) -> Result<T> {
    loop {
        if task.is_finished() {
            return task.await.map_err(|e| anyhow!("{e}"));
        }
        if cancelled() {
            // The model call sees the token and stops; give it a moment, then leave it.
            for _ in 0..20 {
                if task.is_finished() {
                    break;
                }
                ex.timer(Duration::from_millis(100)).await;
            }
            task.abort();
            bail!("cancelled");
        }
        ex.timer(Duration::from_millis(300)).await;
    }
}

impl Render for HookPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = page_title("Hook")
            .icon(ph::LIGHTNING)
            .subtitle("A 1-2 minute hook for any manga Studio has written. The AI finds the peak moments and the one question to plant, writes three hooks in different styles, judges them like a top recap channel's editor, and renders the winner over the chapters' own panels, followed by the character intro and the Netflix-style ribbon transition into chapter 1.");
        let body: AnyElement = match &self.library {
            Load::Loading => muted("Loading your manga...").into_any_element(),
            Load::Failed(e) => div().text_size(px(size::SM)).text_color(Tokens::danger()).child(format!("Could not load your manga. {e}")).into_any_element(),
            Load::Ready(groups) if groups.is_empty() => muted("Download a manga and let Studio write a few chapters: the hook is made from their narration and panels.").into_any_element(),
            Load::Ready(groups) => {
                let groups = groups.clone();
                let current = self.selected.clone().and_then(|id| groups.iter().find(|g| g.id == id).cloned()).or_else(|| groups.first().cloned());
                let strip = self.manga_strip(&groups, cx);
                let mut col = div().flex().flex_col().gap(px(space::LG)).child(strip);
                if let Some(g) = current {
                    col = col.child(self.controls(&g, cx));
                    if let Some(Some(s)) = self.saved.get(&g.id).cloned() {
                        col = col.child(self.result(&g, &s, cx));
                    }
                }
                col.into_any_element()
            }
        };
        div().size_full().flex().flex_col().child(
            div()
                .id("hk-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(div().flex().flex_col().gap(px(space::MD)).px(px(size::PAGE_X)).py(px(size::PAGE_Y)).child(header).child(body)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The UI kit's glob brings its own `test` attribute (for UI tests); these are plain tests.
    use core::prelude::v1::test;

    #[test]
    fn scores_read_at_a_glance() {
        assert_eq!((score_tone(90), score_tone(70), score_tone(50), score_tone(10)), (Tone::Success, Tone::Info, Tone::Warning, Tone::Danger));
        assert_eq!((fmt_secs(92.4), fmt_secs(59.6)), ("1:32".to_string(), "1:00".to_string()));
        assert_eq!(title_case("cold open"), "Cold open");
    }

    #[test]
    fn a_saved_hook_round_trips_and_drops_a_missing_video() {
        let d = tempfile::tempdir().unwrap();
        let s = Saved { video: Some(d.path().join("gone.mp4")), shown: 1, seconds: 61.5, chapters: 3, ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        let back: Saved = serde_json::from_str(&text).unwrap();
        assert_eq!((back.shown, back.chapters, back.seconds), (1, 3, 61.5));
        // Older files without some fields still load.
        let old: Saved = serde_json::from_str(r#"{"result": {"drafts": []}}"#).unwrap();
        assert!(old.video.is_none() && old.result.drafts.is_empty());
    }
    #[test]
    fn an_export_uses_the_hook_alone_and_only_for_its_chapters() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (alone, both) = (PathBuf::from("hook.mp4"), PathBuf::from("hook_intro.mp4"));
        let s = Saved { video: Some(both.clone()), hook_only: Some(alone.clone()), with_intro: true, chapters: 2, chapter_ids: ids(&["a", "b"]), ..Default::default() };
        assert_eq!(hook_for(&s, &ids(&["a", "b", "c"])), Some(alone.clone()), "the export adds the intro itself");
        assert_eq!(hook_for(&s, &ids(&["a", "c"])), None, "made from a chapter the video leaves out");
        // Made before the intro: the video is the hook alone.
        let old = Saved { video: Some(alone.clone()), chapters: 2, ..Default::default() };
        assert_eq!(hook_for(&old, &ids(&["a", "b"])), Some(alone));
        assert_eq!(hook_for(&old, &ids(&["a"])), None);
        let no_hook_alone = Saved { video: Some(both), with_intro: true, chapters: 1, ..Default::default() };
        assert_eq!(hook_for(&no_hook_alone, &ids(&["a"])), None);
    }
}
