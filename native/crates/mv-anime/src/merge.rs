//! Merging AI output into entities the user has edited (design 5.4). A field is Auto unless its
//! JSON pointer (or an ancestor's) is in the entity's `locked` set; card fields (`/frame`,
//! `/voice`, `/sheets`) are never touched by AI merges.
//!
//! Pointers follow RFC 6901 (`~` is written `~0`, `/` is written `~1`). Arrays of objects that
//! carry an `id` (or, for marks, a `character`) are addressed by that key, not by index:
//! `/marks/jinwoo/pos`, `/lines/ln0042/spoken`. Other arrays are addressed by index.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Pointers AI merges never write, whatever the locks say.
pub const CARD_FIELDS: [&str; 3] = ["/frame", "/voice", "/sheets"];

/// Field names that hold a card (or a list of cards) wherever they appear: AI merges never
/// write them, at any depth (a story merge reaches `/lines/<id>/voice`).
const CARD_NAMES: [&str; 6] = ["frame", "voice", "sheets", "sheet", "plates", "clip"];

/// The user's own bookkeeping: an AI merge never clears a lock, an edited mark or a review
/// status (an approved character stays approved).
const BOOKKEEPING: [&str; 3] = ["locked", "edited", "status"];

/// Root fields that identify the entity itself; the current value is always kept.
const ROOT_KEEP: [&str; 2] = ["id", "schema"];

/// Walks `ai` and writes into `current` every leaf whose pointer (and every ancestor pointer)
/// is not locked and not a card field.
///
/// Also never written: `locked`, `edited` and `status` at any depth, and the root `id` and `schema`.
/// Objects merge key by key (keys only in `current` stay). An array with no lock or card under
/// it is replaced whole by the AI's; one with locks under it merges element by element (by `id`
/// / `character` when every element has one, else by index), keeping locked elements the AI
/// left out.
pub fn merge_unlocked(current: &mut Value, ai: &Value, locked: &BTreeSet<String>) {
    merge_at(current, ai, "", locked);
}

/// `pointer` or one of its ancestors is in `locked`.
pub fn is_locked(pointer: &str, locked: &BTreeSet<String>) -> bool {
    if locked.contains(pointer) || locked.contains("") || locked.contains("/") {
        return true;
    }
    // Every proper ancestor: the prefix before each later '/'.
    pointer.char_indices().filter(|&(i, c)| c == '/' && i > 0).any(|(i, _)| locked.contains(&pointer[..i]))
}

/// Escapes one pointer segment (RFC 6901).
pub fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Any lock lies at or under `pointer` (so the value there cannot be replaced whole).
fn locked_under(pointer: &str, locked: &BTreeSet<String>) -> bool {
    let prefix = format!("{pointer}/");
    locked.iter().any(|l| l == pointer || l.starts_with(&prefix))
}

/// A card with the user's work in it lies under this value (takes, a selection, a lock, a
/// review status, the user's own prompt or references): such a value is never replaced whole or
/// dropped by an AI merge.
fn holds_cards(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.iter().any(|(k, x)| (CARD_NAMES.contains(&k.as_str()) && card_used(x)) || holds_cards(x)),
        Value::Array(a) => a.iter().any(holds_cards),
        _ => false,
    }
}

/// Some card (`Card`, `VoiceCard`, a sheet or plate holding one) in `v` carries the user's work.
fn card_used(v: &Value) -> bool {
    match v {
        Value::Object(m) => {
            let truthy = |k: &str| match m.get(k) {
                Some(Value::Array(a)) => !a.is_empty(),
                Some(Value::Bool(b)) => *b,
                Some(Value::String(s)) => k != "status" || !(s.is_empty() || s == "draft"),
                Some(Value::Null) | None => false,
                Some(_) => true,
            };
            ["takes", "selected", "locked", "status", "prompt_user", "refs_user"].iter().any(|k| truthy(k)) || m.values().any(card_used)
        }
        Value::Array(a) => a.iter().any(card_used),
        _ => false,
    }
}

fn skip_key(parent: &str, key: &str, pointer: &str, locked: &BTreeSet<String>) -> bool {
    CARD_FIELDS.contains(&pointer)
        || CARD_NAMES.contains(&key)
        || BOOKKEEPING.contains(&key)
        || (parent.is_empty() && ROOT_KEEP.contains(&key))
        || is_locked(pointer, locked)
}

fn merge_at(current: &mut Value, ai: &Value, pointer: &str, locked: &BTreeSet<String>) {
    match (current, ai) {
        (Value::Object(cur), Value::Object(new)) => merge_object(cur, new, pointer, locked),
        (cur @ Value::Array(_), Value::Array(new)) => {
            if !locked_under(pointer, locked) && !holds_cards(cur) {
                *cur = Value::Array(new.clone());
            } else if let Value::Array(items) = cur {
                merge_array(items, new, pointer, locked);
            }
        }
        (cur, new) => {
            // A leaf, or the AI changed the type. Keep what is ours when anything under it is
            // locked or holds a card; otherwise take the AI's value whole.
            if !locked_under(pointer, locked) && !holds_cards(cur) {
                *cur = new.clone();
            }
        }
    }
}

fn merge_object(cur: &mut Map<String, Value>, new: &Map<String, Value>, pointer: &str, locked: &BTreeSet<String>) {
    for (k, v) in new {
        let p = format!("{pointer}/{}", escape(k));
        if skip_key(pointer, k, &p, locked) {
            continue;
        }
        match cur.get_mut(k) {
            Some(slot) => merge_at(slot, v, &p, locked),
            None => {
                let mut fresh = v.clone();
                strip_cards(&mut fresh);
                cur.insert(k.clone(), fresh);
            }
        }
    }
}

/// The key an array element is addressed by: its `id`, else its `character` (marks).
fn element_key(v: &Value) -> Option<String> {
    let o = v.as_object()?;
    ["id", "character"].iter().find_map(|k| o.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
}

fn merge_array(cur: &mut Vec<Value>, new: &[Value], pointer: &str, locked: &BTreeSet<String>) {
    let cur_keys: Option<Vec<String>> = cur.iter().map(element_key).collect();
    let new_keys: Option<Vec<String>> = new.iter().map(element_key).collect();
    match (cur_keys, new_keys) {
        (Some(cur_keys), Some(new_keys)) => {
            let mut old: Vec<Option<Value>> = std::mem::take(cur).into_iter().map(Some).collect();
            let mut out = Vec::with_capacity(new.len());
            for (key, v) in new_keys.iter().zip(new) {
                let p = format!("{pointer}/{}", escape(key));
                let found = cur_keys.iter().position(|k| k == key).and_then(|i| old[i].take());
                match found {
                    Some(mut mine) => {
                        if !is_locked(&p, locked) {
                            merge_at(&mut mine, v, &p, locked);
                        }
                        out.push(mine);
                    }
                    None if is_locked(&p, locked) => {}
                    None => {
                        let mut fresh = v.clone();
                        strip_cards(&mut fresh);
                        out.push(fresh);
                    }
                }
            }
            // Ours the AI left out: kept when the user locked something in them or they hold a
            // card (pictures and voices are never dropped by an AI merge).
            for (key, mine) in cur_keys.iter().zip(old) {
                if let Some(mine) = mine {
                    let p = format!("{pointer}/{}", escape(key));
                    if locked_under(&p, locked) || holds_cards(&mine) {
                        out.push(mine);
                    }
                }
            }
            *cur = out;
        }
        _ => {
            // By index.
            for (i, v) in new.iter().enumerate() {
                let p = format!("{pointer}/{i}");
                match cur.get_mut(i) {
                    Some(mine) => {
                        if !is_locked(&p, locked) {
                            merge_at(mine, v, &p, locked);
                        }
                    }
                    None => {
                        let mut fresh = v.clone();
                        strip_cards(&mut fresh);
                        cur.push(fresh);
                    }
                }
            }
            // Ours past the AI's end: kept only when locked or holding a card. Each is judged
            // by its own index before anything moves (a lock names the original position).
            let end = new.len();
            let mut i = 0usize;
            cur.retain(|mine| {
                let at = i;
                i += 1;
                at < end || locked_under(&format!("{pointer}/{at}"), locked) || holds_cards(mine)
            });
        }
    }
}

/// A value the AI adds new never brings card contents with it: they start empty (default).
fn strip_cards(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| !CARD_NAMES.contains(&k.as_str()) && !BOOKKEEPING.contains(&k.as_str()));
            m.values_mut().for_each(strip_cards);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_cards),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_locked_element_past_the_ai_end_survives_by_its_own_index() {
        let mut cur = json!({ "keep": ["a", "b", "c", "d", "e"] });
        let ai = json!({ "keep": ["x", "y"] });
        let locked: BTreeSet<String> = ["/keep/4".to_string()].into_iter().collect();
        merge_unlocked(&mut cur, &ai, &locked);
        assert_eq!(cur, json!({ "keep": ["x", "y", "e"] }));
    }
}
