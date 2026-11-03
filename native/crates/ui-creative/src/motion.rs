//! Video: animate a painted chapter panel by panel with the picked video model (FFmpeg on this
//! PC, a local ComfyUI workflow, or an API model), then stitch the clips in reading order.
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

    pub(crate) fn video_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.p.clone().unwrap_or_default();
        if p.chapters.iter().all(|c| c.painted() == 0) {
            return div().child(mv_ui::empty_state("Paint a chapter first", "Once a chapter's pages are painted, each panel becomes a short clip here, stitched into one video."));
        }
        let n = self.chapter;
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
            .child(self.video_bar(cx))
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
            .child(div().flex().flex_col().gap(px(space::MD)).children(ch.pages.iter().filter(|pg| pg.image.is_some()).map(|pg| {
                let page = pg.number;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(format!("Page {page}")))
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
                    })))
            })))
    }
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
