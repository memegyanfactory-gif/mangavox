//! Settings → Storage: where the data folder is (and moving it), plus deleting or resetting
//! each part of what MangaVox keeps. Work on files other parts of the app hold open is queued
//! in `mv_core::paths` and finished by a restart.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::paths::{self, Location, Part};
use mv_ui::{button, progress_bar, size, spinner, ButtonKind, Tokens};

use crate::ui::{card, divided, format_bytes, row};

/// What can be deleted or reset, in the order shown: (part, title, what it removes).
const PARTS: &[(Part, &str, &str)] = &[
    (Part::MangaDownloads, "Downloaded manga chapters", "Offline pages. Chapters go back to online-only; your library and Studio work stay."),
    (Part::VideoDownloads, "Anime & movie downloads", "Files in the default anime, movie and subtitle folders under the data folder."),
    (Part::Studio, "Studio work", "AI reads, scripts, narration, videos and character brains. Studio settings stay."),
    (Part::Chats, "AI chats", "Every Assistant conversation."),
    (Part::Models, "Local AI models & voices", "Downloaded models, voice previews and the local voice engine."),
    (Part::Library, "Library & history", "Every title, reading and watch history, downloaded chapters and Studio work."),
    (Part::Settings, "Settings", "Everything in Settings goes back to its default."),
];

enum Move {
    /// A folder was picked; waiting for how to use it.
    Choose { to: PathBuf, has_data: bool },
    /// Copying the data there ahead of the restart.
    Copying { to: PathBuf, done: Arc<AtomicU64>, total: Arc<AtomicU64> },
}

pub struct StorageManager {
    sizes: HashMap<String, u64>,
    measuring: bool,
    /// Part whose button was clicked once (the second click does it).
    confirm: Option<Part>,
    moving: Option<Move>,
    /// Queued work waiting for a restart (from the pointer file).
    pending: Location,
    note: Option<(String, bool)>,
}

fn key(p: Part) -> String {
    format!("{p:?}")
}

pub fn create(cx: &mut App) -> Entity<StorageManager> {
    cx.new(|cx| {
        let mut m = StorageManager { sizes: HashMap::new(), measuring: false, confirm: None, moving: None, pending: Location::load(), note: None };
        m.measure(cx);
        m
    })
}

/// Starts MangaVox again (it waits for this instance to exit, then finishes queued work).
fn restart(cx: &mut App) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--after-pid", &std::process::id().to_string()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        if cmd.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB).spawn().is_err() {
            let _ = cmd.creation_flags(DETACHED_PROCESS).spawn();
        }
    }
    #[cfg(not(windows))]
    let _ = cmd.spawn();
    cx.quit();
}

impl StorageManager {
    fn measure(&mut self, cx: &mut Context<Self>) {
        if self.measuring {
            return;
        }
        self.measuring = true;
        cx.spawn(async move |this, cx| {
            let sizes = mv_ui::bg::blocking(|| PARTS.iter().map(|(p, _, _)| (key(*p), p.size())).chain([(key(Part::Everything), Part::Everything.size())]).collect::<HashMap<_, _>>()).await;
            let _ = this.update(cx, |m, cx| {
                m.measuring = false;
                if let Ok(s) = sizes {
                    m.sizes = s;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.note = Some((text.into(), error));
        cx.notify();
    }

    fn run(&mut self, part: Part, cx: &mut Context<Self>) {
        if self.confirm != Some(part) {
            self.confirm = Some(part);
            return cx.notify();
        }
        self.confirm = None;
        if part.live() {
            cx.spawn(async move |this, cx| {
                let r = mv_ui::bg::blocking(move || paths::wipe(&paths::root(), part)).await.and_then(|r| r);
                let _ = this.update(cx, |m, cx| {
                    match r {
                        Ok(()) => m.say("Deleted.", false, cx),
                        Err(e) => m.say(format!("Could not delete: {e:#}"), true, cx),
                    }
                    m.measure(cx);
                });
            })
            .detach();
            return;
        }
        match paths::schedule_reset(&[part]) {
            Ok(()) => {
                self.pending = Location::load();
                self.say("Queued. Restart MangaVox to finish.", false, cx);
            }
            Err(e) => self.say(format!("Could not queue it: {e:#}"), true, cx),
        }
    }

    fn cancel_pending(&mut self, cx: &mut Context<Self>) {
        let mut loc = Location::load();
        loc.reset.clear();
        loc.pending_move = None;
        match loc.save() {
            Ok(()) => {
                self.pending = loc;
                self.say("Cancelled.", false, cx);
            }
            Err(e) => self.say(format!("{e:#}"), true, cx),
        }
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Use this folder".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(to) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |m, cx| m.offer(to, cx));
        })
        .detach();
    }

    fn offer(&mut self, to: PathBuf, cx: &mut Context<Self>) {
        // Picking an empty folder means "put MangaVox's data in it"; a folder with other things
        // in it gets a MangaVox subfolder so the data stays tidy.
        let to = if paths::has_data(&to) || std::fs::read_dir(&to).map(|mut r| r.next().is_none()).unwrap_or(true) { to } else { to.join("MangaVox") };
        match paths::check_new_root(&to) {
            Ok(()) => {
                let has_data = paths::has_data(&to);
                self.moving = Some(Move::Choose { to, has_data });
                self.note = None;
            }
            Err(e) => self.note = Some((format!("{e:#}"), true)),
        }
        cx.notify();
    }

    /// `move_data`: copy now (with progress), finish the move at restart. Else just switch.
    fn apply(&mut self, to: PathBuf, move_data: bool, cx: &mut Context<Self>) {
        if !move_data {
            match paths::choose_root(&to, false) {
                Ok(()) => {
                    self.moving = None;
                    self.pending = Location::load();
                    self.say(format!("MangaVox will use {} after a restart. Your current data stays where it is.", to.display()), false, cx);
                }
                Err(e) => self.say(format!("{e:#}"), true, cx),
            }
            return;
        }
        let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        self.moving = Some(Move::Copying { to: to.clone(), done: done.clone(), total: total.clone() });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (d, t) = (done.clone(), total.clone());
            let dest = to.clone();
            let work = mv_ui::bg::blocking(move || {
                let r = paths::precopy(&paths::root(), &dest, &|n, all| {
                    d.store(n, Ordering::Relaxed);
                    t.store(all, Ordering::Relaxed);
                })
                .and_then(|_| paths::choose_root(&dest, true));
                r
            });
            let r: Result<()> = work.await.and_then(|r| r);
            let _ = this.update(cx, |m, cx| {
                m.moving = None;
                m.pending = Location::load();
                match r {
                    Ok(()) => m.say(format!("Copied. Restart MangaVox to finish moving to {}.", to.display()), false, cx),
                    Err(e) => m.say(format!("Could not copy the data: {e:#}"), true, cx),
                }
            });
        })
        .detach();
        // A separate light loop keeps the progress bar moving.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let going = this.update(cx, |m, cx| {
                cx.notify();
                matches!(m.moving, Some(Move::Copying { .. }))
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn size_text(&self, p: Part) -> String {
        match self.sizes.get(&key(p)) {
            Some(b) => format_bytes(*b),
            None => "…".into(),
        }
    }

    fn folder_card(&self, cx: &mut Context<Self>) -> Div {
        let root = paths::root();
        let custom = std::env::var_os("MANGAVOX_HOME").is_none() && root != paths::default_root();
        let r2 = root.clone();
        let mut c = card(
            "Data folder",
            Some("Everything MangaVox saves lives here: library, downloads, Studio work, models, chats, settings and cache. Pick another drive or folder to move it."),
        )
        .child(row(
            "Location",
            &root.display().to_string(),
            div()
                .flex()
                .gap(px(6.))
                .child(button("data-open", "Open", ButtonKind::Ghost).on_click(move |_, _, cx| {
                    let _ = std::fs::create_dir_all(&r2);
                    cx.reveal_path(&r2);
                }))
                .child(button("data-change", "Change…", ButtonKind::Secondary).on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.pick_folder(cx))))
                .when(custom, |d| d.child(button("data-default", "Use default", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.offer(paths::default_root(), cx))))),
        ))
        .child(divided(row(
            "Inside it",
            "library/manga · library/anime · library/movies · studio/projects · models · assistant · cache · temp · logs",
            div(),
        )));
        match &self.moving {
            Some(Move::Choose { to, has_data }) => {
                let (a, b, c2) = (to.clone(), to.clone(), to.clone());
                c = c.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .my(px(8.))
                        .p(px(14.))
                        .rounded(px(10.))
                        .bg(Tokens::surface_2())
                        .border_1()
                        .border_color(Tokens::border())
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("New data folder: {}", to.display())))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if *has_data {
                            "That folder already has MangaVox data. Use it as is, or replace it with what you have now."
                        } else {
                            "Moving copies everything now, then MangaVox restarts and removes the old folder. Large libraries take a while."
                        }))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .when(*has_data, |d| d.child(button("mv-use", "Use the data there", ButtonKind::Primary).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(a.clone(), false, cx)))))
                                .child(
                                    button("mv-move", if *has_data { "Replace with my data" } else { "Move my data" }, if *has_data { ButtonKind::Secondary } else { ButtonKind::Primary })
                                        .on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(b.clone(), true, cx))),
                                )
                                .when(!*has_data, |d| d.child(button("mv-empty", "Start empty there", ButtonKind::Secondary).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(c2.clone(), false, cx)))))
                                .child(button("mv-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| {
                                    m.moving = None;
                                    cx.notify();
                                }))),
                        ),
                );
            }
            Some(Move::Copying { to, done, total, .. }) => {
                let (d, t) = (done.load(Ordering::Relaxed), total.load(Ordering::Relaxed));
                let frac = if t == 0 { 0. } else { d as f32 / t as f32 };
                c = c.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .my(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(spinner("mv-copy-spin", Tokens::accent(), 13.))
                                .child(div().flex_1().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("Copying to {}", to.display())))
                                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} of {}", format_bytes(d), format_bytes(t)))),
                        )
                        .child(progress_bar("mv-copy-bar", frac, true)),
                );
            }
            None => {}
        }
        c
    }

    fn pending_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let loc = &self.pending;
        if loc.reset.is_empty() && loc.pending_move.is_none() && loc.root.as_ref().is_none_or(|r| *r == paths::root()) {
            return None;
        }
        let mut what: Vec<String> = loc.reset.iter().map(|p| PARTS.iter().find(|(q, _, _)| q == p).map(|(_, t, _)| t.to_string()).unwrap_or_else(|| "Everything".into())).collect();
        if let Some(m) = &loc.pending_move {
            what.push(format!("move the data to {}", m.to.display()));
        } else if let Some(r) = loc.root.as_ref().filter(|r| **r != paths::root()) {
            what.push(format!("switch to {}", r.display()));
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(rgba(0xf59e0b1f))
                .border_1()
                .border_color(Tokens::warning())
                .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("Waiting for a restart: {}.", what.join(", "))))
                .child(button("pend-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.cancel_pending(cx))))
                .child(button("pend-restart", "Restart now", ButtonKind::Primary).on_click(|_, _, cx| restart(cx))),
        )
    }

    fn part_row(&self, ix: usize, part: Part, title: &str, desc: &str, cx: &mut Context<Self>) -> Div {
        let confirming = self.confirm == Some(part);
        let queued = self.pending.reset.contains(&part);
        let verb = if part == Part::Settings { "Reset" } else { "Delete" };
        let control = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().min_w(px(64.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(if part == Part::Settings { String::new() } else { self.size_text(part) }))
            .map(|d| {
                if queued {
                    d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("After restart"))
                } else {
                    d.when(confirming, |d| d.child(button(("part-no", ix), "Cancel", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| {
                        m.confirm = None;
                        cx.notify();
                    }))))
                    .child(
                        button(("part", ix), if confirming { format!("Yes, {}", verb.to_lowercase()) } else { verb.to_string() }, ButtonKind::Secondary)
                            .when(confirming, |b| b.bg(Tokens::danger()).text_color(Tokens::base()).border_color(Tokens::danger()))
                            .on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.run(part, cx))),
                    )
                }
            });
        let r = row(title, desc, control);
        if ix == 0 {
            r
        } else {
            divided(r)
        }
    }
}

impl Render for StorageManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = card("Delete & reset", Some("Free space or start over on one part at a time. Anything a running part of the app holds open is finished by a restart."));
        for (ix, (p, title, desc)) in PARTS.iter().enumerate() {
            parts = parts.child(self.part_row(ix, *p, title, desc, cx));
        }
        let everything = Part::Everything;
        let confirming_all = self.confirm == Some(everything);
        let all_queued = self.pending.reset.contains(&everything);
        let reset_all = card("Reset MangaVox", Some("Deletes everything in the data folder: library, downloads, Studio work, models, chats and settings. MangaVox starts like a fresh install.")).child(row(
            "Factory reset",
            &format!("{} in {}", self.size_text(everything), paths::root().display()),
            div()
                .flex()
                .gap(px(8.))
                .when(all_queued, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("After restart")))
                .when(!all_queued && confirming_all, |d| {
                    d.child(button("reset-all-no", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| {
                        m.confirm = None;
                        cx.notify();
                    })))
                })
                .when(!all_queued, |d| {
                    d.child(
                        button("reset-all", if confirming_all { "Yes, erase everything" } else { "Reset MangaVox" }, ButtonKind::Secondary)
                            .text_color(if confirming_all { Tokens::base() } else { Tokens::danger() })
                            .when(confirming_all, |b| b.bg(Tokens::danger()).border_color(Tokens::danger()))
                            .on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.run(everything, cx))),
                    )
                }),
        ));
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .children(self.pending_banner(cx))
            .when_some(self.note.clone(), |d, (text, error)| {
                d.child(div().text_size(px(size::SM)).text_color(if error { Tokens::danger() } else { Tokens::text_2() }).child(text))
            })
            .child(self.folder_card(cx))
            .child(parts)
            .child(reset_all)
    }
}
