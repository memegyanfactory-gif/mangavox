//! The dashboard Video section's **Video style** panel (see `style` for what each choice is and
//! where it is kept): a live frame on the left, rendered by the video renderer itself
//! (`python/style_preview.py`), the grouped choices on the right, the "Use for all videos" lock,
//! and the automatic re-edit before Play / Export while the style is locked.
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_score::SfxAmount;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, filter_chip, icon_button, icon_ph, muted, ph, segmented, size, spinner, switch, tip, ButtonKind, Ph, Tokens};
use serde_json::Value;

use super::style::{self, AutoRun, AutoState, Goal, Next, PanelPrefs, VideoStyle, MOTIONS, MUSIC, PLACES};
use super::{Mode, Quality, HOOK_SECONDS, PART_COUNTS};
use crate::data::{fresh, ChapterItem, MangaGroup};
use crate::look::{Framing, Layered, Look, Preset};
use crate::page::StudioPage;

/// Wait after the last change before rendering a preview frame.
const DEBOUNCE: Duration = Duration::from_millis(700);
/// How often the panel re-reads the settings it shows (changed elsewhere: the Look panel, the
/// caption studio, the Video editor, the assistant).
const RELOAD: Duration = Duration::from_secs(2);
const PREVIEW_W: f32 = 384.;

/// The preview frame.
#[derive(Default)]
struct Preview {
    path: Option<PathBuf>,
    running: bool,
    /// A change came in while a frame was being made: make another when it is done.
    pending: bool,
    error: Option<String>,
    /// The chapter the frame is from, and whether it is an upright (reels) video.
    label: String,
    upright: bool,
}

/// The panel's state on the Studio page.
pub(crate) struct StyleUi {
    pub prefs: PanelPrefs,
    /// (manga, the style its settings give), the Look as read, and when they were read.
    saved: Option<(String, VideoStyle)>,
    look: Look,
    loading: bool,
    loaded_at: Option<Instant>,
    preview: Preview,
    /// (manga, made chapters that do not match the saved settings), and the queue version then.
    stale: Option<(String, usize)>,
    counted_at: u64,
    counted_when: Option<Instant>,
    auto: Option<AutoRun>,
    then: Option<super::QueuedExport>,
    gen: u64,
    /// The caption template category shown.
    category: Option<String>,
    /// Locked saves, one at a time (the latest wins).
    saves: style::SaveQueue<MangaGroup>,
    /// (manga, chapters with captions or music of their own that differ from the panel).
    custom: Option<(String, usize)>,
}

impl StyleUi {
    pub fn load() -> StyleUi {
        StyleUi {
            prefs: PanelPrefs::load(),
            saved: None,
            look: Look::default(),
            loading: false,
            loaded_at: None,
            preview: Preview::default(),
            stale: None,
            counted_at: u64::MAX,
            counted_when: None,
            auto: None,
            then: None,
            gen: 0,
            category: None,
            saves: style::SaveQueue::default(),
            custom: None,
        }
    }
}

impl StudioPage {
    // ------------------------------------------------------------------------------ state

    /// The style shown for manga `id`: its saved settings with the draft over them.
    fn style_now(&self, id: &str) -> Option<VideoStyle> {
        self.style.saved.as_ref().filter(|(m, _)| m == id).map(|(_, s)| s.merged(&self.style.prefs.draft))
    }

    /// The export choices are kept with the panel (shown again next time).
    pub(crate) fn save_export_prefs(&mut self) {
        self.style.prefs.export = self.export_prefs;
        self.style.prefs.save();
    }

    /// Play, Export and Preview export open the panel the first time (the style is picked
    /// before anything is made). True when it opened: the press does nothing else.
    pub(crate) fn style_first_open(&mut self, cx: &mut Context<Self>) -> bool {
        if self.style.prefs.seen {
            return false;
        }
        self.style.prefs.seen = true;
        self.style.prefs.open = true;
        self.style.prefs.save();
        toast::show(cx, Toast::info("Pick how the video looks (and lock it to use it for every video), then press it again").icon(ph::PALETTE));
        cx.notify();
        true
    }

    /// Re-reads the settings the panel shows, every [`RELOAD`], in the background.
    fn style_refresh(&mut self, g: &MangaGroup, cx: &mut Context<Self>) {
        let fresh_enough = self.style.saved.as_ref().is_some_and(|(m, _)| *m == g.id) && self.style.loaded_at.is_some_and(|t| t.elapsed() < RELOAD);
        if fresh_enough || self.style.loading {
            return;
        }
        // Renders finished since the count: count again (at most every 10 s: it reads every chapter).
        let counted_long_ago = self.style.counted_when.is_none_or(|t| t.elapsed() > Duration::from_secs(10));
        if counted_long_ago && self.style.counted_at != crate::pipeline::version() && self.style.auto.as_ref().is_none_or(|a| !a.running()) {
            self.style_count(g, cx);
        }
        self.style.loading = true;
        let (id, chapters, group) = (g.id.clone(), g.chapters.clone(), g.clone());
        cx.spawn(async move |this, cx| {
            let saved = mv_ui::bg::blocking(move || style::load_saved(style::representative(&chapters).as_ref())).await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.style.loading = false;
                this.style.loaded_at = Some(Instant::now());
                let before = this.style_now(&id);
                let first = this.style.saved.as_ref().is_none_or(|(m, _)| *m != id);
                this.style.saved = Some((id.clone(), VideoStyle::from_saved(&saved)));
                this.style.look = saved.look;
                if first {
                    this.style.preview = Preview::default();
                    this.style_count(&group, cx);
                }
                if first || before != this.style_now(&id) {
                    this.style_schedule(&group, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A choice changed: locked, it is saved at once; unlocked, it joins the draft.
    fn style_edit(&mut self, g: &MangaGroup, f: impl FnOnce(&mut VideoStyle), cx: &mut Context<Self>) {
        let Some((_, saved)) = self.style.saved.clone().filter(|(m, _)| *m == g.id) else { return };
        let before = saved.merged(&self.style.prefs.draft);
        let mut s = before.clone();
        f(&mut s);
        if self.style.prefs.locked {
            // Only what this click changed is written (chapters keep their other settings).
            let fields = style::draft_of(&s, &before).into_iter().map(|(k, _)| k).collect();
            self.style.prefs.draft.clear();
            self.style.saved = Some((g.id.clone(), s.clone()));
            self.style_apply(g, s, fields, cx);
        } else {
            self.style.prefs.draft = style::draft_of(&s, &saved);
        }
        self.style.prefs.save();
        self.style_schedule(g, cx);
        cx.notify();
    }

    /// "Use for all videos": locking saves the choices shown as the Studio settings.
    fn style_set_lock(&mut self, g: &MangaGroup, on: bool, cx: &mut Context<Self>) {
        self.style.prefs.locked = on;
        if on {
            if let Some(s) = self.style_now(&g.id) {
                // The draft holds exactly the fields changed here.
                let fields: BTreeSet<String> = std::mem::take(&mut self.style.prefs.draft).into_iter().map(|(k, _)| k).collect();
                self.style.saved = Some((g.id.clone(), s.clone()));
                if !fields.is_empty() {
                    self.style_apply(g, s, fields, cx);
                }
            }
        }
        self.style.prefs.save();
        cx.notify();
    }

    /// Writes the `fields` of `s` through to the Studio settings and this manga's chapters (in
    /// the background, one save at a time), and Generate's remembered answers (chapters
    /// generated later start with them).
    fn style_apply(&mut self, g: &MangaGroup, s: VideoStyle, fields: BTreeSet<String>, cx: &mut Context<Self>) {
        let mut a = crate::generate::Answers::load(&self.prefs);
        a.captions = s.subs;
        a.subtitle_style = Some(s.sub_style.clone());
        a.subtitle_place = crate::restyle::place_index(PLACES[s.sub_place.min(PLACES.len() - 1)].1.map(|y| (50., y)));
        a.subtitle_size = s.sub_size;
        a.music = s.music > 0;
        a.save();
        if let Some(job) = self.style.saves.push(style::PendingSave { manga: g.id.clone(), style: s, fields, ctx: g.clone() }) {
            self.style_save(job, cx);
        }
    }

    /// Runs one queued save; the next waiting one (the latest style) starts when it is done.
    fn style_save(&mut self, job: style::PendingSave<MangaGroup>, cx: &mut Context<Self>) {
        let style::PendingSave { style: s, fields, ctx: group, .. } = job;
        let chapters = group.chapters.clone();
        cx.spawn(async move |this, cx| {
            let applied = mv_ui::bg::blocking(move || style::write_through(&s, &fields, &chapters, &style::Files::studio())).await;
            let _ = this.update(cx, |this, cx| {
                match applied {
                    Ok(a) => {
                        for e in a.errors {
                            toast::show(cx, Toast::error(e));
                        }
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("Could not save the video style: {e:#}"))),
                }
                this.style.loaded_at = None;
                this.style_count(&group, cx);
                if let Some(next) = this.style.saves.done() {
                    this.style_save(next, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drops the unlocked choices (the panel shows the saved settings again).
    fn style_discard(&mut self, g: &MangaGroup, cx: &mut Context<Self>) {
        self.style.prefs.draft.clear();
        self.style.prefs.save();
        self.style_schedule(g, cx);
        cx.notify();
    }

    /// Counts the made chapters whose video does not match the saved settings (background).
    fn style_count(&mut self, g: &MangaGroup, cx: &mut Context<Self>) {
        self.style.counted_at = crate::pipeline::version();
        self.style.counted_when = Some(Instant::now());
        let chapters = self.made_chapters_of(g);
        let (id, all, shown) = (g.id.clone(), g.chapters.clone(), self.style_now(&g.id));
        cx.spawn(async move |this, cx| {
            let (n, custom) = mv_ui::bg::blocking(move || (fresh::stale_of(&chapters).len(), shown.map(|s| style::custom_chapters(&s, &all)))).await.unwrap_or((0, None));
            let _ = this.update(cx, |this, cx| {
                this.style.stale = Some((id.clone(), n));
                if let Some(c) = custom {
                    this.style.custom = Some((id, c));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------------------ preview

    /// A new frame after [`DEBOUNCE`] without further changes.
    fn style_schedule(&mut self, g: &MangaGroup, cx: &mut Context<Self>) {
        self.style.gen += 1;
        let gen = self.style.gen;
        let g = g.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.style.gen == gen && this.style.prefs.open {
                    this.style_preview_now(&g, cx);
                }
            });
        })
        .detach();
    }

    /// Renders (or takes from the cache) one frame of the representative chapter with the style
    /// shown, through `python/style_preview.py` (the renderer's own code path).
    fn style_preview_now(&mut self, g: &MangaGroup, cx: &mut Context<Self>) {
        if self.style.preview.running {
            self.style.preview.pending = true;
            return;
        }
        let Some(s) = self.style_now(&g.id) else { return };
        let fail = |this: &mut Self, msg: &str, cx: &mut Context<Self>| {
            this.style.preview.error = Some(msg.to_string());
            cx.notify();
        };
        if super::exporting() {
            return fail(self, "The preview waits while a video is being exported.", cx);
        }
        let env = self.setup.read(cx).env.clone();
        let Some((py, layout)) = env.and_then(|e| Some((e.python.ok()?, e.scripts.ok()?))) else {
            return fail(self, "The preview is made by Studio's Python, which is not set up yet (finish Setup).", cx);
        };
        let made = self.made_chapters_of(g);
        let Some(rep) = made.iter().find(|c| c.output_dir.join("sync_map.json").is_file()).cloned() else {
            return fail(self, "Make a chapter video first: the preview is a frame of it.", cx);
        };
        let reels = self.statuses.get(&rep.id).is_some_and(|st| !st.reels.is_empty());
        self.style.preview.running = true;
        self.style.preview.error = None;
        self.style.preview.label = rep.label();
        self.style.preview.upright = reels;
        let group = g.clone();
        cx.spawn(async move |this, cx| {
            let res = render_frame(s, rep, reels, py, layout).await;
            let _ = this.update(cx, |this, cx| {
                let p = &mut this.style.preview;
                p.running = false;
                match res {
                    Ok(path) => {
                        p.path = Some(path);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                if std::mem::take(&mut this.style.preview.pending) {
                    this.style_preview_now(&group, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------------------------ re-edit, then play or export

    /// Before Play, Export or Preview export of `g`'s made videos. Locked: re-renders, without
    /// asking, exactly the chapters that do not match the style, then runs `then`. Unlocked: asks
    /// about changed chapters as before (`data::fresh`).
    pub(crate) fn style_gate(&mut self, g: &MangaGroup, goal: Goal, then: super::QueuedExport, window: &mut Window, cx: &mut Context<Self>) {
        let chapters = self.made_chapters_of(g);
        if !self.style.prefs.locked {
            let page = cx.entity().downgrade();
            let after = if goal == Goal::Play { fresh::After::Offer("Play") } else { fresh::After::Auto };
            fresh::guard(
                chapters,
                goal.verb(),
                after,
                move |window, cx| {
                    let _ = page.update(cx, |this, cx| then(this, window, cx));
                },
                window,
                cx,
            );
            return;
        }
        if self.style.auto.as_ref().is_some_and(AutoRun::running) {
            toast::show(cx, Toast::info("Already re-editing chapters to the style: it carries on when they are done"));
            return;
        }
        self.style.auto = Some(AutoRun::new(&g.id, goal));
        self.style.then = Some(then);
        cx.notify();
        // Changes made in the panel and still being saved land first; nothing else is written
        // again (a chapter's own captions or music, set in the caption studio or Music &
        // subtitles, stay). Then the chapters whose video does not match are found.
        cx.spawn_in(window, async move |this, cx| {
            while matches!(this.update(cx, |this, _| this.style.saves.idle()), Ok(false)) {
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
            let found = mv_ui::bg::blocking(move || fresh::stale_of(&chapters)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let stale = found.unwrap_or_default();
                this.style.loaded_at = None;
                this.style_found(stale.into_iter().map(|s| s.item).collect(), window, cx)
            });
        })
        .detach();
    }

    fn style_found(&mut self, stale: Vec<ChapterItem>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.style.auto.as_mut() else { return };
        match run.found(stale.iter().map(|c| c.id.clone()).collect()) {
            Next::Stop => self.style.then = None,
            Next::Go => self.style_go(window, cx),
            Next::Render(ids) => {
                let goal = run.goal;
                self.sync_pipeline(cx);
                match crate::vedit_tools::render(&stale) {
                    Ok(_) => {
                        let n = ids.len();
                        let then = match goal {
                            Goal::Play => "then it plays",
                            Goal::Export => "then it exports",
                            Goal::PreviewExport => "then the export preview opens",
                        };
                        toast::show(cx, Toast::info(format!("Re-editing {n} chapter{} to the locked style, {then}", if n == 1 { "" } else { "s" })).icon(ph::FILM_STRIP));
                        self.style.prefs.open = true;
                        self.style_follow(window, cx);
                    }
                    Err(e) => {
                        // Nothing could be queued: the current videos are used, as when asked.
                        toast::show(cx, Toast::error(format!("{e}: using the current videos")));
                        if let Some(run) = self.style.auto.as_mut() {
                            run.finish(ids.len());
                        }
                        self.style_go(window, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    /// Follows the queued renders until none of them is in the Generate queue any more.
    fn style_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(1000)).await;
            let q = crate::pipeline::snapshot();
            let done = this.update_in(cx, |this, window, cx| {
                let Some(run) = this.style.auto.as_mut().filter(|r| matches!(r.state, AutoState::Rendering { .. })) else { return true };
                let active: HashSet<String> = run.ids.iter().filter(|id| q.for_chapter(id).is_some_and(|i| i.state.active())).cloned().collect();
                let all = run.poll(&active, &|id| q.for_chapter(id).map(|i| i.progress()).unwrap_or(1.));
                cx.notify();
                if all {
                    this.style_finish(window, cx);
                }
                all
            });
            if !matches!(done, Ok(false)) {
                break;
            }
        })
        .detach();
    }

    /// The renders ended: those still not matching failed (they keep their video); go on.
    fn style_finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.style.auto.clone() else { return };
        let items: Vec<ChapterItem> = match &self.library {
            crate::page::Load::Ready(groups) => groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| run.ids.contains(&c.id)).cloned().collect(),
            _ => vec![],
        };
        cx.spawn_in(window, async move |this, cx| {
            let still = mv_ui::bg::blocking(move || fresh::stale_of(&items).len()).await.unwrap_or(0);
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(run) = this.style.auto.as_mut() else { return };
                if run.finish(still).is_none() {
                    return;
                }
                if still > 0 {
                    toast::show(cx, Toast::warning(run.label()));
                }
                this.reload_library(cx);
                this.style_go(window, cx);
            });
        })
        .detach();
    }

    fn style_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.style.counted_at = u64::MAX;
        if let Some(then) = self.style.then.take() {
            then(self, window, cx);
        }
        cx.notify();
    }

    /// Cancel: the queued re-edits are taken out of the queue and nothing plays or exports.
    fn style_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.style.auto.as_mut() else { return };
        let ids = run.cancel();
        self.style.then = None;
        let q = crate::pipeline::snapshot();
        for id in ids {
            if let Some(item) = q.for_chapter(&id).filter(|i| i.state.active()) {
                crate::pipeline::cancel(item.id);
            }
        }
        toast::show(cx, Toast::info("Re-edit cancelled: nothing was played or exported"));
        cx.notify();
    }

    // ------------------------------------------------------------------------------ the panel

    /// The collapsible panel in the Video section.
    pub(crate) fn style_panel(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let open = self.style.prefs.open;
        if open {
            self.style_refresh(g, cx);
        }
        let locked = self.style.prefs.locked;
        let drafted = !locked && !self.style.prefs.draft.is_empty();
        let summary = match (locked, drafted) {
            (true, _) => "Locked · every video uses it",
            (_, true) => "Changed here · not applied yet",
            _ => "How every chapter video looks",
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(div().flex_1().min_w_0().child(crate::widgets::fold_head("vs-head", ph::PALETTE, "Video style", summary, open).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.style.prefs.open = !this.style.prefs.open;
                this.style.prefs.seen = true;
                this.style.prefs.save();
                cx.notify();
            }))))
            .child({
                let group = g.clone();
                div()
                    .id("vs-lock")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .tooltip(tip("Locked: the choices are the Studio-wide settings every chapter and future render uses, and Play and Export first re-edit the chapters whose video does not match (only those)."))
                    .child(icon_ph(if locked { ph::LOCK } else { ph::LOCK_OPEN }, if locked { Tokens::accent() } else { Tokens::text_3() }, icon_px::SM))
                    .child(muted("Use for all videos"))
                    .child(switch("vs-lock-switch", locked).on_click(cx.listener(move |this, on: &bool, _, cx| this.style_set_lock(&group, *on, cx))))
            });
        let mut panel = div().flex().flex_col().gap(px(space::SM)).pt(px(space::XS)).border_t_1().border_color(Tokens::border()).child(head);
        if !open {
            return panel.children(self.style_status(g, cx)).into_any_element();
        }
        let Some(s) = self.style_now(&g.id) else {
            return panel.child(div().flex().items_center().gap(px(space::SM)).child(spinner("vs-loading", Tokens::accent(), icon_px::SM)).child(muted("Reading the video settings…"))).into_any_element();
        };
        let body = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(space::LG))
            .child(self.style_preview_box(g, cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_1()
                    .min_w(px(320.))
                    .gap(px(space::LG))
                    .child(self.style_subtitles(g, &s, cx))
                    .child(self.style_framing(g, &s, cx))
                    .child(self.style_feel(g, &s, cx))
                    .child(self.style_output(cx)),
            );
        panel = panel.child(body);
        panel.children(self.style_status(g, cx)).into_any_element()
    }

    fn style_preview_box(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.style.preview;
        let (w, h) = if p.upright { (PREVIEW_W * 9. / 16., PREVIEW_W) } else { (PREVIEW_W, PREVIEW_W * 9. / 16.) };
        let picture = div()
            .relative()
            .w(px(w))
            .h(px(h))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border_strong())
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match &p.path {
                Some(path) => d.child(img(path.clone()).size_full().object_fit(ObjectFit::Contain)),
                None => d.child(icon_ph(ph::MONITOR_PLAY, Tokens::text_3(), icon_px::XL)),
            })
            .when(p.running, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(space::SM))
                        .bg(Tokens::canvas().alpha(0.55))
                        .child(spinner("vs-preview-spin", Tokens::accent(), icon_px::MD))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child("Rendering a frame…")),
                )
            });
        let note = match (&p.error, p.path.is_some()) {
            (Some(e), _) => div().text_size(px(size::XS)).text_color(Tokens::danger()).child(e.clone()),
            (None, true) => muted(format!("{} · a frame made by the video renderer with these choices (no layered motion, no sound)", p.label)),
            (None, false) => muted("A frame of a made chapter appears here, rendered with these choices."),
        };
        let group = g.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w(px(w))
            .flex_none()
            .child(picture)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::XS))
                    .child(div().flex_1().min_w_0().child(note))
                    .child(icon_button("vs-preview-again", ph::ARROW_CLOCKWISE, "Render the frame again").small().when(p.running, |b| b.disabled("Rendering…")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.style_preview_now(&group, cx);
                    }))),
            )
            .into_any_element()
    }

    fn style_subtitles(&mut self, g: &MangaGroup, s: &VideoStyle, cx: &mut Context<Self>) -> AnyElement {
        let current = crate::video::find_style(&s.sub_style);
        let category = self.style.category.clone().unwrap_or_else(|| current.category.clone());
        let cats = crate::video::catalog().categories.clone();
        let cat_chips = div().flex().flex_wrap().gap(px(4.)).children(cats.into_iter().enumerate().map(|(i, c)| {
            let on = c == category;
            filter_chip(("vs-sub-cat", i), c.clone(), on).h(px(24.)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.style.category = Some(c.clone());
                cx.notify();
            }))
        }));
        let templates = div().flex().flex_wrap().gap(px(4.)).children(crate::video::styles().iter().filter(|t| t.category == category).enumerate().map(|(i, t)| {
            let (id, group) = (t.id.clone(), g.clone());
            filter_chip(("vs-sub-style", i), t.label.clone(), t.id == s.sub_style).h(px(24.)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.style_edit(&group, |v| v.sub_style = id, cx);
            }))
        }));
        let (g1, g2, g3) = (g.clone(), g.clone(), g.clone());
        let body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(switch("vs-subs", s.subs).on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.style_edit(&g1, |v| v.subs = on, cx);
                    })))
                    .child(muted(if s.subs { format!("{} · {}", current.label, current.summary()) } else { "No subtitles".to_string() })),
            )
            .when(s.subs, |d| {
                d.child(cat_chips)
                    .child(templates)
                    .child(field(
                        "Size",
                        segmented("vs-sub-size", crate::restyle::SIZES.iter().map(|z| (None, z.0)), crate::restyle::size_index(s.sub_size))
                            .small()
                            .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                                let z = crate::restyle::SIZES[*ix].1;
                                this.style_edit(&g2, |v| v.sub_size = z, cx);
                            }))
                            .into_any_element(),
                    ))
                    .child(field(
                        "Position",
                        segmented("vs-sub-place", PLACES.iter().map(|p| (None, p.0)), s.sub_place)
                            .small()
                            .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                                let i = *ix;
                                this.style_edit(&g3, |v| v.sub_place = i, cx);
                            }))
                            .into_any_element(),
                    ))
            });
        group_box(ph::SUBTITLES, "Subtitles", body.into_any_element())
    }

    fn style_framing(&mut self, g: &MangaGroup, s: &VideoStyle, cx: &mut Context<Self>) -> AnyElement {
        let (g1, g2, g3) = (g.clone(), g.clone(), g.clone());
        let fits = [(Framing::Fit, "Fit whole panel"), (Framing::Fill, "Fill more of the frame")];
        let body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                segmented("vs-framing", fits.iter().map(|f| (None, f.1)), fits.iter().position(|f| f.0 == s.framing).unwrap_or(0))
                    .small()
                    .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                        let f = fits[*ix].0;
                        this.style_edit(&g1, |v| v.framing = f, cx);
                    })),
            )
            .child(muted(match s.framing {
                Framing::Fit => "Every panel whole, with room around it.",
                Framing::Fill => "Panels drawn larger, still whole: nothing narrated is ever cut.",
            }))
            .child(toggle("vs-duo", "Tall panels side by side", "Pairs tall panels in the two halves of the frame, like the big recap channels.", s.duo, cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.style_edit(&g2, |v| v.duo = on, cx);
            })))
            .child(field(
                "Camera motion",
                segmented("vs-motion", MOTIONS.iter().map(|m| (None, m.0)), style::motion_index(s.motion))
                    .small()
                    .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                        let m = MOTIONS[*ix].1;
                        this.style_edit(&g3, |v| v.motion = m, cx);
                    }))
                    .into_any_element(),
            ));
        group_box(ph::FRAME_CORNERS, "Framing", body.into_any_element())
    }

    fn style_feel(&mut self, g: &MangaGroup, s: &VideoStyle, cx: &mut Context<Self>) -> AnyElement {
        use mv_studio::vfx_pack::{self, PackState};
        let presets = div().flex().flex_wrap().gap(px(4.)).children(Preset::ALL.into_iter().map(|p| {
            let group = g.clone();
            div()
                .id(("vs-preset-tip", p as usize))
                .tooltip(tip(p.blurb()))
                .child(filter_chip(("vs-preset", p as usize), p.label(), s.preset == p).h(px(24.)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.style_edit(&group, |v| v.set_preset(p), cx))))
        }));
        let (g1, g2, g3, g4, g5, g6) = (g.clone(), g.clone(), g.clone(), g.clone(), g.clone(), g.clone());
        let (installed, _) = vfx_pack::cached();
        let layered_pick = segmented("vs-layered", Layered::ALL.map(Layered::label), s.layered as usize).small();
        let layered: AnyElement = if installed {
            layered_pick.on_change(cx.listener(move |this, ix: &usize, _, cx| {
                let l = Layered::ALL[(*ix).min(2)];
                this.style_edit(&g4, |v| v.layered = l, cx);
            }))
            .into_any_element()
        } else {
            let busy = matches!(vfx_pack::state(), Some(PackState::Running(_) | PackState::Removing));
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::SM))
                .child(div().opacity(0.45).child(layered_pick))
                .child(
                    btn("vs-layered-install", if busy { "Installing…" } else { "Install" }, ButtonKind::Ghost)
                        .small()
                        .icon(ph::DOWNLOAD_SIMPLE)
                        .tooltip("Layered motion needs its pack (PyTorch with CUDA and three open models), installed from Look & branding's sources")
                        .when(busy, |b| b.disabled("Installing the pack…"))
                        .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                            vfx_pack::clear_error();
                            cx.spawn(async move |this, cx| {
                                let res = mv_ui::bg::run(vfx_pack::install()).await;
                                let _ = this.update(cx, |_, cx| {
                                    match res {
                                        Ok(Ok(())) => toast::show(cx, Toast::success("Layered motion pack installed")),
                                        Ok(Err(mv_studio::StudioError::Cancelled)) => toast::show(cx, Toast::info("Pack install stopped. Downloads resume next time.")),
                                        Ok(Err(_)) | Err(_) => toast::show(cx, Toast::error("The layered motion pack did not install")),
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        let sfx = [SfxAmount::Off, SfxAmount::Subtle, SfxAmount::Punchy];
        let body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(presets)
            .child(toggle("vs-mood", "Mood-driven edit", crate::look::Effect::Mood.tip(), s.mood, cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.style_edit(&g1, |v| v.mood = on, cx);
            })))
            .child(toggle("vs-edge", "Edge glow", crate::look::Effect::EdgeGlow.tip(), s.edge_glow, cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.style_edit(&g2, |v| v.edge_glow = on, cx);
            })))
            .child(toggle("vs-particles", "Particles", crate::look::Effect::Particles.tip(), s.particles, cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.style_edit(&g3, |v| v.particles = on, cx);
            })))
            .child(field("Layered motion", layered))
            .child(field(
                "Sound effects",
                segmented("vs-sfx", sfx.iter().map(|x| (None, x.label())), sfx.iter().position(|x| *x == s.sfx).unwrap_or(1))
                    .small()
                    .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                        let x = sfx[*ix];
                        this.style_edit(&g5, |v| v.sfx = x, cx);
                    }))
                    .into_any_element(),
            ))
            .child(field(
                "Music",
                segmented("vs-music", MUSIC.iter().map(|m| (None, *m)), s.music)
                    .small()
                    .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                        let m = *ix;
                        this.style_edit(&g6, |v| v.music = m, cx);
                    }))
                    .into_any_element(),
            ));
        group_box(ph::SPARKLE, "Feel", body.into_any_element())
    }

    /// The export's own choices (used when saving, not part of the chapter videos).
    fn style_output(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let prefs = self.export_prefs;
        let mode = prefs.mode();
        let kinds = [(ph::FILM_STRIP, "One video"), (ph::SQUARES_FOUR, "Parts"), (ph::DEVICE_MOBILE, "Reels · 1 min")];
        let mark_ready = self.style.look.logo.path.is_some() || !self.style.look.text.text.trim().is_empty();
        let mark_on = (self.style.look.logo.on && self.style.look.logo.path.is_some()) || (self.style.look.text.on && !self.style.look.text.text.trim().is_empty());
        let body = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(segmented("vs-kind", kinds.iter().map(|&(i, l)| (Some(i), l)), prefs.kind).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.export_prefs.kind = *ix;
                this.save_export_prefs();
                cx.notify();
            })))
            .when(matches!(mode, Mode::Parts(_)), |d| {
                d.child(segmented("vs-parts", PART_COUNTS.iter().map(|p| (None, p.to_string())), PART_COUNTS.iter().position(|p| *p == prefs.parts).unwrap_or(3)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                    this.export_prefs.parts = PART_COUNTS[*ix];
                    this.save_export_prefs();
                    cx.notify();
                })))
            })
            .when(mode == Mode::Whole, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::XS))
                        .child(switch("vs-hook", prefs.hook).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.export_prefs.hook = *on;
                            this.save_export_prefs();
                            cx.notify();
                        })))
                        .child(div().id("vs-hook-label").tooltip(tip("AI picks the most gripping moments, writes and voices a teaser, and puts it before chapter 1.")).child(muted("Hook")))
                        .when(prefs.hook, |d| {
                            d.child(segmented("vs-hook-secs", HOOK_SECONDS.iter().map(|s| (None, format!("{s}s"))), HOOK_SECONDS.iter().position(|s| *s == prefs.hook_secs).unwrap_or(2)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                                this.export_prefs.hook_secs = HOOK_SECONDS[*ix];
                                this.save_export_prefs();
                                cx.notify();
                            })))
                        }),
                )
            })
            .child(toggle(
                "vs-mark",
                if mark_ready { "Watermark" } else { "Watermark (set one up in Look & branding)" },
                "The logo and text watermark from Look & branding, drawn on every export.",
                mark_on,
                cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    if !mark_ready {
                        toast::show(cx, Toast::info("Add a logo or a text watermark in Studio → Video → Look & branding first"));
                        return;
                    }
                    let mut l = Look::load();
                    l.logo.on = on && l.logo.path.is_some();
                    l.text.on = on && !l.text.text.trim().is_empty();
                    match l.save() {
                        Ok(()) => this.style.look = l,
                        Err(e) => toast::show(cx, Toast::error(format!("Could not save the watermark: {e}"))),
                    }
                    cx.notify();
                }),
            ))
            .child(toggle("vs-export-music", "Music in the export", "Off: narration only (the videos carry a music-free track).", prefs.music, cx.listener(|this, on: &bool, _, cx| {
                this.export_prefs.music = *on;
                this.save_export_prefs();
                cx.notify();
            })))
            .child(field(
                "Quality",
                segmented("vs-quality", Quality::ALL.iter().map(|q| (None, q.label())), Quality::ALL.iter().position(|q| *q == prefs.quality).unwrap_or(0))
                    .small()
                    .on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.export_prefs.quality = Quality::ALL[*ix];
                        this.save_export_prefs();
                        cx.notify();
                    }))
                    .into_any_element(),
            ));
        group_box(ph::EXPORT, "Output (export only)", body.into_any_element())
    }

    /// Under the panel: the re-edit in progress (with Cancel), else what Play / Export will do.
    fn style_status(&mut self, g: &MangaGroup, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(run) = self.style.auto.as_ref().filter(|r| r.manga_id == g.id && r.running()) {
            let p = match run.state {
                AutoState::Rendering { progress, .. } => progress,
                _ => 0.,
            };
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().child(crate::phases::progress_line("vs-auto", p, run.label())))
                    .child(btn("vs-auto-cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).tooltip("Stop the re-edit: nothing is played or exported").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.style_cancel(cx))))
                    .into_any_element(),
            );
        }
        let locked = self.style.prefs.locked;
        let stale = self.style.stale.as_ref().filter(|(m, _)| *m == g.id).map(|(_, n)| *n);
        let text = match (locked, !self.style.prefs.draft.is_empty(), stale) {
            (true, _, Some(0)) => "Every made chapter matches this style.".to_string(),
            (true, _, Some(n)) => format!("{n} made chapter{} do{} not match this style yet: Play and Export re-edit {} first (only {}).", if n == 1 { "" } else { "s" }, if n == 1 { "es" } else { "" }, if n == 1 { "it" } else { "them" }, if n == 1 { "that one" } else { "those" }),
            (true, _, None) => "Locked: Play and Export first re-edit the chapters whose video does not match.".to_string(),
            (false, true, _) => "These choices are only previewed. Turn on \"Use for all videos\" to save them for every chapter.".to_string(),
            (false, false, Some(n)) if n > 0 => format!("{n} made chapter{} changed since rendered: Play and Export ask whether to re-edit {} first.", if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" }),
            (false, false, _) => "Unlocked: Play and Export ask before re-editing changed chapters.".to_string(),
        };
        // Locked, a caption or music choice replaces what chapters set for themselves: say so.
        let custom = self.style.custom.as_ref().filter(|(m, n)| locked && *m == g.id && *n > 0).map(|(_, n)| *n);
        let group = g.clone();
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::SM))
                .child(icon_ph(if locked { ph::LOCK } else { ph::INFO }, Tokens::text_3(), icon_px::SM))
                .child(div().flex_1().min_w(px(200.)).text_size(px(size::XS)).text_color(Tokens::text_2()).child(text))
                .when_some(custom, |d, n| {
                    d.child(div().w_full().text_size(px(size::XS)).text_color(Tokens::warning()).child(format!(
                        "{n} chapter{} {} {} own captions or music: changing Subtitles, Sound effects or Music here replaces {} on every chapter.",
                        if n == 1 { "" } else { "s" },
                        if n == 1 { "has" } else { "have" },
                        if n == 1 { "its" } else { "their" },
                        if n == 1 { "it" } else { "them" },
                    )))
                })
                .when(!locked && !self.style.prefs.draft.is_empty(), |d| {
                    d.child(btn("vs-discard", "Discard changes", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.style_discard(&group, cx))))
                })
                .into_any_element(),
        )
    }
}

/// Renders one preview frame (or takes it from the cache): the config the render would get, its
/// key, then `style_preview.py`.
async fn render_frame(s: VideoStyle, rep: ChapterItem, reels: bool, py: mv_studio::PythonInterpreter, layout: mv_studio::PythonLayout) -> Result<PathBuf> {
    let (args_file, out) = mv_ui::bg::blocking(move || -> Result<(Option<PathBuf>, PathBuf)> {
        let cfg = style::preview_config(&s, &rep, reels);
        let key = style::preview_key(&rep.id, &cfg);
        let dir = style::preview_dir();
        let out = style::preview_file(&dir, key);
        if out.is_file() {
            return Ok((None, out));
        }
        std::fs::create_dir_all(&dir)?;
        let args = style::preview_args(&rep, &cfg, &dir.join("work"), &out);
        let file = dir.join("args.json");
        std::fs::write(&file, serde_json::to_string(&args)?)?;
        Ok((Some(file), out))
    })
    .await??;
    let Some(file) = args_file else { return Ok(out) };
    let script = layout.script("style_preview.py").map_err(|_| anyhow!("This version of Studio's scripts has no preview yet (update the app)"))?;
    let dir = layout.dir.clone();
    let arg = file.to_string_lossy().into_owned();
    let (code, stdout, stderr) = mv_ui::bg::run(async move { mv_studio::bridge::run_capture(&py, &script, &[arg], &dir).await }).await?.map_err(|e| anyhow!("{e}"))?;
    let last = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let v: Value = serde_json::from_str(last).unwrap_or(Value::Null);
    if v["ok"].as_bool() == Some(true) && out.is_file() {
        let d = style::preview_dir();
        let _ = mv_ui::bg::blocking(move || style::prune_previews(&d)).await;
        return Ok(out);
    }
    let why = v["message"].as_str().map(str::to_string).or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string)).unwrap_or_else(|| format!("the preview exited {code:?}"));
    bail!("Could not render the preview: {why}")
}

/// A titled group of choices.
fn group_box(icon: Ph, title: &'static str, body: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(space::SM))
        .w(px(300.))
        .flex_none()
        .child(div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(title))
        .child(body)
        .into_any_element()
}

/// A labelled control.
fn field(label: &'static str, control: AnyElement) -> AnyElement {
    div().flex().flex_col().gap(px(3.)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(label)).child(control).into_any_element()
}

/// A switch with its label (and a tooltip).
fn toggle(id: &'static str, label: &'static str, hint: &'static str, on: bool, on_click: impl Fn(&bool, &mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(space::SM))
        .child(div().id(SharedString::from(format!("{id}-label"))).text_size(px(size::SM)).text_color(Tokens::text_2()).tooltip(tip(hint)).child(label))
        .child(switch(id, on).on_click(on_click))
        .into_any_element()
}

