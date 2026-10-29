//! Deleting manga from Studio: every file MangaVox keeps for them on this PC (the downloaded
//! pages, the Studio project folders with narration, videos, full recaps, captions and music,
//! and the manga's character brain), plus the database marks that say they are downloaded or
//! made. The manga and its chapter list stay in the Library. Only folders that resolve inside
//! MangaVox's own data folders are ever removed; anything else is kept and reported.
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui_kit::*;
use mv_core::paths;
use mv_ui::toast::{self, Toast};
use mv_ui::{confirm_dialog, fmt_bytes, glyphs, CheckState};

use crate::page::{Load, StudioPage, View};
use crate::{data, pipeline, run, service};

/// How many titles the confirmation lists before "and N more".
const LISTED: usize = 5;

/// A folder to remove: canonical, and checked to lie inside one of MangaVox's folders.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Doomed {
    pub path: PathBuf,
    pub bytes: u64,
}

/// What deleting one manga removes, measured before anything goes.
#[derive(Clone, Debug, Default)]
pub(crate) struct MangaPlan {
    pub id: String,
    pub title: String,
    /// Every chapter of the manga in the database (downloaded or not).
    pub chapters: Vec<String>,
    pub dirs: Vec<Doomed>,
    /// Folders kept because they were not safe to remove, with why.
    pub refused: Vec<String>,
}

impl MangaPlan {
    pub fn bytes(&self) -> u64 {
        self.dirs.iter().map(|d| d.bytes).sum()
    }
}

/// What a delete did.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Titles removed completely.
    pub deleted: Vec<String>,
    pub freed: u64,
    /// Titles that kept some files (in use), with those files.
    pub failed: Vec<(String, Vec<PathBuf>)>,
    pub refused: Vec<String>,
    pub errors: Vec<String>,
}

/// The folders a delete may touch, and the ones it must never remove (or remove around).
#[derive(Clone, Debug)]
pub(crate) struct Roots {
    pub projects: PathBuf,
    pub studio: PathBuf,
    /// Where downloaded chapters live: the Settings → Storage folder, then the default one.
    pub downloads: Vec<PathBuf>,
    pub protected: Vec<PathBuf>,
}

impl Roots {
    fn current() -> Roots {
        let custom = mv_core::config::Config::open_default()
            .get("storage.manga")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let mut downloads: Vec<PathBuf> = custom.into_iter().collect();
        if !downloads.contains(&paths::manga_downloads()) {
            downloads.push(paths::manga_downloads());
        }
        let mut protected = vec![paths::root(), paths::studio(), paths::studio_projects(), paths::library(), paths::cache(), paths::temp(), paths::database(), paths::config(), paths::music_library()];
        protected.extend(downloads.iter().cloned());
        protected.extend(dirs_home());
        Roots { projects: paths::studio_projects(), studio: paths::studio(), downloads, protected }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// `path`, canonical, when it may be removed: absolute, no `..`, not itself a link, resolving
/// strictly inside `root` at least `min_depth` levels down, and holding none of `protected`.
/// `Ok(None)` when it does not exist; `Err(why)` when it must be kept.
pub(crate) fn checked(path: &Path, root: &Path, min_depth: usize, protected: &[PathBuf]) -> Result<Option<PathBuf>, String> {
    let shown = path.display();
    if !path.is_absolute() {
        return Err(format!("{shown} is not a full path"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{shown} has \"..\" in it"));
    }
    let Ok(md) = std::fs::symlink_metadata(path) else { return Ok(None) };
    // A link or junction could point anywhere; it is never followed.
    if md.file_type().is_symlink() {
        return Err(format!("{shown} is a link"));
    }
    let root_c = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let p = path.canonicalize().map_err(|e| format!("{shown}: {e}"))?;
    let Ok(rel) = p.strip_prefix(&root_c) else { return Err(format!("{shown} is outside {}", root.display())) };
    if rel.components().count() < min_depth.max(1) {
        return Err(format!("{shown} is too close to {}", root.display()));
    }
    if let Some(x) = protected.iter().filter_map(|x| x.canonicalize().ok()).find(|x| x.starts_with(&p)) {
        return Err(format!("{shown} holds {}", x.display()));
    }
    Ok(Some(p))
}

/// Where a chapter's stored pages folder keeps its files: the chapter folder (`<chapter>/pages`
/// in older layouts kept the work next to it, in `<chapter>/pipeline`).
fn chapter_folder(pages_dir: &Path) -> &Path {
    match pages_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) if n.eq_ignore_ascii_case("pages") => pages_dir.parent().unwrap_or(pages_dir),
        _ => pages_dir,
    }
}

/// Works out (and measures) what deleting one manga removes. Blocking.
pub(crate) fn plan_in(id: &str, title: &str, chapters: &[(String, Option<PathBuf>)], r: &Roots) -> MangaPlan {
    let mut plan = MangaPlan { id: id.to_string(), title: title.to_string(), chapters: chapters.iter().map(|(c, _)| c.clone()).collect(), ..Default::default() };
    let mut found: Vec<PathBuf> = vec![];
    let mut take = |res: Result<Option<PathBuf>, String>, plan: &mut MangaPlan| match res {
        Ok(Some(p)) => found.push(p),
        Ok(None) => {}
        Err(e) => plan.refused.push(e),
    };
    take(checked(&r.projects.join(paths::safe_name(id)), &r.projects, 1, &r.protected), &mut plan);
    take(checked(&r.studio.join(mv_recap::safe_id(id)), &r.studio, 1, &r.protected), &mut plan);
    for root in &r.downloads {
        take(checked(&root.join(id), root, 1, &r.protected), &mut plan);
    }
    for dir in chapters.iter().filter_map(|(_, d)| d.as_deref()) {
        let folder = chapter_folder(dir);
        // Inside one of the manga folders, as `<manga>/<chapter>` at least.
        let tries: Vec<Result<Option<PathBuf>, String>> = r.downloads.iter().map(|root| checked(folder, root, 2, &r.protected)).collect();
        if let Some(ok) = tries.iter().find_map(|t| t.as_ref().ok()) {
            found.extend(ok.clone());
        } else if !folder.as_os_str().is_empty() {
            plan.refused.push(format!("{} is not inside MangaVox's manga folder", folder.display()));
        }
    }
    // Each folder once, and none inside another one that goes anyway.
    found.sort();
    found.dedup();
    let mut kept: Vec<PathBuf> = vec![];
    for p in found {
        if !kept.iter().any(|k| p.starts_with(k)) {
            kept.push(p);
        }
    }
    plan.dirs = kept.into_iter().map(|p| Doomed { bytes: paths::dir_size(&p), path: p }).collect();
    plan
}

/// [`plan_in`] for each `(manga id, title)` with the database and folders in use. Blocking.
pub(crate) fn plan(targets: &[(String, String)]) -> Vec<MangaPlan> {
    let r = Roots::current();
    targets
        .iter()
        .map(|(id, title)| {
            let chapters = data::chapters_of(id).unwrap_or_default();
            plan_in(id, title, &chapters, &r)
        })
        .collect()
}

/// Removes `p` and everything in it without following links (a link inside goes, never what it
/// points at). Files that would not go (in use) end up in `failed`.
fn remove_tree(p: &Path, failed: &mut Vec<PathBuf>) {
    let Ok(md) = std::fs::symlink_metadata(p) else { return };
    if md.file_type().is_symlink() {
        if std::fs::remove_file(p).is_err() && std::fs::remove_dir(p).is_err() {
            failed.push(p.to_path_buf());
        }
        return;
    }
    if md.is_dir() {
        let before = failed.len();
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            remove_tree(&e.path(), failed);
        }
        if std::fs::remove_dir(p).is_err() && failed.len() == before {
            failed.push(p.to_path_buf());
        }
    } else if std::fs::remove_file(p).is_err() {
        failed.push(p.to_path_buf());
    }
}

/// [`remove_tree`], tried again shortly for files a stopped job or player still holds.
fn remove_with_retry(p: &Path) -> Vec<PathBuf> {
    let mut failed = vec![];
    for attempt in 0..3 {
        failed.clear();
        remove_tree(p, &mut failed);
        if failed.is_empty() {
            break;
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    failed
}

/// Deletes what the plans list. A manga leaves Studio (its chapters are marked not downloaded
/// and not made) only once all its folders went; otherwise it stays so the delete can be
/// retried. Blocking.
pub(crate) fn execute(plans: Vec<MangaPlan>) -> Outcome {
    let mut out = Outcome::default();
    for p in plans {
        run::forget_brain(&p.id);
        let failed: Vec<PathBuf> = p.dirs.iter().flat_map(|d| remove_with_retry(&d.path)).collect();
        let left: u64 = p.dirs.iter().map(|d| paths::dir_size(&d.path)).sum();
        out.freed += p.bytes().saturating_sub(left);
        out.refused.extend(p.refused.iter().cloned());
        if !failed.is_empty() {
            out.failed.push((p.title.clone(), failed));
            continue;
        }
        for c in &p.chapters {
            let _ = mv_studio::voices::set_chapter_voice(c, None);
        }
        match data::forget_manga(&p.id) {
            Ok(_) => out.deleted.push(p.title.clone()),
            Err(e) => out.errors.push(format!("{}: {e:#}", p.title)),
        }
    }
    out
}

/// Waits (at most `max`) until no audio / video job or AI run works on `chapters` any more.
fn wait_idle(chapters: &BTreeSet<String>, max: Duration) {
    let start = Instant::now();
    while start.elapsed() < max {
        let jobs = service::snapshot().iter().any(|j| chapters.contains(&j.chapter_id) && !j.state.finished());
        let ai = run::snapshot().is_some_and(|r| r.running() && r.chapter_id.as_ref().is_some_and(|c| chapters.contains(c)));
        if !jobs && !ai {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------- words

/// "• A\n• B\nand 3 more".
pub(crate) fn title_list(titles: &[String], max: usize) -> String {
    let mut lines: Vec<String> = titles.iter().take(max).map(|t| format!("• {t}")).collect();
    if titles.len() > max {
        lines.push(format!("and {} more", titles.len() - max));
    }
    lines.join("\n")
}

/// The confirmation's title and body.
pub(crate) fn confirm_text(titles: &[String], bytes: u64) -> (String, String) {
    let title = match titles {
        [one] => format!("Delete \u{201c}{one}\u{201d} from Studio?"),
        _ => format!("Delete {} manga from Studio?", titles.len()),
    };
    let these = if titles.len() == 1 { "this manga" } else { "these manga" };
    let mut body = format!(
        "This permanently deletes everything for {these} on this PC: downloaded chapters and pages, narration audio, chapter videos, full recaps, captions, music and Studio project files. It can't be undone."
    );
    if titles.len() > 1 {
        body.push_str("\n\n");
        body.push_str(&title_list(titles, LISTED));
    }
    if bytes > 0 {
        body.push_str(&format!("\n\nFrees {}. The manga stays in your Library, so you can download it again.", fmt_bytes(bytes)));
    } else {
        body.push_str("\n\nThe manga stays in your Library, so you can download it again.");
    }
    (title, body)
}

/// "Deleted 2 manga and 3.4 GB of data".
pub(crate) fn done_text(n: usize, bytes: u64) -> String {
    if bytes > 0 {
        format!("Deleted {n} manga and {} of data", fmt_bytes(bytes))
    } else {
        format!("Deleted {n} manga")
    }
}

/// The "Select all" box over the manga shown.
pub(crate) fn all_state(shown: &[String], picked: &BTreeSet<String>) -> CheckState {
    match shown.iter().filter(|id| picked.contains(*id)).count() {
        0 => CheckState::Unchecked,
        n if n == shown.len() => CheckState::Checked,
        _ => CheckState::Mixed,
    }
}

/// "2 selected" / "Pick the manga to delete".
pub(crate) fn selected_text(n: usize) -> String {
    if n == 0 {
        "Pick the manga to delete".into()
    } else {
        format!("{n} selected")
    }
}

/// The toast after a delete.
fn outcome_toast(o: &Outcome, cx: &mut App) {
    let n = o.deleted.len();
    if o.failed.is_empty() && o.errors.is_empty() {
        let mut msg = done_text(n, o.freed);
        if !o.refused.is_empty() {
            msg.push_str(&format!(". Kept {} folder{} outside MangaVox's folders.", o.refused.len(), crate::page::plural(o.refused.len())));
        }
        toast::show(cx, Toast::success(msg).icon(glyphs::DELETE));
        return;
    }
    let mut msg = if n > 0 { format!("{}. ", done_text(n, o.freed)) } else { String::new() };
    if !o.failed.is_empty() {
        let titles: Vec<&str> = o.failed.iter().map(|(t, _)| t.as_str()).collect();
        let files: Vec<&PathBuf> = o.failed.iter().flat_map(|(_, f)| f.iter()).collect();
        let names: Vec<String> = files.iter().take(3).map(|f| f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.display().to_string())).collect();
        msg.push_str(&format!(
            "Could not delete everything for {}: {} file{} in use ({}{}). Close any player using them and delete again.",
            titles.join(", "),
            files.len(),
            crate::page::plural(files.len()),
            names.join(", "),
            if files.len() > 3 { ", …" } else { "" }
        ));
    }
    if !o.errors.is_empty() {
        msg.push_str(&format!(" {}", o.errors.join("; ")));
    }
    toast::show(cx, Toast::warning(msg));
}

// ---------------------------------------------------------------- the page

impl StudioPage {
    /// Why the manga `ids` cannot be deleted right now (work that cannot be stopped from here).
    fn delete_blocker(&self, ids: &[String]) -> Option<&'static str> {
        let Load::Ready(groups) = &self.library else { return Some("Studio is still loading") };
        let chapters: BTreeSet<&str> = groups.iter().filter(|g| ids.contains(&g.id)).flat_map(|g| g.chapters.iter().map(|c| c.id.as_str())).collect();
        if self.music.busy.is_some() && self.music.composing.iter().any(|c| chapters.contains(c.as_str())) {
            return Some("Wait for the score to finish");
        }
        if self.asking.as_ref().is_some_and(|(c, _)| chapters.contains(c.as_str())) {
            return Some("Wait for the AI to finish rewriting the page");
        }
        None
    }

    /// Asks before deleting the manga `ids` from Studio; measures what goes first (off the UI
    /// thread) so the dialog can say how much it frees.
    pub(crate) fn confirm_delete_manga(&mut self, ids: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(groups) = &self.library else { return };
        let targets: Vec<(String, String)> = groups.iter().filter(|g| ids.contains(&g.id)).map(|g| (g.id.clone(), g.title.clone())).collect();
        if targets.is_empty() || !self.deleting.is_empty() {
            return;
        }
        if let Some(why) = self.delete_blocker(&ids) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Ok(plans) = mv_ui::bg::blocking(move || plan(&targets)).await else { return };
            let _ = this.update_in(cx, |_, window, cx| {
                let titles: Vec<String> = plans.iter().map(|p| p.title.clone()).collect();
                let (title, body) = confirm_text(&titles, plans.iter().map(MangaPlan::bytes).sum());
                let weak = this.clone();
                confirm_dialog(title, body, "Delete", true)
                    .icon(glyphs::DELETE)
                    .on_confirm(move |_, cx| {
                        let plans = plans.clone();
                        let _ = weak.update(cx, |this, cx| this.delete_manga(plans, cx));
                    })
                    .open(window, cx);
            });
        })
        .detach();
    }

    /// Stops every piece of work on the manga, then deletes their files off the UI thread.
    fn delete_manga(&mut self, plans: Vec<MangaPlan>, cx: &mut Context<Self>) {
        let ids: Vec<String> = plans.iter().map(|p| p.id.clone()).collect();
        // Something may have started on them while the dialog was open.
        if let Some(why) = self.delete_blocker(&ids) {
            toast::show(cx, Toast::warning(why));
            return;
        }
        let chapters: BTreeSet<String> = plans.iter().flat_map(|p| p.chapters.iter().cloned()).collect();
        // Generate's items (waiting, running, done or failed), hand-started jobs and AI runs.
        for it in self.queue.items.iter().filter(|i| ids.contains(&i.chapter.manga_id) || chapters.contains(&i.chapter.id)) {
            pipeline::cancel(it.id);
        }
        for j in self.jobs.iter().filter(|j| chapters.contains(&j.chapter_id) && !j.state.finished()) {
            service::cancel(j.id);
        }
        if self.run.as_ref().is_some_and(|r| r.running() && r.chapter_id.as_ref().is_some_and(|c| chapters.contains(c))) {
            run::cancel();
        }
        if let Some(j) = self.export_job.as_ref().filter(|j| ids.contains(&j.manga_id)) {
            j.cancel.store(true, Ordering::Relaxed);
        }
        // Open editors and the player let go of the files.
        let mut stopped = false;
        for p in &plans {
            for c in &p.chapters {
                stopped |= self.forget_open_work(c, &paths::studio_chapter(&p.id, c));
            }
        }
        if crate::audio::now_playing().is_some_and(|n| plans.iter().flat_map(|p| p.dirs.iter()).any(|d| n.starts_with(&d.path))) {
            crate::audio::stop();
            stopped = true;
        }
        if self.focus.as_ref().is_some_and(|f| ids.contains(&f.item.manga_id)) || self.focus_id.as_ref().is_some_and(|c| chapters.contains(c)) {
            self.focus = None;
            self.focus_id = None;
            self.view = View::Dashboard;
            self.update_assistant_context();
        }
        self.dash_pick.retain(|c| !chapters.contains(c));
        self.selected.retain(|c| !chapters.contains(c));
        self.fixing.retain(|c| !chapters.contains(c));
        for id in &ids {
            self.open_groups.remove(id);
        }
        if self.dash_manga.as_ref().is_some_and(|m| ids.contains(m)) {
            self.dash_manga = None;
            self.dash_anchor = None;
        }
        self.dash_select = None;
        self.deleting = ids;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || {
                if stopped {
                    crate::audio::wait_stopped(Duration::from_millis(600));
                }
                wait_idle(&chapters, Duration::from_secs(10));
                execute(plans)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.deleting.clear();
                match res {
                    Ok(o) => outcome_toast(&o, cx),
                    Err(e) => this.fail(format!("Could not delete: {e:#}")),
                }
                this.reload_library(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{all_state, checked, confirm_text, done_text, plan_in, remove_with_retry, selected_text, title_list, Roots};
    use mv_ui::CheckState;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn roots(base: &Path) -> Roots {
        let (projects, studio, dl) = (base.join("studio/projects"), base.join("studio"), base.join("library/manga"));
        for d in [&projects, &dl] {
            std::fs::create_dir_all(d).unwrap();
        }
        Roots { projects: projects.clone(), studio: studio.clone(), downloads: vec![dl.clone()], protected: vec![base.to_path_buf(), studio, projects, base.join("library"), dl] }
    }

    fn put(p: &Path, bytes: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![1u8; bytes]).unwrap();
    }

    /// A directory link: a symlink where allowed, else a junction (Windows without Developer Mode).
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().map(|o| o.status.success()).unwrap_or(false)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn checked_keeps_to_the_root() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_path_buf();
        let root = base.join("root");
        let inside = root.join("m/c");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        assert_eq!(checked(&inside, &root, 1, &[]).unwrap(), Some(inside.canonicalize().unwrap()));
        assert_eq!(checked(&root.join("gone"), &root, 1, &[]).unwrap(), None, "missing folders are skipped");
        assert!(checked(&base.join("outside"), &root, 1, &[]).is_err(), "outside the root");
        assert!(checked(&root, &root, 1, &[]).is_err(), "never the root itself");
        assert!(checked(&root.join("m"), &root, 2, &[]).is_err(), "not deep enough");
        assert!(checked(&root.join("m").join("..").join("..").join("outside"), &root, 1, &[]).is_err(), "no ..");
        assert!(checked(&root.join("m").join("..").join("m"), &root, 1, &[]).is_err(), "no .. even when it lands inside");
        assert!(checked(Path::new("m/c"), &root, 1, &[]).is_err(), "relative paths are refused");
        assert!(checked(&root.join("m"), &root, 1, std::slice::from_ref(&inside)).is_err(), "never a folder holding a protected one");
    }

    #[test]
    fn checked_refuses_links_out_of_the_root() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_path_buf();
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("m")).unwrap();
        put(&outside.join("precious.txt"), 3);
        let link = root.join("m").join("escape");
        if !link_dir(&outside, &link) {
            eprintln!("could not make a link here; skipped");
            return;
        }
        assert!(checked(&link, &root, 1, &[]).is_err(), "the link itself");
        assert!(checked(&link.join("precious.txt"), &root, 1, &[]).is_err(), "a path through the link");
        // Deleting the folder holding the link removes the link, never what it points at.
        let p = checked(&root.join("m"), &root, 1, &[]).unwrap().unwrap();
        assert!(remove_with_retry(&p).is_empty());
        assert!(!root.join("m").exists());
        assert!(outside.join("precious.txt").is_file());
    }

    #[test]
    fn plan_and_execute_remove_only_the_manga_folders() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_path_buf();
        let r = roots(&base);
        put(&r.projects.join("m1/c1/video_final.mp4"), 1000);
        put(&r.projects.join("m1/full_recap.mp4"), 500);
        put(&r.studio.join(mv_recap::safe_id("m1")).join("brain.json"), 10);
        put(&r.downloads[0].join("m1/c1/0001.jpg"), 200);
        // Another manga and Studio's own files stay.
        put(&r.projects.join("m2/c9/video_final.mp4"), 7);
        put(&r.studio.join("prefs.json"), 7);
        // A stored pages folder outside MangaVox's folders is kept and reported.
        let stray = base.join("elsewhere/m1/c2");
        put(&stray.join("0001.jpg"), 5);
        let chapters = vec![("c1".to_string(), Some(r.downloads[0].join("m1/c1"))), ("c2".to_string(), Some(stray.clone())), ("c3".to_string(), None)];
        let plan = plan_in("m1", "One", &chapters, &r);
        assert_eq!(plan.dirs.len(), 3, "projects, brain and downloads; the chapter folder is inside the manga's");
        assert_eq!(plan.bytes(), 1710);
        assert_eq!(plan.refused.len(), 1);
        assert_eq!(plan.chapters, vec!["c1", "c2", "c3"]);
        for dir in &plan.dirs {
            remove_with_retry(&dir.path);
        }
        assert!(!r.projects.join("m1").exists() && !r.downloads[0].join("m1").exists());
        assert!(r.projects.join("m2/c9/video_final.mp4").is_file());
        assert!(r.studio.join("prefs.json").is_file() && r.projects.is_dir() && r.downloads[0].is_dir());
        assert!(stray.join("0001.jpg").is_file());
    }

    #[test]
    fn an_id_naming_a_studio_folder_never_removes_it() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_path_buf();
        let r = roots(&base);
        put(&r.projects.join("x/c/video_final.mp4"), 3);
        // `studio/<id>` would be `studio/projects`, every manga's work.
        let plan = plan_in("projects", "Odd", &[], &r);
        assert!(plan.dirs.iter().all(|d| d.path != r.projects.canonicalize().unwrap()));
        assert!(!plan.refused.is_empty());
        // A stored pages folder that is a whole manga folder (or the root) is refused too.
        put(&r.downloads[0].join("x/c/0001.jpg"), 3);
        let plan = plan_in("m", "M", &[("c".into(), Some(r.downloads[0].join("x")))], &r);
        assert!(plan.dirs.is_empty() && plan.refused.len() == 1);
    }

    #[test]
    fn confirm_and_selection_words() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (title, body) = confirm_text(&t(&["A", "B"]), 3 * 1024 * 1024 * 1024 + 400 * 1024 * 1024);
        assert_eq!(title, "Delete 2 manga from Studio?");
        assert!(body.starts_with("This permanently deletes everything for these manga on this PC: downloaded chapters and pages, narration audio, chapter videos, full recaps, captions, music and Studio project files. It can't be undone."));
        assert!(body.contains("• A\n• B") && body.contains("Frees 3.4 GB"));
        let (title, body) = confirm_text(&t(&["Solo"]), 0);
        assert_eq!(title, "Delete \u{201c}Solo\u{201d} from Studio?");
        assert!(body.contains("this manga") && !body.contains("• Solo") && !body.contains("Frees"));
        let many = t(&["1", "2", "3", "4", "5", "6", "7"]);
        assert_eq!(title_list(&many, 5), "• 1\n• 2\n• 3\n• 4\n• 5\nand 2 more");
        assert_eq!(done_text(2, 3 * 1024 * 1024 * 1024 + 400 * 1024 * 1024), "Deleted 2 manga and 3.4 GB of data");
        assert_eq!(done_text(1, 0), "Deleted 1 manga");
        assert_eq!(selected_text(2), "2 selected");
        assert_eq!(selected_text(0), "Pick the manga to delete");
        let shown = t(&["a", "b"]);
        let mut picked = BTreeSet::new();
        assert_eq!(all_state(&shown, &picked), CheckState::Unchecked);
        picked.insert("a".to_string());
        picked.insert("zz".to_string());
        assert_eq!(all_state(&shown, &picked), CheckState::Mixed);
        picked.insert("b".to_string());
        assert_eq!(all_state(&shown, &picked), CheckState::Checked);
    }
}
