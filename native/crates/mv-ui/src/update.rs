//! App-wide update state (checks GitHub Releases through `mv-update`) and the small
//! pieces that show it: the version badge next to Settings, its "update available"
//! prompt, and the update row in Settings → About.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_update::Release;

use crate::{icon, size, Tokens};

const AUTO_CHECK: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    Installing(Release),
    Failed(String),
}

pub struct Updater {
    pub status: Status,
    /// The "do you want to download" prompt is open.
    pub prompt: bool,
    /// Download progress in per-mille, written from the download task.
    progress: Arc<AtomicU32>,
}

struct UpdaterGlobal(Entity<Updater>);
impl Global for UpdaterGlobal {}

/// The shared updater; the first call starts the background checks.
pub fn updater(cx: &mut App) -> Entity<Updater> {
    if let Some(g) = cx.try_global::<UpdaterGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|cx| {
        let mut u = Updater { status: Status::Idle, prompt: false, progress: Arc::new(AtomicU32::new(0)) };
        u.auto_check(cx);
        u
    });
    cx.set_global(UpdaterGlobal(e.clone()));
    e
}

pub fn version() -> &'static str {
    mv_update::VERSION
}

impl Updater {
    fn auto_check(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            loop {
                let ok = this.update(cx, |u, cx| {
                    if matches!(u.status, Status::Idle | Status::UpToDate | Status::Failed(_) | Status::Available(_)) {
                        u.check(false, false, cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
                cx.background_executor().timer(AUTO_CHECK).await;
            }
        })
        .detach();
    }

    /// Asks GitHub for the newest release. `manual` reports errors; `ask` opens the
    /// download prompt when one is found.
    pub fn check(&mut self, manual: bool, ask: bool, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Checking | Status::Downloading(_) | Status::Installing(_)) {
            return;
        }
        let prev = self.status.clone();
        self.status = Status::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = crate::bg::run(mv_update::check()).await.and_then(|r| r);
            let _ = this.update(cx, |u, cx| {
                u.status = match res {
                    Ok(Some(r)) => {
                        u.prompt = ask;
                        Status::Available(r)
                    }
                    Ok(None) => Status::UpToDate,
                    // A silent background check keeps whatever was known before.
                    Err(e) if manual => Status::Failed(format!("{e:#}")),
                    Err(_) => match prev {
                        Status::Available(r) => Status::Available(r),
                        _ => Status::Idle,
                    },
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// The badge / button was clicked: re-check, then offer the update.
    pub fn click(&mut self, cx: &mut Context<Self>) {
        match &self.status {
            Status::Downloading(_) | Status::Installing(_) | Status::Checking => {}
            _ => self.check(true, true, cx),
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.prompt = false;
        cx.notify();
    }

    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Downloads the release, swaps it in after the app exits and restarts.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Status::Available(release) = self.status.clone() else { return };
        let Some(dir) = mv_update::install_dir() else {
            self.status = Status::Failed("This is a development build. Install MangaVox with the setup exe to get automatic updates.".into());
            self.prompt = false;
            return cx.notify();
        };
        self.prompt = false;
        self.progress.store(0, Ordering::Relaxed);
        self.status = Status::Downloading(release.clone());
        cx.notify();
        let progress = self.progress.clone();
        // Repaint while the download runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let going = this.update(cx, |u, cx| {
                cx.notify();
                matches!(u.status, Status::Downloading(_))
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let r = release.clone();
            let res = crate::bg::run(async move {
                mv_update::prepare(&r, move |done, total| {
                    if total > 0 {
                        progress.store(((done as f64 / total as f64) * 1000.0) as u32, Ordering::Relaxed);
                    }
                })
                .await
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |u, cx| {
                match res.and_then(|staged| mv_update::apply_and_restart(&staged, &dir, &release.version)) {
                    Ok(()) => {
                        u.status = Status::Installing(release.clone());
                        cx.notify();
                        cx.quit();
                    }
                    Err(e) => {
                        u.status = Status::Failed(format!("Update failed: {e:#}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------- views

/// "✓ MangaVox v8.0.0" next to Settings, or "Update to v8.0.1" when one is out.
pub fn badge(collapsed: bool, cx: &mut App) -> AnyElement {
    let u = updater(cx);
    let st = u.read(cx).status.clone();
    let pct = u.read(cx).progress();
    let (ic, label, color): (&'static str, String, Rgba) = match &st {
        Status::Checking => ("icons/loader-circle.svg", "Checking…".into(), Tokens::text_3()),
        Status::Available(r) => ("icons/circle-arrow-up.svg", format!("Update to v{}", r.version), Tokens::accent()),
        Status::Downloading(_) => ("icons/download.svg", format!("Downloading {}%", (pct * 100.).round() as i32), Tokens::accent()),
        Status::Installing(_) => ("icons/loader-circle.svg", "Restarting…".into(), Tokens::accent()),
        Status::Failed(_) => ("icons/circle-alert.svg", format!("v{}", version()), Tokens::text_3()),
        Status::Idle | Status::UpToDate => ("icons/circle-check.svg", format!("MangaVox v{}", version()), Tokens::text_3()),
    };
    let available = matches!(st, Status::Available(_));
    let u2 = u.clone();
    div()
        .id("update-badge")
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(7.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(size::XS))
        .text_color(color)
        .when(available, |d| d.bg(Tokens::accent_soft()))
        .hover(|s| s.bg(Tokens::hover()))
        .child(icon(ic, color, 13.))
        .when(!collapsed, |d| d.child(label))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            u2.update(cx, |u, cx| u.click(cx));
        })
        .into_any_element()
}

/// The "update available" prompt, shown above the sidebar footer while open.
pub fn prompt(cx: &mut App) -> Option<AnyElement> {
    let u = updater(cx);
    let (open, st) = {
        let r = u.read(cx);
        (r.prompt, r.status.clone())
    };
    let Status::Available(release) = st else { return None };
    if !open {
        return None;
    }
    let (a, b) = (u.clone(), u.clone());
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::accent())
            .child(div().flex().items_center().gap(px(6.)).child(icon("icons/circle-arrow-up.svg", Tokens::accent(), 14.)).child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).child("Update available")))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!(
                "MangaVox v{} is out (you have v{}). Download it now? The app restarts by itself when it's done.",
                release.version,
                version()
            )))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(small_btn("update-later", "Later", false).on_click(move |_, _, cx| a.update(cx, |u, cx| u.dismiss(cx))))
                    .child(small_btn("update-now", "Download", true).on_click(move |_, _, cx| b.update(cx, |u, cx| u.install(cx)))),
            )
            .into_any_element(),
    )
}

/// Settings → About: version, author, and the same update flow.
pub fn about(cx: &mut App) -> AnyElement {
    let u = updater(cx);
    let st = u.read(cx).status.clone();
    let pct = u.read(cx).progress();
    let (line, color) = match &st {
        Status::Idle => ("Updates install automatically from GitHub.".to_string(), Tokens::text_3()),
        Status::Checking => ("Checking for updates…".to_string(), Tokens::text_3()),
        Status::UpToDate => ("You have the latest version.".to_string(), Tokens::accent()),
        Status::Available(r) => (format!("Version {} is available.", r.version), Tokens::accent()),
        Status::Downloading(r) => (format!("Downloading {} · {}%", r.version, (pct * 100.).round() as i32), Tokens::accent()),
        Status::Installing(_) => ("Installing — MangaVox will restart.".to_string(), Tokens::accent()),
        Status::Failed(e) => (e.clone(), Tokens::danger()),
    };
    let (a, b) = (u.clone(), u.clone());
    let available = matches!(st, Status::Available(_));
    let busy = matches!(st, Status::Checking | Status::Downloading(_) | Status::Installing(_));
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(3.))
                .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(format!("MangaVox v{}", version())))
                .child(div().text_size(px(size::SM)).text_color(color).child(line))
                .when(matches!(st, Status::Downloading(_)), |d| {
                    d.child(div().mt(px(4.)).w_full().h(px(3.)).rounded_full().bg(Tokens::active()).child(div().h_full().rounded_full().bg(Tokens::accent()).w(relative(pct))))
                }),
        )
        .when(!busy, |d| {
            d.child(small_btn("about-check", "Check for updates", false).on_click(move |_, _, cx| a.update(cx, |u, cx| u.check(true, false, cx))))
                .when(available, |d| d.child(small_btn("about-update", "Download and restart", true).on_click(move |_, _, cx| b.update(cx, |u, cx| u.install(cx)))))
        })
        .into_any_element()
}

fn small_btn(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| {
            if primary {
                d.bg(Tokens::accent()).text_color(Tokens::base()).hover(|s| s.opacity(0.9))
            } else {
                d.border_1().border_color(Tokens::border()).text_color(Tokens::text_1()).hover(|s| s.bg(Tokens::hover()))
            }
        })
        .child(label)
}
