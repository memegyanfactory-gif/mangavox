//! The whole niche at a glance: what the Competitors section shows before a channel is picked.
//! Every number is measured against the video's own channel (its usual views), so a small channel's
//! hit counts as much as a big channel's ordinary upload. Pure functions over the stored data.
use std::collections::{HashMap, HashSet};

use super::analysis::{points_gain, top_counts, STOP};
use super::track::DAY;
use super::{Change, CompChannel, CompStore, CompVideo};
use crate::timing::local_day_hour;
use crate::trends::median;

/// A video counts once it has been up this long (its views have settled enough to compare).
const SETTLED: i64 = 3 * DAY;
/// A channel needs this many settled videos before its "usual" means anything.
const MIN_FOR_USUAL: usize = 5;
/// A winner did at least this many times its channel's usual.
const WINNER: f64 = 2.0;
/// Shorts and clips are not recaps.
const MIN_SECS: u64 = 180;
/// The best-videos list takes this many from one channel, so one series in parts does not fill it.
const TOP_PER_CHANNEL: usize = 2;

/// Words that are in every kind of title: they say nothing about why one works.
const FILLER: &[&str] = &[
    "then", "before", "after", "they", "them", "their", "been", "were", "just", "even", "only", "every", "about", "because", "while", "until", "over", "also", "than", "there", "here", "where", "which", "would", "could",
    "should", "becomes", "become", "became", "gets", "make", "made", "makes", "being", "more", "most", "some", "such", "very", "hindi", "explain", "summary", "resumen", "completo", "completa", "anime", "novel", "english",
];

/// Small words of the languages YouTube translates titles into (Spanish, Portuguese, French,
/// Indonesian, German), none of them an English word.
const FOREIGN: &[&str] = &[
    "el", "los", "las", "una", "del", "que", "su", "con", "por", "para", "pero", "más", "um", "uma", "os", "da", "não", "com", "ele", "ela", "mas", "le", "les", "des", "du", "et", "est", "avec", "pour", "dans", "qui", "yang",
    "dan", "di", "ini", "itu", "kembali", "semua", "menjadi", "dengan", "untuk", "dari", "tapi", "setelah", "karena", "der", "die", "das", "und", "mit", "ist",
];

/// Is the title in another language than English: an accented letter, or two of [`FOREIGN`]'s words.
fn foreign(title: &str) -> bool {
    let t = title.to_lowercase();
    t.chars().any(|c| "áàâãäéèêëíìîïóòôõöúùûüçñß¿¡".contains(c)) || t.split(|c: char| !c.is_alphanumeric()).filter(|w| FOREIGN.contains(w)).count() >= 2
}

/// YouTube sometimes serves a title in the viewer's language and sometimes in the channel's own, so
/// the same title is stored in two languages: that is not the channel changing it. Neither is a
/// change of spacing, case or punctuation only.
pub fn is_rewrite(before: &str, after: &str) -> bool {
    crate::trends::normalize(before) != crate::trends::normalize(after) && foreign(before) == foreign(after)
}

/// The times the channel really rewrote the title: (the title before, the one it became).
pub fn title_rewrites(v: &CompVideo) -> Vec<(&Change, &Change)> {
    v.titles.windows(2).filter(|w| is_rewrite(&w[0].value, &w[1].value)).map(|w| (&w[0], &w[1])).collect()
}

/// One of the best recent videos.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopVideo {
    pub video: CompVideo,
    /// Its channel's name.
    pub channel: String,
    pub views_per_day: f64,
    /// Views against its channel's usual (2.0 = twice); `None` when the channel has too few videos.
    pub lift: Option<f64>,
}

/// How videos of one length do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LengthBand {
    pub label: &'static str,
    pub videos: usize,
    /// The middle video of the band against its channel's usual (1.0 = as usual).
    pub lift: f64,
    /// Share of the band's videos that did at least twice their channel's usual, 0..1.
    pub hit_rate: f64,
    pub median_views: u64,
}

/// A word that winning titles use more than the rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitleWord {
    pub word: String,
    /// Winning titles with it.
    pub winners: u32,
    /// How much more often winners use it than all titles (2.0 = twice as often).
    pub lift: f64,
}

/// A title somebody changed after uploading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitleChange {
    pub video_id: String,
    pub channel_id: String,
    pub channel: String,
    pub at: i64,
    pub before: String,
    pub after: String,
}

/// A channel that gained subscribers this week.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mover {
    pub channel_id: String,
    pub title: String,
    pub avatar: String,
    pub subscribers: u64,
    pub gained: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub channels: usize,
    /// Videos stored, and how many of them have had their own page read (exact time, tags).
    pub videos: usize,
    pub read_in_full: usize,
    /// Videos the comparisons below are made from.
    pub compared: usize,
    pub winners: usize,
    pub top: Vec<TopVideo>,
    pub lengths: Vec<LengthBand>,
    /// Uploads per local hour and weekday (Monday = 0), exact publish times only.
    pub upload_hours: [u32; 24],
    pub upload_days: [u32; 7],
    pub timed: usize,
    pub words: Vec<TitleWord>,
    pub tags: Vec<(String, u32)>,
    pub changes: Vec<TitleChange>,
    pub movers: Vec<Mover>,
    /// Title length in characters: the winners' middle, and everyone's.
    pub winner_title_chars: usize,
    pub title_chars: usize,
}

const BANDS: [(&str, u64); 6] = [("Under 20 min", 20 * 60), ("20–60 min", 3600), ("1–2 hours", 2 * 3600), ("2–4 hours", 4 * 3600), ("4–8 hours", 8 * 3600), ("Over 8 hours", u64::MAX)];

fn title_words(title: &str) -> HashSet<String> {
    title.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4 && !STOP.contains(w) && !FILLER.contains(w) && !w.chars().all(|c| c.is_ascii_digit())).map(str::to_string).collect()
}

fn middle(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Reads every visible channel's videos and builds the [`Overview`].
pub fn overview(store: &CompStore, now: i64, offset: i32) -> Overview {
    let channels: Vec<CompChannel> = store.channels().into_iter().filter(|c| !c.hidden).collect();
    let videos: HashMap<String, Vec<CompVideo>> = channels.iter().map(|c| (c.id.clone(), store.videos(&c.id))).collect();
    build_overview(&channels, &videos, now, offset)
}

/// [`overview`] over data already loaded (`videos` by channel id).
pub fn build_overview(channels: &[CompChannel], videos: &HashMap<String, Vec<CompVideo>>, now: i64, offset: i32) -> Overview {
    let mut o = Overview { channels: channels.len(), ..Default::default() };
    let recap = |v: &CompVideo| v.duration_secs == 0 || v.duration_secs > MIN_SECS;
    let settled = |v: &CompVideo| v.views > 0 && v.published_at > 0 && now - v.published_at >= SETTLED && recap(v);

    // Each channel's usual views (the middle of its settled videos).
    let usual: HashMap<&str, f64> = channels
        .iter()
        .filter_map(|c| {
            let views: Vec<u64> = videos.get(&c.id)?.iter().filter(|v| settled(v)).map(|v| v.views).collect();
            (views.len() >= MIN_FOR_USUAL).then(|| (c.id.as_str(), median(views).max(1) as f64))
        })
        .collect();
    let lift = |v: &CompVideo| usual.get(v.channel_id.as_str()).map(|u| v.views as f64 / u);

    let mut top: Vec<TopVideo> = vec![];
    let mut bands: Vec<(Vec<f64>, Vec<u64>)> = vec![Default::default(); BANDS.len()];
    let (mut win_words, mut all_words): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let (mut win_len, mut all_len): (Vec<u64>, Vec<u64>) = (vec![], vec![]);
    let mut tags: Vec<String> = vec![];
    for c in channels {
        for v in videos.get(&c.id).into_iter().flatten() {
            o.videos += 1;
            o.read_in_full += v.published_exact as usize;
            if v.published_exact {
                let (d, h) = local_day_hour(v.published_at, offset);
                o.upload_days[d] += 1;
                o.upload_hours[h] += 1;
                o.timed += 1;
            }
            tags.extend(v.tags.iter().map(|t| t.trim().to_lowercase()));
            if let Some((before, after)) = title_rewrites(v).pop().filter(|(_, after)| now - after.at <= 30 * DAY) {
                o.changes.push(TitleChange { video_id: v.id.clone(), channel_id: c.id.clone(), channel: c.title.clone(), at: after.at, before: before.value.clone(), after: after.value.clone() });
            }
            if v.views > 0 && v.published_at > 0 && v.published_at <= now && now - v.published_at <= 30 * DAY && recap(v) {
                let days = ((now - v.published_at) as f64 / DAY as f64).max(0.5);
                top.push(TopVideo { video: v.clone(), channel: c.title.clone(), views_per_day: v.views as f64 / days, lift: lift(v) });
            }
            let Some(l) = lift(v).filter(|_| settled(v)) else { continue };
            o.compared += 1;
            let words = title_words(&v.title);
            let chars = v.title.chars().count() as u64;
            all_len.push(chars);
            if l >= WINNER {
                o.winners += 1;
                win_len.push(chars);
                win_words.extend(words.iter().cloned());
            }
            all_words.extend(words);
            if v.duration_secs > 0 {
                let k = BANDS.iter().position(|(_, max)| v.duration_secs < *max).unwrap_or(BANDS.len() - 1);
                bands[k].0.push(l);
                bands[k].1.push(v.views);
            }
        }
        let gained = points_gain(&c.subs_history, now, 7 * DAY);
        if c.subs_history.len() > 1 && gained > 0 {
            o.movers.push(Mover { channel_id: c.id.clone(), title: c.title.clone(), avatar: c.avatar.clone(), subscribers: c.subscribers, gained });
        }
    }

    top.sort_by(|a, b| b.views_per_day.total_cmp(&a.views_per_day).then(a.video.id.cmp(&b.video.id)));
    let mut taken: HashMap<String, usize> = HashMap::new();
    top.retain(|t| {
        let n = taken.entry(t.video.channel_id.clone()).or_default();
        *n += 1;
        *n <= TOP_PER_CHANNEL
    });
    top.truncate(12);
    o.top = top;
    o.lengths = BANDS.iter().zip(bands).filter(|(_, (l, _))| !l.is_empty()).map(|((label, _), (l, views))| LengthBand { label, videos: l.len(), hit_rate: l.iter().filter(|x| **x >= WINNER).count() as f64 / l.len() as f64, lift: middle(l), median_views: median(views) }).collect();
    o.winner_title_chars = median(win_len) as usize;
    o.title_chars = median(all_len) as usize;

    // Words the winners' titles use more often than titles in general.
    if o.winners >= 5 && o.compared > o.winners {
        let all: HashMap<String, u32> = top_counts(all_words.into_iter(), usize::MAX, 1).into_iter().collect();
        let mut words: Vec<TitleWord> = top_counts(win_words.into_iter(), usize::MAX, 3)
            .into_iter()
            .filter_map(|(word, winners)| {
                let everywhere = *all.get(&word)? as f64 / o.compared as f64;
                let lift = (winners as f64 / o.winners as f64) / everywhere;
                (lift >= 1.25).then_some(TitleWord { word, winners, lift })
            })
            .collect();
        words.sort_by(|a, b| b.winners.cmp(&a.winners).then(b.lift.total_cmp(&a.lift)).then(a.word.cmp(&b.word)));
        words.truncate(18);
        o.words = words;
    }
    o.tags = top_counts(tags.into_iter(), 18, 3);
    o.changes.sort_by(|a, b| b.at.cmp(&a.at).then(a.video_id.cmp(&b.video_id)));
    o.changes.truncate(8);
    o.movers.sort_by(|a, b| b.gained.cmp(&a.gained).then(a.channel_id.cmp(&b.channel_id)));
    o.movers.truncate(6);
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::competitors::Point;

    fn video(id: &str, ch: &str, title: &str, age_days: i64, views: u64, mins: u64, now: i64) -> CompVideo {
        CompVideo { id: id.into(), channel_id: ch.into(), title: title.into(), published_at: now - age_days * DAY, published_exact: true, views, duration_secs: mins * 60, ..Default::default() }
    }

    #[test]
    fn the_niche_at_a_glance() {
        let now = 1000 * DAY;
        let ch = |id: &str, subs: &[(i64, u64)]| CompChannel { id: id.into(), title: format!("Channel {id}"), subscribers: subs.last().map(|p| p.1).unwrap_or(0), subs_history: subs.iter().map(|&(at, value)| Point { at, value }).collect(), ..Default::default() };
        let channels = [ch("big", &[(now - 8 * DAY, 100_000), (now, 107_000)]), ch("small", &[(now, 900)]), CompChannel { hidden: false, ..ch("new", &[]) }];
        let mut videos: HashMap<String, Vec<CompVideo>> = HashMap::new();
        // The big channel usually gets 100K; its "betrayed" videos get far more, and they are long.
        let mut big: Vec<CompVideo> = (0..10).map(|k| video(&format!("b{k}"), "big", &format!("Ordinary dungeon story number {k}"), 10 + k, 100_000, 30, now)).collect();
        for k in 0..5 {
            big.push(video(&format!("w{k}"), "big", &format!("Betrayed hero returns stronger {k}"), 20 + k, 400_000, 180, now));
        }
        big[0].titles = vec![Change { at: now - 9 * DAY, value: "Old title".into() }, Change { at: now - 2 * DAY, value: big[0].title.clone() }];
        big[1].tags = vec!["Manhwa Recap".into()];
        big[2].tags = vec!["manhwa recap".into()];
        big[3].tags = vec!["manhwa recap ".into()];
        videos.insert("big".into(), big);
        // The small channel's hit: far above its usual, and fresh.
        let mut small: Vec<CompVideo> = (0..6).map(|k| video(&format!("s{k}"), "small", "Quiet village tale", 40 + k, 1_000, 30, now)).collect();
        small.push(video("hit", "small", "Betrayed mage wins", 1, 300_000, 30, now));
        small.push(video("short", "small", "A short", 1, 9_000_000, 1, now));
        videos.insert("small".into(), small);
        // Too few videos to have a usual.
        videos.insert("new".into(), vec![video("n0", "new", "First upload", 5, 50_000_000, 30, now)]);

        let o = build_overview(&channels, &videos, now, 0);
        assert_eq!((o.channels, o.videos, o.read_in_full), (3, 24, 24));
        // Compared: the settled videos of channels with a usual (the fresh hit and the short are not).
        assert_eq!((o.compared, o.winners), (21, 5));
        // Best recent: by views a day; shorts are left out, the fresh small hit is there with its lift.
        assert!(o.top.iter().all(|t| t.video.id != "short"));
        let hit = o.top.iter().find(|t| t.video.id == "hit").unwrap();
        assert!((hit.lift.unwrap() - 300.0).abs() < 1e-9 && (hit.views_per_day - 300_000.0).abs() < 1e-9);
        assert_eq!((o.top[0].video.id.as_str(), o.top[0].lift), ("n0", None));
        // Two a channel at most: the big channel's fifteen do not crowd the others out.
        assert_eq!(o.top.len(), 4);
        assert_eq!(o.top.iter().filter(|t| t.video.channel_id == "big").count(), 2);
        assert!(o.top.windows(2).all(|w| w[0].views_per_day >= w[1].views_per_day));
        // Length: the three-hour videos do four times the usual, the half-hour ones as usual.
        let band = |l: &str| o.lengths.iter().find(|b| b.label == l).unwrap();
        assert_eq!((band("20–60 min").videos, band("2–4 hours").videos), (16, 5));
        assert!((band("20–60 min").lift - 1.0).abs() < 1e-9 && (band("2–4 hours").lift - 4.0).abs() < 1e-9);
        assert_eq!((band("20–60 min").hit_rate, band("2–4 hours").hit_rate), (0.0, 1.0));
        // "betrayed" is in every winner and few others; "dungeon" only in ordinary titles.
        assert_eq!(o.words.first().map(|w| (w.word.as_str(), w.winners)), Some(("betrayed", 5)));
        assert!(o.words.iter().all(|w| w.word != "dungeon" && w.lift >= 1.25));
        assert_eq!(o.tags, vec![("manhwa recap".to_string(), 3)]);
        assert_eq!(o.changes.len(), 1);
        assert_eq!((o.changes[0].before.as_str(), o.changes[0].channel.as_str()), ("Old title", "Channel big"));
        // Subscribers gained over 7 days, scaled from the 8 days between the two readings.
        assert_eq!(o.movers.len(), 1);
        assert_eq!((o.movers[0].channel_id.as_str(), o.movers[0].gained), ("big", 6125));
        assert_eq!(o.timed, 24);
        assert_eq!(o.upload_hours.iter().sum::<u32>(), 24);
        // A title stored in two languages, or respaced, is not a rewrite; and coming back is not either.
        assert!(!is_rewrite("A GENIUS Unlocks INFINITE LUCK and the #1 Hunter", "Um GÊNIO Desbloqueia SORTE INFINITA e o Caçador #1"));
        assert!(!is_rewrite("HIS CRAZY BOSS FALLS FOR HIM", "SU JEFE LOCO SE ENAMORA DE EL Y ESCAPA CON LOS DOS"));
        assert!(!is_rewrite("He  Returned!", "he returned"));
        assert!(is_rewrite("[1-2] HE Camps ALONE in Another World", "HE Camps ALONE in Another World"));
        let mut flip = video("f", "big", "He Returned Stronger", 9, 10, 30, now);
        flip.titles = ["He Returned Stronger", "Il est revenu plus fort avec des pouvoirs", "He Returned Stronger", "He Came Back a God"].iter().enumerate().map(|(k, t)| Change { at: k as i64, value: t.to_string() }).collect();
        let real = title_rewrites(&flip);
        assert_eq!(real.len(), 1);
        assert_eq!((real[0].0.value.as_str(), real[0].1.value.as_str()), ("He Returned Stronger", "He Came Back a God"));
        // Nothing tracked: empty, no panic.
        assert_eq!(build_overview(&[], &HashMap::new(), now, 0), Overview::default());
    }
}
