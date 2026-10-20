//! Discover: pick a mood, browse anime (Jikan) or movies / series (Cinemeta) for it.
use gpui_kit::*;
use mv_media::cinemeta::CinemetaCatalog;
use mv_media::types::AnimeSearchResult;
use mv_ui::{empty_view, error_view, filter_chip, glyphs, muted, navigate, page, page_title, ph, segmented, Ph};

use crate::anime_home::genre_key;
use crate::common::{fetch_cached, jikan, result_route, subline, Load, DAY};
use crate::look;
use crate::movies_home::{catalog, catalog_key};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Anime,
    Movie,
    Series,
}

const KINDS: [(Kind, &str, Ph); 3] = [(Kind::Anime, "Anime", glyphs::ANIME), (Kind::Movie, "Movies", glyphs::MOVIES), (Kind::Series, "Series", ph::MONITOR_PLAY)];

/// (label, glyph, MyAnimeList genre id, Cinemeta genre). Each mood has a glyph so the chips scan
/// at a glance (Material 3 filter chips with leading icons).
const MOODS: &[(&str, Ph, u32, &str)] = &[
    ("Epic", ph::MOUNTAINS, 2, "Adventure"),
    ("Hyped", ph::LIGHTNING, 1, "Action"),
    ("Funny", ph::SMILEY, 4, "Comedy"),
    ("Emotional", ph::MASK_SAD, 8, "Drama"),
    ("Romantic", ph::HEART, 22, "Romance"),
    ("Magical", ph::STAR_FOUR, 10, "Fantasy"),
    ("Mind-bending", ph::BRAIN, 24, "Sci-Fi"),
    ("Tense", ph::HEARTBEAT, 41, "Thriller"),
    ("Spooky", ph::GHOST, 14, "Horror"),
    ("Mysterious", ph::DETECTIVE, 7, "Mystery"),
];

/// Poster width in the results grid.
const CARD_W: f32 = 170.;

pub struct Discover {
    kind: Kind,
    mood: usize,
    results: Load<Vec<AnimeSearchResult>>,
}

impl Discover {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut s = Self { kind: Kind::Anime, mood: 0, results: Load::Loading };
        s.load(cx);
        s
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (kind, mood) = (self.kind, self.mood);
        let (_, _, mal, genre) = MOODS[mood];
        let apply = move |v: &mut Self, l: Load<Vec<AnimeSearchResult>>| {
            if v.kind == kind && v.mood == mood {
                v.results = l;
            }
        };
        self.results = match kind {
            Kind::Anime => fetch_cached(cx, genre_key(mal), DAY, async move { jikan()?.anime_by_genre(mal, 1, None).await.map_err(|e| e.to_string()) }, apply),
            Kind::Movie => fetch_cached(cx, catalog_key(true, CinemetaCatalog::Top, Some(genre)), DAY, catalog(true, CinemetaCatalog::Top, Some(genre), 1), apply),
            Kind::Series => fetch_cached(cx, catalog_key(false, CinemetaCatalog::Top, Some(genre)), DAY, catalog(false, CinemetaCatalog::Top, Some(genre), 1), apply),
        };
    }

    fn results(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.results {
            Load::Loading => look::skeleton_posters(12, CARD_W).into_any_element(),
            Load::Failed(e) => error_view(e.clone())
                .title("Picks could not be loaded")
                .retry(cx.listener(|this, _, _, cx| {
                    this.load(cx);
                    cx.notify();
                }))
                .into_any_element(),
            Load::Ready(v) if v.is_empty() => empty_view(ph::MAGNIFYING_GLASS_MINUS, "Nothing for this mood yet", "Try another mood, or switch between anime, movies and series.").into_any_element(),
            Load::Ready(v) => div()
                .flex()
                .flex_wrap()
                .gap_x(px(16.))
                .gap_y(px(28.))
                .children(v.iter().enumerate().map(|(i, r)| {
                    let route = result_route(r);
                    let anime = self.kind == Kind::Anime;
                    let cover = if anime { r.cover_url.clone() } else { r.cover_url.as_deref().map(look::hi_res) };
                    let sub = if anime { subline(r) } else { r.year.map(|y| y.to_string()).unwrap_or_default() };
                    look::poster_card(("discover", i), CARD_W, cover, &r.title, sub, r.score, r.description.as_deref().map(look::blurb), None).on_click(move |_, w, cx| navigate(route.clone(), w, cx))
                }))
                .into_any_element(),
        }
    }
}

impl Render for Discover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind_ix = KINDS.iter().position(|k| k.0 == self.kind).unwrap_or(0);
        let picks = format!(
            "{} for \"{}\"",
            match self.kind {
                Kind::Anime => "Top-rated anime",
                Kind::Movie => "Top movies",
                Kind::Series => "Top series",
            },
            MOODS[self.mood].0
        );
        page("discover")
            .child(page_title("Discover").icon(glyphs::DISCOVER).subtitle("What are you in the mood for?"))
            .child(
                div().mb(px(16.)).child(segmented("discover-kind", KINDS.iter().map(|(_, label, glyph)| (Some(*glyph), *label)), kind_ix).on_change(cx.listener(|this, ix: &usize, _, cx| {
                    if let Some((k, _, _)) = KINDS.get(*ix) {
                        this.kind = *k;
                        this.load(cx);
                        cx.notify();
                    }
                }))),
            )
            .child(div().flex().flex_wrap().gap(px(8.)).mb(px(24.)).children(MOODS.iter().enumerate().map(|(i, (label, glyph, _, _))| {
                filter_chip(("mood", i), *label, self.mood == i).icon(*glyph).on_click(cx.listener(move |this, _, _, cx| {
                    if this.mood != i {
                        this.mood = i;
                        this.load(cx);
                        cx.notify();
                    }
                }))
            })))
            .child(div().mb(px(14.)).child(muted(picks)))
            .child(self.results(cx))
    }
}
