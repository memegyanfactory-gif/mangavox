//! Discover: pick a mood, browse anime (Jikan) or movies / series (Cinemeta) for it.
use gpui_kit::*;
use mv_media::cinemeta::CinemetaCatalog;
use mv_media::types::AnimeSearchResult;
use mv_ui::{chip, page, page_header};

use crate::common::{fetch, grid, jikan, muted, Load};
use crate::movies_home::cinemeta;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Anime,
    Movie,
    Series,
}

/// (label, MyAnimeList genre id, Cinemeta genre)
const MOODS: &[(&str, u32, &str)] = &[
    ("Epic", 2, "Adventure"),
    ("Hyped", 1, "Action"),
    ("Funny", 4, "Comedy"),
    ("Emotional", 8, "Drama"),
    ("Romantic", 22, "Romance"),
    ("Magical", 10, "Fantasy"),
    ("Mind-bending", 24, "Sci-Fi"),
    ("Tense", 41, "Thriller"),
    ("Spooky", 14, "Horror"),
    ("Mysterious", 7, "Mystery"),
];

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
        self.results = Load::Loading;
        let (kind, mood) = (self.kind, self.mood);
        let (_, mal, genre) = MOODS[mood];
        let apply = move |v: &mut Self, l: Load<Vec<AnimeSearchResult>>| {
            if v.kind == kind && v.mood == mood {
                v.results = l;
            }
        };
        match kind {
            Kind::Anime => fetch(cx, async move { jikan()?.anime_by_genre(mal, 1, None).await.map_err(|e| e.to_string()) }, apply),
            Kind::Movie => fetch(cx, async move { cinemeta()?.movies_catalog(CinemetaCatalog::Top, 1, Some(genre)).await.map_err(|e| e.to_string()) }, apply),
            Kind::Series => fetch(cx, async move { cinemeta()?.series_catalog(CinemetaCatalog::Top, 1, Some(genre)).await.map_err(|e| e.to_string()) }, apply),
        }
    }
}

impl Render for Discover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kinds = [(Kind::Anime, "Anime"), (Kind::Movie, "Movies"), (Kind::Series, "Series")];
        page("discover")
            .child(page_header("Discover", Some("What are you in the mood for?".into())))
            .child(div().flex().gap(px(8.)).mb(px(12.)).children(kinds.into_iter().map(|(k, label)| {
                chip(label, label, self.kind == k).on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = k;
                    this.load(cx);
                    cx.notify();
                }))
            })))
            .child(div().flex().flex_wrap().gap(px(8.)).mb(px(24.)).children(MOODS.iter().enumerate().map(|(i, (label, _, _))| {
                chip(("mood", i), *label, self.mood == i).on_click(cx.listener(move |this, _, _, cx| {
                    this.mood = i;
                    this.load(cx);
                    cx.notify();
                }))
            })))
            .child(div().mb(px(12.)).child(muted(format!(
                "{} picks for \"{}\"",
                match self.kind {
                    Kind::Anime => "Top-rated anime",
                    Kind::Movie => "Top movies",
                    Kind::Series => "Top series",
                },
                MOODS[self.mood].0
            ))))
            .child(grid("discover-results", &self.results))
    }
}
