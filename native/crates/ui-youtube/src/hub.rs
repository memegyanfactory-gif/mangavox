//! State shared by the YouTube page and the background work that runs while the app is open:
//! the connected channel, its videos and analytics, the upload queue, Studio's finished recaps,
//! the upload runner and the automation that pulls the next episode forward when one dips.
//! One entity per app (`hub(cx)`), so leaving the page never stops an upload.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use mv_ui::bg;
use mv_ui::toast::{self, Toast};
use mv_ui::{ph, Route};
use mv_youtube::api::{error_kind, ApiErrorKind};
use mv_youtube::{planner, store, Account, Channel, DayStat, LocalEpisode, Settings, UploadItem, UploadState, Video, VideoEdit, YouTube};

/// How often channel numbers are refreshed (and the automation checks for dips) in the background.
const REFRESH_EVERY: Duration = Duration::from_secs(3 * 60 * 60);
/// Videos whose daily views are read (newest first): enough for the dip window and the charts.
const DAILY_FOR: usize = 12;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The local zone's offset from UTC in seconds (east positive).
pub fn utc_offset() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

/// "YYYY-MM-DD" `days` days before today (UTC).
fn days_ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
}

pub struct Hub {
    pub yt: YouTube,
    pub account: Account,
    pub settings: Settings,
    pub channel: Option<Channel>,
    pub videos: Vec<Video>,
    /// Channel analytics, the last 90 days (oldest first).
    pub channel_daily: Vec<DayStat>,
    /// Daily analytics of the newest videos since their release, by video id.
    pub video_daily: HashMap<String, Vec<DayStat>>,
    pub queue: Vec<UploadItem>,
    /// Studio's finished recaps (chapter videos and full recaps).
    pub episodes: Vec<LocalEpisode>,
    pub covers: HashMap<String, PathBuf>,
    pub loading: bool,
    pub loaded_once: bool,
    pub error: Option<String>,
    pub signed_out: bool,
    pub connecting: bool,
    pub uploading: bool,
    /// Progress of the upload in flight (f32 bits), polled into the queue item.
    progress: Arc<AtomicU32>,
    pub refreshed_at: Option<i64>,
    /// What the automation changed, newest first ("Chapter 4 moved to Fri 18:00").
    pub moves: Vec<String>,
}

struct HubGlobal(Entity<Hub>);
impl Global for HubGlobal {}

/// The app-wide YouTube state.
pub fn hub(cx: &mut App) -> Entity<Hub> {
    if let Some(g) = cx.try_global::<HubGlobal>() {
        return g.0.clone();
    }
    let yt = YouTube::new(store::Store::default());
    let s = yt.store().clone();
    let e = cx.new(|_| Hub {
        account: s.account(),
        settings: s.settings(),
        queue: s.queue(),
        yt,
        channel: None,
        videos: vec![],
        channel_daily: vec![],
        video_daily: HashMap::new(),
        episodes: vec![],
        covers: HashMap::new(),
        loading: false,
        loaded_once: false,
        error: None,
        signed_out: false,
        connecting: false,
        uploading: false,
        progress: Arc::new(AtomicU32::new(0)),
        refreshed_at: None,
        moves: vec![],
    });
    cx.set_global(HubGlobal(e.clone()));
    e
}

/// Starts the background refresh (and the dip automation) once at app start, when a channel is
/// connected. Uploads only run when the user starts them.
pub fn start(cx: &mut App) {
    let h = hub(cx);
    if !h.read(cx).account.connected() {
        return;
    }
    h.update(cx, |h, cx| {
        h.reload_local(cx);
        h.refresh(cx);
    });
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(REFRESH_EVERY).await;
        h.update(cx, |h, cx| h.refresh(cx));
    })
    .detach();
}

impl Hub {
    pub fn connected(&self) -> bool {
        self.account.connected() && !self.signed_out
    }

    /// Next releases that are not public yet, soonest first (queue items with a time).
    pub fn upcoming(&self) -> Vec<&UploadItem> {
        let mut v: Vec<&UploadItem> = self.queue.iter().filter(|i| i.publish_at.is_some() && matches!(i.state, UploadState::Planned | UploadState::Uploading | UploadState::Scheduled)).collect();
        v.sort_by_key(|i| i.publish_at);
        v
    }

    pub fn video(&self, id: &str) -> Option<&Video> {
        self.videos.iter().find(|v| v.id == id)
    }

    /// The queue item a video was uploaded from (its series and chapter).
    pub fn item_for_video(&self, id: &str) -> Option<&UploadItem> {
        self.queue.iter().find(|i| i.video_id.as_deref() == Some(id))
    }

    pub fn trend_of(&self, id: &str) -> mv_youtube::Trend {
        let daily: Vec<u64> = self.video_daily.get(id).map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
        planner::trend(&daily, self.settings.dip_threshold)
    }

    pub fn suggestions(&self) -> Vec<mv_youtube::Suggestion> {
        planner::suggestions(now(), &self.videos, &self.video_daily, &self.queue, &self.episodes, &self.settings, utc_offset())
    }

    fn fail(&mut self, e: &anyhow::Error, what: &str) {
        if error_kind(e) == ApiErrorKind::SignedOut {
            self.signed_out = true;
            self.error = Some("YouTube signed you out. Connect again to keep uploading.".into());
        } else {
            self.error = Some(format!("{what}: {e:#}"));
        }
    }

    // ------------------------------------------------------------ local data

    /// Re-reads the queue and Studio's finished recaps.
    pub fn reload_local(&mut self, cx: &mut Context<Self>) {
        self.queue = self.yt.store().queue();
        cx.spawn(async move |this, cx| {
            let found = bg::blocking(load_episodes).await.ok();
            let _ = this.update(cx, |h, cx| {
                if let Some((eps, covers)) = found {
                    h.episodes = eps;
                    h.covers = covers;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_queue(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Vec<UploadItem>)) {
        match self.yt.store().update_queue(f) {
            Ok(()) => self.queue = self.yt.store().queue(),
            Err(e) => toast::show(cx, Toast::error(format!("Could not save the upload queue: {e:#}"))),
        }
        cx.notify();
    }

    // ------------------------------------------------------------ account

    pub fn save_credentials(&mut self, id: String, secret: String, cx: &mut Context<Self>) {
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.set_credentials(mv_youtube::Credentials { client_id: id.trim().into(), client_secret: secret.trim().into() }).await.map(|_| yt) }).await;
            let _ = this.update(cx, |h, cx| {
                if let Ok(Ok(yt)) = r {
                    h.account = yt.store().account();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Browser sign-in, then a full refresh.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        self.connecting = true;
        self.error = None;
        cx.notify();
        let yt = self.yt.clone();
        let (tx, rx) = futures::channel::oneshot::channel::<String>();
        let done = bg::run(async move {
            let r = yt.connect(move |url| {
                let _ = tx.send(url);
            })
            .await;
            (r, yt.store().account())
        });
        cx.spawn(async move |this, cx| {
            if let Ok(url) = rx.await {
                cx.update(|cx| cx.open_url(&url));
            }
            let r = done.await;
            let _ = this.update(cx, |h, cx| {
                h.connecting = false;
                match r {
                    Ok((Ok(ch), account)) => {
                        h.account = account;
                        h.signed_out = false;
                        toast::show(cx, Toast::success(format!("Connected to {}", ch.title)).icon(ph::YOUTUBE_LOGO));
                        h.channel = Some(ch);
                        h.reload_local(cx);
                        h.refresh(cx);
                    }
                    Ok((Err(e), _)) => h.error = Some(format!("Could not connect: {e:#}")),
                    Err(e) => h.error = Some(format!("Could not connect: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let _ = bg::run(async move { yt.disconnect().await }).await;
            let _ = this.update(cx, |h, cx| {
                h.account = h.yt.store().account();
                h.channel = None;
                h.videos.clear();
                h.channel_daily.clear();
                h.video_daily.clear();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn save_settings(&mut self, s: Settings, cx: &mut Context<Self>) {
        if let Err(e) = self.yt.store().save_settings(&s) {
            toast::show(cx, Toast::error(format!("Could not save YouTube settings: {e:#}")));
        }
        self.settings = s;
        cx.notify();
    }

    // ------------------------------------------------------------ refresh

    /// Channel, videos, analytics; then the dip automation when it is on.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.account.connected() {
            return;
        }
        self.loading = true;
        self.error = None;
        cx.notify();
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move {
                let channel = yt.channel().await?;
                let videos = yt.videos(50).await?;
                let daily = yt.channel_daily(90).await.unwrap_or_default();
                let mut per_video = HashMap::new();
                // Live videos from the last few weeks: the dip window and the per-video charts.
                let cutoff = now() - 21 * 86_400;
                for v in videos.iter().filter(|v| v.privacy != "private" && v.published_at > cutoff).take(DAILY_FOR) {
                    let from = chrono::DateTime::from_timestamp(v.published_at, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| days_ago(7));
                    if let Ok(d) = yt.video_daily(&v.id, &from).await {
                        per_video.insert(v.id.clone(), d);
                    }
                }
                anyhow::Ok((channel, videos, daily, per_video))
            })
            .await;
            let _ = this.update(cx, |h, cx| {
                h.loading = false;
                h.loaded_once = true;
                match r {
                    Ok(Ok((channel, videos, daily, per_video))) => {
                        h.signed_out = false;
                        h.channel = Some(channel);
                        h.videos = videos;
                        h.channel_daily = daily;
                        h.video_daily = per_video;
                        h.refreshed_at = Some(now());
                        h.sync_published();
                        if h.settings.auto_reschedule {
                            h.automate(cx);
                        }
                    }
                    Ok(Err(e)) => h.fail(&e, "Could not read your channel"),
                    Err(e) => h.error = Some(format!("Could not read your channel: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Scheduled items whose video has gone public become Published.
    fn sync_published(&mut self) {
        let live: Vec<String> = self.videos.iter().filter(|v| v.privacy != "private").map(|v| v.id.clone()).collect();
        let _ = self.yt.store().update_queue(|q| {
            for i in q.iter_mut().filter(|i| i.state == UploadState::Scheduled) {
                if i.video_id.as_ref().is_some_and(|id| live.contains(id)) {
                    i.state = UploadState::Published;
                }
            }
        });
        self.queue = self.yt.store().queue();
    }

    /// The dip automation: pull the next episode of a dipping series forward, on YouTube too.
    fn automate(&mut self, cx: &mut Context<Self>) {
        let (videos, daily, settings) = (self.videos.clone(), self.video_daily.clone(), self.settings.clone());
        let moved = self.yt.store().update_queue(|q| planner::auto_reschedule(now(), &videos, &daily, q, &settings, utc_offset())).unwrap_or_default();
        if moved.is_empty() {
            return;
        }
        self.queue = self.yt.store().queue();
        let mut remote = vec![];
        for (id, _, at) in &moved {
            let Some(item) = self.queue.iter().find(|i| &i.id == id) else { continue };
            let line = format!("{} of {} moved to {} because the episode before it is losing views", item.episode_label(), item.manga_title.clone().unwrap_or_default(), fmt_when(*at));
            toast::show(cx, Toast::info(line.clone()).icon(ph::TREND_DOWN).action("Open", |w, cx| mv_ui::navigate(Route::YouTube, w, cx)));
            self.moves.insert(0, line);
            if let (Some(vid), UploadState::Scheduled) = (item.video_id.clone(), item.state) {
                remote.push((vid, *at));
            }
        }
        self.moves.truncate(20);
        if remote.is_empty() {
            return;
        }
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move {
                for (vid, at) in remote {
                    yt.update_video(&vid, &VideoEdit { publish_at: Some(Some(at)), ..Default::default() }).await?;
                }
                anyhow::Ok(())
            })
            .await;
            let _ = this.update(cx, |h, cx| {
                if let Ok(Err(e)) = r {
                    h.fail(&e, "Could not move the release on YouTube");
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ queue

    /// Queues a finished recap with the next free release slot after the chapter before it.
    pub fn queue_episode(&mut self, ep: &LocalEpisode, cx: &mut Context<Self>) -> Option<String> {
        if self.queue.iter().any(|i| i.chapter_id.as_deref() == Some(ep.chapter_id.as_str()) && i.state != UploadState::Failed) {
            return None;
        }
        let id = store::new_id();
        let item = UploadItem {
            id: id.clone(),
            file: ep.file.clone(),
            chapter_id: Some(ep.chapter_id.clone()),
            manga_id: Some(ep.manga_id.clone()),
            manga_title: Some(ep.manga_title.clone()),
            chapter: ep.chapter,
            title: format!("{} {} Recap", ep.manga_title, if ep.chapter.is_some() { ep.label.clone() } else { "Full Story".into() }),
            created_at: now(),
            ..Default::default()
        };
        let settings = self.settings.clone();
        let new = id.clone();
        self.save_queue(cx, |q| {
            // The next free release slot, then the series order (a later chapter never goes first).
            let taken: Vec<i64> = q.iter().filter_map(|i| i.publish_at).collect();
            let mut item = item;
            item.publish_at = planner::next_slots(now(), &taken, &settings, 1, utc_offset()).first().copied();
            q.push(item);
            planner::order_series(q, now(), &settings, utc_offset());
            if let Some(i) = q.iter_mut().find(|i| i.id == new) {
                i.rescheduled_from = None;
            }
        });
        Some(id)
    }

    /// Queues any video file (not from Studio).
    pub fn queue_file(&mut self, file: PathBuf, cx: &mut Context<Self>) -> String {
        let id = store::new_id();
        let title = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "New video".into());
        let at = planner::next_slots(now(), &self.queue.iter().filter_map(|i| i.publish_at).collect::<Vec<_>>(), &self.settings, 1, utc_offset()).first().copied();
        let item = UploadItem { id: id.clone(), file, title, publish_at: at, created_at: now(), ..Default::default() };
        self.save_queue(cx, |q| q.push(item));
        id
    }

    pub fn update_item(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut UploadItem)) {
        let id = id.to_string();
        self.save_queue(cx, |q| {
            if let Some(i) = q.iter_mut().find(|i| i.id == id) {
                f(i);
            }
        });
    }

    pub fn remove_item(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.save_queue(cx, |q| q.retain(|i| i.id != id));
    }

    /// Re-plans release times so every series goes out in chapter order.
    pub fn auto_plan(&mut self, cx: &mut Context<Self>) -> usize {
        let settings = self.settings.clone();
        let mut n = 0;
        self.save_queue(cx, |q| n = planner::order_series(q, now(), &settings, utc_offset()).len());
        n
    }

    /// Moves an item's release. A video already scheduled on YouTube is moved there too.
    pub fn reschedule(&mut self, id: &str, at: Option<i64>, cx: &mut Context<Self>) {
        let remote = self.queue.iter().find(|i| i.id == id).filter(|i| i.state == UploadState::Scheduled).and_then(|i| i.video_id.clone());
        self.update_item(id, cx, |i| {
            i.publish_at = at;
            i.rescheduled_from = None;
        });
        if let Some(vid) = remote {
            self.edit_video(vid, VideoEdit { publish_at: Some(at), privacy: at.is_none().then(|| self.settings.privacy.clone()), ..Default::default() }, cx);
        }
    }

    /// Saves a change to a live video and puts the answer in `videos`.
    pub fn edit_video(&mut self, id: String, edit: VideoEdit, cx: &mut Context<Self>) {
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.update_video(&id, &edit).await }).await;
            let _ = this.update(cx, |h, cx| {
                match r {
                    Ok(Ok(v)) => {
                        toast::show(cx, Toast::success(format!("Saved “{}” on YouTube", v.title)).icon(ph::YOUTUBE_LOGO));
                        if let Some(old) = h.videos.iter_mut().find(|o| o.id == v.id) {
                            *old = v;
                        }
                    }
                    Ok(Err(e)) => {
                        h.fail(&e, "Could not save on YouTube");
                        toast::show(cx, Toast::error(h.error.clone().unwrap_or_default()));
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("Could not save on YouTube: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ uploads

    /// Uploads planned items one by one in release order until none are left, the daily quota
    /// runs out, or the user signs out.
    pub fn start_uploads(&mut self, cx: &mut Context<Self>) {
        if self.uploading || !self.connected() {
            return;
        }
        self.uploading = true;
        cx.notify();
        self.upload_next(cx);
        // Progress of the upload in flight, into its queue item.
        let progress = self.progress.clone();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let p = f32::from_bits(progress.load(Ordering::Relaxed));
            let alive = this.update(cx, |h, cx| {
                if let Some(i) = h.queue.iter_mut().find(|i| i.state == UploadState::Uploading) {
                    i.progress = p;
                    cx.notify();
                }
                h.uploading
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    pub fn stop_uploads(&mut self, cx: &mut Context<Self>) {
        self.uploading = false;
        cx.notify();
    }

    fn upload_next(&mut self, cx: &mut Context<Self>) {
        if !self.uploading {
            return;
        }
        let mut planned: Vec<&UploadItem> = self.queue.iter().filter(|i| i.state == UploadState::Planned).collect();
        // Release order; an item without a time goes out first (it publishes right away).
        planned.sort_by_key(|i| (i.publish_at.is_some(), i.publish_at, i.chapter.map(|c| (c * 100.) as i64)));
        let Some(mut item) = planned.first().map(|i| (*i).clone()) else {
            self.uploading = false;
            toast::show(cx, Toast::success("All uploads finished").icon(ph::YOUTUBE_LOGO));
            cx.notify();
            return;
        };
        // YouTube refuses a publishAt in the past: those go out now.
        if item.publish_at.is_some_and(|t| t <= now() + 120) {
            item.publish_at = None;
        }
        let id = item.id.clone();
        self.update_item(&id, cx, |i| {
            i.state = UploadState::Uploading;
            i.error = None;
            i.progress = 0.;
        });
        self.progress.store(0f32.to_bits(), Ordering::Relaxed);
        let (yt, settings, progress) = (self.yt.clone(), self.settings.clone(), self.progress.clone());
        let store = self.yt.store().clone();
        cx.spawn(async move |this, cx| {
            let sid = id.clone();
            let r = bg::run(async move {
                let video = yt
                    .upload(
                        &item,
                        &settings,
                        |url| {
                            let url = url.to_string();
                            let _ = store.update_queue(|q| {
                                if let Some(i) = q.iter_mut().find(|i| i.id == sid) {
                                    i.resume_url = Some(url);
                                }
                            });
                        },
                        move |p| progress.store(p.to_bits(), Ordering::Relaxed),
                    )
                    .await?;
                // Series playlist: viewers find the next chapter in order.
                if settings.series_playlists {
                    if let Some(m) = item.manga_title.as_ref().filter(|m| !m.is_empty()) {
                        if let Ok(pl) = yt.ensure_playlist(&format!("{m} Recap"), &settings.privacy).await {
                            let _ = yt.add_to_playlist(&pl, &video).await;
                        }
                    }
                }
                anyhow::Ok((video, item.publish_at))
            })
            .await;
            let _ = this.update(cx, |h, cx| {
                match r {
                    Ok(Ok((video, at))) => {
                        h.update_item(&id, cx, |i| {
                            i.video_id = Some(video);
                            i.state = if at.is_some() { UploadState::Scheduled } else { UploadState::Published };
                            i.publish_at = at.or(Some(now()));
                            i.resume_url = None;
                            i.progress = 1.;
                        });
                        h.upload_next(cx);
                    }
                    Ok(Err(e)) => {
                        let kind = error_kind(&e);
                        let msg = format!("{e:#}");
                        h.update_item(&id, cx, |i| {
                            // Quota and sign-in problems are not the item's fault: it stays planned.
                            i.state = if kind == ApiErrorKind::Other { UploadState::Failed } else { UploadState::Planned };
                            i.error = Some(msg.clone());
                        });
                        match kind {
                            ApiErrorKind::Quota => {
                                h.uploading = false;
                                toast::show(cx, Toast::warning("YouTube's daily upload quota is used up. Start uploads again tomorrow; the queue keeps its order.").sticky());
                            }
                            ApiErrorKind::SignedOut => {
                                h.uploading = false;
                                h.fail(&e, "Upload stopped");
                            }
                            ApiErrorKind::Other => {
                                toast::show(cx, Toast::error(format!("Upload failed: {msg}")));
                                h.upload_next(cx);
                            }
                        }
                    }
                    Err(e) => {
                        h.uploading = false;
                        h.update_item(&id, cx, |i| {
                            i.state = UploadState::Planned;
                            i.error = Some(format!("{e:#}"));
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Studio's finished recaps: every chapter with a video, plus each manga's joined full recap.
fn load_episodes() -> (Vec<LocalEpisode>, HashMap<String, PathBuf>) {
    let Ok(groups) = ui_studio::data::load() else { return (vec![], HashMap::new()) };
    let mut eps = vec![];
    let mut covers = HashMap::new();
    for g in groups {
        if let Some(c) = &g.cover {
            covers.insert(g.id.clone(), c.clone());
        }
        for c in &g.chapters {
            if let Some(v) = &c.video {
                eps.push(LocalEpisode { chapter_id: c.id.clone(), manga_id: c.manga_id.clone(), manga_title: c.manga_title.clone(), chapter: c.number, label: c.label(), file: v.clone() });
            }
        }
        let full = mv_core::paths::studio_projects().join(mv_core::paths::safe_name(&g.id)).join("full_recap.mp4");
        if full.is_file() {
            eps.push(LocalEpisode { chapter_id: format!("full:{}", g.id), manga_id: g.id.clone(), manga_title: g.title.clone(), chapter: None, label: "Full recap".into(), file: full });
        }
    }
    (eps, covers)
}

/// "Fri 18:00" this week, "Nov 3, 18:00" further out (local time).
pub fn fmt_when(t: i64) -> String {
    let Some(d) = chrono::DateTime::from_timestamp(t, 0) else { return "—".into() };
    let local = d.with_timezone(&chrono::Local);
    let days = (t - now()) / 86_400;
    if (0..6).contains(&days) {
        local.format("%a %H:%M").to_string()
    } else {
        local.format("%b %-d, %H:%M").to_string()
    }
}

/// "3 days ago", "in 2 hours".
pub fn fmt_ago(t: i64) -> String {
    let d = now() - t;
    let (n, unit, future) = match d.abs() {
        s if s < 3600 => ((s / 60).max(1), "minute", d < 0),
        s if s < 86_400 => (s / 3600, "hour", d < 0),
        s if s < 30 * 86_400 => (s / 86_400, "day", d < 0),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month", d < 0),
        s => (s / (365 * 86_400), "year", d < 0),
    };
    let unit = if n == 1 { unit.to_string() } else { format!("{unit}s") };
    if future {
        format!("in {n} {unit}")
    } else {
        format!("{n} {unit} ago")
    }
}

/// 1234 → "1.2K", 2_500_000 → "2.5M".
pub fn fmt_num(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim_zero(format!("{:.1}K", n as f64 / 1e3)),
        _ => trim_zero(format!("{:.1}M", n as f64 / 1e6)),
    }
}

fn trim_zero(s: String) -> String {
    s.replace(".0K", "K").replace(".0M", "M")
}

/// Parses "YYYY-MM-DD HH:MM" (local time) into Unix seconds.
pub fn parse_local(s: &str) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M").ok()?;
    naive.and_local_timezone(chrono::Local).single().map(|d| d.timestamp())
}

/// Unix seconds as "YYYY-MM-DD HH:MM" local time (what `parse_local` reads).
pub fn to_local_input(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui_kit glob brings its own `test` attribute.
    use super::{fmt_num, parse_local, to_local_input};

    #[test]
    fn numbers() {
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(1_000), "1K");
        assert_eq!(fmt_num(1_250), "1.2K");
        assert_eq!(fmt_num(2_500_000), "2.5M");
    }

    #[test]
    fn local_input_round_trips() {
        let t = parse_local("2026-11-03 18:00").unwrap();
        assert_eq!(to_local_input(t), "2026-11-03 18:00");
        assert!(parse_local("tomorrow").is_none());
    }
}
