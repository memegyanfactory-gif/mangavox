//! One manga's thumbnail generator (the Thumbnails page shows it), laid out like Magnific's
//! Image Generator: model, character references (the manga's face cards, called with `@Name` in
//! the prompt), prompt, size and Generate on the left; the creations and the AI's ideas on the
//! right. Every image is saved in `studio/<manga>/thumbnails/` and can be re-edited.
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};

use mv_ui::{appear, glyphs, icon_ph, popup_surface, progress_bar, progress_ring_sized, spinner, Route};

use super::click::{self, Score};
use super::magnific::{self, ImageModel, Usage, CATALOG};
use super::thumbs::{self, Concept, Gallery, Thumb};
use super::*;

mod rules;
pub(super) use rules::guide as rules_guide;

const LEFT_W: f32 = 272.;
const TILE_W: f32 = 248.;
const ASPECTS: [&str; 4] = ["16:9", "9:16", "1:1", "4:3"];
/// The characters offered as references (the API takes up to 14 images).
pub(super) const CAST_FOR_REFS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Creations,
    Ideas,
    /// What the click checklist learned, and each candidate's score.
    Rules,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Model,
    Aspect,
    Resolution,
    Usage,
}

/// A generation in flight.
pub(super) struct Running {
    id: String,
    label: String,
    aspect: &'static str,
    status: Arc<Mutex<String>>,
    started: Instant,
}

/// One manga's generator: what the AI knows about it, and the generator's state.
pub(crate) struct ThumbGen {
    manga_id: String,
    title: String,
    story: Vec<String>,
    /// The YouTube research, for what performs.
    saved: Saved,
    th: ThumbState,
}

impl ThumbGen {
    pub(crate) fn new(g: &MangaGroup) -> Self {
        let brain = crate::run::brain_snapshot(&g.id, &g.title);
        let cast = logic::lead_cast(&brain, CAST_FOR_REFS);
        ThumbGen { manga_id: g.id.clone(), title: g.title.clone(), story: logic::story_so_far(&brain.notes, 6), saved: Saved::load(&g.id), th: ThumbState::new(&g.id, cast) }
    }

    /// The key or the settings changed elsewhere (Settings › Magnific).
    pub(crate) fn refresh_settings(&mut self) {
        self.th.has_key = magnific::key().is_some();
        self.th.usage = Usage::load();
        let prefs = magnific::Prefs::load();
        if let Some(i) = CATALOG.iter().position(|m| m.id == prefs.model) {
            self.th.model = i;
        }
        if self.th.has_key && self.th.key_rejected && magnific::key().as_deref() != self.th.rejected_key.as_deref() {
            self.th.key_rejected = false;
            self.th.error = None;
        }
    }
}

impl Render for ThumbGen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.thumbnail_generator(window, cx)
    }
}

pub(super) struct ThumbState {
    gallery: Gallery,
    /// Magnific answered 401 for this key.
    key_rejected: bool,
    rejected_key: Option<String>,
    improving: bool,
    /// Characters with a face card, most seen first.
    cast: Vec<(Cast, PathBuf)>,
    /// Characters switched on as references (by cast index), besides the `@` mentions.
    picked: Vec<usize>,
    /// Images the user added as references.
    extra: Vec<PathBuf>,
    model: usize,
    aspect: &'static str,
    res: &'static str,
    count: usize,
    tab: Tab,
    menu: Option<Menu>,
    jobs: Vec<Running>,
    designing: bool,
    /// The thumbnail shown large, with its edit box.
    open: Option<String>,
    /// Click scores of the made thumbnails (prompt + pixels), by id.
    scores: HashMap<String, Score>,
    scoring: bool,
    /// Creations by score instead of newest first.
    best_first: bool,
    has_key: bool,
    usage: Usage,
    error: Option<String>,
    prompt: Option<Entity<TextareaState>>,
    key: Option<Entity<InputState>>,
    edit: Option<Entity<InputState>>,
    search: Option<Entity<InputState>>,
    budget: Option<Entity<InputState>>,
    _subs: Vec<Subscription>,
}

impl ThumbState {
    pub(super) fn new(manga_id: &str, cast: Vec<(Cast, Option<PathBuf>)>) -> Self {
        let cast: Vec<(Cast, PathBuf)> = cast.into_iter().filter_map(|(c, f)| Some((c, f?))).collect();
        let prefs = magnific::Prefs::load();
        ThumbState {
            gallery: Gallery::load(manga_id),
            key_rejected: false,
            rejected_key: None,
            improving: false,
            picked: if cast.is_empty() { vec![] } else { vec![0] },
            cast,
            extra: vec![],
            model: CATALOG.iter().position(|m| m.id == prefs.model).unwrap_or(0),
            aspect: ASPECTS.iter().copied().find(|a| *a == prefs.aspect).unwrap_or("16:9"),
            res: magnific::RESOLUTIONS.iter().copied().find(|r| *r == prefs.resolution).unwrap_or("1K"),
            count: 1,
            tab: Tab::Creations,
            menu: None,
            jobs: vec![],
            designing: false,
            open: None,
            scores: HashMap::new(),
            scoring: false,
            best_first: false,
            has_key: magnific::key().is_some(),
            usage: Usage::load(),
            error: None,
            prompt: None,
            key: None,
            edit: None,
            search: None,
            budget: None,
            _subs: vec![],
        }
    }

    fn model(&self) -> &'static ImageModel {
        &CATALOG[self.model.min(CATALOG.len() - 1)]
    }

    fn save_prefs(&self) {
        let mut p = magnific::Prefs::load();
        p.model = self.model().id.to_string();
        p.aspect = self.aspect.to_string();
        p.resolution = self.res.to_string();
        p.save();
    }
}

fn now_id() -> String {
    format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0))
}

fn label_caps(text: &str) -> Div {
    div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(text.to_uppercase())
}

/// A small grey chip: "Refs", "~42s", "2K-4K".
fn spec(text: impl Into<SharedString>) -> Div {
    div().flex_none().px(px(6.)).h(px(20.)).flex().items_center().rounded(px(radius::SM)).bg(Tokens::surface_3()).text_size(px(size::XS)).text_color(Tokens::text_2()).child(text.into())
}

/// The price side of a model row: "◈ 50 - 145" (credits per image), and "∞ Web · 1K" when the
/// web plan makes it unlimited (the API still charges credits).
fn price(m: &ImageModel) -> Div {
    let d = div().flex().flex_none().items_center().justify_end().gap(px(8.)).w(px(150.)).text_size(px(size::XS)).text_color(Tokens::text_2());
    let d = match m.web_unlimited {
        Some(res) => d.child(div().id(SharedString::from(format!("thg-unl-{}", m.id))).flex().items_center().gap(px(3.)).tooltip(tip(format!("Unlimited at {res} in the Magnific web app on your plan. API calls are always billed in credits."))).child(icon_ph(ph::INFINITY, Tokens::info(), icon_px::XS)).child(div().text_color(Tokens::info()).child(format!("Web · {res}")))),
        None => d,
    };
    match m.credits {
        Some((lo, hi)) => d.child(div().flex().items_center().gap(px(3.)).child(icon_ph(ph::COINS, Tokens::text_2(), icon_px::XS)).child(if lo == hi { format!("{lo}") } else { format!("{lo} - {hi}") })),
        None => d.child(div().text_color(Tokens::text_3()).child("credits")),
    }
}

/// A square tile in the References row (Style / Character / Add in Magnific).
fn ref_tile(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .size(px(58.))
        .rounded(px(radius::LG))
        .border_1()
        .border_color(Tokens::border())
        .bg(Tokens::surface_2())
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
}

impl ThumbGen {
    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = &mut self.th;
        if t.prompt.is_some() {
            return;
        }
        let prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 10).placeholder("Describe it, @Name for characters"));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Paste your Magnific API key").masked(true));
        let edit = cx.new(|cx| InputState::new(window, cx).placeholder("Describe a change, e.g. make the sword glow red, add @Name behind"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let budget = cx.new(|cx| InputState::new(window, cx).placeholder("Credits on your plan"));
        let subs = vec![
            cx.subscribe_in(&key, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save_key(window, cx);
                }
            }),
            cx.subscribe_in(&edit, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_edit(window, cx);
                }
            }),
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(&budget, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let v: String = input.read(cx).value().chars().filter(|c| c.is_ascii_digit()).collect();
                    this.th.usage.budget = v.parse().ok();
                    this.th.usage.save();
                    cx.notify();
                }
            }),
        ];
        let t = &mut self.th;
        if let Some(b) = t.usage.budget {
            budget.update(cx, |s, cx| s.set_value(b.to_string(), window, cx));
        }
        t.prompt = Some(prompt);
        t.key = Some(key);
        t.edit = Some(edit);
        t.search = Some(search);
        t.budget = Some(budget);
        t._subs = subs;
    }

    fn prompt_text(&self, cx: &App) -> String {
        self.th.prompt.as_ref().map(|p| p.read(cx).value().to_string()).unwrap_or_default()
    }

    fn set_prompt(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.th.prompt {
            p.update(cx, |s, cx| {
                s.set_value(text, window, cx);
                s.focus(window, cx);
            });
        }
        self.th.tab = Tab::Creations;
        cx.notify();
    }

    fn insert_mention(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((c, _)) = self.th.cast.get(i) else { return };
        let mut text = self.prompt_text(cx);
        if !text.is_empty() && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(&format!("@{} ", thumbs::handle(&c.name)));
        self.set_prompt(text, window, cx);
    }

    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.th.key.clone() else { return };
        let k = input.read(cx).value().trim().to_string();
        if k.is_empty() {
            return;
        }
        if let Err(e) = magnific::set_key(Some(&k)) {
            self.th.error = Some(format!("Could not save the key: {e:#}"));
            cx.notify();
            return;
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        self.th.has_key = true;
        self.th.key_rejected = false;
        self.th.error = None;
        // Check it right away, so a wrong key shows here and not on the first Generate.
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { magnific::verify(&k).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => toast::show(cx, Toast::success("Magnific connected").icon(ph::PLUGS_CONNECTED)),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.starts_with(magnific::KEY_REJECTED) {
                            this.th.key_rejected = true;
                            this.th.rejected_key = magnific::key();
                        }
                        this.th.error = Some(msg);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Rewrites the prompt box with the AI model picked at the top: concrete, true to the manga,
    /// built on what wins clicks, with the `@` characters kept.
    fn improve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.prompt_text(cx);
        if self.th.improving {
            return;
        }
        let (provider, model) = ui_assistant::assistant_model(cx);
        let Ok(provider) = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)) else {
            self.th.error = Some("Pick an AI model first (model button at the top).".into());
            cx.notify();
            return;
        };
        let reg = ui_assistant::registry(cx);
        let cast: Vec<Cast> = self.th.cast.iter().map(|(c, _)| c.clone()).collect();
        let comparable = self.saved.comparable();
        let hook = self.saved.suggestion.as_ref().map(|s| s.thumbnail_text.clone()).unwrap_or_default();
        let (system, user) = thumbs::improve_prompt(&thumbs::Brief { title: &self.title, cast: &cast, story: &self.story, comparable: &comparable, reference: &self.saved.reference_recent, hook: &hook }, &draft);
        self.th.improving = true;
        self.th.error = None;

        cx.spawn_in(window, async move |this, cx| {
            let res = mv_ui::bg::run(ask_text(reg, provider, model, system, user)).await.and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.th.improving = false;
                match res {
                    Ok(p) => this.set_prompt(p, window, cx),
                    Err(e) => this.th.error = Some(format!("The AI could not write the prompt: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Generates from the prompt box (`count` images).
    fn generate_from_prompt(&mut self, cx: &mut Context<Self>) {
        let prompt = self.prompt_text(cx);
        if prompt.trim().is_empty() {
            self.th.error = Some("Write a prompt first, or use one of the AI's ideas.".into());
            cx.notify();
            return;
        }
        for _ in 0..self.th.count {
            self.start(prompt.clone(), "Custom".into(), None, cx);
        }
    }

    /// Generates `prompt` (its `@` mentions attach the face cards); `base` is the thumbnail being
    /// re-edited, sent as Image 1.
    fn start(&mut self, prompt: String, label: String, base: Option<String>, cx: &mut Context<Self>) {
        let Some(key) = magnific::key() else {
            self.th.error = Some("Connect Magnific first: paste your API key on the left.".into());
            cx.notify();
            return;
        };
        let t = &mut self.th;
        let mut model = t.model();
        if (base.is_some() || !t.picked.is_empty() || prompt.contains('@')) && model.refs == 0 {
            // References need a model that takes them; Nano Banana 2.1 does.
            model = &CATALOG[0];
        }
        if !model.takes_aspect() && t.aspect != "1:1" {
            // A square-only model cannot make a 16:9 thumbnail; Nano Banana 2.1 takes the size.
            model = &CATALOG[0];
        }
        let first = usize::from(base.is_some());
        let just_cast: Vec<Cast> = t.cast.iter().map(|(c, _)| c.clone()).collect();
        let (resolved, mut order) = thumbs::resolve(&prompt, &just_cast, first);
        for i in &t.picked {
            if !order.contains(i) {
                order.push(*i);
            }
        }
        let mut refs: Vec<magnific::Reference> = vec![];
        if let Some(b) = &base {
            refs.push(magnific::Reference { file: Gallery::image(&self.manga_id, b), caption: "Image 1: the thumbnail to edit".into() });
        }
        let mut looks = vec![];
        for i in order.iter().take(model.refs.saturating_sub(refs.len())) {
            let (c, f) = &t.cast[*i];
            refs.push(magnific::Reference { file: f.clone(), caption: format!("Image {}: {}", refs.len() + 1, c.name) });
            looks.push((c.name.clone(), thumbs::look_of(c)));
        }
        for f in &t.extra {
            if refs.len() < model.refs {
                refs.push(magnific::Reference { file: f.clone(), caption: format!("Image {}", refs.len() + 1) });
            }
        }
        let full = if base.is_some() {
            format!("Edit Image 1, a YouTube thumbnail. Keep its composition, characters and style; change only this: {}\n{}", resolved.trim(), thumbs::image_prompt(&self.title, "", &looks, first).split_once("\n\n").map(|(_, r)| r).unwrap_or(""))
        } else {
            thumbs::image_prompt(&self.title, &resolved, &looks, first)
        };
        let id = now_id() + &format!("{}", t.jobs.len());
        let status = Arc::new(Mutex::new("Queued".to_string()));
        let aspect = if base.is_some() && model.auto_aspect { "auto" } else { t.aspect };
        let res = if model.resolutions.contains(&t.res) { t.res } else { model.resolutions.first().copied().unwrap_or("Auto") };
        t.jobs.insert(0, Running { id: id.clone(), label: label.clone(), aspect: if aspect == "auto" { "16:9" } else { aspect }, status: status.clone(), started: Instant::now() });
        t.tab = Tab::Creations;
        t.error = None;
        let out = Gallery::image(&self.manga_id, &id);
        let thumb = Thumb { id: id.clone(), prompt, label, parent: base, model: model.name.to_string(), created: now(), resolution: res.to_string(), cost: model.cost_at(res) };
        let st = status.clone();
        let used_key = key.clone();
        let job = async move { magnific::generate(model, &key, &full, &refs, aspect, res, &out, move |s| *st.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string()).await };
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(job).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.th.jobs.retain(|j| j.id != id);
                match res {
                    Ok(()) => {
                        this.th.usage.add(thumb.cost);
                        this.th.gallery.thumbs.insert(0, thumb);
                        this.th.gallery.save(&this.manga_id);
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.starts_with(magnific::KEY_REJECTED) {
                            this.th.key_rejected = true;
                            this.th.rejected_key = Some(used_key.clone());
                        }
                        this.th.error = Some(msg);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        self.tick(cx);
        cx.notify();
    }

    /// Repaints while generations run, for their status and timers.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.th.jobs.len() > 1 {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            let alive = this.update(cx, |this, cx| {
                cx.notify();
                !this.th.jobs.is_empty()
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn apply_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(base), Some(input)) = (self.th.open.clone(), self.th.edit.clone()) else { return };
        let change = input.read(cx).value().trim().to_string();
        if change.is_empty() {
            return;
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        self.th.open = None;
        self.start(change, "Edit".into(), Some(base), cx);
    }

    fn delete_thumb(&mut self, id: &str, cx: &mut Context<Self>) {
        let _ = std::fs::remove_file(Gallery::image(&self.manga_id, id));
        self.th.gallery.thumbs.retain(|t| t.id != id);
        self.th.gallery.save(&self.manga_id);
        if self.th.open.as_deref() == Some(id) {
            self.th.open = None;
        }
        cx.notify();
    }

    fn add_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Use as reference".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.th.extra.extend(paths.into_iter().filter(|p| p.is_file()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Has the picked AI model study the manga and write thumbnail concepts.
    fn design(&mut self, cx: &mut Context<Self>) {
        if self.th.designing {
            return;
        }
        let (provider, model) = ui_assistant::assistant_model(cx);
        let Ok(provider) = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)) else {
            self.th.error = Some("Pick an AI model first (model button at the top).".into());
            cx.notify();
            return;
        };
        let reg = ui_assistant::registry(cx);
        let cast: Vec<Cast> = self.th.cast.iter().map(|(c, _)| c.clone()).collect();

        let comparable = self.saved.comparable();
        let hook = self.saved.suggestion.as_ref().map(|s| s.thumbnail_text.clone()).unwrap_or_default();
        let (system, user) = thumbs::concepts_prompt(&thumbs::Brief { title: &self.title, cast: &cast, story: &self.story, comparable: &comparable, reference: &self.saved.reference_recent, hook: &hook });
        self.th.designing = true;
        self.th.tab = Tab::Ideas;
        self.th.error = None;
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(ask_concepts(reg, provider, model, system, user)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.th.designing = false;
                match res {
                    Ok((analysis, mut concepts)) => {
                        // Best click score first, so "Use the AI's best idea" takes the top one.
                        let m = click::model();
                        concepts.sort_by_key(|c| std::cmp::Reverse(m.score_idea(&c.prompt, &c.text).value));
                        this.th.gallery.analysis = analysis;
                        this.th.gallery.concepts = concepts;
                        this.th.gallery.save(&this.manga_id);
                    }
                    Err(e) => this.th.error = Some(format!("The AI could not design thumbnails: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Scores the made thumbnails that have no score yet, off the UI thread (it reads the images).
    fn ensure_scores(&mut self, cx: &mut Context<Self>) {
        if self.th.scoring {
            return;
        }
        let todo: Vec<(String, String, PathBuf)> = self.th.gallery.thumbs.iter().filter(|t| !self.th.scores.contains_key(&t.id)).map(|t| (t.id.clone(), t.prompt.clone(), Gallery::image(&self.manga_id, &t.id))).collect();
        if todo.is_empty() {
            return;
        }
        self.th.scoring = true;
        cx.spawn(async move |this, cx| {
            let scored = mv_ui::bg::blocking(move || todo.into_iter().map(|(id, prompt, path)| (id, click::model().score_image(&prompt, &path))).collect::<Vec<_>>()).await;
            let _ = this.update(cx, |this, cx| {
                this.th.scoring = false;
                if let Ok(scored) = scored {
                    this.th.scores.extend(scored);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_menu(&mut self, m: Menu, cx: &mut Context<Self>) {
        self.th.menu = if self.th.menu == Some(m) { None } else { Some(m) };
        cx.notify();
    }

    // ------------------------------------------------------------------ view

    fn thumbnail_generator(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.ensure_inputs(window, cx);
        self.ensure_scores(cx);
        let shell = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(space::MD))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::canvas())
            .border_1()
            .border_color(Tokens::border())
            .child(self.left_panel(window, cx))
            .child(self.right_panel(cx));
        let error = self.th.error.clone().map(|e| {
            div()
                .flex()
                .items_start()
                .gap(px(space::SM))
                .p(px(space::MD))
                .rounded(px(radius::LG))
                .bg(Tokens::danger_soft())
                .border_1()
                .border_color(Tokens::danger_border())
                .child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM))
                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).whitespace_normal().child(e))
                .when(self.th.key_rejected, |d| d.child(settings_btn("thg-err-settings", "Change key")))
                .child(icon_button("thg-err-close", ph::X, "Dismiss").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.th.error = None;
                    cx.notify();
                })))
        });
        div().flex().flex_col().gap(px(space::MD)).children(error).child(shell)
    }

    fn left_panel(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let panel = div().flex().flex_col().flex_none().w(px(LEFT_W)).gap(px(space::LG)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::sidebar());
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(2.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Studio · Magnific"))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Thumbnail Generator")),
            )
            .child(self.usage_button(cx));
        if !self.th.has_key || self.th.key_rejected {
            return panel.child(head).child(self.connect_card(cx));
        }
        panel.child(head).child(self.model_field(cx)).child(self.references(cx)).child(self.prompt_field(window, cx)).child(self.size_row(cx)).child(self.generate_block(cx))
    }

    fn connect_card(&self, cx: &mut Context<Self>) -> Div {
        let key = self.th.key.clone().expect("inputs");
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(label_caps(if self.th.key_rejected { "Key rejected" } else { "Connect Magnific" }))
            .child(div().text_size(px(size::XS)).text_color(if self.th.key_rejected { Tokens::danger() } else { Tokens::text_2() }).whitespace_normal().child(if self.th.key_rejected {
                "Magnific did not accept the saved key. Paste a valid one from magnific.com → Developers → API key."
            } else {
                "Paste the API key from magnific.com → Developers → API key. It is kept on this computer only (Studio's data folder), never in the project."
            }))
            .child(Input::new(&key).mask_toggle())
            .child(btn("thg-connect", "Connect", ButtonKind::Primary).icon(ph::PLUGS_CONNECTED).full_width().on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.save_key(window, cx))))
            .child(div().flex().flex_wrap().gap(px(space::XS)).child(settings_btn("thg-open-settings", "Magnific settings")).child(btn("thg-getkey", "Get a key", ButtonKind::Ghost).small().icon(ph::ARROW_SQUARE_OUT).on_click(|_, _, cx| cx.open_url("https://www.magnific.com/developers/dashboard/api-key"))))
    }

    fn model_field(&self, cx: &mut Context<Self>) -> Div {
        let m = self.th.model();
        let open = self.th.menu == Some(Menu::Model);
        let button = div()
            .id("thg-model")
            .flex()
            .flex_1()
            .items_center()
            .gap(px(space::SM))
            .h(px(36.))
            .px(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(if open { Tokens::border_strong() } else { Tokens::surface_2() })
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::hover()))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Model, cx)))
            .child(icon_ph(m.icon(), Tokens::text_1(), icon_px::SM))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(m.name))
            .child(icon_ph(ph::CARET_DOWN, Tokens::text_2(), icon_px::XS));
        let row = div().relative().flex().items_center().gap(px(space::XS)).child(button).child(
            icon_button("thg-key", ph::GEAR_SIX, "Magnific settings: key, default model, credits").on_click(|_, window, cx| open_settings(window, cx)),
        );
        let row = if open { row.child(self.model_menu(cx)) } else { row };
        let square_only = !m.takes_aspect() && self.th.aspect != "1:1";
        div().flex().flex_col().gap(px(space::SM)).child(label_caps("Model")).child(row).when(square_only, |d| {
            d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).whitespace_normal().child(format!("{} only makes square images, so {} thumbnails are made with {} ({}).", m.name, self.th.aspect, CATALOG[0].name, CATALOG[0].cost_label(self.th.res).trim_start_matches(" · "))))
        })
    }

    fn model_row(&self, i: usize, m: &'static ImageModel, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = i == self.th.model;
        div()
            .id(("thg-model-row", i))
            .flex()
            .items_center()
            .gap(px(space::SM))
            .h(px(32.))
            .px(px(space::SM))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .when(current, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.th.model = i;
                if !m.resolutions.contains(&this.th.res) {
                    this.th.res = m.resolutions.first().copied().unwrap_or("1K");
                }
                this.th.menu = None;
                this.th.save_prefs();
                cx.notify();
            }))
            .child(div().w(px(240.)).flex().items_center().gap(px(6.)).child(icon_ph(m.icon(), Tokens::text_2(), icon_px::XS)).child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(m.name)).when(m.new, |d| d.child(badge("New", Tone::Accent))))
            .child(div().flex().flex_1().gap(px(4.)).when(m.refs > 0, |d| d.child(spec("Refs"))).child(spec(m.speed)).child(spec(if m.takes_aspect() { m.res_label() } else { "1:1 only".into() })))
            .child(price(m))
    }

    fn model_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let q = self.th.search.as_ref().map(|s| s.read(cx).value().to_lowercase()).unwrap_or_default();
        let matches = |m: &ImageModel| q.is_empty() || m.name.to_lowercase().contains(&q) || m.provider.to_lowercase().contains(&q);
        let mut list = div().id("thg-model-list").flex().flex_col().gap(px(2.)).max_h(px(420.)).overflow_y_scroll();
        let featured: Vec<usize> = (0..CATALOG.len()).filter(|i| CATALOG[*i].featured && matches(&CATALOG[*i])).collect();
        if !featured.is_empty() && q.is_empty() {
            list = list.child(div().pt(px(6.)).px(px(space::SM)).child(label_caps("Featured")));
            for i in featured {
                list = list.child(self.model_row(i, &CATALOG[i], cx));
            }
            list = list.child(div().pt(px(6.)).px(px(space::SM)).child(label_caps("All models")));
        }
        let mut providers: Vec<&str> = vec![];
        for m in CATALOG {
            if !providers.contains(&m.provider) {
                providers.push(m.provider);
            }
        }
        for p in providers {
            let rows: Vec<usize> = (0..CATALOG.len()).filter(|i| CATALOG[*i].provider == p && matches(&CATALOG[*i])).collect();
            if rows.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .px(px(space::SM))
                    .mt(px(2.))
                    .rounded(px(radius::MD))
                    .bg(Tokens::surface_2())
                    .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(p))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} model{}", rows.len(), if rows.len() == 1 { "" } else { "s" }))),
            );
            for i in rows {
                list = list.child(self.model_row(i, &CATALOG[i], cx));
            }
        }
        let search = self.th.search.clone().expect("inputs");
        let pop = popup_surface()
            .w(px(660.))
            .p(px(space::MD))
            .gap(px(space::SM))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.th.menu = None;
                cx.notify();
            }))
            .child(Input::new(&search).prefix(icon_ph(glyphs::SEARCH, Tokens::text_3(), icon_px::SM)))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Only models the Magnific API offers. Prices are credits per image (low to high resolution). ∞ Web marks what your plan makes unlimited in the Magnific web app; API calls are always billed in credits."))
            .child(list);
        div().absolute().top_full().left_0().child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear("thg-model-pop", pop))).with_priority(3))
    }

    fn references(&self, cx: &mut Context<Self>) -> Div {
        let m = self.th.model();
        let used = self.th.picked.len() + self.th.extra.len();
        let mut grid = div().flex().flex_wrap().gap(px(space::SM));
        for (i, (c, face)) in self.th.cast.iter().enumerate() {
            let on = self.th.picked.contains(&i);
            let handle = thumbs::handle(&c.name);
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .w(px(58.))
                    .child(
                        div()
                            .id(("thg-face", i))
                            .relative()
                            .size(px(58.))
                            .rounded(px(radius::LG))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if on { Tokens::accent() } else { Tokens::border() })
                            .cursor_pointer()
                            .tooltip(tip(format!("{} · click to {} as a reference, or write @{handle} in the prompt", c.name, if on { "stop using" } else { "use" })))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if !this.th.picked.contains(&i) {
                                    this.th.picked.push(i);
                                } else {
                                    this.th.picked.retain(|p| *p != i);
                                }
                                cx.notify();
                            }))
                            .child(img(face.clone()).size_full().object_fit(ObjectFit::Cover))
                            .when(on, |d| d.child(div().absolute().top(px(3.)).right(px(3.)).size(px(16.)).rounded_full().bg(Tokens::accent()).flex().items_center().justify_center().child(icon_ph(ph::CHECK, Tokens::on_accent(), 10.)))),
                    )
                    .child(
                        div()
                            .id(("thg-at", i))
                            .truncate()
                            .text_size(px(10.))
                            .text_color(Tokens::text_2())
                            .cursor_pointer()
                            .hover(|s| s.text_color(Tokens::accent()))
                            .tooltip(tip(format!("Insert @{handle} in the prompt")))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.insert_mention(i, window, cx)))
                            .child(format!("@{handle}")),
                    ),
            );
        }
        for (k, f) in self.th.extra.iter().enumerate() {
            grid = grid.child(
                div().flex().flex_col().gap(px(3.)).w(px(58.)).child(
                    div()
                        .id(("thg-extra", k))
                        .size(px(58.))
                        .rounded(px(radius::LG))
                        .overflow_hidden()
                        .border_2()
                        .border_color(Tokens::accent())
                        .cursor_pointer()
                        .tooltip(tip("Click to remove"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if k < this.th.extra.len() {
                                this.th.extra.remove(k);
                            }
                            cx.notify();
                        }))
                        .child(img(f.clone()).size_full().object_fit(ObjectFit::Cover)),
                ),
            );
        }
        grid = grid.child(
            div().flex().flex_col().w(px(58.)).child(
                ref_tile("thg-add")
                    .tooltip(tip("Add your own reference image"))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.add_reference(window, cx)))
                    .child(icon_ph(ph::PLUS, Tokens::text_1(), icon_px::SM))
                    .child(div().text_size(px(10.)).text_color(Tokens::text_2()).child("Add")),
            ),
        );
        let head = div().flex().items_center().child(div().flex_1().child(label_caps("References"))).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{used}/{}", m.refs.max(1))));
        let body = if self.th.cast.is_empty() { div().flex().flex_col().gap(px(space::SM)).child(muted("No face cards yet. Read a chapter so Studio learns the characters.")).child(grid) } else { grid };
        div().flex().flex_col().gap(px(space::SM)).child(head).child(body).when(m.refs == 0, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child(format!("{} takes no references; Nano Banana 2.1 is used when you attach characters.", m.name))))
    }

    fn prompt_field(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let prompt = self.th.prompt.clone().expect("inputs");
        let focused = prompt.read(cx).focus_handle(cx).contains_focused(window, cx);
        div().flex().flex_col().gap(px(space::SM)).child(label_caps("Prompt")).child(
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .p(px(space::MD))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(if focused { Tokens::text_1() } else { Tokens::border() })
                .bg(Tokens::surface())
                .child(Textarea::new(&prompt).appearance(false).bordered(false))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::XS))
                        .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} chars", self.prompt_text(cx).chars().count())))
                        .child(if self.th.improving {
                            spinner("thg-improving", Tokens::accent(), icon_px::SM).into_any_element()
                        } else {
                            icon_button("thg-improve", ph::MAGIC_WAND, if self.prompt_text(cx).trim().is_empty() { "Write a prompt with AI from the manga" } else { "Improve with AI: sharper, truer to the manga, built for the click" })
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.improve(window, cx)))
                                .into_any_element()
                        })
                        .child(icon_button("thg-best-idea", ph::LIGHTBULB, "Use the AI's best idea").on_click(cx.listener(|this, _: &ClickEvent, window, cx| match this.th.gallery.concepts.first().cloned() {
                            Some(c) => this.set_prompt(c.prompt, window, cx),
                            None => this.design(cx),
                        })))
                        .child(icon_button("thg-clear", ph::X, "Clear the prompt").on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_prompt(String::new(), window, cx)))),
                ),
        )
    }

    fn size_row(&self, cx: &mut Context<Self>) -> Div {
        let m = self.th.model();
        let unlimited = m.web_unlimited_at(self.th.res);
        let pill = |id: &'static str| {
            div().id(id).flex().items_center().gap(px(6.)).h(px(30.)).px(px(10.)).rounded(px(radius::MD)).bg(Tokens::surface_2()).cursor_pointer().hover(|s| s.bg(Tokens::hover())).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1())
        };
        let counter = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .px(px(4.))
            .rounded(px(radius::MD))
            .bg(Tokens::surface_2())
            .child(icon_button("thg-less", ph::MINUS, "Fewer").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.th.count = this.th.count.saturating_sub(1).max(1);
                cx.notify();
            })))
            .child(div().w(px(14.)).text_center().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.th.count.to_string()))
            .child(icon_button("thg-more", ph::PLUS, "More (up to 4)").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.th.count = (this.th.count + 1).min(4);
                cx.notify();
            })));
        let aspect = div().relative().child(pill("thg-aspect").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Aspect, cx))).child(icon_ph(ph::CROP, Tokens::text_2(), icon_px::XS)).child(self.th.aspect)).when(self.th.menu == Some(Menu::Aspect), |d| {
            d.child(self.small_menu("thg-aspect-pop", ASPECTS.iter().map(|a| (*a, a.to_string(), false)).collect(), self.th.aspect, cx, |this, v| this.th.aspect = v))
        });
        let res_text = if m.resolutions.is_empty() { "Auto" } else { self.th.res };
        let res = div()
            .relative()
            .child(
                pill("thg-res")
                    .tooltip(tip(if m.resolutions.is_empty() { format!("{} picks its own size", m.name) } else { "Resolution; ∞ marks the size your plan makes unlimited in the Magnific web app".to_string() }))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if !this.th.model().resolutions.is_empty() {
                            this.set_menu(Menu::Resolution, cx);
                        }
                    }))
                    .child(icon_ph(ph::FRAME_CORNERS, Tokens::text_2(), icon_px::XS))
                    .child(res_text)
                    .when(unlimited, |d| d.child(icon_ph(ph::INFINITY, Tokens::info(), icon_px::XS))),
            )
            .when(self.th.menu == Some(Menu::Resolution), |d| {
                let items = m.resolutions.iter().map(|r| (*r, format!("{r}{}", m.cost_label(r)), m.web_unlimited_at(r))).collect();
                d.child(self.small_menu("thg-res-pop", items, self.th.res, cx, |this, v| this.th.res = v))
            });
        div().flex().flex_wrap().items_center().gap(px(space::XS)).child(counter).child(aspect).child(res)
    }

    /// A small option list under a pill; `items` are (value, label, unlimited).
    fn small_menu(&self, id: &'static str, items: Vec<(&'static str, String, bool)>, current: &'static str, cx: &mut Context<Self>, set: fn(&mut ThumbGen, &'static str)) -> impl IntoElement {
        let mut pop = popup_surface().w(px(180.)).gap(px(2.)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.th.menu = None;
            cx.notify();
        }));
        for (k, (v, label, unl)) in items.into_iter().enumerate() {
            pop = pop.child(
                div()
                    .id((id, k))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .h(px(30.))
                    .px(px(space::SM))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .when(v == current, |d| d.bg(Tokens::active()))
                    .hover(|s| s.bg(Tokens::hover()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        set(this, v);
                        this.th.menu = None;
                        this.th.save_prefs();
                        cx.notify();
                    }))
                    .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(label))
                    .when(unl, |d| d.child(div().flex().items_center().gap(px(3.)).px(px(5.)).h(px(18.)).rounded(px(radius::SM)).bg(Tokens::info_soft()).child(icon_ph(ph::INFINITY, Tokens::info(), icon_px::XS)).child(div().text_size(px(10.)).text_color(Tokens::info()).child("Web")))),
            );
        }
        div().absolute().top_full().left_0().child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear(id, pop))).with_priority(3))
    }

    fn generate_block(&self, cx: &mut Context<Self>) -> Div {
        let m = self.th.model();
        let unlimited = m.web_unlimited_at(self.th.res);
        let cost = m.cost_at(self.th.res);
        let running = self.th.jobs.len();
        let button = div()
            .id("thg-generate")
            .flex()
            .items_center()
            .h(px(40.))
            .px(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::accent())
            .cursor_pointer()
            .hover(|s| s.bg(Tokens::accent_hover()))
            .active(|s| s.bg(Tokens::accent_pressed()))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.generate_from_prompt(cx)))
            .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::on_accent()).child(if running > 0 { format!("Generate · {running} running") } else { "Generate".into() }))
            .child(icon_ph(ph::SPARKLE, Tokens::on_accent(), icon_px::SM));
        let line = |icon: Ph, color: Rgba, bg: Rgba, text: String| {
            div().flex().items_center().gap(px(6.)).px(px(space::MD)).py(px(6.)).rounded(px(radius::MD)).bg(bg).child(icon_ph(icon, color, icon_px::XS)).child(div().flex_1().text_size(px(size::XS)).text_color(color).whitespace_normal().child(text))
        };
        let price = if cost > 0. { format!("About {} credits × {} from your API balance", cost.round(), self.th.count) } else { format!("Billed in API credits × {} (Magnific lists no price for it)", self.th.count) };
        div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(button)
            .child(line(ph::COINS, Tokens::text_2(), Tokens::surface_2(), price))
            .when(unlimited, |d| d.child(line(ph::INFINITY, Tokens::info(), Tokens::info_soft(), format!("Unlimited at {} in the Magnific web app only; the API still uses credits", m.web_unlimited.unwrap_or(self.th.res)))))
    }

    fn usage_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let u = &self.th.usage;
        let p = u.budget.map(|b| (u.spent / b.max(1) as f64) as f32);
        // With a plan total: a ring filling up and what is left; without: what was used.
        let shown = match u.budget {
            Some(b) => format!("{} left", short_count((b as f64 - u.spent).max(0.) as u64)),
            None => short_count(u.spent as u64),
        };
        div()
            .relative()
            .child(
                div()
                    .id("thg-usage")
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(radius::PILL))
                    .bg(Tokens::surface_3())
                    .cursor_pointer()
                    .hover(|s| s.bg(Tokens::hover()))
                    .map(|d| match p {
                        Some(p) => d.child(progress_ring_small(p)),
                        None => d.child(icon_ph(ph::COINS, Tokens::text_2(), icon_px::XS)),
                    })
                    .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(shown))
                    .tooltip(tip(match u.budget {
                        Some(b) => format!("{} of {} credits used · {} left", u.spent.round(), b, (b as f64 - u.spent).max(0.).round()),
                        None => format!("{} credits used by Studio", u.spent.round()),
                    }))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_menu(Menu::Usage, cx))),
            )
            .when(self.th.menu == Some(Menu::Usage), |d| d.child(self.usage_menu(cx)))
    }

    fn usage_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let u = &self.th.usage;
        let budget = self.th.budget.clone().expect("inputs");
        let (used, left, p) = match u.budget {
            Some(b) => (format!("{} used", u.spent.round()), format!("{} left", (b as f64 - u.spent).max(0.).round()), (u.spent / b.max(1) as f64) as f32),
            None => (format!("{} used", u.spent.round()), "Set your plan's credits to see what is left".into(), 0.),
        };
        let pop = popup_surface()
            .w(px(300.))
            .p(px(space::MD))
            .gap(px(space::SM))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.th.menu = None;
                cx.notify();
            }))
            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Magnific credits"))
            .child(div().flex().items_end().gap(px(space::SM)).child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(used)).child(div().pb(px(3.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(left)))
            .child(progress_bar("thg-usage-bar", p, false))
            .child(div().flex().gap(px(space::LG)).text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{} images", u.images)).child(format!("{} unpriced", u.unpriced)).child(format!("this month {}", u.month_spent().round())))
            .child(label_caps("Credits on your plan"))
            .child(Input::new(&budget))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Magnific's API has no balance endpoint, so Studio counts each image at Magnific's listed price (models with no listed price count as images only). Your exact balance is on magnific.com."))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(btn("thg-usage-open", "Open balance", ButtonKind::Secondary).small().icon(ph::ARROW_SQUARE_OUT).on_click(|_, _, cx| cx.open_url("https://www.magnific.com/developers/dashboard")))
                    .child(btn("thg-usage-reset", "Reset count", ButtonKind::Ghost).small().icon(ph::ARROW_COUNTER_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        let budget = this.th.usage.budget;
                        this.th.usage = Usage { budget, ..Default::default() };
                        this.th.usage.save();
                        cx.notify();
                    }))),
            );
        div().absolute().top_full().left_0().child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear("thg-usage-pop", pop))).with_priority(3))
    }

    fn right_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let tab = |id: &'static str, icon: Ph, label: String, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(32.))
                .px(px(space::MD))
                .rounded(px(radius::LG))
                .cursor_pointer()
                .text_size(px(size::SM))
                .font_weight(FontWeight::SEMIBOLD)
                .map(|d| if on { d.bg(Tokens::surface_3()).text_color(Tokens::text_1()) } else { d.text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover())) })
                .child(icon_ph(icon, if on { Tokens::text_1() } else { Tokens::text_2() }, icon_px::XS))
                .child(label)
        };
        let n = self.th.gallery.thumbs.len() + self.th.jobs.len();
        let tabs = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::XS))
            .child(tab("thg-tab-made", ph::IMAGES, format!("Creations{}", if n > 0 { format!(" · {n}") } else { String::new() }), self.th.tab == Tab::Creations).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.th.tab = Tab::Creations;
                cx.notify();
            })))
            .child(tab("thg-tab-ideas", ph::LIGHTBULB, format!("AI ideas{}", if self.th.gallery.concepts.is_empty() { String::new() } else { format!(" · {}", self.th.gallery.concepts.len()) }), self.th.tab == Tab::Ideas).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.th.tab = Tab::Ideas;
                cx.notify();
            })))
            .child(tab("thg-tab-rules", ph::LIST_CHECKS, "Thumbnail rules".into(), self.th.tab == Tab::Rules).tooltip(tip("What wins clicks on recap thumbnails, measured on real channels, and how your candidates score")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.th.tab = Tab::Rules;
                cx.notify();
            })))
            .child(div().flex_1())
            .when(self.th.tab == Tab::Creations && self.th.gallery.thumbs.len() > 1 && self.th.open.is_none(), |d| {
                d.child(
                    btn("thg-sort", if self.th.best_first { "Best score first" } else { "Newest first" }, ButtonKind::Ghost)
                        .small()
                        .icon(if self.th.best_first { ph::SORT_DESCENDING } else { ph::CLOCK })
                        .tooltip("Order the creations by click score or by date")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.th.best_first = !this.th.best_first;
                            cx.notify();
                        })),
                )
            })
            .child(
                btn("thg-design", if self.th.designing { "Studying the manga…" } else if self.th.gallery.concepts.is_empty() { "Design with AI" } else { "Design again" }, ButtonKind::Secondary)
                    .small()
                    .icon(ph::MAGIC_WAND)
                    .loading(self.th.designing)
                    .tooltip("The AI model at the top reads the manga's characters and story, studies what wins clicks on recap thumbnails, and writes 4 concepts")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.design(cx))),
            )
            .child(icon_button("thg-folder", ph::FOLDER_OPEN, "Open the thumbnails folder").on_click({
                let dir = Gallery::dir(&self.manga_id);
                move |_, _, cx| {
                    let _ = std::fs::create_dir_all(&dir);
                    cx.reveal_path(&dir)
                }
            }));
        let body = match self.th.tab {
            Tab::Creations => self.creations(cx),
            Tab::Ideas => self.ideas(cx),
            Tab::Rules => self.rules_tab(cx),
        };
        div().flex().flex_col().flex_1().min_w(px(320.)).gap(px(space::MD)).child(tabs).child(body)
    }

    fn creations(&self, cx: &mut Context<Self>) -> Div {
        if let Some(open) = self.th.open.clone().and_then(|id| self.th.gallery.thumbs.iter().find(|t| t.id == id).cloned()) {
            return self.viewer(&open, cx);
        }
        if self.th.gallery.thumbs.is_empty() && self.th.jobs.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::SM))
                .min_h(px(240.))
                .rounded(px(radius::LG))
                .border_1()
                .border_dashed()
                .border_color(Tokens::border())
                .child(ph_icon(ph::IMAGES, Weight::Duotone, Tokens::text_3(), 32.))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Start creating"))
                .child(div().max_w(px(380.)).text_center().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Press Design with AI for concepts made from this manga, or write your own prompt on the left. Use @Name to put a character in."));
        }
        let mut grid = div().flex().flex_wrap().gap(px(space::MD));
        for (k, j) in self.th.jobs.iter().enumerate() {
            let status = j.status.lock().map(|s| s.clone()).unwrap_or_default();
            let (w, h) = tile_size(j.aspect);
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .w(px(TILE_W))
                    .child(
                        div()
                            .id(("thg-job", k))
                            .relative()
                            .w(px(w))
                            .h(px(h))
                            .rounded(px(radius::LG))
                            .overflow_hidden()
                            .bg(Tokens::surface_2())
                            .child(shimmer())
                            .child(div().absolute().inset_0().flex().flex_col().items_center().justify_center().gap(px(6.)).child(spinner(("thg-spin", k), Tokens::accent(), icon_px::MD)).child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{status} · {}s", j.started.elapsed().as_secs())))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(j.label.clone())),
            );
        }
        let mut order: Vec<&Thumb> = self.th.gallery.thumbs.iter().collect();
        if self.th.best_first {
            order.sort_by_key(|t| std::cmp::Reverse(self.th.scores.get(&t.id).map_or(0, |s| s.value)));
        }
        for (k, t) in order.into_iter().enumerate() {
            grid = grid.child(self.thumb_tile(k, t, cx));
        }
        div().child(grid)
    }

    fn thumb_tile(&self, k: usize, t: &Thumb, cx: &mut Context<Self>) -> Div {
        let path = Gallery::image(&self.manga_id, &t.id);
        let id = t.id.clone();
        let (id2, id3, prompt) = (t.id.clone(), t.id.clone(), t.prompt.clone());
        let hover_id = SharedString::from(format!("thg-tile-{}", t.id));
        let actions = div()
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .flex()
            .gap(px(4.))
            .invisible()
            .group_hover(hover_id.clone(), |s| s.visible())
            .child(icon_button(("thg-use", k), ph::TEXT_T, "Use this prompt").on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_prompt(prompt.clone(), window, cx))))
            .child(icon_button(("thg-show", k), ph::FOLDER_OPEN, "Show in folder").on_click({
                let path = path.clone();
                move |_, _, cx| cx.reveal_path(&path)
            }))
            .child(icon_button(("thg-del", k), ph::TRASH, "Delete").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.delete_thumb(&id2, cx))));
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w(px(TILE_W))
            .child(
                div()
                    .id(("thg-tile", k))
                    .group(hover_id)
                    .relative()
                    .w(px(TILE_W))
                    .h(px(TILE_W * 9. / 16.))
                    .rounded(px(radius::LG))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.th.open = Some(id.clone());
                        cx.notify();
                    }))
                    .child(img(path).size_full().object_fit(ObjectFit::Cover))
                    .child(actions)
                    .child(div().absolute().left(px(6.)).bottom(px(6.)).child(badge(t.resolution.clone(), Tone::Neutral)))
                    .when_some(self.th.scores.get(&t.id), |d, s| d.child(div().absolute().left(px(6.)).top(px(6.)).child(score_badge(("thg-tile-score", k), s)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon_ph(if t.parent.is_some() { ph::PENCIL_SIMPLE } else { ph::SPARKLE }, Tokens::text_2(), icon_px::XS))
                    .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(t.label.clone()))
                    .child(div().flex_1().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(t.model.clone()))
                    .child(icon_button(("thg-edit", k), ph::PENCIL_SIMPLE, "Re-edit").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.th.open = Some(id3.clone());
                        cx.notify();
                    }))),
            )
    }

    /// One thumbnail large, with its prompt and the edit box.
    fn viewer(&self, t: &Thumb, cx: &mut Context<Self>) -> Div {
        let path = Gallery::image(&self.manga_id, &t.id);
        let edit = self.th.edit.clone().expect("inputs");
        let (id, prompt) = (t.id.clone(), t.prompt.clone());
        let parent = t.parent.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(btn("thg-back", "All creations", ButtonKind::Ghost).small().icon(ph::ARROW_LEFT).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.th.open = None;
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .when_some(parent, |d, p| {
                        d.child(btn("thg-parent", "Edited from", ButtonKind::Ghost).small().icon(ph::ARROW_U_UP_LEFT).tooltip("Open the thumbnail this was edited from").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if this.th.gallery.thumbs.iter().any(|t| t.id == p) {
                                this.th.open = Some(p.clone());
                            }
                            cx.notify();
                        })))
                    })
                    .child(btn("thg-open-file", "Open", ButtonKind::Secondary).small().icon(ph::ARROW_SQUARE_OUT).on_click({
                        let path = path.clone();
                        move |_, _, cx| cx.open_with_system(&path)
                    }))
                    .child(btn("thg-copy-path", "Folder", ButtonKind::Secondary).small().icon(ph::FOLDER_OPEN).on_click({
                        let path = path.clone();
                        move |_, _, cx| cx.reveal_path(&path)
                    }))
                    .child(btn("thg-del-open", "Delete", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.delete_thumb(&id, cx)))),
            )
            .child(div().w_full().max_w(px(860.)).rounded(px(radius::LG)).overflow_hidden().bg(Tokens::surface_2()).child(img(path).w_full().object_fit(ObjectFit::Contain)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .max_w(px(860.))
                    .p(px(space::SM))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(icon_ph(ph::PENCIL_SIMPLE, Tokens::text_2(), icon_px::SM))
                    .child(div().flex_1().child(Input::new(&edit).appearance(false)))
                    .child(btn("thg-apply", "Re-edit", ButtonKind::Primary).small().icon(ph::SPARKLE).tooltip("Make a new version of this thumbnail with your change; the original stays").on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.apply_edit(window, cx)))),
            )
            .child(row("Prompt", div().flex().items_start().gap(px(space::SM)).child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(prompt.clone())).child(
                btn("thg-reuse", "Use", ButtonKind::Ghost).small().icon(ph::TEXT_T).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_prompt(prompt.clone(), window, cx))),
            )))
            .child(row("Made with", div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{} · {} · {}", t.model, t.resolution, if t.cost == 0. { "unlimited".to_string() } else { format!("{} credits", t.cost.round()) }))))
            .when_some(self.th.scores.get(&t.id), |d, s| d.child(row("Click score", score_detail("thg-view-score", s))))
    }

    fn ideas(&self, cx: &mut Context<Self>) -> Div {
        let g = &self.th.gallery;
        if self.th.designing {
            return div().flex().flex_col().gap(px(space::MD)).child(skeleton_rows(&[None, Some(420.)], 10.)).child(div().flex().flex_wrap().gap(px(space::MD)).children((0..4).map(|_| div().w(px(300.)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::surface()).child(skeleton_rows(&[Some(120.), None, None, Some(200.)], 10.)))));
        }
        if g.concepts.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::SM))
                .min_h(px(240.))
                .rounded(px(radius::LG))
                .border_1()
                .border_dashed()
                .border_color(Tokens::border())
                .child(ph_icon(ph::LIGHTBULB, Weight::Duotone, Tokens::text_3(), 32.))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Let the AI design them"))
                .child(div().max_w(px(420.)).text_center().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("It reads the characters and the story Studio learned from the chapters, looks at the recaps that get the most views, and picks the click patterns that fit: onlookers, power reveal, scale, before / after, reaction…"));
        }
        let mut cards = div().flex().flex_wrap().gap(px(space::MD));
        for (k, c) in g.concepts.iter().enumerate() {
            cards = cards.child(self.concept_card(k, c, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .when(!g.analysis.is_empty(), |d| {
                d.child(div().flex().items_start().gap(px(space::SM)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).child(ph_icon(ph::CHART_LINE_UP, Weight::Duotone, Tokens::accent(), icon_px::SM)).child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_1()).whitespace_normal().child(g.analysis.clone())))
            })
            .child(cards)
    }

    fn concept_card(&self, k: usize, c: &Concept, cx: &mut Context<Self>) -> Div {
        let (p1, p2, name) = (c.prompt.clone(), c.prompt.clone(), c.name.clone());
        let score = click::model().score_idea(&c.prompt, &c.text);
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .w(px(300.))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(div().flex().items_center().gap(px(space::SM)).child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(c.name.clone())).when(!c.text.is_empty(), |d| d.child(badge(c.text.clone(), Tone::Accent))))
            .when(!c.why.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(c.why.clone())))
            .child(score_detail(SharedString::from(format!("thg-idea-score-{k}")), &score))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().line_clamp(5).child(c.prompt.clone()))
            .child(
                div()
                    .flex()
                    .gap(px(space::SM))
                    .child(btn(("thg-idea-gen", k), "Generate", ButtonKind::Primary).small().icon(ph::SPARKLE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start(p1.clone(), name.clone(), None, cx))))
                    .child(btn(("thg-idea-edit", k), "Edit prompt", ButtonKind::Ghost).small().icon(ph::PENCIL_SIMPLE).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_prompt(p2.clone(), window, cx)))),
            )
    }
}

/// Green / amber / grey by where the score sits among the studied thumbnails.
fn score_tone(v: u8) -> Tone {
    match v {
        65.. => Tone::Success,
        40..=64 => Tone::Warning,
        _ => Tone::Neutral,
    }
}

pub(super) fn score_badge(id: impl Into<ElementId>, s: &Score) -> Stateful<Div> {
    div()
        .id(id)
        .tooltip(tip(format!(
            "Click score {}: beats {}% of the {} recap thumbnails studied on the checklist{}. It is a checklist of what the biggest recap channels do, not a views forecast (see Thumbnail rules).",
            s.value,
            s.value,
            click::model().data.labelled,
            if s.from_image { ", read from the prompt and the image" } else { ", read from the prompt" }
        )))
        .child(badge(format!("Score {}", s.value), score_tone(s.value)))
}

/// The score with its reasons (what helps, what hurts) and the top tips.
pub(super) fn score_detail(id: impl Into<ElementId>, s: &Score) -> Div {
    let reason = |r: &click::Reason| {
        let good = r.effect > 0.;
        let color = if good { Tokens::success() } else { Tokens::danger() };
        div().flex().items_center().gap(px(4.)).text_size(px(size::XS)).child(icon_ph(if good { ph::ARROW_UP } else { ph::ARROW_DOWN }, color, 10.)).child(div().text_color(Tokens::text_2()).child(r.label.clone()))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().flex().items_center().gap(px(space::SM)).child(score_badge(id, s)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if s.from_image { "prompt + image" } else { "from the prompt" })))
        .child(div().flex().flex_wrap().gap_x(px(space::SM)).gap_y(px(2.)).children(s.reasons.iter().take(5).map(reason)))
        .children(s.tips.iter().map(|t| div().flex().items_start().gap(px(4.)).text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(icon_ph(ph::LIGHTBULB, Tokens::warning(), 10.)).child(div().flex_1().child(format!("Try: {t}")))))
}

fn tile_size(aspect: &str) -> (f32, f32) {
    let (w, h) = aspect.split_once(':').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))).unwrap_or((16., 9.));
    let max_h = TILE_W * 9. / 16.;
    if w >= h {
        (TILE_W, (TILE_W * h / w).min(max_h))
    } else {
        (max_h * w / h, max_h)
    }
}

/// A 22px ring for the credits button.
fn progress_ring_small(p: f32) -> AnyElement {
    progress_ring_sized(p, 22.)
}

async fn ask_concepts(reg: std::sync::Arc<mv_ai::Registry>, provider: mv_ai::AiProviderId, model: String, system: String, user: String) -> Result<(String, Vec<Concept>)> {
    static N: AtomicU64 = AtomicU64::new(0);
    let mut messages = vec![mv_ai::AiMessage::system(system), mv_ai::AiMessage::user(user)];
    for attempt in 0..2 {
        let req = mv_ai::AiStepRequest {
            request_id: format!("studio-thumbs-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
            provider_id: provider,
            model: model.clone(),
            messages: messages.clone(),
            tools: None,
            effort: None,
            access: None,
            images: Vec::new(),
            workspace: None,
            lean: true,
            mcp: None,
        };
        let r = reg.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
        if let Some(e) = r.error {
            bail!(e);
        }
        if let Some(c) = thumbs::parse_concepts(&r.text) {
            return Ok(c);
        }
        if attempt == 0 {
            messages.push(mv_ai::AiMessage::assistant(r.text));
            messages.push(mv_ai::AiMessage::user("Your previous answer was not valid JSON. Reply with the JSON object only."));
        }
    }
    Err(anyhow!("the answer had no usable concept; try again or pick another model"))
}

/// Opens Settings › Magnific.
fn open_settings(window: &mut Window, cx: &mut App) {
    mv_ui::navigate(Route::Settings { tab: Some("magnific".into()) }, window, cx);
}

fn settings_btn(id: &'static str, label: &'static str) -> Btn {
    btn(id, label, ButtonKind::Secondary).small().icon(ph::GEAR_SIX).on_click(|_, window, cx| open_settings(window, cx))
}

/// One plain-text answer: the AI's rewritten prompt, without fences, quotes or a JSON wrapper.
async fn ask_text(reg: std::sync::Arc<mv_ai::Registry>, provider: mv_ai::AiProviderId, model: String, system: String, user: String) -> Result<String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let req = mv_ai::AiStepRequest {
        request_id: format!("studio-thumb-prompt-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)),
        provider_id: provider,
        model,
        messages: vec![mv_ai::AiMessage::system(system), mv_ai::AiMessage::user(user)],
        tools: None,
        effort: None,
        access: None,
        images: Vec::new(),
        workspace: None,
        lean: true,
        mcp: None,
    };
    let r = reg.run_step_cancellable(req, mv_ai::emit_noop(), mv_ai::CancellationToken::new()).await;
    if let Some(e) = r.error {
        bail!(e);
    }
    thumbs::clean_prompt(&r.text).ok_or_else(|| anyhow!("the answer was empty; try again or pick another model"))
}
