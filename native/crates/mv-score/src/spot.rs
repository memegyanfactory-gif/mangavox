//! Spotting: where each music cue starts and how it feels, the way a film composer marks up a cut.
//! Pages are classified into [`Style`]s from the Read phase's scene kind / mood / intensity (or the
//! optional AI directions), grouped into cues of sensible length, and given hits, drops and risers.

use crate::model::*;
use crate::synth::Hit;

/// What the Read phase knows about one page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageInfo {
    pub page: usize,
    pub kind: String,
    pub mood: String,
    pub intensity: u8,
    pub tags: Vec<String>,
    /// Sound-effect lettering on the page, with where it sits in the page's narration (0..1).
    pub sfx: Vec<(f32, String)>,
}

/// One page's narration on the timeline (seconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub page: usize,
    pub start: f64,
    pub end: f64,
}

/// A cue never gets shorter than this unless the story turns hard (a reveal or a fight starting).
pub const MIN_CUE: f64 = 20.0;
/// Long stretches in one style get a fresh variation this often.
pub const MAX_CUE: f64 = 90.0;
/// Reveals (drop + hit) at least this far apart; closer ones join the cue they are in.
pub const REVEAL_GAP: f64 = 60.0;
/// Impacts at least this far apart.
pub const HIT_GAP: f64 = 30.0;

/// 0 intense (action, epic, tense, reveal), 1 soft, 2 comic.
fn family(s: Style) -> u8 {
    match s {
        Style::Action | Style::Epic | Style::Tense | Style::Reveal => 0,
        Style::Comedy => 2,
        _ => 1,
    }
}

fn has(words: &str, any: &[&str]) -> bool {
    any.iter().any(|w| words.contains(w))
}

/// The style a page asks for.
pub fn classify(p: &PageInfo) -> Style {
    let kind = p.kind.to_lowercase();
    let words = format!("{} {}", p.mood.to_lowercase(), p.tags.join(" ").to_lowercase());
    let i = p.intensity;
    match kind.as_str() {
        "fight" | "action" | "battle" => {
            if i >= 9 {
                Style::Epic
            } else {
                Style::Action
            }
        }
        // A quiet reveal is a mystery beat, not a drop-and-hit.
        "reveal" | "twist" => {
            if i >= 5 {
                Style::Reveal
            } else {
                Style::Mystery
            }
        }
        "horror" => Style::Tense,
        "emotional" | "sad" => Style::Emotional,
        "romance" => Style::Romance,
        "comedy" => Style::Comedy,
        "flashback" | "memory" => Style::Flashback,
        _ => {
            if has(&words, &["ominous", "tense", "dread", "menac", "creep", "scary", "fear", "danger", "suspense"]) {
                Style::Tense
            } else if has(&words, &["mysteri", "strange", "curious", "secret", "eerie"]) {
                Style::Mystery
            } else if has(&words, &["sad", "grief", "tear", "melanchol", "loss", "lonely", "bittersweet"]) {
                Style::Emotional
            } else if has(&words, &["funny", "silly", "playful", "comic", "goofy", "light-hearted", "lighthearted"]) {
                Style::Comedy
            } else if has(&words, &["love", "tender", "romantic", "blush"]) {
                Style::Romance
            } else if has(&words, &["triumph", "heroic", "epic", "determin", "resolve", "hype"]) || i >= 8 {
                Style::Epic
            } else if i >= 6 {
                Style::Action
            } else {
                Style::Calm
            }
        }
    }
}

/// Tempo for a style at an energy (0..10).
pub fn tempo(style: Style, energy: f32) -> f32 {
    let e = (energy / 10.0).clamp(0.0, 1.0);
    let (lo, hi) = match style {
        Style::Action => (138.0, 160.0),
        Style::Epic => (88.0, 104.0),
        Style::Tense => (68.0, 88.0),
        Style::Mystery => (76.0, 88.0),
        Style::Emotional => (64.0, 76.0),
        Style::Romance => (70.0, 84.0),
        Style::Comedy => (112.0, 128.0),
        Style::Calm => (74.0, 90.0),
        Style::Flashback => (66.0, 76.0),
        Style::Reveal => (84.0, 96.0),
    };
    (lo + (hi - lo) * e).round()
}

pub fn mode(style: Style) -> &'static str {
    match style {
        Style::Action | Style::Reveal => "harmonic minor",
        Style::Epic | Style::Emotional => "minor",
        Style::Tense => "phrygian",
        Style::Mystery | Style::Flashback => "dorian",
        Style::Romance | Style::Comedy => "major",
        Style::Calm => "lydian",
    }
}

/// Bright styles sit in the relative major so the whole chapter stays in one family of keys.
fn root_for(style: Style, home: u8) -> u8 {
    match style {
        Style::Romance | Style::Comedy | Style::Calm => (home + 3) % 12,
        _ => home,
    }
}

/// Builds the cue sheet. `directions` (from the AI pass) override the per-page reading;
/// `forced` styles (user overrides, keyed by a cue's first page) win over both.
pub fn plan(pages: &[PageInfo], spans: &[Span], directions: &[Direction], settings: &MusicSettings, home_key: u8) -> (Vec<Cue>, EnergyCurve) {
    let info = |page: usize| pages.iter().find(|p| p.page == page);
    let direction = |page: usize| directions.iter().find(|d| d.page == page);
    struct Pg {
        span: Span,
        style: Style,
        energy: f32,
        hit: bool,
        drop: bool,
    }
    let pg: Vec<Pg> = spans
        .iter()
        .map(|s| {
            let p = info(s.page).cloned().unwrap_or(PageInfo { page: s.page, kind: "calm".into(), intensity: 3, ..Default::default() });
            let d = direction(s.page);
            let style = d.and_then(|d| d.style).unwrap_or_else(|| classify(&p));
            let energy = d.and_then(|d| d.energy).unwrap_or(p.intensity as f32).clamp(0.0, 10.0);
            Pg { span: *s, style, energy, hit: d.is_some_and(|d| d.hit), drop: d.is_some_and(|d| d.drop) }
        })
        .collect();

    // Group pages into cues, the way a composer holds one bed through a sequence: a change of
    // family (intense / soft / comic) needs MIN_CUE, a change within a family twice that, and only
    // a fight breaking out of a quiet stretch (or a well-spaced reveal) cuts in sooner.
    let mut groups: Vec<(Style, Vec<usize>)> = vec![];
    let mut last_reveal = f64::NEG_INFINITY;
    for (i, p) in pg.iter().enumerate() {
        let start_new = match groups.last() {
            None => true,
            Some((style, idx)) => {
                let len = p.span.start - pg[idx[0]].span.start;
                let reveal = (p.style == Style::Reveal || p.drop) && p.span.start - last_reveal >= REVEAL_GAP && len >= 6.0;
                let breaks_out = family(p.style) == 0 && family(*style) != 0 && len >= 8.0;
                let change = if family(p.style) != family(*style) { len >= MIN_CUE } else { p.style != *style && len >= 2.0 * MIN_CUE };
                reveal || breaks_out || p.hit || change || len >= MAX_CUE
            }
        };
        if start_new {
            if p.style == Style::Reveal || p.drop {
                last_reveal = p.span.start;
            }
            groups.push((p.style, vec![i]));
        } else {
            groups.last_mut().unwrap().1.push(i);
        }
    }
    // Each cue takes the style that fills most of it (a reveal keeps its own).
    for (style, idx) in groups.iter_mut() {
        if *style == Style::Reveal {
            continue;
        }
        let mut time: Vec<(Style, f64)> = vec![];
        for &i in idx.iter() {
            let d = pg[i].span.end - pg[i].span.start;
            let st = if pg[i].style == Style::Reveal { *style } else { pg[i].style };
            match time.iter_mut().find(|(s, _)| *s == st) {
                Some((_, t)) => *t += d,
                None => time.push((st, d)),
            }
        }
        if let Some((s, _)) = time.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            *style = s;
        }
    }
    // Neighbours that ended up in the same style become one cue (up to MAX_CUE).
    let mut merged: Vec<(Style, Vec<usize>)> = vec![];
    for (style, idx) in groups {
        if let Some((ps, pidx)) = merged.last_mut() {
            let len = pg[*idx.last().unwrap()].span.end - pg[pidx[0]].span.start;
            if *ps == style && style != Style::Reveal && !pg[idx[0]].hit && len <= MAX_CUE {
                pidx.extend(idx);
                continue;
            }
        }
        merged.push((style, idx));
    }
    let mut groups = merged;
    // A short tail folds into the cue before it (a reveal stays on its own).
    if groups.len() > 1 {
        let last = groups.last().unwrap();
        let len = pg[*last.1.last().unwrap()].span.end - pg[last.1[0]].span.start;
        if len < MIN_CUE / 2.0 && last.0 != Style::Reveal {
            let (_, tail) = groups.pop().unwrap();
            groups.last_mut().unwrap().1.extend(tail);
        }
    }

    let mut cues: Vec<Cue> = groups
        .iter()
        .enumerate()
        .map(|(index, (style, idx))| {
            let first = &pg[idx[0]];
            let last = &pg[*idx.last().unwrap()];
            let total: f64 = idx.iter().map(|&i| pg[i].span.end - pg[i].span.start).sum::<f64>().max(1e-3);
            let energy = idx.iter().map(|&i| pg[i].energy as f64 * (pg[i].span.end - pg[i].span.start)).sum::<f64>() / total;
            let style = settings.styles.get(&first.span.page).copied().unwrap_or(*style);
            Cue {
                index,
                start: first.span.start,
                end: last.span.end,
                style,
                pages: (first.span.page, last.span.page),
                bpm: tempo(style, energy as f32),
                root: root_for(style, home_key),
                mode: mode(style).to_string(),
                energy: energy as f32,
                hit: first.hit || style == Style::Reveal,
                drop_before: first.drop || style == Style::Reveal,
                riser_out: false,
            }
        })
        .collect();
    // The first cue starts with the video; the last one runs to its end.
    if let Some(c) = cues.first_mut() {
        c.start = 0.0;
        c.drop_before = false;
    }
    // Hits land where the energy jumps or a fight breaks out, never closer than HIT_GAP apart
    // (a reveal always gets its hit).
    let mut last_hit = f64::NEG_INFINITY;
    for i in 0..cues.len() {
        if i > 0 {
            let (a, b) = (&cues[i - 1], &cues[i]);
            let jump = b.energy - a.energy >= 3.0;
            let into_big = family(b.style) == 0 && family(a.style) != 0 && b.style != Style::Tense;
            let wanted = b.hit || jump || into_big;
            cues[i].hit = b.style == Style::Reveal || (wanted && b.start - last_hit >= HIT_GAP);
        }
        if cues[i].hit {
            last_hit = cues[i].start;
        }
    }
    for i in 0..cues.len().saturating_sub(1) {
        let build = cues[i + 1].hit || cues[i + 1].drop_before;
        cues[i].riser_out = build && cues[i].end - cues[i].start >= 6.0;
    }

    (cues, energy_curve(&pg.iter().map(|p| (p.span, p.energy)).collect::<Vec<_>>()))
}

/// Energy sampled every [`Score::ENERGY_STEP`] s and smoothed over ~3 s, so build-ups ramp.
fn energy_curve(pages: &[(Span, f32)]) -> EnergyCurve {
    let end = pages.iter().map(|(s, _)| s.end).fold(0.0, f64::max);
    let n = (end / Score::ENERGY_STEP).ceil() as usize + 1;
    let raw: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 * Score::ENERGY_STEP;
            pages.iter().find(|(s, _)| t >= s.start && t < s.end).or(pages.last()).map(|(_, e)| *e).unwrap_or(3.0)
        })
        .collect();
    let w = 3usize;
    let smooth = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(w);
            let hi = (i + w + 1).min(n);
            raw[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        })
        .collect();
    EnergyCurve(smooth)
}

impl EnergyCurve {
    pub fn at(&self, t: f64) -> f32 {
        let i = (t / Score::ENERGY_STEP).max(0.0) as usize;
        self.0.get(i).or(self.0.last()).copied().unwrap_or(3.0)
    }
}

/// Maps manga sound-effect lettering ("BOOM", "ドキ", "gogogo"...) to a sound.
pub fn sfx_for(text: &str) -> Option<Hit> {
    let t: String = text.to_lowercase().chars().filter(|c| !c.is_ascii_punctuation() && !c.is_whitespace()).collect();
    if t.is_empty() {
        return None;
    }
    let table: &[(&[&str], Hit)] = &[
        (&["doki", "badump", "thumpthump", "dokun", "heartbeat", "ドキ", "ドクン"], Hit::Heartbeat),
        (&["gogo", "rumble", "dododo", "ゴゴ", "ドドド", "rrrr"], Hit::Rumble),
        (&["kaboom", "boom", "blast", "explo", "bang", "バン", "ドカーン"], Hit::Explosion),
        (&["dooon", "don", "ドン", "impact"], Hit::Impact),
        (&["slash", "shing", "slice", "zan", "zing", "ザン", "ズバ", "シュパ"], Hit::Slash),
        (&["whoosh", "fwoosh", "swoosh", "woosh", "swish", "vwoom", "shuu", "シュ", "ヒュ"], Hit::Whoosh),
        (&["punch", "pow", "bam", "wham", "smack", "thwack", "bash", "crack", "dokan", "gon", "ゴン", "バキ", "ドカ"], Hit::Punch),
        (&["crash", "shatter", "glass", "パリン", "ガシャ"], Hit::Shatter),
        (&["thud", "thump", "dosa", "plop", "ドサ"], Hit::Thud),
        (&["sparkle", "kira", "shine", "twinkle", "キラ"], Hit::Sparkle),
        (&["zap", "bzzt", "zzt", "crackle", "bizz", "バチ"], Hit::Zap),
        (&["ding", "ring", "chime", "ping", "ピン"], Hit::Ding),
        (&["step", "tap", "tok", "katsu", "footstep", "コツ", "スタ"], Hit::Footstep),
    ];
    table.iter().find(|(words, _)| words.iter().any(|w| t.contains(w))).map(|(_, h)| *h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(page: usize, kind: &str, intensity: u8) -> PageInfo {
        PageInfo { page, kind: kind.into(), intensity, ..Default::default() }
    }

    fn spans(lens: &[f64]) -> Vec<Span> {
        let mut t = 0.0;
        lens.iter()
            .enumerate()
            .map(|(page, l)| {
                let s = Span { page, start: t, end: t + l };
                t += l;
                s
            })
            .collect()
    }

    #[test]
    fn classifies_scene_kinds_and_moods() {
        assert_eq!(classify(&page(0, "fight", 6)), Style::Action);
        assert_eq!(classify(&page(0, "fight", 10)), Style::Epic);
        assert_eq!(classify(&page(0, "reveal", 5)), Style::Reveal);
        assert_eq!(classify(&PageInfo { kind: "dialogue".into(), mood: "Ominous".into(), intensity: 4, ..Default::default() }), Style::Tense);
        assert_eq!(classify(&page(0, "dialogue", 2)), Style::Calm);
    }

    #[test]
    fn groups_pages_and_marks_builds_and_hits() {
        let pages = vec![page(0, "calm", 2), page(1, "calm", 3), page(2, "calm", 3), page(3, "fight", 8), page(4, "fight", 8), page(5, "reveal", 9)];
        let sp = spans(&[8.0, 7.0, 6.0, 10.0, 9.0, 6.0]);
        let (cues, energy) = plan(&pages, &sp, &[], &MusicSettings::default(), 9);
        let styles: Vec<Style> = cues.iter().map(|c| c.style).collect();
        assert_eq!(styles, vec![Style::Calm, Style::Action, Style::Reveal]);
        assert!(cues[0].riser_out && cues[1].hit, "a riser builds into the fight");
        assert!(cues[2].drop_before && cues[2].hit, "the reveal drops then hits");
        assert_eq!(cues[0].start, 0.0);
        assert_eq!(cues[2].end, 46.0);
        assert!(energy.at(40.0) > energy.at(2.0));
    }

    #[test]
    fn short_style_flickers_do_not_split_cues() {
        let pages = vec![page(0, "calm", 2), page(1, "comedy", 3), page(2, "calm", 3)];
        let (cues, _) = plan(&pages, &spans(&[4.0, 3.0, 5.0]), &[], &MusicSettings::default(), 0);
        assert_eq!(cues.len(), 1);
    }

    #[test]
    fn user_override_wins() {
        let pages = vec![page(0, "calm", 2), page(1, "calm", 3)];
        let mut s = MusicSettings::default();
        s.styles.insert(0, Style::Mystery);
        let (cues, _) = plan(&pages, &spans(&[10.0, 10.0]), &[], &s, 0);
        assert_eq!(cues[0].style, Style::Mystery);
        assert_eq!(cues[0].mode, "dorian");
    }

    #[test]
    fn maps_sound_effect_lettering() {
        assert_eq!(sfx_for("BOOM!!"), Some(Hit::Explosion));
        assert_eq!(sfx_for("ドキ ドキ"), Some(Hit::Heartbeat));
        assert_eq!(sfx_for("Go go go go"), Some(Hit::Rumble));
        assert_eq!(sfx_for("..."), None);
    }
}
