//! Video: animate a painted chapter panel by panel with the picked video model (FFmpeg on this
//! PC, a local ComfyUI workflow, or an API model), then stitch the clips in reading order.
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::video::{self, VideoVia, VIDEO_MODELS};
use mv_create::{existing, Proposal};
use mv_ui::theme::{radius, space};
use mv_ui::{btn, icon_ph, menu_button, ph, segmented, size, ButtonKind, MenuItem, Tokens};

use crate::jobs::{self, Task};
use crate::page::CreativePage;

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

    /// The picked clip: what the scene is, its lines, the notes box and Make again.
    fn clip_editor(&self, p: &mv_create::Project, chapter: u32, page: u32, i: usize, cx: &mut Context<Self>) -> Option<Div> {
        let x = p.chapter(chapter)?.pages.iter().find(|pg| pg.number == page)?.panels.get(i)?.clone();
        let clip = existing(p, x.clip.as_ref());
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
                        .child(btn("cr-clip-redo", if clip.is_some() { "Make this clip again with my notes" } else { "Make this clip with my notes" }, ButtonKind::Primary).small().icon(ph::ARROWS_CLOCKWISE).loading(queued).tooltip("The notes are sent to the video model with the scene; the chapter video is stitched again after").on_click(cx.listener(move |this, _, _, cx| this.redo_clip(chapter, page, i, cx))))
                        .when_some(clip, |d, f| {
                            d.child(btn("cr-clip-play", "Play this clip", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(move |_, window, cx| {
                                let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                                mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
                            }))
                        })
                        .child(btn("cr-clip-close", "Close", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| {
                            this.clip_sel = None;
                            cx.notify();
                        }))),
                ),
        )
    }

    /// The road from a manga to its video as five steps: which are done, which is being made
    /// now, and one button that makes everything that is left.
    fn steps(&self, p: &mv_create::Project, n: u32, cx: &mut Context<Self>) -> Div {
        let stage = jobs::stage(p, n);
        let at = stage.index();
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let opening = p.opening_cast();
        let with_sheets = opening.iter().filter(|name| p.character(name).is_some_and(|c| c.has_sheets())).count();
        let places = p.locations.iter().filter(|l| l.sheet.is_some()).count();
        let scenes: usize = ch.pages.iter().map(|pg| pg.panels.len()).sum();
        let clips = ch.pages.iter().flat_map(|pg| &pg.panels).filter(|x| x.clip.is_some()).count();
        let busy = self.busy();
        let rows: [(&str, String); 5] = [
            ("Characters and story", if p.cast.is_empty() { "Read from the manga: who is in it and what happens".into() } else { format!("{} characters, {} chapters", p.cast.len(), p.chapters.len()) }),
            ("Character sheets and places", format!("{with_sheets} of {} main characters have their sheets · {places} of {} places pictured (every scene is made from these, so faces and places stay the same)", opening.len(), p.locations.len())),
            ("Scenes written", if scenes == 0 { format!("The AI writes chapter {n} scene by scene: what is seen, who speaks, how it moves") } else { format!("{scenes} scenes on {} pages", ch.pages.len()) }),
            ("Scene pictures", format!("{} of {} pages shot", ch.painted(), ch.pages.len())),
            ("Voices, clips and the video", if at == 5 { "Ready to play".into() } else { format!("{clips} of {scenes} scenes voiced and filmed, then joined") }),
        ];
        let now = self.running.as_ref().map(|r| format!("{} · {}", r.label, r.status.lock().map(|s| s.clone()).unwrap_or_default())).or_else(|| self.thinking.as_ref().map(|s| s.lock().map(|s| if s.is_empty() { "The AI is writing".to_string() } else { s.clone() }).unwrap_or_default()));
        let auto = self.auto.is_some();
        let label = match (at, auto && busy) {
            (_, true) => "Making it automatically…".to_string(),
            (5, _) => "The video is made".to_string(),
            (0 | 1, _) if with_sheets == 0 => "Make the whole video automatically".to_string(),
            _ => "Continue automatically from here".to_string(),
        };
        let mut col = div().flex().flex_col().gap(px(space::SM)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::surface_2()).border_1().border_color(Tokens::border());
        for (i, (title, detail)) in rows.into_iter().enumerate() {
            // Step i is done once the stage is past it (the stage index is one ahead of the rows).
            let (done, current) = (at > i, at == i);
            let lead: AnyElement = if done {
                icon_ph(ph::CHECK_CIRCLE, Tokens::success(), 18.).into_any_element()
            } else if current && busy {
                mv_ui::spinner(("cr-step-spin", i), Tokens::accent(), 16.).into_any_element()
            } else {
                div().w(px(18.)).h(px(18.)).rounded_full().border_1().border_color(if current { Tokens::accent() } else { Tokens::border() }).flex().items_center().justify_center().text_size(px(10.)).text_color(Tokens::text_3()).child(format!("{}", i + 1)).into_any_element()
            };
            col = col.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::SM))
                    .child(div().pt(px(1.)).child(lead))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(if done || current { Tokens::text_1() } else { Tokens::text_3() }).child(title))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(detail))
                            .when_some(now.clone().filter(|_| current && busy), |d, t| d.child(div().text_size(px(size::XS)).text_color(Tokens::accent()).whitespace_normal().child(format!("Now: {t}")))),
                    ),
            );
        }
        col.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::SM))
                .pt(px(space::XS))
                .when(at < 5, |d| {
                    d.child(btn("cr-auto", label.clone(), ButtonKind::Primary).icon(ph::PLAY).loading(auto && busy).tooltip("Does every step that is left, one after another, without asking again. Stop it any time; what is made is kept.").on_click(cx.listener(|this, _, _, cx| {
                        if !this.busy() {
                            this.auto_start(cx);
                        }
                    })))
                })
                .when(busy, |d| {
                    d.child(btn("cr-auto-stop", "Stop", ButtonKind::Ghost).small().icon(ph::STOP).on_click(cx.listener(|this, _, _, cx| {
                        this.auto = None;
                        if this.thinking.is_some() {
                            this.stop_ai(cx);
                        }
                        this.stop(cx);
                    })))
                })
                .child(div().flex_1().min_w(px(200.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(if auto && busy { "Runs by itself to the end. It also shows in Activity." } else { "Or do one step at a time with the buttons below." })),
        )
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
        let steps = self.steps(&p, n, cx);
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let todo = jobs::plan(&p, &Proposal::Video { chapter: n });
        let clips_left = todo.iter().filter(|t| matches!(t, Task::Clip { .. })).count();
        let total: usize = ch.pages.iter().filter(|pg| pg.image.is_some()).map(|pg| pg.panels.len()).sum();
        let made = total - clips_left;
        let video_file = existing(&p, ch.video.as_ref());
        let title = format!("{} · Chapter {n}", p.title);
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(self.chapter_picker(&p, cx))
            .child(steps)
            .child(mv_ui::section_label("How it is made"))
            .child(self.look_bar(cx))
            .child(self.video_bar(cx))
            .child(mv_ui::section_label("Scenes"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{made} of {total} panel clips made for chapter {n}{}.", if ch.painted() < ch.pages.len() { format!(" ({} pages still unpainted)", ch.pages.len() - ch.painted()) } else { String::new() })))
                    .when(total > 0, |d| {
                        d.child(btn("cr-animate", if clips_left == 0 { "Stitch again".to_string() } else if made == 0 { format!("Animate chapter {n}") } else { format!("Animate the {clips_left} left") }, ButtonKind::Primary).small().icon(ph::PLAY).on_click(cx.listener(move |this, _, _, cx| this.enqueue(todo.clone(), cx))))
                    }),
            )
            .when_some(video_file, |d, f| {
                let f2 = f.clone();
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .p(px(space::MD))
                        .rounded(px(radius::LG))
                        .bg(Tokens::surface_2())
                        .border_1()
                        .border_color(Tokens::border())
                        .child(icon_ph(ph::FILM_STRIP, Tokens::accent(), 18.))
                        .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(f.display().to_string()))
                        .child(btn("cr-play", "Play", ButtonKind::Primary).small().icon(ph::PLAY).on_click(move |_, window, cx| {
                            let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                            mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
                        }))
                        .child(btn("cr-reveal", "Show in folder", ButtonKind::Ghost).small().icon(ph::FOLDER_OPEN).on_click(move |_, _, _| reveal(&f2))),
                )
            })
            .children(self.clip_sel.and_then(|(page, i)| self.clip_editor(&p, n, page, i, cx)))
            .child(div().flex().flex_col().gap(px(space::MD)).children(ch.pages.iter().map(|pg| {
                let page = pg.number;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(format!("Page {page}{}", if pg.beat.is_empty() { String::new() } else { format!(" · {}", pg.beat) })))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(pg.panels.iter().enumerate().map(|(i, x)| {
                        let src = existing(&p, x.image.as_ref()).or_else(|| existing(&p, pg.image.as_ref()));
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
                                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(if busy { "Animating…".to_string() } else if x.motion.is_empty() { format!("Panel {}", i + 1) } else { x.motion.clone() }))
                                    .child(btn(SharedString::from(format!("cr-reclip-{page}-{i}")), if has { "Redo" } else { "Make" }, ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Clip { chapter: n, page, panel: i }, Task::Stitch { chapter: n }], cx)))),
                            )
                            .child(btn(SharedString::from(format!("cr-clipnotes-{page}-{i}")), if x.notes.trim().is_empty() { "Prompt & notes" } else { "Prompt & notes ✓" }, if self.clip_sel == Some((page, i)) { ButtonKind::Secondary } else { ButtonKind::Ghost }).small().icon(ph::PENCIL_SIMPLE).tooltip("Read exactly what is asked for this scene's picture and clip, and write what should be different").on_click(cx.listener(move |this, _, window, cx| this.pick_clip(n, page, i, window, cx))))
                    })))
            })))
    }
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
