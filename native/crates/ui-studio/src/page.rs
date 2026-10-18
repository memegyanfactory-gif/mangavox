//! The Studio page: setup check, pick chapters, pick a voice, narrate, follow the
//! queue, and open what was produced.
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::nav::PlayRequest;
use mv_ui::{button, chip, empty_state, error_state, icon, page_header, section_title, size, ButtonKind, Route, Tokens};

use crate::data::{self, ChapterItem, MangaGroup};
use crate::service::{self, JobInfo, JobView, NarrationRequest};
use crate::setup::{self, Env, OcrProvider, VoiceReady};

const SPEEDS: [f64; 4] = [0.9, 1.0, 1.1, 1.25];
const RESOLUTIONS: [&str; 2] = ["720p", "1080p"];
const POLL: Duration = Duration::from_millis(400);
const MONO: &str = if cfg!(windows) {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

/// Last environment check, so returning to the page shows it immediately.
static LAST_ENV: Mutex<Option<Env>> = Mutex::new(None);

enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Ok,
    Warn,
    Error,
}

struct SetupRow {
    id: &'static str,
    level: Level,
    title: String,
    detail: Option<String>,
    commands: Vec<(&'static str, String)>,
}

struct Notice {
    error: bool,
    text: String,
}

pub struct StudioPage {
    env: Option<Env>,
    checking: bool,
    library: Load<Vec<MangaGroup>>,
    manga: Option<String>,
    selected: BTreeSet<String>,
    lang: &'static str,
    voice: String,
    speed: f64,
    video: bool,
    resolution: &'static str,
    jobs: Vec<JobInfo>,
    seen_version: u64,
    expanded: Option<u64>,
    notice: Option<Notice>,
    previewing: bool,
    copied: Option<String>,
}

impl StudioPage {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let voice = setup::saved_voice()
            .filter(|v| mv_studio::tts::find_voice(v).is_some() && setup::voices(lang_of(v)).iter().any(|x| x.id == v))
            .unwrap_or_else(|| "af_heart".into());
        let speed = setup::saved_speed().filter(|s| SPEEDS.contains(s)).unwrap_or(1.0);
        let mut page = Self {
            env: LAST_ENV.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            checking: false,
            library: Load::Loading,
            manga: None,
            selected: BTreeSet::new(),
            lang: lang_of(&voice),
            voice,
            speed,
            video: true,
            resolution: "1080p",
            jobs: service::snapshot(),
            seen_version: service::version(),
            expanded: None,
            notice: None,
            previewing: false,
            copied: None,
        };
        page.recheck(cx);
        page.reload_library(cx);
        page.poll(cx);
        page
    }

    // ------------------------------------------------------------ async work

    fn recheck(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.spawn(async move |this, cx| {
            let env = mv_ui::bg::run(setup::check()).await;
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                match env {
                    Ok(env) => {
                        *LAST_ENV.lock().unwrap_or_else(|e| e.into_inner()) = Some(env.clone());
                        // Video needs FFmpeg; default to audio-only without it.
                        if !env.ffmpeg {
                            this.video = false;
                        }
                        this.env = Some(env);
                    }
                    Err(e) => this.notice = Some(Notice { error: true, text: format!("Setup check failed: {e:#}") }),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reload_library(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(data::load).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(groups) => {
                        let keep = this.manga.as_ref().is_some_and(|id| groups.iter().any(|g| &g.id == id));
                        if !keep {
                            this.manga = groups.first().map(|g| g.id.clone());
                            this.selected.clear();
                        }
                        this.library = Load::Ready(groups);
                    }
                    Err(e) => this.library = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Follows the shared queue while the page is alive.
    fn poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let version = service::version();
            let alive = this.update(cx, |this, cx| {
                if version == this.seen_version {
                    return;
                }
                this.seen_version = version;
                let jobs = service::snapshot();
                let done = |js: &[JobInfo]| js.iter().filter(|j| j.state == JobView::Done).count();
                let reload = done(&jobs) > done(&this.jobs);
                this.jobs = jobs;
                if reload {
                    this.reload_library(cx);
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (Some(env), Load::Ready(groups)) = (&self.env, &self.library) else { return };
        let chapters: Vec<ChapterItem> =
            groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| self.selected.contains(&c.id)).cloned().collect();
        let req = NarrationRequest { voice: self.voice.clone(), speed: self.speed, video: self.video, resolution: self.resolution.into() };
        match service::enqueue(env, &req, &chapters) {
            Ok(0) => self.notice = Some(Notice { error: false, text: "Those chapters are already in the queue.".into() }),
            Ok(n) => {
                self.notice = Some(Notice { error: false, text: format!("Queued {n} chapter{}.", plural(n)) });
                self.selected.clear();
                self.jobs = service::snapshot();
                self.seen_version = service::version();
                self.expanded = self.jobs.first().map(|j| j.id);
            }
            Err(e) => self.notice = Some(Notice { error: true, text: format!("{e:#}") }),
        }
        cx.notify();
    }

    fn preview(&mut self, cx: &mut Context<Self>) {
        let Some(env) = &self.env else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else { return };
        let (voice, speed) = (self.voice.clone(), self.speed);
        self.previewing = true;
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let samples = mv_studio::tts::default_samples_dir();
                mv_studio::tts::voice_preview(&py, &layout, &samples, &std::env::temp_dir(), &voice, speed, None).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.previewing = false;
                match res {
                    Ok(Ok(p)) => cx.open_with_system(&p.path),
                    Ok(Err(e)) => this.notice = Some(Notice { error: true, text: format!("Preview failed: {e}") }),
                    Err(e) => this.notice = Some(Notice { error: true, text: format!("Preview failed: {e:#}") }),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ derived state

    fn current_group(&self) -> Option<&MangaGroup> {
        let Load::Ready(groups) = &self.library else { return None };
        groups.iter().find(|g| Some(&g.id) == self.manga.as_ref())
    }

    fn selected_chapters(&self) -> Vec<&ChapterItem> {
        let Load::Ready(groups) = &self.library else { return vec![] };
        groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| self.selected.contains(&c.id)).collect()
    }

    fn voice_ready(&self) -> VoiceReady {
        match (&self.env, mv_studio::tts::find_voice(&self.voice)) {
            (Some(env), Some(v)) => setup::voice_ready(&env.engines, v),
            _ => VoiceReady::Missing,
        }
    }

    /// Why "Narrate" can't run yet, if anything.
    fn blocker(&self) -> Option<String> {
        let Some(env) = &self.env else { return Some("Checking your setup…".into()) };
        if !env.runnable() {
            return Some("Studio needs Python and the pipeline scripts. See Setup above.".into());
        }
        let chapters = self.selected_chapters();
        if chapters.is_empty() {
            return Some("Pick at least one chapter.".into());
        }
        if !self.voice_ready().usable() {
            return Some("This voice isn't installed yet. See Setup above.".into());
        }
        if self.video && !env.ffmpeg {
            return Some("Video needs FFmpeg. Choose Audio only, or install FFmpeg (see Setup).".into());
        }
        let need_text = chapters.iter().filter(|c| !c.has_text).count();
        if need_text > 0 && !env.ocr.ready() {
            return Some(format!(
                "{need_text} chapter{} {} no text yet and no page-text reader is set up. See Setup above.",
                plural(need_text),
                if need_text == 1 { "has" } else { "have" }
            ));
        }
        None
    }

    // ------------------------------------------------------------ sections

    fn setup_card(&self, cx: &mut Context<Self>) -> Div {
        let rows = self.env.as_ref().map(setup_rows).unwrap_or_default();
        let issues = rows.iter().filter(|r| r.level != Level::Ok).count();
        let subtitle = match (&self.env, self.checking) {
            (None, _) => "Checking Python, voices and tools…".to_string(),
            (Some(_), true) => "Checking again…".to_string(),
            (Some(_), false) if issues == 0 => "Everything Studio needs is installed.".to_string(),
            (Some(_), false) => format!("{issues} item{} need{} attention. Run the commands below in a terminal, then press Re-check.", plural(issues), if issues == 1 { "s" } else { "" }),
        };
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(card_title("Setup", Some(subtitle)))
                    .child(
                        button("studio-recheck", if self.checking { "Checking…" } else { "Re-check" }, ButtonKind::Ghost)
                            .when(!self.checking, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.recheck(cx)))),
                    ),
            )
            .children(rows.into_iter().map(|r| self.setup_row(r, cx)))
    }

    fn setup_row(&self, row: SetupRow, cx: &mut Context<Self>) -> Div {
        let (path, color) = match row.level {
            Level::Ok => ("icons/circle-check.svg", Tokens::accent()),
            Level::Warn => ("icons/triangle-alert.svg", Tokens::warning()),
            Level::Error => ("icons/circle-x.svg", Tokens::danger()),
        };
        div()
            .flex()
            .gap(px(10.))
            .pt(px(12.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(div().pt(px(1.)).child(icon(path, color, 16.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(row.title))
                    .when_some(row.detail, |d, t| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(t)))
                    .children(row.commands.into_iter().enumerate().map(|(i, (label, cmd))| self.command_box(format!("{}-{i}", row.id), label, cmd, cx))),
            )
    }

    fn command_box(&self, id: String, label: &'static str, cmd: String, cx: &mut Context<Self>) -> Div {
        let copied = self.copied.as_deref() == Some(cmd.as_str());
        let text = cmd.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(label))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .bg(Tokens::base())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(div().flex_1().min_w_0().font_family(MONO).text_size(px(size::SM)).text_color(Tokens::text_2()).child(cmd))
                    .child(
                        div()
                            .id(SharedString::from(format!("copy-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .flex_none()
                            .px(px(6.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_size(px(size::XS))
                            .text_color(if copied { Tokens::accent() } else { Tokens::text_2() })
                            .hover(|s| s.bg(Tokens::hover()))
                            .child(icon(if copied { "icons/check.svg" } else { "icons/copy.svg" }, if copied { Tokens::accent() } else { Tokens::text_2() }, 13.))
                            .child(if copied { "Copied" } else { "Copy" })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                this.copied = Some(text.clone());
                                cx.notify();
                            })),
                    ),
            )
    }

    fn chapters_card(&self, cx: &mut Context<Self>) -> Div {
        let body: AnyElement = match &self.library {
            Load::Loading => muted("Loading your downloaded chapters…").into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .flex_col()
                .items_center()
                .child(error_state(e.clone()))
                .child(button("studio-lib-retry", "Retry", ButtonKind::Secondary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.library = Load::Loading;
                    this.reload_library(cx);
                    cx.notify();
                })))
                .into_any_element(),
            Load::Ready(groups) if groups.is_empty() => div()
                .flex()
                .flex_col()
                .items_center()
                .child(empty_state("No downloaded chapters", "Studio narrates chapters you've downloaded. Open a manga in your library and download a chapter."))
                .child(button("studio-open-library", "Open Library", ButtonKind::Primary).on_click(|_, w, cx| mv_ui::navigate(Route::Library, w, cx)))
                .into_any_element(),
            Load::Ready(groups) => {
                let chips = div().flex().flex_wrap().gap(px(8.)).children(groups.iter().map(|g| {
                    let id = g.id.clone();
                    chip(format!("manga-{}", g.id), format!("{}  {}", g.title, g.chapters.len()), self.manga.as_ref() == Some(&g.id)).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            if this.manga.as_ref() != Some(&id) {
                                this.manga = Some(id.clone());
                                this.selected.clear();
                                cx.notify();
                            }
                        },
                    ))
                }));
                let list = self.current_group().map(|g| self.chapter_list(g, cx));
                div().flex().flex_col().gap(px(12.)).child(chips).children(list).into_any_element()
            }
        };
        let actions = self.current_group().map(|g| {
            let ids: Vec<String> = g.chapters.iter().map(|c| c.id.clone()).collect();
            div()
                .flex()
                .gap(px(4.))
                .child(button("studio-select-all", "Select all", ButtonKind::Ghost).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.selected.extend(ids.iter().cloned());
                    cx.notify();
                })))
                .when(!self.selected.is_empty(), |d| {
                    d.child(button("studio-select-none", "Clear", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.selected.clear();
                        cx.notify();
                    })))
                })
        });
        card()
            .child(div().flex().items_center().gap(px(10.)).child(card_title("1  Chapters", Some("Downloaded chapters from your library".into()))).children(actions))
            .child(body)
    }

    fn chapter_list(&self, group: &MangaGroup, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().children(group.chapters.iter().map(|c| {
            let on = self.selected.contains(&c.id);
            let id = c.id.clone();
            let mut badges = vec![];
            if c.video.is_some() {
                badges.push(badge("Video", true));
            } else if c.audio_files > 0 {
                badges.push(badge("Audio", true));
            }
            if c.has_text {
                badges.push(badge("Text ready", false));
            }
            div()
                .id(SharedString::from(format!("ch-{}", c.id)))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(38.))
                .px(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(on, |d| d.bg(Tokens::accent_soft()))
                .when(!on, |d| d.hover(|s| s.bg(Tokens::hover())))
                .child(icon(if on { "icons/square-check.svg" } else { "icons/square.svg" }, if on { Tokens::accent() } else { Tokens::text_3() }, 16.))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(c.label()))
                .children(badges)
                .child(div().w(px(70.)).flex().justify_end().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} pages", c.page_count)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !this.selected.remove(&id) {
                        this.selected.insert(id.clone());
                    }
                    cx.notify();
                }))
        }))
    }

    fn voice_card(&self, cx: &mut Context<Self>) -> Div {
        let engines = self.env.as_ref().map(|e| e.engines.clone()).unwrap_or_default();
        let checked = self.env.is_some();
        let langs = div().flex().gap(px(8.)).children([("en", "English"), ("hi", "Hindi")].map(|(code, label)| {
            chip(format!("lang-{code}"), label, self.lang == code).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.lang = code;
                if lang_of(&this.voice) != code {
                    // Keep a ready voice selected when switching language.
                    let env = this.env.as_ref().map(|e| e.engines.clone()).unwrap_or_default();
                    let list = setup::voices(code);
                    let pick = list.iter().find(|v| matches!(setup::voice_ready(&env, v), VoiceReady::Ready(_))).or(list.first());
                    if let Some(v) = pick {
                        this.voice = v.id.to_string();
                    }
                }
                cx.notify();
            }))
        }));
        let tiles = div().flex().flex_wrap().gap(px(10.)).children(setup::voices(self.lang).into_iter().map(|v| {
            let on = self.voice == v.id;
            let ready = setup::voice_ready(&engines, v);
            let (status, color) = match ready {
                _ if !checked => ("Checking…".to_string(), Tokens::text_3()),
                VoiceReady::Ready(e) => (format!("Ready · {e}"), Tokens::accent()),
                VoiceReady::Fallback(e) => (format!("Uses {e} until installed"), Tokens::warning()),
                VoiceReady::Missing => ("Not installed".to_string(), Tokens::text_3()),
            };
            let id = v.id;
            div()
                .id(SharedString::from(format!("voice-{}", v.id)))
                .flex()
                .flex_col()
                .gap(px(2.))
                .w(px(214.))
                .px(px(12.))
                .py(px(9.))
                .rounded(px(10.))
                .border_1()
                .cursor_pointer()
                .map(|d| if on { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()).hover(|s| s.bg(Tokens::hover())) })
                .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(v.label))
                .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(v.desc))
                .child(div().truncate().text_size(px(size::XS)).text_color(color).child(status))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.voice = id.to_string();
                    cx.notify();
                }))
        }));
        let ffmpeg = self.env.as_ref().is_some_and(|e| e.ffmpeg);
        let speeds = div().flex().gap(px(6.)).children(SPEEDS.map(|s| {
            chip(format!("speed-{s}"), format!("{s}x"), self.speed == s).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.speed = s;
                cx.notify();
            }))
        }));
        let outputs = div()
            .flex()
            .gap(px(6.))
            .child(chip("out-video", "Video + audio", self.video).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.video = true;
                cx.notify();
            })))
            .child(chip("out-audio", "Audio only", !self.video).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.video = false;
                cx.notify();
            })))
            .when(self.video, |d| {
                d.child(div().w(px(8.))).children(RESOLUTIONS.map(|r| {
                    chip(format!("res-{r}"), r, self.resolution == r).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.resolution = r;
                        cx.notify();
                    }))
                }))
            })
            .when(self.video && checked && !ffmpeg, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("FFmpeg not found")));
        let can_preview = self.env.as_ref().is_some_and(|e| e.runnable()) && self.voice_ready().usable() && !self.previewing;
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(card_title("2  Voice", Some("Narrator voice and output".into())))
                    .child(
                        button("studio-preview", if self.previewing { "Generating preview…" } else { "Preview voice" }, ButtonKind::Secondary)
                            .when(!can_preview, |b| b.opacity(0.5).cursor_default())
                            .when(can_preview, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.preview(cx)))),
                    ),
            )
            .child(langs)
            .child(tiles)
            .child(option_row("Speed", speeds))
            .child(option_row("Output", outputs))
    }

    fn action_bar(&self, cx: &mut Context<Self>) -> Div {
        let n = self.selected.len();
        let label = if n == 0 { "Narrate".to_string() } else { format!("Narrate {n} chapter{}", plural(n)) };
        let blocker = self.blocker();
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(
                button("studio-start", label, ButtonKind::Primary)
                    .h(px(36.))
                    .px(px(18.))
                    .when(blocker.is_some(), |b| b.opacity(0.45).cursor_default())
                    .when(blocker.is_none(), |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start(cx)))),
            )
            .when_some(blocker, |d, why| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(why)))
    }

    fn queue_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.jobs.is_empty() {
            return None;
        }
        let any_finished = self.jobs.iter().any(|j| j.state.finished());
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(section_title("Queue"))
                        .child(div().flex_1())
                        .when(any_finished, |d| {
                            d.child(button("studio-clear", "Clear finished", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                service::clear_finished();
                                this.jobs = service::snapshot();
                                cx.notify();
                            })))
                        }),
                )
                .children(self.jobs.iter().map(|j| self.job_row(j, cx))),
        )
    }

    fn job_row(&self, job: &JobInfo, cx: &mut Context<Self>) -> Div {
        let id = job.id;
        let open = self.expanded == Some(id);
        let (status_color, fill) = match &job.state {
            JobView::Failed(_) => (Tokens::danger(), Tokens::danger()),
            JobView::Cancelled => (Tokens::text_3(), Tokens::text_3()),
            JobView::Done => (Tokens::accent(), Tokens::accent()),
            _ => (Tokens::text_2(), Tokens::accent()),
        };
        let title = format!("{} — {}", job.manga_title, job.chapter_label);
        let mut actions = div().flex().items_center().gap(px(4.));
        match &job.state {
            JobView::Queued | JobView::Running => {
                actions = actions.child(button(format!("job-cancel-{id}"), "Cancel", ButtonKind::Ghost).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if !service::cancel(id) {
                        this.notice = Some(Notice { error: true, text: "That job already finished.".into() });
                    }
                    cx.notify();
                })));
            }
            JobView::Done => {
                if let Some(video) = job.video_path() {
                    let t = title.clone();
                    actions = actions.child(button(format!("job-play-{id}"), "Play", ButtonKind::Secondary).on_click(move |_, w, cx| play(&t, &video, w, cx)));
                }
                let dir = job.output_dir.clone();
                actions = actions.child(button(format!("job-folder-{id}"), "Open folder", ButtonKind::Ghost).on_click(move |_, _, cx| cx.open_with_system(&dir)));
            }
            _ => {}
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .rounded(px(10.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .id(SharedString::from(format!("job-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .child(icon(if open { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" }, Tokens::text_3(), 14.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title))
                            .child(div().truncate().text_size(px(size::SM)).text_color(status_color).child(job.status.clone())),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(voice_label(&job.voice)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.expanded = if this.expanded == Some(id) { None } else { Some(id) };
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(Tokens::surface_2())
                            .child(div().h_full().rounded(px(2.)).bg(fill).w(relative(job.progress() as f32))),
                    )
                    .child(div().w(px(36.)).flex().justify_end().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{:.0}%", job.progress() * 100.0)))
                    .child(actions),
            )
            .when(open, |d| d.child(log_panel(job)))
    }

    fn outputs_section(&self) -> Option<Div> {
        let Load::Ready(groups) = &self.library else { return None };
        let done: Vec<&ChapterItem> = groups.iter().flat_map(|g| g.chapters.iter()).filter(|c| c.has_output()).collect();
        if done.is_empty() {
            return None;
        }
        Some(div().flex().flex_col().gap(px(8.)).child(section_title("Finished narrations")).children(done.into_iter().map(|c| {
            let title = format!("{} — {}", c.manga_title, c.label());
            let meta = match (&c.video, c.audio_files) {
                (Some(_), 0) => "Video".to_string(),
                (Some(_), n) => format!("Video · {n} audio clip{}", plural(n)),
                (None, n) => format!("{n} audio clip{}", plural(n)),
            };
            let mut actions = div().flex().items_center().gap(px(4.));
            if let Some(video) = c.video.clone() {
                let t = title.clone();
                actions = actions
                    .child(button(format!("out-play-{}", c.id), "Play", ButtonKind::Secondary).on_click(move |_, w, cx| play(&t, &video, w, cx)));
            }
            if c.audio_files > 0 {
                let audio = c.audio_dir();
                actions = actions.child(button(format!("out-audio-{}", c.id), "Audio files", ButtonKind::Ghost).on_click(move |_, _, cx| cx.open_with_system(&audio)));
            }
            let reveal = c.video.clone().unwrap_or_else(|| c.output_dir.clone());
            actions = actions.child(button(format!("out-folder-{}", c.id), "Show in folder", ButtonKind::Ghost).on_click(move |_, _, cx| cx.reveal_path(&reveal)));
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(10.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(Tokens::border())
                .child(icon(if c.video.is_some() { "icons/film.svg" } else { "icons/audio-lines.svg" }, Tokens::text_2(), 16.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(title))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(meta)),
                )
                .child(actions)
        })))
    }

    fn notice_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let n = self.notice.as_ref()?;
        let color = if n.error { Tokens::danger() } else { Tokens::accent() };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(10.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(color)
                .child(icon(if n.error { "icons/circle-x.svg" } else { "icons/info.svg" }, color, 16.))
                .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_1()).child(n.text.clone()))
                .child(
                    div()
                        .id("studio-notice-close")
                        .cursor_pointer()
                        .child(icon("icons/x.svg", Tokens::text_3(), 14.))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                ),
        )
    }
}

impl Render for StudioPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.jobs.iter().filter(|j| !j.state.finished()).count();
        let subtitle = if running > 0 {
            format!("Narrating · {running} in queue")
        } else {
            "Turn downloaded chapters into narrated audio and video".to_string()
        };
        div().size_full().child(
            mv_ui::page("studio-page").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .max_w(px(1100.))
                    .child(page_header("Studio", Some(subtitle.into())).mb(px(2.)))
                    .children(self.notice_bar(cx))
                    .child(self.setup_card(cx))
                    .child(self.chapters_card(cx))
                    .child(self.voice_card(cx))
                    .child(self.action_bar(cx))
                    .children(self.queue_section(cx))
                    .children(self.outputs_section())
                    .child(div().h(px(24.))),
            ),
        )
    }
}

// ---------------------------------------------------------------- helpers

fn setup_rows(env: &Env) -> Vec<SetupRow> {
    let mut rows = vec![];
    let py = env.python_cmd();
    match &env.python {
        Ok(p) => rows.push(SetupRow { id: "python", level: Level::Ok, title: format!("Python {}", p.version), detail: Some(p.command_line()), commands: vec![] }),
        Err(e) => rows.push(SetupRow {
            id: "python",
            level: Level::Error,
            title: "Python 3.10+ not found".into(),
            detail: Some(format!("{e}. Install Python, reopen MangaVox, then press Re-check.")),
            commands: vec![("Install Python", setup::python_install_cmd().into())],
        }),
    }
    match &env.scripts {
        Ok(l) => rows.push(SetupRow { id: "scripts", level: Level::Ok, title: "Pipeline scripts".into(), detail: Some(l.dir.display().to_string()), commands: vec![] }),
        Err(e) => rows.push(SetupRow {
            id: "scripts",
            level: Level::Error,
            title: "Pipeline scripts not found".into(),
            detail: Some(format!("{e}. Keep the python/ folder next to the app, or set MANGAVOX_PYTHON_DIR to its path.")),
            commands: vec![],
        }),
    }
    if env.runnable() {
        if env.engines.any() {
            rows.push(SetupRow {
                id: "voices",
                level: Level::Ok,
                title: format!("Voice engines: {}", env.engines.names().join(", ")),
                detail: None,
                commands: vec![],
            });
        } else {
            let mut commands = vec![];
            if let Some(c) = env.tts_packages_cmd() {
                commands.push(("Install the voice engines (Kokoro + Piper)", c));
            }
            if let Some(c) = env.voice_models_cmd() {
                commands.push(("Download the voice models (about 450 MB, once)", c));
            }
            let detail = match &env.tts_error {
                Some(e) => format!("The voice check failed: {e}"),
                None if commands.is_empty() => "Voice packages and models look present but no engine loads. Re-run the install commands from the Electron app's Audio tab, or check the Python install.".into(),
                None => "Narration needs a local text-to-speech engine. Nothing has been installed for you.".into(),
            };
            rows.push(SetupRow { id: "voices", level: Level::Error, title: "No voice engine installed".into(), detail: Some(detail), commands });
        }
        let ocr = match &env.ocr {
            OcrProvider::Gemini => SetupRow { id: "ocr", level: Level::Ok, title: "Page text: Gemini".into(), detail: None, commands: vec![] },
            OcrProvider::Ollama { model } => {
                SetupRow { id: "ocr", level: Level::Ok, title: format!("Page text: Ollama ({model})"), detail: None, commands: vec![] }
            }
            OcrProvider::Local => SetupRow { id: "ocr", level: Level::Ok, title: "Page text: local OCR".into(), detail: None, commands: vec![] },
            OcrProvider::Unavailable { ollama_running } => {
                let mut commands = vec![("Or: a local vision model with Ollama", setup::OLLAMA_PULL_CMD.to_string())];
                if let Some(dir) = env.scripts_dir() {
                    commands.push(("Or: offline OCR packages (large download)", setup::local_ocr_cmd(&py, dir)));
                }
                SetupRow {
                    id: "ocr",
                    level: Level::Warn,
                    title: "No page-text reader".into(),
                    detail: Some(format!(
                        "Only needed for chapters without text yet. {}Add a Gemini API key (gemini.api_key in {}), or use one of these:",
                        if *ollama_running { "Ollama is running but has no vision model. " } else { "" },
                        mv_core::paths::config().display()
                    )),
                    commands,
                }
            }
        };
        rows.push(ocr);
    }
    rows.push(if env.ffmpeg {
        SetupRow { id: "ffmpeg", level: Level::Ok, title: "FFmpeg".into(), detail: None, commands: vec![] }
    } else {
        SetupRow {
            id: "ffmpeg",
            level: Level::Warn,
            title: "FFmpeg not found".into(),
            detail: Some("Needed to render video. Audio-only narration still works.".into()),
            commands: vec![("Install FFmpeg", setup::ffmpeg_install_cmd().into())],
        }
    });
    rows
}

fn lang_of(voice: &str) -> &'static str {
    match mv_studio::tts::find_voice(voice).map(|v| v.lang) {
        Some("hi") => "hi",
        _ => "en",
    }
}

fn voice_label(id: &str) -> String {
    mv_studio::tts::find_voice(id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string())
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn play(title: &str, path: &PathBuf, window: &mut Window, cx: &mut App) {
    let request = PlayRequest {
        title: title.to_string(),
        source: path.display().to_string(),
        file_idx: None,
        imdb_id: None,
        mal_id: None,
        season: None,
        episode: None,
    };
    mv_ui::navigate(Route::Player { request }, window, cx);
}

fn card() -> Div {
    div().flex().flex_col().gap(px(14.)).p(px(20.)).rounded(px(12.)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

fn card_title(title: &str, sub: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .flex_1()
        .min_w_0()
        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string()))
        .when_some(sub, |d, s| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(s)))
}

fn option_row(label: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(56.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(label.to_string()))
        .child(control)
}

fn badge(text: &'static str, strong: bool) -> Div {
    div()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(6.))
        .text_size(px(size::XS))
        .map(|d| if strong { d.bg(Tokens::accent_soft()).text_color(Tokens::accent()) } else { d.bg(Tokens::surface_2()).text_color(Tokens::text_2()) })
        .child(text)
}

fn muted(text: &str) -> Div {
    div().py(px(8.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.to_string())
}

fn log_panel(job: &JobInfo) -> Stateful<Div> {
    let lines: Vec<String> = job.logs.iter().cloned().collect();
    div()
        .id(SharedString::from(format!("job-log-{}", job.id)))
        .flex()
        .flex_col()
        .max_h(px(240.))
        .overflow_y_scroll()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(Tokens::base())
        .border_1()
        .border_color(Tokens::border())
        .font_family(MONO)
        .text_size(px(size::XS))
        .text_color(Tokens::text_2())
        .when(lines.is_empty(), |d| d.child(div().text_color(Tokens::text_3()).child("No output yet.")))
        .children(lines.into_iter().map(|l| div().child(l)))
        .child(div().pt(px(4.)).text_color(Tokens::text_3()).child(format!("Output: {}", job.output_dir.display())))
}
