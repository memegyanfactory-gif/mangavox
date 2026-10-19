//! MangaVox setup: one window, one button. Unpacks the embedded app (or the latest
//! release when built without one) into `%LOCALAPPDATA%\Programs\MangaVox`, adds Start
//! Menu / Desktop shortcuts and an Apps entry, then starts MangaVox. Later versions
//! arrive through the app's own updater, never through this installer.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::{icon, size, Tokens};

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

#[derive(Clone, PartialEq)]
enum Step {
    Ready,
    Working(&'static str),
    Done,
    Failed(String),
}

struct Setup {
    step: Step,
    dir: PathBuf,
    progress: Arc<AtomicU32>,
}

/// Does the install; `progress` is per-mille of the download (when downloading).
async fn install(dir: PathBuf, progress: Arc<AtomicU32>, set: impl Fn(&'static str) + Send + Sync) -> Result<()> {
    let work = mv_update::updates_dir();
    std::fs::create_dir_all(&work)?;
    let zip = work.join("setup-payload.zip");
    let version = if PAYLOAD.is_empty() {
        set("Downloading MangaVox…");
        let r = mv_update::latest().await?;
        mv_update::download(&r.zip_url, &zip, |done, total| {
            if total > 0 {
                progress.store(((done as f64 / total as f64) * 1000.0) as u32, Ordering::Relaxed);
            }
        })
        .await?;
        r.version
    } else {
        std::fs::write(&zip, PAYLOAD).context("Could not write the package")?;
        mv_update::VERSION.to_string()
    };
    set("Installing…");
    let staged = work.join("setup-staged");
    mv_update::extract(&zip, &staged)?;
    let _ = std::fs::remove_file(&zip);
    std::fs::create_dir_all(&dir)?;
    let mut cmd = Command::new("robocopy");
    cmd.arg(&staged).arg(&dir).args(["/E", "/R:3", "/W:1", "/NFL", "/NDL", "/NJH", "/NJS", "/NP"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let code = cmd.status().context("Could not copy files")?.code().unwrap_or(16);
    let _ = std::fs::remove_dir_all(&staged);
    if code >= 8 {
        bail!("Could not copy the app files. If MangaVox is open, close it and try again.");
    }
    set("Adding shortcuts…");
    mv_update::create_shortcuts(&dir)?;
    mv_update::register_uninstall(&dir, &version)?;
    Ok(())
}

impl Setup {
    fn start(&mut self, cx: &mut Context<Self>) {
        self.step = Step::Working("Preparing…");
        cx.notify();
        let (dir, progress) = (self.dir.clone(), self.progress.clone());
        let label: Arc<std::sync::Mutex<&'static str>> = Arc::new(std::sync::Mutex::new("Preparing…"));
        let l2 = label.clone();
        // Repaint with the latest label while the install runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            let text = *label.lock().unwrap();
            let going = this.update(cx, |s, cx| {
                if matches!(s.step, Step::Working(_)) {
                    s.step = Step::Working(text);
                    cx.notify();
                    true
                } else {
                    false
                }
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(install(dir, progress, move |t| *l2.lock().unwrap() = t)).await.and_then(|r| r);
            let _ = this.update(cx, |s, cx| {
                s.step = match res {
                    Ok(()) => Step::Done,
                    Err(e) => Step::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn launch(&self, cx: &mut Context<Self>) {
        let _ = Command::new(self.dir.join("mangavox.exe")).current_dir(&self.dir).spawn();
        cx.quit();
    }
}

impl Render for Setup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pct = self.progress.load(Ordering::Relaxed) as f32 / 1000.0;
        let (status, color): (String, Rgba) = match &self.step {
            Step::Ready => (format!("Installs to {}", self.dir.display()), Tokens::text_3()),
            Step::Working(t) => (t.to_string(), Tokens::text_2()),
            Step::Done => ("Installed. Updates arrive inside the app automatically.".into(), Tokens::accent()),
            Step::Failed(e) => (e.clone(), Tokens::danger()),
        };
        let working = matches!(self.step, Step::Working(_));
        let (label, primary_click): (&str, bool) = match self.step {
            Step::Ready => ("Install", true),
            Step::Failed(_) => ("Try again", true),
            Step::Done => ("Open MangaVox", true),
            Step::Working(_) => ("Installing…", false),
        };
        let done = self.step == Step::Done;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Tokens::base())
            .text_color(Tokens::text_1())
            .font_family("Segoe UI")
            .child(TitleBar::new().bg(Tokens::base()).child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("MangaVox Setup")))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(14.))
                    .px(px(36.))
                    .child(
                        div()
                            .size(px(64.))
                            .rounded(px(16.))
                            .bg(Tokens::accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(Tokens::base())
                            .text_size(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .child("M"),
                    )
                    .child(div().text_size(px(size::XXL)).font_weight(FontWeight::BOLD).child("MangaVox"))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("Version {} · Made by {}", mv_update::VERSION, mv_update::AUTHOR)))
                    .child(div().h(px(8.)))
                    .when(working, |d| {
                        d.child(div().w(px(320.)).h(px(4.)).rounded_full().bg(Tokens::active()).child(div().h_full().rounded_full().bg(Tokens::accent()).w(relative(if pct > 0.0 { pct } else { 0.15 }))))
                    })
                    .child(div().max_w(px(420.)).text_size(px(size::SM)).text_color(color).text_center().child(status))
                    .child(
                        div()
                            .id("setup-main")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(38.))
                            .px(px(22.))
                            .mt(px(6.))
                            .rounded(px(10.))
                            .text_size(px(size::MD))
                            .font_weight(FontWeight::SEMIBOLD)
                            .map(|d| if primary_click { d.bg(Tokens::accent()).text_color(Tokens::base()).cursor_pointer().hover(|s| s.opacity(0.9)) } else { d.bg(Tokens::surface_2()).text_color(Tokens::text_3()) })
                            .when(done, |d| d.child(icon("icons/play.svg", Tokens::base(), 14.)))
                            .child(label.to_string())
                            .when(primary_click, |d| {
                                d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if this.step == Step::Done {
                                        this.launch(cx);
                                    } else {
                                        this.start(cx);
                                    }
                                }))
                            }),
                    ),
            )
    }
}

fn main() {
    gpui_kit::application().with_assets(mv_ui::Assets).run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(560.), px(400.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                is_resizable: false,
                titlebar: Some(TitlebarOptions { title: Some("MangaVox Setup".into()), ..TitleBar::title_bar_options() }),
                ..TitleBar::window_options()
            },
            cx,
            |_, cx| cx.new(|_| Setup { step: Step::Ready, dir: mv_update::default_install_dir(), progress: Arc::new(AtomicU32::new(0)) }),
        )
        .expect("open window");
    });
}
