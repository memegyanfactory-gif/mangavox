//! Small helpers more than one step screen needs, kept once so the screens agree: the take
//! cache of the background loads, the latest round of pictures, the scene order and a shot's
//! length.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mv_anime::{Shot, Store, Take};

/// Takes read by a screen's background loads, by id (a take never changes once written).
pub type TakeCache = Arc<Mutex<HashMap<String, Take>>>;

/// A take through the cache. Blocking: call it from a background load.
pub fn cached_take(store: &Store, cache: &TakeCache, id: &str) -> Option<Take> {
    if let Some(t) = cache.lock().ok().and_then(|c| c.get(id).cloned()) {
        return Some(t);
    }
    let t = store.take(id)?;
    if let Ok(mut c) = cache.lock() {
        c.insert(id.to_string(), t.clone());
    }
    Some(t)
}

/// The latest round of pictures from a card's takes (oldest first, with their batch): every take
/// of the newest take's batch, or the newest take alone (an upload has no batch).
pub fn latest_round(takes: &[(String, Option<String>)]) -> Vec<String> {
    let Some((last, batch)) = takes.last() else { return Vec::new() };
    match batch {
        Some(b) => takes.iter().filter(|(_, x)| x.as_deref() == Some(b.as_str())).map(|(id, _)| id.clone()).collect(),
        None => vec![last.clone()],
    }
}

/// Scene order of the episode (the episode's own order, else the story's), then any other scene
/// in `more` that neither lists. Shot numbers ("3.4") follow this order, like the engine's.
pub fn scene_order(episode: &[String], story: &[String], more: &[String]) -> Vec<String> {
    let base = if episode.is_empty() { story } else { episode };
    let mut out: Vec<String> = Vec::new();
    for s in base.iter().chain(more) {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// A shot's length on screen: the length set by the voices or the user, else the planned one.
pub fn shot_secs(shot: &Shot) -> f32 {
    shot.locked_s.filter(|s| s.is_finite() && *s > 0.).unwrap_or(shot.planned_s).max(0.)
}

/// The one line of the "bible not approved" notice on Shots and Frames: the count of what is
/// still to do, never the whole list (the Bible step shows that).
#[allow(dead_code)] // used by Shots and Frames; a build without them leaves it unused
pub fn bible_line(missing: usize, what: &str) -> String {
    match missing {
        0 => format!("The bible is not approved yet, so {what} may still change."),
        n => format!("The bible is not approved yet: {n} thing{} left, so {what} may still change.", if n == 1 { "" } else { "s" }),
    }
}

/// What the screens say when a save failed (the details go to the log).
#[allow(dead_code)] // used by Story and Shots; a build without them leaves it unused
pub const SAVE_FAILED: &str = "Your change could not be saved. Check that the project's drive is connected, then try again.";

/// The part of an error a person can read: the first sentence of its first line, when it names
/// no file, folder or internal id ("sc0004", "ep01"), else `None` (the details go to the log).
#[allow(dead_code)] // used by Story and Shots; a build without them leaves it unused
pub fn plain_reason(error: &str) -> Option<String> {
    let first = error.lines().map(str::trim).find(|l| !l.is_empty())?;
    // An error chain reads "context: cause: cause"; the outer context is the plainest part.
    let head = first.split(": ").next().unwrap_or(first).trim().trim_end_matches(['.', ':', ';', ',']).trim();
    if head.is_empty() || head.chars().count() > 160 {
        return None;
    }
    let technical = |w: &str| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        let digits = w.chars().filter(char::is_ascii_digit).count();
        let letters = w.chars().filter(|c| c.is_ascii_alphabetic()).count();
        // "sc0004", "ep01", "sh12": letters then a run of digits.
        let id = digits >= 2 && letters >= 2 && w.chars().take_while(|c| c.is_ascii_alphabetic()).count() == letters;
        id || w.contains('_')
    };
    if head.contains(['\\', '/', '{', '}', '`']) || head.contains(".json") || head.contains("os error") || head.split_whitespace().any(technical) {
        return None;
    }
    Some(format!("{head}."))
}

/// The "bible not approved" notice: one line with "Go to Bible" at its end.
#[allow(dead_code)] // used by Shots and Frames; a build without them leaves it unused
pub fn bible_notice(id: &'static str, missing: usize, what: &str, go: super::Go) -> gpui_kit::Div {
    use mv_ui::{btn, ph, ButtonKind};
    let action = btn(id, "Go to Bible", ButtonKind::Ghost).small().icon(ph::USERS).on_click(move |_, w, cx| go(super::Step::Bible, w, cx));
    crate::widgets::warning_strip(ph::WARNING, bible_line(missing, what), action)
}

#[cfg(test)]
mod tests {
    use super::{bible_line, plain_reason};

    #[test]
    fn errors_are_shown_only_when_plain() {
        assert_eq!(plain_reason("Claude's usage limit was reached: retry later").as_deref(), Some("Claude's usage limit was reached."));
        assert_eq!(plain_reason("Scene sc0004 is no longer in the story of episode ep01."), None);
        assert_eq!(plain_reason("The script could not be read: E:\\Manga VOX\\story.json: os error 2").as_deref(), Some("The script could not be read."));
        assert_eq!(plain_reason("could not open episodes/ep01/story.json"), None);
        assert_eq!(plain_reason("   "), None);
    }

    #[test]
    fn the_bible_notice_is_one_line_with_a_count() {
        assert_eq!(bible_line(16, "places and cast"), "The bible is not approved yet: 16 things left, so places and cast may still change.");
        assert_eq!(bible_line(1, "faces"), "The bible is not approved yet: 1 thing left, so faces may still change.");
        assert!(bible_line(0, "x").starts_with("The bible is not approved yet, so"));
    }
}
