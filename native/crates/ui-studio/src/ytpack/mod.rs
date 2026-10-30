//! The dashboard's YouTube package for one manga: a thumbnail prompt for any image AI built from
//! the Studio character bank (with the face crops to attach), the recaps of the same manga already
//! on YouTube (found under every title it goes by, compared with a reference channel), a rough
//! views prediction, and a title / description / thumbnail concept from the picked AI model.
//! Saved per manga in `studio/<manga>/youtube_package.json`.
mod assist;
mod logic;
mod scrape;

pub(crate) use assist::{has_title, register_tools};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, bail, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{badge, btn, icon_button, muted, ph, ph_icon, shimmer, size, tip, Btn, ButtonKind, Ph, Tokens, Tone, Weight};
use serde::{Deserialize, Serialize};

use self::logic::{short_count, Cast, Matcher, Prediction, Suggestion, Video};
use crate::data::MangaGroup;
use crate::page::{Load, StudioPage};

/// The channel recaps are compared with.
const REFERENCE: &str = "@Manhwa_Fresh";
/// A video counts as a recap of this manga from this match score up.
const SAME_MANGA: f32 = 0.6;
const CAST_FOR_PROMPT: usize = 3;
const CAST_FOR_MATCH: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// Every title the manga goes by (AniList), the library's first.
    names: Vec<String>,
    /// Search results, scored.
    videos: Vec<Video>,
    reference_name: String,
    reference_subs: String,
    /// The reference channel's latest uploads (the baseline).
    reference_recent: Vec<Video>,
    prediction: Option<Prediction>,
    suggestion: Option<Suggestion>,
    /// Unix seconds of the last research.
    researched_at: u64,
}

impl Saved {
    fn path(manga_id: &str) -> PathBuf {
        mv_recap::Brain::dir(manga_id).join("youtube_package.json")
    }
    fn load(manga_id: &str) -> Saved {
        std::fs::read(Self::path(manga_id)).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }
    fn save(&self, manga_id: &str) {
        let p = Self::path(manga_id);
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(d) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, d);
        }
    }
    fn comparable(&self) -> Vec<Video> {
        let mut v: Vec<Video> = self.videos.iter().filter(|v| v.score >= SAME_MANGA && self.names.first().is_some_and(|t| *t == v.covers)).cloned().collect();
        v.sort_by_key(|v| std::cmp::Reverse(v.views));
        v
    }
}

pub(crate) struct YtPackage {
    manga_id: String,
    title: String,
    /// Other manga in the library, so a recap can be told apart from theirs.
    others: Vec<String>,
    cast: Vec<(Cast, Option<PathBuf>)>,
    /// Characters' names and aliases for matching descriptions.
    cast_names: Vec<String>,
    story: Vec<String>,
    faces_dir: PathBuf,
    saved: Saved,
    open: bool,
    show_all: bool,
    /// Long texts currently shown in full.
    expanded: std::collections::HashSet<&'static str>,
    researching: bool,
    suggesting: bool,
    error: Option<String>,
}

struct Panels(HashMap<String, Entity<YtPackage>>);
impl Global for Panels {}

impl StudioPage {
    /// The YouTube package card for the open manga on the dashboard.
    pub(crate) fn youtube_package(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        if !cx.has_global::<Panels>() {
            cx.set_global(Panels(HashMap::new()));
        }
        let others: Vec<String> = match &self.library {
            Load::Ready(gs) => gs.iter().filter(|o| o.id != g.id).map(|o| o.title.clone()).collect(),
            _ => vec![],
        };
        let found = cx.global::<Panels>().0.get(&g.id).cloned();
        let panel = match found {
            Some(p) => {
                p.update(cx, |p, _| p.others = others);
                p
            }
            None => {
                let p = cx.new(|_| YtPackage::new(g, others));
                cx.global_mut::<Panels>().0.insert(g.id.clone(), p.clone());
                p
            }
        };
        AnyView::from(panel).into_any_element()
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl YtPackage {
    fn new(g: &MangaGroup, others: Vec<String>) -> Self {
        let brain = crate::run::brain_snapshot(&g.id, &g.title);
        let cast = logic::lead_cast(&brain, CAST_FOR_MATCH);
        let cast_names = cast.iter().flat_map(|(c, _)| std::iter::once(c.name.clone()).chain(c.aliases.iter().cloned())).collect();
        let saved = Saved::load(&g.id);
        YtPackage {
            manga_id: g.id.clone(),
            title: g.title.clone(),
            others,
            cast_names,
            cast: cast.into_iter().take(CAST_FOR_PROMPT).collect(),
            story: logic::story_so_far(&brain.notes, 4),
            faces_dir: brain.faces_dir(),
            open: saved.researched_at > 0,
            saved,
            show_all: false,
            expanded: Default::default(),
            researching: false,
            suggesting: false,
            error: None,
        }
    }

    fn prompt(&self) -> String {
        let cast: Vec<Cast> = self.cast.iter().map(|(c, _)| c.clone()).collect();
        let refs: Vec<bool> = self.cast.iter().map(|(_, f)| f.is_some()).collect();
        let hook = self.saved.suggestion.as_ref().map(|s| s.thumbnail_text.clone()).unwrap_or_default();
        logic::thumbnail_prompt(&self.title, &cast, &refs, &self.story, &hook)
    }

    fn research(&mut self, cx: &mut Context<Self>) {
        if self.researching {
            return;
        }
        self.researching = true;
        self.error = None;
        self.open = true;
        let (title, others, cast_names) = (self.title.clone(), self.others.clone(), self.cast_names.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(research(title, others, cast_names)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.researching = false;
                match res {
                    Ok(mut s) => {
                        s.suggestion = this.saved.suggestion.take();
                        this.saved = s;
                        this.saved.save(&this.manga_id);
                    }
                    Err(e) => this.error = Some(format!("YouTube research failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn suggest(&mut self, cx: &mut Context<Self>) {
        if self.suggesting {
            return;
        }
        let (provider, model) = ui_assistant::assistant_model(cx);
        let Ok(provider) = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)) else {
            self.error = Some("Pick an AI model first (model button at the top).".into());
            cx.notify();
            return;
        };
        let reg = ui_assistant::registry(cx);
        let cast: Vec<Cast> = self.cast.iter().map(|(c, _)| c.clone()).collect();
        let comparable = self.saved.comparable();
        let prediction = self.saved.prediction.clone().unwrap_or_else(|| logic::predict(&comparable, &self.saved.reference_recent, &self.saved.reference_name));
        let b = logic::Brief {
            title: &self.title,
            aliases: self.saved.names.get(1..).unwrap_or(&[]),
            cast: &cast,
            story: &self.story,
            comparable: &comparable,
            reference: &self.saved.reference_recent,
            reference_name: &self.saved.reference_name,
            prediction: &prediction,
        };
        let (system, user) = logic::suggest_prompt(&b);
        self.suggesting = true;
        self.error = None;
        self.open = true;
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(ask(reg, provider, model, system, user)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.suggesting = false;
                match res {
                    Ok(s) => {
                        this.saved.suggestion = Some(s);
                        this.saved.save(&this.manga_id);
                    }
                    Err(e) => this.error = Some(format!("The AI could not write suggestions: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Searches YouTube under every title of the manga and reads the reference channel.
async fn research(title: String, others: Vec<String>, cast_names: Vec<String>) -> Result<Saved> {
    let mut names = vec![title.clone()];
    for n in scrape::anilist_titles(&title).await.unwrap_or_default() {
        if !names.iter().any(|m| logic::normalize(m) == logic::normalize(&n)) {
            names.push(n);
        }
    }
    // Latin titles get "recap"; the original-language ones are searched as they are.
    let mut queries: Vec<String> = names.iter().filter(|n| n.is_ascii()).take(3).map(|n| format!("{n} recap")).collect();
    queries.extend(names.iter().find(|n| !n.is_ascii()).cloned());
    let searches = futures::future::join_all(queries.iter().map(|q| scrape::search(q)));
    let reference = scrape::channel_videos(REFERENCE);
    let (results, reference) = futures::join!(searches, reference);
    let (reference_name, reference_subs, reference_recent) = reference.unwrap_or_else(|_| (REFERENCE.trim_start_matches('@').replace('_', " "), String::new(), vec![]));
    let ref_hits = futures::future::join_all(names.iter().filter(|n| n.is_ascii()).take(2).map(|n| scrape::channel_search(REFERENCE, n, &reference_name))).await;

    let mut videos: Vec<Video> = vec![];
    let mut failures = 0;
    for r in results {
        match r {
            Ok(vs) => videos.extend(vs),
            Err(_) => failures += 1,
        }
    }
    if failures == queries.len() && !queries.is_empty() {
        bail!("could not search YouTube (offline, or YouTube blocked the request)");
    }
    for v in ref_hits.into_iter().flatten().flatten() {
        videos.push(Video { reference: true, ..v });
    }
    let mut seen = std::collections::HashSet::new();
    videos.retain(|v| seen.insert(v.id.clone()));

    let mut matcher = Matcher { manga: vec![(title.clone(), names.clone(), cast_names)] };
    matcher.manga.extend(others.into_iter().map(|t| (t.clone(), vec![t], vec![])));
    let mut reference_recent = reference_recent;
    for v in videos.iter_mut().chain(reference_recent.iter_mut()) {
        let (covers, score) = matcher.best(v);
        v.covers = covers;
        v.score = score;
    }
    videos.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.views.cmp(&a.views)));
    let mut saved = Saved { names, videos, reference_name, reference_subs, reference_recent, researched_at: now(), ..Default::default() };
    saved.prediction = Some(logic::predict(&saved.comparable(), &saved.reference_recent, &saved.reference_name));
    Ok(saved)
}

async fn ask(reg: std::sync::Arc<mv_ai::Registry>, provider: mv_ai::AiProviderId, model: String, system: String, user: String) -> Result<Suggestion> {
    static N: AtomicU64 = AtomicU64::new(0);
    let mut messages = vec![mv_ai::AiMessage::system(system), mv_ai::AiMessage::user(user)];
    for attempt in 0..2 {
        let req = mv_ai::AiStepRequest {
            request_id: format!("studio-ytpack-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
            provider_id: provider,
            model: model.clone(),
            messages: messages.clone(),
            tools: None,
            effort: None,
            access: None,
            images: Vec::new(),
            workspace: None,
            lean: true,
        };
        let r = reg.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
        if let Some(e) = r.error {
            bail!(e);
        }
        if let Some(s) = logic::parse_suggestion(&r.text) {
            return Ok(s);
        }
        if attempt == 0 {
            messages.push(mv_ai::AiMessage::assistant(r.text));
            messages.push(mv_ai::AiMessage::user("Your previous answer was not valid JSON. Reply with the JSON object only."));
        }
    }
    Err(anyhow!("the answer had no usable title; try again or pick another model"))
}

// ---------------------------------------------------------------- view

fn copy_btn(id: impl Into<ElementId>, what: &'static str, text: String) -> Btn {
    btn(id, "Copy", ButtonKind::Ghost).small().icon(ph::COPY).tooltip(format!("Copy the {what}")).on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        toast::show(cx, Toast::success(format!("Copied the {what}")).icon(ph::COPY));
    })
}

/// A small section heading: icon, title and a dim hint.
fn section(icon: Ph, title: &str, sub: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .child(ph_icon(icon, Weight::Duotone, Tokens::accent(), icon_px::SM))
        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string()))
        .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(1).child(sub.into()))
}

/// Label column plus value, the card's one grid shape.
fn row(label: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(space::MD))
        .child(div().flex_none().w(px(84.)).pt(px(2.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(label))
        .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(value))
}

/// Whether text is long enough to need the "Show more" toggle at this clamp.
fn is_long(text: &str, lines: usize) -> bool {
    text.chars().count() > 85 * lines || text.lines().count() > lines
}

/// A grey placeholder bar with the shared shimmer sweeping across it.
fn skeleton(w: Option<f32>, h: f32) -> Div {
    let d = div().relative().overflow_hidden().h(px(h)).rounded(px(radius::SM)).bg(Tokens::surface_2()).child(shimmer());
    match w {
        Some(w) => d.w(px(w)),
        None => d.w_full(),
    }
}

fn skeleton_rows(widths: &[Option<f32>], h: f32) -> Div {
    div().flex().flex_col().gap(px(6.)).children(widths.iter().map(|w| skeleton(*w, h)))
}

fn match_tone(score: f32) -> Tone {
    if score >= 0.9 {
        Tone::Success
    } else if score >= SAME_MANGA {
        Tone::Info
    } else {
        Tone::Neutral
    }
}

fn video_card(id: (&'static str, usize), v: &Video) -> Stateful<Div> {
    let url = v.url();
    let pct = format!("{:.0}%", v.score * 100.);
    let tag = if v.score >= 0.3 && !v.covers.is_empty() { format!("{pct} · {}", v.covers.chars().take(16).collect::<String>()) } else { format!("{pct} match") };
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_none()
        .w(px(160.))
        .gap(px(4.))
        .rounded(px(radius::MD))
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .child(
            div()
                .relative()
                .w(px(160.))
                .h(px(90.))
                .rounded(px(radius::SM))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .child(img(SharedString::from(v.thumb())).size_full().object_fit(ObjectFit::Cover))
                .when(!v.length.is_empty(), |d| {
                    d.child(div().absolute().right(px(4.)).bottom(px(4.)).px(px(4.)).rounded(px(3.)).bg(Tokens::scrim()).text_size(px(size::XS)).text_color(Tokens::text_1()).child(v.length.clone()))
                }),
        )
        .child(div().h(px(32.)).overflow_hidden().text_size(px(size::XS)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).line_clamp(2).child(v.title.clone()))
        .child(div().text_size(px(10.)).text_color(Tokens::text_3()).line_clamp(1).child(format!("{} · {} views", v.channel, short_count(v.views))))
        .child(div().flex().gap(px(space::XS)).child(badge(tag, match_tone(v.score))).when(v.reference, |d| d.child(badge("Ref", Tone::Accent))))
}

impl Render for YtPackage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.saved;
        let comparable = s.comparable();
        let summary = if s.researched_at == 0 {
            "Thumbnail prompt, recaps on YouTube, and a title that fits".to_string()
        } else {
            format!("{} recap{} found · checked {}", comparable.len(), if comparable.len() == 1 { "" } else { "s" }, crate::widgets::ago(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s.researched_at)))
        };
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(div().flex_1().min_w(px(180.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(summary))
            .child(
                btn("ytp-research", if s.researched_at == 0 { "Research YouTube" } else { "Research again" }, ButtonKind::Secondary)
                    .small()
                    .icon(ph::MAGNIFYING_GLASS)
                    .loading(self.researching)
                    .tooltip(format!("Find recaps of this manga under all its titles, and compare with {REFERENCE}"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.research(cx))),
            )
            .child(
                btn("ytp-suggest", if self.suggesting { "Thinking…" } else { "Suggest with AI" }, ButtonKind::Primary)
                    .small()
                    .icon(ph::MAGIC_WAND)
                    .loading(self.suggesting)
                    .tooltip("Title, description and thumbnail idea from the model picked at the top, based on what performs best")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.suggest(cx))),
            )
            .child(
                icon_button("ytp-toggle", if self.open { ph::CARET_UP } else { ph::CARET_DOWN }, if self.open { "Hide" } else { "Show" }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.open = !this.open;
                    cx.notify();
                })),
            );

        let mut card = crate::widgets::card().gap(px(space::SM)).child(head);
        if let Some(e) = &self.error {
            card = card.child(div().text_size(px(size::XS)).text_color(Tokens::danger()).child(e.clone()));
        }
        if !self.open {
            return card;
        }
        card.child(self.thumbnail_section(cx)).child(self.suggestion_section(&comparable, cx)).child(self.competitor_section(&comparable, cx))
    }
}

impl YtPackage {
    /// Text clamped to `lines` lines with a Show more / Show less toggle (only when it is long).
    fn clamped(&self, key: &'static str, text: String, lines: usize, cx: &mut Context<Self>) -> Div {
        let open = self.expanded.contains(key);
        let long = is_long(&text, lines);
        let body = div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().when(!open, |d| d.line_clamp(lines)).child(text);
        div().flex().flex_col().gap(px(2.)).child(body).when(long, |d| {
            d.child(
                div().flex().child(
                    btn((SharedString::from(format!("ytp-more-{key}")), 0usize), if open { "Show less" } else { "Show more" }, ButtonKind::Ghost)
                        .small()
                        .icon(if open { ph::CARET_UP } else { ph::CARET_DOWN })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if !this.expanded.remove(key) {
                                this.expanded.insert(key);
                            }
                            cx.notify();
                        })),
                ),
            )
        })
    }

    /// A clamped value with its Copy button on the right.
    fn copy_row(&self, label: &'static str, id: &'static str, what: &'static str, text: String, lines: usize, cx: &mut Context<Self>) -> Div {
        row(label, div().flex().items_start().gap(px(space::SM)).child(div().flex_1().min_w_0().child(self.clamped(id, text.clone(), lines, cx))).child(div().flex_none().child(copy_btn(id, what, text))))
    }

    fn thumbnail_section(&self, cx: &mut Context<Self>) -> Div {
        let prompt = self.prompt();
        let faces: Vec<(String, PathBuf)> = self.cast.iter().filter_map(|(c, f)| f.clone().map(|f| (c.name.clone(), f))).collect();
        let dir = self.faces_dir.clone();
        let avatars = if faces.is_empty() {
            div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("No faces yet. Read a chapter so Studio learns the characters.").into_any_element()
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::SM))
                .children(faces.iter().enumerate().map(|(i, (name, path))| {
                    div()
                        .id(("ytp-face", i))
                        .size(px(48.))
                        .flex_none()
                        .rounded_full()
                        .overflow_hidden()
                        .border_1()
                        .border_color(Tokens::border())
                        .tooltip(tip(format!("{}. {name}", i + 1)))
                        .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                }))
                .child(btn("ytp-faces", "Faces folder", ButtonKind::Ghost).small().icon(ph::FOLDER_OPEN).tooltip("Open the faces folder; attach these images in this order").on_click(move |_, _, cx| cx.reveal_path(&dir)))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(section(ph::IMAGE, "Thumbnail prompt", "for Midjourney, GPT image, Flux…"))
            .child(self.copy_row("Prompt", "ytp-prompt", "thumbnail prompt", prompt, 3, cx))
            .child(row("Attach", avatars))
    }

    fn suggestion_section(&self, comparable: &[Video], cx: &mut Context<Self>) -> Div {
        let s = &self.saved;
        let mut col = div().flex().flex_col().gap(px(space::SM)).child(section(ph::LIGHTBULB, "Suggestions", "from what performs best"));
        if let Some(p) = &s.prediction {
            let tone = match p.confidence.as_str() {
                "high" => Tone::Success,
                "medium" => Tone::Info,
                _ => Tone::Warning,
            };
            let range = if p.high == 0 { "Not enough data".to_string() } else { format!("{} – {} views", short_count(p.low), short_count(p.high)) };
            let head = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::SM))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(range))
                .child(badge(format!("{} confidence", p.confidence), tone))
                .when(comparable.is_empty(), |d| d.child(badge("no direct comparison", Tone::Neutral)));
            let why = p.reasoning.join(" · ");
            col = col.child(row("Forecast", div().flex().flex_col().gap(px(2.)).child(head).when(!why.is_empty(), |d| d.child(self.clamped("ytp-why", why, 2, cx)))));
        }
        if self.suggesting {
            return col
                .child(row("Title", skeleton_rows(&[Some(300.), None, Some(220.)], 12.)))
                .child(row("Description", skeleton_rows(&[None, None, Some(260.)], 10.)))
                .child(row("Thumbnail", skeleton_rows(&[None, Some(200.)], 10.)));
        }
        match &s.suggestion {
            None => col.child(muted(if s.researched_at == 0 { "Research YouTube first for better suggestions, then press Suggest with AI." } else { "Press Suggest with AI for a title, description and thumbnail idea." })),
            Some(g) => {
                let concept = if g.thumbnail_text.is_empty() { g.thumbnail_concept.clone() } else { format!("{}\n\nText on the thumbnail: \"{}\"", g.thumbnail_concept, g.thumbnail_text) };
                col.child(self.copy_row("Title", "ytp-title", "title", g.title.clone(), 2, cx))
                    .when(!g.alt_titles.is_empty(), |d| d.child(row("Alternatives", div().flex().flex_col().gap(px(2.)).children(g.alt_titles.iter().map(|t| div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(1).child(t.clone()))))))
                    .child(self.copy_row("Description", "ytp-desc", "description", g.description.clone(), 3, cx))
                    .child(self.copy_row("Thumbnail", "ytp-concept", "thumbnail concept", concept, 3, cx))
                    .when(!g.notes.is_empty(), |d| d.child(row("Notes", self.clamped("ytp-notes", g.notes.clone(), 2, cx))))
            }
        }
    }

    fn competitor_section(&self, comparable: &[Video], cx: &mut Context<Self>) -> Div {
        let s = &self.saved;
        let mut col = div().flex().flex_col().gap(px(space::SM));
        if s.researched_at == 0 {
            return col.child(section(ph::USERS, "Competing recaps", "")).child(muted(format!("Press Research YouTube to find recaps of {} under all its titles.", self.title)));
        }
        col = col.child(section(ph::USERS, "Competing recaps", format!("searched as: {}", s.names.join(" · "))));
        let weaker: Vec<&Video> = s.videos.iter().filter(|v| !comparable.iter().any(|c| c.id == v.id)).collect();
        if comparable.is_empty() {
            col = col.child(muted("No recap of this manga was found on YouTube. Being first is an opening."));
        } else {
            col = col.child(div().flex().flex_wrap().gap(px(space::SM)).children(comparable.iter().take(12).enumerate().map(|(i, v)| video_card(("ytp-comp", i), v))));
        }
        if !weaker.is_empty() {
            col = col.child(
                div().flex().child(
                    btn("ytp-more", if self.show_all { "Hide other results".to_string() } else { format!("Show {} other result{} (likely other manga)", weaker.len(), if weaker.len() == 1 { "" } else { "s" }) }, ButtonKind::Ghost)
                        .small()
                        .icon(if self.show_all { ph::CARET_UP } else { ph::CARET_DOWN })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.show_all = !this.show_all;
                            cx.notify();
                        })),
                ),
            );
            if self.show_all {
                col = col.child(div().flex().flex_wrap().gap(px(space::SM)).children(weaker.iter().take(30).enumerate().map(|(i, v)| video_card(("ytp-other", i), v))));
            }
        }
        if !s.reference_recent.is_empty() {
            let mut views: Vec<u64> = s.reference_recent.iter().map(|v| v.views).collect();
            views.sort_unstable();
            let median = views[views.len() / 2];
            let mut top: Vec<&Video> = s.reference_recent.iter().collect();
            top.sort_by_key(|v| std::cmp::Reverse(v.views));
            col = col
                .child(section(
                    ph::CHART_LINE_UP,
                    &format!("Reference: {}", s.reference_name),
                    format!("{}{}median {} views over its last {} uploads", s.reference_subs, if s.reference_subs.is_empty() { "" } else { " · " }, short_count(median), views.len()),
                ))
                .child(div().flex().flex_wrap().gap(px(space::SM)).children(top.iter().take(4).enumerate().map(|(i, v)| video_card(("ytp-ref", i), v))));
        }
        col
    }
}

#[cfg(test)]
mod tests {
    /// Hits YouTube and AniList: `cargo test -p ui-studio live_research -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_research() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let s = rt.block_on(super::research("The Sword-Eating Swordmaster".into(), vec!["Solo Leveling".into()], vec!["Arhan Karavan".into(), "Arhan".into()])).unwrap();
        println!("names: {:?}\nreference: {} {} ({} recent)", s.names, s.reference_name, s.reference_subs, s.reference_recent.len());
        for v in &s.videos {
            println!("{:>4.0}% {:<30} {:>9} {:<10} {} | {}", v.score * 100., v.covers.chars().take(30).collect::<String>(), v.views, v.published, v.channel, v.title);
        }
        println!("{:#?}", s.prediction);
        assert!(s.names.len() > 1 && !s.comparable().is_empty() && !s.reference_recent.is_empty());
    }
}

#[cfg(test)]
mod prompt_check {
    /// Prints the thumbnail prompt for a real brain: `MV_BRAIN_DIR=<studio/<manga>> cargo test -p ui-studio print_prompt -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_prompt() {
        let dir = std::path::PathBuf::from(std::env::var("MV_BRAIN_DIR").unwrap());
        let brain = mv_recap::Brain::load_in(dir, "m", "The Sword-Eating Swordmaster");
        let cast = super::logic::lead_cast(&brain, 3);
        let refs: Vec<bool> = cast.iter().map(|(_, f)| f.is_some()).collect();
        let c: Vec<super::Cast> = cast.iter().map(|(c, _)| c.clone()).collect();
        println!("{}", super::logic::thumbnail_prompt(&brain.manga_title, &c, &refs, &super::logic::story_so_far(&brain.notes, 4), "HE EATS SWORDS"));
        for (c, f) in &cast {
            println!("face for {}: {:?}", c.name, f);
        }
    }
}
