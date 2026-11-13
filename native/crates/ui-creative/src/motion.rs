//! Video: animate a painted chapter panel by panel with the picked video model (FFmpeg on this
//! PC, a local ComfyUI workflow, or an API model), then stitch the clips in reading order. The
//! page shows every chapter as a small card, the open chapter's steps with one button that makes
//! what is left, where it is all saved, and a Delete menu to make any of it again.
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::video::{self, VideoVia, VIDEO_MODELS};
use mv_create::{existing, Clear, Cleared, Project, Proposal};
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_ph, menu_button, ph, segmented, size, AlertKind, ButtonKind, MenuItem, StepMark, Tokens};

use crate::jobs::{self, Task};
use crate::page::CreativePage;

/// A chapter card's width and height, the gap between cards, and how many rows of them show
/// before the list scrolls.
const CARD: (f32, f32) = (124., 68.);
const CARD_GAP: f32 = 8.;
const CARD_ROWS: f32 = 3.;

/// What each of the five steps does, for its tooltip.
const STEP_TIPS: [&str; 5] = [
    "The app reads the manga: who is in it and what happens.",
    "A sheet of every main character and a picture of every place. Every scene is made from these, so faces and places stay the same.",
    "The AI writes the chapter scene by scene: what is seen, who speaks, how it moves.",
    "A picture is made of every scene.",
    "Each scene is voiced and filmed from its picture, then the clips are joined into the video.",
];

/// A delete the Video page offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClearAsk {
    Chapter(u32, Clear),
    /// Everything made for every chapter, the written scenes included.
    Chapters,
    /// Every character sheet, expression sheet and place picture.
    Sheets,
}

impl ClearAsk {
    /// What goes, as it reads after "Delete" and "Deleted".
    fn name(self) -> String {
        match self {
            ClearAsk::Chapter(n, Clear::Film) => format!("chapter {n}'s video and clips"),
            ClearAsk::Chapter(n, Clear::Pictures) => format!("chapter {n}'s pictures, clips and video"),
            ClearAsk::Chapter(n, Clear::All) => format!("everything made for chapter {n}"),
            ClearAsk::Chapters => "everything made for every chapter".into(),
            ClearAsk::Sheets => "the character sheets and place pictures".into(),
        }
    }

    /// What stays, and what happens next.
    fn consequence(self) -> &'static str {
        match self {
            ClearAsk::Chapter(_, Clear::Film) => "Its scene pictures and written scenes stay, so the clips can be filmed again.",
            ClearAsk::Chapter(_, Clear::Pictures) => "Its written scenes stay, so the pictures and clips can be made again.",
            ClearAsk::Chapter(_, Clear::All) => "The chapter starts over: its scenes are written again, then pictured and filmed.",
            ClearAsk::Chapters => "Every chapter starts over. The characters, their sheets and the story stay.",
            ClearAsk::Sheets => "The characters keep their descriptions; their sheets and the places are made again before the next pictures.",
        }
    }

    /// Bytes and files it frees.
    fn disk(self, p: &Project) -> (u64, usize) {
        match self {
            ClearAsk::Chapter(n, what) => p.chapter_disk(n, what),
            ClearAsk::Chapters => p.chapters_disk(),
            ClearAsk::Sheets => p.sheets_disk(),
        }
    }

    fn run(self, p: &mut Project) -> Cleared {
        match self {
            ClearAsk::Chapter(n, what) => p.clear_chapter(n, what),
            ClearAsk::Chapters => p.clear_chapters(),
            ClearAsk::Sheets => p.clear_sheets(),
        }
    }
}

/// What the open chapter and the whole project take on disk.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiskUse {
    /// The project, the chapter and the project's change it was measured for.
    pub key: (String, u32, u64),
    pub chapter: u64,
    pub project: u64,
}

impl CreativePage {
    /// Video model, clip length, frame shape (and the ComfyUI workflow when that is picked).
    pub(crate) fn video_bar(&self, cx: &mut Context<Self>) -> Div {
        let Some(p) = self.p.as_ref() else { return div() };
        let vm = video::video_model(&p.settings.video_model);
        let weak = cx.entity().downgrade();
        let items = VIDEO_MODELS
            .iter()
            .map(|m| {
                let w = weak.clone();
                // From the keys read once per change (fal.ai is one of the image services too),
                // not from the key files on every repaint.
                let ready = match m.via {
                    VideoVia::Magnific => self.keys.magnific,
                    VideoVia::Service(id) => self.keys.services.contains(&id),
                    _ => true,
                };
                MenuItem::new(format!("{}{}", m.name, if ready { "" } else { " · needs a key" }), move |_, cx| {
                    let _ = w.update(cx, |this, cx| {
                        this.edit(|p| p.settings.video_model = m.id.to_string());
                        cx.notify();
                    });
                })
                .icon(if m.is_local() { ph::CUBE } else { ph::FILM_STRIP })
                .checked(m.id == vm.id)
            })
            .collect();
        let secs = vm.fit_seconds(p.settings.clip_seconds);
        let sec_ix = vm.seconds.iter().position(|s| *s == secs).unwrap_or(0);
        let workflow = p.settings.comfy_workflow.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(menu_button("cr-vmodel", ph::FILM_STRIP, "Video model", items).label(vm.name).small().min_width(320.))
                    .child(segmented("cr-secs", vm.seconds.iter().map(|s| (None::<mv_ui::Ph>, format!("{s} s"))).collect::<Vec<_>>(), sec_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                        let Some(p) = this.p.as_ref() else { return };
                        let vm = video::video_model(&p.settings.video_model);
                        let s = vm.seconds.get(*ix).copied().unwrap_or(5);
                        this.edit(|p| p.settings.clip_seconds = s);
                        cx.notify();
                    })))
                    .child(segmented("cr-vaspect", ["16:9", "9:16"], usize::from(p.settings.video_aspect == "9:16")).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.edit(|p| p.settings.video_aspect = if *ix == 1 { "9:16".into() } else { "16:9".into() });
                        cx.notify();
                    })))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(vm.note)),
            )
            .when(vm.via == VideoVia::ComfyUi, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(btn("cr-comfy", if workflow.is_empty() { "Pick workflow (API format JSON)…" } else { "Change workflow…" }, ButtonKind::Secondary).small().icon(ph::FOLDER_OPEN).on_click(cx.listener(|this, _, _, cx| this.pick_workflow(cx))))
                        .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(if workflow.is_empty() {
                            format!("ComfyUI at {}. In ComfyUI use Save (API Format); put {{{{image}}}} in the Load Image node, {{{{prompt}}}} in the text and {{{{frames}}}} for the length.", p.settings.comfy_url)
                        } else {
                            workflow.clone()
                        })),
                )
            })
            .when(vm.via == VideoVia::ComfyLtx, |d| {
                d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                    "Uses the LTX 2.3 image + audio workflow you installed in ComfyUI (at {}), built in: no workflow file to pick. Keep ComfyUI open. Each scene's lines are spoken with Studio's voice, then LTX films the scene from its picture so the mouths follow that voice; the clip is as long as the voice (up to 10 s). A scene waits its turn behind anything ComfyUI is already making.",
                    p.settings.comfy_url
                )))
            })
            .when(vm.via == VideoVia::Local && self.ffmpeg == Some(false), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("FFmpeg is not installed: clips and stitching need it (winget install Gyan.FFmpeg).")))
    }

    fn pick_workflow(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this workflow".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(files))) = rx.await else { return };
            let Some(f) = files.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.edit(|p| p.settings.comfy_workflow = f.to_string_lossy().to_string());
                cx.notify();
            });
        })
        .detach();
    }

    /// Picks a clip to give notes to: its notes go in the box.
    fn pick_clip(&mut self, chapter: u32, page: u32, panel: usize, window: &mut Window, cx: &mut Context<Self>) {
        let notes = self.p.as_ref().and_then(|p| p.chapter(chapter)).and_then(|c| c.pages.iter().find(|x| x.number == page)).and_then(|pg| pg.panels.get(panel)).map(|x| x.notes.clone()).unwrap_or_default();
        if let Some(t) = &self.clip_notes {
            t.update(cx, |s, cx| {
                s.set_value(notes, window, cx);
                s.focus(window, cx);
            });
        }
        self.clip_sel = Some((page, panel));
        cx.notify();
    }

    /// Keeps the notes typed for a clip and makes it again with them (then the chapter video).
    fn redo_clip(&mut self, chapter: u32, page: u32, panel: usize, cx: &mut Context<Self>) {
        let notes = self.clip_notes.as_ref().map(|t| t.read(cx).value().trim().to_string()).unwrap_or_default();
        self.edit(|p| {
            if let Some(x) = p.chapter_mut(chapter).pages.iter_mut().find(|x| x.number == page).and_then(|pg| pg.panels.get_mut(panel)) {
                x.notes = notes;
            }
        });
        self.enqueue(vec![Task::Clip { chapter, page, panel }, Task::Stitch { chapter }], cx);
    }

    /// Deletes one scene's clip and its voice from disk; Continue or Make films it again.
    fn delete_clip(&mut self, chapter: u32, page: u32, panel: usize, cx: &mut Context<Self>) {
        if self.busy() {
            self.error = Some("Stop what is being made first, then delete the clip.".into());
            cx.notify();
            return;
        }
        let mut done = Cleared::default();
        self.edit(|p| done = p.clear_clip(chapter, page, panel));
        self.disk_for = None;
        let what = format!("the clip of page {page}, scene {}", panel + 1);
        toast::show(cx, if done.kept > 0 { Toast::warning(format!("Could not delete {what}: it is open in another app. Close it and delete again.")) } else { Toast::success(format!("Deleted {what}")).icon(ph::TRASH) });
        cx.notify();
    }

    /// The picked clip: what the scene is, its lines, the notes box and Make again.
    fn clip_editor(&self, p: &Project, chapter: u32, page: u32, i: usize, cx: &mut Context<Self>) -> Option<Div> {
        let x = p.chapter(chapter)?.pages.iter().find(|pg| pg.number == page)?.panels.get(i)?.clone();
        let clip = existing(p, x.clip.as_ref());
        let has_clip = x.clip.is_some();
        let title = format!("{} · chapter {chapter} · page {page} · scene {}", p.title, i + 1);
        let lines: Vec<String> = x.lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| format!("{}: {}", if l.speaker.is_empty() { "Narrator" } else { &l.speaker }, l.text.trim())).collect();
        let queued = self.queue.contains(&Task::Clip { chapter, page, panel: i }) || self.running.as_ref().is_some_and(|r| r.label == format!("Chapter {chapter} · page {page} · panel {}", i + 1));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::SM))
                .p(px(space::MD))
                .rounded(px(radius::LG))
                .bg(Tokens::surface_2())
                .border_1()
                .border_color(Tokens::accent())
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Page {page} · scene {}", i + 1)))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(3).child(if x.motion.is_empty() { x.description.clone() } else { format!("{} Moves: {}", x.description, x.motion) }))
                .when(!lines.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(4).child(format!("Spoken: {}", lines.join(" / ")))))
                .child(prompt_box("What the picture is made from", mv_create::prompts::panel(p, &x, &p.settings.video_aspect, &[])))
                .child(prompt_box("What the clip is filmed from", mv_create::prompts::motion(p, &x)))
                .children(self.clip_notes.as_ref().map(|t| Textarea::new(t)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::SM))
                        .child(btn("cr-clip-redo", if has_clip { "Make this clip again with my notes" } else { "Make this clip with my notes" }, ButtonKind::Primary).small().icon(ph::ARROWS_CLOCKWISE).loading(queued).tooltip("The notes are sent to the video model with the scene; the chapter video is stitched again after").on_click(cx.listener(move |this, _, _, cx| this.redo_clip(chapter, page, i, cx))))
                        .when_some(clip, |d, f| {
                            d.child(btn("cr-clip-play", "Play this clip", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(move |_, window, cx| {
                                let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                                mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
                            }))
                        })
                        .when(has_clip, |d| d.child(btn("cr-clip-delete", "Delete this clip", ButtonKind::DangerGhost).small().icon(ph::TRASH).tooltip("Removes this scene's clip and its voice from this PC. Continue or Make films it again.").on_click(cx.listener(move |this, _, _, cx| this.delete_clip(chapter, page, i, cx)))))
                        .child(btn("cr-clip-close", "Close", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| {
                            this.clip_sel = None;
                            cx.notify();
                        }))),
                ),
        )
    }

    /// Opens chapter `n`. One the project has not met yet is added, with what Studio read of it
    /// as its summary, so its scenes are written from the real story.
    fn open_chapter(&mut self, n: u32, cx: &mut Context<Self>) {
        if self.busy() && self.auto.is_some() {
            return;
        }
        self.chapter = n;
        self.clip_sel = None;
        self.confirm_clear = None;
        let known = self.p.as_ref().is_some_and(|p| p.chapter(n).is_some_and(|c| !c.summary.trim().is_empty() || !c.pages.is_empty()));
        if !known {
            let title = self.p.as_ref().and_then(|p| p.adapts.clone());
            self.edit(|p| {
                p.chapter_mut(n);
            });
            if let Some(title) = title {
                let pid = self.project_id();
                cx.spawn(async move |this, cx| {
                    let found = mv_ui::bg::blocking(move || chapter_recap(&title, n)).await.ok().flatten();
                    let _ = this.update(cx, |this, cx| {
                        if let (Some(summary), true) = (found, this.project_id() == pid) {
                            this.edit(|p| {
                                let ch = p.chapter_mut(n);
                                if ch.summary.trim().is_empty() {
                                    ch.summary = summary;
                                }
                            });
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// One chapter as a small card: its number, where its video stands in words, and its four
    /// steps (scenes, pictures, clips, video) as marks, the one being made now moving.
    fn chapter_card(&self, p: &Project, n: u32, open: u32, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = open == n;
        let (line, done) = chapter_state(p.chapter(n));
        let working = self.busy() && (self.auto == Some(n) || self.running.as_ref().is_some_and(|r| r.label.starts_with(&format!("Chapter {n} "))));
        let mut next = working;
        let marks: Vec<(&'static str, StepMark)> = ["Scenes", "Pictures", "Clips", "Video"]
            .into_iter()
            .zip(done)
            .map(|(name, d)| (name, if d { StepMark::Done } else if std::mem::take(&mut next) { StepMark::Running } else { StepMark::Todo }))
            .collect();
        div()
            .id(("cr-vch", n as usize))
            .flex_none()
            .w(px(CARD.0))
            .h(px(CARD.1))
            .flex()
            .flex_col()
            .justify_between()
            .px(px(10.))
            .py(px(space::SM))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(if selected { Tokens::accent() } else { Tokens::border() })
            .bg(if selected { Tokens::accent_soft() } else { Tokens::surface_2() })
            .cursor_pointer()
            .when(!selected, |d| d.hover(|s| s.bg(Tokens::hover()).border_color(Tokens::border_strong())))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Chapter {n}")))
                    .when(done[3], |d| d.child(icon_ph(ph::CHECK_CIRCLE, Tokens::success(), 14.))),
            )
            .child(div().truncate().text_size(px(size::XS)).text_color(if done[0] { Tokens::text_2() } else { Tokens::text_3() }).child(line))
            .child(mv_ui::step_cards_small(format!("cr-vch-steps-{n}"), marks))
            .on_click(cx.listener(move |this, _, _, cx| this.open_chapter(n, cx)))
    }

    /// Every chapter there is to make a video of, as cards: the manga's downloaded chapters (all
    /// of them, not only the ones the outline covers) and the project's own. A click opens one.
    fn video_chapters(&mut self, p: &Project, open: u32, cx: &mut Context<Self>) -> Div {
        let norm = |s: &str| s.trim().to_lowercase();
        let downloaded: Vec<u32> = p.adapts.as_ref().and_then(|t| self.library.iter().find(|m| norm(&m.title) == norm(t))).map(|m| m.chapters.clone()).unwrap_or_default();
        let mut all: Vec<u32> = downloaded.iter().copied().chain(p.chapters.iter().map(|c| c.number)).collect();
        all.sort_unstable();
        all.dedup();
        // A chapter the project has not met yet is forecast as the usual chapter: once for all of them.
        let usual = jobs::forecast(p, u32::MAX);
        let (mut video, mut left, mut made) = (0u64, 0u64, 0usize);
        for n in &all {
            let f = if p.chapter(*n).is_some() { jobs::forecast(p, *n) } else { usual };
            video += f.video_secs;
            left += f.left_secs;
            made += usize::from(chapter_state(p.chapter(*n)).1[3]);
        }
        // The open chapter's card is brought into view when the chapter, the project, the list
        // (the downloaded chapters arrive a moment after the page) or the window's width (the
        // cards flow into other rows) changes; a click on a card already in view moves nothing.
        let at = (p.id.clone(), open, all.len(), self.view_w);
        if self.chapters_at.as_ref() != Some(&at) {
            if let Some(ix) = all.iter().position(|n| *n == open) {
                self.chapters_scroll.scroll_to_item(ix);
            }
            self.chapters_at = Some(at);
        }
        let rows_h = CARD_ROWS * CARD.1 + (CARD_ROWS - 1.) * CARD_GAP;
        let cards: Vec<Stateful<Div>> = all.iter().map(|n| self.chapter_card(p, *n, open, cx)).collect();
        let totals = format!("{made} of {} videos made · all together {} of video, {} left to make", all.len(), jobs::about(video), if left == 0 { "nothing".to_string() } else { jobs::about(left) });
        div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(mv_ui::section_label(if downloaded.is_empty() { "Chapters".to_string() } else { format!("Chapters · {} downloaded", downloaded.len()) }))
                    .child(div().flex_1())
                    .child(div().pb(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(totals)),
            )
            // A few rows of cards that scroll on their own, with a scrollbar to show there are more.
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .max_h(px(rows_h))
                    .child(div().id("cr-vch-scroll").flex().flex_wrap().gap(px(CARD_GAP)).pr(px(space::LG)).max_h(px(rows_h)).overflow_y_scroll().track_scroll(&self.chapters_scroll).children(cards))
                    .vertical_scrollbar(&self.chapters_scroll),
            )
    }

    /// Asks before a delete: measures what it frees, then shows the question where the button is.
    fn ask_clear(&mut self, ask: ClearAsk, cx: &mut Context<Self>) {
        let Some(p) = self.p.clone() else { return };
        if self.busy() {
            self.error = Some("Stop what is being made first, then delete.".into());
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            let pid = p.id.clone();
            let (bytes, files) = mv_ui::bg::blocking(move || ask.disk(&p)).await.unwrap_or((0, 0));
            let _ = this.update(cx, |this, cx| {
                if this.project_id().as_deref() == Some(pid.as_str()) {
                    this.confirm_clear = Some((pid, ask, bytes, files));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Does the delete that was confirmed, on the project it was asked for.
    fn do_clear(&mut self, cx: &mut Context<Self>) {
        let Some((pid, ask, ..)) = self.confirm_clear.take() else { return };
        if self.project_id().as_deref() != Some(pid.as_str()) {
            cx.notify();
            return;
        }
        if self.busy() {
            self.error = Some("Stop what is being made first, then delete.".into());
            cx.notify();
            return;
        }
        let mut done = Cleared::default();
        self.edit(|p| done = ask.run(p));
        self.clip_sel = None;
        self.auto = None;
        self.auto_last = None;
        self.disk_for = None;
        let what = ask.name();
        let freed = if done.bytes == 0 { String::new() } else { format!(" · {} freed", size_label(done.bytes)) };
        self.note(format!("Deleted {what}{freed}."));
        toast::show(
            cx,
            if done.kept > 0 {
                Toast::warning(format!("Deleted {what}{freed}, but {} file{} open in another app stayed. Close it and delete again.", done.kept, if done.kept == 1 { "" } else { "s" }))
            } else {
                Toast::success(format!("Deleted {what}{freed}")).icon(ph::TRASH)
            },
        );
        cx.notify();
    }

    /// Measures what the open chapter and the project take on disk, off the UI thread, whenever
    /// the project, the chapter or what is made changed.
    fn measure_disk(&mut self, p: &Project, n: u32, cx: &mut Context<Self>) {
        let key = (p.id.clone(), n, p.updated);
        if self.disk_for.as_ref() == Some(&key) {
            return;
        }
        self.disk_for = Some(key.clone());
        let p = p.clone();
        cx.spawn(async move |this, cx| {
            let Ok(found) = mv_ui::bg::blocking(move || DiskUse { chapter: p.chapter_disk(n, Clear::All).0, project: p.disk(), key }).await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.disk_for.as_ref() == Some(&found.key) {
                    this.disk = Some(found);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where the project is saved and how much it takes, a menu that opens its folders, and the
    /// Delete menu (with its question, once something was picked).
    fn storage(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Div {
        let dir = p.dir();
        let sizes = match self.disk.as_ref().filter(|d| d.key.0 == p.id && d.key.1 == n) {
            Some(d) => format!("Chapter {n}: {} · whole project: {}", size_label(d.chapter), size_label(d.project)),
            None => "Measuring…".to_string(),
        };
        let mut folders: Vec<MenuItem> = vec![
            MenuItem::new("The project folder", {
                let d = dir.clone();
                move |_, _| open_folder(&d)
            })
            .icon(ph::FOLDER_OPEN),
            MenuItem::separator(),
            MenuItem::heading("Inside it"),
        ];
        for (sub, what) in mv_create::MADE_DIRS.into_iter().chain([(mv_create::SHEETS_DIR, "Character sheets and places")]) {
            let d = dir.join(sub);
            folders.push(MenuItem::new(format!("{what} · {sub}"), move |_, _| open_folder(&d)).icon(ph::FOLDER));
        }
        let weak = cx.entity().downgrade();
        let ask = |a: ClearAsk, label: &str, icon: mv_ui::Ph| {
            let w = weak.clone();
            MenuItem::new(label.to_string(), move |_, cx| {
                let _ = w.update(cx, |this, cx| this.ask_clear(a, cx));
            })
            .icon(icon)
        };
        let delete = vec![
            MenuItem::heading(format!("Chapter {n}")),
            ask(ClearAsk::Chapter(n, Clear::Film), "Video and clips", ph::FILM_STRIP),
            ask(ClearAsk::Chapter(n, Clear::Pictures), "Pictures, clips and video", ph::IMAGES),
            ask(ClearAsk::Chapter(n, Clear::All), "Everything, to start the chapter over", ph::ARROWS_CLOCKWISE),
            MenuItem::separator(),
            MenuItem::heading("Whole project"),
            ask(ClearAsk::Chapters, "Everything made for every chapter", ph::TRASH),
            ask(ClearAsk::Sheets, "Character sheets and place pictures", ph::USERS),
        ];
        let confirm = self.confirm_clear.as_ref().filter(|(pid, ..)| *pid == p.id).map(|(_, ask, bytes, files)| {
            let frees = if *files == 0 { "None of it is on this PC, so no space is freed.".to_string() } else { format!("Frees {} ({files} file{}). It cannot be undone.", size_label(*bytes), if *files == 1 { "" } else { "s" }) };
            mv_ui::inline_alert(AlertKind::Warning, format!("Delete {}?", ask.name()), format!("{} {frees}", ask.consequence())).action("Delete", cx.listener(|this, _, _, cx| this.do_clear(cx))).dismissible(cx.listener(|this, _, _, cx| {
                this.confirm_clear = None;
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .pt(px(space::MD))
            .border_t_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(ph::HARD_DRIVE, Tokens::text_3(), 18.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(260.))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Saved on this PC · {sizes}")))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(dir.display().to_string())),
                    )
                    .child(menu_button("cr-folders", ph::FOLDER_OPEN, "Open the folders the pictures, clips, voices and videos are kept in", folders).label("Open folder").small().min_width(300.))
                    .child(menu_button("cr-delete", ph::TRASH, "Delete what was made, to make it again", delete).label("Delete").small().min_width(300.)),
            )
            .children(confirm)
    }

    /// The open chapter: its five steps from manga to video (done, being made now, to do), the
    /// one button that makes everything that is left, and where it is all saved.
    fn chapter_panel(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Div {
        let stage = jobs::stage(p, n);
        let at = stage.index();
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let opening = p.opening_cast();
        let with_sheets = opening.iter().filter(|name| p.character(name).is_some_and(|c| c.has_sheets())).count();
        let places = p.locations.iter().filter(|l| l.sheet.is_some()).count();
        let scenes: usize = ch.pages.iter().map(|pg| pg.panels.len()).sum();
        let clips = ch.pages.iter().flat_map(|pg| &pg.panels).filter(|x| x.clip.is_some()).count();
        let busy = self.busy();
        let auto = self.auto.is_some();
        let rows: [(&str, String); 5] = [
            ("Characters and story", if p.cast.is_empty() { "Read from the manga".into() } else { format!("{} characters · {} chapters", p.cast.len(), p.chapters.len()) }),
            ("Sheets and places", format!("{with_sheets} of {} characters · {places} of {} places", opening.len(), p.locations.len())),
            ("Scenes written", if scenes == 0 { "Not written yet".into() } else { format!("{scenes} scenes on {} pages", ch.pages.len()) }),
            ("Scene pictures", if ch.pages.is_empty() { "After the scenes".into() } else { format!("{} of {} pages pictured", ch.painted(), ch.pages.len()) }),
            ("Voices, clips and video", if at == 5 { "Ready to play".into() } else if scenes == 0 { "After the pictures".into() } else { format!("{clips} of {scenes} scenes filmed") }),
        ];
        let now = self.running.as_ref().map(|r| format!("{} · {}", r.label, r.status.lock().map(|s| s.clone()).unwrap_or_default())).or_else(|| self.thinking.as_ref().map(|s| s.lock().map(|s| if s.is_empty() { "The AI is writing".to_string() } else { s.clone() }).unwrap_or_default()));
        let fc = jobs::forecast(p, n);
        let forecast = format!("{}{} scenes · a video of {} · {}", if fc.guessed { "about " } else { "" }, fc.scenes, jobs::about(fc.video_secs), if fc.left_secs == 0 { "everything is made".to_string() } else { format!("{} left to make on this PC", jobs::about(fc.left_secs)) });
        let label = match (at, auto && busy) {
            (_, true) => "Making it automatically…",
            (0 | 1, _) if with_sheets == 0 => "Make the whole video automatically",
            _ => "Continue automatically from here",
        };
        let mut steps = div().flex().flex_wrap().gap(px(space::SM));
        for (i, (title, detail)) in rows.into_iter().enumerate() {
            // Step i is done once the stage is past it (the stage index is one ahead of the rows).
            let (done, current) = (at > i, at == i);
            let lead: AnyElement = if done {
                icon_ph(ph::CHECK_CIRCLE, Tokens::success(), 18.).into_any_element()
            } else if current && busy {
                mv_ui::spinner(("cr-step-spin", i), Tokens::accent(), 16.).into_any_element()
            } else {
                div().flex_none().w(px(18.)).h(px(18.)).rounded_full().border_1().border_color(if current { Tokens::accent() } else { Tokens::border_strong() }).flex().items_center().justify_center().text_size(px(10.)).text_color(if current { Tokens::text_1() } else { Tokens::text_3() }).child(format!("{}", i + 1)).into_any_element()
            };
            steps = steps.child(
                div()
                    .id(("cr-step", i))
                    .flex_1()
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .gap(px(space::XXS))
                    .px(px(space::MD))
                    .py(px(10.))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(if current { Tokens::accent_border() } else { Tokens::border() })
                    .bg(if current { Tokens::accent_soft() } else { Tokens::surface_3() })
                    .tooltip(mv_ui::tip(STEP_TIPS[i]))
                    .child(div().flex().items_center().gap(px(6.)).child(lead).child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(if done || current { Tokens::text_1() } else { Tokens::text_3() }).child(title)))
                    .child(div().text_size(px(size::XS)).text_color(if done || current { Tokens::text_2() } else { Tokens::text_3() }).whitespace_normal().child(detail))
                    .when_some(now.clone().filter(|_| current && busy), |d, t| d.child(div().text_size(px(size::XS)).text_color(Tokens::accent()).whitespace_normal().child(t))),
            );
        }
        let video_file = existing(p, ch.video.as_ref());
        let title = format!("{} · Chapter {n}", p.title);
        let hint = if auto && busy {
            "Runs by itself to the end. It also shows in Activity."
        } else if at == 5 {
            "The video is made. Delete lets you make any part of it again."
        } else {
            "Does every step that is left, one after another. Stop any time: what is made is kept."
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .when(at < 5, |d| {
                d.child(btn("cr-auto", label, ButtonKind::Primary).icon(ph::PLAY).loading(auto && busy).on_click(cx.listener(|this, _, _, cx| {
                    if !this.busy() {
                        this.auto_start(cx);
                    }
                })))
            })
            .when_some(video_file, |d, f| {
                let f2 = f.clone();
                d.child(btn("cr-play", "Play the video", if at == 5 { ButtonKind::Primary } else { ButtonKind::Secondary }).icon(ph::PLAY).on_click(move |_, window, cx| {
                    let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                    mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
                }))
                .child(btn("cr-reveal", "Show the video in its folder", ButtonKind::Ghost).icon(ph::FOLDER_OPEN).on_click(move |_, _, _| reveal(&f2)))
            })
            .when(busy, |d| {
                d.child(btn("cr-auto-stop", "Stop", ButtonKind::Ghost).icon(ph::STOP).on_click(cx.listener(|this, _, _, cx| {
                    this.auto = None;
                    if this.thinking.is_some() {
                        this.stop_ai(cx);
                    }
                    this.stop(cx);
                })))
            })
            .child(div().flex_1().min_w(px(200.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(hint));
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .p(px(space::LG))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Chapter {n}")))
                    .when(!ch.title.trim().is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(ch.title.trim().to_string())))
                    .child(div().flex_1())
                    .child(icon_ph(ph::CLOCK, Tokens::text_3(), 14.))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(forecast)),
            )
            .child(steps)
            .child(actions)
            .child(self.storage(p, n, cx))
    }

    /// "Magnific website · Live action · Colour · LTX 2.3 with voice · 5 s · 16:9": how the
    /// pictures and clips are made, in one line.
    fn setup_summary(&self, p: &Project) -> String {
        let vm = video::video_model(&p.settings.video_model);
        [
            if p.settings.web_images { "Magnific website".to_string() } else { jobs::image_model(p, &self.keys).name.to_string() },
            if p.settings.live_action { "Live action" } else { "Drawn manga" }.to_string(),
            if p.settings.color { "Colour" } else { "B&W" }.to_string(),
            vm.name.to_string(),
            format!("{} s", vm.fit_seconds(p.settings.clip_seconds)),
            p.settings.video_aspect.clone(),
        ]
        .join(" · ")
    }

    /// The open chapter's scenes page by page: each one's picture, whether its clip is made,
    /// Make / Redo, and its prompt and notes.
    fn scenes(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Div {
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let todo = jobs::plan(p, &Proposal::Video { chapter: n });
        let clips_left = todo.iter().filter(|t| matches!(t, Task::Clip { .. })).count();
        let total: usize = ch.pages.iter().filter(|pg| pg.image.is_some()).map(|pg| pg.panels.len()).sum();
        let made = total - clips_left;
        let unpainted = ch.pages.len() - ch.painted();
        let count = format!("{made} of {total} clips made{}", if unpainted > 0 { format!(" · {unpainted} page{} still without a picture", if unpainted == 1 { "" } else { "s" }) } else { String::new() });
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
                    .child(mv_ui::section_label("Scenes").pb_0())
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).child(count))
                    .when(total > 0, |d| {
                        d.child(btn("cr-animate", if clips_left == 0 { "Join the video again".to_string() } else if made == 0 { format!("Film chapter {n}") } else { format!("Film the {clips_left} left") }, ButtonKind::Secondary).small().icon(ph::PLAY).tooltip("Films only the clips (and joins the video), without the other steps").on_click(cx.listener(move |this, _, _, cx| this.enqueue(todo.clone(), cx))))
                    }),
            )
            .children(self.clip_sel.and_then(|(page, i)| self.clip_editor(p, n, page, i, cx)))
            .children(ch.pages.iter().map(|pg| {
                let page = pg.number;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(format!("Page {page}{}", if pg.beat.is_empty() { String::new() } else { format!(" · {}", pg.beat) })))
                    .child(div().flex().flex_wrap().gap(px(space::SM)).children(pg.panels.iter().enumerate().map(|(i, x)| {
                        let src = existing(p, x.image.as_ref()).or_else(|| existing(p, pg.image.as_ref()));
                        let has = x.clip.is_some();
                        let busy = self.running.as_ref().is_some_and(|r| r.label == format!("Chapter {n} · page {page} · panel {}", i + 1));
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .w(px(150.))
                            .child(self.thumb(SharedString::from(format!("cr-clip-{page}-{i}")), src, 150., 90., cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(icon_ph(if has { ph::CHECK } else { ph::FILM_STRIP }, if has { Tokens::success() } else { Tokens::text_3() }, 12.))
                                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(if busy { "Filming…".to_string() } else if x.motion.is_empty() { format!("Scene {}", i + 1) } else { x.motion.clone() }))
                                    .child(btn(SharedString::from(format!("cr-reclip-{page}-{i}")), if has { "Redo" } else { "Make" }, ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Clip { chapter: n, page, panel: i }, Task::Stitch { chapter: n }], cx)))),
                            )
                            .child(btn(SharedString::from(format!("cr-clipnotes-{page}-{i}")), if x.notes.trim().is_empty() { "Prompt & notes" } else { "Prompt & notes ✓" }, if self.clip_sel == Some((page, i)) { ButtonKind::Secondary } else { ButtonKind::Ghost }).small().icon(ph::PENCIL_SIMPLE).tooltip("Read exactly what is asked for this scene's picture and clip, write what should be different, or delete its clip").on_click(cx.listener(move |this, _, window, cx| this.pick_clip(n, page, i, window, cx))))
                    })))
            }))
    }

    pub(crate) fn video_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(p) = self.p.clone() else {
            // Nothing open: the one thing to do is pick the manga.
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::LG))
                .pt(px(48.))
                .child(icon_ph(ph::FILM_STRIP, Tokens::accent(), 44.))
                .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Make a video from a manga"))
                .child(div().max_w(px(600.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Pick a manga you downloaded. The app reads its characters and story, makes a realistic sheet of every main character and a picture of every place, writes the scenes, shoots a picture of each, then voices and films them and joins the video. You watch each step and can open any scene to read or change what was asked for."))
                .children(self.adapt_menu("cr-adapt-video", cx))
                .when(self.library.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("No downloaded manga found yet: download chapters of one first.")));
        };
        if self.error.is_some() {
            self.auto = None;
        }
        let n = if p.chapter(self.chapter).is_some() { self.chapter } else { p.first_chapter() };
        self.measure_disk(&p, n, cx);
        let written = p.chapter(n).is_some_and(|c| !c.pages.is_empty());
        let chapters = self.video_chapters(&p, n, cx);
        let panel = self.chapter_panel(&p, n, cx);
        // The models and the look are set once, so they stay folded away under their one-line summary.
        let setup = mv_ui::disclosure("cr-setup", format!("How it is made · {}", self.setup_summary(&p))).content(div().flex().flex_col().gap(px(space::MD)).pb(px(space::SM)).child(self.look_bar(cx)).child(self.video_bar(cx)));
        let scenes = written.then(|| self.scenes(&p, n, cx));
        div().flex().flex_col().gap(px(space::LG)).child(chapters).child(panel).child(setup).children(scenes)
    }
}

/// Where a chapter's video stands, for its card: the line under its number, and its four steps
/// (scenes written, pictures made, clips filmed, video joined) as done or not.
fn chapter_state(ch: Option<&mv_create::Chapter>) -> (String, [bool; 4]) {
    let Some(ch) = ch.filter(|c| !c.pages.is_empty()) else { return ("Not started".into(), [false; 4]) };
    let scenes: usize = ch.pages.iter().map(|pg| pg.panels.len()).sum();
    let clips = ch.pages.iter().flat_map(|pg| &pg.panels).filter(|x| x.clip.is_some()).count();
    let (pages, painted) = (ch.pages.len(), ch.painted());
    let pictured = painted == pages;
    let filmed = pictured && clips == scenes;
    let joined = filmed && ch.video.is_some();
    let line = if joined {
        "Video ready".to_string()
    } else if filmed {
        "Ready to join".to_string()
    } else if pictured && clips == 0 {
        "Pictures ready".to_string()
    } else if pictured {
        format!("{clips} of {scenes} clips")
    } else if painted == 0 {
        format!("{scenes} scenes written")
    } else {
        format!("{painted} of {pages} pictures")
    };
    (line, [true, pictured, filmed, joined])
}

/// "1.2 GB", "340 MB", "under 1 MB", "nothing yet".
fn size_label(bytes: u64) -> String {
    match bytes {
        0 => "nothing yet".into(),
        1..=1_048_575 => "under 1 MB".into(),
        _ => mv_ui::fmt_bytes(bytes),
    }
}

/// What Studio read of chapter `n` of the manga `title` (its story up to the end of that
/// chapter), when it has read it.
fn chapter_recap(title: &str, n: u32) -> Option<String> {
    let norm = |s: &str| s.trim().to_lowercase();
    let groups = ui_studio::data::load().ok()?;
    let g = groups.iter().find(|g| norm(&g.title) == norm(title))?;
    let brain = mv_recap::Brain::load(&g.id, &g.title);
    brain.recaps.iter().find(|r| label_number(&r.label) == Some(n)).map(|r| r.summary.trim().to_string()).filter(|s| !s.is_empty())
}

/// The chapter number in a label ("Vol. 2 Ch. 12" is 12, "Chapter 75 · The Duel" is 75).
fn label_number(label: &str) -> Option<u32> {
    let l = label.to_lowercase();
    let from = l.find("ch").unwrap_or(0);
    let digits: String = l[from..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// A prompt as sent, in a box that can be read whole.
fn prompt_box(title: &'static str, text: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(px(space::SM))
        .rounded(px(radius::MD))
        .bg(Tokens::surface_3())
        .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(title))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(text))
}

/// Shows the file `f` selected in its folder.
fn reveal(f: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", f.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = f;
    }
}

/// Opens the folder `dir` in the file manager (the nearest folder above it that exists, while
/// nothing has been made in it yet).
fn open_folder(dir: &std::path::Path) {
    let dir = dir.ancestors().find(|d| d.is_dir()).unwrap_or(dir);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
    }
}

#[cfg(test)]
mod tests {
    // Named imports: `super::*` would bring in the UI kit's own `test` attribute over the plain one.
    use super::{chapter_state, label_number, size_label, ClearAsk};
    use mv_create::{Chapter, Clear, Page, Panel};

    #[test]
    fn a_chapters_card_says_where_its_video_stands() {
        let panel = |image: bool, clip: bool| Panel { image: image.then(|| "panels/a.png".to_string()), clip: clip.then(|| "clips/a.mp4".to_string()), ..Default::default() };
        let page = |painted: bool, clips: [bool; 2]| Page { number: 1, image: painted.then(|| "pages/a.png".to_string()), panels: clips.iter().map(|c| panel(painted, *c)).collect(), ..Default::default() };
        let state = |pages: Vec<Page>, video: bool| chapter_state(Some(&Chapter { number: 1, pages, video: video.then(|| "video/a.mp4".to_string()), ..Default::default() }));
        assert_eq!(chapter_state(None), ("Not started".to_string(), [false; 4]));
        assert_eq!(state(vec![], false), ("Not started".to_string(), [false; 4]));
        assert_eq!(state(vec![page(false, [false; 2]), page(false, [false; 2])], false), ("4 scenes written".to_string(), [true, false, false, false]));
        assert_eq!(state(vec![page(true, [false; 2]), page(false, [false; 2])], false), ("1 of 2 pictures".to_string(), [true, false, false, false]));
        assert_eq!(state(vec![page(true, [false; 2]), page(true, [false; 2])], false), ("Pictures ready".to_string(), [true, true, false, false]));
        assert_eq!(state(vec![page(true, [true, false]), page(true, [true, true])], false), ("3 of 4 clips".to_string(), [true, true, false, false]));
        assert_eq!(state(vec![page(true, [true; 2])], false), ("Ready to join".to_string(), [true, true, true, false]));
        assert_eq!(state(vec![page(true, [true; 2])], true), ("Video ready".to_string(), [true; 4]));
        // A repainted page lost its clips: the old video no longer counts as made.
        assert_eq!(state(vec![page(true, [true, false])], true), ("1 of 2 clips".to_string(), [true, true, false, false]));
    }

    #[test]
    fn sizes_and_deletes_read_plainly() {
        assert_eq!((size_label(0), size_label(900), size_label(340 * 1024 * 1024)), ("nothing yet".to_string(), "under 1 MB".to_string(), "340 MB".to_string()));
        assert_eq!(ClearAsk::Chapter(3, Clear::Film).name(), "chapter 3's video and clips");
        assert_eq!(ClearAsk::Chapter(3, Clear::All).name(), "everything made for chapter 3");
        assert_eq!(label_number("Vol. 2 Ch. 12"), Some(12));
    }
}
