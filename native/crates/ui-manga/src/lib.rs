//! Manga pages: Library, Browse, History, Manga detail, Reader and the Settings -> Sources panel.
//! The shell calls [`open`] for every route; this crate returns `Some(view)` for the routes it owns.
use gpui_kit::*;
use mv_ui::Route;

mod assist;
pub mod backend;
mod browse;
mod categories;
mod detail;
mod discover;
mod downloads;
mod downloads_page;
mod history;
mod library;
pub mod logic;
mod narrator;
pub mod ops;
mod reader;
pub mod recap;
mod sources;
pub mod store;
pub mod tags;
mod ui;

pub use backend::set_source_ctx;
pub use downloads::{downloads, toast as download_toast};

/// Gives the assistant the manga tools (search, chapters, downloads, reader). Called once at startup.
pub fn register_assistant_tools() {
    assist::register();
}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Library => Some(cx.new(|cx| library::LibraryPage::new(window, cx)).into()),
        Route::Browse => Some(cx.new(|cx| browse::BrowsePage::new(window, cx)).into()),
        Route::MangaHistory => Some(cx.new(|cx| history::HistoryPage::new(window, cx)).into()),
        Route::MangaDownloads => Some(cx.new(|cx| downloads_page::DownloadsPage::new(window, cx)).into()),
        Route::MangaDetail { manga_id, .. } => {
            let id = manga_id.clone();
            Some(cx.new(|cx| detail::DetailPage::new(id, cx)).into())
        }
        Route::Reader { manga_id, chapter_id } => {
            let (m, c) = (manga_id.clone(), chapter_id.clone());
            Some(cx.new(|cx| reader::ReaderPage::new(m, c, window, cx)).into())
        }
        _ => None,
    }
}

/// Settings -> Sources panel (installed sources, plugins, enable/test, install from URL).
pub fn sources_panel(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| sources::SourcesPanel::new(window, cx)).into()
}
