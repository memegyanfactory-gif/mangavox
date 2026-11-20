//! The episode director's check (演出チェック) of a storyboard: what a program can test of film
//! grammar and of the pacing measured on real episodes. [`autofix`] puts right what needs no
//! judgment (a cut too short for its lines, a mouth that cannot be seen); [`check`] lists the
//! rest as notes the director revises its scene from.
use std::collections::HashMap;

use super::{pacing, said_aloud, size_rank, AnimeChapter, Cut, Issue, Scene};
use crate::project::Project;

/// The fastest a still drawing is panned: a frame in seven seconds.
const PAN_MAX: f32 = 1. / 7.;

fn kind_of(kinds: &[(u32, String)], scene: u32) -> &str {
    kinds.iter().find(|(n, _)| *n == scene).map(|(_, k)| k.as_str()).unwrap_or("dialogue")
}

/// Recipes that repeat a framing on purpose.
const REPEATS: [&str; 3] = ["darting-eyes", "clock-cutaway", "intro-inserts"];

fn speaker_seen(c: &Cut) -> bool {
    c.lines.iter().filter(|l| said_aloud(l)).any(|l| c.characters.iter().any(|x| x.name.eq_ignore_ascii_case(l.speaker.trim())))
}

/// Puts right what needs no judgment and returns how many cuts changed: a cut is made long
/// enough for its lines; a mouth only moves when its speaker is in the frame, and then the cut
/// is at least mouth flaps; an animated speaker facing us moves their mouth; a frozen drawing
/// under a moving camera is "camera" and the reverse.
pub fn autofix(p: &mut Project, chapter: u32) -> usize {
    let anime = &mut p.chapter_mut(chapter).anime;
    let anime_kinds: Vec<(u32, String)> = anime.scenes.iter().map(|s| (s.number, s.kind.clone())).collect();
    let mut changed = 0;
    for c in &mut anime.cuts {
        let before = (c.seconds, c.mouth, c.motion.clone(), c.plate_only);
        let need = pacing::spoken_seconds(&c.lines);
        if need > 0. && c.seconds < need {
            c.seconds = ((need * 10.).ceil() / 10.).min(pacing::CUT_MAX);
        }
        c.seconds = c.seconds.clamp(pacing::CUT_MIN, pacing::CUT_MAX);
        if c.plate_only && c.characters.iter().any(|x| !x.name.trim().is_empty()) {
            c.plate_only = false;
        }
        // A mouth moves only when its speaker is in the frame; and when the director animates a
        // speaker who faces us (lip, pose, full), the mouth is what moves.
        let seen = speaker_seen(c);
        if c.mouth && !seen {
            c.mouth = false;
        }
        let turned_away = c.lines.iter().filter(|l| said_aloud(l)).all(|l| c.characters.iter().filter(|x| x.name.eq_ignore_ascii_case(l.speaker.trim())).all(|x| x.facing == "away"));
        if !c.mouth && seen && !turned_away && matches!(c.motion.as_str(), "lip" | "pose" | "full") {
            c.mouth = true;
        }
        // Only thoughts or narration and no other motion: there is no mouth to flap.
        if c.motion == "lip" && !c.lines.iter().any(said_aloud) {
            c.motion = if c.camera == "fix" { "hold" } else { "camera" }.into();
        }
        // A still drawing is never panned faster than a frame in seven seconds.
        if matches!(c.motion.as_str(), "hold" | "camera") && matches!(c.camera.as_str(), "pan_left" | "pan_right" | "tilt_up" | "tilt_down") {
            let travel = pacing::camera_extent(kind_of(&anime_kinds, c.scene), &c.camera, c.seconds, c.amount).0;
            if travel / c.seconds.max(0.1) > PAN_MAX {
                c.amount = (c.amount * PAN_MAX * c.seconds / travel).max(0.25);
            }
        }
        match (c.motion.as_str(), c.camera.as_str()) {
            ("hold" | "camera", _) if c.mouth => c.motion = "lip".into(),
            ("camera", "fix") => c.motion = "hold".into(),
            ("hold", cam) if cam != "fix" && cam != "shake" => c.motion = "camera".into(),
            _ => {}
        }
        if before != (c.seconds, c.mouth, c.motion.clone(), c.plate_only) {
            changed += 1;
        }
    }
    // Real episodes move the camera about half the time. Where a talking scene moves it far more,
    // the gentlest moves on acted cuts (a push or pull on a face that is talking) are locked off,
    // shortest first, until the scene is back near the measured share.
    for (scene, kind) in &anime_kinds {
        if kind == "action" {
            continue;
        }
        let total: f32 = anime.cuts.iter().filter(|c| c.scene == *scene).map(|c| c.seconds).sum();
        if total <= 0. || anime.cuts.iter().filter(|c| c.scene == *scene).count() < 4 {
            continue;
        }
        let moving = |cuts: &[Cut]| cuts.iter().filter(|c| c.scene == *scene && c.camera != "fix").map(|c| c.seconds).sum::<f32>() / total;
        let mut gentle: Vec<usize> = (0..anime.cuts.len()).filter(|i| {
            let c = &anime.cuts[*i];
            c.scene == *scene && matches!(c.camera.as_str(), "push_in" | "pull_out") && matches!(c.motion.as_str(), "lip" | "pose")
        }).collect();
        gentle.sort_by(|a, b| anime.cuts[*a].seconds.total_cmp(&anime.cuts[*b].seconds));
        for i in gentle {
            if moving(&anime.cuts) <= MOVE_SHARE {
                break;
            }
            anime.cuts[i].camera = "fix".into();
            changed += 1;
        }
    }
    changed
}

/// The share of a talking scene's time a camera move may take before gentle moves are locked off.
const MOVE_SHARE: f32 = 0.6;

fn issue(code: &str, error: bool, scene: u32, cut: u32, message: String) -> Issue {
    Issue { code: code.into(), severity: if error { "error" } else { "warning" }.into(), scene, cut, message }
}

/// Who a cut is of: its characters, or its plate when nobody is in it.
fn subject(c: &Cut) -> Vec<String> {
    let mut who: Vec<String> = c.characters.iter().map(|x| x.name.trim().to_lowercase()).filter(|n| !n.is_empty()).collect();
    who.sort();
    who.dedup();
    if who.is_empty() {
        who.push(format!("plate:{}", c.plate));
    }
    who
}

fn side_of(s: &str) -> Option<i8> {
    match s {
        "left" => Some(-1),
        "centre" => Some(0),
        "right" => Some(1),
        _ => None,
    }
}

/// A shot on the line itself (everyone seen from the front or from behind, or through
/// someone's eyes): after it the camera may be on either side.
fn neutral(c: &Cut) -> bool {
    c.size == "pov" || (!c.characters.is_empty() && c.characters.iter().all(|x| matches!(x.facing.as_str(), "camera" | "away")))
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The spoken lines of the scene's panels that no cut of it carries.
fn missing_lines(a: &AnimeChapter, s: &Scene, cuts: &[&Cut]) -> (usize, Vec<String>) {
    let said: Vec<String> = cuts.iter().flat_map(|c| &c.lines).map(|l| letters(&l.text)).filter(|t| !t.is_empty()).collect();
    let end = s.to.min(a.panels.len());
    let mut total = 0;
    let mut missing = vec![];
    for l in a.panels[s.from.min(end)..end].iter().filter(|x| x.story()).flat_map(|x| &x.lines) {
        let t = letters(&l.text);
        if t.len() < 4 {
            continue;
        }
        total += 1;
        let head: String = t.chars().take(22).collect();
        let found = said.iter().any(|x| x.contains(&head) || t.contains(&x.chars().take(22).collect::<String>()));
        if !found {
            missing.push(l.text.trim().to_string());
        }
    }
    (total, missing)
}

/// Everything the check finds in chapter `chapter`'s board, in the order of the episode.
pub fn check(p: &Project, chapter: u32) -> Vec<Issue> {
    let Some(ch) = p.chapter(chapter) else { return vec![] };
    let a = &ch.anime;
    let mut out: Vec<Issue> = vec![];
    let mut prev_place = String::new();
    for s in &a.scenes {
        let cuts: Vec<&Cut> = a.cuts.iter().filter(|c| c.scene == s.number).collect();
        let n = s.number;
        if cuts.is_empty() {
            out.push(issue("scene-empty", true, n, 0, format!("scene {n} \"{}\" has no cuts", s.title)));
            continue;
        }
        let pace = pacing::pace(&s.kind);
        let total: f32 = cuts.iter().map(|c| c.seconds).sum();
        let first = cuts[0].number;

        // Length and rhythm against the measured pacing.
        if s.seconds > 0. && (total < s.seconds * 0.6 || total > s.seconds * 1.6) {
            let how = if total < s.seconds { "it is rushed: give the beats their cuts and the lines their time" } else { "it drags: trim or merge cuts" };
            out.push(issue("scene-length", false, n, 0, format!("the scene runs {total:.0} s where about {:.0} s is right for its panels; {how}", s.seconds)));
        }
        if cuts.len() >= 4 {
            let mean = total / cuts.len() as f32;
            if mean < pace.mean_cut.0 * 0.85 {
                out.push(issue("scene-pace", false, n, 0, format!("its cuts average {mean:.1} s; a {} scene averages {:.1}-{:.1} s: merge neighbouring cuts into longer takes that carry several lines", s.kind, pace.mean_cut.0, pace.mean_cut.1)));
            } else if mean > pace.mean_cut.1 * 1.15 {
                out.push(issue("scene-pace", false, n, 0, format!("its cuts average {mean:.1} s; a {} scene averages {:.1}-{:.1} s: split the long takes with reactions and inserts", s.kind, pace.mean_cut.0, pace.mean_cut.1)));
            }
        }

        // A new place is shown wide early.
        let place = s.location.trim().to_lowercase();
        if matches!(s.kind.as_str(), "dialogue" | "action" | "dread") && cuts.len() >= 3 && place != prev_place {
            let wide = cuts.iter().take(3).any(|c| size_rank(&c.size) <= 1 || c.angle.contains("bird"));
            if !wide {
                out.push(issue("establish", true, n, first, format!("the scene moves to {} but none of its first three cuts shows the place: open on an establishing or long shot (who is where)", if s.location.is_empty() { "a new place" } else { &s.location })));
            }
        }
        prev_place = place;

        // The line of a conversation, jump cuts, eyelines, variety.
        let mut order: HashMap<(String, String), i8> = HashMap::new();
        let mut run = 1;
        for (i, c) in cuts.iter().enumerate() {
            if c.lines.len() > 5 {
                out.push(issue("too-many-lines", true, n, c.number, format!("cut {} carries {} lines: split it into two takes", c.number, c.lines.len())));
            }
            if pacing::spoken_seconds(&c.lines) > pacing::CUT_MAX {
                out.push(issue("lines-fit", true, n, c.number, format!("cut {} has more words than one cut can hold: split its lines over two or three cuts", c.number)));
            }
            if c.seconds > pace.long_take {
                out.push(issue("long-take", false, n, c.number, format!("cut {} is {:.0} s, a very long take for a {} scene: split it unless the hold is the point", c.number, c.seconds, s.kind)));
            }
            if c.size == "insert" && c.seconds < 1.2 && s.kind != "action" && !REPEATS.contains(&c.recipe.as_str()) {
                out.push(issue("insert-short", false, n, c.number, format!("cut {} is an insert of {:.1} s: give it at least 1.5 s to be read", c.number, c.seconds)));
            }
            if i > 0 && c.transition != "cut" && !matches!(s.kind.as_str(), "narration" | "montage" | "flashback") {
                out.push(issue("transition", false, n, c.number, format!("cut {} comes in with a {} in the middle of a continuous scene: use a hard cut (dissolves are for time passing, black for the end of a scene)", c.number, c.transition)));
            }
            if neutral(c) {
                order.clear();
            } else {
                let placed: Vec<(String, i8)> = c.characters.iter().filter_map(|x| Some((x.name.trim().to_lowercase(), side_of(&x.side)?))).collect();
                let mut crossed = false;
                for (ai, (an, asd)) in placed.iter().enumerate() {
                    for (bn, bsd) in placed.iter().skip(ai + 1) {
                        if an == bn || asd == bsd {
                            continue;
                        }
                        let (key, sign) = if an < bn { ((an.clone(), bn.clone()), (asd - bsd).signum()) } else { ((bn.clone(), an.clone()), (bsd - asd).signum()) };
                        match order.get(&key) {
                            Some(was) if *was != sign && !crossed => {
                                crossed = true;
                                let (l, r) = if *was < 0 { (&key.0, &key.1) } else { (&key.1, &key.0) };
                                out.push(issue("axis", true, n, c.number, format!("cut {} crosses the line: until now {l} was on the left of {r}; keep them on their sides, or put a shot on the line (from behind them, or straight on one of them) before changing", c.number)));
                            }
                            Some(_) => {}
                            None => {
                                order.insert(key, sign);
                            }
                        }
                    }
                }
            }
            if i == 0 {
                continue;
            }
            let b = cuts[i - 1];
            let repeat = REPEATS.contains(&c.recipe.as_str()) || REPEATS.contains(&b.recipe.as_str());
            if c.size == b.size {
                run += 1;
                if run == 5 {
                    out.push(issue("variety", false, n, c.number, format!("cuts {}-{} are five {} shots in a row: vary the sizes (go wide, or go in for an insert)", c.number - 4, c.number, c.size)));
                }
            } else {
                run = 1;
            }
            if !repeat && c.transition == "cut" && c.size != "insert" && b.size != "insert" && subject(c) == subject(b) && size_rank(&c.size) == size_rank(&b.size) && c.angle == b.angle {
                out.push(issue("jump-cut", true, n, c.number, format!("cuts {} and {} show the same subject at the same size and angle ({} / {}): a jump cut. Change the size by two steps or the angle clearly, or cut away between them", b.number, c.number, c.size, c.angle)));
            }
            // Shot and reverse shot: the two look at each other.
            if let ([x], [y]) = (b.characters.as_slice(), c.characters.as_slice()) {
                let both = |f: &str| x.facing == f && y.facing == f;
                if !x.name.eq_ignore_ascii_case(&y.name) && (both("left") || both("right")) {
                    out.push(issue("eyeline", false, n, c.number, format!("cuts {} and {}: {} and {} both face {}, so their eyelines never meet; in a shot and its reverse one faces left and the other right", b.number, c.number, x.name, y.name, x.facing)));
                }
            }
        }
        // A key animator's review: effects need their source, a person is seen whole before a big
        // close-up, and a scene is not all faces.
        for (i, c) in cuts.iter().enumerate() {
            let effect = c.flash > 0. || !c.fx.is_empty();
            let nobody = |x: &Cut| x.characters.is_empty();
            if effect && nobody(c) && (i == 0 || nobody(cuts[i - 1])) && cuts.get(i + 1).is_none_or(|x| nobody(x)) {
                out.push(issue("fx-source", false, n, c.number, format!("cut {} shows an effect with nobody beside it: show who or what makes it in the cut, or in the cut before or after", c.number)));
            }
        }
        let mut seen: Vec<String> = vec![];
        for c in &cuts {
            for x in &c.characters {
                let name = x.name.trim().to_lowercase();
                if seen.contains(&name) {
                    continue;
                }
                seen.push(name);
                if c.size == "extreme close-up" && !matches!(c.recipe.as_str(), "intro-inserts" | "darting-eyes" | "ecu-eyes" | "reveal-partial") && n != a.scenes.first().map(|x| x.number).unwrap_or(0) {
                    out.push(issue("progression", false, n, c.number, format!("cut {} is the first sight of {} in this scene and it is an extreme close-up: show them whole first (where they stand, what they face), then go in", c.number, x.name)));
                }
            }
        }
        if cuts.len() >= 6 && s.kind != "action" {
            let faces = cuts.iter().filter(|c| !c.characters.is_empty() && matches!(c.size.as_str(), "medium close-up" | "close-up" | "extreme close-up")).count();
            if faces * 10 > cuts.len() * 6 {
                out.push(issue("faces", false, n, 0, format!("{faces} of the scene's {} cuts are faces: carry some of the feeling through poses, hands, the body, silhouettes or what the framing leaves out", cuts.len())));
            }
        }
        if cuts.len() >= 6 {
            let moving = cuts.iter().filter(|c| c.camera != "fix").map(|c| c.seconds).sum::<f32>() / total.max(0.1);
            if moving > 0.75 {
                out.push(issue("scene-moves", false, n, 0, format!("{:.0}% of the scene has a camera move; real episodes have about half: lock off the cuts whose move has no reason (reveal, follow, tighten, unsettle)", moving * 100.)));
            }
        }
        // Shake belongs to physical events, never to talk, narration or memory.
        if matches!(s.kind.as_str(), "dialogue" | "narration" | "flashback") {
            if let Some(c) = cuts.iter().find(|c| c.camera == "shake" || c.shake > 0.) {
                out.push(issue("shake", false, n, c.number, format!("cut {} shakes the camera in a {} scene: shake only for a hit, a landing, an explosion or a scare", c.number, s.kind)));
            }
        }
        for c in &cuts {
            let said = c.lines.iter().filter(|l| said_aloud(l)).count();
            let speakers: std::collections::HashSet<String> = c.lines.iter().filter(|l| said_aloud(l)).map(|l| l.speaker.trim().to_lowercase()).collect();
            if (said >= 3 || speakers.len() >= 2) && c.characters.len() == 1 && matches!(c.size.as_str(), "medium close-up" | "close-up" | "extreme close-up") {
                out.push(issue("take-size", false, n, c.number, format!("cut {} carries {} lines from {} speaker(s) on a single {}: a take of several lines or two speakers is a two-shot, over-the-shoulder or wider", c.number, said, speakers.len(), c.size)));
            }
        }
        for (i, c) in cuts.iter().enumerate() {
            if c.size == "pov" {
                let looker = |j: usize| cuts.get(j).is_some_and(|x| x.size != "pov" && !x.characters.is_empty());
                if !(i > 0 && looker(i - 1) || looker(i + 1)) {
                    out.push(issue("pov", false, n, c.number, format!("cut {} is a point of view with no cut of the looker beside it: put the one whose eyes they are before or after it", c.number)));
                }
            }
        }
        if s.kind == "action" && cuts.len() >= 6 {
            let wide = cuts.iter().filter(|c| size_rank(&c.size) <= 3).count();
            if wide * 10 < cuts.len() * 3 {
                out.push(issue("fight-wide", false, n, 0, format!("only {wide} of the fight's {} cuts are cowboy or wider: keep about a third wide enough to show attacker and target together", cuts.len())));
            }
            let mut run = 0;
            for c in &cuts {
                if c.seconds < 1. {
                    run += 1;
                } else {
                    run = 0;
                }
            }
            if run > 6 {
                out.push(issue("burst-rest", false, n, 0, format!("the fight ends in a burst of {run} cuts under a second: break it with a rest of a second or more")));
            }
        }
        if s.kind == "flashback" && !matches!(cuts[0].transition.as_str(), "white" | "dissolve") && a.scenes.first().map(|x| x.number) != Some(n) {
            out.push(issue("flashback-entry", false, n, first, format!("the memory begins on cut {first} with a plain cut: enter it with white or a dissolve")));
        }
        let mut sizes: Vec<&str> = cuts.iter().map(|c| c.size.as_str()).collect();
        sizes.sort_unstable();
        sizes.dedup();
        if cuts.len() >= 6 && sizes.len() < 3 {
            out.push(issue("variety", false, n, 0, format!("the scene uses only {}: use at least three sizes (a wide, a medium, a close-up or an insert)", sizes.join(" and "))));
        }

        // How much is animated, against what this kind of scene uses.
        if cuts.len() >= 4 && total > 0. {
            // (+ 0: the sum of nothing is minus zero, which would read "-0%".)
            let share = |m: &[&str]| (cuts.iter().filter(|c| m.contains(&c.motion.as_str())).map(|c| c.seconds).sum::<f32>() + 0.) / total;
            let (full, still) = (share(&["full"]), share(&["hold", "camera"]));
            if full > pace.full.1 + 0.1 {
                out.push(issue("motion", false, n, 0, format!("{:.0}% of the scene is continuous animation; a {} scene uses at most {:.0}%: make talk cuts lip or pose, and let lines play over holds, slow camera moves and hidden mouths", full * 100., s.kind, pace.full.1 * 100.)));
            }
            if full < pace.full.0 - 0.1 {
                out.push(issue("motion", false, n, 0, format!("only {:.0}% of this {} scene is continuous animation; give its blows, dashes and falls full motion (at least {:.0}%)", full * 100., s.kind, pace.full.0 * 100.)));
            }
            if cuts.len() >= 5 && still < pace.still_min - 0.1 {
                out.push(issue("motion", false, n, 0, format!("only {:.0}% of the scene is a held drawing or a still under the camera; a {} scene has about {:.0}% or more: play some lines over a wide shot, the listener, backs or an insert", still * 100., s.kind, pace.still_min * 100.)));
            }
        }
        let drift = cuts.iter().filter(|c| c.grade != s.grade).count();
        if cuts.len() >= 2 && drift * 10 > cuts.len() * 3 {
            out.push(issue("grade", false, n, 0, format!("{drift} of its {} cuts leave the scene's colour grade ({}): one grade per scene, another only for a memory or another place", cuts.len(), s.grade)));
        }
        let (lines, missing) = missing_lines(a, s, &cuts);
        if missing.len() >= 2 && missing.len() * 4 > lines {
            let some: Vec<String> = missing.iter().take(3).map(|m| format!("\"{}\"", m.chars().take(60).collect::<String>())).collect();
            out.push(issue("lines-missing", false, n, 0, format!("{} of the manga's {lines} lines in this scene are in no cut (for example {}): keep the dialogue unless the plan cuts it", missing.len(), some.join(", "))));
        }
    }

    // Flashes: never more than three in any second (the Japanese broadcast guideline).
    let mut t = 0.;
    let mut events: Vec<(f32, u32, u32)> = vec![];
    for c in &a.cuts {
        if c.flash > 0. || c.transition == "white" {
            events.push((t, c.scene, c.number));
        }
        t += c.seconds;
    }
    for (i, (at, scene, cut)) in events.iter().enumerate() {
        let within = events[i..].iter().take_while(|(b, ..)| *b < at + 1.).count();
        if within > 3 {
            out.push(issue("flash-rate", true, *scene, *cut, format!("{within} flashes within one second from cut {cut}: at most three a second (broadcast safety); drop some flashes or space the cuts")));
            break;
        }
    }

    // The episode as a whole.
    if let Some(last) = a.cuts.last().filter(|_| a.boarded()) {
        if last.seconds < 4. || last.motion == "full" {
            out.push(issue("ending", false, last.scene, last.number, format!("the episode ends on cut {} ({:.1} s, {}): end on ONE held image of 4 s or more (the eyes, the object that stands for the cliffhanger)", last.number, last.seconds, last.motion)));
        }
        let total = a.seconds();
        if a.cuts.len() >= 12 && total > 0. {
            let moving = (a.cuts.iter().filter(|c| c.camera != "fix").map(|c| c.seconds).sum::<f32>() + 0.) / total;
            if !(0.35..=0.65).contains(&moving) {
                out.push(issue("camera-share", false, 0, 0, format!("{:.0}% of the episode has a camera move; real episodes have about half: {}", moving * 100., if moving < 0.35 { "add slow drifts and pushes over the still cuts" } else { "lock off more cuts; a move needs a reason" })));
            }
        }
    }
    out
}

/// How bad a set of issues is (an error weighs three warnings): a revision is kept only when
/// it brings this down.
pub fn score(issues: &[Issue]) -> usize {
    issues.iter().map(|i| if i.severity == "error" { 3 } else { 1 }).sum()
}

/// The scenes worth sending back to the director (any error, or two warnings), each with its
/// notes as the director reads them.
pub fn to_revise(issues: &[Issue]) -> Vec<(u32, Vec<String>)> {
    let mut by: Vec<(u32, Vec<&Issue>)> = vec![];
    for i in issues.iter().filter(|i| i.scene > 0) {
        match by.iter_mut().find(|(s, _)| *s == i.scene) {
            Some((_, v)) => v.push(i),
            None => by.push((i.scene, vec![i])),
        }
    }
    by.into_iter().filter(|(_, v)| v.iter().any(|i| i.severity == "error") || v.len() >= 2).map(|(s, v)| (s, v.into_iter().map(|i| i.message.clone()).collect())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anime::tests::proj;
    use crate::anime::SourcePanel;
    use crate::project::{Line, PanelChar};

    fn who(name: &str, side: &str, facing: &str) -> PanelChar {
        PanelChar { name: name.into(), side: side.into(), facing: facing.into(), ..Default::default() }
    }

    fn cut(scene: u32, size: &str, secs: f32, chars: Vec<PanelChar>) -> Cut {
        Cut { scene, size: size.into(), angle: "eye level".into(), camera: "fix".into(), motion: "hold".into(), seconds: secs, characters: chars, plate: "p".into(), transition: "cut".into(), grade: "daylight".into(), ..Default::default() }
    }

    fn line(speaker: &str, text: &str) -> Line {
        Line { speaker: speaker.into(), text: text.into(), kind: "speech".into() }
    }

    fn project(kind: &str, seconds: f32, cuts: Vec<Cut>) -> Project {
        let mut p = proj();
        let a = &mut p.chapter_mut(1).anime;
        a.panels = vec![SourcePanel { read: true, description: "x".into(), ..Default::default() }];
        a.scenes = vec![Scene { number: 1, title: "S".into(), kind: kind.into(), location: "Street".into(), grade: "daylight".into(), from: 0, to: 1, seconds, ..Default::default() }];
        a.cuts = cuts;
        a.renumber();
        p
    }

    fn codes(p: &Project) -> Vec<String> {
        check(p, 1).into_iter().map(|i| format!("{}@{}", i.code, i.cut)).collect()
    }

    #[test]
    fn what_needs_no_judgment_is_put_right() {
        let mut talk = cut(1, "medium", 1., vec![who("Mira", "left", "right")]);
        talk.lines = vec![line("Mira", "Too bad I already caught it all."), line("Jin Woo", "Sorry.")];
        talk.mouth = true;
        let mut over = cut(1, "long", 4., vec![who("Mira", "", "")]);
        over.lines = vec![line("Jin Woo", "Let's go.")];
        over.mouth = true;
        over.plate_only = true;
        let mut drift = cut(1, "long", 4., vec![]);
        drift.camera = "pan_left".into();
        // Animated and facing us while speaking: the mouth moves. Seen from behind: it does not.
        let mut acted = cut(1, "medium close-up", 4., vec![who("Mira", "left", "right")]);
        acted.lines = vec![line("Mira", "Go.")];
        acted.motion = "pose".into();
        let mut back = acted.clone();
        back.characters[0].facing = "away".into();
        let mut p = project("dialogue", 12., vec![talk, over, drift, cut(1, "close-up", 2., vec![]), acted, back]);
        assert_eq!(autofix(&mut p, 1), 4);
        assert_eq!((p.chapter(1).unwrap().anime.cuts[4].mouth, p.chapter(1).unwrap().anime.cuts[5].mouth), (true, false));
        let c = &p.chapter(1).unwrap().anime.cuts;
        // Long enough for both lines; she is seen speaking, so the cut is at least mouth flaps.
        assert!((c[0].seconds - 4.2).abs() < 0.001, "{}", c[0].seconds);
        assert_eq!((c[0].mouth, c[0].motion.as_str()), (true, "lip"));
        // The speaker is not in the frame: no mouth moves; someone is in it: not the plate alone.
        assert_eq!((c[1].mouth, c[1].motion.as_str(), c[1].plate_only), (false, "hold", false));
        assert_eq!(c[2].motion, "camera");
        assert_eq!(autofix(&mut p, 1), 0);
    }

    #[test]
    fn the_line_of_a_conversation_is_kept() {
        let two = |l: &str, r: &str| vec![who(l, "left", "right"), who(r, "right", "left")];
        let mut p = project("dialogue", 16., vec![cut(1, "long", 4., two("Mira", "Jin Woo")), cut(1, "medium", 4., vec![who("Mira", "left", "right")]), cut(1, "two-shot", 4., two("Jin Woo", "Mira")), cut(1, "close-up", 4., vec![who("Jin Woo", "right", "left")])]);
        assert_eq!(codes(&p), vec!["axis@3"]);
        assert!(check(&p, 1)[0].message.contains("mira was on the left of jin woo"));
        // A shot on the line (from behind them both) frees the camera to change sides.
        p.chapter_mut(1).anime.cuts[1].characters = vec![who("Mira", "left", "away"), who("Jin Woo", "right", "away")];
        assert_eq!(codes(&p), Vec::<String>::new());
    }

    #[test]
    fn jump_cuts_eyelines_and_sameness_are_found() {
        let solo = |n: &str, f: &str| vec![who(n, "", f)];
        let mut p = project("dialogue", 20., vec![
            cut(1, "long", 4., vec![]),
            cut(1, "medium", 4., solo("Mira", "right")),
            cut(1, "medium", 4., solo("Mira", "right")),
            cut(1, "medium", 4., solo("Jin Woo", "right")),
            cut(1, "medium", 4., solo("Mira", "left")),
        ]);
        assert_eq!(codes(&p), vec!["jump-cut@3", "eyeline@4"]);
        // Five of one size in a row, and only two sizes in a scene of six.
        let a = &mut p.chapter_mut(1).anime;
        a.cuts[2].angle = "low".into();
        a.cuts[3].characters[0].facing = "left".into();
        a.cuts[4].characters[0].facing = "right".into();
        a.cuts.push(cut(1, "medium", 4., solo("Jin Woo", "left")));
        a.scenes[0].seconds = 24.;
        a.renumber();
        assert_eq!(codes(&p), vec!["variety@6", "variety@0"]);
        // A deliberate repeat is not a jump cut.
        let mut eyes = cut(1, "extreme close-up", 0.5, solo("Mira", "left"));
        eyes.recipe = "darting-eyes".into();
        let p = project("dread", 10., vec![cut(1, "long", 4., vec![]), eyes.clone(), eyes, cut(1, "close-up", 5., vec![])]);
        assert_eq!(codes(&p), Vec::<String>::new());
    }

    #[test]
    fn a_scene_is_held_to_the_measured_pacing() {
        // A new place never shown, cuts far too short for talk, and everything fully animated.
        let mut cuts: Vec<Cut> = (0..6).map(|i| cut(1, if i % 2 == 0 { "medium" } else { "close-up" }, 1.5, vec![who(if i % 2 == 0 { "Mira" } else { "Jin Woo" }, "", "")])).collect();
        for c in &mut cuts {
            c.motion = "full".into();
        }
        cuts[5].size = "insert".into();
        cuts[5].seconds = 0.9;
        cuts[5].characters.clear();
        let p = project("dialogue", 40., cuts);
        let found = check(&p, 1);
        let c: Vec<&str> = found.iter().map(|i| i.code.as_str()).collect();
        assert_eq!(c, vec!["scene-length", "scene-pace", "establish", "insert-short", "motion", "motion", "ending"]);
        assert!(found[0].message.contains("runs 8 s where about 40 s") && found[1].message.contains("average 1.4 s") && found[4].message.contains("100% of the scene is continuous animation"));
        assert_eq!(found.iter().filter(|i| i.severity == "error").count(), 1);
        assert_eq!(score(&found), 9);
        assert_eq!(to_revise(&found).len(), 1);
        assert_eq!(to_revise(&found)[0].1.len(), 7, "the episode's ending is the last scene's to fix too");
        // An action scene that never moves.
        let still: Vec<Cut> = (0..4).map(|i| cut(1, ["long", "medium", "close-up", "insert"][i], 2., vec![])).collect();
        let p = project("action", 8., still);
        let found = check(&p, 1);
        assert!(found.iter().any(|i| i.code == "motion" && i.message.contains("only 0% of this action scene")), "{found:#?}");
    }

    #[test]
    fn the_research_rules_hold() {
        // A still panned three frames' worth over six seconds is slowed to a frame in seven.
        let mut pan = cut(1, "long", 6., vec![]);
        pan.camera = "pan_right".into();
        pan.motion = "camera".into();
        pan.amount = 6.;
        // A thought moves no mouth.
        let mut thought = cut(1, "close-up", 3., vec![who("Mira", "", "camera")]);
        thought.motion = "lip".into();
        thought.lines = vec![Line { speaker: "Mira".into(), text: "Not again.".into(), kind: "thought".into() }];
        let mut p = project("dialogue", 9., vec![pan, thought]);
        autofix(&mut p, 1);
        let c = &p.chapter(1).unwrap().anime.cuts;
        let travel = pacing::camera_extent("dialogue", "pan_right", 6., c[0].amount).0;
        assert!(travel / 6. <= PAN_MAX + 0.001, "{travel}");
        assert_eq!((c[1].motion.as_str(), c[1].mouth), ("hold", false));
        // A scene where every acted cut pushes in has its shortest pushes locked off.
        let pushes: Vec<Cut> = (0..5).map(|i| {
            let mut c = cut(1, "medium", 2. + i as f32, vec![who("Mira", "", "camera")]);
            c.camera = "push_in".into();
            c.motion = "pose".into();
            c
        }).collect();
        let mut p = project("dialogue", 20., pushes);
        autofix(&mut p, 1);
        let cams: Vec<&str> = p.chapter(1).unwrap().anime.cuts.iter().map(|c| c.camera.as_str()).collect();
        assert_eq!(cams, vec!["fix", "fix", "fix", "push_in", "push_in"], "100% moving: locking 2, 3 and 4 s brings it to 55%");
        // Shake in talk; four flashes in a second.
        let mut shaky = cut(1, "medium", 3., vec![]);
        shaky.camera = "shake".into();
        let flashes: Vec<Cut> = (0..4).map(|_| {
            let mut c = cut(1, "insert", 0.2, vec![]);
            c.flash = 0.8;
            c
        }).collect();
        let mut cuts = vec![cut(1, "long", 4., vec![]), shaky];
        cuts.extend(flashes);
        cuts.push(cut(1, "close-up", 5., vec![]));
        let p = project("dialogue", 13., cuts);
        let found = check(&p, 1);
        assert!(found.iter().any(|i| i.code == "shake" && i.cut == 2));
        assert!(found.iter().any(|i| i.code == "flash-rate" && i.severity == "error"));
    }

    #[test]
    fn dropped_dialogue_and_a_scene_without_cuts_are_noticed() {
        let mut p = project("dialogue", 8., vec![cut(1, "long", 4., vec![]), cut(1, "medium", 4., vec![who("Mira", "", "")])]);
        {
            let a = &mut p.chapter_mut(1).anime;
            a.panels[0].lines = vec![line("Mira", "You're hurt again?!"), line("Jin Woo", "I'm used to it."), line("Mira", "Be careful!"), line("Mira", "...")];
            a.cuts[1].lines = vec![line("Mira", "You are hurt again?!")];
            a.cuts[1].seconds = 4.;
        }
        let found = check(&p, 1);
        let lost = found.iter().find(|i| i.code == "lines-missing").expect("two of three lines are gone");
        assert!(lost.message.starts_with("3 of the manga's 3 lines") || lost.message.starts_with("2 of the manga's 3 lines"), "{}", lost.message);
        p.chapter_mut(1).anime.cuts[1].lines = vec![line("Mira", "You're hurt again?!"), line("Jin Woo", "I'm used to it."), line("Mira", "Be careful!")];
        assert!(!check(&p, 1).iter().any(|i| i.code == "lines-missing"));
        p.chapter_mut(1).anime.scenes.push(Scene { number: 2, title: "Later".into(), from: 1, to: 1, ..Default::default() });
        let found = check(&p, 1);
        assert!(found.iter().any(|i| i.code == "scene-empty" && i.severity == "error" && i.scene == 2));
        assert_eq!(to_revise(&found).iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![2]);
    }
}
