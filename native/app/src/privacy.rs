//! Private mode in the shell: the title-bar toggle and "Private" pill on Manga, Anime and Movies
//! pages, a faint tinted edge round those pages while it is on, and the end-of-session offer to
//! clear what the session created (see `mv_core::privacy` for what is and is not recorded).
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::privacy::{self, human_bytes, Choice, CleanPlan, Probe, Session};
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, checkbox, icon_button, icon_tag, ph, ph_icon, size, ButtonKind, RampExt as _, Route, Tokens, Tone, Weight};

use crate::routes::is_workspace;

/// Pages of the Manga, Anime and Movies areas: the only places private mode is offered and
/// shown. Studio, Thumbnails, YouTube, Settings, the phone page and the welcome setup never
/// show it, nor does the player on a local video (a Studio recap).
pub fn in_scope(route: &Route) -> bool {
    match route {
        Route::Settings { .. } | Route::Mobile | Route::Welcome | Route::Back => false,
        Route::Player { request } => ui_player::backend::ident(request).tracked,
        r => !is_workspace(r),
    }
}

/// The toggle (and, while on, the "Private" pill) for the right side of the title bar.
pub fn title_controls(route: &Route) -> Option<Div> {
    if !in_scope(route) {
        return None;
    }
    let on = privacy::active();
    let tip = if on { "Turn off private mode" } else { "Private mode: nothing you read or watch is recorded" };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .when(on, |d| d.child(icon_tag(ph::EYE_SLASH, "Private", Tone::Info).rounded(px(radius::PILL))))
            .child(
                icon_button("private-mode", ph::EYE_SLASH.on(on), tip)
                    .selected(on)
                    .when(on, |b| b.color(Tokens::info()))
                    .occlude()
                    .on_click(|_, w, cx| toggle(w, cx)),
            ),
    )
}

/// A faint tinted edge round the page while private mode is on (it takes no clicks).
pub fn page_tint(route: &Route) -> Option<Div> {
    (privacy::active() && in_scope(route)).then(|| div().absolute().inset_0().border_1().border_color(Tokens::info().alpha(0.45)))
}

fn toggle(window: &mut Window, cx: &mut App) {
    if privacy::active() {
        offer(privacy::end(), false, window, cx);
    } else {
        privacy::start();
        toast::show(cx, Toast::info("Private mode is on. Reading, watching and searches are not recorded.").icon(ph::EYE_SLASH));
    }
    window.refresh();
}

/// At startup: the app was closed while private mode was on (or before its clean-up offer was
/// answered), so the offer comes now.
pub fn check_on_start(window: &mut Window, cx: &mut App) {
    if let Some(s) = privacy::pending() {
        offer(s, true, window, cx);
    }
}

/// Works out what the session left behind and offers to clear it.
fn offer(session: Session, restarted: bool, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let res = mv_ui::bg::blocking(move || DiskProbe::load(&session).map(|p| (privacy::plan(&session, &p), session))).await;
            let _ = cx.update(|window, cx| match res {
                Ok(Ok((plan, _))) if plan.is_empty() && plan.studio_kept.is_empty() => {
                    privacy::finish();
                    if !restarted {
                        toast::show(cx, Toast::info("Private mode is off. Nothing from the session was kept.").icon(ph::EYE));
                    }
                }
                Ok(Ok((plan, session))) => open(plan, session, restarted, window, cx),
                Ok(Err(e)) | Err(e) => toast::show(cx, Toast::error(format!("Could not check what the private session saved. {e:#}"))),
            });
        })
        .detach();
}

/// The dialog's state: what can be cleared and which parts are ticked.
struct CleanView {
    plan: CleanPlan,
    choice: Choice,
    restarted: bool,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl CleanView {
    fn row(&self, id: &'static str, on: bool, label: String, note: Option<String>, set: fn(&mut Choice, bool), cx: &mut Context<Self>) -> Stateful<Div> {
        checkbox(id, on)
            .items_start()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .pt(px(3.))
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(label))
                    .when_some(note, |d, n| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(n))),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                set(&mut this.choice, !on);
                cx.notify();
            }))
    }
}

impl Render for CleanView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.plan.clone();
        let c = self.choice;
        let intro = if self.restarted {
            "MangaVox was closed while private mode was on. Nothing you read or watched was recorded; these were created during the session:"
        } else {
            "Nothing you read or watched was recorded. These were created during the session:"
        };
        let mut rows = div().flex().flex_col().gap(px(8.));
        if !p.downloads.is_empty() {
            let label = format!("{} downloaded ({})", plural(p.downloads.len(), "chapter", "chapters"), human_bytes(p.download_bytes));
            rows = rows.child(self.row("clear-downloads", c.downloads, label, None, |c, v| c.downloads = v, cx));
        }
        if !p.library.is_empty() || !p.list.is_empty() {
            let mut parts = vec![];
            if !p.library.is_empty() {
                parts.push(format!("{} added to your library", plural(p.library.len(), "manga", "manga")));
            }
            if !p.list.is_empty() {
                parts.push(format!("{} added to My List", plural(p.list.len(), "title", "titles")));
            }
            rows = rows.child(self.row("clear-library", c.library, parts.join(", "), None, |c, v| c.library = v, cx));
        }
        if !p.titles.is_empty() {
            let label = format!("{} opened for the first time", plural(p.titles.len(), "manga", "manga"));
            rows = rows.child(self.row("clear-titles", c.titles, label, Some("Their details and chapter lists".into()), |c, v| c.titles = v, cx));
        }
        if !p.files.is_empty() {
            let label = format!("Cached images and pages ({})", human_bytes(p.file_bytes));
            rows = rows.child(self.row("clear-files", c.files, label, Some(plural(p.files.len(), "file", "files")), |c, v| c.files = v, cx));
        }
        let studio = (!p.studio_kept.is_empty()).then(|| {
            div()
                .flex()
                .gap(px(8.))
                .items_start()
                .child(ph_icon(ph::INFO, Weight::Regular, Tokens::text_3(), icon_px::SM))
                .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_3()).child(format!(
                    "{} Studio work, so {} kept with {}.",
                    plural(p.studio_kept.len(), "chapter downloaded in the session has", "chapters downloaded in the session have"),
                    if p.studio_kept.len() == 1 { "it is" } else { "they are" },
                    if p.studio_kept.len() == 1 { "its download" } else { "their downloads" },
                )))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_2()).child(intro))
            .child(rows)
            .children(studio)
    }
}

fn open(plan: CleanPlan, session: Session, restarted: bool, window: &mut Window, cx: &mut App) {
    let view = cx.new(|_| CleanView { plan, choice: Choice::ALL, restarted });
    window.open_dialog(cx, move |dialog, _, _| {
        let heading = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(ph_icon(ph::EYE_SLASH, Weight::Fill, Tokens::info(), icon_px::LG))
            .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child("Clear your private session?"));
        let (v, downloads) = (view.clone(), session.downloads.clone());
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .w_full()
            .child(btn("private-keep", "Keep everything", ButtonKind::Secondary).on_click(|_, w, cx| {
                w.close_dialog(cx);
                privacy::finish();
            }))
            .child(btn("private-clear", "Clear selected", ButtonKind::Primary).icon(ph::BROOM).on_click(move |_, w, cx| {
                let (plan, choice) = v.read_with(cx, |v, _| (v.plan.clone(), v.choice));
                w.close_dialog(cx);
                clear(plan.selected(choice), choice.downloads, &downloads, cx);
            }));
        // Esc or the backdrop decides nothing: the offer comes back at the next start.
        dialog.w(px(480.)).title(heading).child(view.clone()).footer(footer).on_ok(|_, _, _| false).on_cancel(|_, _, _| true)
    });
}

/// Clears the selected part of the session, then forgets its record.
fn clear(plan: CleanPlan, stop_downloads: bool, session_downloads: &[(String, String)], cx: &mut App) {
    // Chapters of the session still queued or downloading stop first.
    let dl = ui_manga::downloads(cx);
    if stop_downloads && session_downloads.iter().any(|(_, c)| dl.read(cx).status(c).is_some()) {
        dl.update(cx, |d, cx| {
            for (_, c) in session_downloads {
                if d.status(c).is_some() {
                    d.cancel_job(c, cx);
                }
            }
        });
    }
    cx.spawn(async move |cx| {
        let res = mv_ui::bg::blocking(move || remove(&plan)).await;
        let _ = cx.update(|cx| match res {
            Ok(Ok(())) => {
                privacy::finish();
                ui_media::list_changed();
                toast::show(cx, Toast::success("Private session cleared").icon(ph::EYE_SLASH));
            }
            Ok(Err(e)) | Err(e) => toast::show(cx, Toast::error(format!("Could not clear everything from the private session. {e:#}"))),
        });
    })
    .detach();
}

/// Deletes what `plan` names: download folders, library and My List additions, the title rows
/// the session created and its cache files. Blocking.
fn remove(plan: &CleanPlan) -> anyhow::Result<()> {
    let root = ui_manga::backend::downloads_dir();
    if !plan.downloads.is_empty() {
        ui_manga::backend::with_db(|c| {
            for (m, ch) in &plan.downloads {
                // A chapter saved before the manga folder changed lives where the database says.
                let stored = mv_core::repo::chapter::get(c, ch)?.and_then(|x| x.pages_dir).map(PathBuf::from);
                if let Some(d) = stored.filter(|d| d.file_name().is_some_and(|n| n.to_string_lossy() == *ch)) {
                    let _ = std::fs::remove_dir_all(d);
                }
                let _ = std::fs::remove_dir_all(root.join(m).join(ch));
                c.execute("UPDATE chapter SET download_status = 'none', pages_dir = NULL WHERE id = ?1", [ch])?;
            }
            Ok(())
        })?;
        // Titles left without any downloads lose their (now empty) folder too.
        for m in plan.downloads.iter().map(|(m, _)| m).collect::<BTreeSet<_>>() {
            let d = root.join(m);
            if std::fs::read_dir(&d).map(|mut r| r.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(&d);
            }
        }
    }
    for m in &plan.library {
        ui_manga::backend::set_in_library(m, false);
    }
    if !plan.list.is_empty() {
        let conn = mv_core::db::open_default()?;
        for id in &plan.list {
            mv_core::repo::watchlist::remove_status(&conn, id)?;
        }
    }
    ui_manga::backend::forget_titles(&plan.titles)?;
    for f in &plan.files {
        let _ = std::fs::remove_file(f);
    }
    ui_manga::backend::bump_data();
    Ok(())
}

/// The state of what a session record names, read once from the database and the disk.
struct DiskProbe {
    downloads_root: PathBuf,
    /// Recorded chapters that still exist: their stored download folder.
    pages_dirs: HashMap<String, Option<PathBuf>>,
    titles: HashSet<String>,
    browse_only: HashSet<String>,
    list: HashSet<String>,
}

impl DiskProbe {
    fn load(s: &Session) -> anyhow::Result<DiskProbe> {
        let conn = mv_core::db::open_default()?;
        let mut pages_dirs = HashMap::new();
        for (_, c) in &s.downloads {
            if let Some(ch) = mv_core::repo::chapter::get(&conn, c)? {
                pages_dirs.insert(c.clone(), ch.pages_dir.map(PathBuf::from));
            }
        }
        let mut titles = HashSet::new();
        for m in s.titles.iter().chain(&s.library_adds).chain(s.downloads.iter().map(|(m, _)| m)) {
            if mv_core::repo::manga::get(&conn, m)?.is_some() {
                titles.insert(m.clone());
            }
        }
        let list = mv_core::repo::watchlist::list(&conn, None)?.into_iter().map(|e| e.media_id).collect();
        Ok(DiskProbe { downloads_root: ui_manga::backend::downloads_dir(), pages_dirs, titles, browse_only: ui_manga::backend::browse_only(), list })
    }
}

fn dir_size(d: &Path) -> u64 {
    std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn not_empty(d: &Path) -> bool {
    std::fs::read_dir(d).map(|mut r| r.next().is_some()).unwrap_or(false)
}

impl Probe for DiskProbe {
    fn download_size(&self, manga: &str, chapter: &str) -> Option<u64> {
        let stored = self.pages_dirs.get(chapter)?.clone();
        let dir = stored.filter(|d| d.is_dir()).unwrap_or_else(|| self.downloads_root.join(manga).join(chapter));
        not_empty(&dir).then(|| dir_size(&dir))
    }

    fn chapter_in_studio(&self, manga: &str, chapter: &str) -> bool {
        not_empty(&mv_core::paths::studio_chapter(manga, chapter))
    }

    fn title_in_studio(&self, manga: &str) -> bool {
        not_empty(&mv_core::paths::studio_projects().join(mv_core::paths::safe_name(manga))) || not_empty(&mv_core::paths::studio_manga(manga))
    }

    fn in_library(&self, manga: &str) -> bool {
        self.titles.contains(manga) && !self.browse_only.contains(manga)
    }

    fn in_list(&self, media: &str) -> bool {
        self.list.contains(media)
    }

    fn title_exists(&self, manga: &str) -> bool {
        self.titles.contains(manga)
    }

    fn file_info(&self, path: &Path) -> Option<(u64, i64)> {
        let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
        let at = m.created().or_else(|_| m.modified()).ok()?;
        let secs = at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        Some((m.len(), secs))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::in_scope;
    use mv_ui::Route;

    #[test]
    fn offered_only_in_manga_anime_and_movies() {
        for r in [Route::Library, Route::Browse, Route::MangaHistory, Route::MangaDownloads, Route::AnimeHome, Route::MoviesHome, Route::Discover, Route::MediaHistory, Route::Favorites] {
            assert!(in_scope(&r), "{r:?}");
        }
        assert!(in_scope(&Route::Reader { manga_id: "m".into(), chapter_id: "c".into() }));
        for r in [Route::Studio, Route::Thumbnails, Route::YouTube, Route::Suggestions, Route::Settings { tab: None }, Route::Mobile, Route::Welcome] {
            assert!(!in_scope(&r), "{r:?}");
        }
        let play = |mal: Option<&str>| Route::Player { request: mv_ui::nav::PlayRequest { title: "x".into(), source: "x".into(), file_idx: None, imdb_id: None, mal_id: mal.map(Into::into), season: None, episode: None } };
        assert!(in_scope(&play(Some("21"))), "an anime episode");
        assert!(!in_scope(&play(None)), "a Studio recap video");
    }
}
