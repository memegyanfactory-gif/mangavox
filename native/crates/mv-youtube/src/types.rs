//! Data shared by the API client, the store, the planner and the UI. Times are Unix seconds (UTC).
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The user's own Google Cloud OAuth client ("Desktop app" type) with the YouTube Data API v3
/// and YouTube Analytics API enabled. MangaVox ships no client of its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

impl Credentials {
    pub fn is_set(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when `access_token` stops working.
    pub expires_at: i64,
    pub scope: String,
}

/// `youtube/account.json`: the OAuth client, its tokens and the connected channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub credentials: Credentials,
    pub tokens: Option<Tokens>,
    pub channel_id: Option<String>,
    pub channel_title: Option<String>,
}

impl Account {
    pub fn connected(&self) -> bool {
        self.tokens.as_ref().is_some_and(|t| !t.refresh_token.is_empty())
    }
}

/// `youtube/settings.json`: how the scheduler plans and what uploads default to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Hours between two planned releases (24 = one a day).
    pub cadence_hours: u32,
    /// Local hour (0-23) planned releases go out at.
    pub publish_hour: u8,
    /// Pull the next episode of a series forward when its latest one dips (off until turned on).
    pub auto_reschedule: bool,
    /// A drop of this share from the video's best day counts as a dip (0.4 = 40%).
    pub dip_threshold: f32,
    /// Never pull a release closer than this many hours from now.
    pub min_lead_hours: u32,
    /// Visibility of new uploads: "private" (the default: nothing goes live by itself), "unlisted"
    /// or "public". Scheduled release times apply only to "unlisted" and "public".
    pub privacy: String,
    /// YouTube category id ("1" Film & Animation, "24" Entertainment).
    pub category_id: String,
    pub made_for_kids: bool,
    /// Add each episode to a playlist named after its manga.
    pub series_playlists: bool,
    /// BCP-47 language of titles and descriptions ("en").
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cadence_hours: 24,
            publish_hour: 18,
            auto_reschedule: false,
            dip_threshold: 0.4,
            min_lead_hours: 2,
            privacy: "private".into(),
            category_id: "1".into(),
            made_for_kids: false,
            series_playlists: true,
            language: "en".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UploadState {
    /// Waiting in the queue; uploads when the queue runs.
    #[default]
    Planned,
    Uploading,
    /// On YouTube as private with `publishAt` set: YouTube publishes it, MangaVox need not run.
    Scheduled,
    /// Public (or published right away).
    Published,
    Failed,
}

impl UploadState {
    pub fn label(self) -> &'static str {
        match self {
            UploadState::Planned => "Planned",
            UploadState::Uploading => "Uploading",
            UploadState::Scheduled => "Scheduled",
            UploadState::Published => "Published",
            UploadState::Failed => "Failed",
        }
    }

    /// On YouTube already (scheduled or live).
    pub fn on_youtube(self) -> bool {
        matches!(self, UploadState::Scheduled | UploadState::Published)
    }
}

/// One video in the upload queue (`youtube/queue.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UploadItem {
    /// Local id ("up-<millis>-<n>").
    pub id: String,
    pub file: PathBuf,
    /// Studio chapter the video was made from (None for a full recap or a picked file).
    pub chapter_id: Option<String>,
    pub manga_id: Option<String>,
    pub manga_title: Option<String>,
    /// Chapter number, for continuity ("Chapter 2 before Chapter 5").
    pub chapter: Option<f64>,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    /// When it goes public. None = public as soon as the upload finishes.
    pub publish_at: Option<i64>,
    pub state: UploadState,
    pub video_id: Option<String>,
    pub error: Option<String>,
    /// 0..1 while uploading.
    pub progress: f32,
    pub created_at: i64,
    /// Set when the automation moved `publish_at`: the time it had before.
    pub rescheduled_from: Option<i64>,
    /// Resumable session URL of an upload that stopped half-way.
    pub resume_url: Option<String>,
}

impl UploadItem {
    /// "Chapter 2" / "Chapter 3.5" / the title.
    pub fn episode_label(&self) -> String {
        match self.chapter {
            Some(n) if n.fract() == 0.0 => format!("Chapter {}", n as i64),
            Some(n) => format!("Chapter {n}"),
            None => self.title.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub title: String,
    /// "@handle"
    pub handle: String,
    pub description: String,
    pub thumbnail: String,
    pub subscribers: u64,
    pub views: u64,
    pub video_count: u64,
    /// The channel's "uploads" playlist id.
    pub uploads_playlist: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Video {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub thumbnail: String,
    pub category_id: String,
    /// When it went (or goes) public; upload time for private videos.
    pub published_at: i64,
    /// "public" | "unlisted" | "private"
    pub privacy: String,
    /// Scheduled release of a private video.
    pub publish_at: Option<i64>,
    pub views: u64,
    pub likes: u64,
    pub comments: u64,
    pub duration_secs: u64,
}

impl Video {
    pub fn url(&self) -> String {
        format!("https://youtu.be/{}", self.id)
    }

    pub fn studio_url(&self) -> String {
        format!("https://studio.youtube.com/video/{}/edit", self.id)
    }

    pub fn is_scheduled(&self) -> bool {
        self.privacy == "private" && self.publish_at.is_some()
    }
}

/// A change to a video's details. `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoEdit {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Some(Some(t)): schedule for t (makes it private until then). Some(None): drop the schedule.
    pub publish_at: Option<Option<i64>>,
    pub privacy: Option<String>,
}

/// One day of analytics ("YYYY-MM-DD", channel or one video).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DayStat {
    pub date: String,
    pub views: u64,
    pub watch_minutes: f64,
    pub avg_view_secs: f64,
    pub subs_gained: i64,
    pub likes: u64,
}

/// How a video is doing over its first days (see `planner::trend`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trend {
    /// Fewer than two full days of data.
    TooNew,
    Rising,
    Steady,
    /// Down `drop` (0..1) from its best day.
    Dipping { drop: f32 },
}

/// What a suggestion offers to do with one click.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Move a queued or scheduled episode's release to `at`.
    PublishSooner { item_id: String, at: i64 },
    /// Ask the AI for a better title, description and tags for a live video.
    RefreshMetadata { video_id: String },
    /// Queue the next chapter of a series (it has a finished recap that is not queued yet).
    QueueNext { chapter_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Urgent,
}

/// A card on the Insights page ("Chapter 3 is losing views: release Chapter 4 tomorrow").
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub severity: Severity,
    pub title: String,
    pub body: String,
    pub video_id: Option<String>,
    pub action: Option<Action>,
}

/// A finished recap on disk that could be uploaded (from Studio's chapter list).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalEpisode {
    pub chapter_id: String,
    pub manga_id: String,
    pub manga_title: String,
    pub chapter: Option<f64>,
    pub label: String,
    pub file: PathBuf,
}

/// What the AI writes for an upload.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaSuggestion {
    /// Five title options, best first.
    pub titles: Vec<String>,
    pub description: String,
    /// "#OnePiece" style, 3-5 of them; also placed at the end of the description.
    pub hashtags: Vec<String>,
    /// Search tags (no '#'), under 500 characters together.
    pub tags: Vec<String>,
    /// One or two sentences on why these should do well.
    pub notes: String,
}

/// What the AI knows when it writes `MetaSuggestion`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaContext {
    pub manga_title: String,
    /// "Chapter 2 · The Duel"
    pub episode_label: String,
    /// The recap's story text (trimmed), so the copy is about what happens.
    pub summary: String,
    pub channel_title: String,
    /// Titles of the channel's best videos, so new ones match what works.
    pub top_titles: Vec<String>,
    /// The previous episode's title and link, for "Previous episode" in the description.
    pub previous: Option<(String, String)>,
    /// Improving a live video: its current title, description, tags and numbers.
    pub current: Option<Video>,
    /// What the user asked for ("more dramatic", "shorter").
    pub instruction: String,
    pub language: String,
}
