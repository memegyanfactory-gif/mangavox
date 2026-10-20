//! App-wide keyboard shortcuts.
//!
//! The actions are bound globally so the command palette and tooltips can show each shortcut
//! next to its command. The shell runs them from a keystroke observer rather than from action
//! listeners, because GPUI only routes an action along the focused element's path, and with the
//! kit a mouse click no longer moves focus: an observer works whether or not anything is focused.
//! Shortcut set after Linear and Raycast (Ctrl+K), Windows (Alt+Left/Right, F11) and Fluent
//! NavigationView (toggle pane): <https://learn.microsoft.com/en-us/windows/apps/design/input/keyboard-accelerators>
use gpui_kit::*;

actions!(
    mangavox,
    [CommandPalette, ToggleSidebar, ToggleAssistant, OpenSettings, ShowManga, ShowAnime, ShowMovies, GoBack, GoForward, ToggleFullscreen]
);

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Palette,
    Sidebar,
    Assistant,
    Settings,
    Manga,
    Anime,
    Movies,
    Back,
    Forward,
    Fullscreen,
}

/// The one table: key, what it does. `bindings()` and `shortcut_for()` both read it.
pub const SHORTCUTS: &[(&str, Shortcut)] = &[
    ("ctrl-k", Shortcut::Palette),
    ("ctrl-b", Shortcut::Sidebar),
    ("ctrl-j", Shortcut::Assistant),
    ("ctrl-,", Shortcut::Settings),
    ("ctrl-1", Shortcut::Manga),
    ("ctrl-2", Shortcut::Anime),
    ("ctrl-3", Shortcut::Movies),
    ("alt-left", Shortcut::Back),
    ("alt-right", Shortcut::Forward),
    ("f11", Shortcut::Fullscreen),
];

impl Shortcut {
    pub fn action(self) -> Box<dyn Action> {
        match self {
            Shortcut::Palette => Box::new(CommandPalette),
            Shortcut::Sidebar => Box::new(ToggleSidebar),
            Shortcut::Assistant => Box::new(ToggleAssistant),
            Shortcut::Settings => Box::new(OpenSettings),
            Shortcut::Manga => Box::new(ShowManga),
            Shortcut::Anime => Box::new(ShowAnime),
            Shortcut::Movies => Box::new(ShowMovies),
            Shortcut::Back => Box::new(GoBack),
            Shortcut::Forward => Box::new(GoForward),
            Shortcut::Fullscreen => Box::new(ToggleFullscreen),
        }
    }

    /// The key as `kbd()` / `tip_kbd()` take it, e.g. `"ctrl-b"`.
    pub fn keys(self) -> &'static str {
        SHORTCUTS.iter().find(|(_, s)| *s == self).map(|(k, _)| *k).unwrap_or("")
    }
}

/// Registers the bindings (call once at startup, after `gpui_kit::init`).
pub fn bind(cx: &mut App) {
    cx.bind_keys(SHORTCUTS.iter().map(|(keys, s)| binding(keys, *s)));
}

fn binding(keys: &str, s: Shortcut) -> KeyBinding {
    match s {
        Shortcut::Palette => KeyBinding::new(keys, CommandPalette, None),
        Shortcut::Sidebar => KeyBinding::new(keys, ToggleSidebar, None),
        Shortcut::Assistant => KeyBinding::new(keys, ToggleAssistant, None),
        Shortcut::Settings => KeyBinding::new(keys, OpenSettings, None),
        Shortcut::Manga => KeyBinding::new(keys, ShowManga, None),
        Shortcut::Anime => KeyBinding::new(keys, ShowAnime, None),
        Shortcut::Movies => KeyBinding::new(keys, ShowMovies, None),
        Shortcut::Back => KeyBinding::new(keys, GoBack, None),
        Shortcut::Forward => KeyBinding::new(keys, GoForward, None),
        Shortcut::Fullscreen => KeyBinding::new(keys, ToggleFullscreen, None),
    }
}

/// The shortcut a keystroke stands for, if any (exact modifiers, so Ctrl+Shift+K is not Ctrl+K).
pub fn shortcut_for(k: &Keystroke) -> Option<Shortcut> {
    SHORTCUTS.iter().find_map(|(keys, s)| {
        let want = Keystroke::parse(keys).ok()?;
        (want.modifiers == k.modifiers && want.key.eq_ignore_ascii_case(&k.key)).then_some(*s)
    })
}

/// Escape with no modifiers.
pub fn is_escape(k: &Keystroke) -> bool {
    k.key == "escape" && !k.modifiers.modified()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::{shortcut_for, SHORTCUTS};
    use gpui_kit::Keystroke;

    #[test]
    fn every_shortcut_parses_and_round_trips() {
        for (keys, s) in SHORTCUTS {
            let k = Keystroke::parse(keys).unwrap_or_else(|_| panic!("{keys} does not parse"));
            assert_eq!(shortcut_for(&k), Some(*s), "{keys}");
            assert_eq!(s.keys(), *keys);
        }
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let k = Keystroke::parse("ctrl-shift-k").unwrap();
        assert_eq!(shortcut_for(&k), None);
        let plain = Keystroke::parse("k").unwrap();
        assert_eq!(shortcut_for(&plain), None);
    }

    #[test]
    fn keys_are_unique() {
        for (i, (a, _)) in SHORTCUTS.iter().enumerate() {
            assert!(SHORTCUTS.iter().skip(i + 1).all(|(b, _)| a != b), "{a} bound twice");
        }
    }
}
