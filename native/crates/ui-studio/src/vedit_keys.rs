//! Video-mode keyboard commands as data: id, label, group, default keys, plus user overrides in
//! `studio/vedit_keys.json`. Ported from bhippi-Video-editor (MIT) `keymap.ts`.
//! Keystrokes are GPUI-style lowercase strings (`ctrl-shift-z`, `left`, `space`).
#![allow(dead_code)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// Everything the keyboard can do in Video mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    PlayPause,
    ShuttleBack,
    ShuttlePause,
    ShuttleForward,
    FrameBack,
    FrameForward,
    Back5Frames,
    Forward5Frames,
    PrevEdit,
    NextEdit,
    GoStart,
    GoEnd,
    MarkIn,
    MarkOut,
    AddMarker,
    Split,
    RippleTrimStart,
    RippleTrimEnd,
    Delete,
    RippleDelete,
    Undo,
    Redo,
    NudgeLeft,
    NudgeRight,
    NudgeUp,
    NudgeDown,
    VolumeDown,
    VolumeUp,
    DefaultTransition,
    Export,
    SelectTool,
    RazorTool,
    ToggleSnap,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    PrevChapter,
    NextChapter,
    Shortcuts,
}

/// One row of the command table.
pub struct Entry {
    pub command: Command,
    /// Stable id, the key in the overrides file.
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub keys: &'static [&'static str],
}

macro_rules! e {
    ($c:ident, $id:expr, $label:expr, $group:expr, [$($k:expr),*]) => {
        Entry { command: Command::$c, id: $id, label: $label, group: $group, keys: &[$($k),*] }
    };
}

/// The command table with default shortcuts.
pub const COMMANDS: &[Entry] = &[
    e!(PlayPause, "play_pause", "Play / pause", "Playback", ["space"]),
    e!(ShuttleBack, "shuttle_back", "Play backward (repeat: 2x, 4x, 8x)", "Playback", ["j"]),
    e!(ShuttlePause, "shuttle_pause", "Pause", "Playback", ["k"]),
    e!(ShuttleForward, "shuttle_forward", "Play forward (repeat: 2x, 4x, 8x)", "Playback", ["l"]),
    e!(FrameBack, "frame_back", "Back one frame", "Playback", ["left"]),
    e!(FrameForward, "frame_forward", "Forward one frame", "Playback", ["right"]),
    e!(Back5Frames, "back_5", "Back five frames", "Playback", ["shift-left"]),
    e!(Forward5Frames, "forward_5", "Forward five frames", "Playback", ["shift-right"]),
    e!(PrevEdit, "prev_edit", "Previous edit point", "Playback", ["up"]),
    e!(NextEdit, "next_edit", "Next edit point", "Playback", ["down"]),
    e!(GoStart, "go_start", "Go to start", "Playback", ["home"]),
    e!(GoEnd, "go_end", "Go to end", "Playback", ["end"]),
    e!(MarkIn, "mark_in", "Mark in", "Marks", ["i"]),
    e!(MarkOut, "mark_out", "Mark out", "Marks", ["o"]),
    e!(AddMarker, "add_marker", "Add marker", "Marks", ["m"]),
    e!(Split, "split", "Split at playhead", "Editing", ["ctrl-k"]),
    e!(RippleTrimStart, "ripple_trim_start", "Ripple trim start to playhead", "Editing", ["q"]),
    e!(RippleTrimEnd, "ripple_trim_end", "Ripple trim end to playhead", "Editing", ["w"]),
    e!(Delete, "delete", "Delete", "Editing", ["delete", "backspace"]),
    e!(RippleDelete, "ripple_delete", "Ripple delete", "Editing", ["shift-delete", "shift-backspace"]),
    e!(Undo, "undo", "Undo", "Editing", ["ctrl-z"]),
    e!(Redo, "redo", "Redo", "Editing", ["ctrl-shift-z", "ctrl-y"]),
    e!(NudgeLeft, "nudge_left", "Nudge clip left", "Editing", ["alt-left"]),
    e!(NudgeRight, "nudge_right", "Nudge clip right", "Editing", ["alt-right"]),
    e!(NudgeUp, "nudge_up", "Move clip to the track above", "Editing", ["alt-up"]),
    e!(NudgeDown, "nudge_down", "Move clip to the track below", "Editing", ["alt-down"]),
    e!(VolumeDown, "volume_down", "Volume down 1 dB", "Audio", ["["]),
    e!(VolumeUp, "volume_up", "Volume up 1 dB", "Audio", ["]"]),
    e!(DefaultTransition, "default_transition", "Apply default transition", "Editing", ["ctrl-d"]),
    e!(Export, "export", "Export", "File", ["ctrl-m"]),
    e!(SelectTool, "select_tool", "Selection tool", "Tools", ["v"]),
    e!(RazorTool, "razor_tool", "Razor tool", "Tools", ["c"]),
    e!(ToggleSnap, "toggle_snap", "Snap on / off (hold Shift to invert while dragging)", "Tools", ["s"]),
    e!(ZoomIn, "zoom_in", "Zoom in", "View", ["=", "shift-="]),
    e!(ZoomOut, "zoom_out", "Zoom out", "View", ["-"]),
    e!(ZoomFit, "zoom_fit", "Zoom to the whole sequence", "View", ["\\"]),
    e!(PrevChapter, "prev_chapter", "Previous chapter (All chapters)", "View", ["pageup"]),
    e!(NextChapter, "next_chapter", "Next chapter (All chapters)", "View", ["pagedown"]),
    e!(Shortcuts, "shortcuts", "Keyboard shortcuts", "View", ["shift-/"]),
];

/// The keystroke a key event makes, in the table's form (`ctrl-shift-z`). A typed `?` (some
/// layouts report it unshifted) counts as `shift-/`.
pub fn stroke_of(key: &str, ctrl: bool, alt: bool, shift: bool, cmd: bool) -> String {
    if key == "?" {
        return "shift-/".into();
    }
    let mut s = String::new();
    for (on, m) in [(ctrl || cmd, "ctrl"), (alt, "alt"), (shift, "shift")] {
        if on {
            s.push_str(m);
            s.push('-');
        }
    }
    s.push_str(&key.to_lowercase());
    normalize(&s)
}

/// Playback speed for the `n`th consecutive J or L press: 1x, 2x, 4x, then 8x.
pub fn shuttle_speed(presses: u32) -> f32 {
    match presses {
        0 | 1 => 1.,
        2 => 2.,
        3 => 4.,
        _ => 8.,
    }
}

/// Canonical form of a keystroke: lowercase, modifiers ordered ctrl-alt-shift-cmd.
pub fn normalize(stroke: &str) -> String {
    let lower = stroke.trim().to_lowercase();
    let mut parts: Vec<&str> = lower.split('-').filter(|p| !p.is_empty()).collect();
    if lower.ends_with("--") || lower == "-" {
        parts.push("-");
    }
    let key = parts.pop().unwrap_or("");
    let mut out = String::new();
    for m in ["ctrl", "alt", "shift", "cmd"] {
        let has = parts.iter().any(|p| *p == m || (m == "cmd" && (*p == "meta" || *p == "super")) || (m == "ctrl" && *p == "control"));
        if has {
            out.push_str(m);
            out.push('-');
        }
    }
    out.push_str(key);
    out
}

/// Default keys plus the user's overrides.
pub struct Keymap {
    by_stroke: HashMap<String, Command>,
    keys: HashMap<&'static str, Vec<String>>,
}

impl Keymap {
    /// Builds a keymap; `overrides` maps a command id to its replacement keys (empty = unbound).
    pub fn new(overrides: &HashMap<String, Vec<String>>) -> Self {
        let mut keys: HashMap<&'static str, Vec<String>> = HashMap::new();
        for e in COMMANDS {
            let k = match overrides.get(e.id) {
                Some(o) => o.iter().map(|s| normalize(s)).collect(),
                None => e.keys.iter().map(|s| normalize(s)).collect(),
            };
            keys.insert(e.id, k);
        }
        let mut by_stroke = HashMap::new();
        // Defaults first, then overrides, so a user binding wins a clash.
        for pass_override in [false, true] {
            for e in COMMANDS {
                if overrides.contains_key(e.id) == pass_override {
                    for k in &keys[e.id] {
                        by_stroke.insert(k.clone(), e.command);
                    }
                }
            }
        }
        // A default key a user rebound elsewhere no longer reaches its old command.
        for e in COMMANDS {
            keys.get_mut(e.id).unwrap().retain(|k| by_stroke.get(k) == Some(&e.command));
        }
        Self { by_stroke, keys }
    }

    pub fn defaults() -> Self {
        Self::new(&HashMap::new())
    }

    pub fn lookup(&self, stroke: &str) -> Option<Command> {
        self.by_stroke.get(&normalize(stroke)).copied()
    }

    /// Keys currently bound to `id`.
    pub fn keys_for(&self, id: &str) -> &[String] {
        self.keys.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Rows for a shortcuts dialog: (group, label, keys as shown), in table order.
    pub fn help_rows(&self) -> Vec<(&'static str, &'static str, String)> {
        COMMANDS.iter().map(|e| (e.group, e.label, self.keys_for(e.id).iter().map(|k| display(k)).collect::<Vec<_>>().join(" / "))).collect()
    }
}

/// `ctrl-shift-z` -> `Ctrl+Shift+Z`.
pub fn display(stroke: &str) -> String {
    stroke
        .split('-')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) if p.len() > 1 || f.is_alphabetic() => f.to_uppercase().collect::<String>() + c.as_str(),
                _ => p.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

// ------------------------------------------------------------------ user overrides

pub fn overrides_path() -> PathBuf {
    mv_core::paths::studio().join("vedit_keys.json")
}

/// Reads an overrides file; missing or malformed gives none. Unknown ids are dropped.
pub fn load_overrides_from(path: &Path) -> HashMap<String, Vec<String>> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    let mut m: HashMap<String, Vec<String>> = serde_json::from_str(&text).unwrap_or_default();
    m.retain(|id, _| COMMANDS.iter().any(|e| e.id == id));
    m
}

pub fn save_overrides_to(path: &Path, overrides: &HashMap<String, Vec<String>>) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let sorted: std::collections::BTreeMap<_, _> = overrides.iter().collect();
    std::fs::write(path, serde_json::to_string_pretty(&sorted).unwrap_or_default())
}

fn global() -> &'static RwLock<Keymap> {
    static G: OnceLock<RwLock<Keymap>> = OnceLock::new();
    G.get_or_init(|| RwLock::new(Keymap::new(&load_overrides_from(&overrides_path()))))
}

/// The command bound to `stroke` in the user's keymap.
pub fn lookup(stroke: &str) -> Option<Command> {
    global().read().ok()?.lookup(stroke)
}

/// Rows for the shortcuts dialog using the user's keymap.
pub fn help_rows() -> Vec<(&'static str, &'static str, String)> {
    global().read().map(|k| k.help_rows()).unwrap_or_default()
}

/// Rebinds `id` to `keys` (empty unbinds), saving to disk and updating the live keymap.
pub fn set_binding(id: &str, keys: Vec<String>) -> std::io::Result<()> {
    let path = overrides_path();
    let mut o = load_overrides_from(&path);
    o.insert(id.to_string(), keys);
    save_overrides_to(&path, &o)?;
    if let Ok(mut g) = global().write() {
        *g = Keymap::new(&o);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve() {
        let k = Keymap::defaults();
        assert_eq!(k.lookup("space"), Some(Command::PlayPause));
        assert_eq!(k.lookup("Ctrl-K"), Some(Command::Split));
        assert_eq!(k.lookup("shift-ctrl-z"), Some(Command::Redo));
        assert_eq!(k.lookup("ctrl-y"), Some(Command::Redo));
        assert_eq!(k.lookup("]"), Some(Command::VolumeUp));
        assert_eq!(k.lookup("f12"), None);
    }

    #[test]
    fn no_default_clashes_and_unique_ids() {
        let mut seen = HashMap::new();
        for e in COMMANDS {
            for k in e.keys {
                assert_eq!(normalize(k), *k, "{k} not canonical");
                assert!(seen.insert(*k, e.id).is_none(), "{k} bound twice");
            }
        }
        let mut ids: Vec<_> = COMMANDS.iter().map(|e| e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), COMMANDS.len());
    }

    #[test]
    fn override_replaces_and_steals() {
        let mut o = HashMap::new();
        o.insert("split".to_string(), vec!["s".to_string()]);
        o.insert("add_marker".to_string(), vec!["Ctrl-K".to_string()]);
        let k = Keymap::new(&o);
        assert_eq!(k.lookup("s"), Some(Command::Split));
        assert_eq!(k.lookup("ctrl-k"), Some(Command::AddMarker));
        assert_eq!(k.lookup("m"), None);
        assert_eq!(k.keys_for("split"), ["s"]);
    }

    #[test]
    fn overrides_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("vedit_keys.json");
        assert!(load_overrides_from(&p).is_empty());
        let mut o = HashMap::new();
        o.insert("undo".to_string(), vec!["ctrl-u".to_string()]);
        o.insert("bogus".to_string(), vec!["x".to_string()]);
        save_overrides_to(&p, &o).unwrap();
        let back = load_overrides_from(&p);
        assert_eq!(back.len(), 1);
        assert_eq!(Keymap::new(&back).lookup("ctrl-u"), Some(Command::Undo));
    }

    #[test]
    fn key_events_map_to_commands() {
        let k = Keymap::defaults();
        let look = |key: &str, ctrl, alt, shift| k.lookup(&stroke_of(key, ctrl, alt, shift, false));
        assert_eq!(look("k", true, false, false), Some(Command::Split));
        assert_eq!(look("K", false, false, false), Some(Command::ShuttlePause));
        assert_eq!(look("delete", false, false, true), Some(Command::RippleDelete));
        assert_eq!(look("z", true, false, true), Some(Command::Redo));
        assert_eq!(look("left", false, true, false), Some(Command::NudgeLeft));
        assert_eq!(look("s", false, false, false), Some(Command::ToggleSnap));
        assert_eq!(look("?", false, false, true), Some(Command::Shortcuts));
        assert_eq!(look("/", false, false, true), Some(Command::Shortcuts));
        assert_eq!(look("-", false, false, false), Some(Command::ZoomOut));
        assert_eq!(look("\\", false, false, false), Some(Command::ZoomFit));
        assert_eq!(k.lookup(&stroke_of("m", true, false, false, true)), Some(Command::Export));
    }

    #[test]
    fn help_and_shuttle() {
        let rows = Keymap::defaults().help_rows();
        assert_eq!(rows.len(), COMMANDS.len());
        assert!(rows.iter().any(|r| r.1 == "Redo" && r.2 == "Ctrl+Shift+Z / Ctrl+Y"));
        assert_eq!(display("space"), "Space");
        assert_eq!(display("["), "[");
        assert_eq!([1, 2, 3, 4, 9].map(shuttle_speed), [1., 2., 4., 8., 8.]);
    }
}
