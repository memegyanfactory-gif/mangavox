//! Stream lookup (installed addons, IMDb mapping for anime) and the source picker.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_media::stream_utils::stream_resource_id;
use mv_media::types::AnimeTorrentStream;
use mv_ui::nav::PlayRequest;
use mv_ui::{bg, button, navigate, size, ButtonKind, Route, Tokens};

use crate::common::{fmt_size, jikan, muted, registry};

pub const NO_ADDONS: &str = "No stream addons are installed. Add one in Settings, Addons.";

/// What is being played; becomes the `PlayRequest` once a source is chosen.
#[derive(Clone, Debug)]
pub struct PlayBase {
    /// Display title for the player, e.g. "Show - Episode 3".
    pub title: String,
    pub imdb_id: Option<String>,
    pub mal_id: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

#[derive(Default)]
pub enum StreamState {
    #[default]
    Idle,
    Searching(String),
    Picking {
        label: String,
        base: PlayBase,
        streams: Vec<AnimeTorrentStream>,
    },
    Failed(String),
}

fn playable_source(hash: &str) -> String {
    let h = hash.trim();
    if h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("magnet:?xt=urn:btih:{h}")
    } else {
        h.to_string()
    }
}

pub fn request_for(base: &PlayBase, s: &AnimeTorrentStream) -> PlayRequest {
    PlayRequest {
        title: base.title.clone(),
        source: playable_source(&s.info_hash),
        file_idx: s.file_idx.map(|i| i as usize),
        imdb_id: base.imdb_id.clone(),
        mal_id: base.mal_id.clone(),
        season: base.season,
        episode: base.episode,
    }
}

async fn lookup(mut base: PlayBase, movie: bool, kind: &'static str) -> Result<(PlayBase, Vec<AnimeTorrentStream>), String> {
    let reg = registry();
    if reg.stream_addons(kind).is_empty() {
        return Err(NO_ADDONS.into());
    }
    if base.imdb_id.is_none() {
        if let Some(mal) = base.mal_id.clone() {
            base.imdb_id = jikan()?.imdb_id(&mal).await;
        }
    }
    let imdb = base.imdb_id.clone().ok_or("This title has no IMDb mapping, so sources cannot be looked up.")?;
    let rid = stream_resource_id(&imdb, movie, base.season.unwrap_or(1), base.episode.unwrap_or(1)).ok_or("Invalid title id.")?;
    let streams = reg.find_streams(&rid, kind).await;
    Ok((base, streams))
}

/// Look up sources for `base`. With `autoplay` the best-ranked source starts playing at once;
/// otherwise the list is handed back for the page to show.
/// The caller sets `StreamState::Searching` first; `set` receives the outcome.
pub fn find<V: 'static>(cx: &mut Context<V>, window: &Window, label: String, base: PlayBase, movie: bool, autoplay: bool, set: fn(&mut V, StreamState)) {
    let kind = if movie { "movie" } else { "series" };
    cx.spawn_in(window, async move |this, cx| {
        let outcome = bg::run(lookup(base, movie, kind)).await.map_err(|e| e.to_string()).and_then(|r| r);
        let state = match outcome {
            Ok((_, s)) if s.is_empty() => StreamState::Failed("No playable sources were found for this title.".into()),
            Ok((base, streams)) => StreamState::Picking { label, base, streams },
            Err(e) => StreamState::Failed(e),
        };
        let _ = this.update_in(cx, |v, window, cx| {
            if let (true, StreamState::Picking { base, streams, .. }) = (autoplay, &state) {
                navigate(Route::Player { request: request_for(base, &streams[0]) }, window, cx);
            }
            set(v, state);
            cx.notify();
        });
    })
    .detach();
}

/// Picker / progress / error card for the current state (`None` when idle).
pub fn panel<V: 'static>(state: &StreamState, cx: &mut Context<V>, close: fn(&mut V)) -> Option<Div> {
    let shell = || div().flex().flex_col().gap(px(8.)).p(px(18.)).rounded(px(12.)).bg(Tokens::surface()).border_1().border_color(Tokens::border()).shadow_lg();
    let close_btn = |cx: &mut Context<V>| button("stream-close", "Close", ButtonKind::Ghost).on_click(cx.listener(move |this, _, _, cx| {
        close(this);
        cx.notify();
    }));
    match state {
        StreamState::Idle => None,
        StreamState::Searching(label) => Some(shell().child(muted(format!("Finding sources for {label}...")))),
        StreamState::Failed(msg) => {
            let no_addons = msg == NO_ADDONS;
            Some(
                shell()
                    .child(div().text_size(px(size::MD)).text_color(Tokens::text_1()).child(SharedString::from(msg.clone())))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .when(no_addons, |d| {
                                d.child(button("open-addons", "Open addons", ButtonKind::Primary).on_click(|_, w, cx| {
                                    navigate(Route::Settings { tab: Some("addons".into()) }, w, cx);
                                }))
                            })
                            .child(close_btn(cx)),
                    ),
            )
        }
        StreamState::Picking { label, base, streams } => {
            let rows = streams.iter().take(10).enumerate().map(|(i, s)| {
                let req = request_for(base, s);
                let quality = s.quality.clone().unwrap_or_default();
                let meta = [
                    s.source.clone(),
                    s.seeders.map(|n| format!("{n} seeds")),
                    s.size.map(fmt_size),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("  ·  ");
                div()
                    .id(("stream", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Tokens::hover()))
                    .child(
                        div()
                            .w(px(54.))
                            .flex_none()
                            .text_size(px(size::SM))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if i == 0 { Tokens::accent() } else { Tokens::text_2() })
                            .child(SharedString::from(if quality.is_empty() { "Auto".to_string() } else { quality })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(SharedString::from(s.title.replace('\n', " "))))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(meta))),
                    )
                    .when(i == 0, |d| {
                        d.child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::accent()).child("Best match"))
                    })
                    .on_click(move |_, w, cx| navigate(Route::Player { request: req.clone() }, w, cx))
            });
            Some(
                shell()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(format!("Choose a source for {label}"))))
                            .child(close_btn(cx)),
                    )
                    .children(rows),
            )
        }
    }
}

/// The best source already found for this season/episode, if any (lets Play start instantly).
pub fn best(state: &StreamState, season: Option<u32>, episode: Option<u32>) -> Option<PlayRequest> {
    match state {
        StreamState::Picking { base, streams, .. } if base.season == season && base.episode == episode => streams.first().map(|s| request_for(base, s)),
        _ => None,
    }
}

/// Inline "Sources" section for detail pages: loads by itself, every link plays with one click.
pub fn sources_section(state: &StreamState, accent: Rgba) -> Option<Div> {
    let head = |note: Option<String>| {
        div()
            .flex()
            .items_baseline()
            .gap(px(10.))
            .mb(px(10.))
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("Sources"))
            .when_some(note, |d, n| d.child(muted(n)))
    };
    let shell = || div().flex().flex_col();
    match state {
        StreamState::Idle => None,
        StreamState::Searching(label) => Some(
            shell()
                .child(head(Some(format!("Finding links for {label}..."))))
                .children((0..3).map(|_| div().h(px(56.)).mb(px(6.)).rounded(px(6.)).bg(Tokens::surface()))),
        ),
        StreamState::Failed(msg) => {
            let no_addons = msg == NO_ADDONS;
            Some(
                shell().child(head(None)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .p(px(14.))
                        .rounded(px(6.))
                        .bg(Tokens::surface())
                        .child(div().flex_1().text_size(px(size::MD)).text_color(Tokens::text_2()).child(SharedString::from(msg.clone())))
                        .when(no_addons, |d| {
                            d.child(button("open-addons", "Open addons", ButtonKind::Primary).on_click(|_, w, cx| navigate(Route::Settings { tab: Some("addons".into()) }, w, cx)))
                        }),
                ),
            )
        }
        StreamState::Picking { label, base, streams } => {
            let rows = streams.iter().take(12).enumerate().map(|(i, s)| {
                let req = request_for(base, s);
                let quality = s.quality.clone().filter(|q| !q.is_empty()).unwrap_or_else(|| "Auto".into());
                let meta = [s.source.clone(), s.size.map(fmt_size)].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
                let group: SharedString = format!("src-{i}").into();
                div()
                    .id(("source", i))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .px(px(14.))
                    .h(px(56.))
                    .mb(px(6.))
                    .rounded(px(6.))
                    .bg(Tokens::surface())
                    .cursor_pointer()
                    .hover(|d| d.bg(Tokens::surface_2()))
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(size::XS))
                            .font_weight(FontWeight::BOLD)
                            .map(|d| if i == 0 { d.bg(accent).text_color(rgb(0x000000)) } else { d.border_1().border_color(rgba(0xffffff40)).text_color(Tokens::text_1()) })
                            .child(SharedString::from(quality)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(SharedString::from(s.title.replace('\n', " "))))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(meta))),
                    )
                    .children(s.seeders.map(|n| {
                        let c = if n >= 20 { rgb(0x46d369) } else if n >= 5 { Tokens::warning() } else { Tokens::danger() };
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_2())
                            .child(div().size(px(7.)).rounded_full().bg(c))
                            .child(SharedString::from(format!("{n} seeds")))
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(px(32.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_1()
                            .border_color(rgba(0xffffff40))
                            .group_hover(group, |d| d.bg(rgba(0xffffff26)))
                            .child(mv_ui::icon("icons/play.svg", Tokens::text_1(), 14.)),
                    )
                    .on_click(move |_, w, cx| navigate(Route::Player { request: req.clone() }, w, cx))
            });
            let count = if streams.len() == 1 { "1 link".to_string() } else { format!("{} links", streams.len()) };
            Some(shell().child(head(Some(format!("{count} for {label}")))).children(rows))
        }
    }
}
