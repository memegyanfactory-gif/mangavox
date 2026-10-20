//! First-run setup (`Route::Welcome`): a short, skippable, resumable welcome that asks what the
//! user will do, then offers manga sources (built in, repositories, plugin links), streaming
//! addons and narration voices. Nothing is switched on or downloaded until the user picks it;
//! "Start empty" and finishing with nothing chosen leave the same clean app.
//!
//! When it shows is decided by `mv_core::onboarding` (once per start, in `main`). Progress is
//! saved in `config.json` under `onboarding`, so quitting part-way resumes at that screen.
use gpui_kit::*;
use mv_ui::Route;

mod cards;
pub mod plan;
mod view;

pub use view::WelcomeView;

/// The view for `Route::Welcome`; `None` for every other route.
pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Welcome => Some(cx.new(|cx| WelcomeView::new(window, cx)).into()),
        _ => None,
    }
}

/// Whether the app opens on the welcome setup at this start (see `mv_core::onboarding::init`).
pub fn shows_at_start() -> bool {
    mv_core::onboarding::launch().is_some_and(|d| d.shows_welcome())
}
