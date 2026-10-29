//! Stream lookup (installed addons, IMDb mapping for anime) and the source picker.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::cache::DataCache;
use mv_media::episode_art;
use mv_media::stream_utils::stream_resource_id;
use mv_media::types::AnimeTorrentStream;
use mv_ui::nav::PlayRequest;
use mv_ui::theme::{icon as isz, radius};
use mv_ui::{
    bg, btn, disclosure, empty_view, error_view, filter_chip, fmt_bytes, glyphs, icon, icon_button, icon_tag, muted, navigate, ph, size, skeleton, spinner, tag,
    tip, ButtonKind, EmptyState, Interactive, Ph, Route, Tokens, Tone,
};

use crate::common::{jikan, registry, STREAMS_TTL};
use crate::look;

pub const NO_ADDONS: &str = "No stream addons are installed. Add one in Settings, Addons.";
pub(crate) const NOT_FOUND: &str = "No playable sources were found for this title.";
const NO_MAPPING: &str = "This title has no IMDb or Kitsu mapping, so sources cannot be looked up.";

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

fn last_key(imdb: &str) -> String {
    format!("media:last-played:{imdb}")
}

/// Season and episode of a series last started from these pages. The history table keeps only
/// the episode number, so this is how Continue Watching and Resume know the season.
pub fn last_played(imdb: &str) -> Option<(u32, u32)> {
    DataCache::shared().get::<(u32, u32)>(&last_key(imdb)).map(|(v, _)| v)
}

/// Opens the player on `req`, remembering a series' season and episode first.
pub fn play_now(req: PlayRequest, window: &mut Window, cx: &mut App) {
    if req.mal_id.is_none() {
        if let (Some(imdb), Some(s), Some(e)) = (req.imdb_id.clone(), req.season, req.episode) {
            bg::runtime().spawn_blocking(move || DataCache::shared().put(&last_key(&imdb), &(s, e)));
        }
    }
    navigate(Route::Player { request: req }, window, cx);
}

pub(crate) async fn lookup(mut base: PlayBase, movie: bool, kind: &'static str) -> Result<(PlayBase, Vec<AnimeTorrentStream>), String> {
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
        return Err(NO_MAPPING.into());
    }
    let streams = reg.find_streams_any(&ids, kind).await;
    Ok((base, streams))
}

/// Cache key for a title's links; includes the enabled addons so installing one refetches.
pub(crate) fn stream_key(base: &PlayBase, kind: &str) -> String {
    let addons: Vec<String> = registry().stream_addons(kind).into_iter().map(|m| format!("{}@{}", m.id, m.version)).collect();
    let id = base.imdb_id.clone().or_else(|| base.mal_id.as_ref().map(|m| format!("mal{m}"))).unwrap_or_default();
    format!("streams2:{kind}:{id}:{}:{}:{}", base.season.unwrap_or(0), base.episode.unwrap_or(0), addons.join(","))
}

/// Look up sources for `base`. With `autoplay` the best-ranked source starts playing at once;
/// otherwise the list is handed back for the page to show.
/// Links cached on disk are shown at once and refreshed in the background when older than
/// `STREAMS_TTL`. The caller sets `StreamState::Searching` first; `set` receives the outcome.
///
/// The page keeps the returned task: replacing it (a newer lookup) or dropping it (Cancel,
/// closing the page) cancels this one, so a stale lookup never opens the player or replaces
/// newer links.
pub fn find<V: 'static>(cx: &mut Context<V>, window: &Window, label: String, base: PlayBase, movie: bool, autoplay: bool, set: fn(&mut V, StreamState)) -> Task<()> {
    let kind = if movie { "movie" } else { "series" };
    let key = stream_key(&base, kind);
    let hit = DataCache::shared().get::<(Option<String>, Vec<AnimeTorrentStream>)>(&key).filter(|((_, s), _)| !s.is_empty());
    cx.spawn_in(window, async move |this, cx| {
        let shown = hit.is_some();
        if let Some(((imdb, streams), age)) = hit {
            let mut b = base.clone();
            b.imdb_id = b.imdb_id.or(imdb);
            let _ = this.update_in(cx, |v, window, cx| {
                let best = request_for(&b, &streams[0]);
                if autoplay {
                    play_now(best, window, cx);
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
            let _ = this.update(cx, |v, cx| {
                set(v, StreamState::Picking { label, base, streams });
                cx.notify();
            });
            return;
        }
        let state = match outcome {
            Ok((_, s)) if s.is_empty() => StreamState::Failed(NOT_FOUND.into()),
            Ok((base, streams)) => StreamState::Picking { label, base, streams },
            Err(e) => StreamState::Failed(e),
        };
        let _ = this.update_in(cx, |v, window, cx| {
            if let StreamState::Picking { base, streams, .. } = &state {
                let best = request_for(base, &streams[0]);
                if autoplay {
                    play_now(best, window, cx);
                } else {
                    // Warm the best link so Play starts almost instantly.
                    ui_player::prefetch(&best);
                }
            }
            set(v, state);
            cx.notify();
        });
    })
}

/// The best source already found for this season/episode, if any (lets Play start instantly).
pub fn best(state: &StreamState, season: Option<u32>, episode: Option<u32>) -> Option<PlayRequest> {
    match state {
        StreamState::Picking { base, streams, .. } if base.season == season && base.episode == episode => streams.first().map(|s| request_for(base, s)),
        _ => None,
    }
}

/// Quality the found links actually offer ("4K", "HDR"), best first; empty until links are
/// known, so the hero never claims a quality the sources cannot back.
pub fn quality_tags(state: &StreamState) -> Vec<&'static str> {
    let StreamState::Picking { streams, .. } = state else { return vec![] };
    let words: Vec<Vec<String>> = streams
        .iter()
        .map(|s| format!("{} {}", s.quality.as_deref().unwrap_or(""), s.title).to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect())
        .collect();
    let any = |f: &dyn Fn(&str) -> bool| words.iter().any(|w| w.iter().any(|x| f(x)));
    let res = if any(&|w| w == "2160p" || w == "4k" || w == "uhd") {
        Some("4K")
    } else if any(&|w| w == "1080p") {
        Some("1080p")
    } else if any(&|w| w == "720p") {
        Some("720p")
    } else {
        None
    };
    let hdr = any(&|w| w.starts_with("hdr") || w == "dv" || w == "dovi");
    res.into_iter().chain(hdr.then_some("HDR")).collect()
}

// ---------- shared pieces of the two pickers ----------

/// Seeder health in three steps, each with its own glyph, word and tooltip, so it never rests on
/// colour alone (WCAG 1.4.1).
fn seed_health(n: u64) -> (Ph, Rgba, Option<&'static str>, &'static str) {
    match n {
        20.. => (ph::USERS, Tokens::success(), None, "Plenty of seeders: should start quickly"),
        5..=19 => (ph::USERS, Tokens::warning(), Some("slow"), "Few seeders: may start slowly"),
        _ => (ph::WARNING, Tokens::danger(), Some("few"), "Very few seeders: may not start"),
    }
}

/// Installed addons' logos by lower-case name, for the source rows.
fn addon_logos() -> Vec<(String, String)> {
    registry().list().into_iter().filter_map(|a| Some((a.manifest.name.to_lowercase(), a.manifest.icon.filter(|u| u.starts_with("http"))?))).filter(|(n, _)| !n.is_empty()).collect()
}

/// The logo of the addon a link came from. Links carry the addon's display name, often with a
/// suffix ("Torrentio 4K"), so the match is by prefix, on a whole word: a bare prefix gave a link
/// named "HD" or "Stream" the logo of "HDHub" or "StreamFusion". No match shows the name alone.
fn logo_for(logos: &[(String, String)], addon: &str) -> Option<String> {
    let a = addon.trim().to_lowercase();
    let word_prefix = |long: &str, short: &str| !short.is_empty() && long.strip_prefix(short).is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric()));
    logos.iter().find(|(n, _)| word_prefix(&a, n) || word_prefix(n, &a)).map(|(_, u)| u.clone())
}

fn meta_item(glyph: Ph, text: impl Into<SharedString>) -> Div {
    div().flex().flex_none().items_center().gap(px(4.)).child(icon(glyph.bold(), Tokens::text_3(), isz::XS)).child(text.into())
}

/// One playable link. The whole row plays; the play glyph says so, and the best match gets the
/// tonal accent container.
fn source_row(key: (&'static str, usize), s: &AnimeTorrentStream, base: &PlayBase, best: bool, logos: &[(String, String)]) -> Stateful<Div> {
    let req = request_for(base, s);
    let d = describe(&s.title);
    let langs = lang_names(&d.flags);
    let quality = s.quality.clone().filter(|q| !q.is_empty()).unwrap_or_else(|| "Auto".into());
    // Only the link's own addon name finds a logo, never the "Stream" placeholder.
    let logo = s.addon.as_deref().and_then(|a| logo_for(logos, a)).map(|u| div().flex_none().size(px(isz::MD)).rounded(px(radius::XS)).overflow_hidden().child(look::pic_small(u, glyphs::SOURCES, isz::XS)));
    let addon = s.addon.clone().unwrap_or_else(|| "Stream".into());
    let group: SharedString = format!("src-{}-{}", key.0, key.1).into();
    let seeds = s.seeders.map(|n| {
        let (glyph, color, word, hint) = seed_health(n);
        div()
            .id("seeds")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .text_color(color)
            .tooltip(tip(hint))
            .child(icon(glyph.bold(), color, isz::XS))
            .child(n.to_string())
            .when_some(word, |d, w| d.child(w))
    });
    let lang_tag = (!langs.is_empty()).then(|| {
        let summary = match langs.len() {
            1..=2 => langs.join(", "),
            n => format!("{}, {} +{}", langs[0], langs[1], n - 2),
        };
        div().id("langs").flex_none().tooltip(tip(langs.join(", "))).child(icon_tag(glyphs::LANGUAGE, summary, Tone::Neutral))
    });
    div()
        .id(key)
        .group(group.clone())
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(12.))
        .py(px(10.))
        .mb(px(6.))
        .rounded(px(radius::MD))
        .border_1()
        .role(Role::Button)
        .aria_label(SharedString::from(format!("Play {quality} from {addon}")))
        .map(|d| if best { d.bg(Tokens::accent_soft()).border_color(Tokens::accent_border()) } else { d.border_color(Tokens::base().alpha(0.)) })
        .pressable()
        .hover(move |x| x.bg(if best { Tokens::accent_a(0.22) } else { Tokens::hover() }))
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(4.))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .children(logo)
                        .child(div().min_w_0().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(addon))),
                )
                .child(tag(quality, if best { Tone::Accent } else { Tone::Neutral }))
                .when(best, |x| x.child(div().flex().items_center().gap(px(4.)).text_size(px(size::XS)).text_color(Tokens::accent()).child(icon(ph::SEAL_CHECK.fill(), Tokens::accent(), isz::XS)).child("Best match"))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(d.names.into_iter().map(|n| div().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(SharedString::from(n))))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(px(12.))
                        .gap_y(px(4.))
                        .text_size(px(size::XS))
                        .text_color(Tokens::text_2())
                        .children(seeds)
                        .children(s.size.map(|b| meta_item(ph::HARD_DRIVE, fmt_bytes(b))))
                        .children(s.source.clone().map(|src| meta_item(ph::GLOBE_SIMPLE, src)))
                        .children(lang_tag)
                        .when(!d.tags.is_empty(), |x| x.child(div().min_w_0().text_color(Tokens::text_3()).truncate().child(SharedString::from(d.tags.clone())))),
                ),
        )
        .child(icon(ph::PLAY.fill(), if best { Tokens::accent() } else { Tokens::text_3() }, isz::MD).group_hover(group, |s| s.text_color(Tokens::accent())))
        .on_click(move |_, w, cx| play_now(req.clone(), w, cx))
}

/// Closes the source overlay on a home page (it gets the window to hand focus back).
type CloseModal<V> = fn(&mut V, &mut Window, &mut Context<V>);

/// What went wrong, in words the user can act on, with the way forward (NN/g error messages).
/// Unknown errors keep the technical text behind a "Details" disclosure. `modal` is the overlay's
/// close on the home pages: leaving for Settings closes it first (those pages are kept alive, so
/// it would still be open on return), and only there may "Open addons" be the primary button
/// (a detail page already has its Play as the one primary).
fn failure<V: 'static>(msg: &str, cx: &mut Context<V>, retry: Option<fn(&mut V, &mut Window, &mut Context<V>)>, modal: Option<CloseModal<V>>) -> AnyElement {
    let open_addons = |label: &'static str, kind: ButtonKind, cx: &mut Context<V>| {
        btn("open-addons", label, kind).icon(glyphs::SOURCES).on_click(cx.listener(move |this, _, w, cx| {
            if let Some(close) = modal {
                close(this, w, cx);
                cx.notify();
            }
            navigate(Route::Settings { tab: Some("addons".into()) }, w, cx);
        }))
    };
    let with_retry = |e: EmptyState, cx: &mut Context<V>| match retry {
        Some(f) => e.secondary(btn("sources-retry", "Try again", ButtonKind::Ghost).icon(glyphs::RETRY).on_click(cx.listener(move |this, _, w, cx| f(this, w, cx)))),
        None => e,
    };
    let first_kind = if modal.is_some() { ButtonKind::Primary } else { ButtonKind::Secondary };
    match msg {
        NO_ADDONS => empty_view(glyphs::SOURCES, "No stream addons yet", "Install a Stremio addon in Settings to find sources for movies, series and anime.")
            .compact()
            .action(open_addons("Open addons", first_kind, cx))
            .into_any_element(),
        NOT_FOUND => {
            let manage = open_addons("Manage addons", ButtonKind::Secondary, cx);
            with_retry(empty_view(ph::MAGNIFYING_GLASS_MINUS, "No sources found", "Your addons have no links for this yet. Try again later or add another addon.").compact().action(manage), cx).into_any_element()
        }
        NO_MAPPING => empty_view(ph::LINK_BREAK, "Sources can't be looked up", "This title has no IMDb or Kitsu id, which addons need to find links.").compact().into_any_element(),
        raw => div()
            .flex()
            .flex_col()
            .items_center()
            .child(with_retry(error_view("Your addons did not answer. Check your connection, then try again.").title("Could not load sources").compact(), cx))
            .child(div().w_full().max_w(px(420.)).child(disclosure("sources-error-details", "Details").content(muted(raw.to_string())))
            )
            .into_any_element(),
    }
}

/// Picker / progress / error card for the overlay on the home pages (`None` when idle).
/// `close` closes the overlay (and hands focus back); `retry` runs the same lookup again.
pub fn panel<V: 'static>(state: &StreamState, cx: &mut Context<V>, close: CloseModal<V>, retry: fn(&mut V, &mut Window, &mut Context<V>)) -> Option<AnyElement> {
    let shell = || div().id("stream-panel").flex().flex_col().gap(px(14.)).p(px(20.)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border_strong()).shadow_lg();
    let close_x = |cx: &mut Context<V>| {
        icon_button("stream-close", glyphs::CLOSE, "Close").kbd("escape").on_click(cx.listener(move |this, _, w, cx| {
            close(this, w, cx);
            cx.notify();
        }))
    };
    let header = |lead: AnyElement, title: &str, sub: String, cx: &mut Context<V>| {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(lead)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(SharedString::from(title.to_string())))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).truncate().child(SharedString::from(sub))),
            )
            .child(close_x(cx))
    };
    match state {
        StreamState::Idle => None,
        StreamState::Searching(label) => Some(
            shell()
                .child(header(spinner("stream-spin", Tokens::accent(), isz::LG).into_any_element(), "Finding sources", label.clone(), cx))
                .children((0..3).map(|_| skeleton(relative(1.), px(58.), radius::MD)))
                .child(
                    div().flex().justify_end().child(btn("stream-cancel", "Cancel", ButtonKind::Ghost).on_click(cx.listener(move |this, _, w, cx| {
                        close(this, w, cx);
                        cx.notify();
                    }))),
                )
                .into_any_element(),
        ),
        StreamState::Failed(msg) => Some(shell().child(div().flex().justify_end().child(close_x(cx))).child(failure(msg, cx, Some(retry), Some(close))).into_any_element()),
        StreamState::Picking { label, base, streams } => Some(
            shell()
                .child(header(icon(glyphs::PLAY.regular(), Tokens::accent(), isz::LG).into_any_element(), "Choose a source", label.clone(), cx))
                .child(div().flex().flex_col().children({
                    let logos = addon_logos();
                    streams.iter().take(10).enumerate().map(|(i, s)| source_row(("stream", i), s, base, i == 0, &logos)).collect::<Vec<_>>()
                }))
                .into_any_element(),
        ),
    }
}

/// A stream title split for display: release name lines, tag text ("Dual Audio") and
/// country codes taken from flag emoji (Windows can't draw flag emoji, so we name the language).
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

/// Language names for a link's country codes, without repeats ("gb" and "us" are both English).
fn lang_names(codes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for name in codes.iter().map(|c| lang_name(c)) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Stremio-style sources panel: addon + quality on the left, release name, seeders / size /
/// source and languages on the right; filter chips by addon and language. Every row plays on
/// click. `close` adds a close button (the inline panel under an episode); `retry` runs the
/// lookup again after a failure.
#[allow(clippy::too_many_arguments)]
pub fn sources_panel<V: 'static>(
    state: &StreamState,
    heading: String,
    filter: Option<&str>,
    cx: &mut Context<V>,
    set_filter: fn(&mut V, Option<String>),
    close: Option<fn(&mut V)>,
    retry: fn(&mut V, &mut Window, &mut Context<V>),
) -> Option<Stateful<Div>> {
    let shell = div().id("sources-panel").flex().flex_col().size_full().overflow_y_scroll().p(px(16.)).rounded(px(radius::LG)).bg(Tokens::glass_strong()).border_1().border_color(Tokens::glass_border()).shadow_lg();
    let head = |note: AnyElement, cx: &mut Context<V>| {
        div()
            .flex()
            .items_start()
            .gap(px(8.))
            .mb(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(SharedString::from(heading.clone())))
                    .child(note),
            )
            .when_some(close, |d, f| {
                d.child(icon_button("sources-close", glyphs::CLOSE, "Close sources").small().on_click(cx.listener(move |this, _, _, cx| {
                    f(this);
                    cx.notify();
                })))
            })
    };
    match state {
        StreamState::Idle => None,
        StreamState::Searching(_) => {
            let note = div().flex().items_center().gap(px(8.)).child(spinner("sources-spin", Tokens::accent(), isz::SM)).child(muted("Finding links from your addons…")).into_any_element();
            Some(shell.child(head(note, cx)).children((0..5).map(|_| div().mb(px(8.)).child(skeleton(relative(1.), px(64.), radius::MD)))))
        }
        StreamState::Failed(msg) => Some(shell.child(head(muted("No links").into_any_element(), cx)).child(failure(msg, cx, Some(retry), None))),
        StreamState::Picking { base, streams, .. } => {
            let mut addons: Vec<(String, usize)> = vec![];
            for a in streams.iter().filter_map(|s| s.addon.clone()) {
                match addons.iter_mut().find(|(n, _)| *n == a) {
                    Some((_, c)) => *c += 1,
                    None => addons.push((a, 1)),
                }
            }
            addons.sort_by(|a, b| a.0.cmp(&b.0));
            // Languages by how many links carry them, most first.
            let described: Vec<Vec<String>> = streams.iter().map(|s| lang_names(&describe(&s.title).flags)).collect();
            let mut langs: Vec<(String, usize)> = vec![];
            for name in described.iter().flatten() {
                match langs.iter_mut().find(|(n, _)| n == name) {
                    Some((_, c)) => *c += 1,
                    None => langs.push((name.clone(), 1)),
                }
            }
            langs.sort_by_key(|l| std::cmp::Reverse(l.1));
            let chips = (addons.len() > 1 || langs.len() > 1).then(|| {
                let chip = |i: usize, label: String, glyph: Option<Ph>, count: usize, value: Option<String>, cx: &mut Context<V>| {
                    let active = filter == value.as_deref();
                    filter_chip(("source-chip", i), label, active).count(count).when_some(glyph, |c, g| c.icon(g)).on_click(cx.listener(move |this, _, _, cx| {
                        set_filter(this, value.clone());
                        cx.notify();
                    }))
                };
                let mut row = div().flex().flex_wrap().gap(px(6.)).mb(px(12.)).child(chip(0, "All".into(), None, streams.len(), None, cx));
                if addons.len() > 1 {
                    for (i, (a, n)) in addons.iter().enumerate() {
                        row = row.child(chip(1 + i, a.clone(), Some(glyphs::SOURCES), *n, Some(a.clone()), cx));
                    }
                }
                if langs.len() > 1 {
                    for (i, (name, n)) in langs.iter().take(12).enumerate() {
                        row = row.child(chip(100 + i, name.clone(), Some(glyphs::LANGUAGE), *n, Some(format!("lang:{name}")), cx));
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
                        Some(name) => langs.iter().any(|l| l == name),
                        None => s.addon.as_deref() == Some(f),
                    },
                })
                .map(|(s, _)| s)
                .collect();
            let count = if shown.len() == 1 { "1 link".to_string() } else { format!("{} links", shown.len()) };
            let logos = addon_logos();
            let rows = shown.iter().take(80).enumerate().map(|(i, s)| source_row(("source", i), s, base, i == 0 && filter.is_none(), &logos));
            Some(shell.child(head(muted(count).into_any_element(), cx)).children(chips).children(rows))
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob exports its own `test` attribute.
    use super::{lang_names, logo_for, name_langs, ph, quality_tags, seed_health, AnimeTorrentStream, PlayBase, StreamState};

    #[test]
    fn addon_logo_matches_by_name_prefix() {
        let logos = vec![("torrentio".to_string(), "https://t/logo.png".to_string())];
        assert_eq!(logo_for(&logos, "Torrentio 4K").as_deref(), Some("https://t/logo.png"));
        assert_eq!(logo_for(&logos, "Comet"), None);
        // Whole words only, and a short link name does not take a longer addon's logo.
        let more = vec![("streamfusion".to_string(), "https://s/logo.png".to_string()), ("torrentio rd".to_string(), "https://r/logo.png".to_string())];
        assert_eq!(logo_for(&more, "Stream"), None);
        assert_eq!(logo_for(&more, "StreamFusionX"), None);
        assert_eq!(logo_for(&more, "Torrentio").as_deref(), Some("https://r/logo.png"));
        assert_eq!(logo_for(&more, ""), None);
    }

    fn stream(quality: &str, title: &str) -> AnimeTorrentStream {
        AnimeTorrentStream { quality: Some(quality.into()), title: title.into(), ..Default::default() }
    }

    #[test]
    fn quality_tags_come_from_the_links() {
        let base = PlayBase { title: String::new(), imdb_id: None, mal_id: None, season: None, episode: None };
        let picking = |s: Vec<AnimeTorrentStream>| StreamState::Picking { label: String::new(), base: base.clone(), streams: s };
        assert!(quality_tags(&StreamState::Idle).is_empty());
        assert_eq!(quality_tags(&picking(vec![stream("1080p", "Show.1080p.WEB")])), vec!["1080p"]);
        assert_eq!(quality_tags(&picking(vec![stream("720p", "a"), stream("4k", "Show.2160p.DV.HDR10")])), vec!["4K", "HDR"]);
        assert!(quality_tags(&picking(vec![stream("", "Show.CAM")])).is_empty());
    }

    #[test]
    fn languages_are_named_once() {
        assert_eq!(lang_names(&["gb".into(), "us".into(), "jp".into()]), vec!["English".to_string(), "Japanese".to_string()]);
        assert_eq!(name_langs("Show.ITA.ENG.1080p"), vec!["it".to_string(), "gb".to_string()]);
    }

    #[test]
    fn seed_health_changes_shape_not_just_colour() {
        assert!(seed_health(50).2.is_none());
        assert_eq!(seed_health(7).2, Some("slow"));
        assert_eq!(seed_health(1).0, ph::WARNING);
    }
}
