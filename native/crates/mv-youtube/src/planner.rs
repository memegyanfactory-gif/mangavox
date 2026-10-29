//! The smart scheduler, as pure functions so it can be tested without YouTube: series order
//! (Chapter 2 goes out before Chapter 5), release slots, dip detection and the suggestions and
//! automatic reschedules that follow from them.
use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};

use crate::types::{Action, DayStat, LocalEpisode, Settings, Severity, Suggestion, Trend, UploadItem, UploadState, Video};

const EPS: f64 = 1e-6;
const DAY: i64 = 86_400;

fn same_chapter(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

fn num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// "Fri 18:00" in the local zone.
fn fmt_local(ts: i64, utc_offset_secs: i32) -> String {
    let tz = FixedOffset::east_opt(utc_offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    DateTime::<Utc>::from_timestamp(ts, 0).map(|d| d.with_timezone(&tz).format("%a %H:%M").to_string()).unwrap_or_default()
}

fn movable(i: &UploadItem) -> bool {
    matches!(i.state, UploadState::Planned | UploadState::Scheduled)
}

/// In the queue (any state but Failed) for this manga and chapter.
fn is_queued(queue: &[UploadItem], manga_id: &str, chapter: f64) -> bool {
    queue.iter().any(|q| q.state != UploadState::Failed && q.manga_id.as_deref() == Some(manga_id) && q.chapter.is_some_and(|c| same_chapter(c, chapter)))
}

/// Earlier chapters of `item`'s series that exist as finished recaps here but are not in the
/// queue (a release before them would break the story for viewers). Queue items that are
/// planned, scheduled or already on YouTube count as present. Sorted ascending.
pub fn missing_before(manga_id: &str, chapter: f64, episodes: &[LocalEpisode], queue: &[UploadItem]) -> Vec<f64> {
    let mut out: Vec<f64> = episodes
        .iter()
        .filter(|e| e.manga_id == manga_id)
        .filter_map(|e| e.chapter)
        .filter(|&c| c < chapter - EPS && !is_queued(queue, manga_id, c))
        .collect();
    out.sort_by(|a, b| a.total_cmp(b));
    out.dedup_by(|a, b| same_chapter(*a, *b));
    out
}

/// Per manga, the lowest-numbered finished recap that is not queued or uploaded yet.
pub fn next_up<'a>(episodes: &'a [LocalEpisode], queue: &[UploadItem]) -> Vec<&'a LocalEpisode> {
    let queued = |e: &LocalEpisode| {
        queue.iter().any(|q| q.state != UploadState::Failed && (q.chapter_id.as_deref() == Some(e.chapter_id.as_str()) || (q.manga_id.as_deref() == Some(e.manga_id.as_str()) && matches!((q.chapter, e.chapter), (Some(a), Some(b)) if same_chapter(a, b)))))
    };
    let mut order: Vec<&str> = Vec::new();
    let mut best: HashMap<&str, &LocalEpisode> = HashMap::new();
    for e in episodes.iter().filter(|e| !queued(e)) {
        let m = e.manga_id.as_str();
        match best.get(m) {
            None => {
                order.push(m);
                best.insert(m, e);
            }
            Some(cur) => {
                let better = match (e.chapter, cur.chapter) {
                    (Some(a), Some(b)) => a < b,
                    (Some(_), None) => true,
                    _ => false,
                };
                if better {
                    best.insert(m, e);
                }
            }
        }
    }
    order.into_iter().map(|m| best[m]).collect()
}

/// Sorts episodes of one manga by chapter number (unnumbered last, stable).
pub fn series_order(episodes: &mut [LocalEpisode]) {
    episodes.sort_by(|a, b| match (a.chapter, b.chapter) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// `n` release times after `now`: the first at `settings.publish_hour` local time at least
/// `min_lead_hours` ahead, then every `cadence_hours`, skipping any time within half a cadence of
/// one in `taken`. `utc_offset_secs` is the local zone's offset (east positive).
pub fn next_slots(now: i64, taken: &[i64], settings: &Settings, n: usize, utc_offset_secs: i32) -> Vec<i64> {
    let off = FixedOffset::east_opt(utc_offset_secs).map(|o| o.local_minus_utc()).unwrap_or(0) as i64;
    let earliest = now + settings.min_lead_hours as i64 * 3600;
    let day_start_local = (earliest + off).div_euclid(DAY) * DAY;
    let mut t = day_start_local + settings.publish_hour.min(23) as i64 * 3600 - off;
    if t < earliest {
        t += DAY;
    }
    let cadence = settings.cadence_hours.max(1) as i64 * 3600;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        if !taken.iter().any(|&x| (x - t).abs() * 2 < cadence) {
            out.push(t);
        }
        t += cadence;
    }
    out
}

/// Gives every planned or scheduled item of a series a release time in chapter order: an item
/// may not go out before an earlier chapter of the same manga. Items without a time get the next
/// free slot after the chapter before them (when that chapter has a time). Returns the ids whose
/// `publish_at` changed.
pub fn order_series(queue: &mut [UploadItem], now: i64, settings: &Settings, utc_offset_secs: i32) -> Vec<String> {
    let mut changed = Vec::new();
    let mut mangas: Vec<String> = Vec::new();
    for q in queue.iter() {
        if let Some(m) = &q.manga_id {
            if q.chapter.is_some() && q.state != UploadState::Failed && !mangas.contains(m) {
                mangas.push(m.clone());
            }
        }
    }
    for manga in mangas {
        let mut idx: Vec<usize> = (0..queue.len()).filter(|&i| queue[i].manga_id.as_deref() == Some(manga.as_str()) && queue[i].chapter.is_some() && queue[i].state != UploadState::Failed).collect();
        idx.sort_by(|&a, &b| queue[a].chapter.unwrap_or(0.0).total_cmp(&queue[b].chapter.unwrap_or(0.0)));
        let mut prev: Option<i64> = None;
        for i in idx {
            if movable(&queue[i]) {
                if let Some(p) = prev {
                    let ok = queue[i].publish_at.is_some_and(|t| t > p);
                    if !ok {
                        let taken: Vec<i64> = queue.iter().enumerate().filter(|(j, q)| *j != i && q.state != UploadState::Failed).filter_map(|(_, q)| q.publish_at).collect();
                        let slot = next_slots(now, &taken, settings, 64, utc_offset_secs).into_iter().find(|&s| s > p).unwrap_or(p + settings.cadence_hours.max(1) as i64 * 3600);
                        let old = queue[i].publish_at;
                        if old != Some(slot) {
                            if old.is_some() {
                                queue[i].rescheduled_from = old;
                            }
                            queue[i].publish_at = Some(slot);
                            changed.push(queue[i].id.clone());
                        }
                    }
                }
            }
            if let Some(t) = queue[i].publish_at {
                prev = Some(t);
            }
        }
    }
    changed
}

/// How a video is doing from its daily views since release (`daily[0]` = release day; the last
/// entry may be a partial day and is ignored). Dipping: the latest full day is at least
/// `threshold` below the best day and below the day before it. Rising: latest full day is the
/// best so far. Steady otherwise.
pub fn trend(daily: &[u64], threshold: f32) -> Trend {
    if daily.len() < 3 {
        return Trend::TooNew;
    }
    let full = &daily[..daily.len() - 1];
    let latest = full[full.len() - 1];
    let before = full[full.len() - 2];
    let best = full.iter().copied().max().unwrap_or(0);
    if best == 0 {
        return Trend::Steady;
    }
    let drop = 1.0 - latest as f32 / best as f32;
    if drop >= threshold && latest < before {
        Trend::Dipping { drop }
    } else if latest >= best && latest > before {
        Trend::Rising
    } else {
        Trend::Steady
    }
}

/// Days since release (0 on release day).
pub fn age_days(now: i64, published_at: i64) -> i64 {
    (now - published_at).div_euclid(DAY)
}

/// The next queued (planned or scheduled, not yet public) episode of `manga_id` after
/// `after_chapter`, earliest chapter first.
pub fn next_in_series<'a>(queue: &'a [UploadItem], manga_id: &str, after_chapter: Option<f64>) -> Option<&'a UploadItem> {
    queue
        .iter()
        .filter(|q| movable(q) && q.manga_id.as_deref() == Some(manga_id))
        .filter(|q| q.chapter.is_some_and(|c| after_chapter.is_none_or(|a| c > a + EPS)))
        .min_by(|a, b| a.chapter.unwrap_or(0.0).total_cmp(&b.chapter.unwrap_or(0.0)))
}

fn views_of(daily: &HashMap<String, Vec<DayStat>>, id: &str) -> Vec<u64> {
    daily.get(id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default()
}

/// A dipping public video released 1-5 days ago: (drop, video).
fn dipping<'a>(now: i64, videos: &'a [Video], daily: &HashMap<String, Vec<DayStat>>, settings: &Settings) -> Vec<(f32, &'a Video, usize)> {
    videos
        .iter()
        .filter(|v| v.privacy != "private" && (1..=5).contains(&age_days(now, v.published_at)))
        .filter_map(|v| {
            let views = views_of(daily, &v.id);
            match trend(&views, settings.dip_threshold) {
                Trend::Dipping { drop } => Some((drop, v, views.len())),
                _ => None,
            }
        })
        .collect()
}

/// The earliest free slot for `next` that is not before `floor`.
fn earliest_slot(floor: i64, queue: &[UploadItem], skip_id: &str, settings: &Settings, utc_offset_secs: i32) -> i64 {
    let taken: Vec<i64> = queue.iter().filter(|q| q.id != skip_id && q.state != UploadState::Failed).filter_map(|q| q.publish_at).collect();
    let lead = settings.min_lead_hours as i64 * 3600;
    next_slots(floor - lead, &taken, settings, 1, utc_offset_secs)[0]
}

/// For every dipping video released 1-5 days ago that belongs to a series (found through the
/// queue by `video_id`): pull the next episode of that series forward to the next free slot
/// (never earlier than `now + min_lead_hours`, never before an earlier chapter's release).
/// Returns (item id, old time, new time) for each move; sets `rescheduled_from`.
pub fn auto_reschedule(
    now: i64,
    videos: &[Video],
    daily: &HashMap<String, Vec<DayStat>>,
    queue: &mut [UploadItem],
    settings: &Settings,
    utc_offset_secs: i32,
) -> Vec<(String, Option<i64>, i64)> {
    let mut moves = Vec::new();
    for (_, v, _) in dipping(now, videos, daily, settings) {
        let Some((manga, chapter)) = queue.iter().find(|q| q.video_id.as_deref() == Some(v.id.as_str())).and_then(|q| Some((q.manga_id.clone()?, q.chapter))) else { continue };
        let Some((id, current)) = next_in_series(queue, &manga, chapter).map(|n| (n.id.clone(), n.publish_at)) else { continue };
        let floor = (now + settings.min_lead_hours as i64 * 3600).max(v.published_at + 3600);
        let at = earliest_slot(floor, queue, &id, settings, utc_offset_secs);
        if current.is_none_or(|c| at < c) {
            if let Some(item) = queue.iter_mut().find(|q| q.id == id) {
                if current.is_some() {
                    item.rescheduled_from = current;
                }
                item.publish_at = Some(at);
                moves.push((id, current, at));
            }
        }
    }
    moves
}

/// "Solo Leveling Ch. 3" for a queue item.
fn item_name(i: &UploadItem) -> String {
    let base = i.manga_title.clone().filter(|t| !t.is_empty());
    match (base, i.chapter) {
        (Some(m), Some(c)) => format!("{m} Ch. {}", num(c)),
        (Some(m), None) => format!("{m}: {}", i.title),
        (None, _) => i.title.clone(),
    }
}

/// Insight cards, most severe first:
/// - a video 1-5 days old is dipping → "publish the next episode sooner" (Action::PublishSooner)
///   when a next episode is queued, else "queue the next chapter" (Action::QueueNext) when one is
///   finished locally, plus "refresh title/description" (Action::RefreshMetadata);
/// - a queued episode would go out before an earlier chapter → continuity warning;
/// - nothing is scheduled in the next `cadence_hours * 2` → "keep the rhythm" with next_up;
/// - a video is rising → "it is working: keep this title style".
pub fn suggestions(
    now: i64,
    videos: &[Video],
    daily: &HashMap<String, Vec<DayStat>>,
    queue: &[UploadItem],
    episodes: &[LocalEpisode],
    settings: &Settings,
    utc_offset_secs: i32,
) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();

    for (drop, v, len) in dipping(now, videos, daily, settings) {
        let src = queue.iter().find(|q| q.video_id.as_deref() == Some(v.id.as_str()));
        let name = src.map(item_name).unwrap_or_else(|| v.title.clone());
        let pct = (drop * 100.0).round() as i32;
        let lead = format!("Views fell {pct}% from its best day on day {}.", len.saturating_sub(1));
        let manga = src.and_then(|s| s.manga_id.clone());
        let chapter = src.and_then(|s| s.chapter);
        let mut body = lead.clone();
        let mut action = None;
        if let Some(manga) = manga.as_deref() {
            if let Some(next) = next_in_series(queue, manga, chapter) {
                let floor = (now + settings.min_lead_hours as i64 * 3600).max(v.published_at + 3600);
                let at = earliest_slot(floor, queue, &next.id, settings, utc_offset_secs);
                if next.publish_at.is_none_or(|c| at < c) {
                    body = format!("{lead} Release {} sooner to keep viewers in the story — the next free slot is {}.", next.episode_label(), fmt_local(at, utc_offset_secs));
                    action = Some(Action::PublishSooner { item_id: next.id.clone(), at });
                }
            } else if let Some(ep) = next_up(episodes, queue).into_iter().find(|e| e.manga_id == manga) {
                body = format!("{lead} {} is finished but not queued: queue it to keep viewers in the story.", ep.label);
                action = Some(Action::QueueNext { chapter_id: ep.chapter_id.clone() });
            }
        }
        out.push(Suggestion {
            severity: if drop >= 0.6 { Severity::Urgent } else { Severity::Warning },
            title: format!("“{name}” is losing views"),
            body,
            video_id: Some(v.id.clone()),
            action,
        });
        out.push(Suggestion {
            severity: Severity::Warning,
            title: format!("Refresh the title of “{name}”"),
            body: "A sharper title, description and tags can win back clicks. The AI can rewrite them for you.".into(),
            video_id: Some(v.id.clone()),
            action: Some(Action::RefreshMetadata { video_id: v.id.clone() }),
        });
    }

    // Continuity: a queued episode that would go out before an earlier chapter.
    for q in queue.iter().filter(|q| movable(q)) {
        let (Some(manga), Some(ch)) = (q.manga_id.as_deref(), q.chapter) else { continue };
        let missing = missing_before(manga, ch, episodes, queue);
        if let Some(&first) = missing.first() {
            let ep = episodes.iter().find(|e| e.manga_id == manga && e.chapter.is_some_and(|c| same_chapter(c, first)));
            out.push(Suggestion {
                severity: Severity::Warning,
                title: format!("“{}” would go out before Chapter {}", item_name(q), num(first)),
                body: format!("Chapter {} is finished but not queued. Queue it first so viewers can follow the story in order.", num(first)),
                video_id: q.video_id.clone(),
                action: ep.map(|e| Action::QueueNext { chapter_id: e.chapter_id.clone() }),
            });
            continue;
        }
        let earlier = queue.iter().find(|o| {
            o.id != q.id && o.state != UploadState::Failed && o.manga_id.as_deref() == Some(manga) && o.chapter.is_some_and(|c| c < ch - EPS) && matches!((o.publish_at, q.publish_at), (Some(a), Some(b)) if b < a)
        });
        if let Some(o) = earlier {
            out.push(Suggestion {
                severity: Severity::Warning,
                title: format!("“{}” is set to go out before {}", item_name(q), o.episode_label()),
                body: "Chapters should be released in order. Re-plan the series so the earlier chapter goes first.".into(),
                video_id: q.video_id.clone(),
                action: None,
            });
        }
    }

    // Rhythm: nothing coming up.
    let horizon = now + settings.cadence_hours.max(1) as i64 * 2 * 3600;
    let upcoming = queue.iter().any(|q| movable(q) && q.publish_at.is_none_or(|t| t > now && t <= horizon));
    if !upcoming {
        if let Some(ep) = next_up(episodes, queue).first() {
            let taken: Vec<i64> = queue.iter().filter(|q| q.state != UploadState::Failed).filter_map(|q| q.publish_at).collect();
            let slot = next_slots(now, &taken, settings, 1, utc_offset_secs)[0];
            out.push(Suggestion {
                severity: Severity::Info,
                title: "Keep the rhythm going".into(),
                body: format!("Nothing is scheduled for the next {} hours. {} of {} is ready: queue it for {}.", settings.cadence_hours.max(1) * 2, ep.label, ep.manga_title, fmt_local(slot, utc_offset_secs)),
                video_id: None,
                action: Some(Action::QueueNext { chapter_id: ep.chapter_id.clone() }),
            });
        }
    }

    // Rising videos.
    for v in videos.iter().filter(|v| v.privacy != "private" && (1..=7).contains(&age_days(now, v.published_at))) {
        if trend(&views_of(daily, &v.id), settings.dip_threshold) == Trend::Rising {
            out.push(Suggestion {
                severity: Severity::Info,
                title: format!("“{}” is picking up", v.title),
                body: "Its latest full day was its best so far. This title style is working: keep it for the next episodes.".into(),
                video_id: Some(v.id.clone()),
                action: None,
            });
        }
    }

    out.sort_by_key(|a| std::cmp::Reverse(a.severity));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, 0, 0).unwrap().and_utc().timestamp()
    }

    fn ep(manga: &str, ch: Option<f64>) -> LocalEpisode {
        LocalEpisode {
            chapter_id: format!("{manga}-{}", ch.map(num).unwrap_or_else(|| "x".into())),
            manga_id: manga.into(),
            manga_title: "Solo Leveling".into(),
            chapter: ch,
            label: format!("Chapter {}", ch.map(num).unwrap_or_default()),
            ..Default::default()
        }
    }

    fn item(id: &str, manga: &str, ch: f64, state: UploadState, at: Option<i64>) -> UploadItem {
        UploadItem { id: id.into(), manga_id: Some(manga.into()), manga_title: Some("Solo Leveling".into()), chapter: Some(ch), title: format!("t{ch}"), state, publish_at: at, ..Default::default() }
    }

    fn day(views: u64) -> DayStat {
        DayStat { views, ..Default::default() }
    }

    // Monday 2025-01-06 12:00 UTC
    fn now() -> i64 {
        ts(2025, 1, 6, 12)
    }

    #[test]
    fn missing_before_finds_unqueued_earlier_chapters() {
        let eps = vec![ep("m", Some(1.0)), ep("m", Some(2.0)), ep("m", Some(3.0)), ep("m", Some(4.0)), ep("o", Some(1.0))];
        let q = vec![item("a", "m", 1.0, UploadState::Published, None), item("b", "m", 3.0, UploadState::Failed, None)];
        assert_eq!(missing_before("m", 4.0, &eps, &q), vec![2.0, 3.0]);
        assert!(missing_before("m", 1.0, &eps, &q).is_empty());
        assert_eq!(missing_before("m", 2.5, &eps, &q), vec![2.0]);
    }

    #[test]
    fn next_up_is_lowest_unqueued_per_manga() {
        let eps = vec![ep("m", Some(3.0)), ep("m", Some(2.0)), ep("m", Some(1.0)), ep("o", None), ep("o", Some(7.0))];
        let q = vec![item("a", "m", 1.0, UploadState::Scheduled, None)];
        let n = next_up(&eps, &q);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].chapter, Some(2.0));
        assert_eq!(n[1].chapter, Some(7.0));
    }

    #[test]
    fn series_order_sorts_numbered_first() {
        let mut v = vec![ep("m", None), ep("m", Some(10.0)), ep("m", Some(2.5)), ep("m", Some(2.0))];
        series_order(&mut v);
        assert_eq!(v.iter().map(|e| e.chapter).collect::<Vec<_>>(), vec![Some(2.0), Some(2.5), Some(10.0), None]);
    }

    #[test]
    fn slots_respect_local_hour_and_taken() {
        let s = Settings::default();
        // 21:00 local at +9h: earliest 23:00 local, so the first 18:00 local is tomorrow = 09:00 UTC.
        let slots = next_slots(now(), &[], &s, 3, 9 * 3600);
        assert_eq!(slots, vec![ts(2025, 1, 7, 9), ts(2025, 1, 8, 9), ts(2025, 1, 9, 9)]);
        let slots = next_slots(now(), &[ts(2025, 1, 7, 9)], &s, 2, 9 * 3600);
        assert_eq!(slots, vec![ts(2025, 1, 8, 9), ts(2025, 1, 9, 9)]);
        // Same day when there is time: 12:00 UTC, 18:00 local UTC.
        assert_eq!(next_slots(now(), &[], &s, 1, 0), vec![ts(2025, 1, 6, 18)]);
        // Min lead pushes past today's slot.
        let s2 = Settings { min_lead_hours: 8, ..Settings::default() };
        assert_eq!(next_slots(now(), &[], &s2, 1, 0), vec![ts(2025, 1, 7, 18)]);
        // Negative offsets.
        assert_eq!(next_slots(now(), &[], &s, 1, -5 * 3600), vec![ts(2025, 1, 6, 23)]);
    }

    #[test]
    fn trend_detects_dip_and_rise() {
        assert_eq!(trend(&[100], 0.4), Trend::TooNew);
        assert_eq!(trend(&[100, 80], 0.4), Trend::TooNew);
        match trend(&[100, 60, 30, 5], 0.4) {
            Trend::Dipping { drop } => assert!((drop - 0.7).abs() < 1e-3),
            t => panic!("{t:?}"),
        }
        assert_eq!(trend(&[10, 20, 40, 3], 0.4), Trend::Rising);
        assert_eq!(trend(&[100, 90, 95, 1], 0.4), Trend::Steady);
        assert_eq!(trend(&[0, 0, 0], 0.4), Trend::Steady);
    }

    #[test]
    fn age() {
        assert_eq!(age_days(now(), now() - 3 * DAY - 5), 3);
        assert_eq!(age_days(now(), now() - 100), 0);
    }

    #[test]
    fn order_series_moves_later_chapter_after_earlier() {
        let s = Settings::default();
        let mut q = vec![item("c2", "m", 2.0, UploadState::Planned, Some(ts(2025, 1, 7, 18))), item("c1", "m", 1.0, UploadState::Scheduled, Some(ts(2025, 1, 9, 18))), item("c3", "m", 3.0, UploadState::Planned, None)];
        let changed = order_series(&mut q, now(), &s, 0);
        assert_eq!(changed, vec!["c2".to_string(), "c3".to_string()]);
        assert_eq!(q[0].publish_at, Some(ts(2025, 1, 10, 18)));
        assert_eq!(q[0].rescheduled_from, Some(ts(2025, 1, 7, 18)));
        assert_eq!(q[2].publish_at, Some(ts(2025, 1, 11, 18)));
        assert_eq!(q[1].publish_at, Some(ts(2025, 1, 9, 18)));
        // Already in order: nothing changes.
        assert!(order_series(&mut q, now(), &s, 0).is_empty());
    }

    #[test]
    fn next_in_series_picks_earliest_queued_after() {
        let q = vec![item("c4", "m", 4.0, UploadState::Planned, None), item("c3", "m", 3.0, UploadState::Scheduled, None), item("c5", "m", 5.0, UploadState::Failed, None), item("c2", "m", 2.0, UploadState::Published, None)];
        assert_eq!(next_in_series(&q, "m", Some(2.0)).unwrap().id, "c3");
        assert_eq!(next_in_series(&q, "m", Some(3.0)).unwrap().id, "c4");
        assert!(next_in_series(&q, "m", Some(4.0)).is_none());
        assert!(next_in_series(&q, "other", None).is_none());
    }

    fn dip_fixture() -> (Vec<Video>, HashMap<String, Vec<DayStat>>, Vec<UploadItem>) {
        let v = Video { id: "v1".into(), title: "Solo Leveling Ch. 3".into(), privacy: "public".into(), published_at: now() - 3 * DAY, ..Default::default() };
        let mut daily = HashMap::new();
        daily.insert("v1".to_string(), vec![day(100), day(60), day(30), day(5)]);
        let mut c3 = item("c3", "m", 3.0, UploadState::Published, Some(v.published_at));
        c3.video_id = Some("v1".into());
        let c4 = item("c4", "m", 4.0, UploadState::Planned, Some(ts(2025, 1, 12, 18)));
        (vec![v], daily, vec![c3, c4])
    }

    #[test]
    fn auto_reschedule_pulls_next_chapter_forward() {
        let (videos, daily, mut q) = dip_fixture();
        let moves = auto_reschedule(now(), &videos, &daily, &mut q, &Settings::default(), 0);
        assert_eq!(moves, vec![("c4".to_string(), Some(ts(2025, 1, 12, 18)), ts(2025, 1, 6, 18))]);
        assert_eq!(q[1].publish_at, Some(ts(2025, 1, 6, 18)));
        assert_eq!(q[1].rescheduled_from, Some(ts(2025, 1, 12, 18)));
        // Already earlier: no second move.
        assert!(auto_reschedule(now(), &videos, &daily, &mut q, &Settings::default(), 0).is_empty());
    }

    #[test]
    fn auto_reschedule_ignores_healthy_and_old_videos() {
        let (mut videos, mut daily, mut q) = dip_fixture();
        daily.insert("v1".into(), vec![day(100), day(110), day(120), day(5)]);
        assert!(auto_reschedule(now(), &videos, &daily, &mut q, &Settings::default(), 0).is_empty());
        daily.insert("v1".into(), vec![day(100), day(60), day(30), day(5)]);
        videos[0].published_at = now() - 9 * DAY;
        assert!(auto_reschedule(now(), &videos, &daily, &mut q, &Settings::default(), 0).is_empty());
    }

    #[test]
    fn suggestions_offer_publish_sooner_for_a_dip() {
        let (videos, daily, q) = dip_fixture();
        let s = suggestions(now(), &videos, &daily, &q, &[], &Settings::default(), 0);
        let first = &s[0];
        assert_eq!(first.severity, Severity::Urgent);
        assert_eq!(first.title, "“Solo Leveling Ch. 3” is losing views");
        assert!(first.body.starts_with("Views fell 70% from its best day on day 3."), "{}", first.body);
        assert!(first.body.contains("Chapter 4") && first.body.contains("Mon 18:00"), "{}", first.body);
        assert_eq!(first.action, Some(Action::PublishSooner { item_id: "c4".into(), at: ts(2025, 1, 6, 18) }));
        assert!(s.iter().any(|x| x.action == Some(Action::RefreshMetadata { video_id: "v1".into() })));
    }

    #[test]
    fn suggestions_queue_next_when_nothing_is_queued() {
        let (videos, daily, mut q) = dip_fixture();
        q.pop();
        let eps = vec![ep("m", Some(3.0)), ep("m", Some(4.0))];
        let s = suggestions(now(), &videos, &daily, &q, &eps, &Settings::default(), 0);
        assert_eq!(s[0].action, Some(Action::QueueNext { chapter_id: "m-4".into() }));
    }

    #[test]
    fn suggestions_warn_about_continuity_and_rhythm() {
        let eps = vec![ep("m", Some(1.0)), ep("m", Some(2.0))];
        let q = vec![item("c2", "m", 2.0, UploadState::Planned, Some(ts(2025, 1, 20, 18)))];
        let s = suggestions(now(), &[], &HashMap::new(), &q, &eps, &Settings::default(), 0);
        assert!(s.iter().any(|x| x.title.contains("before Chapter 1") && x.action == Some(Action::QueueNext { chapter_id: "m-1".into() })));
        let rhythm = s.iter().find(|x| x.title == "Keep the rhythm going").unwrap();
        assert_eq!(rhythm.action, Some(Action::QueueNext { chapter_id: "m-1".into() }));
        assert!(rhythm.body.contains("Mon 18:00"), "{}", rhythm.body);
        assert!(s.windows(2).all(|w| w[0].severity >= w[1].severity));
    }

    #[test]
    fn suggestions_celebrate_a_rising_video() {
        let (mut videos, mut daily, q) = dip_fixture();
        daily.insert("v1".into(), vec![day(10), day(20), day(40), day(3)]);
        videos[0].published_at = now() - 3 * DAY;
        let s = suggestions(now(), &videos, &daily, &q, &[], &Settings::default(), 0);
        assert!(s.iter().any(|x| x.title.contains("is picking up") && x.severity == Severity::Info));
    }
}
