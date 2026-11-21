//! The 8-step bar of an open project (design 9.2): a 48 px bar with a mark per step (done,
//! running, waiting, paused, failed), icons only below `style::NARROW`, Animate shown as "coming
//! next" and never done. Only a step whose job really runs shows a spinner; a waiting or paused
//! step is still, so a held queue costs no redraws. Also the completion rule of each step, read
//! from the project's files.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{JobKind, JobState, Queue};
use mv_anime::{Episode, LineKind, Series, Severity, Status, Store};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{interactive, ph, ph_icon, size, spinner, tip, Ph, Tokens, Weight};

use crate::steps::Step;
use crate::style::STEPBAR_H;

/// The step a queue job belongs to.
pub fn step_of(kind: &JobKind) -> Option<Step> {
    match kind {
        JobKind::Story { .. } | JobKind::AdaptScene { .. } => Some(Step::Story),
        JobKind::Bible { .. } | JobKind::Crops { .. } | JobKind::Sheet { .. } | JobKind::Audition { .. } => Some(Step::Bible),
        JobKind::PlanScene { .. } => Some(Step::Shots),
        JobKind::Frame { .. } => Some(Step::Frames),
        JobKind::Voice { .. } => Some(Step::Sound),
        JobKind::Animatic { .. } => Some(Step::Cut),
        JobKind::Unknown => None,
    }
}

/// The job belongs to `episode` (or to the whole project: the bible, its pictures and voices).
pub fn for_episode(kind: &JobKind, episode: &str) -> bool {
    match kind {
        JobKind::Story { episode: e } | JobKind::AdaptScene { episode: e, .. } | JobKind::PlanScene { episode: e, .. } | JobKind::Frame { episode: e, .. } | JobKind::Voice { episode: e, .. } | JobKind::Animatic { episode: e } => e == episode,
        JobKind::Bible { .. } | JobKind::Crops { .. } | JobKind::Sheet { .. } | JobKind::Audition { .. } | JobKind::Unknown => true,
    }
}

/// What one step's mark shows. Only [`Mark::Running`] moves (a spinner); every other mark is
/// still, so a paused or waiting queue costs no redraws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Upcoming,
    Done,
    /// One of its jobs is running right now.
    Running,
    /// A job waits its turn in a queue that runs.
    Waiting,
    /// A job is paused, or waits in a queue that is paused (all paused, stopped by a restart,
    /// or its lane held).
    Paused,
    /// A job failed and its failure is still shown.
    Failed,
}

/// A job's state as the bar reads it: a job waiting while its queue is held is paused, not
/// waiting (nothing will start it until the user resumes).
pub fn effective(state: JobState, held: bool) -> JobState {
    if state == JobState::Queued && held {
        JobState::Paused
    } else {
        state
    }
}

/// Each step's mark, from its jobs' (effective) states and the completion rules. A failure
/// still shown wins, then running, waiting, paused; a step with work left is never "done".
pub fn marks(states: &[(Step, JobState, bool)], done: &[bool]) -> Vec<Mark> {
    Step::ALL
        .into_iter()
        .map(|s| {
            let has = |want: JobState| states.iter().any(|(st, state, hidden)| *st == s && *state == want && !(want == JobState::Failed && *hidden));
            if has(JobState::Failed) {
                Mark::Failed
            } else if has(JobState::Running) {
                Mark::Running
            } else if has(JobState::Queued) {
                Mark::Waiting
            } else if has(JobState::Paused) {
                Mark::Paused
            } else if s != Step::Animate && done.get(s.index()).copied().unwrap_or(false) {
                Mark::Done
            } else {
                Mark::Upcoming
            }
        })
        .collect()
}

/// The steps' jobs in `q` for one project's episode, plus Studio's Reads of the episode's
/// chapters (they count as the Manga step), each with its effective state ([`effective`]) and
/// whether a failure is hidden.
pub fn job_states(q: &Queue, project: &str, episode: &str, studio: &ui_studio::pipeline::Queue, chapters: &[String]) -> Vec<(Step, JobState, bool)> {
    let all_held = q.paused || q.interrupted;
    let mut v: Vec<(Step, JobState, bool)> = q
        .jobs
        .iter()
        .filter(|j| j.project == project && for_episode(&j.kind, episode))
        .filter_map(|j| step_of(&j.kind).map(|s| (s, effective(j.state, all_held || q.paused_lanes.contains(&j.lane)), j.hidden)))
        .collect();
    let studio_held = studio.paused || studio.interrupted;
    for it in studio.items.iter().filter(|it| chapters.contains(&it.chapter.id)) {
        use ui_studio::pipeline::ItemState as I;
        let state = match it.state {
            I::Running => JobState::Running,
            I::Queued => JobState::Queued,
            I::Paused => JobState::Paused,
            I::Done => JobState::Done,
            I::Failed => JobState::Failed,
        };
        v.push((Step::Manga, effective(state, studio_held), false));
    }
    v
}

/// What makes a step done, in words (the tooltips).
pub fn done_rule(step: Step) -> &'static str {
    match step {
        Step::Manga => "every chapter of the episode is read",
        Step::Story => "the chapters are adapted into scenes",
        Step::Bible => "you approve the bible",
        Step::Shots => "every scene is planned with no errors",
        Step::Frames => "every shot has an approved frame",
        Step::Animate => "",
        Step::Sound => "every line has a voice",
        Step::Cut => "the animatic is exported from the current cut",
    }
}

/// One tooltip per step, from its mark.
pub fn tooltips(marks: &[Mark]) -> Vec<String> {
    Step::ALL
        .into_iter()
        .map(|s| {
            let label = s.label();
            if s == Step::Animate {
                return format!("{label}: coming in the next update");
            }
            if !s.built() {
                return format!("{label}: not part of this build");
            }
            match marks.get(s.index()).copied().unwrap_or(Mark::Upcoming) {
                Mark::Failed => format!("{label}: something failed; the queue below says why"),
                Mark::Running => format!("{label}: working on it"),
                Mark::Waiting => format!("{label}: waiting its turn"),
                Mark::Paused => format!("{label}: paused"),
                Mark::Done => format!("{label}: done"),
                Mark::Upcoming => format!("{label}: done when {}", done_rule(s)),
            }
        })
        .collect()
}

/// Each step's completion rule (design 9.2), from the project's files. Blocking: run it off the
/// UI thread.
pub fn completion(store: &Store, series: &Series, episode: &Episode) -> [bool; 8] {
    let ep = episode.id.as_str();
    let mut done = [false; 8];
    done[Step::Manga.index()] = !episode.chapters.is_empty()
        && episode.chapters.iter().all(|c| {
            let (read, pages) = mv_anime::source::read_progress(c, &series.manga_title);
            pages > 0 && read >= pages
        });
    done[Step::Story.index()] = store.path(&format!("episodes/{ep}/story.json")).is_file();
    done[Step::Bible.index()] = series.bible_approved.is_some();
    let story = if done[Step::Story.index()] { store.story(ep).ok() } else { None };
    if let Some(story) = &story {
        let planned = !story.scenes.is_empty() && story.scenes.iter().all(|s| store.scene_shots(ep, &s.id).map(|ss| !ss.shots.is_empty()).unwrap_or(false));
        let clean = store.checks(ep).map(|issues| !issues.iter().any(|i| i.severity == Severity::Error)).unwrap_or(false);
        done[Step::Shots.index()] = planned && clean;
        let spoken: Vec<_> = story.lines.iter().filter(|l| l.kind != LineKind::Sfx && !l.spoken.trim().is_empty()).collect();
        done[Step::Sound.index()] = !spoken.is_empty() && spoken.iter().all(|l| l.voice.selected.is_some());
    }
    if done[Step::Shots.index()] {
        done[Step::Frames.index()] = store.all_shots(ep).map(|all| {
            let shots: Vec<_> = all.values().flatten().collect();
            !shots.is_empty() && shots.iter().all(|s| s.frame.status == Status::Approved && s.frame.selected.is_some())
        })
        .unwrap_or(false);
    }
    done[Step::Cut.index()] = cut_done(store, episode);
    done
}

/// An animatic was exported from the cut as it is now (its timing hash).
#[cfg(feature = "cut")]
fn cut_done(store: &Store, episode: &Episode) -> bool {
    let ep = episode.id.as_str();
    let exported = std::fs::read_dir(store.path(&format!("episodes/{ep}/exports"))).map(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4")))).unwrap_or(false);
    if !exported {
        return false;
    }
    let Ok(now) = mv_anime::animatic::plan_cut(store, ep) else { return false };
    if now.timing_hash.is_empty() {
        return false;
    }
    let saved: mv_anime::animatic::CutPlan = mv_anime::store::read_json(&store.path(&format!("episodes/{ep}/cut.json"))).unwrap_or_default();
    saved.timing_hash == now.timing_hash || episode.animatic_approved.as_ref().is_some_and(|a| a.hash == now.timing_hash)
}

#[cfg(not(feature = "cut"))]
fn cut_done(_store: &Store, _episode: &Episode) -> bool {
    false
}

/// A step's round mark: a check when done, a spinner only while a job really runs, a still
/// hourglass while one waits its turn, a still pause sign while paused, a cross when failed,
/// else the step's own icon.
fn disc(i: usize, mark: Mark, icon: Ph, current: bool) -> Div {
    let d = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(icon_px::XL))
        .rounded_full()
        .when(current, |d| d.shadow(vec![BoxShadow { color: Tokens::accent_a(0.35).into(), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(3.), inset: false }]));
    match mark {
        Mark::Done => d.bg(Tokens::accent()).child(ph_icon(ph::CHECK, Weight::Bold, Tokens::on_accent(), icon_px::XS)),
        Mark::Running => d.bg(Tokens::accent_soft()).child(spinner(("ae-step-spin", i), Tokens::accent(), icon_px::SM)),
        Mark::Waiting => d.bg(Tokens::accent_soft()).border_1().border_color(Tokens::accent_border()).child(ph_icon(ph::HOURGLASS, Weight::Bold, Tokens::accent(), icon_px::XS)),
        Mark::Paused => d.bg(Tokens::surface_2()).border_1().border_color(Tokens::border_strong()).child(ph_icon(ph::PAUSE, Weight::Fill, Tokens::text_2(), icon_px::XS)),
        Mark::Failed => d.bg(Tokens::danger_soft()).border_1().border_color(Tokens::danger_border()).child(ph_icon(ph::X, Weight::Bold, Tokens::danger(), icon_px::XS)),
        Mark::Upcoming => d.border_1().border_color(if current { Tokens::accent() } else { Tokens::track() }).child(ph_icon(icon, Weight::Bold, if current { Tokens::text_1() } else { Tokens::text_3() }, icon_px::XS)),
    }
}

/// The bar: the eight steps in a 48 px row under the header, with their marks
/// ([`marks`]) and tooltips. `compact` shows icons only (the labels move into the tooltips).
pub fn bar(current: Step, marks: &[Mark], compact: bool, on_select: impl Fn(Step, &mut Window, &mut App) + 'static) -> Div {
    let tips = tooltips(marks);
    let on_select = std::rc::Rc::new(on_select);
    let mark_of = |i: usize| marks.get(i).copied().unwrap_or(Mark::Upcoming);
    let mut row = div().id("anime-steps").flex().items_center().w_full().min_w_0().role(Role::List);
    let n = Step::ALL.len();
    for (i, s) in Step::ALL.into_iter().enumerate() {
        let mark = mark_of(i);
        let is_current = s == current;
        let tip_text = tips.get(i).cloned().unwrap_or_default();
        let label_color = match (mark, is_current) {
            (Mark::Failed, _) => Tokens::danger(),
            (_, true) => Tokens::text_1(),
            (Mark::Upcoming, false) => Tokens::text_3(),
            (_, false) => Tokens::text_2(),
        };
        let select = on_select.clone();
        let step = interactive(("ae-step", i))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::SM))
            .py(px(space::XS))
            .rounded(px(radius::MD))
            .role(Role::ListItem)
            .aria_label(SharedString::from(tip_text.clone()))
            .aria_selected(is_current)
            .tooltip(tip(tip_text))
            .on_click(move |_, w, cx| select(s, w, cx))
            .child(disc(i, mark, s.icon(), is_current))
            .when(!compact, |d| d.child(div().text_size(px(size::SM)).font_weight(if is_current { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).text_color(label_color).whitespace_nowrap().child(s.label())));
        row = row.child(step);
        if i + 1 < n {
            let next = Step::from_index(i + 1);
            let color = if mark == Mark::Done {
                Tokens::accent()
            } else if next == Some(current) || mark_of(i + 1) == Mark::Running {
                Tokens::accent_a(0.45)
            } else {
                Tokens::border_strong()
            };
            row = row.child(div().flex_1().min_w(px(space::MD)).h(px(space::XXS)).mx(px(space::XS)).rounded_full().bg(color));
        }
    }
    div().flex_none().h(px(STEPBAR_H)).flex().items_center().px(px(space::XL)).border_b_1().border_color(Tokens::border()).child(div().flex_1().min_w_0().child(row))
}

#[cfg(test)]
mod tests {
    use super::{done_rule, effective, for_episode, job_states, marks, step_of, tooltips, Mark};
    use crate::steps::Step;
    use mv_anime::queue::{Job, JobKind, JobState, Lane, Queue};
    use mv_anime::Target;

    #[test]
    fn jobs_land_on_their_step() {
        assert_eq!(step_of(&JobKind::Frame { episode: "e".into(), shot: "s".into(), variants: 1 }), Some(Step::Frames));
        assert_eq!(step_of(&JobKind::Sheet { target: Target::StyleFrame, variants: 1 }), Some(Step::Bible));
        assert_eq!(step_of(&JobKind::Animatic { episode: "e".into() }), Some(Step::Cut));
        assert_eq!(step_of(&JobKind::Unknown), None);
    }

    #[test]
    fn another_episode_s_jobs_do_not_show() {
        assert!(for_episode(&JobKind::Frame { episode: "ep01".into(), shot: "s".into(), variants: 1 }, "ep01"));
        assert!(!for_episode(&JobKind::Voice { episode: "ep02".into(), line: "l".into() }, "ep01"));
        assert!(!for_episode(&JobKind::AdaptScene { episode: "ep02".into(), scene: "s".into() }, "ep01"));
        // The bible and its pictures belong to every episode.
        assert!(for_episode(&JobKind::Bible { episode: "ep02".into() }, "ep01"));
        assert!(for_episode(&JobKind::Sheet { target: Target::StyleFrame, variants: 1 }, "ep01"));
    }

    /// The mark and tooltip of one step with these jobs (and whether it is done by its rule).
    fn one(step: Step, jobs: &[(JobState, bool)], done: bool) -> (Mark, String) {
        let states: Vec<(Step, JobState, bool)> = jobs.iter().map(|(s, h)| (step, *s, *h)).collect();
        let mut d = [false; 8];
        d[step.index()] = done;
        let m = marks(&states, &d);
        let t = tooltips(&m);
        (m[step.index()], t[step.index()].clone())
    }

    #[test]
    fn each_step_state_has_its_own_still_or_moving_mark() {
        if !Step::Shots.built() {
            return;
        }
        // Running: the only mark that moves.
        assert_eq!(one(Step::Shots, &[(JobState::Running, false)], false), (Mark::Running, "Shots: working on it".into()));
        // Waiting its turn in a queue that runs.
        assert_eq!(one(Step::Shots, &[(JobState::Queued, false)], false), (Mark::Waiting, "Shots: waiting its turn".into()));
        // Paused (or waiting in a paused queue, see below): still, and says so.
        assert_eq!(one(Step::Shots, &[(JobState::Paused, false)], true), (Mark::Paused, "Shots: paused".into()));
        // Failed, unless the failure is hidden.
        assert_eq!(one(Step::Shots, &[(JobState::Failed, false), (JobState::Running, false)], false).0, Mark::Failed);
        assert!(one(Step::Shots, &[(JobState::Failed, false)], false).1.contains("something failed"));
        assert_eq!(one(Step::Shots, &[(JobState::Failed, true)], false).0, Mark::Upcoming);
        // Done: by its rule, with nothing left to do.
        assert_eq!(one(Step::Shots, &[(JobState::Done, false)], true), (Mark::Done, "Shots: done".into()));
        assert_eq!(one(Step::Shots, &[], false), (Mark::Upcoming, format!("Shots: done when {}", done_rule(Step::Shots))));
        // A done step that works again (Ch. 2 still adapting) is not "done".
        assert_eq!(one(Step::Shots, &[(JobState::Queued, false)], true).0, Mark::Waiting);
        // Waiting beats paused when both are there; running beats both.
        assert_eq!(one(Step::Shots, &[(JobState::Paused, false), (JobState::Queued, false)], false).0, Mark::Waiting);
        assert_eq!(one(Step::Shots, &[(JobState::Paused, false), (JobState::Running, false)], false).0, Mark::Running);
    }

    #[test]
    fn a_job_waiting_in_a_held_queue_is_paused_not_running() {
        assert_eq!(effective(JobState::Queued, true), JobState::Paused);
        assert_eq!(effective(JobState::Queued, false), JobState::Queued);
        // A job already running keeps running until it ends.
        assert_eq!(effective(JobState::Running, true), JobState::Running);
        let job = |id: u64, kind: JobKind, state: JobState| Job { id, project: "p".into(), lane: kind.lane(), kind, state, ..Default::default() };
        let plan = || JobKind::PlanScene { episode: "ep01".into(), scene: "sc1".into() };
        let frame = || JobKind::Frame { episode: "ep01".into(), shot: "s".into(), variants: 1 };
        let studio = ui_studio::pipeline::Queue::default();
        // "Pause all", then a plan queued: the Shots step is paused (N1), nothing spins.
        let mut q = Queue { paused: true, jobs: vec![job(1, plan(), JobState::Queued)], ..Default::default() };
        let m = marks(&job_states(&q, "p", "ep01", &studio, &[]), &[false; 8]);
        assert_eq!(m[Step::Shots.index()], Mark::Paused);
        assert!(!m.contains(&Mark::Running));
        // Stopped by a restart: the same.
        q.paused = false;
        q.interrupted = true;
        assert_eq!(marks(&job_states(&q, "p", "ep01", &studio, &[]), &[false; 8])[Step::Shots.index()], Mark::Paused);
        // A running queue: waiting its turn.
        q.interrupted = false;
        assert_eq!(marks(&job_states(&q, "p", "ep01", &studio, &[]), &[false; 8])[Step::Shots.index()], Mark::Waiting);
        // Only the held lane's jobs are paused (pictures stopped for a sign-in).
        q.jobs.push(job(2, frame(), JobState::Queued));
        q.paused_lanes = vec![Lane::Web];
        let m = marks(&job_states(&q, "p", "ep01", &studio, &[]), &[false; 8]);
        assert_eq!((m[Step::Shots.index()], m[Step::Frames.index()]), (Mark::Waiting, Mark::Paused));
        // Another project's jobs never show.
        assert!(job_states(&q, "other", "ep01", &studio, &[]).is_empty());
    }

    #[test]
    fn tooltips_say_what_done_means() {
        let t = tooltips(&[Mark::Upcoming; 8]);
        assert_eq!(t.len(), 8);
        // A build without a step's feature says so instead.
        assert!(t[Step::Animate.index()].contains("next update"));
        assert_eq!(t[0], if Step::Manga.built() { format!("Manga: done when {}", done_rule(Step::Manga)) } else { "Manga: not part of this build".to_string() });
        for s in Step::ALL {
            assert!(s == Step::Animate || !done_rule(s).is_empty());
        }
        // Animate is never done, whatever the files say.
        assert_eq!(marks(&[], &[true; 8])[Step::Animate.index()], Mark::Upcoming);
    }
}
