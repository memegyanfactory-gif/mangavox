//! The channel on autopilot: what the Automation page runs and remembers (`youtube/automation.json`),
//! as data and pure functions so it can be tested without YouTube or Studio.
//!
//! A [`Job`] is one manga turned into one long recap video, moving through [`Stage`]s:
//! Discover → Download (every chapter first) → Read (every chapter) → Write (the whole story, with
//! all chapters known) → Render → Approve → Schedule → Publish (uploaded private with
//! `status.publishAt`, so YouTube publishes it with the PC off) → Collect (views, retention and
//! comments, fed back into the next suggestions). Every stage is worked out again from what is on
//! disk, so a job carries on after a restart.
//!
//! Release time ([`best_slot`]): the user's own slots (5:30, 6:30, 6:45 PM by default), the golden
//! hours of other recap channels (`timing`), published studies on when long videos do best, and
//! once the channel has history, how its own uploads did by release hour.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use chrono::{Datelike, FixedOffset, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

use crate::types::{DayStat, Video};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    #[default]
    Discover,
    Download,
    Read,
    Write,
    Render,
    Approve,
    Schedule,
    Publish,
    Collect,
    Done,
}

impl Stage {
    pub const PIPELINE: [Stage; 9] = [Stage::Discover, Stage::Download, Stage::Read, Stage::Write, Stage::Render, Stage::Approve, Stage::Schedule, Stage::Publish, Stage::Collect];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Discover => "Discover",
            Stage::Download => "Download",
            Stage::Read => "Read",
            Stage::Write => "Write",
            Stage::Render => "Edit & render",
            Stage::Approve => "Approve",
            Stage::Schedule => "Schedule",
            Stage::Publish => "Publish",
            Stage::Collect => "Collect results",
            Stage::Done => "Done",
        }
    }

    /// What the stage does, for its tooltip and the empty state.
    pub fn detail(self) -> &'static str {
        match self {
            Stage::Discover => "Finds the manga recap viewers are loving right now",
            Stage::Download => "Downloads every chapter before anything else",
            Stage::Read => "The AI reads every chapter, page by page",
            Stage::Write => "Writes the whole story in one pass, knowing every chapter",
            Stage::Render => "Voices, edits and renders through the normal Studio pipeline",
            Stage::Approve => "You check the finished video (skipped in full auto)",
            Stage::Schedule => "Picks the best release time",
            Stage::Publish => "Uploads it private with a publish time; YouTube publishes it",
            Stage::Collect => "Reads views, retention and comments to improve the next pick",
            Stage::Done => "Finished",
        }
    }

    pub fn index(self) -> usize {
        Stage::PIPELINE.iter().position(|s| *s == self).unwrap_or(Stage::PIPELINE.len())
    }

    pub fn next(self) -> Stage {
        Stage::PIPELINE.get(self.index() + 1).copied().unwrap_or(Stage::Done)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    /// Working on `stage` (or waiting for its turn).
    #[default]
    Running,
    /// Waiting for the user (Approve).
    Waiting,
    /// Paused by the user.
    Paused,
    Failed,
    Done,
}

/// A manga the Discover step suggests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pick {
    /// MangaDex id.
    pub mangadex_id: String,
    pub title: String,
    pub cover: String,
    /// 0-100, best first.
    pub score: u8,
    /// One line on why ("1.2M recap views this month, 4 channels, 3 viewers asked for it").
    pub reason: String,
    pub recap_views: u64,
    /// Views per hour of the newest recaps.
    pub velocity: f64,
    pub channels: usize,
    /// Commenters on the user's videos who asked for it.
    pub asked: u32,
    /// A recap of it that is doing well right now: (title, url).
    pub example: Option<(String, String)>,
}

/// Numbers read after release.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Results {
    pub views: u64,
    pub likes: u64,
    pub comments: u64,
    /// Average view duration in seconds.
    pub avg_view_secs: f64,
    /// Share of the video an average viewer watches (0..1).
    pub retention: f64,
    pub collected_at: i64,
}

/// One manga on its way to a published recap.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Job {
    /// "auto-<millis>"
    pub id: String,
    pub pick: Pick,
    /// The manga's id in the library once it is imported.
    pub manga_id: Option<String>,
    pub stage: Stage,
    pub state: JobState,
    /// Chapters this video covers, in reading order (library chapter ids).
    pub chapters: Vec<String>,
    /// Chapters past the current stage (downloaded, read, rendered...).
    pub done: usize,
    /// One line on what the stage is doing.
    pub step: String,
    pub error: Option<String>,
    /// The joined recap.
    pub file: Option<PathBuf>,
    pub thumbnail: Option<PathBuf>,
    pub duration_secs: f64,
    /// The upload queue item.
    pub upload_id: Option<String>,
    pub video_id: Option<String>,
    pub publish_at: Option<i64>,
    /// Why that time ("6:30 PM: your slot, golden hour of recap channels, Sunday").
    pub why_time: String,
    pub results: Option<Results>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The user approved the render (or full auto did).
    pub approved: bool,
    /// Started by full auto, not by a click.
    pub auto: bool,
    /// The stage whose Studio work is queued already (so a tick does not queue it twice).
    pub queued_for: Option<Stage>,
}

impl Job {
    /// Overall progress 0..1 across the stages (the current one counts by `done / chapters`).
    pub fn progress(&self) -> f32 {
        if self.stage == Stage::Done {
            return 1.;
        }
        let n = Stage::PIPELINE.len() as f32;
        let within = if self.chapters.is_empty() { 0. } else { (self.done as f32 / self.chapters.len() as f32).clamp(0., 1.) };
        let within = if matches!(self.stage, Stage::Download | Stage::Read | Stage::Write | Stage::Render) { within } else { 0. };
        ((self.stage.index() as f32 + within) / n).clamp(0., 1.)
    }

    pub fn active(&self) -> bool {
        matches!(self.state, JobState::Running | JobState::Waiting)
    }

    /// Building the video (not yet waiting on YouTube or results).
    pub fn producing(&self) -> bool {
        self.active() && self.stage < Stage::Publish
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogEntry {
    pub at: i64,
    pub kind: LogKind,
    pub job: Option<String>,
    pub text: String,
    /// Done by full auto without asking.
    pub auto: bool,
}

/// A release slot the user likes, local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub hour: u8,
    pub minute: u8,
}

impl Slot {
    pub const fn new(hour: u8, minute: u8) -> Self {
        Slot { hour, minute }
    }

    /// Minutes after local midnight.
    pub fn minutes(self) -> i64 {
        self.hour as i64 * 60 + self.minute as i64
    }
}

/// The user's own research: around 5–7 PM.
pub const DEFAULT_SLOTS: [Slot; 3] = [Slot::new(17, 30), Slot::new(18, 30), Slot::new(18, 45)];

/// `youtube/automation.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Automation {
    /// Runs everything, discovery to scheduling, without asking.
    pub full_auto: bool,
    /// Without full auto: wait for the user to approve each render.
    pub approve_renders: bool,
    /// Chapters per video (0 = every chapter of the manga).
    pub max_chapters: usize,
    /// Videos built at the same time.
    pub parallel: usize,
    /// Days between two automatic releases.
    pub cadence_days: u32,
    pub slots: Vec<Slot>,
    pub picks: Vec<Pick>,
    pub picks_at: i64,
    pub jobs: Vec<Job>,
    /// Newest last, at most [`LOG_MAX`].
    pub log: Vec<LogEntry>,
    /// What comments asked for, summed over the channel ("omniscient reader" → 4).
    pub requests: BTreeMap<String, u32>,
    /// The last time comments and results were collected.
    pub collected_at: i64,
}

pub const LOG_MAX: usize = 300;

impl Default for Automation {
    fn default() -> Self {
        Self {
            full_auto: false,
            approve_renders: true,
            max_chapters: 50,
            parallel: 1,
            cadence_days: 2,
            slots: DEFAULT_SLOTS.to_vec(),
            picks: vec![],
            picks_at: 0,
            jobs: vec![],
            log: vec![],
            requests: BTreeMap::new(),
            collected_at: 0,
        }
    }
}

impl Automation {
    pub fn log(&mut self, now: i64, kind: LogKind, job: Option<&str>, text: impl Into<String>) {
        let auto = self.full_auto;
        self.log.push(LogEntry { at: now, kind, job: job.map(str::to_string), text: text.into(), auto });
        if self.log.len() > LOG_MAX {
            let extra = self.log.len() - LOG_MAX;
            self.log.drain(..extra);
        }
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn job_mut(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Already made or in the works (so Discover does not suggest it again).
    pub fn has_manga(&self, mangadex_id: &str) -> bool {
        self.jobs.iter().any(|j| j.pick.mangadex_id == mangadex_id && j.state != JobState::Failed)
    }

    /// Release times taken by the automation's jobs.
    pub fn taken(&self) -> Vec<i64> {
        self.jobs.iter().filter(|j| j.state != JobState::Failed).filter_map(|j| j.publish_at).collect()
    }

    /// Full auto may start another video: fewer than `parallel` are being built.
    pub fn room_for_more(&self) -> bool {
        self.jobs.iter().filter(|j| j.producing()).count() < self.parallel.max(1)
    }
}

/// A new job id.
pub fn new_job_id(now_millis: u128) -> String {
    format!("auto-{now_millis}")
}

/// The chapters a video covers: the first `max` in reading order (all when `max` is 0).
pub fn pick_chapters<T: Clone>(ordered: &[T], max: usize) -> Vec<T> {
    if max == 0 {
        ordered.to_vec()
    } else {
        ordered.iter().take(max).cloned().collect()
    }
}

/// Discover's ranking: the trends crawl's opportunity score, plus what the channel's own viewers
/// ask for in comments (each ask +6, up to +24), minus manga already made.
pub fn rank_picks(mut picks: Vec<Pick>, requests: &BTreeMap<String, u32>, auto: &Automation) -> Vec<Pick> {
    for p in picks.iter_mut() {
        let name = p.title.to_lowercase();
        let asked: u32 = requests.iter().filter(|(k, _)| crate::trends::similarity(k, &name) >= 0.8).map(|(_, n)| *n).sum();
        p.asked = asked;
        p.score = (p.score as u32 + (asked * 6).min(24)).min(100) as u8;
        if asked > 0 {
            p.reason.push_str(&format!(", {asked} of your viewers asked for it"));
        }
    }
    picks.retain(|p| !auto.has_manga(&p.mangadex_id));
    picks.sort_by(|a, b| b.score.cmp(&a.score).then(b.recap_views.cmp(&a.recap_views)));
    picks
}

/// Day-of-week weights for long videos (Monday = 0), from large studies of when long-form uploads
/// do best: Sunday and Saturday a little above the average day, Wednesday a little below
/// (Buffer 2026, 1.8M videos; vidIQ, 40M US long-form videos; SocialPilot, 301k videos).
pub const DAY_WEIGHT: [f64; 7] = [1.02, 1.02, 0.97, 1.0, 1.0, 1.03, 1.05];

/// How the channel's own uploads did by local release hour: views relative to the channel's
/// median, per hour, with how many videos back it (only videos at least 3 days old count).
pub fn own_hour_lift(videos: &[Video], now: i64, offset: i32) -> [(f64, usize); 24] {
    let settled: Vec<&Video> = videos.iter().filter(|v| v.privacy == "public" && v.published_at > 0 && now - v.published_at > 3 * 86_400).collect();
    let mut out = [(1.0, 0usize); 24];
    if settled.len() < 3 {
        return out;
    }
    let mut views: Vec<u64> = settled.iter().map(|v| v.views).collect();
    views.sort_unstable();
    let median = views[views.len() / 2].max(1) as f64;
    let mut by_hour: HashMap<usize, Vec<f64>> = HashMap::new();
    for v in settled {
        let (_, h) = crate::timing::local_day_hour(v.published_at, offset);
        by_hour.entry(h).or_default().push(v.views as f64 / median);
    }
    for (h, l) in by_hour {
        let n = l.len();
        // Shrink toward 1.0 until a few videos back the hour.
        let mean = l.iter().sum::<f64>() / n as f64;
        let k = 3.0;
        out[h] = ((mean * n as f64 + k) / (n as f64 + k), n);
    }
    out
}

/// Retention of a video: average view duration over its length (0..1).
pub fn retention(avg_view_secs: f64, duration_secs: u64) -> f64 {
    if duration_secs == 0 {
        0.
    } else {
        (avg_view_secs / duration_secs as f64).clamp(0., 1.)
    }
}

/// Totals of a video's daily analytics: (views, average view seconds weighted by views, likes).
pub fn sum_daily(days: &[DayStat]) -> (u64, f64, u64) {
    let views: u64 = days.iter().map(|d| d.views).sum();
    let avg = if views == 0 { 0. } else { days.iter().map(|d| d.avg_view_secs * d.views as f64).sum::<f64>() / views as f64 };
    (views, avg, days.iter().map(|d| d.likes).sum())
}

/// What decides a release time.
#[derive(Clone, Debug, Default)]
pub struct SlotInputs {
    /// The user's preferred times.
    pub slots: Vec<Slot>,
    /// Golden windows from the timing research: (local start hour, hours, lift).
    pub golden: Vec<(usize, usize, f64)>,
    /// The channel's own lift per local hour (see [`own_hour_lift`]).
    pub own: Option<[(f64, usize); 24]>,
    /// Release times already used (this video should not land within `spacing_hours` of one).
    pub taken: Vec<i64>,
    pub spacing_hours: i64,
    /// Never sooner than this many hours from now (upload and processing time).
    pub min_lead_hours: i64,
    /// Look this many days ahead.
    pub horizon_days: i64,
}

/// A chosen release time and the reasons for it.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotChoice {
    pub at: i64,
    pub score: f64,
    pub reasons: Vec<String>,
}

const DAY_NAMES: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// The best release time after `now` (local zone `offset` seconds east of UTC, no DST). Same as
/// [`best_slot_tz`] with a fixed offset.
pub fn best_slot(now: i64, offset: i32, inp: &SlotInputs) -> Option<SlotChoice> {
    best_slot_tz(now, &FixedOffset::east_opt(offset)?, inp)
}

/// The best release time after `now` in the zone `tz` (DST-aware: every candidate is built from its
/// local date and time, so "6:30 PM" stays 6:30 PM across a clock change). Candidates are the
/// user's slots plus the start of each golden window, on each of the next `horizon_days` days.
/// Score = day weight × (1 + 0.12 for the user's slot + golden lift above 1 + own lift above 1),
/// minus a little for every day of waiting, so a near-best time this week beats a perfect one later.
/// Local times that do not exist (the spring-forward gap) are skipped; an ambiguous one takes its
/// first occurrence.
pub fn best_slot_tz<Tz: TimeZone>(now: i64, tz: &Tz, inp: &SlotInputs) -> Option<SlotChoice> {
    let today = tz.timestamp_opt(now, 0).single()?.date_naive();
    let mut minutes: Vec<(i64, bool)> = inp.slots.iter().map(|s| (s.minutes(), true)).collect();
    for (h, _, _) in &inp.golden {
        let m = (*h as i64 % 24) * 60;
        if !minutes.iter().any(|(x, _)| *x == m) {
            minutes.push((m, false));
        }
    }
    if minutes.is_empty() {
        minutes.push((18 * 60, false));
    }
    let earliest = now + inp.min_lead_hours.max(0) * 3600;
    let mut best: Option<SlotChoice> = None;
    for d in 0..inp.horizon_days.max(1) {
        let Some(date) = today.checked_add_signed(chrono::Duration::days(d)) else { continue };
        for (m, preferred) in &minutes {
            let Some(local) = date.and_hms_opt((m / 60) as u32, (m % 60) as u32, 0) else { continue };
            let Some(when) = tz.from_local_datetime(&local).earliest() else { continue };
            let at = when.timestamp();
            if at < earliest || inp.taken.iter().any(|t| (t - at).abs() < inp.spacing_hours * 3600) {
                continue;
            }
            let day = when.weekday().num_days_from_monday() as usize;
            let hour = when.hour() as usize;
            let mut reasons = vec![];
            let mut s = 1.0;
            if *preferred {
                s += 0.12;
                reasons.push("your preferred time".to_string());
            }
            if let Some((_, _, lift)) = inp.golden.iter().find(|(h, n, _)| (0..*n).any(|i| (h + i) % 24 == hour)) {
                s += (lift - 1.0).clamp(-0.3, 0.6);
                if *lift > 1.0 {
                    reasons.push(format!("recap channels get {lift:.1}× their usual views then"));
                }
            }
            if let Some(own) = &inp.own {
                let (lift, n) = own[hour];
                if n > 0 {
                    s += (lift - 1.0).clamp(-0.4, 0.8);
                    if lift > 1.05 {
                        reasons.push(format!("your videos released at this hour did {lift:.1}× your usual"));
                    }
                }
            }
            s *= DAY_WEIGHT[day];
            if DAY_WEIGHT[day] > 1.02 {
                reasons.push(format!("{} is a strong day for long videos", DAY_NAMES[day]));
            }
            s -= 0.015 * d as f64;
            if best.as_ref().is_none_or(|b| s > b.score + 1e-9) {
                best = Some(SlotChoice { at, score: s, reasons });
            }
        }
    }
    best
}

/// A release time that already passed (or is too close) moved to the same local wall-clock time on
/// the first later day that is at least `min_lead_secs` from `now`. `at` comes back unchanged when
/// it is far enough ahead. DST-aware: local times that do not exist are skipped.
pub fn roll_forward<Tz: TimeZone>(at: i64, now: i64, min_lead_secs: i64, tz: &Tz) -> i64 {
    let floor = now + min_lead_secs;
    if at >= floor {
        return at;
    }
    let (Some(then), Some(today)) = (tz.timestamp_opt(at, 0).single(), tz.timestamp_opt(now, 0).single()) else { return floor };
    let time = then.time();
    let today = today.date_naive();
    for d in 0..=400 {
        let Some(date) = today.checked_add_signed(chrono::Duration::days(d)) else { break };
        if let Some(c) = tz.from_local_datetime(&date.and_time(time)).earliest() {
            if c.timestamp() >= floor {
                return c.timestamp();
            }
        }
    }
    floor
}

#[cfg(test)]
mod tests {
    use super::*;

    // Monday 2026-11-02 12:00 UTC.
    const NOW: i64 = 1_793_620_800;

    #[test]
    fn stages_walk_in_order() {
        assert_eq!(Stage::Discover.next(), Stage::Download);
        assert_eq!(Stage::Approve.next(), Stage::Schedule);
        assert_eq!(Stage::Collect.next(), Stage::Done);
        assert_eq!(Stage::Done.next(), Stage::Done);
        let j = Job { stage: Stage::Read, chapters: vec!["a".into(), "b".into()], done: 1, ..Default::default() };
        let p = j.progress();
        assert!(p > 2. / 9. && p < 3. / 9., "{p}");
        assert!(j.producing());
    }

    #[test]
    fn log_is_capped() {
        let mut a = Automation::default();
        for i in 0..LOG_MAX + 10 {
            a.log(i as i64, LogKind::Info, None, format!("{i}"));
        }
        assert_eq!(a.log.len(), LOG_MAX);
        assert_eq!(a.log[0].text, "10");
    }

    #[test]
    fn json_round_trip_and_defaults() {
        let a: Automation = serde_json::from_str("{}").unwrap();
        assert_eq!(a, Automation::default());
        assert_eq!(a.slots, DEFAULT_SLOTS.to_vec());
        let mut a = Automation { full_auto: true, ..Default::default() };
        a.jobs.push(Job { id: "auto-1".into(), stage: Stage::Render, chapters: vec!["c1".into()], ..Default::default() });
        let back: Automation = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn picks_rank_with_viewer_requests() {
        let p = |id: &str, title: &str, score: u8| Pick { mangadex_id: id.into(), title: title.into(), score, ..Default::default() };
        let mut req = BTreeMap::new();
        req.insert("omniscient reader".to_string(), 3);
        let mut auto = Automation::default();
        auto.jobs.push(Job { pick: p("made", "Made Already", 0), ..Default::default() });
        let out = rank_picks(vec![p("a", "Solo Leveling", 70), p("b", "Omniscient Reader", 60), p("made", "Made Already", 99)], &req, &auto);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].mangadex_id, "b");
        assert_eq!(out[0].score, 78);
        assert_eq!(out[0].asked, 3);
    }

    #[test]
    fn chapters_are_capped() {
        let v: Vec<u32> = (1..=80).collect();
        assert_eq!(pick_chapters(&v, 50).len(), 50);
        assert_eq!(pick_chapters(&v, 0).len(), 80);
    }

    #[test]
    fn best_slot_prefers_user_times_and_respects_lead_and_spacing() {
        let ist = 19_800;
        let inp = SlotInputs { slots: DEFAULT_SLOTS.to_vec(), min_lead_hours: 2, spacing_hours: 20, horizon_days: 7, ..Default::default() };
        let c = best_slot(NOW, ist, &inp).unwrap();
        // 12:00 UTC is 17:30 IST: 17:30 today is too soon (2 h lead); 18:30 and 18:45 also are.
        // Monday's weight equals Tuesday's, so the next day's 17:30 loses only to waiting less... the
        // first candidate that fits is tomorrow 17:30 IST unless the weekend outweighs the wait.
        let (day, hour) = crate::timing::local_day_hour(c.at, ist);
        assert!(c.at >= NOW + 2 * 3600);
        assert!((17..=18).contains(&hour), "{hour}");
        assert!(day == 1 || day >= 5, "{day}");
        // A taken time pushes it away by the spacing.
        let taken = SlotInputs { taken: vec![c.at], ..inp.clone() };
        let c2 = best_slot(NOW, ist, &taken).unwrap();
        assert!((c2.at - c.at).abs() >= 20 * 3600);
    }

    #[test]
    fn golden_and_own_data_move_the_time() {
        let inp = SlotInputs {
            slots: vec![Slot::new(18, 30)],
            golden: vec![(21, 2, 1.8)],
            min_lead_hours: 1,
            spacing_hours: 20,
            horizon_days: 3,
            ..Default::default()
        };
        let c = best_slot(NOW, 0, &inp).unwrap();
        assert_eq!(crate::timing::local_day_hour(c.at, 0).1, 21);
        assert!(c.reasons.iter().any(|r| r.contains("1.8×")));
        let mut own = [(1.0, 0usize); 24];
        own[18] = (2.5, 6);
        let c = best_slot(NOW, 0, &SlotInputs { own: Some(own), ..inp }).unwrap();
        assert_eq!(crate::timing::local_day_hour(c.at, 0).1, 18);
    }

    #[test]
    fn own_lift_by_hour() {
        let v = |id: &str, h: i64, views: u64| Video { id: id.into(), privacy: "public".into(), published_at: NOW - 10 * 86_400 + h * 3600 - 12 * 3600, views, ..Default::default() };
        let vids = vec![v("a", 18, 3000), v("b", 18, 2800), v("c", 9, 500), v("d", 9, 600), v("e", 12, 1000)];
        let l = own_hour_lift(&vids, NOW, 0);
        assert!(l[18].0 > 1.2 && l[18].1 == 2);
        assert!(l[9].0 < 1.0);
        assert_eq!(l[3].1, 0);
        assert_eq!(retention(300., 600), 0.5);
        let (views, avg, _) = sum_daily(&[DayStat { views: 10, avg_view_secs: 100., ..Default::default() }, DayStat { views: 30, avg_view_secs: 200., ..Default::default() }]);
        assert_eq!((views, avg), (40, 175.));
    }

    /// US Central time by hand (no tz database here): UTC-5 until the clocks go back at
    /// 2026-11-03 06:00 UTC, UTC-6 after.
    #[derive(Clone)]
    struct Central;
    #[derive(Clone, Copy)]
    struct Off(i32);
    impl chrono::Offset for Off {
        fn fix(&self) -> FixedOffset {
            FixedOffset::east_opt(self.0).unwrap()
        }
    }
    impl std::fmt::Debug for Off {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl std::fmt::Display for Off {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    const SWITCH: i64 = NOW + 18 * 3600;
    impl TimeZone for Central {
        type Offset = Off;
        fn from_offset(_: &Off) -> Self {
            Central
        }
        fn offset_from_local_date(&self, d: &chrono::NaiveDate) -> chrono::LocalResult<Off> {
            self.offset_from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, l: &chrono::NaiveDateTime) -> chrono::LocalResult<Off> {
            chrono::LocalResult::Single(Off(if l.and_utc().timestamp() + 5 * 3600 < SWITCH { -5 * 3600 } else { -6 * 3600 }))
        }
        fn offset_from_utc_date(&self, d: &chrono::NaiveDate) -> Off {
            self.offset_from_utc_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, u: &chrono::NaiveDateTime) -> Off {
            Off(if u.and_utc().timestamp() < SWITCH { -5 * 3600 } else { -6 * 3600 })
        }
    }

    #[test]
    fn best_slot_follows_dst() {
        let inp = SlotInputs { slots: vec![Slot::new(18, 30)], min_lead_hours: 1, spacing_hours: 20, horizon_days: 2, ..Default::default() };
        let c = best_slot_tz(NOW, &Central, &inp).unwrap();
        let c2 = best_slot_tz(NOW, &Central, &SlotInputs { taken: vec![c.at], ..inp.clone() }).unwrap();
        for at in [c.at, c2.at] {
            let l = Central.timestamp_opt(at, 0).unwrap();
            assert_eq!((l.hour(), l.minute()), (18, 30));
        }
        // Monday 18:30 at -5h and Tuesday 18:30 at -6h are 25 hours apart, not 24.
        assert_eq!(c2.at - c.at, 25 * 3600);
        // The fixed-offset wrapper agrees with the zone version.
        assert_eq!(best_slot(NOW, 19_800, &inp), best_slot_tz(NOW, &FixedOffset::east_opt(19_800).unwrap(), &inp));
    }

    #[test]
    fn roll_forward_keeps_wall_clock() {
        let ist = FixedOffset::east_opt(19_800).unwrap();
        // 18:30 IST three days ago.
        let at = NOW - 3 * 86_400 + 3600;
        assert_eq!(roll_forward(NOW + 10 * 3600, NOW, 3600, &ist), NOW + 10 * 3600, "far enough: unchanged");
        let r = roll_forward(at, NOW, 3600, &ist);
        assert!(r >= NOW + 3600 && r - NOW < 86_400 + 3600);
        let l = ist.timestamp_opt(r, 0).unwrap();
        assert_eq!((l.hour(), l.minute()), (18, 30));
        // Too close counts as passed.
        let soon = roll_forward(NOW + 600, NOW, 3600, &ist);
        assert_eq!(soon, NOW + 600 + 86_400);
        // Across the clock change the wall-clock time is kept (25 h later).
        let at = SWITCH - 10 * 3600;
        let r = roll_forward(at, SWITCH - 5 * 3600 + 1, 0, &Central);
        let (a, b) = (Central.timestamp_opt(at, 0).unwrap(), Central.timestamp_opt(r, 0).unwrap());
        assert_eq!((a.hour(), a.minute()), (b.hour(), b.minute()));
        assert_eq!(r - at, 25 * 3600);
    }
}
