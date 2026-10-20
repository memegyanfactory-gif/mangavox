//! Spotting: where each music cue starts and which genre it is in, the way a recap editor lays music
//! under a chapter. Pages are read from the Read phase's scene kind / mood / intensity (or the optional
//! AI directions), grouped into cues of sensible length (one bed per arc, not per page), and given
//! impacts, drops before reveals, risers and record stops.

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

impl PageInfo {
    fn words(&self) -> String {
        format!("{} {} {}", self.kind.to_lowercase(), self.mood.to_lowercase(), self.tags.join(" ").to_lowercase())
    }

    /// A reveal / twist page strong enough for a drop and an impact.
    pub fn is_reveal(&self) -> bool {
        matches!(self.kind.to_lowercase().as_str(), "reveal" | "twist" | "shock") && self.intensity >= 5
    }

    /// A fight page.
    pub fn is_fight(&self) -> bool {
        matches!(self.kind.to_lowercase().as_str(), "fight" | "action" | "battle") || has(&self.words(), &["fight", "battle", "clash", "attack"])
    }

    /// The "system window" / status-screen beats of hunter and regression manhwa (glitch effect).
    pub fn is_system(&self) -> bool {
        has(&self.words(), &["system", "status window", "notification", "quest", "level up", "glitch", "hologram"])
    }

    fn bright(&self) -> bool {
        has(&self.words(), &["love", "romantic", "tender", "warm", "hope", "happy", "funny", "comedy", "silly", "playful", "cheer", "peaceful"])
    }
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
/// Long stretches in one genre get a fresh variation this often.
pub const MAX_CUE: f64 = 90.0;
/// Reveals (drop + hit) at least this far apart; closer ones join the cue they are in.
pub const REVEAL_GAP: f64 = 45.0;
/// Impacts at least this far apart.
pub const HIT_GAP: f64 = 25.0;

/// 0 intense (fights, climaxes, dark beds), 1 soft, 2 light.
pub fn family(s: Style) -> u8 {
    match s {
        Style::Phonk | Style::Epic | Style::Tension | Style::Horror => 0,
        Style::Lofi => 2,
        Style::Sad | Style::Mystery => 1,
    }
}

fn has(words: &str, any: &[&str]) -> bool {
    any.iter().any(|w| words.contains(w))
}

/// The genre a page asks for.
pub fn classify(p: &PageInfo) -> Style {
    let kind = p.kind.to_lowercase();
    let words = p.words();
    let i = p.intensity;
    let power = has(&words, &["power", "awaken", "level up", "triumph", "heroic", "epic", "transform", "unleash", "aura", "climax", "resolve", "determin"]);
    let horror = has(&words, &["horror", "monster", "creep", "gore", "blood", "scary", "terrif", "nightmare", "corpse", "ghost", "demon"]);
    match kind.as_str() {
        "fight" | "action" | "battle" => {
            if power || i >= 9 {
                Style::Epic
            } else {
                Style::Phonk
            }
        }
        "reveal" | "twist" | "shock" => {
            if horror {
                Style::Horror
            } else if i >= 7 {
                Style::Epic
            } else if i >= 5 {
                Style::Tension
            } else {
                Style::Mystery
            }
        }
        "horror" => Style::Horror,
        "emotional" | "sad" | "romance" | "flashback" | "memory" => Style::Sad,
        "comedy" | "slice-of-life" | "slice of life" => Style::Lofi,
        _ => {
            if horror {
                Style::Horror
            } else if has(&words, &["ominous", "tense", "dread", "menac", "danger", "suspense", "threat", "scheme", "cold", "villain"]) {
                Style::Tension
            } else if has(&words, &["sad", "grief", "tear", "melanchol", "loss", "lonely", "bittersweet", "nostalg", "love", "tender", "romantic", "regret"]) {
                Style::Sad
            } else if has(&words, &["funny", "silly", "playful", "comic", "goofy", "light-hearted", "lighthearted", "cheer", "happy", "daily", "school", "peaceful"]) {
                Style::Lofi
            } else if has(&words, &["badass", "cocky", "confident", "smug", "arrogant", "sigma", "swagger", "hype"]) {
                Style::Phonk
            } else if power || i >= 9 {
                Style::Epic
            } else if has(&words, &["mysteri", "strange", "curious", "secret", "eerie", "investig", "clue", "unknown", "puzzl"]) {
                Style::Mystery
            } else if i >= 7 {
                Style::Tension
            } else {
                // A neutral narration bed: soft and dark-ish, never in the way.
                Style::Mystery
            }
        }
    }
}

/// Tempo for a genre at an energy (0..10). Phonk and trap are counted full time (the drums play
/// half time over it); see `docs/design/music.md` for the ranges.
pub fn tempo(style: Style, energy: f32) -> f32 {
    let e = (energy / 10.0).clamp(0.0, 1.0);
    let (lo, hi) = match style {
        Style::Phonk => (130.0, 150.0),
        Style::Epic => (92.0, 124.0),
        Style::Tension => (84.0, 100.0),
        Style::Sad => (62.0, 74.0),
        Style::Lofi => (76.0, 90.0),
        Style::Horror => (60.0, 70.0),
        Style::Mystery => (84.0, 98.0),
    };
    (lo + (hi - lo) * e).round()
}

pub fn mode(style: Style, bright: bool) -> &'static str {
    match style {
        Style::Phonk | Style::Horror => "phrygian",
        Style::Epic => "harmonic minor",
        Style::Tension => "minor",
        Style::Mystery => "harmonic minor",
        Style::Sad => {
            if bright {
                "major"
            } else {
                "minor"
            }
        }
        Style::Lofi => {
            if bright {
                "major"
            } else {
                "dorian"
            }
        }
    }
}

/// Major-mode cues sit in the relative major so the whole chapter stays in one family of keys.
fn root_for(mode: &str, home: u8) -> u8 {
    if mode == "major" {
        (home + 3) % 12
    } else {
        home
    }
}

/// Builds the cue sheet. `directions` (from the AI pass) override the per-page reading; the chapter
/// genre (`settings.genre`) replaces every cue's genre; `settings.styles` (per-cue overrides, keyed by
/// a cue's first page) win over everything.
pub fn plan(pages: &[PageInfo], spans: &[Span], directions: &[Direction], settings: &MusicSettings, home_key: u8) -> (Vec<Cue>, EnergyCurve) {
    let info = |page: usize| pages.iter().find(|p| p.page == page);
    let direction = |page: usize| directions.iter().find(|d| d.page == page);
    let shift = settings.energy_shift();
    struct Pg {
        span: Span,
        style: Style,
        energy: f32,
        hit: bool,
        reveal: bool,
        bright: bool,
    }
    let pg: Vec<Pg> = spans
        .iter()
        .map(|s| {
            let p = info(s.page).cloned().unwrap_or(PageInfo { page: s.page, kind: "dialogue".into(), intensity: 3, ..Default::default() });
            let d = direction(s.page);
            let style = d.and_then(|d| d.style).unwrap_or_else(|| classify(&p));
            let energy = (d.and_then(|d| d.energy).unwrap_or(p.intensity as f32) + shift).clamp(0.0, 10.0);
            let reveal = d.map(|d| d.drop).unwrap_or_else(|| p.is_reveal());
            Pg { span: *s, style, energy, hit: d.is_some_and(|d| d.hit), reveal, bright: p.bright() }
        })
        .collect();

    // Group pages into cues, the way an editor holds one track through a sequence: a change of
    // family (intense / soft / light) needs MIN_CUE, a change within a family twice that, and only a
    // fight breaking out of a quiet stretch (or a well-spaced reveal) cuts in sooner.
    let mut groups: Vec<(Style, Vec<usize>)> = vec![];
    let mut last_reveal = f64::NEG_INFINITY;
    for (i, p) in pg.iter().enumerate() {
        let start_new = match groups.last() {
            None => true,
            Some((style, idx)) => {
                let len = p.span.start - pg[idx[0]].span.start;
                let reveal = p.reveal && p.span.start - last_reveal >= REVEAL_GAP && len >= 6.0;
                let breaks_out = family(p.style) == 0 && family(*style) != 0 && len >= 8.0;
                let change = if family(p.style) != family(*style) { len >= MIN_CUE } else { p.style != *style && len >= 2.0 * MIN_CUE };
                reveal || breaks_out || (p.hit && len >= 6.0) || change || len >= MAX_CUE
            }
        };
        if start_new {
            if p.reveal {
                last_reveal = p.span.start;
            }
            groups.push((p.style, vec![i]));
        } else {
            groups.last_mut().unwrap().1.push(i);
        }
    }
    // Each cue takes the genre that fills most of it; a reveal cue keeps the reveal page's genre.
    for (style, idx) in groups.iter_mut() {
        if pg[idx[0]].reveal {
            continue;
        }
        let mut time: Vec<(Style, f64)> = vec![];
        for &i in idx.iter() {
            let d = pg[i].span.end - pg[i].span.start;
            match time.iter_mut().find(|(s, _)| *s == pg[i].style) {
                Some((_, t)) => *t += d,
                None => time.push((pg[i].style, d)),
            }
        }
        if let Some((s, _)) = time.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            *style = s;
        }
    }
    // Neighbours that ended up in the same genre become one cue (up to MAX_CUE).
    let mut merged: Vec<(Style, Vec<usize>)> = vec![];
    for (style, idx) in groups {
        if let Some((ps, pidx)) = merged.last_mut() {
            let len = pg[*idx.last().unwrap()].span.end - pg[pidx[0]].span.start;
            let first = &pg[idx[0]];
            if *ps == style && !first.reveal && !first.hit && len <= MAX_CUE {
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
        if len < MIN_CUE / 2.0 && !pg[last.1[0]].reveal {
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
            let bright_time: f64 = idx.iter().filter(|&&i| pg[i].bright).map(|&i| pg[i].span.end - pg[i].span.start).sum();
            let style = settings.styles.get(&first.span.page).copied().or(settings.genre).unwrap_or(*style);
            let mode = mode(style, bright_time * 2.0 > total);
            Cue {
                index,
                start: first.span.start,
                end: last.span.end,
                style,
                pages: (first.span.page, last.span.page),
                bpm: tempo(style, energy as f32),
                root: root_for(mode, home_key),
                mode: mode.to_string(),
                energy: energy as f32,
                hit: first.hit || first.reveal,
                drop_before: first.reveal,
                riser_out: false,
                record_stop: false,
            }
        })
        .collect();
    // The first cue starts with the video.
    if let Some(c) = cues.first_mut() {
        c.start = 0.0;
        c.drop_before = false;
    }
    // Impacts land where the energy jumps or a fight breaks out, never closer than HIT_GAP apart (a
    // reveal always gets its impact). Light cues after intense ones stop like a record instead.
    let mut last_hit = f64::NEG_INFINITY;
    for i in 0..cues.len() {
        if i > 0 {
            let (a, b) = (&cues[i - 1], &cues[i]);
            let jump = b.energy - a.energy >= 3.0;
            let into_big = family(b.style) == 0 && family(a.style) != 0 && b.style != Style::Tension && b.style != Style::Horror;
            let wanted = b.hit || jump || into_big;
            let hit = b.drop_before || (wanted && b.start - last_hit >= HIT_GAP);
            let stop = !hit && b.style == Style::Lofi && family(a.style) == 0;
            cues[i].hit = hit;
            cues[i].record_stop = stop;
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
        (&["clang", "clash", "kiin", "gakin", "キン", "ガキン"], Hit::Clash),
        (&["dooon", "don", "ドン", "impact"], Hit::Impact),
        (&["slash", "shing", "slice", "zan", "zing", "ザン", "ズバ", "シュパ"], Hit::Slash),
        (&["whoosh", "fwoosh", "swoosh", "woosh", "swish", "vwoom", "shuu", "シュ", "ヒュ"], Hit::Whoosh),
        (&["punch", "pow", "bam", "wham", "smack", "thwack", "bash", "crack", "dokan", "gon", "ゴン", "バキ", "ドカ"], Hit::Punch),
        (&["crash", "shatter", "glass", "パリン", "ガシャ"], Hit::Shatter),
        (&["thud", "thump", "dosa", "plop", "ドサ"], Hit::Thud),
        (&["sparkle", "kira", "shine", "twinkle", "キラ"], Hit::Sparkle),
        (&["zap", "bzzt", "zzt", "crackle", "bizz", "バチ"], Hit::Zap),
        (&["beep", "blip", "glitch", "ピコ"], Hit::Glitch),
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
        assert_eq!(classify(&page(0, "fight", 6)), Style::Phonk);
        assert_eq!(classify(&page(0, "fight", 10)), Style::Epic);
        assert_eq!(classify(&PageInfo { kind: "fight".into(), mood: "awakening power".into(), intensity: 7, ..Default::default() }), Style::Epic);
        assert_eq!(classify(&page(0, "reveal", 8)), Style::Epic);
        assert_eq!(classify(&PageInfo { kind: "dialogue".into(), mood: "Ominous".into(), intensity: 4, ..Default::default() }), Style::Tension);
        assert_eq!(classify(&PageInfo { kind: "dialogue".into(), mood: "creepy, monster".into(), intensity: 4, ..Default::default() }), Style::Horror);
        assert_eq!(classify(&page(0, "comedy", 3)), Style::Lofi);
        assert_eq!(classify(&page(0, "emotional", 3)), Style::Sad);
        assert_eq!(classify(&page(0, "dialogue", 2)), Style::Mystery);
    }

    #[test]
    fn groups_pages_and_marks_builds_and_hits() {
        let pages = vec![page(0, "calm", 2), page(1, "calm", 3), page(2, "calm", 3), page(3, "fight", 7), page(4, "fight", 7), page(5, "reveal", 9)];
        let sp = spans(&[8.0, 7.0, 6.0, 10.0, 9.0, 6.0]);
        let (cues, energy) = plan(&pages, &sp, &[], &MusicSettings::default(), 9);
        let styles: Vec<Style> = cues.iter().map(|c| c.style).collect();
        assert_eq!(styles, vec![Style::Mystery, Style::Phonk, Style::Epic]);
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
    fn user_override_and_chapter_genre_win() {
        let pages = vec![page(0, "calm", 2), page(1, "fight", 8)];
        let mut s = MusicSettings { genre: Some(Style::Phonk), ..Default::default() };
        let (cues, _) = plan(&pages, &spans(&[25.0, 25.0]), &[], &s, 0);
        assert!(cues.iter().all(|c| c.style == Style::Phonk));
        s.styles.insert(0, Style::Sad);
        let (cues, _) = plan(&pages, &spans(&[25.0, 25.0]), &[], &s, 0);
        assert_eq!(cues[0].style, Style::Sad);
        assert_eq!(cues[0].mode, "minor");
    }

    #[test]
    fn energy_bias_moves_the_tempo() {
        let pages = vec![page(0, "fight", 6)];
        let chill = MusicSettings { energy: 0.0, ..Default::default() };
        let hyped = MusicSettings { energy: 1.0, ..Default::default() };
        let (a, _) = plan(&pages, &spans(&[30.0]), &[], &chill, 0);
        let (b, _) = plan(&pages, &spans(&[30.0]), &[], &hyped, 0);
        assert!(b[0].bpm > a[0].bpm && b[0].energy > a[0].energy);
    }

    #[test]
    fn comedy_after_a_fight_stops_like_a_record() {
        let pages = vec![page(0, "fight", 7), page(1, "fight", 7), page(2, "comedy", 3), page(3, "comedy", 3)];
        let (cues, _) = plan(&pages, &spans(&[12.0, 12.0, 12.0, 12.0]), &[], &MusicSettings::default(), 0);
        assert_eq!(cues.last().unwrap().style, Style::Lofi);
        assert!(cues.last().unwrap().record_stop);
    }

    #[test]
    fn old_style_names_still_parse() {
        let s: MusicSettings = serde_json::from_str(r#"{"sfx":true,"styles":{"0":"action","4":"calm","9":"reveal"}}"#).unwrap();
        assert_eq!(s.sfx, SfxAmount::Subtle);
        assert_eq!(s.styles[&0], Style::Phonk);
        assert_eq!(s.styles[&4], Style::Lofi);
        assert_eq!(s.styles[&9], Style::Epic);
        assert_eq!(Style::parse("Emotional"), Some(Style::Sad));
        assert_eq!(Style::parse("phonk fight"), Some(Style::Phonk));
    }

    #[test]
    fn maps_sound_effect_lettering() {
        assert_eq!(sfx_for("BOOM!!"), Some(Hit::Explosion));
        assert_eq!(sfx_for("ドキ ドキ"), Some(Hit::Heartbeat));
        assert_eq!(sfx_for("Go go go go"), Some(Hit::Rumble));
        assert_eq!(sfx_for("CLANG"), Some(Hit::Clash));
        assert_eq!(sfx_for("..."), None);
    }
}
