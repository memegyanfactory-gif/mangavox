//! Studio › YouTube videos: the user's rendered videos as YouTube will show them, next to real
//! videos about the same and similar manga. A home grid (the user's cards among real ones, chips
//! to filter), and a watch page that plays the rendered file in a YouTube-style player with the
//! title, channel row, description and the related column. Each video can preview any thumbnail
//! made on the Thumbnails page (the pick is also what the app's own posters use, see
//! `video_thumb`). Real videos open on youtube.com.
mod home;
mod library;
mod player;
mod related;
mod watch;
mod yt;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;

use self::library::{Channel, MyVideo};
use self::player::Player;
use self::related::{Related, YtVideo};
use super::video_thumb::{self, VideoThumbsChanged};
use crate::page::Load;

/// The thumbnails the page shows, read off the UI thread.
#[derive(Default, Clone)]
struct Thumbs {
    /// Per manga: the made thumbnails (newest first) with when each was made.
    gallery: HashMap<String, Vec<(PathBuf, std::time::SystemTime)>>,
    /// Per manga: the made thumbnail its videos use.
    manga: HashMap<String, Option<PathBuf>>,
    /// Per video key: the picture it shows, and its own pick.
    shown: HashMap<String, Option<PathBuf>>,
    own: HashMap<String, Option<PathBuf>>,
}

impl Thumbs {
    fn read(manga: &[String], videos: &[MyVideo]) -> Thumbs {
        let mtime = |p: &PathBuf| p.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let mut t = Thumbs::default();
        for id in manga {
            t.gallery.insert(id.clone(), mv_recap::thumb::gallery_images(id).into_iter().map(|p| (p.clone(), mtime(&p))).collect());
            t.manga.insert(id.clone(), video_thumb::in_use(id));
        }
        for v in videos {
            let dir = mv_recap::media::manga_dir(&v.manga_id);
            t.own.insert(v.key(), mv_recap::thumb::Picks::load(&dir).for_video(&dir, &v.file));
            t.shown.insert(v.key(), video_thumb::for_video(&v.manga_id, &v.file));
        }
        t
    }
}

/// A manga's related videos.
enum Rel {
    Loading,
    Ready(Related),
    Failed(String),
}

/// The chips over the home grid.
#[derive(Clone, PartialEq)]
enum Chip {
    All,
    Manga(String),
    Yours,
}

/// The open watch page.
struct Watch {
    video: MyVideo,
    player: Option<Player>,
    /// The description box shows everything.
    more: bool,
    theater: bool,
    /// Last mouse move over the player (controls hide 3 s after it while playing).
    moved: Instant,
    hover_player: bool,
    hover_volume: bool,
    /// The progress bar's and the volume slider's bounds, for clicks and drags.
    bar: Rc<Cell<Bounds<Pixels>>>,
    vol: Rc<Cell<Bounds<Pixels>>>,
    /// Where the progress bar is being dragged to (0..1); the seek happens on release.
    drag: Option<f32>,
}

pub(crate) struct VideosPage {
    videos: Load<Vec<MyVideo>>,
    channel: Channel,
    related: HashMap<String, Rel>,
    /// Manga whose related videos are being searched again, and the last search's error.
    refreshing: std::collections::HashSet<String>,
    rel_error: HashMap<String, String>,
    /// Manga searched again on this visit (Try again clears it).
    asked: std::collections::HashSet<String>,
    thumbs: Thumbs,
    chip: Chip,
    watch: Option<Watch>,
    /// The video whose thumbnail menu is open.
    menu: Option<String>,
    /// The page's width, for the number of grid columns.
    width: Rc<Cell<f32>>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl VideosPage {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<VideoThumbsChanged>() {
            cx.set_global(VideoThumbsChanged::default());
        }
        let subs = vec![cx.observe_global::<VideoThumbsChanged>(|this: &mut Self, cx| {
            this.load_thumbs(cx);
        })];
        let mut page = VideosPage {
            videos: Load::Loading,
            channel: Channel::load(),
            related: HashMap::new(),
            refreshing: Default::default(),
            rel_error: HashMap::new(),
            asked: Default::default(),
            thumbs: Thumbs::default(),
            chip: Chip::All,
            watch: None,
            menu: None,
            width: Rc::new(Cell::new(1200.)),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        // Studio's saved library first (instant), then the full scan replaces it.
        cx.spawn(async move |this, cx| {
            if let Ok(Some(videos)) = mv_ui::bg::blocking(|| crate::data::read_snapshot().map(|(groups, _)| library::scan(&groups))).await {
                let _ = this.update(cx, |this, cx| {
                    if matches!(this.videos, Load::Loading) && !videos.is_empty() {
                        this.videos = Load::Ready(videos);
                        this.load_thumbs(cx);
                        this.load_related(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(|| -> anyhow::Result<Vec<MyVideo>> {
                let groups = crate::data::load()?;
                let videos = library::scan(&groups);
                // The posters drawn for the app follow the made thumbnails too.
                let mut seen = std::collections::HashSet::new();
                for v in &videos {
                    if seen.insert(v.manga_id.clone()) {
                        let _ = mv_recap::thumb::sync_picked(&v.manga_id);
                    }
                }
                Ok(videos)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        if !matches!(&this.videos, Load::Ready(old) if *old == v) {
                            this.videos = Load::Ready(v);
                            this.load_thumbs(cx);
                        }
                        this.load_related(cx);
                    }
                    Err(e) => this.videos = Load::Failed(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            if let Ok(Some(c)) = mv_ui::bg::run(Channel::refresh()).await {
                let _ = this.update(cx, |this, cx| {
                    this.channel = c;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn my_videos(&self) -> &[MyVideo] {
        match &self.videos {
            Load::Ready(v) => v,
            _ => &[],
        }
    }

    fn manga_ids(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = vec![];
        for v in self.my_videos() {
            if !out.iter().any(|(id, _)| *id == v.manga_id) {
                out.push((v.manga_id.clone(), v.manga_title.clone()));
            }
        }
        out
    }

    /// Reads the made thumbnails and each video's picture in the background (on load and after a
    /// pick), so renders never touch the disk.
    fn load_thumbs(&mut self, cx: &mut Context<Self>) {
        let videos: Vec<MyVideo> = self.my_videos().to_vec();
        let manga: Vec<String> = self.manga_ids().into_iter().map(|(id, _)| id).collect();
        cx.spawn(async move |this, cx| {
            let t = mv_ui::bg::blocking(move || Thumbs::read(&manga, &videos)).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(t) = t {
                    this.thumbs = t;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every visit searches YouTube again for each manga's related videos: the saved ones show at
    /// once, the new ones replace them when they arrive. A search already running (from an
    /// earlier visit) is waited for instead of started twice.
    fn load_related(&mut self, cx: &mut Context<Self>) {
        for (id, title) in self.manga_ids() {
            if self.refreshing.contains(&id) || !self.asked.insert(id.clone()) {
                continue;
            }
            self.refreshing.insert(id.clone());
            self.rel_error.remove(&id);
            if !self.related.contains_key(&id) {
                self.related.insert(id.clone(), Rel::Loading);
            }
            cx.spawn(async move |this, cx| {
                let mid = id.clone();
                if let Ok(Some(saved)) = mv_ui::bg::blocking(move || Related::load(&mid)).await {
                    let _ = this.update(cx, |this, cx| {
                        if !matches!(this.related.get(&id), Some(Rel::Ready(_))) {
                            this.related.insert(id.clone(), Rel::Ready(saved));
                            cx.notify();
                        }
                    });
                }
                let res = if related::start(&id) {
                    let (mid, t) = (id.clone(), title.clone());
                    let res = match mv_ui::bg::blocking(move || library::brief(&mid, &t)).await {
                        Ok(b) => mv_ui::bg::run(related::fetch(b)).await.and_then(|r| r),
                        Err(e) => Err(e),
                    };
                    if let Ok(r) = &res {
                        let (mid, r) = (id.clone(), r.clone());
                        let _ = mv_ui::bg::blocking(move || r.save(&mid)).await;
                    }
                    related::finish(&id);
                    res
                } else {
                    // Another visit's search: wait for it, then read what it saved.
                    while related::running(&id) {
                        cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    }
                    let mid = id.clone();
                    mv_ui::bg::blocking(move || Related::load(&mid)).await.and_then(|r| r.ok_or_else(|| anyhow::anyhow!("the search found nothing")))
                };
                let _ = this.update(cx, |this, cx| {
                    this.refreshing.remove(&id);
                    match res {
                        Ok(r) => {
                            this.related.insert(id.clone(), Rel::Ready(r));
                        }
                        // Keep what was saved when a refresh fails.
                        Err(e) => {
                            this.rel_error.insert(id.clone(), format!("{e:#}"));
                            if !matches!(this.related.get(&id), Some(Rel::Ready(_))) {
                                this.related.insert(id.clone(), Rel::Failed(format!("{e:#}")));
                            }
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn related_for(&self, manga_id: &str) -> &[YtVideo] {
        match self.related.get(manga_id) {
            Some(Rel::Ready(r)) => &r.videos,
            _ => &[],
        }
    }

    fn open(&mut self, v: MyVideo, cx: &mut Context<Self>) {
        let mut player = Player::new(v.file.clone(), v.secs, 1280, 720);
        player.set_playing(true);
        self.menu = None;
        self.watch = Some(Watch {
            video: v,
            player: Some(player),
            more: false,
            theater: false,
            moved: Instant::now(),
            hover_player: false,
            hover_volume: false,
            bar: Rc::new(Cell::new(Bounds::default())),
            vol: Rc::new(Cell::new(Bounds::default())),
            drag: None,
        });
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.watch_route(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(mut w) = self.watch.take() {
            if let Some(p) = w.player.as_mut() {
                p.stop();
            }
        }
        cx.notify();
    }

    /// Picks the thumbnail one video previews (`None`: the manga's again).
    fn pick(&mut self, v: &MyVideo, image: Option<PathBuf>, cx: &mut Context<Self>) {
        self.thumbs.shown.insert(v.key(), image.clone().or_else(|| self.thumbs.manga.get(&v.manga_id).cloned().flatten()));
        self.thumbs.own.insert(v.key(), image.clone());
        video_thumb::pick_for_video(&v.manga_id, &v.file, image, cx);
        self.menu = None;
        cx.notify();
    }
}

impl Render for VideosPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = yt::font(cx);
        // The app's Back (title bar, mouse back) leaves the watch page first.
        let entity = cx.entity().downgrade();
        let id = cx.entity_id();
        mv_ui::nav::set_local_back(cx, id, move |_, cx| {
            entity
                .update(cx, |this, cx| {
                    let open = this.watch.is_some();
                    this.close(cx);
                    open
                })
                .unwrap_or(false)
        });
        let body = if self.watch.is_some() { self.watch_page(window, cx) } else { self.home(window, cx) };
        let width = self.width.clone();
        div()
            .id("ytv-page")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(yt::c(yt::BG))
            .font_family(font)
            .text_color(yt::c(yt::TEXT))
            .child(self.masthead(cx))
            .child(
                div()
                    .id("ytv-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        canvas(
                            move |b, _, _| {
                                width.set(f32::from(b.size.width));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(1.)),
                    )
                    .child(body),
            )
    }
}

impl VideosPage {
    /// YouTube's top bar: menu, logo, search, mic, and Create / bell / avatar.
    fn masthead(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.watch.as_ref().map(|w| w.video.manga_title.clone()).unwrap_or_default();
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(56.))
            .px(px(16.))
            .bg(yt::c(yt::BG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(px(220.))
                    .gap(px(16.))
                    .child(yt::icon_button("ytv-menu", mv_ui::ph::LIST, 40., 24.))
                    .child(div().id("ytv-logo").cursor_pointer().child(yt::logo()).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.close(cx);
                        this.chip = Chip::All;
                    }))),
            )
            .child(
                div().flex().flex_1().justify_center().child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .max_w(px(728.))
                        .gap(px(16.))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .h(px(40.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .flex_1()
                                        .pl(px(16.))
                                        .rounded_l(px(40.))
                                        .border_1()
                                        .border_color(yt::c(yt::SEARCH_BORDER))
                                        .bg(yt::c(yt::SEARCH_BG))
                                        .child(yt::text(16., 22., FontWeight::NORMAL, if query.is_empty() { 0x888888 } else { yt::TEXT }).child(if query.is_empty() { "Search".to_string() } else { query })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .w(px(64.))
                                        .rounded_r(px(40.))
                                        .border_1()
                                        .border_l_0()
                                        .border_color(yt::c(yt::SEARCH_BORDER))
                                        .bg(yt::c(yt::SEARCH_BTN))
                                        .child(mv_ui::ph_icon(mv_ui::ph::MAGNIFYING_GLASS, mv_ui::Weight::Regular, yt::c(yt::TEXT), 22.)),
                                ),
                        )
                        .child(yt::round_button("ytv-mic", mv_ui::ph::MICROPHONE, 40.)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .w(px(220.))
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(36.))
                            .pl(px(10.))
                            .pr(px(14.))
                            .rounded(px(18.))
                            .bg(yt::tone(0.1))
                            .child(mv_ui::ph_icon(mv_ui::ph::PLUS, mv_ui::Weight::Regular, yt::c(yt::TEXT), 22.))
                            .child(yt::text(14., 36., FontWeight::MEDIUM, yt::TEXT).child("Create")),
                    )
                    .child(yt::icon_button("ytv-bell", mv_ui::ph::BELL, 40., 24.))
                    .child(div().pl(px(8.)).child(yt::avatar(&self.channel.avatar, &self.channel.display_name(), 32.))),
            )
    }
}
