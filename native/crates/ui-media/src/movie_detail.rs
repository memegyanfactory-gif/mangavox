//! Movie / series detail in the shape of Netflix's "More Info" modal: art with the logo, a light
//! Play button and the My List disc; a two-column body (meta and synopsis | cast, genres,
//! director); numbered episode rows with thumbnails; and a "More Like This" card grid. It runs as
//! a page (`Route::MovieDetail`) or inside [`crate::detail_modal::detail_modal`] over Movies home
//! ([`MovieDetail::new_modal`]). The hero resumes where you left off; an episode row plays on
//! click and can open its sources right under it.
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::media::PlaybackState;
use mv_media::types::{AnimeEpisode, AnimeSearchResult};
use mv_ui::theme::{icon as isz, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    empty_view, error_view, icon, icon_button, icon_tag, menu_button, navigate, ph, segmented, size, skeleton, skeleton_line, spinner, tip, Interactive, MenuItem, Ph,
    Route, Tokens, Tone,
};

use crate::common::{
    announce_completed, announce_list_toggle, completion_due, fetch, fetch_cached, fraction, in_progress, in_watchlist, list_epoch, mark_completed, mark_watching, media_id,
    media_title, progress_of, progress_stamp, record_open, set_watchlist, Load, DAY, EPISODES_TTL,
};
use crate::look::{self, EDGE};
use crate::movies_home::{background_of, cinemeta, kind_of, logo_of, series_point};
use crate::streams::{self, PlayBase, StreamState};

pub struct MovieDetail {
    imdb_id: String,
    kind: String,
    title: String,
    details: Load<AnimeSearchResult>,
    episodes: Load<Vec<AnimeEpisode>>,
    similar: Vec<AnimeSearchResult>,
    season: Option<u32>,
    in_list: Option<bool>,
    /// `common::list_epoch()` when `in_list` was read: the page is kept for Back, so a title
    /// removed on My List meanwhile no longer shows "In My List".
    list_seen: Option<u64>,
    /// My List is being saved.
    saving: bool,
    streams: StreamState,
    /// The running source lookup; replacing or dropping it cancels the lookup.
    lookup: Option<Task<()>>,
    /// The first source lookup (movie, or the first episode) has been started.
    auto: bool,
    /// (season, episode) whose links are shown.
    sel: Option<(u32, u32)>,
    /// The links open under the clicked episode row instead of in the hero.
    inline: bool,
    /// Addon chip selected in the sources panel.
    addon_filter: Option<String>,
    resume: Option<PlaybackState>,
    /// The resume point has been read (the first source lookup waits for it).
    resume_loaded: bool,
    /// Episode numbers finished (only used for one-season shows: the table has no season).
    watched: HashSet<u32>,
    /// `common::progress_stamp()` when `watched` was loaded (player saves and History edits).
    watch_epoch: Option<(u64, u64)>,
    /// Stamp whose `watched` and `resume` are in, and the one last checked for "finished".
    progress_epoch: Option<(u64, u64)>,
    checked_epoch: Option<(u64, u64)>,
    scroll: ScrollHandle,
    /// Shown inside [`crate::detail_modal::detail_modal`]: a shorter hero, and the sources panel
    /// sits under the body instead of beside the hero copy (the modal is too narrow for both).
    modal: bool,
    /// Opens a "More Like This" title. Unset, the title opens as a page (`navigate`); the modal's
    /// host sets it to swap the modal's title instead.
    on_open: Option<OpenTitle>,
}

/// Handler for a "More Like This" click, given the title's `Route::MovieDetail`.
pub type OpenTitle = Rc<dyn Fn(&Route, &mut Window, &mut App)>;

/// Season number from a Stremio video id (`tt123:2:5`).
fn season_of(ep: &AnimeEpisode) -> u32 {
    ep.id.split(':').nth(1).and_then(|s| s.parse().ok()).unwrap_or(1)
}

fn first_season(eps: &[AnimeEpisode]) -> Option<u32> {
    eps.iter().map(season_of).filter(|s| *s > 0).min().or_else(|| eps.first().map(season_of))
}

fn seasons_of(eps: &[AnimeEpisode]) -> Vec<u32> {
    let mut s: Vec<u32> = eps.iter().map(season_of).collect();
    s.sort_unstable();
    s.dedup();
    s
}

fn season_label(s: u32) -> String {
    if s == 0 {
        "Specials".to_string()
    } else {
        format!("Season {s}")
    }
}

/// Cinemeta's release span says the run is over ("2008–2013"); a running show reads "2019–".
fn run_ended(release: &str) -> bool {
    release.split(['–', '-']).nth(1).map(str::trim).is_some_and(|y| y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()))
}

fn runtime(mins: u32) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// The hero's next action.
struct NextUp {
    label: String,
    season: u32,
    episode: u32,
    progress: Option<(f32, String)>,
}

fn retry_sources(v: &mut MovieDetail, window: &mut Window, cx: &mut Context<MovieDetail>) {
    let (s, e) = v.sel.unwrap_or((1, 1));
    v.lookup(s, e, false, window, cx);
}

fn close_inline(v: &mut MovieDetail) {
    v.inline = false;
    v.sel = None;
    v.streams = StreamState::Idle;
    v.lookup = None;
}

/// Netflix's main action: a light button with dark text and the filled play glyph, the inverse of
/// the page so it reads over any art.
fn play_button(id: impl Into<ElementId>, label: String) -> Stateful<Div> {
    let label = SharedString::from(label);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(44.))
        .pl(px(20.))
        .pr(px(26.))
        .rounded(px(radius::XS))
        .bg(Tokens::text_1())
        .text_size(px(size::LG))
        .font_weight(FontWeight::BOLD)
        .text_color(Tokens::base())
        .role(Role::Button)
        .aria_label(label.clone())
        .cursor_pointer()
        .focus_ring()
        .hover(|s| s.bg(Tokens::text_1().alpha(0.8)))
        .active(|s| s.bg(Tokens::text_1().alpha(0.7)))
        .child(icon(ph::PLAY.fill(), Tokens::base(), isz::LG))
        .child(label)
}

/// My List as Netflix's round outlined disc: a plus to add, a check once listed, a spinner while
/// saving. The tooltip names what a click will do.
fn list_disc(id: impl Into<ElementId>, saved: bool, saving: bool) -> Stateful<Div> {
    let label = if saved { "Remove from My List" } else { "Add to My List" };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(42.))
        .rounded_full()
        .bg(Tokens::glass())
        .border_2()
        .border_color(Tokens::text_1().alpha(0.5))
        .role(Role::Button)
        .aria_label(label)
        .aria_toggled(if saved { Toggled::True } else { Toggled::False })
        .tooltip(tip(label))
        .cursor_pointer()
        .focus_ring()
        .hover(|s| s.bg(Tokens::text_1().alpha(0.1)).border_color(Tokens::text_1()))
        .map(|d| if saving { d.child(spinner("list-saving", Tokens::text_1(), isz::MD)) } else { d.child(icon(if saved { ph::CHECK.bold() } else { ph::PLUS.bold() }, Tokens::text_1(), isz::MD)) })
}

/// Small outlined box in a meta line ("HD", "4K", "Series"), as Netflix marks quality.
fn box_tag(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(6.))
        .rounded(px(3.))
        .border_1()
        .border_color(Tokens::text_3())
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Tokens::text_2())
        .child(text.into())
}

/// "Cast: a, b, c" in the body's right column: the label muted, the names wrapping beside it.
fn info_line(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .gap(px(4.))
        .text_size(px(14.))
        .line_height(px(20.))
        .child(div().flex_none().text_color(Tokens::text_3()).child(label))
        .child(div().flex_1().min_w_0().text_color(Tokens::text_1()).child(SharedString::from(value)))
}

/// Section heading in the modal's 24px bold.
fn section_title(text: &'static str) -> Div {
    div().text_size(px(24.)).line_height(px(32.)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(text)
}

impl MovieDetail {
    pub fn new(imdb_id: String, kind: String, title: String, cx: &mut Context<Self>) -> Self {
        let series = kind == "series";
        let (details, similar) = Self::fetch_details(&imdb_id, &kind, &title, cx);
        let episodes = if series {
            let id = imdb_id.clone();
            fetch_cached(cx, format!("cinemeta:episodes:{id}"), EPISODES_TTL, async move { cinemeta()?.series_episodes(&id).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| {
                if let Load::Ready(eps) = &l {
                    v.season = v.season.or_else(|| first_season(eps));
                }
                v.episodes = l;
            })
        } else {
            Load::Ready(Vec::new())
        };
        let season = episodes.ready().and_then(|e| first_season(e));
        Self {
            imdb_id,
            kind,
            title,
            details,
            episodes,
            similar,
            season,
            in_list: None,
            list_seen: None,
            saving: false,
            streams: StreamState::Idle,
            lookup: None,
            auto: false,
            sel: None,
            inline: false,
            addon_filter: None,
            resume: None,
            resume_loaded: false,
            watched: HashSet::new(),
            watch_epoch: None,
            progress_epoch: None,
            checked_epoch: None,
            scroll: ScrollHandle::new(),
            modal: false,
            on_open: None,
        }
    }

    /// [`Self::new`] for showing inside [`crate::detail_modal::detail_modal`] (the wrapper draws
    /// the scrim and the close button).
    pub fn new_modal(imdb_id: String, kind: String, title: String, cx: &mut Context<Self>) -> Self {
        Self { modal: true, ..Self::new(imdb_id, kind, title, cx) }
    }

    /// What a "More Like This" click does instead of opening the title as a page.
    pub fn on_open(mut self, f: impl Fn(&Route, &mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }

    fn fetch_details(imdb_id: &str, kind: &str, title: &str, cx: &mut Context<Self>) -> (Load<AnimeSearchResult>, Vec<AnimeSearchResult>) {
        let (id, t, k) = (imdb_id.to_string(), title.to_string(), kind.to_string());
        let key = format!("cinemeta:meta:{kind}:{imdb_id}");
        let warm = mv_core::cache::DataCache::shared().age(&key).is_some();
        let details = fetch_cached(
            cx,
            key,
            DAY,
            async move {
                let c = cinemeta()?;
                let d = if k == "series" { c.series_details(&id).await } else { c.movie_details(&id).await };
                let d = d.map_err(|e| e.to_string())?.ok_or_else(|| "Title not found.".to_string())?;
                if !warm {
                    let _ = record_open(media_title(&k, &id, &d, &t)).await;
                }
                // "More like this": the top titles of the first genre, minus this one.
                let genre = d.genres.as_ref().and_then(|g| g.first().cloned());
                let similar = match genre {
                    Some(g) if k == "series" => c.series_catalog(Default::default(), 1, Some(&g)).await.unwrap_or_default(),
                    Some(g) => c.movies_catalog(Default::default(), 1, Some(&g)).await.unwrap_or_default(),
                    None => vec![],
                };
                let similar = similar.into_iter().filter(|s| s.imdb_id.as_deref() != Some(id.as_str())).take(18).collect::<Vec<_>>();
                Ok((d, similar))
            },
            |v: &mut Self, l| match l {
                Load::Ready((d, s)) => {
                    v.details = Load::Ready(d);
                    v.similar = s;
                }
                Load::Failed(e) => v.details = Load::Failed(e),
                Load::Loading => {}
            },
        );
        match details {
            Load::Ready((d, s)) => {
                fetch(cx, record_open(media_title(kind, imdb_id, &d, title)), |_: &mut Self, _| {});
                (Load::Ready(d), s)
            }
            _ => (Load::Loading, vec![]),
        }
    }

    fn reload_details(&mut self, cx: &mut Context<Self>) {
        self.details = Load::Loading;
        cx.notify();
        let (id, k, t) = (self.imdb_id.clone(), self.kind.clone(), self.title.clone());
        let (d, s) = Self::fetch_details(&id, &k, &t, cx);
        self.details = d;
        if !s.is_empty() {
            self.similar = s;
        }
    }

    fn series(&self) -> bool {
        self.kind == "series"
    }

    fn display_title(&self) -> String {
        self.details.ready().map(|d| d.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| self.title.clone())
    }

    fn seasons(&self) -> Vec<u32> {
        self.episodes.ready().map(|v| seasons_of(v)).unwrap_or_default()
    }

    fn first_episode_of(&self, season: u32) -> u32 {
        self.episodes.ready().and_then(|e| e.iter().find(|e| season_of(e) == season).map(|e| e.number)).unwrap_or(1)
    }

    /// The episode after (s, e): the next one in the season, else the next season's first.
    fn after(&self, s: u32, e: u32) -> Option<(u32, u32)> {
        let eps = self.episodes.ready()?;
        if let Some(n) = eps.iter().filter(|x| season_of(x) == s && x.number > e).map(|x| x.number).min() {
            return Some((s, n));
        }
        let next = self.seasons().into_iter().find(|x| *x > s)?;
        Some((next, self.first_episode_of(next)))
    }

    /// Where the history row points, with its season (remembered, or the only regular one:
    /// Specials, season 0, do not count, or a one-season show with specials never resumed).
    fn resume_at(&self, h: &PlaybackState) -> Option<(u32, u32)> {
        series_point(&self.imdb_id, h.episode_number).or_else(|| {
            let ep = u32::try_from(h.episode_number?).ok()?;
            let regular: Vec<u32> = self.seasons().into_iter().filter(|s| *s > 0).collect();
            (regular.len() == 1).then(|| (regular[0], ep))
        })
    }

    fn next(&self) -> NextUp {
        let fs = self.season.unwrap_or(1);
        let fe = self.first_episode_of(fs);
        if !self.series() {
            return match &self.resume {
                Some(h) if in_progress(h) => NextUp { label: "Resume".into(), season: 1, episode: 1, progress: Some((fraction(h), look::time_left(h.position_secs, h.duration_secs).unwrap_or_default())) },
                Some(h) if h.completed => NextUp { label: "Watch again".into(), season: 1, episode: 1, progress: None },
                _ => NextUp { label: "Play".into(), season: 1, episode: 1, progress: None },
            };
        }
        let start = NextUp { label: format!("Play S{fs} E{fe}"), season: fs, episode: fe, progress: None };
        let Some(h) = &self.resume else { return start };
        let Some((s, e)) = self.resume_at(h) else { return start };
        if in_progress(h) {
            return NextUp { label: format!("Resume S{s} E{e}"), season: s, episode: e, progress: Some((fraction(h), look::time_left(h.position_secs, h.duration_secs).unwrap_or_default())) };
        }
        let finished = h.completed || (h.duration_secs > 0.0 && h.position_secs >= h.duration_secs * 0.95);
        if !finished {
            return NextUp { label: format!("Play S{s} E{e}"), season: s, episode: e, progress: None };
        }
        match self.after(s, e) {
            Some((ns, ne)) => NextUp { label: format!("Continue S{ns} E{ne}"), season: ns, episode: ne, progress: None },
            None => NextUp { label: format!("Watch again S{fs} E{fe}"), ..start },
        }
    }

    /// Whether the title is in My List, reread whenever My List changed since (the page is kept
    /// for Back). A toggle being saved has the newer answer, so a read that lands then is dropped.
    fn refresh_in_list(&mut self, cx: &mut Context<Self>) {
        let epoch = list_epoch();
        if self.list_seen == Some(epoch) {
            return;
        }
        self.list_seen = Some(epoch);
        fetch(cx, in_watchlist(media_id(&self.kind, &self.imdb_id)), move |v: &mut Self, l| {
            if let (Load::Ready(on), false, true) = (l, v.saving, v.list_seen == Some(epoch)) {
                v.in_list = Some(on);
            }
        });
    }

    /// Reload the resume point and finished episodes after the player saved progress or History
    /// changed.
    fn refresh_watched(&mut self, cx: &mut Context<Self>) {
        let epoch = progress_stamp();
        if self.watch_epoch == Some(epoch) {
            return;
        }
        self.watch_epoch = Some(epoch);
        let kind: &'static str = if self.series() { "series" } else { "movie" };
        fetch(cx, progress_of(kind, media_id(kind, &self.imdb_id)), move |v: &mut Self, l| {
            if let Load::Ready((watched, resume)) = l {
                v.watched = watched;
                v.resume = resume;
                v.progress_epoch = Some(epoch);
            }
            v.resume_loaded = true;
        });
    }

    /// A listed title the user has finished moves to Completed in My List: a movie watched to the
    /// end, or an ended one-season series with every episode watched (the history table has no
    /// season, so longer shows are left to the user). Checked once per player save; adding an
    /// already finished title is not a finish (see `completion_due`).
    fn check_completed(&mut self, cx: &mut Context<Self>) {
        if !completion_due(self.in_list, self.progress_epoch, &mut self.checked_epoch) {
            return;
        }
        let Some(d) = self.details.ready() else { return };
        if self.series() && self.episodes.ready().is_none() {
            return;
        }
        self.checked_epoch = self.progress_epoch;
        let done = if self.series() {
            let eps = self.episodes.ready().map(Vec::as_slice).unwrap_or_default();
            let seasons: Vec<u32> = seasons_of(eps).into_iter().filter(|s| *s > 0).collect();
            let ended = d.status.as_deref().is_some_and(run_ended);
            ended && seasons.len() == 1 && {
                let in_season: Vec<u32> = eps.iter().filter(|e| season_of(e) == seasons[0]).map(|e| e.number).collect();
                !in_season.is_empty() && in_season.iter().all(|n| self.watched.contains(n))
            }
        } else {
            self.resume.as_ref().is_some_and(|h| h.completed)
        };
        if done {
            let id = media_id(&self.kind, &self.imdb_id);
            cx.spawn(async move |_, cx| {
                if let Ok(Some(before)) = mark_completed(id).await {
                    cx.update(|cx| announce_completed(cx, before));
                }
            })
            .detach();
        }
    }

    /// Play the best link right away when it is already known, otherwise look it up and play.
    fn play(&mut self, season: u32, episode: u32, window: &mut Window, cx: &mut Context<Self>) {
        let movie = !self.series();
        if self.in_list == Some(true) {
            fetch(cx, mark_watching(media_id(&self.kind, &self.imdb_id)), |_: &mut Self, _| {});
        }
        if let Some(req) = streams::best(&self.streams, (!movie).then_some(season), (!movie).then_some(episode)) {
            streams::play_now(req, window, cx);
            return;
        }
        self.lookup(season, episode, true, window, cx);
    }

    /// [`Self::play`] from the hero (`from_list` false) or an episode row. When the links still have
    /// to be found, the progress and any failure show next to what was clicked: under the row, or
    /// in the hero. Before, a row far down a season reported into the hero, off screen.
    fn play_at(&mut self, season: u32, episode: u32, from_list: bool, window: &mut Window, cx: &mut Context<Self>) {
        let movie = !self.series();
        if streams::best(&self.streams, (!movie).then_some(season), (!movie).then_some(episode)).is_none() {
            self.inline = from_list;
        }
        self.play(season, episode, window, cx);
    }

    /// Find the links for a movie / episode and list them on the page.
    fn lookup(&mut self, season: u32, episode: u32, autoplay: bool, window: &mut Window, cx: &mut Context<Self>) {
        let movie = !self.series();
        self.sel = Some((season, episode));
        self.addon_filter = None;
        let name = self.display_title();
        let label = if movie { name.clone() } else { format!("{name} S{season:02}E{episode:02}") };
        self.streams = StreamState::Searching(label.clone());
        cx.notify();
        let base = PlayBase {
            title: label.clone(),
            imdb_id: Some(self.imdb_id.clone()),
            mal_id: None,
            season: (!movie).then_some(season),
            episode: (!movie).then_some(episode),
        };
        self.lookup = Some(streams::find(cx, window, label, base, movie, autoplay, |v: &mut Self, s| v.streams = s));
    }

    /// Adds or removes the title; the toast offers Undo (Material 3 snackbar), which puts a removed
    /// title back with the status it had.
    fn toggle_list(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.in_list.filter(|_| !self.saving) else { return };
        let fallback = AnimeSearchResult { title: self.title.clone(), ..Default::default() };
        let t = media_title(&self.kind, &self.imdb_id, self.details.ready().unwrap_or(&fallback), &self.title);
        self.saving = true;
        cx.notify();
        let undo_title = t.clone();
        cx.spawn(async move |this, cx| {
            let r = set_watchlist(t, !cur).await;
            let _ = this.update(cx, |v, cx| {
                v.saving = false;
                match r {
                    Ok(before) => {
                        let now = !cur;
                        v.in_list = Some(now);
                        announce_list_toggle(cx, now, before, undo_title, |v: &mut Self, on| v.in_list = Some(on));
                    }
                    Err(e) => {
                        v.in_list = Some(cur);
                        toast::show(cx, Toast::error(format!("My List could not be updated: {e}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn panel(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let heading = match (self.series(), self.sel) {
            (true, Some((s, e))) => {
                let name = self.episodes.ready().and_then(|v| v.iter().find(|x| season_of(x) == s && x.number == e)).and_then(|x| x.title.clone()).filter(|t| !t.is_empty());
                format!("S{s} E{e}{}", name.map(|n| format!(": {n}")).unwrap_or_default())
            }
            _ => "Sources".to_string(),
        };
        let close: Option<fn(&mut Self)> = if self.inline { Some(close_inline) } else { None };
        streams::sources_panel(&self.streams, heading, self.addon_filter.as_deref(), cx, |v: &mut Self, f| v.addon_filter = f, close, retry_sources)
    }

    /// Art with the logo and the actions bottom-left. As a page, the sources panel docks on the
    /// right of the art; in the modal it sits under the body ([`Self::sources_below`]).
    fn hero(&self, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let height = if self.modal { 480. } else { 560. };
        let fallback = AnimeSearchResult { title: self.title.clone(), imdb_id: Some(self.imdb_id.clone()), ..Default::default() };
        let d = self.details.ready().unwrap_or(&fallback);
        let next = self.next();
        let docked = !self.modal && !self.inline;
        let panel = if docked { self.panel(cx) } else { None };
        let mut copy = look::hero_copy(if self.modal { 36. } else { 56. }).when(panel.is_some(), |d| d.max_w(look::beside_panel(width))).child(look::hero_title(&d.title, logo_of(d)));
        if let Load::Failed(e) = &self.details {
            copy = copy.child(error_view(e.clone()).title("This title could not be loaded").compact().retry(cx.listener(|this, _, _, cx| this.reload_details(cx))).items_start());
        }
        let (ps, pe) = (next.season, next.episode);
        look::billboard("movie-hero", height, background_of(d), None).children(panel.map(|p| div().absolute().right(px(EDGE)).top(px(24.)).bottom(px(24.)).w(px(look::SIDE_PANEL)).child(p))).child(
            copy.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .mt(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(play_button("play-main", next.label.clone()).on_click(cx.listener(move |this, _, w, cx| this.play_at(ps, pe, false, w, cx))))
                            .child(list_disc("toggle-list", self.in_list == Some(true), self.saving).on_click(cx.listener(|this, _, _, cx| this.toggle_list(cx)))),
                    )
                    .when_some(next.progress, |c, (p, left)| c.child(look::resume_note("resume-bar", p, left))),
            ),
        )
    }

    /// Two columns under the art: the meta line ("2022  4 Seasons  [HD]") and the synopsis on
    /// the left, cast, genres and director on the right.
    fn about(&self) -> Option<Div> {
        let body = div().flex().items_start().gap(px(32.)).px(px(EDGE)).pt(px(8.)).mb(px(36.));
        let d = match &self.details {
            Load::Loading => {
                return Some(body.child(div().flex_1().flex().flex_col().gap(px(12.)).child(skeleton(px(220.), px(20.), radius::XS)).child(skeleton_line(relative(0.9))).child(skeleton_line(relative(0.7)))));
            }
            Load::Failed(_) => return None,
            Load::Ready(d) => d,
        };
        let series = self.series();
        let seasons = self.episodes.ready().map(|v| seasons_of(v).into_iter().filter(|s| *s > 0).count());
        let meta = look::meta_line(
            look::score(d.score, isz::MD, Some("IMDb")),
            vec![
                d.year.map(|y| y.to_string()).unwrap_or_default(),
                if series { seasons.filter(|n| *n > 0).map(|n| if n == 1 { "1 Season".to_string() } else { format!("{n} Seasons") }).unwrap_or_default() } else { d.duration_minutes.map(runtime).unwrap_or_default() },
            ],
        )
        .text_size(px(size::LG))
        // Quality comes from the links that were found, never assumed.
        .children(streams::quality_tags(&self.streams).into_iter().map(box_tag))
        .child(box_tag(if series { "Series" } else { "Movie" }));
        let join = |v: &Option<Vec<String>>| v.clone().unwrap_or_default().into_iter().take(5).collect::<Vec<_>>().join(", ");
        let (cast, genres, directors) = (join(&d.cast), join(&d.genres), join(&d.directors));
        Some(
            body.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(meta)
                    .child(div().text_size(px(16.)).line_height(px(24.)).text_color(Tokens::text_1()).child(SharedString::from(d.description.clone().unwrap_or_else(|| "No overview available.".into())))),
            )
            .child(
                div()
                    .w(relative(0.35))
                    .max_w(px(420.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .when(!cast.is_empty(), |x| x.child(info_line("Cast:", cast)))
                    .when(!genres.is_empty(), |x| x.child(info_line("Genres:", genres)))
                    .when(!directors.is_empty(), |x| x.child(info_line("Director:", directors))),
            ),
        )
    }

    /// The modal's sources panel, full width under the body: beside the hero copy it would cover
    /// it at the modal's width. Rows that opened their sources inline keep them under the row.
    fn sources_below(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.modal || self.inline {
            return None;
        }
        self.panel(cx).map(|p| div().px(px(EDGE)).mb(px(36.)).h(px(440.)).child(p))
    }

    /// One Netflix episode row: number, 16:9 thumbnail, title with the air date on the right and
    /// the watch state under it. Click plays it (the hover shows a play glyph); the list button
    /// opens its sources right under the row. The episode the hero continues with sits on
    /// `surface_2` (Netflix's current row); a row whose sources are open also keeps an accent bar,
    /// so it never looks like a hover (Material 3 states).
    fn episode_row(&self, ep: &AnimeEpisode, thumb: Option<String>, single_season: bool, current: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (n, s) = (ep.number, season_of(ep));
        let group: SharedString = format!("mep-{}-{s}-{n}", self.imdb_id).into();
        let selected = self.inline && self.sel == Some((s, n));
        let lit = selected || current;
        // History has no season: the marks belong to the one regular season, never to Specials.
        let watched = single_season && s > 0 && self.watched.contains(&n);
        let resume = self.resume.as_ref().filter(|h| in_progress(h) && self.resume_at(h) == Some((s, n)));
        let progress = resume.map(fraction);
        let left = resume.and_then(|h| look::time_left(h.position_secs, h.duration_secs));
        let title = ep.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Episode {n}"));
        let aired = ep.air_date.as_deref().map(|d| d.chars().take(10).collect::<String>()).filter(|d| !d.is_empty());
        div()
            .id(("ep", (s * 10000 + n) as usize))
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(16.))
            .py(px(16.))
            .rounded(px(radius::XS))
            .border_b_1()
            .border_color(Tokens::border())
            .role(Role::Button)
            .aria_label(SharedString::from(format!("Play episode {n}: {title}")))
            .when(lit, |d| d.bg(Tokens::surface_2()))
            .pressable()
            .hover(move |x| x.bg(if lit { Tokens::surface_3() } else { Tokens::hover() }))
            .when(selected, |d| d.child(div().absolute().left_0().top(px(16.)).bottom(px(16.)).w(px(3.)).rounded_full().bg(Tokens::accent())))
            .child(div().w(px(36.)).flex_none().flex().justify_center().text_size(px(24.)).line_height(px(32.)).text_color(Tokens::text_2()).child(SharedString::from(n.to_string())))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(130.))
                    .h(px(73.))
                    .rounded(px(radius::XS))
                    .overflow_hidden()
                    .bg(Tokens::surface_3())
                    .when_some(thumb, |d, u| d.child(look::pic(u)))
                    .when(watched, |d| d.child(div().absolute().inset_0().bg(Tokens::scrim())))
                    .child(
                        div().absolute().inset_0().flex().items_center().justify_center().opacity(0.).group_hover(group.clone(), |s| s.opacity(1.)).child(
                            div().size(px(32.)).rounded_full().bg(Tokens::glass()).border_1().border_color(Tokens::text_1()).flex().items_center().justify_center().child(icon(ph::PLAY.fill(), Tokens::text_1(), isz::SM)),
                        ),
                    )
                    .when(watched, |d| d.child(div().absolute().left_0().bottom_0().w_full().h(px(3.)).bg(Tokens::accent())))
                    .when_some(progress, |d, p| d.child(div().absolute().left_0().right_0().bottom_0().h(px(3.)).bg(Tokens::text_1().alpha(0.25)).child(div().h_full().w(relative(p.max(0.02))).bg(Tokens::accent())))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().flex_1().min_w_0().text_size(px(16.)).font_weight(FontWeight::BOLD).text_color(if watched { Tokens::text_3() } else { Tokens::text_1() }).truncate().child(SharedString::from(title)))
                            .when_some(aired, |d, a| d.child(div().flex_none().text_size(px(14.)).text_color(Tokens::text_2()).child(SharedString::from(a)))),
                    )
                    .when(watched || left.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .text_color(Tokens::text_2())
                                .when(watched, |d| d.child(icon_tag(ph::CHECK, "Watched", Tone::Neutral)))
                                .when_some(left, |d, l| d.child(SharedString::from(l))),
                        )
                    }),
            )
            .child(icon_button(("ep-sources", (s * 10000 + n) as usize), ph::LIST, if selected { "Hide sources" } else { "Choose source" }).selected(selected).on_click(cx.listener(move |this, _, w, cx| {
                // The row under the button would start playback.
                cx.stop_propagation();
                if this.inline && this.sel == Some((s, n)) {
                    close_inline(this);
                    cx.notify();
                    return;
                }
                this.inline = true;
                this.lookup(s, n, false, w, cx);
            })))
            .on_click(cx.listener(move |this, _, w, cx| this.play_at(s, n, true, w, cx)))
    }

    fn episodes(&self, cx: &mut Context<Self>) -> Div {
        let mut picker: Option<AnyElement> = None;
        let mut count: Option<String> = None;
        let body = match &self.episodes {
            Load::Loading => div().flex().flex_col().children((0..5).map(|_| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .px(px(16.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(skeleton(px(28.), px(24.), radius::XS))
                    .child(skeleton(px(130.), px(73.), radius::XS))
                    .child(div().flex_1().flex().flex_col().gap(px(8.)).child(skeleton_line(relative(0.5))).child(skeleton_line(relative(0.2))))
            })),
            Load::Failed(e) => div().child(error_view(e.clone()).title("Episodes could not be loaded").compact().retry(cx.listener(|this, _, _, cx| {
                this.episodes = Load::Loading;
                let id = this.imdb_id.clone();
                this.episodes = fetch_cached(cx, format!("cinemeta:episodes:{id}"), EPISODES_TTL, async move { cinemeta()?.series_episodes(&id).await.map_err(|e| e.to_string()) }, |v: &mut Self, l| {
                    if let Load::Ready(eps) = &l {
                        v.season = v.season.or_else(|| first_season(eps));
                    }
                    v.episodes = l;
                });
                cx.notify();
            }))),
            Load::Ready(v) if v.is_empty() => div().child(empty_view(ph::PLAYLIST, "No episode list yet", "Use the play button above to look for sources anyway.").compact()),
            Load::Ready(v) => {
                let seasons = seasons_of(v);
                let current = self.season.filter(|s| seasons.contains(s)).unwrap_or(seasons[0]);
                let current_ix = seasons.iter().position(|s| *s == current).unwrap_or(0);
                picker = match seasons.len() {
                    0 | 1 => None,
                    // A few seasons: one segmented control (Material 3); many: a menu, like Netflix.
                    2..=5 => {
                        let list = seasons.clone();
                        Some(
                            segmented("season-picker", seasons.iter().map(|s| (None::<Ph>, season_label(*s))), current_ix)
                                .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                                    this.season = list.get(*ix).copied();
                                    cx.notify();
                                }))
                                .into_any_element(),
                        )
                    }
                    _ => {
                        let weak = cx.weak_entity();
                        let items = seasons
                            .iter()
                            .map(|&s| {
                                let weak = weak.clone();
                                MenuItem::new(season_label(s), move |_, cx| {
                                    let _ = weak.update(cx, |v, cx| {
                                        v.season = Some(s);
                                        cx.notify();
                                    });
                                })
                                .checked(s == current)
                            })
                            .collect();
                        Some(menu_button("season-menu", ph::LIST_NUMBERS, "Choose season", items).label(season_label(current)).into_any_element())
                    }
                };
                let in_season: Vec<&AnimeEpisode> = v.iter().filter(|e| season_of(e) == current).collect();
                count = Some(match in_season.len() {
                    1 => "1 episode".to_string(),
                    n => format!("{n} episodes"),
                });
                let fallback_img = self.details.ready().and_then(background_of);
                let single = seasons.iter().filter(|s| **s > 0).count() <= 1;
                let next = self.next();
                let mut list = div().flex().flex_col();
                for ep in in_season {
                    let (s, n) = (season_of(ep), ep.number);
                    list = list.child(self.episode_row(ep, ep.thumbnail.clone().or_else(|| fallback_img.clone()), single, (next.season, next.episode) == (s, n), cx));
                    if self.inline && self.sel == Some((s, n)) {
                        if let Some(p) = self.panel(cx) {
                            list = list.child(div().w_full().h(px(440.)).my(px(8.)).child(p));
                        }
                    }
                }
                list
            }
        };
        div()
            .px(px(EDGE))
            .mb(px(40.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .mb(px(16.))
                    .child(section_title("Episodes"))
                    .when_some(count, |d, c| d.child(div().text_size(px(size::MD)).text_color(Tokens::text_3()).child(SharedString::from(c))))
                    .child(div().flex_1())
                    .children(picker),
            )
            .child(body)
    }

    /// "More Like This" as Netflix's card grid: three per row in the modal (more on a wide page),
    /// landscape art over a `surface_2` card with the meta and a three-line synopsis.
    fn more_like_this(&self, width: f32) -> Option<Div> {
        if self.similar.is_empty() {
            return None;
        }
        let gap = 16.;
        let cols = if self.modal { 3 } else { look::per_row(width, 260., gap).max(3) };
        let w = ((width - EDGE * 2.0 - gap * (cols as f32 - 1.0)) / cols as f32).max(160.);
        let on_open = self.on_open.clone();
        Some(
            div().px(px(EDGE)).mb(px(40.)).child(section_title("More Like This").mb(px(16.))).child(div().flex().flex_wrap().gap(px(gap)).children(self.similar.iter().enumerate().map(|(i, r)| {
                let route = Route::MovieDetail { imdb_id: r.imdb_id.clone().unwrap_or_else(|| r.id.clone()), kind: kind_of(r).into(), title: r.title.clone() };
                let on_open = on_open.clone();
                let title = SharedString::from(r.title.clone());
                let art = background_of(r).or_else(|| r.cover_url.as_deref().map(look::hi_res));
                let blurb = r.description.as_deref().map(look::blurb).filter(|b| !b.is_empty());
                let group = look::group_of("mlt", ("similar", i));
                div()
                    .id(("similar", i))
                    .group(group.clone())
                    .flex_none()
                    .w(px(w))
                    .flex()
                    .flex_col()
                    .rounded(px(radius::XS))
                    .overflow_hidden()
                    .bg(Tokens::surface_2())
                    .role(Role::Link)
                    .aria_label(title.clone())
                    .cursor_pointer()
                    .focus_ring()
                    .hover(|s| s.bg(Tokens::surface_3()))
                    .child(
                        div().relative().w_full().h(px(w * 9. / 16.)).bg(Tokens::surface_3()).when_some(art, |d, u| d.child(look::pic(u))).child(
                            div().absolute().inset_0().flex().items_center().justify_center().opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                                div().size(px(40.)).rounded_full().bg(Tokens::glass()).border_1().border_color(Tokens::text_1()).flex().items_center().justify_center().child(icon(ph::INFO.bold(), Tokens::text_1(), isz::MD)),
                            ),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .p(px(14.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).truncate().child(title))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(14.))
                                    .text_color(Tokens::text_2())
                                    .children(look::score(r.score, isz::SM, None))
                                    .when_some(r.year, |d, y| d.child(SharedString::from(y.to_string())))
                                    .child(box_tag(if kind_of(r) == "series" { "Series" } else { "Movie" })),
                            )
                            .when_some(blurb, |d, b| d.child(div().text_size(px(14.)).line_height(px(20.)).text_color(Tokens::text_2()).line_clamp(3).child(SharedString::from(b)))),
                    )
                    .on_click(move |_, w, cx| match &on_open {
                        Some(f) => f(&route, w, cx),
                        None => navigate(route.clone(), w, cx),
                    })
            }))),
        )
    }
}

impl Render for MovieDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = look::page_width(&self.scroll, window);
        let series = self.series();
        self.refresh_in_list(cx);
        self.refresh_watched(cx);
        self.check_completed(cx);
        // Links load by themselves: the movie at once, a series' next episode once the list is in.
        if !self.auto && self.resume_loaded {
            let first = self.episodes.ready().map(|_| {
                let n = self.next();
                (n.season, n.episode)
            });
            match (series, first) {
                (false, _) => {
                    self.auto = true;
                    self.lookup(1, 1, false, window, cx);
                }
                (true, Some((s, e))) => {
                    self.auto = true;
                    // The list opens on the season the hero continues in (the hero said "Resume
                    // S2 E4" over Season 1's episodes), as Netflix opens on the current season.
                    if self.seasons().contains(&s) {
                        self.season = Some(s);
                    }
                    self.lookup(s, e, false, window, cx);
                }
                _ => {}
            }
        }

        div().relative().size_full().bg(Tokens::base()).child(
            mv_ui::scroll_page("movie-detail", &self.scroll)
                .child(self.hero(width, cx))
                .children(self.about())
                .children(self.sources_below(cx))
                .when(series, |d| d.child(self.episodes(cx)))
                .children(self.more_like_this(width)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{run_ended, runtime, season_label, season_of, seasons_of};

    #[test]
    fn release_span_tells_ended_from_running() {
        assert!(run_ended("2008–2013"));
        assert!(run_ended("2008-2013"));
        assert!(!run_ended("2019–"));
        assert!(!run_ended("2019"));
    }
    use mv_media::types::AnimeEpisode;

    fn ep(id: &str, n: u32) -> AnimeEpisode {
        AnimeEpisode { id: id.into(), number: n, title: None, air_date: None, thumbnail: None }
    }

    #[test]
    fn seasons_come_from_video_ids() {
        let eps = [ep("tt1:2:1", 1), ep("tt1:1:1", 1), ep("tt1:1:2", 2), ep("tt1:0:1", 1)];
        assert_eq!(season_of(&eps[0]), 2);
        assert_eq!(seasons_of(&eps), vec![0, 1, 2]);
        assert_eq!(season_label(0), "Specials");
        assert_eq!(runtime(135), "2h 15m");
    }
}
