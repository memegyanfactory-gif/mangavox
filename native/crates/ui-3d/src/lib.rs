//! The Creative page's "3D" tab: pick any downloaded chapter, and mv-3d turns it into a 3D
//! episode scene by scene. Each finished scene is shown here (previz per shot, rendered stills,
//! the clip) to approve or redo with a note; the asset library is browsable by category.
//! The host embeds [`view`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_3d::plan::{AssetNeed, ScenePlan};
use mv_3d::{Category, Episode, Library, Origin, RunOptions, SceneStatus};
use mv_recap::{ChapterRef, ModelChoice};
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_state, muted, ph, segmented, size, spinner, switch, tag, ButtonKind, Tokens, Tone};
use ui_studio::data::{ChapterItem, MangaGroup};

const POLL: Duration = Duration::from_millis(700);

/// The 3D tab.
pub fn view(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ThreeD::new(window, cx)).into()
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Scenes,
    Assets,
    Library,
}

struct ThreeD {
    groups: Vec<MangaGroup>,
    loading: bool,
    load_error: Option<String>,
    open_groups: HashSet<String>,
    chapter: Option<ChapterItem>,
    episode: Option<Episode>,
    library: Library,
    tab: Tab,
    category: Category,
    keep_going: bool,
    blender: Option<PathBuf>,
    note: Entity<InputState>,
    /// Which scene the redo note is for.
    note_for: Option<usize>,
    seen_rev: u64,
    /// When each scene was last rendered, to drop stale cached stills after a redo.
    rendered: HashMap<usize, i64>,
    busy: bool,
    picker: AnyView,
    /// Chapters that have a 3D episode, found off the UI thread with the chapter list.
    has_episode: HashSet<String>,
    /// Library assets whose preview render exists.
    previews: HashSet<String>,
    /// The joined episode video, when it is on disk.
    video: Option<PathBuf>,
    /// Blender was looked for (so the setup notice does not flash before the check).
    blender_checked: bool,
}

/// What [`ThreeD::reload`] reads from disk in the background.
struct Snapshot {
    library: Library,
    episode: Option<Episode>,
    previews: HashSet<String>,
    video: Option<PathBuf>,
}

fn snapshot(dir: Option<PathBuf>) -> Snapshot {
    let library = Library::open_default();
    let previews = library.assets.iter().filter(|a| library.preview_of(a).is_file()).map(|a| a.id.clone()).collect();
    let episode = dir.and_then(|d| Episode::load(&d));
    let video = episode.as_ref().and_then(mv_3d::episode_video);
    Snapshot { library, episode, previews, video }
}

fn chref(c: &ChapterItem) -> ChapterRef {
    ChapterRef { chapter_id: c.id.clone(), manga_id: c.manga_id.clone(), manga_title: c.manga_title.clone(), label: c.label(), pages_dir: c.pages_dir.clone(), output_dir: c.output_dir.clone() }
}

impl ThreeD {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let note = cx.new(|cx| InputState::new(window, cx).placeholder("What should change? (optional)"));
        let mut v = ThreeD {
            groups: vec![],
            loading: true,
            load_error: None,
            open_groups: HashSet::new(),
            chapter: None,
            episode: None,
            library: Library::empty(mv_3d::paths::library()),
            tab: Tab::Scenes,
            category: Category::Characters,
            keep_going: false,
            blender: None,
            note,
            note_for: None,
            seen_rev: 0,
            rendered: HashMap::new(),
            busy: false,
            picker: ui_assistant::model_picker(window, cx),
            has_episode: HashSet::new(),
            previews: HashSet::new(),
            video: None,
            blender_checked: false,
        };
        v.load_chapters(cx);
        v.reload(cx);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            if this.update(cx, |v, cx| v.poll(cx)).is_err() {
                break;
            }
        })
        .detach();
        v
    }

    fn load_chapters(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(|| {
                let groups = ui_studio::data::load()?;
                let has: HashSet<String> = groups
                    .iter()
                    .flat_map(|g| g.chapters.iter())
                    .filter(|c| mv_3d::paths::episode_dir(&c.manga_id, &c.id).join("episode.json").is_file())
                    .map(|c| c.id.clone())
                    .collect();
                anyhow::Ok((groups, has, mv_3d::blender::find()))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |v, cx| {
                v.loading = false;
                v.blender_checked = true;
                match r {
                    Ok((g, has, blender)) => {
                        v.has_episode = has;
                        v.blender = blender;
                        if v.open_groups.is_empty() {
                            if let Some(first) = g.first() {
                                v.open_groups.insert(first.id.clone());
                            }
                        }
                        v.groups = g;
                    }
                    Err(e) => v.load_error = Some(format!("Could not load your downloaded chapters: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn dir(&self) -> Option<PathBuf> {
        self.chapter.as_ref().map(|c| mv_3d::paths::episode_dir(&c.manga_id, &c.id))
    }

    fn live(&self) -> Option<mv_3d::Live> {
        self.chapter.as_ref().and_then(|c| mv_3d::live(&c.id))
    }

    fn running(&self) -> bool {
        self.live().is_some_and(|l| l.running)
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.live() else { return };
        if l.rev != self.seen_rev {
            self.seen_rev = l.rev;
            self.reload(cx);
            cx.notify();
        }
    }

    /// Re-reads the library and the open episode in the background, then shows them.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let dir = self.dir();
        let chapter = self.chapter.as_ref().map(|c| c.id.clone());
        cx.spawn(async move |this, cx| {
            let Ok(snap) = mv_ui::bg::blocking(move || snapshot(dir)).await else { return };
            let _ = this.update(cx, |v, cx| {
                if v.chapter.as_ref().map(|c| c.id.clone()) != chapter {
                    return;
                }
                v.apply(snap, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        self.library = snap.library;
        self.previews = snap.previews;
        self.video = snap.video;
        self.episode = snap.episode;
        if let (Some(c), Some(_)) = (&self.chapter, &self.episode) {
            self.has_episode.insert(c.id.clone());
        }
        // Older takes reused the same file names: forget cached images so a new take shows.
        if let Some(ep) = &self.episode {
            for s in &ep.scenes {
                let Some(r) = &s.render else { continue };
                if self.rendered.get(&s.index).is_some_and(|t| *t != r.rendered_at) {
                    for p in r.previz.iter().chain(r.stills.iter()) {
                        ImageSource::from(p.clone()).remove_asset(cx);
                    }
                }
                self.rendered.insert(s.index, r.rendered_at);
            }
        }
    }

    fn select(&mut self, c: ChapterItem, cx: &mut Context<Self>) {
        self.chapter = Some(c);
        self.rendered.clear();
        self.note_for = None;
        self.seen_rev = self.live().map(|l| l.rev).unwrap_or(0);
        self.episode = None;
        self.video = None;
        self.reload(cx);
        cx.notify();
    }

    fn model(cx: &mut App) -> Option<ModelChoice> {
        let (provider, model) = ui_assistant::assistant_model(cx);
        let provider = serde_json::from_value::<mv_ai::AiProviderId>(serde_json::Value::String(provider)).ok()?;
        (!model.is_empty()).then_some(ModelChoice { provider, model })
    }

    fn start(&mut self, opts: RunOptions, cx: &mut Context<Self>) {
        let Some(c) = self.chapter.clone() else { return };
        if self.blender.is_none() {
            self.blender = mv_3d::blender::find();
            if self.blender.is_none() {
                toast::show(cx, Toast::error(mv_3d::blender::SETUP_MESSAGE));
                return;
            }
        }
        let Some(model) = Self::model(cx) else {
            toast::show(cx, Toast::error("Pick an AI model first (model button at the top of this tab)."));
            return;
        };
        let ctx = mv_3d::Ctx::new(ui_assistant::registry(cx), model);
        let label = c.label();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(mv_3d::run(ctx, chref(&c), opts)).await.and_then(|r| r);
            let _ = this.update(cx, |v, cx| {
                v.reload(cx);
                match r {
                    Ok(ep) if ep.video.is_some() => toast::show(cx, Toast::success(format!("The 3D episode of {label} is ready")).icon(ph::FILM_STRIP)),
                    Ok(ep) => {
                        if let Some(s) = ep.scenes.iter().find(|s| s.status == SceneStatus::Review) {
                            toast::show(cx, Toast::success(format!("Scene {} of {label} is ready to review", s.index + 1)).icon(ph::FILM_STRIP));
                        }
                    }
                    Err(e) if e.to_string() == "cancelled" => {}
                    Err(e) => toast::show(cx, Toast::error(format!("3D run stopped: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
        // The run registers itself on the tokio side a moment later; poll picks it up.
        cx.notify();
    }

    fn approve(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(dir) = self.dir() else { return };
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || mv_3d::approve(&dir, i)).await.and_then(|r| r);
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(ep) => {
                        let all = ep.all_approved();
                        v.episode = Some(ep);
                        if all {
                            v.join(cx);
                        } else if !v.running() {
                            // On to the next scene.
                            v.start(RunOptions { keep_going: v.keep_going, ..Default::default() }, cx);
                        }
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("{e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn redo(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(dir) = self.dir() else { return };
        let note = self.note.read(cx).value().to_string();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || mv_3d::redo(&dir, i, Some(note))).await.and_then(|r| r);
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(ep) => {
                        v.episode = Some(ep);
                        v.note_for = None;
                        v.start(RunOptions { only: Some(i), ..Default::default() }, cx);
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("{e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn join(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.dir() else { return };
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(mv_3d::join_episode(dir)).await.and_then(|r| r);
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match r {
                    Ok(ep) => {
                        if ep.video.is_some() {
                            toast::show(cx, Toast::success("Every scene is approved: the episode is ready").icon(ph::FILM_STRIP));
                        }
                        v.episode = Some(ep);
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("Could not join the episode: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── rendering ─────────────────────────────────────────────────────────────────────────

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap(px(2.));
        if self.loading {
            list = list.child(muted("Loading your downloads…"));
        }
        if let Some(e) = &self.load_error {
            list = list.child(div().text_color(Tokens::danger()).text_size(px(size::SM)).child(e.clone()));
        }
        if !self.loading && self.groups.is_empty() && self.load_error.is_none() {
            list = list.child(muted("Download a manga chapter first; every downloaded chapter shows up here."));
        }
        let selected = self.chapter.as_ref().map(|c| c.id.clone());
        for (gi, g) in self.groups.iter().enumerate() {
            let open = self.open_groups.contains(&g.id);
            let gid = g.id.clone();
            list = list.child(
                div()
                    .id(("g3d", gi))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .px(px(space::SM))
                    .py(px(6.))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(Tokens::hover()))
                    .child(mv_ui::icon_ph(if open { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Tokens::text_3(), 12.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).child(g.title.clone()))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(g.chapters.len().to_string()))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.open_groups.remove(&gid) {
                            v.open_groups.insert(gid.clone());
                        }
                        cx.notify();
                    })),
            );
            if open {
                for (ci, c) in g.chapters.iter().enumerate() {
                    let item = c.clone();
                    let active = selected.as_deref() == Some(c.id.as_str());
                    let has_ep = self.has_episode.contains(&c.id);
                    list = list.child(
                        div()
                            .id(("c3d", gi * 10_000 + ci))
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .pl(px(28.))
                            .pr(px(space::SM))
                            .py(px(5.))
                            .rounded(px(radius::SM))
                            .cursor_pointer()
                            .when(active, |d| d.bg(Tokens::active()))
                            .hover(|d| d.bg(Tokens::hover()))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).child(c.label()))
                            .when(has_ep, |d| d.child(mv_ui::icon_ph(ph::CUBE, Tokens::accent(), 12.)))
                            .on_click(cx.listener(move |v, _, _, cx| v.select(item.clone(), cx))),
                    );
                }
            }
        }
        div()
            .w(px(280.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(Tokens::border())
            .child(div().px(px(space::LG)).pt(px(space::LG)).pb(px(space::SM)).child(mv_ui::section_label("Downloaded manga")))
            .child(div().id("ch3d-list").flex_1().overflow_y_scroll().px(px(space::SM)).pb(px(space::LG)).child(list))
    }

    fn header(&self, c: &ChapterItem, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running();
        let ep = self.episode.as_ref();
        let started = ep.is_some_and(|e| !e.scenes.is_empty());
        let in_review = ep.and_then(|e| e.scenes.iter().find(|s| s.status == SceneStatus::Review)).map(|s| s.index);
        let all_done = ep.is_some_and(|e| e.all_approved());
        let go_label = if !started { "Make the 3D episode" } else if in_review.is_some() { "Render next scene" } else { "Continue" };
        let keep = self.keep_going;
        let chapter_id = c.id.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::LG))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(c.manga_title.clone()))
                            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).child(ep.map(|e| e.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| c.label())))
                            .children(ep.map(|e| e.logline.clone()).filter(|l| !l.is_empty()).map(|l| div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(l))),
                    )
                    .child(div().flex_none().child(self.picker.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(space::SM))
                    .when(!running && !all_done, |d| {
                        d.child(btn("go3d", go_label, ButtonKind::Primary).icon(ph::PLAY).on_click(cx.listener(move |v, _, _, cx| v.start(RunOptions { keep_going: keep, ..Default::default() }, cx))))
                    })
                    .when(running, |d| d.child(btn("stop3d", "Stop", ButtonKind::Secondary).icon(ph::STOP).on_click(move |_, _, _| mv_3d::stop(&chapter_id))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(switch("keep3d", keep).on_click(cx.listener(|v, on: &bool, _, cx| {
                                v.keep_going = *on;
                                cx.notify();
                            })))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Render every scene without stopping")),
                    )
                    .when(started && !running, |d| {
                        d.child(btn("replan3d", "Plan again", ButtonKind::Ghost).small().icon(ph::ARROWS_CLOCKWISE).tooltip("Break the chapter into scenes again (the library is kept)").on_click(cx.listener(|v, _, _, cx| {
                            v.start(RunOptions { replan: true, ..Default::default() }, cx)
                        })))
                    })
                    .when_some(self.video.clone(), |d, video| {
                        d.child(btn("play-ep", "Play the episode", ButtonKind::Primary).icon(ph::FILM_STRIP).on_click(move |_, _, cx| cx.open_with_system(&video)))
                    })
                    .when(all_done && ep.is_some_and(|e| e.video.is_none()), |d| d.child(btn("join3d", "Join the episode", ButtonKind::Secondary).loading(self.busy).on_click(cx.listener(|v, _, _, cx| v.join(cx))))),
            )
            .when(self.blender_checked && self.blender.is_none(), |d| d.child(notice(mv_3d::blender::SETUP_MESSAGE, Tone::Warning)))
    }

    fn live_panel(&self) -> Option<impl IntoElement> {
        let l = self.live()?;
        if !l.running && l.error.is_none() {
            return None;
        }
        let tail: Vec<String> = l.log.iter().rev().take(6).rev().cloned().collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(space::MD))
                .rounded(px(radius::MD))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .when(l.running, |d| d.child(spinner("live3d", Tokens::accent(), 14.)))
                        .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).child(if l.stage.is_empty() { "Starting…".to_string() } else { l.stage.clone() })),
                )
                .children(l.waiting.map(|w| div().text_size(px(size::SM)).text_color(Tokens::warning()).child(w)))
                .children(l.error.filter(|_| !l.running).map(|e| div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e)))
                .children(tail.into_iter().map(|t| div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(t))),
        )
    }

    fn scenes(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(ep) = &self.episode else {
            return empty_state(
                "No 3D episode yet",
                "Press \"Make the 3D episode\": the AI reads the chapter, lists every location, character and prop, builds them in Blender, then stages and renders the chapter one scene at a time for you to review.",
            )
            .into_any_element();
        };
        let running = self.running();
        div().flex().flex_col().gap(px(space::MD)).children(ep.scenes.iter().map(|s| self.scene_card(ep, s, running, cx))).into_any_element()
    }

    fn scene_card(&self, ep: &Episode, s: &ScenePlan, running: bool, cx: &mut Context<Self>) -> AnyElement {
        let i = s.index;
        let tone = match s.status {
            SceneStatus::Approved => Tone::Success,
            SceneStatus::Review => Tone::Accent,
            SceneStatus::Failed => Tone::Danger,
            st if st.busy() => Tone::Info,
            _ => Tone::Neutral,
        };
        let name_of = |k: &String| ep.need(k).map(|n| n.name.clone()).unwrap_or_else(|| k.clone());
        let place = name_of(&s.location);
        let cast: Vec<String> = s.characters.iter().map(name_of).collect();
        let render = s.render.clone();
        let note_open = self.note_for == Some(i);
        let can_act = !running && matches!(s.status, SceneStatus::Review | SceneStatus::Approved | SceneStatus::Failed);
        div()
            .id(("scene3d", i))
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(if s.status == SceneStatus::Review { Tokens::accent_border() } else { Tokens::border() })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).child(format!("{}. {}", i + 1, if s.title.is_empty() { "Scene" } else { s.title.as_str() })))
                    .child(tag(s.status.label(), tone))
                    .when(s.takes > 1, |d| d.child(tag(format!("take {}", s.takes), Tone::Neutral)))
                    .child(div().flex_1())
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("pages {}-{}", s.pages[0] + 1, s.pages[1] + 1))),
            )
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{place}{}{}", if cast.is_empty() { "" } else { " · " }, cast.join(", "))))
            .when(!s.summary.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s.summary.clone())))
            .children(s.error.clone().map(|e| notice(&e, Tone::Danger)))
            .when_some(render.clone(), |d, r| {
                d.child(div().flex().gap(px(space::SM)).flex_wrap().children(r.stills.iter().take(4).map(|p| frame(p.clone(), 220.))))
                    .when(!r.previz.is_empty(), |d| {
                        d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Previz, one still per shot"))
                            .child(div().flex().gap(px(6.)).flex_wrap().children(r.previz.iter().map(|p| frame(p.clone(), 112.))))
                    })
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{:.1} s · {} shots", r.seconds, r.shots.len())))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(space::SM))
                    .when_some(render.as_ref().and_then(|r| r.clip.clone()), |d, clip| d.child(btn(("play3d", i), "Play scene", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(move |_, _, cx| cx.open_with_system(&clip))))
                    .when_some(render.as_ref().map(|r| r.blend.clone()), |d, blend| {
                        d.child(btn(("blend3d", i), "Open in Blender", ButtonKind::Ghost).small().icon(ph::CUBE).on_click(move |_, _, cx| cx.open_with_system(&blend)))
                    })
                    .when(s.status == SceneStatus::Review && !running, |d| d.child(btn(("ok3d", i), "Approve", ButtonKind::Primary).small().icon(ph::CHECK).on_click(cx.listener(move |v, _, _, cx| v.approve(i, cx)))))
                    .when(can_act && !note_open, |d| {
                        d.child(btn(("redo3d", i), "Redo…", ButtonKind::Secondary).small().icon(ph::ARROWS_CLOCKWISE).on_click(cx.listener(move |v, _, _, cx| {
                            v.note_for = Some(i);
                            cx.notify();
                        })))
                    })
                    .when(!running && matches!(s.status, SceneStatus::Planned), |d| {
                        d.child(btn(("only3d", i), "Render this scene", ButtonKind::Ghost).small().on_click(cx.listener(move |v, _, _, cx| v.start(RunOptions { only: Some(i), ..Default::default() }, cx))))
                    }),
            )
            .when(note_open, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().flex_1().child(Input::new(&self.note).small()))
                        .child(btn(("redo-go3d", i), "Redo scene", ButtonKind::Primary).small().on_click(cx.listener(move |v, _, _, cx| v.redo(i, cx))))
                        .child(btn(("redo-x3d", i), "Cancel", ButtonKind::Ghost).small().on_click(cx.listener(|v, _, _, cx| {
                            v.note_for = None;
                            cx.notify();
                        }))),
                )
            })
            .into_any_element()
    }

    fn episode_assets(&self) -> AnyElement {
        let Some(ep) = &self.episode else { return empty_state("No assets yet", "They appear once the chapter is broken down.").into_any_element() };
        let mut col = div().flex().flex_col().gap(px(space::LG));
        for cat in Category::ALL {
            let needs: Vec<&AssetNeed> = ep.assets.iter().filter(|a| a.category == cat).collect();
            if needs.is_empty() {
                continue;
            }
            col = col.child(mv_ui::section_label(cat.label())).child(
                div().flex().flex_wrap().gap(px(space::MD)).children(needs.into_iter().map(|n| {
                    let a = n.asset_id.as_deref().and_then(|id| self.library.get(id));
                    let status = match a {
                        Some(a) if a.used_in.iter().any(|c| c != &ep.chapter_id) || a.created < ep.created => "from the library",
                        Some(_) => "built for this episode",
                        None => "to build",
                    };
                    asset_card(n.name.clone(), a.filter(|a| self.previews.contains(&a.id)).map(|a| self.library.preview_of(a)), status.to_string(), a.map(origin_label))
                })),
            );
        }
        if let Some(map) = ep.world_map.as_deref().and_then(|id| self.library.get(id)) {
            col = col.child(mv_ui::section_label("World map")).child(asset_card(map.name.clone(), self.previews.contains(&map.id).then(|| self.library.preview_of(map)), format!("{} locations", map.tags.len()), None));
        }
        col.into_any_element()
    }

    fn library_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let cats: Vec<(Option<mv_ui::Ph>, String)> = Category::ALL.iter().map(|c| (None, format!("{} ({})", c.label(), self.library.in_category(*c).len()))).collect();
        let sel = Category::ALL.iter().position(|c| *c == self.category).unwrap_or(0);
        let items: Vec<AnyElement> = self
            .library
            .in_category(self.category)
            .into_iter()
            .map(|a| {
                let sub = match &a.manga_id {
                    Some(_) if !a.manga_title.is_empty() => a.manga_title.clone(),
                    _ => "Shared".into(),
                };
                asset_card(a.name.clone(), self.previews.contains(&a.id).then(|| self.library.preview_of(a)), format!("{sub} · used in {}", a.used_in.len()), Some(origin_label(a))).into_any_element()
            })
            .collect();
        let root = self.library.root.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(segmented("libcat3d", cats, sel).small().on_change(cx.listener(|v, i: &usize, _, cx| {
                        v.category = Category::ALL[*i];
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .child(btn("libdir3d", "Show folder", ButtonKind::Ghost).small().icon(ph::FOLDER_OPEN).on_click(move |_, _, cx| {
                        let _ = std::fs::create_dir_all(&root);
                        cx.open_with_system(&root)
                    })),
            )
            .child(if items.is_empty() {
                empty_state("Nothing here yet", "Assets land here as episodes are made, and later scenes reuse them.").into_any_element()
            } else {
                div().flex().flex_wrap().gap(px(space::MD)).children(items).into_any_element()
            })
            .into_any_element()
    }
}

fn origin_label(a: &mv_3d::Asset) -> String {
    match &a.origin {
        Origin::Procedural => "modelled by the AI".into(),
        Origin::Character => "character builder".into(),
        Origin::Template => "stand-in".into(),
        Origin::Online { source, .. } => format!("from {source}"),
        Origin::ImageTo3d { service } => format!("{service} image to 3D"),
    }
}

fn frame(p: PathBuf, w: f32) -> impl IntoElement {
    div().w(px(w)).h(px(w * 9. / 16.)).flex_none().rounded(px(radius::SM)).overflow_hidden().bg(Tokens::well()).child(img(p).size_full().object_fit(ObjectFit::Cover))
}

fn asset_card(name: String, preview: Option<PathBuf>, status: String, origin: Option<String>) -> impl IntoElement {
    div()
        .w(px(150.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(div().w(px(150.)).h(px(150.)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::well()).children(preview.map(|p| img(p).size_full().object_fit(ObjectFit::Cover))))
        .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).truncate().child(name))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(status))
        .children(origin.map(|o| div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(o)))
}

fn notice(text: &str, tone: Tone) -> Div {
    let (fg, bg) = match tone {
        Tone::Danger => (Tokens::danger(), Tokens::danger_soft()),
        _ => (Tokens::warning(), Tokens::warning_soft()),
    };
    div().p(px(space::SM)).rounded(px(radius::SM)).bg(bg).text_color(fg).text_size(px(size::SM)).child(text.to_string())
}

impl Render for ThreeD {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.chapter.clone() {
            None => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .px(px(size::PAGE_X))
                .child(div().max_w(px(520.)).child(empty_state(
                    "Turn any chapter into a 3D episode",
                    "Pick a downloaded chapter on the left. The AI reads its pages, builds the locations, characters and props in Blender (kept in a library so later chapters reuse them), then stages, animates and renders it scene by scene for you to review.",
                )))
                .into_any_element(),
            Some(c) => {
                let tabs = segmented("tab3d", ["Scenes", "Assets", "Library"], match self.tab {
                    Tab::Scenes => 0,
                    Tab::Assets => 1,
                    Tab::Library => 2,
                })
                .small()
                .on_change(cx.listener(|v, i: &usize, _, cx| {
                    v.tab = [Tab::Scenes, Tab::Assets, Tab::Library][*i];
                    cx.notify();
                }));
                let body = match self.tab {
                    Tab::Scenes => self.scenes(cx),
                    Tab::Assets => self.episode_assets(),
                    Tab::Library => self.library_view(cx),
                };
                div()
                    .id("main3d")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(size::PAGE_X))
                    .py(px(size::PAGE_Y))
                    .flex()
                    .flex_col()
                    .gap(px(space::LG))
                    .child(self.header(&c, cx))
                    .children(self.live_panel())
                    .child(div().child(tabs))
                    .child(body)
                    .into_any_element()
            }
        };
        div().size_full().flex().bg(Tokens::base()).text_color(Tokens::text_1()).child(self.sidebar(cx)).child(main)
    }
}
