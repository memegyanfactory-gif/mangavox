//! Stream lookup (installed addons, IMDb mapping for anime) and the source picker.
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_media::episode_art;
use mv_media::stream_utils::stream_resource_id;
use mv_media::types::AnimeTorrentStream;
use mv_ui::nav::PlayRequest;
use mv_ui::{bg, button, navigate, size, ButtonKind, Route, Tokens};

use crate::common::{fmt_size, jikan, muted, registry, STREAMS_TTL};

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
    let mal = base.mal_id.clone();
    if base.imdb_id.is_none() {
        if let Some(mal) = &mal {
            base.imdb_id = jikan()?.imdb_id(mal).await;
        }
    }
    let (season, episode) = (base.season.unwrap_or(1), base.episode.unwrap_or(1));
    let mut ids = vec![];
    if let Some(imdb) = &base.imdb_id {
        // Anime count episodes from 1 across the show; IMDb may split them into seasons.
        let (s, e) = match &mal {
            Some(_) if !movie => episode_art::imdb_episode(imdb, episode).await.unwrap_or((season, episode)),
            _ => (season, episode),
        };
        ids.extend(stream_resource_id(imdb, movie, s, e));
    }
    // Kitsu ids number anime episodes like MAL, and anime addons index far more releases by them.
    if let Some(mal) = &mal {
        if let Some(k) = episode_art::kitsu_id(mal).await {
            ids.push(if movie { format!("kitsu:{k}") } else { format!("kitsu:{k}:{episode}") });
        }
    }
    if ids.is_empty() {
        return Err("This title has no IMDb or Kitsu mapping, so sources cannot be looked up.".into());
    }
    let streams = reg.find_streams_any(&ids, kind).await;
    Ok((base, streams))
}

/// Cache key for a title's links; includes the enabled addons so installing one refetches.
fn stream_key(base: &PlayBase, kind: &str) -> String {
    let addons: Vec<String> = registry().stream_addons(kind).into_iter().map(|m| format!("{}@{}", m.id, m.version)).collect();
    let id = base.imdb_id.clone().or_else(|| base.mal_id.as_ref().map(|m| format!("mal{m}"))).unwrap_or_default();
    format!("streams2:{kind}:{id}:{}:{}:{}", base.season.unwrap_or(0), base.episode.unwrap_or(0), addons.join(","))
}

/// Bumped by every lookup, so a background refresh never replaces a newer request's links.
static LOOKUPS: AtomicU64 = AtomicU64::new(0);

/// Look up sources for `base`. With `autoplay` the best-ranked source starts playing at once;
/// otherwise the list is handed back for the page to show.
/// Links cached on disk are shown at once and refreshed in the background when older than
/// `STREAMS_TTL`. The caller sets `StreamState::Searching` first; `set` receives the outcome.
pub fn find<V: 'static>(cx: &mut Context<V>, window: &Window, label: String, base: PlayBase, movie: bool, autoplay: bool, set: fn(&mut V, StreamState)) {
    let kind = if movie { "movie" } else { "series" };
    let key = stream_key(&base, kind);
    let ticket = LOOKUPS.fetch_add(1, Ordering::Relaxed) + 1;
    let hit = DataCache::shared().get::<(Option<String>, Vec<AnimeTorrentStream>)>(&key).filter(|((_, s), _)| !s.is_empty());
    cx.spawn_in(window, async move |this, cx| {
        let shown = hit.is_some();
        if let Some(((imdb, streams), age)) = hit {
            let mut b = base.clone();
            b.imdb_id = b.imdb_id.or(imdb);
            let _ = this.update_in(cx, |v, window, cx| {
                let best = request_for(&b, &streams[0]);
                if autoplay {
                    navigate(Route::Player { request: best }, window, cx);
                } else {
                    ui_player::prefetch(&best);
                }
                set(v, StreamState::Picking { label: label.clone(), base: b, streams });
                cx.notify();
            });
            if age < STREAMS_TTL {
                return;
            }
        }
        let outcome = bg::run(async move {
            let r = lookup(base, movie, kind).await;
            if let Ok((b, s)) = &r {
                DataCache::shared().put(&key, &(&b.imdb_id, s));
            }
            r
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        if shown {
            // Silent refresh: replace the cached links only with a newer, non-empty list.
            let Some((base, streams)) = outcome.ok().filter(|(_, s)| !s.is_empty()) else { return };
            if LOOKUPS.load(Ordering::Relaxed) == ticket {
                let _ = this.update(cx, |v, cx| {
                    set(v, StreamState::Picking { label, base, streams });
                    cx.notify();
                });
            }
            return;
        }
        let state = match outcome {
            Ok((_, s)) if s.is_empty() => StreamState::Failed("No playable sources were found for this title.".into()),
            Ok((base, streams)) => StreamState::Picking { label, base, streams },
            Err(e) => StreamState::Failed(e),
        };
        let _ = this.update_in(cx, |v, window, cx| {
            if let StreamState::Picking { base, streams, .. } = &state {
                let best = request_for(base, &streams[0]);
                if autoplay {
                    navigate(Route::Player { request: best }, window, cx);
                } else {
                    // Warm the best link so Play starts almost instantly.
                    ui_player::prefetch(&best);
                }
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

/// A stream title split for display: release name lines, tag text ("Dual Audio") and
/// country codes taken from flag emoji (Windows can't draw flag emoji, so we show images).
struct Described {
    names: Vec<String>,
    tags: String,
    flags: Vec<String>,
}

fn describe(title: &str) -> Described {
    let (mut names, mut tags, mut flags) = (vec![], vec![], vec![]);
    for line in title.lines().map(str::trim).filter(|l| !l.is_empty()) {
        // The seeders / size / source line: we already have those as fields.
        if line.contains('👤') || line.contains('💾') || line.contains('⚙') {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut rest = String::new();
        let mut i = 0;
        let mut had_flag = false;
        while i < chars.len() {
            let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
            if ri(chars[i]) && i + 1 < chars.len() && ri(chars[i + 1]) {
                let cc: String = [chars[i], chars[i + 1]].iter().map(|c| (b'a' + (*c as u32 - 0x1F1E6) as u8) as char).collect();
                if !flags.contains(&cc) {
                    flags.push(cc);
                }
                had_flag = true;
                i += 2;
            } else {
                rest.push(chars[i]);
                i += 1;
            }
        }
        let rest = rest.trim().trim_matches(|c: char| c == '/' || c == '|' || c.is_whitespace()).to_string();
        let tagish = had_flag || ["Dub", "Dual", "Multi", "Sub"].iter().any(|t| rest.starts_with(t));
        if tagish {
            if !rest.is_empty() {
                tags.push(rest);
            }
        } else if names.len() < 2 {
            names.push(rest);
        }
    }
    if flags.is_empty() {
        flags = name_langs(title);
    }
    Described { names, tags: tags.join(" / "), flags }
}

/// Language tag in release names, its flag's country code and display name.
const LANG_TAGS: &[(&[&str], &str, &str)] = &[
    (&["eng", "english"], "gb", "English"),
    (&["jap", "jpn", "japanese"], "jp", "Japanese"),
    (&["ita", "italian"], "it", "Italian"),
    (&["spa", "esp", "spanish", "castellano"], "es", "Spanish"),
    (&["lat", "latino"], "mx", "Latino"),
    (&["fre", "fra", "french", "vf", "vff", "vostfr", "truefrench"], "fr", "French"),
    (&["ger", "deu", "german"], "de", "German"),
    (&["hin", "hindi"], "in", "Hindi"),
    (&["tam", "tamil"], "in", "Tamil"),
    (&["rus", "russian"], "ru", "Russian"),
    (&["por", "portuguese", "ptbr"], "pt", "Portuguese"),
    (&["kor", "korean"], "kr", "Korean"),
    (&["chi", "chs", "cht", "chinese"], "cn", "Chinese"),
    (&["ara", "arabic"], "sa", "Arabic"),
    (&["tur", "turkish"], "tr", "Turkish"),
    (&["pol", "polish"], "pl", "Polish"),
];

/// Country codes for language tags in a release name ("Show.ITA.ENG.1080p"), for addons
/// that give no flag emoji.
fn name_langs(title: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for word in title.split(|c: char| !c.is_ascii_alphanumeric()).map(str::to_ascii_lowercase) {
        if let Some((_, cc, _)) = LANG_TAGS.iter().find(|(tags, _, _)| tags.contains(&word.as_str())) {
            if !out.iter().any(|c| c == cc) {
                out.push(cc.to_string());
            }
        }
    }
    out
}

/// Language name for a flag country code (flag emoji use countries, not languages).
fn lang_name(cc: &str) -> String {
    match cc {
        "gb" | "us" => "English".into(),
        "es" => "Spanish".into(),
        "br" => "Portuguese (BR)".into(),
        "cz" => "Czech".into(),
        "ua" => "Ukrainian".into(),
        "nl" => "Dutch".into(),
        "se" => "Swedish".into(),
        "gr" => "Greek".into(),
        "il" => "Hebrew".into(),
        "vn" => "Vietnamese".into(),
        "th" => "Thai".into(),
        "id" => "Indonesian".into(),
        "hu" => "Hungarian".into(),
        "ro" => "Romanian".into(),
        _ => LANG_TAGS.iter().find(|(_, c, _)| *c == cc).map(|(_, _, n)| n.to_string()).unwrap_or_else(|| cc.to_uppercase()),
    }
}

fn flag(cc: &str) -> Img {
    // English releases often use the UK flag code "gb"; flagcdn knows both.
    img(SharedString::from(format!("https://flagcdn.com/w40/{cc}.png"))).w(px(20.)).h(px(14.)).rounded(px(2.)).object_fit(ObjectFit::Cover)
}

/// Stremio-style sources panel: addon + quality on the left, release name, seeders / size /
/// source and language flags on the right; chips filter by addon. Every row plays on click.
pub fn sources_panel<V: 'static>(
    state: &StreamState,
    accent: Rgba,
    heading: String,
    filter: Option<&str>,
    cx: &mut Context<V>,
    set_filter: fn(&mut V, Option<String>),
) -> Option<Stateful<Div>> {
    let shell = div().id("sources-panel").flex().flex_col().size_full().overflow_y_scroll().p(px(16.)).rounded(px(12.)).bg(rgba(0x0b0d10d9)).border_1().border_color(Tokens::border());
    let head = |note: String| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .mb(px(12.))
            .child(div().text_size(px(size::LG)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(heading.clone())))
            .child(muted(note))
    };
    match state {
        StreamState::Idle => None,
        StreamState::Searching(_) => Some(
            shell.child(head("Finding links from your addons...".into())).children((0..5).map(|i| {
                div().h(px(74.)).mb(px(8.)).rounded(px(8.)).bg(Tokens::surface()).opacity(1.0 - i as f32 * 0.15)
            })),
        ),
        StreamState::Failed(msg) => {
            let no_addons = msg == NO_ADDONS;
            Some(
                shell.child(head("No links".into())).child(div().text_size(px(size::MD)).text_color(Tokens::text_2()).mb(px(12.)).child(SharedString::from(msg.clone()))).when(no_addons, |d| {
                    d.child(div().child(button("open-addons", "Open addons", ButtonKind::Primary).on_click(|_, w, cx| navigate(Route::Settings { tab: Some("addons".into()) }, w, cx))))
                }),
            )
        }
        StreamState::Picking { base, streams, .. } => {
            let mut addons: Vec<String> = streams.iter().filter_map(|s| s.addon.clone()).collect();
            addons.sort();
            addons.dedup();
            // Languages by how many links carry them, most first.
            let described: Vec<Vec<String>> = streams.iter().map(|s| describe(&s.title).flags).collect();
            let mut langs: Vec<(String, usize)> = vec![];
            for cc in described.iter().flatten() {
                match langs.iter_mut().find(|(c, _)| c == cc) {
                    Some((_, n)) => *n += 1,
                    None => langs.push((cc.clone(), 1)),
                }
            }
            langs.sort_by(|a, b| b.1.cmp(&a.1));
            let chips = (addons.len() > 1 || langs.len() > 1).then(|| {
                let chip = |id: usize, label: String, cc: Option<String>, active: bool, value: Option<String>, cx: &mut Context<V>| {
                    div()
                        .id(("source-chip", id))
                        .px(px(10.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(999.))
                        .text_size(px(size::XS))
                        .cursor_pointer()
                        .map(|d| if active { d.bg(Tokens::text_1()).text_color(rgb(0x000000)) } else { d.bg(Tokens::surface_2()).text_color(Tokens::text_2()).hover(|s| s.text_color(Tokens::text_1())) })
                        .children(cc.map(|c| flag(&c).w(px(16.)).h(px(11.))))
                        .child(SharedString::from(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            set_filter(this, value.clone());
                            cx.notify();
                        }))
                };
                let mut row = div().flex().flex_wrap().gap(px(6.)).mb(px(12.)).child(chip(0, "All".into(), None, filter.is_none(), None, cx));
                if addons.len() > 1 {
                    for (i, a) in addons.iter().enumerate() {
                        row = row.child(chip(1 + i, a.clone(), None, filter == Some(a.as_str()), Some(a.clone()), cx));
                    }
                }
                if langs.len() > 1 {
                    for (i, (cc, _)) in langs.iter().take(12).enumerate() {
                        let value = format!("lang:{cc}");
                        row = row.child(chip(100 + i, lang_name(cc), Some(cc.clone()), filter == Some(value.as_str()), Some(value), cx));
                    }
                }
                row
            });
            let shown: Vec<&AnimeTorrentStream> = streams
                .iter()
                .zip(&described)
                .filter(|(s, langs)| match filter {
                    None => true,
                    Some(f) => match f.strip_prefix("lang:") {
                        Some(cc) => langs.iter().any(|l| l == cc),
                        None => s.addon.as_deref() == Some(f),
                    },
                })
                .map(|(s, _)| s)
                .collect();
            let rows = shown.iter().take(80).enumerate().map(|(i, s)| {
                let req = request_for(base, s);
                let d = describe(&s.title);
                let quality = s.quality.clone().filter(|q| !q.is_empty()).unwrap_or_else(|| "Auto".into());
                let best = i == 0 && filter.is_none();
                div()
                    .id(("source", i))
                    .flex()
                    .gap(px(14.))
                    .px(px(12.))
                    .py(px(10.))
                    .mb(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(best, |x| x.bg(rgba(0xffffff0f)).border_1().border_color(accent))
                    .hover(|x| x.bg(rgba(0xffffff1a)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(84.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(s.addon.clone().unwrap_or_else(|| "Stream".into()))))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(if best { accent } else { Tokens::text_2() }).child(SharedString::from(quality)))
                            .when(best, |x| x.child(div().text_size(px(size::XS)).text_color(accent).child("Best match"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .children(d.names.into_iter().map(|n| div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(SharedString::from(n))))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_2())
                                    .children(s.seeders.map(|n| {
                                        let c = if n >= 20 { rgb(0x46d369) } else if n >= 5 { Tokens::warning() } else { Tokens::danger() };
                                        div().flex().items_center().gap(px(4.)).child(mv_ui::icon("icons/user.svg", c, 12.)).child(SharedString::from(n.to_string()))
                                    }))
                                    .children(s.size.map(|b| div().flex().items_center().gap(px(4.)).child(mv_ui::icon("icons/hard-drive.svg", Tokens::text_3(), 12.)).child(SharedString::from(fmt_size(b)))))
                                    .children(s.source.clone().map(|src| div().flex().items_center().gap(px(4.)).min_w_0().child(mv_ui::icon("icons/globe.svg", Tokens::text_3(), 12.)).child(div().truncate().child(SharedString::from(src))))),
                            )
                            .when(!d.flags.is_empty() || !d.tags.is_empty(), |x| {
                                x.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .children(d.flags.iter().take(8).map(|cc| flag(cc)))
                                        .when(!d.tags.is_empty(), |x| x.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(SharedString::from(d.tags.clone())))),
                                )
                            }),
                    )
                    .on_click(move |_, w, cx| navigate(Route::Player { request: req.clone() }, w, cx))
            });
            let count = if shown.len() == 1 { "1 link".to_string() } else { format!("{} links", shown.len()) };
            Some(shell.child(head(count)).children(chips).children(rows))
        }
    }
}
