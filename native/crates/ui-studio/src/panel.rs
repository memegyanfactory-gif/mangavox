//! Studio's sections of the app-wide side panel (`mv_ui::panel`): the Activity and Issues content
//! the shell draws next to every page (the reader and the player included), so what Studio is
//! doing, and what failed, can be seen from anywhere. The content is the same one Studio's own
//! side panel shows, minus its scroll container and padding (the shell's panel scrolls and pads)
//! and minus its empty states (the shell draws one for all tabs).
use gpui_kit::*;
use mv_ui::theme::space;

use crate::page::StudioPage;
use crate::Kept;

/// One of Studio's sections of the app-wide panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The AI run, exports, audio and video jobs, the first issues and the Generate queue.
    Activity,
    /// Every failure as a sentence with its one-click fixes.
    Issues,
}

/// What the panel's toggles show about Studio.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// Something runs: the live dot.
    pub live: bool,
    /// Activity has something to show.
    pub any: bool,
    /// Issues waiting for a fix (those Generate is already fixing are progress, not counted).
    pub issues: usize,
}

/// The raw facts [`summary`] is made of, so the rule itself can be tested.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Facts {
    run_present: bool,
    run_running: bool,
    jobs: usize,
    jobs_unfinished: usize,
    queue_items: usize,
    queue_active: usize,
    exports: usize,
    issues_total: usize,
    issues_open: usize,
}

fn summarize(f: Facts) -> Summary {
    Summary {
        live: f.run_running || f.jobs_unfinished > 0 || f.queue_active > 0 || f.exports > 0,
        any: f.run_present || f.jobs > 0 || f.queue_items > 0 || f.exports > 0 || f.issues_total > 0,
        issues: f.issues_open,
    }
}

/// Studio's state for the panel's toggles and empty state. Cheap and safe every frame: it reads
/// the page's own copies, never `pipeline::snapshot()` (its lock is held while the queue is
/// written to disk, which would stall the UI thread).
pub fn summary(cx: &App) -> Summary {
    let exports = crate::pipeline::exports().len();
    let Some(Kept(page)) = cx.try_global::<Kept>() else {
        // No Studio page yet: only the exports (they run without one) can be news.
        return summarize(Facts { exports, ..Facts::default() });
    };
    let p = page.read(cx);
    summarize(Facts {
        run_present: p.run.is_some(),
        run_running: p.run.as_ref().is_some_and(|r| r.running()),
        jobs: p.jobs.len(),
        jobs_unfinished: p.jobs.iter().filter(|j| !j.state.finished()).count(),
        queue_items: p.queue.items.len(),
        queue_active: p.queue.active(),
        exports,
        issues_total: p.issues.len(),
        issues_open: p.issues.iter().filter(|i| i.fixing.is_none()).count(),
    })
}

/// Whether Studio's own side panel (on the Studio page) is showing.
pub fn side_panel_open(cx: &App) -> bool {
    match cx.try_global::<Kept>() {
        Some(Kept(page)) => page.read(cx).side_shown(),
        None => false,
    }
}

/// Shows or hides Studio's own side panel, like its title-bar button does.
pub fn toggle_side_panel(cx: &mut App) {
    let Some(page) = cx.try_global::<Kept>().map(|k| k.0.clone()) else { return };
    page.update(cx, |p, cx| {
        p.toggle_side();
        cx.notify();
    });
}

/// The session's Studio page, made when it does not exist yet and Studio mode is on. Made
/// without taking keyboard focus: the panel can be opened while a video plays or a chapter is
/// read, whose shortcuts must keep working (opening the Studio route focuses the page).
fn kept_page(window: &mut Window, cx: &mut App) -> Option<Entity<StudioPage>> {
    if let Some(Kept(page)) = cx.try_global::<Kept>() {
        return Some(page.clone());
    }
    if !mv_ui::mode::studio_enabled(cx) {
        return None;
    }
    let page = cx.new(|cx| StudioPage::new_unfocused(window, cx));
    cx.set_global(Kept(page.clone()));
    Some(page)
}

/// The view of one section, kept per section so repeated calls (every frame of the shell) share
/// one view and one subscription. `None` outside Studio mode.
pub fn section(section: Section, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let page = kept_page(window, cx)?;
    if let Some(views) = cx.try_global::<Views>() {
        if let Some((_, v)) = views.0.iter().find(|(s, v)| *s == section && v.read(cx).page == page) {
            return Some(v.clone().into());
        }
    }
    let view = cx.new(|cx| SectionView::new(section, page, cx));
    let mut views = cx.try_global::<Views>().map(|v| v.0.clone()).unwrap_or_default();
    views.retain(|(s, _)| *s != section);
    views.push((section, view.clone()));
    cx.set_global(Views(views));
    Some(view.into())
}

#[derive(Clone)]
struct Views(Vec<(Section, Entity<SectionView>)>);
impl Global for Views {}

struct SectionView {
    section: Section,
    page: Entity<StudioPage>,
    _observe: Subscription,
}

impl SectionView {
    fn new(section: Section, page: Entity<StudioPage>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&page, |_, _, cx| cx.notify());
        Self { section, page, _observe: observe }
    }
}

impl Render for SectionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.section;
        // Studio's own render recomputes the issues, but it does not run while the user is on
        // another page. This view renders when the page notifies (a poll that saw a change, a
        // click such as Dismiss), which is exactly when they can have changed, and not every
        // frame (collect_issues builds a name map of the whole library).
        let items = self.page.update(cx, |p, cx| {
            p.refresh_issues(cx);
            match section {
                Section::Activity => p.activity_items(true, cx),
                Section::Issues => p.issues_items(true, cx),
            }
        });
        let gap = match section {
            Section::Activity => space::MD,
            Section::Issues => space::SM,
        };
        div().flex().flex_col().gap(px(gap)).children(items)
    }
}

#[cfg(test)]
mod tests {
    use super::{summarize, Facts, Summary};

    #[test]
    fn nothing_going_on_is_quiet() {
        assert_eq!(summarize(Facts::default()), Summary::default());
    }

    #[test]
    fn any_run_job_queue_export_or_issue_gives_activity_something_to_show() {
        for f in [
            Facts { run_present: true, ..Facts::default() },
            Facts { jobs: 1, ..Facts::default() },
            Facts { queue_items: 1, ..Facts::default() },
            Facts { exports: 1, ..Facts::default() },
            Facts { issues_total: 1, ..Facts::default() },
        ] {
            assert!(summarize(f).any, "{f:?}");
        }
    }

    #[test]
    fn only_work_in_progress_is_live() {
        // A finished run, finished jobs and a done queue are history, not a live dot.
        let history = Facts { run_present: true, jobs: 2, queue_items: 3, issues_total: 1, issues_open: 1, ..Facts::default() };
        let s = summarize(history);
        assert!(!s.live);
        assert!(s.any);
        assert_eq!(s.issues, 1);
        for f in [
            Facts { run_present: true, run_running: true, ..Facts::default() },
            Facts { jobs: 1, jobs_unfinished: 1, ..Facts::default() },
            Facts { queue_items: 1, queue_active: 1, ..Facts::default() },
            Facts { exports: 1, ..Facts::default() },
        ] {
            assert!(summarize(f).live, "{f:?}");
        }
    }

    #[test]
    fn issues_being_fixed_are_not_counted() {
        let s = summarize(Facts { issues_total: 3, issues_open: 1, ..Facts::default() });
        assert_eq!(s.issues, 1);
        assert!(s.any);
    }
}
