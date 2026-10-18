//! Small helpers shared by the manga pages.
use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use mv_ui::{card_grid, skeleton_card, CoverSource, Tokens};

use crate::backend;

/// Row height of the virtualized cover grids (card 254 px + 22 px gap).
pub const ROW_H: f32 = 276.0;

/// Loading / ready / failed state of an async page section.
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

pub fn cover_src(map: &HashMap<String, PathBuf>, url: Option<&str>) -> Option<CoverSource> {
    url.and_then(|u| map.get(u)).cloned().map(CoverSource::File)
}

pub fn search_box(state: &Entity<InputState>) -> Div {
    div().w(px(260.)).child(Input::new(state).cleanable(true))
}

pub fn skeleton_grid(n: usize) -> Div {
    card_grid().children((0..n).map(|_| skeleton_card()))
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_size(px(mv_ui::size::SM)).text_color(Tokens::text_3()).child(text.into())
}

/// Download missing covers (6 at a time) into the shared cache, calling `store` to reach the
/// page's `url -> file` map, and repaint after each batch.
pub fn spawn_cover_downloads<T: 'static>(
    cx: &mut Context<T>,
    missing: Vec<(String, Option<String>)>,
    store: impl Fn(&mut T) -> &mut HashMap<String, PathBuf> + 'static,
) {
    if missing.is_empty() {
        return;
    }
    cx.spawn(async move |this, cx| {
        for chunk in missing.chunks(6) {
            let futs: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|(u, s)| {
                    let key = u.clone();
                    let f = mv_ui::bg::run(backend::ensure_cover(u, s));
                    async move { (key, f.await.ok().flatten()) }
                })
                .collect();
            let results = futures::future::join_all(futs).await;
            let alive = this.update(cx, |t, cx| {
                for (u, p) in results {
                    if let Some(p) = p {
                        store(t).insert(u, p);
                    }
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}
