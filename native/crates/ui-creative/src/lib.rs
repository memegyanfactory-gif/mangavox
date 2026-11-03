//! Creative: make original manga with AI (see `mv-create` for the engine). The shell calls
//! [`open`] for every route; this crate returns `Some(view)` for `Route::Creative`.
use gpui_kit::*;
use mv_ui::Route;

mod ai;
mod cast;
mod chat;
mod jobs;
mod manga;
mod motion;
mod page;

/// The one Creative page of the app. Every visit shows the same page, so a paint queue or an AI
/// answer still running from an earlier visit is never raced by a second copy of the project.
struct Shared(Entity<page::CreativePage>);
impl Global for Shared {}

pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Creative => {
            if let Some(Shared(e)) = cx.try_global::<Shared>() {
                return Some(e.clone().into());
            }
            let e = cx.new(|cx| page::CreativePage::new(window, cx));
            cx.set_global(Shared(e.clone()));
            Some(e.into())
        }
        _ => None,
    }
}
