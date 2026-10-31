//! The shell's map of the app: sections, their sidebar destinations and which routes are pages,
//! details or full-screen. Icons come from `mv_ui::glyphs`, so one meaning has one glyph on every
//! page (Apple HIG SF Symbols: <https://developer.apple.com/design/human-interface-guidelines/sf-symbols>).
use mv_ui::{glyphs, Ph, Route};

use crate::keys::Shortcut;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Section {
    Manga,
    Anime,
    Movies,
}

pub const SECTIONS: [Section; 3] = [Section::Manga, Section::Anime, Section::Movies];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Manga => "Manga",
            Section::Anime => "Anime",
            Section::Movies => "Movies",
        }
    }

    pub fn home(self) -> Route {
        match self {
            Section::Manga => Route::Library,
            Section::Anime => Route::AnimeHome,
            Section::Movies => Route::MoviesHome,
        }
    }

    pub fn icon(self) -> Ph {
        match self {
            Section::Manga => glyphs::MANGA,
            Section::Anime => glyphs::ANIME,
            Section::Movies => glyphs::MOVIES,
        }
    }

    pub fn shortcut(self) -> Shortcut {
        match self {
            Section::Manga => Shortcut::Manga,
            Section::Anime => Shortcut::Anime,
            Section::Movies => Shortcut::Movies,
        }
    }

    pub fn index(self) -> usize {
        SECTIONS.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// One sidebar destination.
pub struct NavEntry {
    pub route: Route,
    pub label: &'static str,
    pub icon: Ph,
    /// Extra words the command palette matches.
    pub keywords: &'static [&'static str],
}

fn entry(route: Route, label: &'static str, icon: Ph, keywords: &'static [&'static str]) -> NavEntry {
    NavEntry { route, label, icon, keywords }
}

pub fn nav_for(section: Section) -> Vec<NavEntry> {
    match section {
        Section::Manga => vec![
            entry(Route::Browse, "Browse", glyphs::BROWSE, &["search", "sources", "find", "explore"]),
            entry(Route::Library, "Library", glyphs::LIBRARY, &["saved", "collection", "home"]),
            entry(Route::MangaHistory, "History", glyphs::HISTORY, &["recent", "read", "continue"]),
            entry(Route::MangaDownloads, "Downloads", glyphs::DOWNLOADS, &["offline", "queue"]),
        ],
        Section::Anime => vec![
            entry(Route::AnimeHome, "Home", glyphs::HOME, &["trending", "seasonal", "watch"]),
            entry(Route::Discover, "Discover", glyphs::DISCOVER, &["explore", "genres", "mood"]),
            entry(Route::MediaHistory, "History", glyphs::HISTORY, &["recent", "watched", "continue"]),
            entry(Route::Favorites, "Favorites", glyphs::FAVORITES, &["starred", "watchlist"]),
        ],
        Section::Movies => vec![
            entry(Route::MoviesHome, "Home", glyphs::HOME, &["popular", "series", "watch"]),
            entry(Route::Discover, "Discover", glyphs::DISCOVER, &["explore", "genres", "mood"]),
            entry(Route::MediaHistory, "History", glyphs::HISTORY, &["recent", "watched", "continue"]),
            entry(Route::Favorites, "Favorites", glyphs::FAVORITES, &["starred", "watchlist"]),
        ],
    }
}

/// The label of a sidebar destination in `section`, if `route` is one.
pub fn nav_label(section: Section, route: &Route) -> Option<&'static str> {
    nav_for(section).into_iter().find(|e| &e.route == route).map(|e| e.label)
}

/// Studio and YouTube: wide pages of their own (no section, their own assistant panel), shown
/// only in Studio mode.
pub fn is_workspace(route: &Route) -> bool {
    matches!(route, Route::Studio | Route::YouTube | Route::Suggestions)
}

/// Full-screen routes hide the sidebar and dock (the welcome setup fills the window too).
pub fn is_focus(route: &Route) -> bool {
    matches!(route, Route::Reader { .. } | Route::Player { .. } | Route::Welcome)
}

/// Pages under a sidebar destination (they highlight the destination they were opened from).
pub fn is_detail(route: &Route) -> bool {
    matches!(route, Route::MangaDetail { .. } | Route::Reader { .. } | Route::AnimeDetail { .. } | Route::MovieDetail { .. } | Route::Player { .. })
}

/// Pages that stay alive once opened, so the sidebar brings back the same scroll position,
/// source, filters and loaded results instead of starting over.
pub fn keep_alive(route: &Route) -> bool {
    matches!(route, Route::Browse | Route::AnimeHome | Route::MoviesHome | Route::Discover)
}

/// Settings pages offered in the command palette, by `Route::Settings { tab }` id (the
/// canonical `Tab::id`). Labels, icons and search words come from ui-settings so a row always
/// matches its entry in the Settings rail.
pub const SETTINGS_TABS: &[&str] = &["profile", "mode", "themes", "playback", "audio", "sources", "addons", "providers", "models", "cli-tools", "storage", "system", "studio", "about"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_home_is_a_destination() {
        for s in SECTIONS {
            assert!(nav_for(s).iter().any(|e| e.route == s.home()), "{s:?}");
            assert_eq!(SECTIONS[s.index()], s);
        }
    }

    #[test]
    fn destinations_are_not_details() {
        for s in SECTIONS {
            for e in nav_for(s) {
                assert!(!is_detail(&e.route) && !is_focus(&e.route), "{}", e.label);
            }
        }
    }

    #[test]
    fn studio_and_youtube_are_workspaces() {
        assert!(is_workspace(&Route::Studio) && is_workspace(&Route::YouTube));
        for s in SECTIONS {
            for e in nav_for(s) {
                assert!(!is_workspace(&e.route), "{}", e.label);
            }
        }
    }

    #[test]
    fn settings_tabs_are_known_to_settings() {
        for id in SETTINGS_TABS {
            // Canonical ids, so the palette can mark the page the Settings view reports.
            assert_eq!(ui_settings::Tab::from_id(id).map(|t| t.id()), Some(*id), "{id}");
        }
    }
}
