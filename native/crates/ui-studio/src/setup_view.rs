//! Setup sheet: Python, pipeline scripts, voice engines and FFmpeg, each gap with the
//! exact command that fixes it. Nothing is installed from here.
use std::sync::Mutex;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{button, icon, size, ButtonKind, Tokens};

use crate::setup::{self, Env};

/// Last check, so reopening Studio shows it immediately.
static LAST_ENV: Mutex<Option<Env>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

pub struct SetupRow {
    id: &'static str,
    pub level: Level,
    title: String,
    detail: Option<String>,
    commands: Vec<(&'static str, String)>,
}

pub struct SetupView {
    pub env: Option<Env>,
    pub checking: bool,
    pub error: Option<String>,
    copied: Option<String>,
}

impl SetupView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut v = Self { env: LAST_ENV.lock().unwrap_or_else(|e| e.into_inner()).clone(), checking: false, error: None, copied: None };
        v.recheck(cx);
        v
    }

    pub fn recheck(&mut self, cx: &mut Context<Self>) {
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
                        this.env = Some(env);
                        this.error = None;
                    }
                    Err(e) => this.error = Some(format!("Setup check failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Items needing attention (0 until the first check finishes).
    pub fn issues(&self) -> usize {
        self.env.as_ref().map(|e| setup_rows(e).iter().filter(|r| r.level != Level::Ok).count()).unwrap_or(0)
    }

    fn row(&self, row: SetupRow, cx: &mut Context<Self>) -> Div {
        let (path, color) = match row.level {
            Level::Ok => ("icons/circle-check.svg", Tokens::accent()),
            Level::Warn => ("icons/triangle-alert.svg", Tokens::warning()),
            Level::Error => ("icons/circle-x.svg", Tokens::danger()),
        };
        div()
            .flex()
            .gap(px(10.))
            .py(px(10.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(div().pt(px(1.)).child(icon(path, color, 15.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(row.title))
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
                    .child(div().flex_1().min_w_0().font_family(crate::page::MONO).text_size(px(size::SM)).text_color(Tokens::text_2()).child(cmd))
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
}

impl Render for SetupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.env.as_ref().map(setup_rows).unwrap_or_default();
        let issues = rows.iter().filter(|r| r.level != Level::Ok).count();
        let subtitle = match (&self.env, self.checking) {
            (None, _) => "Checking Python, voices and tools…".to_string(),
            (Some(_), true) => "Checking again…".to_string(),
            (Some(_), false) if issues == 0 => "Everything Studio needs is installed.".to_string(),
            (Some(_), false) => format!(
                "{issues} item{} need{} attention. Run the commands in a terminal, then press Re-check.",
                if issues == 1 { "" } else { "s" },
                if issues == 1 { "s" } else { "" }
            ),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pb(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap(px(2.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).child("Setup"))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(subtitle)),
                    )
                    .child(
                        button("studio-recheck", if self.checking { "Checking…" } else { "Re-check" }, ButtonKind::Ghost)
                            .when(!self.checking, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.recheck(cx)))),
                    ),
            )
            .when_some(self.error.clone(), |d, e| d.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e)))
            .children(rows.into_iter().map(|r| self.row(r, cx)))
    }
}

fn setup_rows(env: &Env) -> Vec<SetupRow> {
    let mut rows = vec![];
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
            rows.push(SetupRow { id: "voices", level: Level::Ok, title: format!("Voice engines: {}", env.engines.names().join(", ")), detail: None, commands: vec![] });
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
                None if commands.is_empty() => "Voice packages and models look present but no engine loads. Re-run the install commands, or check the Python install.".into(),
                None => "Narration needs a local text-to-speech engine. Nothing has been installed for you.".into(),
            };
            rows.push(SetupRow { id: "voices", level: Level::Error, title: "No voice engine installed".into(), detail: Some(detail), commands });
        }
    }
    rows.push(if env.ffmpeg {
        SetupRow { id: "ffmpeg", level: Level::Ok, title: "FFmpeg".into(), detail: None, commands: vec![] }
    } else {
        SetupRow {
            id: "ffmpeg",
            level: Level::Warn,
            title: "FFmpeg not found".into(),
            detail: Some("Needed to render the video. Audio still works.".into()),
            commands: vec![("Install FFmpeg", setup::ffmpeg_install_cmd().into())],
        }
    });
    rows
}
