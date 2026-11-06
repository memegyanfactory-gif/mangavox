//! Time left for Generate: how long each manga in the queue still takes, learned from how long
//! the steps really took on this PC.
//!
//! Two parts. [`History`] is a small file (`studio/step_times.json`) of measured seconds per page
//! for every step, per manga and for all manga together; it is written only when a step finishes.
//! [`estimate`] is pure: it plays the queue forward through the same three lanes the runner uses
//! (AI steps, voice and music, video render; see `pipeline`) and says when each manga's last chapter
//! is done. Until every step still to run has a measured rate it says "estimating" instead of a
//! guess.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Steps in the pipeline (Read, Transcript, Story, Audio, Music, Video).
pub const STEPS: usize = 6;
/// The lane each step runs in: AI, voice and music, video render.
const LANE_OF: [usize; STEPS] = [0, 0, 0, 1, 1, 2];
const LANES: usize = 3;
/// The transcript is only a fill-in that takes moments, so it needs no measured rate.
const TRANSCRIPT: usize = 1;
const TRANSCRIPT_SECS: f64 = 1.0;
/// A step that runs longer than expected is given at least this much more time (seconds)...
const OVERRUN_MIN: f64 = 30.0;
/// ...or this share of what was expected, when that is more.
const OVERRUN_SHARE: f64 = 0.25;

// ---------------------------------------------------------------- history

/// Samples kept per step for one manga, and for all manga.
const KEEP_MANGA: usize = 10;
const KEEP_GLOBAL: usize = 30;
/// Manga remembered (the least recently made are forgotten first).
const KEEP_MANGAS: usize = 40;
/// The median looks at this many of the newest samples.
const MEDIAN_OF: usize = 7;

/// One manga's measured seconds per page, per step (newest last).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MangaTimes {
    /// When it was last measured (unix seconds), for forgetting old manga.
    pub at: u64,
    pub steps: Vec<Vec<f64>>,
}

/// Measured seconds per page for every step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub global: Vec<Vec<f64>>,
    pub manga: BTreeMap<String, MangaTimes>,
}

fn push_sample(lists: &mut Vec<Vec<f64>>, step: usize, v: f64, keep: usize) {
    if lists.len() < STEPS {
        lists.resize(STEPS, Vec::new());
    }
    let l = &mut lists[step];
    l.push(v);
    if l.len() > keep {
        let extra = l.len() - keep;
        l.drain(..extra);
    }
}

/// The middle of the newest samples (robust against one slow or odd chapter).
fn median(samples: &[f64]) -> Option<f64> {
    let from = samples.len().saturating_sub(MEDIAN_OF);
    let mut v: Vec<f64> = samples[from..].iter().copied().filter(|x| x.is_finite() && *x > 0.).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2. })
}

impl History {
    /// A step of `manga_id` took `secs` for `units` pages, at unix time `now`.
    pub fn record(&mut self, manga_id: &str, step: usize, secs: f64, units: f64, now: u64) {
        if step >= STEPS || step == TRANSCRIPT || !secs.is_finite() || !units.is_finite() || secs <= 0. || units <= 0. {
            return;
        }
        let per_page = secs / units;
        push_sample(&mut self.global, step, per_page, KEEP_GLOBAL);
        let m = self.manga.entry(manga_id.to_string()).or_default();
        m.at = now;
        push_sample(&mut m.steps, step, per_page, KEEP_MANGA);
        while self.manga.len() > KEEP_MANGAS {
            let Some(oldest) = self.manga.iter().min_by_key(|(_, m)| m.at).map(|(k, _)| k.clone()) else { break };
            self.manga.remove(&oldest);
        }
    }

    /// Seconds per page for a step: the manga's own measurements first, else everyone's.
    pub fn rate(&self, manga_id: &str, step: usize) -> Option<f64> {
        let own = self.manga.get(manga_id).and_then(|m| m.steps.get(step)).and_then(|s| median(s));
        own.or_else(|| self.global.get(step).and_then(|s| median(s)))
    }
}

fn path() -> PathBuf {
    mv_recap::studio_root().join("step_times.json")
}

/// A missing or damaged file just means nothing was measured yet.
fn load() -> History {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(h: &History) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(h) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A step really ran for `secs` (waiting and pauses left out) over `units` pages: remember it.
pub fn record(manga_id: &str, step: usize, secs: f64, units: f64) {
    let mut g = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let h = g.get_or_insert_with(load);
    let before = h.clone();
    h.record(manga_id, step, secs, units, unix_now());
    if *h != before {
        save(h);
    }
}

/// A copy of what was measured, for [`estimate`] (reads the file the first time).
pub fn history() -> History {
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(load).clone()
}

// ---------------------------------------------------------------- estimate

/// The step running now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Running {
    pub step: usize,
    /// Seconds it has really worked (time waiting for an AI limit left out).
    pub elapsed: f64,
    /// Seconds it still has to wait for an AI usage limit.
    pub wait_left: f64,
}

/// One chapter of the queue, in queue order.
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    pub manga: String,
    /// Steps still to make (the running one included).
    pub todo: [bool; STEPS],
    /// Pages each step still has to go through.
    pub units: [f64; STEPS],
    /// Already handed on: it is in a lane (running, or waiting for it), not waiting to be read.
    pub started: bool,
    pub running: Option<Running>,
    /// Only the video was asked for again: it goes straight to the render lane.
    pub render_only: bool,
    /// Paused on its own.
    pub held: bool,
}

/// Time left for one manga (or for everything).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Line {
    /// Seconds left; `None` while there is not enough measured to say.
    pub secs: Option<u64>,
    /// On hold: the time is what it takes once resumed.
    pub paused: bool,
}

impl Line {
    /// "About 3 h 20 min left", "Estimating time left…", "About 2 h left once resumed".
    pub fn text(&self) -> String {
        match self.secs {
            None => "Estimating time left…".into(),
            Some(s) if self.paused => format!("{} left once resumed", phrase(s)),
            Some(s) => format!("{} left", phrase(s)),
        }
    }

    /// When it will be done, as unix milliseconds, if it is running and measured.
    pub fn done_at_ms(&self, now_ms: u64) -> Option<u64> {
        self.secs.filter(|_| !self.paused).map(|s| now_ms.saturating_add(s.saturating_mul(1000)))
    }
}

/// "About 3 h 20 min", "About 45 min", "Less than a minute" (rounded to what is worth saying).
pub fn phrase(secs: u64) -> String {
    if secs < 60 {
        return "Less than a minute".into();
    }
    let round = |x: u64, g: u64| (x + g / 2) / g * g;
    let mut mins = (secs + 30) / 60;
    if mins >= 24 * 60 {
        let hours = round(mins, 60) / 60;
        let (d, h) = (hours / 24, hours % 24);
        return if h == 0 { format!("About {d} d") } else { format!("About {d} d {h} h") };
    }
    if mins >= 180 {
        mins = round(mins, 10);
    } else if mins >= 10 {
        mins = round(mins, 5);
    }
    match (mins / 60, mins % 60) {
        (0, m) => format!("About {m} min"),
        (h, 0) => format!("About {h} h"),
        (h, m) => format!("About {h} h {m} min"),
    }
}

/// Time left per manga in the queue, and for the whole queue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub total: Line,
    pub manga: HashMap<String, Line>,
}

impl Report {
    pub fn of(&self, manga_id: &str) -> Option<Line> {
        self.manga.get(manga_id).copied()
    }
}

/// What the running step still needs: its expected time minus what it has done; an overrun is
/// given a little more time instead of showing nothing left.
fn running_left(full: f64, r: &Running) -> f64 {
    let left = full - r.elapsed;
    if left > 0. {
        left
    } else {
        (full * OVERRUN_SHARE).max(OVERRUN_MIN)
    }
}

/// One chapter's work per lane, and whether a step it needs has no measured rate.
fn lane_time(w: &Work, rate: &dyn Fn(&str, usize) -> Option<f64>) -> ([f64; LANES], bool) {
    let mut dur = [0.; LANES];
    let mut unknown = false;
    for step in (0..STEPS).filter(|&s| w.todo[s]) {
        let lane = if w.render_only { LANES - 1 } else { LANE_OF[step] };
        let full = match rate(&w.manga, step) {
            Some(r) => r * w.units[step].max(1.),
            None if step == TRANSCRIPT => TRANSCRIPT_SECS,
            None => {
                unknown = true;
                0.
            }
        };
        dur[lane] += match w.running.filter(|r| r.step == step) {
            Some(r) => running_left(full, &r) + r.wait_left.max(0.),
            None => full,
        };
    }
    (dur, unknown)
}

/// The lane a chapter is in (or enters) first.
fn entry_lane(w: &Work) -> usize {
    if w.render_only {
        return LANES - 1;
    }
    if let Some(r) = w.running {
        return LANE_OF[r.step];
    }
    if !w.started {
        return 0;
    }
    (0..STEPS).find(|&s| w.todo[s]).map_or(0, |s| LANE_OF[s])
}

/// Plays the queue forward. Every chapter goes through the lanes in order, a lane works on one
/// chapter at a time in the order they reach it (the running one first), and a manga is done when
/// its last chapter leaves the last lane. `paused`: the whole queue is on hold.
pub fn estimate(items: &[Work], paused: bool, rate: &dyn Fn(&str, usize) -> Option<f64>) -> Report {
    let n = items.len();
    let times: Vec<([f64; LANES], bool)> = items.iter().map(|w| lane_time(w, rate)).collect();
    let entry: Vec<usize> = items.iter().map(entry_lane).collect();
    let mut ready = vec![0f64; n];
    for lane in 0..LANES {
        // (rank, ready, queue index): the running one, then ones already handed on, then
        // render requests, then the rest in the order they arrive.
        let mut here: Vec<(u8, f64, usize)> = (0..n)
            .filter(|&i| entry[i] <= lane && (!items[i].render_only || lane == LANES - 1))
            .map(|i| {
                let w = &items[i];
                // A chapter on hold is never claimed, so it goes behind everything that will run.
                let rank = if w.held {
                    4
                } else if w.running.is_some_and(|r| LANE_OF[r.step] == lane || w.render_only) {
                    0
                } else if w.started && entry[i] == lane {
                    1
                } else if w.render_only {
                    2
                } else {
                    3
                };
                (rank, if entry[i] == lane { 0. } else { ready[i] }, i)
            })
            .collect();
        here.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut free = 0f64;
        for (_, at, i) in here {
            let d = times[i].0[lane];
            if d > 0. {
                free = free.max(at) + d;
                ready[i] = free;
            } else {
                ready[i] = at;
            }
        }
    }
    // A chapter whose rates are missing, or that comes after one, cannot be told yet.
    let first_unknown = times.iter().position(|t| t.1);
    let mut last_of: HashMap<&str, usize> = HashMap::new();
    let mut done_of: HashMap<&str, f64> = HashMap::new();
    let mut all_held: HashMap<&str, bool> = HashMap::new();
    for (i, w) in items.iter().enumerate() {
        last_of.insert(w.manga.as_str(), i);
        let d = done_of.entry(w.manga.as_str()).or_insert(0.);
        *d = d.max(ready[i]);
        let h = all_held.entry(w.manga.as_str()).or_insert(true);
        *h &= w.held;
    }
    let secs = |s: f64| s.max(1.).ceil() as u64;
    let mut report = Report::default();
    for (m, done) in &done_of {
        let known = first_unknown.is_none_or(|u| u > last_of[m]);
        report.manga.insert((*m).to_string(), Line { secs: known.then(|| secs(*done)), paused: paused || all_held[m] });
    }
    if n > 0 {
        let total = ready.iter().copied().fold(0., f64::max);
        report.total = Line { secs: first_unknown.is_none().then(|| secs(total)), paused: paused || all_held.values().all(|h| *h) };
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(manga: &str, todo: &[usize]) -> Work {
        let mut t = [false; STEPS];
        for &s in todo {
            t[s] = true;
        }
        Work { manga: manga.into(), todo: t, units: [10.; STEPS], started: false, running: None, render_only: false, held: false }
    }

    /// 1 s per page for Read, 2 for Story, 3 for Audio, 4 for Video: with 10 pages, 10 / 20 / 30 / 40 s.
    fn rates(_: &str, step: usize) -> Option<f64> {
        Some([1., 1., 2., 3., 1., 4.][step])
    }

    const FULL: [usize; 5] = [0, 2, 3, 5, 1];

    #[test]
    fn one_chapter_adds_its_steps() {
        let r = estimate(&[work("a", &[0, 2, 3, 5])], false, &rates);
        assert_eq!(r.of("a").unwrap().secs, Some(100));
        assert_eq!(r.total.secs, Some(100));
    }

    #[test]
    fn lanes_overlap_so_the_second_chapter_costs_its_slowest_lane() {
        // Video (40 s) is the slowest lane: chapter 2's video starts when chapter 1's is done.
        let r = estimate(&[work("a", &[0, 2, 3, 5]), work("a", &[0, 2, 3, 5])], false, &rates);
        // AI 30 s each, voice 30 s each, render 40 s each: 30+30+40 for the first, then the render lane.
        assert_eq!(r.of("a").unwrap().secs, Some(140));
    }

    #[test]
    fn work_ahead_counts_for_the_manga_behind_it() {
        let r = estimate(&[work("a", &[0, 2, 3, 5]), work("b", &[0, 2, 3, 5])], false, &rates);
        let (a, b) = (r.of("a").unwrap().secs.unwrap(), r.of("b").unwrap().secs.unwrap());
        assert_eq!(a, 100);
        assert!(b > a && b <= 200, "b waits only for the lanes, not for all of a: {b}");
        assert_eq!(r.total.secs, Some(b));
    }

    #[test]
    fn running_step_counts_down_and_overrun_never_hits_zero() {
        let mut w = work("a", &[0]);
        w.started = true;
        w.running = Some(Running { step: 0, elapsed: 4., wait_left: 0. });
        assert_eq!(estimate(&[w.clone()], false, &rates).total.secs, Some(6));
        w.running = Some(Running { step: 0, elapsed: 500., wait_left: 0. });
        let s = estimate(&[w.clone()], false, &rates).total.secs.unwrap();
        assert!(s >= 1 && s <= 60, "{s}");
        // Waiting for an AI limit adds the wait.
        w.running = Some(Running { step: 0, elapsed: 4., wait_left: 600. });
        assert_eq!(estimate(&[w], false, &rates).total.secs, Some(606));
    }

    #[test]
    fn missing_rate_means_estimating_for_it_and_what_waits_behind_it() {
        let no_video = |_: &str, s: usize| if s == 5 { None } else { rates("", s) };
        let r = estimate(&[work("a", &[0]), work("b", &[5]), work("c", &[0])], false, &no_video);
        assert!(r.of("a").unwrap().secs.is_some());
        assert_eq!(r.of("b").unwrap().secs, None);
        assert_eq!(r.of("c").unwrap().secs, None);
        assert_eq!(r.total.secs, None);
        assert_eq!(r.total.text(), "Estimating time left…");
    }

    #[test]
    fn transcript_needs_no_measurement() {
        let none = |_: &str, _: usize| -> Option<f64> { None };
        assert_eq!(estimate(&[work("a", &[1])], false, &none).total.secs, Some(1));
    }

    #[test]
    fn render_only_goes_straight_to_the_render_lane() {
        let mut w = work("a", &[5]);
        w.render_only = true;
        let r = estimate(&[work("b", &[0, 2, 3]), w], false, &rates);
        assert_eq!(r.of("a").unwrap().secs, Some(40));
    }

    #[test]
    fn a_chapter_already_in_the_voice_lane_does_not_wait_for_the_ai() {
        let mut w = work("a", &[3]);
        w.started = true;
        let r = estimate(&[work("b", &[0, 2]), w], false, &rates);
        assert_eq!(r.of("a").unwrap().secs, Some(30));
        assert_eq!(r.of("b").unwrap().secs, Some(30));
    }

    #[test]
    fn paused_is_said() {
        let r = estimate(&[work("a", &FULL)], true, &rates);
        let l = r.of("a").unwrap();
        assert!(l.paused && l.text().ends_with("left once resumed"));
        assert_eq!(l.done_at_ms(5), None);
        let mut held = work("b", &[0]);
        held.held = true;
        let r = estimate(&[held, work("c", &[0])], false, &rates);
        assert!(r.of("b").unwrap().paused && !r.of("c").unwrap().paused && !r.total.paused);
    }

    #[test]
    fn a_chapter_on_hold_does_not_delay_the_others() {
        let mut held = work("a", &FULL);
        held.held = true;
        let behind = estimate(&[held, work("b", &[0])], false, &rates);
        let alone = estimate(&[work("b", &[0])], false, &rates);
        assert_eq!(behind.of("b").unwrap().secs, alone.of("b").unwrap().secs);
    }

    #[test]
    fn never_zero() {
        let r = estimate(&[work("a", &[])], false, &rates);
        assert_eq!(r.of("a").unwrap().secs, Some(1));
    }

    #[test]
    fn rounding_reads_well() {
        assert_eq!(phrase(20), "Less than a minute");
        assert_eq!(phrase(60), "About 1 min");
        assert_eq!(phrase(7 * 60 + 10), "About 7 min");
        assert_eq!(phrase(44 * 60 + 40), "About 45 min");
        assert_eq!(phrase(2 * 3600), "About 2 h");
        assert_eq!(phrase(2 * 3600 + 7 * 60), "About 2 h 5 min");
        assert_eq!(phrase(3 * 3600 + 21 * 60), "About 3 h 20 min");
        assert_eq!(phrase(26 * 3600), "About 1 d 2 h");
        assert_eq!(Line { secs: Some(45 * 60), paused: false }.text(), "About 45 min left");
        assert_eq!(Line { secs: Some(2 * 3600), paused: true }.text(), "About 2 h left once resumed");
        assert_eq!(Line { secs: Some(90), paused: false }.done_at_ms(1000), Some(91_000));
    }

    #[test]
    fn history_prefers_the_manga_and_falls_back_to_everyone() {
        let mut h = History::default();
        h.record("a", 0, 100., 10., 1);
        h.record("b", 0, 50., 10., 2);
        assert_eq!(h.rate("a", 0), Some(10.));
        assert_eq!(h.rate("b", 0), Some(5.));
        // Everyone's median of 10 and 5 for a manga never measured.
        assert_eq!(h.rate("zzz", 0), Some(7.5));
        assert_eq!(h.rate("a", 3), None);
        // Bad numbers and the transcript are ignored.
        h.record("a", 3, f64::NAN, 10., 3);
        h.record("a", 3, 10., 0., 3);
        h.record("a", TRANSCRIPT, 10., 1., 3);
        assert_eq!(h.rate("a", 3), None);
        assert_eq!(h.rate("a", TRANSCRIPT), None);
    }

    #[test]
    fn history_stays_small_and_survives_a_slow_chapter() {
        let mut h = History::default();
        for i in 0..100u64 {
            h.record("a", 0, if i == 50 { 1000. } else { 10. }, 10., i);
        }
        assert_eq!(h.manga["a"].steps[0].len(), KEEP_MANGA);
        assert_eq!(h.global[0].len(), KEEP_GLOBAL);
        assert_eq!(h.rate("a", 0), Some(1.));
        for i in 0..(KEEP_MANGAS as u64 + 5) {
            h.record(&format!("m{i}"), 0, 10., 10., 1000 + i);
        }
        assert_eq!(h.manga.len(), KEEP_MANGAS);
        assert!(!h.manga.contains_key("a") && h.manga.contains_key(&format!("m{}", KEEP_MANGAS + 4)));
    }

    #[test]
    fn a_damaged_history_file_means_no_history() {
        let h: History = serde_json::from_slice(b"{\"global\": 5").ok().unwrap_or_default();
        assert_eq!(h, History::default());
        let h: History = serde_json::from_slice(b"{}").unwrap();
        assert_eq!(h.rate("a", 0), None);
    }
}
