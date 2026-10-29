//! Settings → Storage, top to bottom: where the space goes (a stacked usage bar with a legend),
//! the download folders, the cache, the data folder (and moving it), then one danger zone with
//! every delete and the factory reset. Work on files other parts of the app hold open is queued
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
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, confirm_inline, glyphs, icon_button, inline_alert, ph, ph_icon, progress_bar, setting_row, setting_section, size, skeleton, skeleton_line, spinner, usage_bar, AlertKind, ButtonKind, Hue, Ph, Tokens, Weight};

use crate::data;
use crate::ui::{danger_zone, divided, format_bytes, path_row};

/// What can be deleted or reset, in the order shown: (part, title, what it removes).
const PARTS: &[(Part, &str, &str)] = &[
    (Part::MangaDownloads, "Downloaded manga chapters", "Offline pages. Chapters go back to online-only; your library and Studio work stay."),
    (Part::VideoDownloads, "Anime & movie downloads", "Files in the default anime, movie and subtitle folders under the data folder."),
    (Part::Studio, "Studio work", "AI reads, scripts, narration, videos and character brains. Studio settings stay."),
    (Part::Chats, "AI chats", "Every Assistant conversation."),
    (
        Part::Models,
        "Local AI models & voices",
        "Downloaded models and voices, voice samples, the voice engines and the private Python and package cache voice setup installed. Voices need setting up again in Audio.",
    ),
    (Part::Library, "Library & history", "Every title, reading and watch history, downloaded chapters and Studio work."),
    (Part::Settings, "Settings", "Everything in Settings goes back to its default."),
];

/// Segments of the usage bar: parts that do not overlap, each with a legend icon and hue.
/// Apple: Storage in System Settings shows space by category, <https://support.apple.com/en-us/102624>.
const USAGE: [(Part, &str, Ph, Hue); 6] = [
    (Part::MangaDownloads, "Manga chapters", glyphs::MANGA, Hue::Blue),
    (Part::VideoDownloads, "Anime & movies", ph::FILM_STRIP, Hue::Violet),
    (Part::Studio, "Studio work", glyphs::STUDIO, Hue::Rose),
    (Part::Models, "Models & voices", glyphs::MODEL, Hue::Green),
    (Part::Chats, "AI chats", ph::CHAT_CIRCLE, Hue::Amber),
    (Part::Cache, "Cache", ph::DATABASE, Hue::Cyan),
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
    /// Free space on the drive that holds the data folder.
    free: Option<u64>,
    /// Part whose Delete was pressed (the inline confirm is showing).
    confirm: Option<Part>,
    moving: Option<Move>,
    /// Queued work waiting for a restart (from the pointer file).
    pending: Location,
    /// Custom folder per `data::STORAGE_KINDS` entry ("" = default).
    folders: Vec<String>,
}

fn key(p: Part) -> String {
    format!("{p:?}")
}

pub fn create(cx: &mut App) -> Entity<StorageManager> {
    cx.new(|cx| {
        let mut m = StorageManager {
            sizes: HashMap::new(),
            measuring: false,
            free: None,
            confirm: None,
            moving: None,
            pending: Location::load(),
            folders: vec![String::new(); data::STORAGE_KINDS.len()],
        };
        m.refresh(cx);
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
    /// Measures every part and the free space, and rereads the folder settings.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.measuring {
            return;
        }
        self.measuring = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(|| {
                let sizes = PARTS.iter().map(|(p, _, _)| *p).chain([Part::Cache, Part::Everything]).map(|p| (key(p), p.size())).collect::<HashMap<_, _>>();
                (sizes, crate::models::scan::free_space(&paths::root()), data::load_storage_folders())
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                m.measuring = false;
                if let Ok((sizes, free, folders)) = r {
                    m.sizes = sizes;
                    m.free = free;
                    m.folders = folders;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn say(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let text: String = text.into();
        toast::show(cx, if error { Toast::error(text) } else { Toast::success(text) });
        cx.notify();
    }

    fn size_of(&self, p: Part) -> Option<u64> {
        self.sizes.get(&key(p)).copied()
    }

    fn size_text(&self, p: Part) -> String {
        self.size_of(p).map(format_bytes).unwrap_or_else(|| "…".into())
    }

    /// Deletes (or queues for the restart) one part. The inline confirm already ran.
    fn run(&mut self, part: Part, cx: &mut Context<Self>) {
        self.confirm = None;
        let title = PARTS.iter().find(|(p, _, _)| *p == part).map(|(_, t, _)| *t).unwrap_or("Everything");
        if part.live() {
            cx.spawn(async move |this, cx| {
                let r = mv_ui::bg::blocking(move || paths::wipe(&paths::root(), part)).await.and_then(|r| r);
                let _ = this.update(cx, |m, cx| {
                    match r {
                        Ok(()) => m.say(format!("{title} deleted"), false, cx),
                        Err(e) => m.say(format!("Could not delete {}: {e:#}", title.to_lowercase()), true, cx),
                    }
                    m.refresh(cx);
                });
            })
            .detach();
            return cx.notify();
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
        let cancelled = loc.pending_move.take();
        // A switch to another folder without moving is cancelled too.
        if loc.root.as_ref().is_some_and(|r| *r != paths::root()) {
            loc.root = (paths::root() != paths::default_root()).then(paths::root);
        }
        match loc.save() {
            Ok(()) => {
                self.pending = loc;
                self.say("Queued changes cancelled", false, cx);
                // The copy made for the move is not needed any more.
                if let Some(m) = cancelled {
                    cx.background_executor().spawn(async move { paths::discard_incomplete(&m.to) }).detach();
                }
            }
            Err(e) => self.say(format!("{e:#}"), true, cx),
        }
    }

    /// Hides the result of the last move (shown once after the restart).
    fn forget_move_result(&mut self, cx: &mut Context<Self>) {
        let mut loc = Location::load();
        loc.moved = None;
        loc.move_error = None;
        let _ = loc.save();
        self.pending = loc;
        cx.notify();
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
        let to = if paths::has_data(&to) || paths::is_empty_dir(&to) || paths::is_unfinished_copy(&to) { to } else { to.join("MangaVox") };
        match paths::check_new_root(&to) {
            Ok(()) => {
                let has_data = paths::has_data(&to);
                self.moving = Some(Move::Choose { to, has_data });
            }
            Err(e) => self.say(format!("{e:#}"), true, cx),
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
            let work = mv_ui::bg::blocking(move || -> Result<bool> {
                let from = paths::root();
                paths::check_new_root(&dest)?;
                // On the same drive the move at restart is a rename: nothing to copy now.
                let same_drive = paths::is_empty_dir(&dest) && paths::same_volume(&from, &dest);
                if !same_drive {
                    let need = paths::copy_size(&from).saturating_sub(paths::dir_size(&dest));
                    if let Some(free) = crate::models::scan::free_space(&dest) {
                        const MARGIN: u64 = 512 * 1024 * 1024;
                        if need.saturating_add(MARGIN) > free {
                            anyhow::bail!("Not enough space there: the data needs {} and that drive has {} free.", format_bytes(need), format_bytes(free));
                        }
                    }
                    // Copy ahead only into a folder of our own: data being replaced stays as it
                    // is until the restart, so cancelling leaves it untouched. Files in use now
                    // are copied at the restart.
                    if paths::is_empty_dir(&dest) || paths::is_unfinished_copy(&dest) {
                        paths::precopy(&from, &dest, &|n, all| {
                            d.store(n, Ordering::Relaxed);
                            t.store(all, Ordering::Relaxed);
                        })?;
                    }
                }
                paths::choose_root(&dest, true)?;
                Ok(same_drive)
            });
            let r: Result<bool> = work.await.and_then(|r| r);
            let _ = this.update(cx, |m, cx| {
                m.moving = None;
                m.pending = Location::load();
                match r {
                    Ok(true) => m.say(format!("Ready. Restart MangaVox to move the data to {} (same drive, so it is quick).", to.display()), false, cx),
                    Ok(false) => m.say(format!("Copied. Restart MangaVox to finish moving to {}.", to.display()), false, cx),
                    Err(e) => m.say(format!("Could not move the data: {e:#}. MangaVox still uses {}.", paths::root().display()), true, cx),
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

    // ── Download folders and cache ───────────────────────────────────────────────────────────

    fn pick_storage(&mut self, idx: usize, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else { return };
            let kind = data::STORAGE_KINDS[idx].0;
            let saved = mv_ui::bg::blocking(move || data::save_storage(kind, &path.to_string_lossy())).await.and_then(|r| r);
            let _ = this.update(cx, |m, cx| match saved {
                Ok(value) => {
                    m.folders[idx] = value;
                    m.say(format!("{} now save to the new folder", data::STORAGE_KINDS[idx].1), false, cx);
                }
                Err(e) => m.say(format!("{e:#}"), true, cx),
            });
        })
        .detach();
    }

    fn reset_storage(&mut self, idx: usize, cx: &mut Context<Self>) {
        let kind = data::STORAGE_KINDS[idx].0;
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || data::save_storage(kind, "")).await.and_then(|r| r);
            let _ = this.update(cx, |m, cx| match r {
                Ok(value) => {
                    m.folders[idx] = value;
                    m.say("Folder set back to the default", false, cx);
                }
                Err(e) => m.say(format!("{e:#}"), true, cx),
            });
        })
        .detach();
    }

    fn reveal(path: PathBuf, cx: &mut App) {
        let _ = std::fs::create_dir_all(&path);
        cx.reveal_path(&path);
    }

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(data::clear_cache).await.and_then(|r| r);
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(()) => m.say("Cache cleared. Covers download again as you browse.", false, cx),
                    Err(e) => m.say(format!("Could not clear the cache: {e:#}"), true, cx),
                }
                m.refresh(cx);
            });
        })
        .detach();
    }

    // ── Render ──────────────────────────────────────────────────────────────────────────────

    fn render_usage(&self) -> Div {
        let section = setting_section("Space used", Some(format!("Everything MangaVox keeps lives in {}.", paths::root().display()).into()));
        let Some(total) = self.size_of(Part::Everything).filter(|_| !self.sizes.is_empty()) else {
            // Shaped like the bar and legend that are coming. NN/g skeleton screens:
            // <https://www.nngroup.com/articles/skeleton-screens/>
            return section.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .py(px(12.))
                    .child(skeleton_line(px(180.)))
                    .child(skeleton(relative(1.), px(8.), radius::PILL))
                    .child(div().grid().grid_cols(3).gap(px(10.)).children((0..6).map(|_| skeleton_line(relative(0.7))))),
            );
        };
        let parts: Vec<(Part, &str, Ph, Hue, u64)> = USAGE.iter().map(|(p, l, i, h)| (*p, *l, *i, *h, self.size_of(*p).unwrap_or(0))).collect();
        let known: u64 = parts.iter().map(|p| p.4).sum();
        let other = total.saturating_sub(known);
        let mut segments: Vec<(f32, Rgba)> = parts.iter().map(|p| (p.4 as f32, Tokens::hue(p.3))).collect();
        segments.push((other as f32, Tokens::hue(Hue::Slate)));
        let legend_item = |icon: Ph, hue: Hue, label: &'static str, bytes: u64| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(div().flex_none().size(px(8.)).rounded_full().bg(Tokens::hue(hue)))
                .child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::SM))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(label))
                .child(div().flex_none().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format_bytes(bytes)))
        };
        let legend = div()
            .grid()
            .grid_cols(2)
            .gap_x(px(24.))
            .gap_y(px(8.))
            .children(parts.iter().map(|p| legend_item(p.2, p.3, p.1, p.4)))
            .child(legend_item(ph::BOOKS, Hue::Slate, "Library, settings & other", other));
        section.child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .py(px(12.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format_bytes(total)))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("used by MangaVox"))
                        .child(div().flex_1())
                        .when(self.measuring, |d| d.child(spinner("storage-measure", Tokens::text_3(), icon_px::SM)))
                        .when_some(self.free, |d, f| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!("{} free on this drive", format_bytes(f))))),
                )
                .child(usage_bar(&segments, total as f32))
                .child(legend),
        )
    }

    fn render_folders(&self, cx: &mut Context<Self>) -> Div {
        let mut folders = setting_section("Download folders", Some("Where each kind of download is saved. Changes apply to new downloads; existing files stay where they are.".into()));
        for (idx, (kind, label, hint)) in data::STORAGE_KINDS.iter().enumerate() {
            let custom = self.folders.get(idx).cloned().unwrap_or_default();
            let effective = if custom.is_empty() { data::storage_default(kind) } else { PathBuf::from(&custom) };
            let icon = match *kind {
                "anime" => glyphs::ANIME,
                "movies" => glyphs::MOVIES,
                "manga" => glyphs::MANGA,
                _ => glyphs::SUBTITLES,
            };
            let open = effective.clone();
            let actions = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(icon_button(("storage-open", idx), ph::FOLDER_OPEN, "Show in Explorer").on_click(move |_, _, cx| Self::reveal(open.clone(), cx)))
                .child(icon_button(("storage-change", idx), ph::PENCIL_SIMPLE, "Change folder…").on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.pick_storage(idx, cx))))
                .when(!custom.is_empty(), |d| {
                    d.child(icon_button(("storage-reset", idx), ph::ARROW_COUNTER_CLOCKWISE, "Use the default folder").on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.reset_storage(idx, cx))))
                });
            let row = path_row(icon, *label, *hint, effective.display().to_string(), actions);
            folders = folders.child(if idx == 0 { row } else { divided(row) });
        }
        folders
    }

    fn render_cache(&self, cx: &mut Context<Self>) -> Div {
        let size = self.size_of(Part::Cache);
        setting_section("Cache", Some("Cover images and other data MangaVox can fetch again. Your library and downloads are not affected.".into())).child(setting_row(
            "Cache size",
            size.map(format_bytes).unwrap_or_else(|| "Measuring…".into()),
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(icon_button("cache-open", ph::FOLDER_OPEN, "Show in Explorer").on_click(|_, _, cx| Self::reveal(data::cache_dir(), cx)))
                .child(
                    btn("cache-clear", "Clear cache", ButtonKind::Secondary)
                        .icon(ph::BROOM)
                        .when(size == Some(0), |b| b.disabled("The cache is already empty"))
                        .on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.clear_cache(cx))),
                ),
        ))
    }

    fn render_data_folder(&self, cx: &mut Context<Self>) -> Div {
        let root = paths::root();
        let env_set = std::env::var_os("MANGAVOX_HOME").is_some_and(|p| !p.is_empty());
        let custom = !env_set && root != paths::default_root();
        let busy = matches!(self.moving, Some(Move::Copying { .. })) || self.pending.pending_move.is_some();
        let r2 = root.clone();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(icon_button("data-open", ph::FOLDER_OPEN, "Show in Explorer").on_click(move |_, _, cx| Self::reveal(r2.clone(), cx)))
            .when(custom && !busy, |d| {
                d.child(icon_button("data-default", ph::ARROW_COUNTER_CLOCKWISE, "Go back to the default folder").on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.offer(paths::default_root(), cx))))
            })
            .child(
                btn("data-change", "Move…", ButtonKind::Secondary)
                    .icon(ph::FOLDER_SIMPLE_PLUS)
                    .when(env_set, |b| b.disabled("The MANGAVOX_HOME environment variable sets the data folder"))
                    .when(!env_set && busy, |b| b.disabled("A move is already under way"))
                    .on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.pick_folder(cx))),
            );
        let hint = if env_set {
            "Set by MANGAVOX_HOME"
        } else if custom {
            "A custom folder"
        } else {
            "The default folder"
        };
        let mut c = setting_section("Data folder", Some("Library, downloads, Studio work, models, chats, settings and cache all live here. Pick another drive or folder to move it.".into()))
            .child(path_row(glyphs::FOLDER, "Location", hint, root.display().to_string(), actions));
        let weak = cx.entity().downgrade();
        if let Some(err) = self.pending.move_error.clone() {
            let w = weak.clone();
            c = c.child(div().my(px(8.)).child(inline_alert(AlertKind::Error, "The data folder was not moved", err).dismissible(move |_, _, cx| {
                let _ = w.update(cx, |m, cx| m.forget_move_result(cx));
            })));
        } else if let Some(done) = self.pending.moved.clone() {
            let w = weak.clone();
            let body = match &self.pending.cleanup {
                Some(old) => format!("{done}. The old folder {} is being removed.", old.display()),
                None => format!("{done}."),
            };
            c = c.child(div().my(px(8.)).child(inline_alert(AlertKind::Success, "Data folder moved", body).dismissible(move |_, _, cx| {
                let _ = w.update(cx, |m, cx| m.forget_move_result(cx));
            })));
        } else if let Some(old) = self.pending.cleanup.clone() {
            c = c.child(div().my(px(8.)).child(inline_alert(AlertKind::Info, "Removing the old data folder", format!("{} is deleted in the background; files still in use go at the next start.", old.display()))));
        }
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
                        .rounded(px(radius::LG))
                        .bg(Tokens::surface())
                        .border_1()
                        .border_color(Tokens::border_strong())
                        .child(div().flex().items_center().gap(px(8.)).child(ph_icon(ph::FOLDER_OPEN, Weight::Fill, Tokens::accent(), icon_px::MD)).child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child("New data folder")))
                        .child(mv_ui::path_chip(to.display().to_string()))
                        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(if *has_data {
                            "That folder already has MangaVox data. Use it as it is (your current data stays where it is), or replace it with your current data: what is there now is deleted when MangaVox restarts."
                        } else {
                            "Moving copies everything there now (on the same drive it is a quick rename at restart). When you restart, MangaVox finishes the copy, checks every file, updates saved paths, switches to the new folder and then deletes the old one. If anything fails it keeps using the old folder."
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_end()
                                .gap(px(8.))
                                .child(btn("mv-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| {
                                    m.moving = None;
                                    cx.notify();
                                })))
                                .when(!*has_data, |d| d.child(btn("mv-empty", "Start empty there", ButtonKind::Secondary).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(c2.clone(), false, cx)))))
                                .when(*has_data, |d| d.child(btn("mv-replace", "Replace with my data", ButtonKind::Secondary).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(b.clone(), true, cx)))))
                                .child(if *has_data {
                                    btn("mv-use", "Use the data there", ButtonKind::Primary).icon(ph::CHECK).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(a.clone(), false, cx)))
                                } else {
                                    btn("mv-move", "Move my data", ButtonKind::Primary).icon(ph::ARROW_RIGHT).on_click(cx.listener(move |m, _: &ClickEvent, _, cx| m.apply(a.clone(), true, cx)))
                                }),
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
                                .child(spinner("mv-copy-spin", Tokens::accent(), icon_px::SM))
                                .child(div().flex_1().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("Copying to {}", to.display())))
                                .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(if t == 0 { "Checking space…".to_string() } else { format!("{} of {}", format_bytes(d), format_bytes(t)) })),
                        )
                        .child(progress_bar("mv-copy-bar", frac, true)),
                );
            }
            None => {}
        }
        c
    }

    fn pending_alert(&self, cx: &mut Context<Self>) -> Option<Div> {
        let loc = &self.pending;
        if loc.reset.is_empty() && loc.pending_move.is_none() && loc.root.as_ref().is_none_or(|r| *r == paths::root()) {
            return None;
        }
        let mut what: Vec<String> = loc.reset.iter().map(|p| PARTS.iter().find(|(q, _, _)| q == p).map(|(_, t, _)| t.to_lowercase()).unwrap_or_else(|| "erase everything".into())).collect();
        if let Some(m) = &loc.pending_move {
            what.push(format!("move the data to {}", m.to.display()));
        } else if let Some(r) = loc.root.as_ref().filter(|r| **r != paths::root()) {
            what.push(format!("switch to {}", r.display()));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(inline_alert(AlertKind::Warning, "Restart to finish", format!("Waiting for a restart: {}.", what.join(", "))))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(btn("pend-cancel", "Cancel these changes", ButtonKind::Ghost).on_click(cx.listener(|m, _: &ClickEvent, _, cx| m.cancel_pending(cx))))
                        .child(btn("pend-restart", "Restart now", ButtonKind::Primary).icon(ph::ARROW_CLOCKWISE).on_click(|_, _, cx| restart(cx))),
                ),
        )
    }

    fn part_row(&self, ix: usize, part: Part, title: &str, desc: &str, cx: &mut Context<Self>) -> Div {
        let queued = self.pending.reset.contains(&part);
        let settings = part == Part::Settings;
        let control: AnyElement = if queued {
            mv_ui::status_tag(mv_ui::StatusKind::Queued).label("After restart").into_any_element()
        } else if self.confirm == Some(part) {
            // NN/g: name the item and its consequence, label the button with the action.
            let prompt = if settings { "Reset every setting?".to_string() } else { format!("Delete {}?", self.size_text(part)) };
            confirm_inline(prompt, if settings { "Reset" } else { "Delete" }, true)
                .cancel_label("Keep")
                .on_cancel(cx.listener(|m, _: &ClickEvent, _, cx| {
                    m.confirm = None;
                    cx.notify();
                }))
                .on_confirm(cx.listener(move |m, _: &ClickEvent, _, cx| m.run(part, cx)))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .when(!settings, |d| d.child(div().min_w(px(56.)).flex().justify_end().text_size(px(size::SM)).text_color(Tokens::text_2()).child(self.size_text(part))))
                .child(
                    btn(("part", ix), if settings { "Reset" } else { "Delete" }, ButtonKind::DangerGhost)
                        .icon(if settings { ph::ARROW_COUNTER_CLOCKWISE } else { glyphs::DELETE })
                        .when(!settings && self.size_of(part) == Some(0), |b| b.disabled("Nothing to delete"))
                        .on_click(cx.listener(move |m, _: &ClickEvent, _, cx| {
                            m.confirm = Some(part);
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        let r = setting_row(title.to_string(), desc.to_string(), control);
        if ix == 0 {
            r
        } else {
            divided(r)
        }
    }

    fn render_danger(&self, cx: &mut Context<Self>) -> Div {
        let mut zone = danger_zone("Danger zone", "Free space or start over, one part at a time. Deleted files cannot be recovered. Anything the app holds open is finished by a restart.");
        for (ix, (p, title, desc)) in PARTS.iter().enumerate() {
            zone = zone.child(self.part_row(ix, *p, title, desc, cx));
        }
        let everything = Part::Everything;
        let all_queued = self.pending.reset.contains(&everything);
        let total = self.size_text(everything);
        let control: AnyElement = if all_queued {
            mv_ui::status_tag(mv_ui::StatusKind::Queued).label("After restart").into_any_element()
        } else {
            btn("reset-all", "Erase everything", ButtonKind::Danger)
                .icon(ph::WARNING)
                .on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
                    let weak = cx.entity().downgrade();
                    // The most destructive action gets a modal: nothing focused, Enter does not
                    // confirm. Apple HIG alerts: <https://developer.apple.com/design/human-interface-guidelines/alerts>
                    mv_ui::confirm_dialog(
                        "Erase all MangaVox data?",
                        format!("Deletes {total} in {}: library, downloads, Studio work, models, chats and settings. MangaVox restarts like a fresh install.", paths::root().display()),
                        "Erase everything",
                        true,
                    )
                    .cancel_label("Keep my data")
                    .on_confirm(move |_, cx| {
                        let _ = weak.update(cx, |m, cx| m.run(everything, cx));
                    })
                    .open(window, cx);
                }))
                .into_any_element()
        };
        zone.child(divided(setting_row("Factory reset", "Deletes everything in the data folder. MangaVox starts like a fresh install.", control)))
    }
}

impl Render for StorageManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .children(self.pending_alert(cx))
            .child(self.render_usage())
            .child(self.render_folders(cx))
            .child(self.render_cache(cx))
            .child(self.render_data_folder(cx))
            .child(self.render_danger(cx))
    }
}
