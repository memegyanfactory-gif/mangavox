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
    /// The last upload-time research of other recap channels (`timing.json`).
    pub timing: mv_youtube::timing::Research,
    pub timing_busy: bool,
    /// What the research is doing ("Reading upload times (40/280)").
    pub timing_step: Arc<std::sync::Mutex<String>>,
    pub timing_error: Option<String>,
    /// What the automation changed, newest first ("Chapter 4 moved to Fri 6:00 PM").
    pub moves: Vec<String>,
    /// Comment threads on the channel's videos, newest first (cached in `comments.json`).
    pub threads: Vec<mv_youtube::comments::Thread>,
    pub threads_next: Option<String>,
    pub comments_busy: bool,
    pub comments_error: Option<String>,
    pub comments_at: i64,
    /// Thread ids with a reply being sent.
    pub replying: Vec<String>,
    /// The user pressed "Pause after this one": the automation does not start uploads either.
    pub user_paused: bool,
    /// YouTube refused an upload for quota: no upload starts on its own before this (the next
    /// Pacific midnight); the user's own "Upload" click still tries.
    pub uploads_blocked_until: i64,
    /// The run uploads only these items (the automation's); None = every planned item.
    only: Option<Vec<String>>,
    /// Studio's own thumbnail next to each finished recap, by chapter id (found off the UI thread).
    pub local_thumbs: HashMap<String, String>,
}

/// `youtube/cache.json`: the last channel read, shown at once when the app opens (a refresh only
/// runs when it is older than [`FRESH_FOR`] or the user asks).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Cache {
    channel: Option<Channel>,
    videos: Vec<Video>,
    channel_daily: Vec<DayStat>,
    video_daily: HashMap<String, Vec<DayStat>>,
    refreshed_at: i64,
}

/// `youtube/comments.json`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct CommentsCache {
    threads: Vec<mv_youtube::comments::Thread>,
    at: i64,
}

/// An automation upload this close to its release (or past it) waits for the same time next day.
const OVERDUE_LEAD: i64 = 45 * 60;

/// The queue at app start. An upload cut off when the app closed goes back to Planned (its saved
/// session URL stays, so it resumes where it stopped).
fn requeue_interrupted(s: &store::Store) -> Vec<UploadItem> {
    let _ = s.update_queue(|q| {
        for i in q.iter_mut().filter(|i| i.state == UploadState::Uploading) {
            i.state = UploadState::Planned;
            i.progress = 0.;
        }
    });
    s.queue()
}
/// A refresh younger than this is not repeated when a page opens (the quota is precious).
const FRESH_FOR: i64 = 20 * 60;
/// Comments are read again after this when the Community tab opens.
const COMMENTS_FRESH_FOR: i64 = 10 * 60;

struct HubGlobal(Entity<Hub>);
impl Global for HubGlobal {}

/// The app-wide YouTube state.
pub fn hub(cx: &mut App) -> Entity<Hub> {
    if let Some(g) = cx.try_global::<HubGlobal>() {
        return g.0.clone();
    }
    let yt = YouTube::new(store::Store::default());
    let s = yt.store().clone();
    let account = s.account();
    // The last channel read, so the page opens with numbers (only for the account it belongs to).
    let cache: Cache = if account.connected() { s.load_json("cache.json") } else { Cache::default() };
    let cache_ok = cache.channel.as_ref().is_some_and(|c| Some(&c.id) == account.channel_id.as_ref());
    let cache = if cache_ok { cache } else { Cache::default() };
    let comments: CommentsCache = if cache_ok { s.load_json("comments.json") } else { CommentsCache::default() };
    let e = cx.new(|_| Hub {
        account,
        settings: s.settings(),
        queue: requeue_interrupted(&s),
        yt,
        loaded_once: cache.channel.is_some() && now() - cache.refreshed_at < FRESH_FOR,
        refreshed_at: cache.channel.as_ref().map(|_| cache.refreshed_at),
        channel: cache.channel,
        videos: cache.videos,
        channel_daily: cache.channel_daily,
        video_daily: cache.video_daily,
        episodes: vec![],
        covers: HashMap::new(),
        loading: false,
        error: None,
        signed_out: false,
        connecting: false,
        uploading: false,
        progress: Arc::new(AtomicU32::new(0)),
        moves: vec![],
        timing: s.timing(),
        timing_busy: false,
        timing_step: Arc::default(),
        timing_error: None,
        threads: comments.threads,
        threads_next: None,
        comments_busy: false,
        comments_error: None,
        comments_at: comments.at,
        replying: vec![],
        user_paused: false,
        uploads_blocked_until: 0,
        only: None,
        local_thumbs: HashMap::new(),
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
        h.update(cx, |h, cx| {
            // Background reads never eat into the next upload's quota.
            if h.yt.can_spare(60) {
                h.refresh_now(cx);
                h.load_comments(true, cx);
            }
        });
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
            let found = bg::blocking(|| {
                let (eps, covers) = load_episodes();
                let thumbs: HashMap<String, String> = eps.iter().filter_map(|e| local_thumb(&e.file).map(|t| (e.chapter_id.clone(), t))).collect();
                (eps, covers, thumbs)
            })
            .await
            .ok();
            let _ = this.update(cx, |h, cx| {
                if let Some((eps, covers, thumbs)) = found {
                    h.episodes = eps;
                    h.covers = covers;
                    h.local_thumbs = thumbs;
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

    /// The research is missing or older than a week.
    pub fn timing_stale(&self) -> bool {
        now() - self.timing.fetched_at > 7 * 86_400
    }

    /// When other recap channels upload and which hours do best, in local time.
    pub fn timing_analysis(&self) -> mv_youtube::timing::Analysis {
        mv_youtube::timing::analyze(&self.timing, utc_offset())
    }

    /// Reads recent recap uploads' exact publish times (about a minute; no API quota).
    pub fn research_timing(&mut self, cx: &mut Context<Self>) {
        if self.timing_busy {
            return;
        }
        self.timing_busy = true;
        self.timing_error = None;
        let step = self.timing_step.clone();
        *step.lock().unwrap_or_else(|e| e.into_inner()) = "Searching recap uploads".into();
        let store = self.yt.store().clone();
        cx.spawn(async move |this, cx| {
            let res = bg::run(async move { mv_youtube::timing::research(move |m| *step.lock().unwrap_or_else(|e| e.into_inner()) = m).await }).await;
            let _ = this.update(cx, |h, cx| {
                h.timing_busy = false;
                match res.and_then(|r| r) {
                    Ok(r) => {
                        let _ = store.save_timing(&r);
                        h.timing = r;
                    }
                    Err(e) => h.timing_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        // Repaints the progress line while it runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let alive = this.update(cx, |h, cx| {
                cx.notify();
                h.timing_busy
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    pub fn save_settings(&mut self, s: Settings, cx: &mut Context<Self>) {
        if let Err(e) = self.yt.store().save_settings(&s) {
            toast::show(cx, Toast::error(format!("Could not save YouTube settings: {e:#}")));
        }
        self.settings = s;
        cx.notify();
    }

    // ------------------------------------------------------------ refresh

    /// [`Self::refresh_now`] unless the last read is younger than [`FRESH_FOR`].
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshed_at.is_some_and(|t| now() - t < FRESH_FOR) && self.channel.is_some() {
            self.loaded_once = true;
            return;
        }
        self.refresh_now(cx);
    }

    /// Channel, videos, analytics (about 4 quota units; analytics has its own quota); then the dip
    /// automation when it is on. The answer is cached for the next start.
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
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
                let videos = yt.videos_of(&channel.uploads_playlist, 50).await?;
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
                        h.save_cache();
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

    fn save_cache(&self) {
        let c = Cache { channel: self.channel.clone(), videos: self.videos.clone(), channel_daily: self.channel_daily.clone(), video_daily: self.video_daily.clone(), refreshed_at: self.refreshed_at.unwrap_or(0) };
        let _ = self.yt.store().save_json("cache.json", &c);
    }

    // ------------------------------------------------------------ comments

    /// Top-level comments nobody from the channel has answered.
    pub fn comments_unanswered(&self) -> usize {
        self.threads.iter().filter(|t| !t.top.mine && !t.answered()).count()
    }

    /// Reads the newest 100 threads of the channel (1 quota unit) unless they are fresh;
    /// `force` reads again anyway. What viewers ask for goes to the automation's suggestions.
    pub fn load_comments(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.comments_busy || !self.connected() || (!force && now() - self.comments_at < COMMENTS_FRESH_FOR) {
            return;
        }
        self.comments_busy = true;
        self.comments_error = None;
        cx.notify();
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.comment_threads(None, None, 100).await }).await;
            let _ = this.update(cx, |h, cx| {
                h.comments_busy = false;
                match r {
                    Ok(Ok(page)) => {
                        h.threads = page.threads;
                        h.threads_next = page.next_page;
                        h.comments_at = now();
                        h.save_comments();
                        crate::automation::learn_from_comments(&h.threads, cx);
                    }
                    Ok(Err(e)) => {
                        h.fail(&e, "Could not read comments");
                        h.comments_error = h.error.take();
                    }
                    Err(e) => h.comments_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The next page of older threads.
    pub fn more_comments(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.threads_next.clone() else { return };
        if self.comments_busy {
            return;
        }
        self.comments_busy = true;
        cx.notify();
        let yt = self.yt.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.comment_threads(None, Some(&page), 100).await }).await;
            let _ = this.update(cx, |h, cx| {
                h.comments_busy = false;
                match r {
                    Ok(Ok(p)) => {
                        for t in p.threads {
                            if !h.threads.iter().any(|x| x.id == t.id) {
                                h.threads.push(t);
                            }
                        }
                        h.threads_next = p.next_page;
                        h.save_comments();
                    }
                    Ok(Err(e)) => h.comments_error = Some(format!("{e:#}")),
                    Err(e) => h.comments_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every reply of a thread (when it has more than the five that come with it).
    pub fn load_replies(&mut self, thread_id: String, cx: &mut Context<Self>) {
        let yt = self.yt.clone();
        let id = thread_id.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.replies(&id).await }).await;
            let _ = this.update(cx, |h, cx| {
                if let Ok(Ok(replies)) = r {
                    if let Some(t) = h.threads.iter_mut().find(|t| t.id == thread_id) {
                        t.replies = replies;
                    }
                    h.save_comments();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Answers a thread as the channel (50 quota units). `done(true)` once it is on YouTube.
    pub fn reply(&mut self, thread_id: String, text: String, cx: &mut Context<Self>, done: impl FnOnce(bool, &mut App) + 'static) {
        if self.replying.contains(&thread_id) {
            return;
        }
        self.replying.push(thread_id.clone());
        cx.notify();
        let yt = self.yt.clone();
        let id = thread_id.clone();
        cx.spawn(async move |this, cx| {
            let r = bg::run(async move { yt.reply(&id, &text).await }).await;
            let _ = this.update(cx, |h, cx| {
                h.replying.retain(|x| x != &thread_id);
                let ok = match r {
                    Ok(Ok(c)) => {
                        if let Some(t) = h.threads.iter_mut().find(|t| t.id == thread_id) {
                            t.replies.push(c);
                            t.reply_count += 1;
                        }
                        h.save_comments();
                        toast::show(cx, Toast::success("Reply posted on YouTube").icon(ph::CHAT_CIRCLE));
                        true
                    }
                    Ok(Err(e)) => {
                        toast::show(cx, Toast::error(format!("Could not reply: {e:#}")));
                        false
                    }
                    Err(e) => {
                        toast::show(cx, Toast::error(format!("Could not reply: {e:#}")));
                        false
                    }
                };
                done(ok, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_comments(&self) {
        let _ = self.yt.store().save_json("comments.json", &CommentsCache { threads: self.threads.clone(), at: self.comments_at });
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

    /// Adds a ready item (the automation's: release time and copy already set).
    pub fn push_item(&mut self, item: UploadItem, cx: &mut Context<Self>) {
        self.save_queue(cx, |q| q.push(item));
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
    /// runs out, or the user signs out (the user's "Upload N planned").
    pub fn start_uploads(&mut self, cx: &mut Context<Self>) {
        self.user_paused = false;
        self.only = None;
        self.run_uploads(cx);
    }

    /// Uploads just these items (the automation's), never the user's own planned ones, and not
    /// while the user has uploads paused.
    pub fn upload_items(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if self.user_paused || self.uploads_blocked_until > now() {
            return;
        }
        if self.uploading {
            // A full run takes them anyway; a run of chosen items takes them too.
            if let Some(o) = self.only.as_mut() {
                for id in ids {
                    if !o.contains(&id) {
                        o.push(id);
                    }
                }
            }
            return;
        }
        self.only = Some(ids);
        self.run_uploads(cx);
    }

    fn run_uploads(&mut self, cx: &mut Context<Self>) {
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

    /// "Pause after this one": also holds the automation's uploads until the user starts again.
    pub fn stop_uploads(&mut self, cx: &mut Context<Self>) {
        self.uploading = false;
        self.user_paused = true;
        self.only = None;
        cx.notify();
    }

    fn upload_next(&mut self, cx: &mut Context<Self>) {
        if !self.uploading {
            return;
        }
        let only = self.only.clone();
        let mut planned: Vec<&UploadItem> = self.queue.iter().filter(|i| i.state == UploadState::Planned && only.as_ref().is_none_or(|o| o.contains(&i.id))).collect();
        // Release order; an item without a time goes out first (it publishes right away).
        planned.sort_by_key(|i| (i.publish_at.is_some(), i.publish_at, i.chapter.map(|c| (c * 100.) as i64)));
        let Some(mut item) = planned.first().map(|i| (*i).clone()) else {
            self.uploading = false;
            if self.only.take().is_none() {
                toast::show(cx, Toast::success("All uploads finished").icon(ph::YOUTUBE_LOGO));
            }
            cx.notify();
            return;
        };
        // An automation upload whose release time passed (quota, the app was closed) keeps its
        // good hour: same local time on the next day it can still make, never public right away.
        if item.publish_public {
            if let Some(at) = item.publish_at.filter(|t| *t <= now() + OVERDUE_LEAD) {
                let next = mv_youtube::automation::roll_forward(at, now(), OVERDUE_LEAD, &chrono::Local);
                item.publish_at = Some(next);
                item.resume_url = None;
                let (iid, title) = (item.id.clone(), item.title.clone());
                self.update_item(&iid, cx, |i| {
                    i.publish_at = Some(next);
                    i.resume_url = None;
                });
                let line = format!("“{title}” missed its release time; it now goes public {}", fmt_when(next));
                toast::show(cx, Toast::info(line.clone()).icon(ph::CALENDAR_CHECK));
                self.moves.insert(0, line);
            }
        }
        // YouTube refuses a publishAt in the past: those go out now.
        if item.publish_at.is_some_and(|t| t <= now() + 120) {
            item.publish_at = None;
        }
        // Only an upload with publishAt is scheduled on YouTube; a private one without stays private.
        let scheduled = item.publish_at.is_some() && (item.publish_public || !matches!(self.settings.privacy.as_str(), "" | "private"));
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
                // Its own thumbnail (best effort: a channel needs phone verification for these).
                if let Some(t) = item.thumbnail.as_ref().filter(|t| t.is_file()) {
                    let _ = yt.set_thumbnail(&video, t).await;
                }
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
                            i.state = if scheduled { UploadState::Scheduled } else { UploadState::Published };
                            i.publish_at = if scheduled { at } else { Some(now()) };
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
                                h.only = None;
                                // Nothing uploads again before the quota day turns over; one notice a day.
                                let first = h.uploads_blocked_until <= now();
                                h.uploads_blocked_until = mv_youtube::quota::next_reset(now());
                                if first {
                                    toast::show(cx, Toast::warning(format!("YouTube's upload limit is used up. Uploads go on {}; the queue keeps its order.", fmt_when(h.uploads_blocked_until))).sticky());
                                }
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

/// The thumbnail Studio drew next to a video, if any.
fn local_thumb(video: &std::path::Path) -> Option<String> {
    let dir = video.parent()?;
    ["thumbnail.jpg", "full_recap_thumbnail.jpg", "thumb.jpg"].iter().map(|n| dir.join(n)).find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned())
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

/// "Today, 6:30 PM", "Sat, 6:30 PM", "Nov 3, 6:30 PM" (local, 12-hour).
pub fn fmt_when(t: i64) -> String {
    crate::when::when(t)
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
