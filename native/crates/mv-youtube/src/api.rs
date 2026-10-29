//! YouTube Data API v3 and YouTube Analytics API v2 for the connected channel. Every call gets a
//! fresh access token (refreshed and saved when it is about to expire).
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::store::Store;
use crate::types::{Account, Channel, DayStat, Settings, UploadItem, Video, VideoEdit};

pub const DATA_API: &str = "https://www.googleapis.com/youtube/v3";
pub const UPLOAD_API: &str = "https://www.googleapis.com/upload/youtube/v3/videos";
pub const ANALYTICS_API: &str = "https://youtubeanalytics.googleapis.com/v2/reports";
const THUMBNAIL_API: &str = "https://www.googleapis.com/upload/youtube/v3/thumbnails/set";

/// Upload chunk size (a multiple of 256 KiB as the resumable protocol requires).
const CHUNK: u64 = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(300);

/// Errors the UI words differently.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiErrorKind {
    /// Not connected, or the refresh token was revoked: sign in again.
    SignedOut,
    /// Daily API quota used up (an upload costs ~1600 of the default 10,000 units).
    Quota,
    Other,
}

/// The error type behind every failure of this module; `error_kind` looks for it.
#[derive(Debug)]
struct ApiError {
    kind: ApiErrorKind,
    msg: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for ApiError {}

fn api_err(kind: ApiErrorKind, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ApiError { kind, msg: msg.into() })
}

fn signed_out() -> anyhow::Error {
    api_err(ApiErrorKind::SignedOut, "Not connected to YouTube: connect your channel first.")
}

/// Classifies an error returned by this module.
pub fn error_kind(e: &anyhow::Error) -> ApiErrorKind {
    for cause in e.chain() {
        if let Some(a) = cause.downcast_ref::<ApiError>() {
            return a.kind.clone();
        }
    }
    let text = format!("{e:#}");
    if text.contains("invalid_grant") {
        ApiErrorKind::SignedOut
    } else if ["quotaExceeded", "dailyLimitExceeded", "uploadLimitExceeded"].iter().any(|m| text.contains(m)) {
        ApiErrorKind::Quota
    } else {
        ApiErrorKind::Other
    }
}

/// Turns a failed HTTP answer into a typed error.
fn classify(status: u16, body: &str) -> ApiError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| v.get("error_description").and_then(Value::as_str).map(str::to_string))
        .or_else(|| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(300).collect());
    let quota = (status == 403 || status == 400) && ["quotaExceeded", "dailyLimitExceeded", "uploadLimitExceeded"].iter().any(|m| body.contains(m));
    if quota {
        ApiError { kind: ApiErrorKind::Quota, msg: format!("YouTube's quota is used up for now (it resets at midnight Pacific time): {detail}") }
    } else if status == 401 {
        ApiError { kind: ApiErrorKind::SignedOut, msg: format!("YouTube rejected the sign-in (HTTP 401): {detail}. Connect your channel again.") }
    } else {
        ApiError { kind: ApiErrorKind::Other, msg: format!("YouTube returned HTTP {status}: {detail}") }
    }
}

fn now_ts() -> i64 {
    Utc::now().timestamp()
}

/// RFC 3339 → Unix seconds.
fn parse_rfc3339(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

/// Unix seconds → "2025-01-06T18:00:00Z".
fn rfc3339_z(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(ts, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// ISO-8601 duration ("PT1H2M3S", "P1DT2H") → seconds.
fn parse_iso_duration(s: &str) -> u64 {
    let Some(rest) = s.strip_prefix('P') else { return 0 };
    let mut total = 0f64;
    let mut num = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' => num.push(c),
            _ => {
                let n: f64 = num.parse().unwrap_or(0.0);
                num.clear();
                total += match (c, in_time) {
                    ('D', false) => n * 86_400.0,
                    ('W', false) => n * 604_800.0,
                    ('H', true) => n * 3600.0,
                    ('M', true) => n * 60.0,
                    ('S', true) => n,
                    _ => 0.0,
                };
            }
        }
    }
    total as u64
}

/// `bytes=0-42` → 43 (how many bytes the server holds).
fn parse_range_header(h: &str) -> Option<u64> {
    let end = h.trim().strip_prefix("bytes=")?.rsplit('-').next()?;
    end.trim().parse::<u64>().ok().map(|e| e + 1)
}

fn content_range(from: u64, len: u64, total: u64) -> String {
    format!("bytes {from}-{}/{total}", from + len - 1)
}

/// A count YouTube sends as a string ("12345") or a number.
fn count(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn text(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("").to_string()
}

/// high → medium → default.
fn thumbnail(v: &Value) -> String {
    let t = v.pointer("/snippet/thumbnails");
    ["high", "medium", "default"].iter().find_map(|k| t.and_then(|t| t.pointer(&format!("/{k}/url"))).and_then(Value::as_str)).unwrap_or("").to_string()
}

fn parse_channel(item: &Value) -> Channel {
    Channel {
        id: text(item, "/id"),
        title: text(item, "/snippet/title"),
        handle: text(item, "/snippet/customUrl"),
        description: text(item, "/snippet/description"),
        thumbnail: thumbnail(item),
        subscribers: count(item.pointer("/statistics/subscriberCount")),
        views: count(item.pointer("/statistics/viewCount")),
        video_count: count(item.pointer("/statistics/videoCount")),
        uploads_playlist: text(item, "/contentDetails/relatedPlaylists/uploads"),
    }
}

fn parse_video(item: &Value) -> Video {
    Video {
        id: text(item, "/id"),
        title: text(item, "/snippet/title"),
        description: text(item, "/snippet/description"),
        tags: item.pointer("/snippet/tags").and_then(Value::as_array).map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        thumbnail: thumbnail(item),
        category_id: text(item, "/snippet/categoryId"),
        published_at: parse_rfc3339(&text(item, "/snippet/publishedAt")).unwrap_or(0),
        privacy: text(item, "/status/privacyStatus"),
        publish_at: parse_rfc3339(&text(item, "/status/publishAt")),
        views: count(item.pointer("/statistics/viewCount")),
        likes: count(item.pointer("/statistics/likeCount")),
        comments: count(item.pointer("/statistics/commentCount")),
        duration_secs: parse_iso_duration(&text(item, "/contentDetails/duration")),
    }
}

/// The `videos.update` body: the fetched video with `edit` applied. Snippet updates need the
/// title and category, so those are always sent; status fields YouTube lets us write are kept.
fn apply_edit(id: &str, original: &Value, edit: &VideoEdit) -> Result<Value> {
    let mut snippet = Map::new();
    let title = crate::seo::clean_title(edit.title.as_deref().unwrap_or(&text(original, "/snippet/title")));
    if title.is_empty() {
        bail!("A video needs a title");
    }
    snippet.insert("title".into(), title.into());
    let description = crate::seo::clean_description(edit.description.as_deref().unwrap_or(&text(original, "/snippet/description")));
    snippet.insert("description".into(), description.into());
    let tags = match &edit.tags {
        Some(t) => crate::seo::clean_tags(t),
        None => parse_video(original).tags,
    };
    snippet.insert("tags".into(), json!(tags));
    let category = text(original, "/snippet/categoryId");
    snippet.insert("categoryId".into(), if category.is_empty() { "22".into() } else { category }.into());
    for k in ["defaultLanguage", "defaultAudioLanguage"] {
        if let Some(v) = original.pointer(&format!("/snippet/{k}")) {
            snippet.insert(k.into(), v.clone());
        }
    }

    let mut status = Map::new();
    for k in ["privacyStatus", "publishAt", "license", "embeddable", "publicStatsViewable", "selfDeclaredMadeForKids"] {
        if let Some(v) = original.pointer(&format!("/status/{k}")) {
            status.insert(k.into(), v.clone());
        }
    }
    match edit.publish_at {
        Some(Some(t)) => {
            status.insert("privacyStatus".into(), "private".into());
            status.insert("publishAt".into(), rfc3339_z(t).into());
        }
        Some(None) => {
            status.remove("publishAt");
            status.insert("privacyStatus".into(), edit.privacy.clone().unwrap_or_else(|| "public".into()).into());
        }
        None => {
            if let Some(p) = &edit.privacy {
                if p != "private" {
                    status.remove("publishAt");
                }
                status.insert("privacyStatus".into(), p.clone().into());
            }
        }
    }
    Ok(json!({ "id": id, "snippet": snippet, "status": status }))
}

/// The resumable-upload metadata for `item`.
fn upload_metadata(item: &UploadItem, settings: &Settings, now: i64) -> Value {
    let mut snippet = json!({
        "title": crate::seo::clean_title(&item.title),
        "description": crate::seo::clean_description(&item.description),
        "tags": crate::seo::clean_tags(&item.tags),
        "categoryId": settings.category_id,
    });
    if !settings.language.trim().is_empty() {
        snippet["defaultLanguage"] = settings.language.clone().into();
        snippet["defaultAudioLanguage"] = settings.language.clone().into();
    }
    let privacy = if settings.privacy.is_empty() { "private" } else { settings.privacy.as_str() };
    let mut status = json!({ "privacyStatus": privacy, "selfDeclaredMadeForKids": settings.made_for_kids });
    // `publishAt` makes YouTube publish the video publicly, so a private upload stays private.
    if let Some(at) = item.publish_at.filter(|&t| t > now && privacy != "private") {
        status["privacyStatus"] = "private".into();
        status["publishAt"] = rfc3339_z(at).into();
    }
    json!({ "snippet": snippet, "status": status })
}

/// Analytics rows → by date. Columns are read by name so their order does not matter.
fn parse_daily(v: &Value) -> HashMap<String, DayStat> {
    let names: Vec<&str> = v.get("columnHeaders").and_then(Value::as_array).map(|h| h.iter().map(|c| c.get("name").and_then(Value::as_str).unwrap_or("")).collect()).unwrap_or_default();
    let mut out = HashMap::new();
    for row in v.get("rows").and_then(Value::as_array).into_iter().flatten() {
        let Some(cells) = row.as_array() else { continue };
        let mut d = DayStat::default();
        for (name, cell) in names.iter().zip(cells) {
            let n = cell.as_f64().unwrap_or(0.0);
            match *name {
                "day" => d.date = cell.as_str().unwrap_or("").to_string(),
                "views" => d.views = n.max(0.0) as u64,
                "estimatedMinutesWatched" => d.watch_minutes = n,
                "averageViewDuration" => d.avg_view_secs = n,
                "subscribersGained" => d.subs_gained = n as i64,
                "likes" => d.likes = n.max(0.0) as u64,
                _ => {}
            }
        }
        if !d.date.is_empty() {
            out.insert(d.date.clone(), d);
        }
    }
    out
}

/// One row per day from `start` to `end` inclusive, zeros for days without data.
fn fill_days(start: NaiveDate, end: NaiveDate, mut by_date: HashMap<String, DayStat>) -> Vec<DayStat> {
    let mut out = Vec::new();
    let mut d = start;
    while d <= end {
        let key = d.format("%Y-%m-%d").to_string();
        out.push(by_date.remove(&key).unwrap_or(DayStat { date: key, ..Default::default() }));
        d += ChronoDuration::days(1);
    }
    out
}

fn thumbnail_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
}

enum UploadStatus {
    Done(String),
    Resume(u64),
    Gone,
}

enum ChunkResult {
    Done(String),
    Next(u64),
}

enum ChunkError {
    Retry(anyhow::Error),
    Fatal(anyhow::Error),
}

fn video_id_of(body: &str) -> Result<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string)).ok_or_else(|| anyhow!("YouTube finished the upload but sent no video id"))
}

#[derive(Clone)]
pub struct YouTube {
    http: reqwest::Client,
    store: Store,
    account: Arc<tokio::sync::Mutex<Account>>,
}

impl YouTube {
    /// Loads `account.json` from `store`.
    pub fn new(store: Store) -> Self {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
        let account = Arc::new(tokio::sync::Mutex::new(store.account()));
        Self { http, store, account }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn account(&self) -> Account {
        self.account.lock().await.clone()
    }

    /// Saves the client id/secret (keeps tokens if the client is unchanged, else drops them).
    pub async fn set_credentials(&self, creds: crate::types::Credentials) -> Result<()> {
        let creds = crate::types::Credentials { client_id: creds.client_id.trim().to_string(), client_secret: creds.client_secret.trim().to_string() };
        let mut acc = self.account.lock().await;
        if acc.credentials != creds {
            acc.tokens = None;
            acc.channel_id = None;
            acc.channel_title = None;
        }
        acc.credentials = creds;
        self.store.save_account(&acc)
    }

    /// Browser sign-in (see `auth::sign_in`), then reads the channel and saves both.
    pub async fn connect(&self, open_browser: impl FnOnce(String) + Send) -> Result<Channel> {
        let creds = self.account.lock().await.credentials.clone();
        if !creds.is_set() {
            bail!("Add your Google OAuth client id and secret first");
        }
        let tokens = crate::auth::sign_in(&creds, open_browser).await?;
        {
            let mut acc = self.account.lock().await;
            acc.tokens = Some(tokens);
            self.store.save_account(&acc)?;
        }
        let channel = self.channel().await?;
        let mut acc = self.account.lock().await;
        acc.channel_id = Some(channel.id.clone());
        acc.channel_title = Some(channel.title.clone());
        self.store.save_account(&acc)?;
        Ok(channel)
    }

    /// Revokes and forgets the tokens and channel (keeps the OAuth client and the queue).
    pub async fn disconnect(&self) -> Result<()> {
        let mut acc = self.account.lock().await;
        if let Some(t) = acc.tokens.take() {
            let token = if t.refresh_token.is_empty() { t.access_token } else { t.refresh_token };
            if !token.is_empty() {
                let _ = crate::auth::revoke(&self.http, &token).await;
            }
        }
        acc.channel_id = None;
        acc.channel_title = None;
        self.store.save_account(&acc)
    }

    /// A valid access token, refreshing (and saving) it when it expires within 60 s.
    pub async fn access_token(&self) -> Result<String> {
        self.token(false).await
    }

    async fn token(&self, force: bool) -> Result<String> {
        let mut acc = self.account.lock().await;
        let Some(t) = acc.tokens.clone().filter(|t| !t.refresh_token.is_empty()) else { return Err(signed_out()) };
        if !force && !t.access_token.is_empty() && t.expires_at > now_ts() + 60 {
            return Ok(t.access_token);
        }
        let mut fresh = match crate::auth::refresh(&self.http, &acc.credentials, &t.refresh_token).await {
            Ok(f) => f,
            Err(e) if format!("{e:#}").contains("invalid_grant") => {
                return Err(api_err(ApiErrorKind::SignedOut, "Google says the sign-in expired or was revoked (invalid_grant). Connect your channel again."));
            }
            Err(e) => return Err(e),
        };
        if fresh.scope.is_empty() {
            fresh.scope = t.scope;
        }
        let access = fresh.access_token.clone();
        acc.tokens = Some(fresh);
        self.store.save_account(&acc)?;
        Ok(access)
    }

    /// Sends an authorized request; a 401 refreshes the token once and retries. Only success
    /// answers come back, everything else is a typed error.
    async fn request(&self, build: impl Fn(&str) -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut force = false;
        loop {
            let token = self.token(force).await?;
            let resp = build(&token).send().await.context("Could not reach YouTube")?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            if status.as_u16() == 401 && !force {
                force = true;
                continue;
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::Error::new(classify(status.as_u16(), &body)));
        }
    }

    async fn get_json(&self, url: &str, query: &[(&str, String)]) -> Result<Value> {
        let resp = self.request(|t| self.http.get(url).query(query).bearer_auth(t).timeout(REQUEST_TIMEOUT)).await?;
        resp.json().await.context("YouTube sent an unreadable answer")
    }

    async fn send_json(&self, method: reqwest::Method, url: &str, query: &[(&str, String)], body: &Value) -> Result<Value> {
        let resp = self.request(|t| self.http.request(method.clone(), url).query(query).bearer_auth(t).json(body).timeout(REQUEST_TIMEOUT)).await?;
        resp.json().await.context("YouTube sent an unreadable answer")
    }

    /// `channels.list?part=snippet,statistics,contentDetails&mine=true`
    pub async fn channel(&self) -> Result<Channel> {
        let v = self.get_json(&format!("{DATA_API}/channels"), &[("part", "snippet,statistics,contentDetails".into()), ("mine", "true".into())]).await?;
        let item = v.pointer("/items/0").ok_or_else(|| anyhow!("This Google account has no YouTube channel"))?;
        Ok(parse_channel(item))
    }

    /// The newest `max` uploads (uploads playlist, then `videos.list` for stats, status and
    /// details in batches of 50). Private and scheduled videos included.
    pub async fn videos(&self, max: usize) -> Result<Vec<Video>> {
        if max == 0 {
            return Ok(vec![]);
        }
        let playlist = self.channel().await?.uploads_playlist;
        if playlist.is_empty() {
            return Ok(vec![]);
        }
        let mut ids: Vec<String> = Vec::new();
        let mut page: Option<String> = None;
        while ids.len() < max {
            let mut q = vec![("part", "contentDetails".to_string()), ("playlistId", playlist.clone()), ("maxResults", "50".into())];
            if let Some(p) = &page {
                q.push(("pageToken", p.clone()));
            }
            let v = self.get_json(&format!("{DATA_API}/playlistItems"), &q).await?;
            ids.extend(v.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(|i| i.pointer("/contentDetails/videoId").and_then(Value::as_str)).map(str::to_string));
            page = v.get("nextPageToken").and_then(Value::as_str).map(str::to_string);
            if page.is_none() {
                break;
            }
        }
        ids.truncate(max);
        let mut by_id: HashMap<String, Video> = HashMap::new();
        for batch in ids.chunks(50) {
            let q = [("part", "snippet,statistics,status,contentDetails".to_string()), ("id", batch.join(",")), ("maxResults", "50".into())];
            let v = self.get_json(&format!("{DATA_API}/videos"), &q).await?;
            for item in v.get("items").and_then(Value::as_array).into_iter().flatten() {
                let video = parse_video(item);
                by_id.insert(video.id.clone(), video);
            }
        }
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// `videos.update` (snippet and/or status, fetched first so untouched fields are kept).
    /// Scheduling sets privacy to private with `publishAt`; dropping it uses `privacy` or "public".
    pub async fn update_video(&self, id: &str, edit: &VideoEdit) -> Result<Video> {
        let v = self.get_json(&format!("{DATA_API}/videos"), &[("part", "snippet,status,statistics,contentDetails".into()), ("id", id.to_string())]).await?;
        let original = v.pointer("/items/0").ok_or_else(|| anyhow!("That video was not found on your channel"))?;
        let body = apply_edit(id, original, edit)?;
        let resp = self.send_json(reqwest::Method::PUT, &format!("{DATA_API}/videos"), &[("part", "snippet,status".into())], &body).await?;
        let mut video = parse_video(&resp);
        let before = parse_video(original);
        video.views = before.views;
        video.likes = before.likes;
        video.comments = before.comments;
        video.duration_secs = before.duration_secs;
        if video.thumbnail.is_empty() {
            video.thumbnail = before.thumbnail;
        }
        if video.published_at == 0 {
            video.published_at = before.published_at;
        }
        Ok(video)
    }

    /// Resumable upload of `item.file` (8 MiB chunks, `progress` 0..1). With `publish_at` in the
    /// future the video is private with `publishAt`; otherwise `settings.privacy`. Resumes from
    /// `item.resume_url` when it is still valid. Returns the video id. `on_session` gets the
    /// session URL as soon as it exists so the caller can save it for resuming.
    pub async fn upload(
        &self,
        item: &UploadItem,
        settings: &Settings,
        on_session: impl Fn(&str) + Send + Sync,
        progress: impl Fn(f32) + Send + Sync,
    ) -> Result<String> {
        let total = tokio::fs::metadata(&item.file).await.with_context(|| format!("Cannot read {}", item.file.display()))?.len();
        if total == 0 {
            bail!("{} is empty", item.file.display());
        }
        let mut offset = 0u64;
        let mut session: Option<String> = None;
        if let Some(url) = item.resume_url.as_deref().filter(|u| !u.is_empty()) {
            match self.upload_status(url, total).await? {
                UploadStatus::Done(id) => {
                    progress(1.0);
                    return Ok(id);
                }
                UploadStatus::Resume(off) => {
                    offset = off;
                    session = Some(url.to_string());
                }
                UploadStatus::Gone => {}
            }
        }
        let session = match session {
            Some(s) => s,
            None => {
                let s = self.start_session(item, settings, total).await?;
                on_session(&s);
                s
            }
        };
        progress(offset as f32 / total as f32);

        let mut file = tokio::fs::File::open(&item.file).await.with_context(|| format!("Cannot open {}", item.file.display()))?;
        let mut buf: Vec<u8> = Vec::new();
        let mut failures = 0u32;
        loop {
            let len = CHUNK.min(total - offset);
            buf.resize(len as usize, 0);
            file.seek(SeekFrom::Start(offset)).await?;
            file.read_exact(&mut buf).await.with_context(|| format!("Cannot read {}", item.file.display()))?;
            let failure = match self.put_chunk(&session, offset, &buf, total).await {
                Ok(ChunkResult::Done(id)) => {
                    progress(1.0);
                    return Ok(id);
                }
                Ok(ChunkResult::Next(next)) if next > offset => {
                    failures = 0;
                    offset = next.min(total);
                    progress(offset as f32 / total as f32);
                    continue;
                }
                Ok(ChunkResult::Next(_)) => anyhow!("YouTube did not accept the data"),
                Err(ChunkError::Fatal(e)) => return Err(e),
                Err(ChunkError::Retry(e)) => e,
            };
            failures += 1;
            if failures > 3 {
                return Err(failure.context("Upload stopped after 3 retries"));
            }
            tokio::time::sleep(Duration::from_secs(1 << failures)).await;
            match self.upload_status(&session, total).await {
                Ok(UploadStatus::Done(id)) => {
                    progress(1.0);
                    return Ok(id);
                }
                Ok(UploadStatus::Resume(off)) => offset = off.min(total),
                Ok(UploadStatus::Gone) => bail!("The upload session expired: start the upload again"),
                Err(e) if error_kind(&e) != ApiErrorKind::Other => return Err(e),
                Err(_) => {}
            }
        }
    }

    async fn start_session(&self, item: &UploadItem, settings: &Settings, total: u64) -> Result<String> {
        let body = upload_metadata(item, settings, now_ts());
        let url = format!("{UPLOAD_API}?uploadType=resumable&part=snippet,status");
        let resp = self
            .request(|t| self.http.post(&url).bearer_auth(t).header("X-Upload-Content-Type", "video/*").header("X-Upload-Content-Length", total).json(&body).timeout(REQUEST_TIMEOUT))
            .await?;
        resp.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string).ok_or_else(|| anyhow!("YouTube did not return an upload session"))
    }

    /// Asks the session how much it already holds.
    async fn upload_status(&self, session: &str, total: u64) -> Result<UploadStatus> {
        let mut force = false;
        loop {
            let token = self.token(force).await?;
            let resp = self
                .http
                .put(session)
                .bearer_auth(&token)
                .header("Content-Range", format!("bytes */{total}"))
                .header(reqwest::header::CONTENT_LENGTH, 0)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .context("Could not reach YouTube")?;
            let status = resp.status().as_u16();
            match status {
                200 | 201 => return Ok(UploadStatus::Done(video_id_of(&resp.text().await.unwrap_or_default())?)),
                308 => return Ok(UploadStatus::Resume(resp.headers().get(reqwest::header::RANGE).and_then(|v| v.to_str().ok()).and_then(parse_range_header).unwrap_or(0))),
                404 | 410 => return Ok(UploadStatus::Gone),
                401 if !force => force = true,
                _ => return Err(anyhow::Error::new(classify(status, &resp.text().await.unwrap_or_default()))),
            }
        }
    }

    async fn put_chunk(&self, session: &str, offset: u64, data: &[u8], total: u64) -> std::result::Result<ChunkResult, ChunkError> {
        let token = self.token(false).await.map_err(ChunkError::Fatal)?;
        let resp = self
            .http
            .put(session)
            .bearer_auth(&token)
            .header("Content-Range", content_range(offset, data.len() as u64, total))
            .body(data.to_vec())
            .timeout(CHUNK_TIMEOUT)
            .send()
            .await
            .map_err(|e| ChunkError::Retry(anyhow!(e).context("Lost the connection to YouTube")))?;
        let status = resp.status().as_u16();
        match status {
            200 | 201 => video_id_of(&resp.text().await.unwrap_or_default()).map(ChunkResult::Done).map_err(ChunkError::Fatal),
            308 => Ok(ChunkResult::Next(resp.headers().get(reqwest::header::RANGE).and_then(|v| v.to_str().ok()).and_then(parse_range_header).unwrap_or(0))),
            401 => {
                let _ = self.token(true).await.map_err(ChunkError::Fatal)?;
                Err(ChunkError::Retry(anyhow!("YouTube asked for a new sign-in token")))
            }
            404 | 410 => Err(ChunkError::Fatal(anyhow!("The upload session expired: start the upload again"))),
            500..=599 => Err(ChunkError::Retry(anyhow::Error::new(classify(status, &resp.text().await.unwrap_or_default())))),
            _ => Err(ChunkError::Fatal(anyhow::Error::new(classify(status, &resp.text().await.unwrap_or_default())))),
        }
    }

    /// Custom thumbnail (`thumbnails.set`, JPEG/PNG under 2 MB).
    pub async fn set_thumbnail(&self, video_id: &str, image: &Path) -> Result<()> {
        let bytes = tokio::fs::read(image).await.with_context(|| format!("Cannot read {}", image.display()))?;
        if bytes.len() > 2 * 1024 * 1024 {
            bail!("A YouTube thumbnail must be under 2 MB (this one is {:.1} MB)", bytes.len() as f64 / 1_048_576.0);
        }
        let ct = thumbnail_content_type(image);
        let q = [("videoId", video_id.to_string())];
        self.request(|t| self.http.post(THUMBNAIL_API).query(&q).bearer_auth(t).header(reqwest::header::CONTENT_TYPE, ct).body(bytes.clone()).timeout(REQUEST_TIMEOUT)).await?;
        Ok(())
    }

    /// The channel's playlist with this exact title, created with `privacy` ("private",
    /// "unlisted" or "public") when missing.
    pub async fn ensure_playlist(&self, title: &str, privacy: &str) -> Result<String> {
        let mut page: Option<String> = None;
        loop {
            let mut q = vec![("part", "snippet".to_string()), ("mine", "true".into()), ("maxResults", "50".into())];
            if let Some(p) = &page {
                q.push(("pageToken", p.clone()));
            }
            let v = self.get_json(&format!("{DATA_API}/playlists"), &q).await?;
            let found = v.get("items").and_then(Value::as_array).into_iter().flatten().find(|i| i.pointer("/snippet/title").and_then(Value::as_str) == Some(title));
            if let Some(id) = found.and_then(|i| i.get("id")).and_then(Value::as_str) {
                return Ok(id.to_string());
            }
            page = v.get("nextPageToken").and_then(Value::as_str).map(str::to_string);
            if page.is_none() {
                break;
            }
        }
        let privacy = if privacy.is_empty() { "private" } else { privacy };
        let body = json!({ "snippet": { "title": title }, "status": { "privacyStatus": privacy } });
        let v = self.send_json(reqwest::Method::POST, &format!("{DATA_API}/playlists"), &[("part", "snippet,status".into())], &body).await?;
        v.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("YouTube created the playlist but sent no id"))
    }

    /// Adds a video to a playlist at the end (no-op when it is already there).
    pub async fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<()> {
        let q = [("part", "id".to_string()), ("playlistId", playlist_id.to_string()), ("videoId", video_id.to_string()), ("maxResults", "1".into())];
        let v = self.get_json(&format!("{DATA_API}/playlistItems"), &q).await?;
        if v.get("items").and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
            return Ok(());
        }
        let body = json!({ "snippet": { "playlistId": playlist_id, "resourceId": { "kind": "youtube#video", "videoId": video_id } } });
        self.send_json(reqwest::Method::POST, &format!("{DATA_API}/playlistItems"), &[("part", "snippet".into())], &body).await?;
        Ok(())
    }

    async fn report(&self, query: Vec<(&str, String)>) -> Result<Value> {
        let mut q = vec![("ids", "channel==MINE".to_string())];
        q.extend(query);
        self.get_json(ANALYTICS_API, &q).await
    }

    async fn daily(&self, start: NaiveDate, end: NaiveDate, video_id: Option<&str>) -> Result<Vec<DayStat>> {
        if start > end {
            return Ok(vec![]);
        }
        let mut q = vec![
            ("startDate", start.format("%Y-%m-%d").to_string()),
            ("endDate", end.format("%Y-%m-%d").to_string()),
            ("metrics", "views,estimatedMinutesWatched,averageViewDuration,subscribersGained,likes".into()),
            ("dimensions", "day".into()),
            ("sort", "day".into()),
        ];
        if let Some(id) = video_id {
            q.push(("filters", format!("video=={id}")));
        }
        let v = self.report(q).await?;
        Ok(fill_days(start, end, parse_daily(&v)))
    }

    /// Channel analytics per day for the last `days` days ending yesterday
    /// (views, estimatedMinutesWatched, averageViewDuration, subscribersGained, likes).
    /// Days without data are filled with zeros so the result has exactly `days` rows.
    pub async fn channel_daily(&self, days: u32) -> Result<Vec<DayStat>> {
        if days == 0 {
            return Ok(vec![]);
        }
        let end = Utc::now().date_naive() - ChronoDuration::days(1);
        self.daily(end - ChronoDuration::days(days as i64 - 1), end, None).await
    }

    /// One video's analytics per day from `from` ("YYYY-MM-DD") to yesterday, zero-filled.
    pub async fn video_daily(&self, video_id: &str, from: &str) -> Result<Vec<DayStat>> {
        let start = NaiveDate::parse_from_str(from.trim(), "%Y-%m-%d").with_context(|| format!("\"{from}\" is not a YYYY-MM-DD date"))?;
        let end = Utc::now().date_naive() - ChronoDuration::days(1);
        self.daily(start, end, Some(video_id)).await
    }

    /// Views per video over the last `days` days, best first (`dimensions=video`, top 25).
    pub async fn top_videos(&self, days: u32) -> Result<Vec<(String, u64)>> {
        let end = Utc::now().date_naive() - ChronoDuration::days(1);
        let start = end - ChronoDuration::days(days.max(1) as i64 - 1);
        let v = self
            .report(vec![
                ("startDate", start.format("%Y-%m-%d").to_string()),
                ("endDate", end.format("%Y-%m-%d").to_string()),
                ("metrics", "views".into()),
                ("dimensions", "video".into()),
                ("sort", "-views".into()),
                ("maxResults", "25".into()),
            ])
            .await?;
        let mut out: Vec<(String, u64)> = v
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| Some((r.get(0)?.as_str()?.to_string(), r.get(1)?.as_f64()?.max(0.0) as u64)))
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.1));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_iso_duration("PT1H2M3S"), 3723);
        assert_eq!(parse_iso_duration("PT45S"), 45);
        assert_eq!(parse_iso_duration("PT10M"), 600);
        assert_eq!(parse_iso_duration("P1DT1H"), 90_000);
        assert_eq!(parse_iso_duration("PT1M3.5S"), 63);
        assert_eq!(parse_iso_duration("P0D"), 0);
        assert_eq!(parse_iso_duration("garbage"), 0);
    }

    #[test]
    fn rfc3339_round_trip() {
        let t = parse_rfc3339("2025-01-06T18:00:00Z").unwrap();
        assert_eq!(rfc3339_z(t), "2025-01-06T18:00:00Z");
        assert_eq!(parse_rfc3339("2025-01-06T19:00:00+01:00"), Some(t));
        assert_eq!(parse_rfc3339("2025-01-06T18:00:00.000Z"), Some(t));
        assert_eq!(parse_rfc3339("nope"), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range_header("bytes=0-8388607"), Some(8_388_608));
        assert_eq!(parse_range_header(" bytes=0-0 "), Some(1));
        assert_eq!(parse_range_header("junk"), None);
        assert_eq!(content_range(0, 8 * 1024 * 1024, 20_000_000), "bytes 0-8388607/20000000");
        assert_eq!(content_range(16_777_216, 3_222_784, 20_000_000), "bytes 16777216-19999999/20000000");
    }

    #[test]
    fn error_kinds() {
        let quota = anyhow::Error::new(classify(403, r#"{"error":{"code":403,"message":"The request cannot be completed because you have exceeded your quota.","errors":[{"reason":"quotaExceeded"}]}}"#));
        assert_eq!(error_kind(&quota), ApiErrorKind::Quota);
        assert_eq!(error_kind(&quota.context("while uploading")), ApiErrorKind::Quota);
        assert_eq!(error_kind(&anyhow::Error::new(classify(401, r#"{"error":{"message":"Invalid Credentials"}}"#))), ApiErrorKind::SignedOut);
        assert_eq!(error_kind(&anyhow::Error::new(classify(403, r#"{"error":{"message":"Forbidden","errors":[{"reason":"forbidden"}]}}"#))), ApiErrorKind::Other);
        assert_eq!(error_kind(&anyhow!("Google refused the request (invalid_grant): Token has been expired or revoked.")), ApiErrorKind::SignedOut);
        assert_eq!(error_kind(&signed_out()), ApiErrorKind::SignedOut);
        assert_eq!(error_kind(&anyhow!("boom")), ApiErrorKind::Other);
        assert!(classify(500, "oops").msg.contains("500"));
    }

    #[test]
    fn parses_video_and_channel() {
        let v = json!({
            "id": "abc",
            "snippet": { "title": "T", "description": "D", "tags": ["a", "b"], "categoryId": "1", "publishedAt": "2025-01-06T18:00:00Z",
                "thumbnails": { "default": { "url": "d" }, "medium": { "url": "m" } } },
            "status": { "privacyStatus": "private", "publishAt": "2025-01-10T18:00:00Z" },
            "statistics": { "viewCount": "1200", "likeCount": "30", "commentCount": "4" },
            "contentDetails": { "duration": "PT9M30S" }
        });
        let p = parse_video(&v);
        assert_eq!((p.id.as_str(), p.thumbnail.as_str(), p.views, p.likes, p.comments, p.duration_secs), ("abc", "m", 1200, 30, 4, 570));
        assert!(p.is_scheduled());
        assert_eq!(p.publish_at, parse_rfc3339("2025-01-10T18:00:00Z"));
        assert_eq!(p.tags, vec!["a", "b"]);

        let c = parse_channel(&json!({
            "id": "UC1", "snippet": { "title": "Chan", "customUrl": "@chan", "thumbnails": { "high": { "url": "h" }, "default": { "url": "d" } } },
            "statistics": { "subscriberCount": "10", "viewCount": "999", "videoCount": "3" },
            "contentDetails": { "relatedPlaylists": { "uploads": "UU1" } }
        }));
        assert_eq!((c.id.as_str(), c.handle.as_str(), c.thumbnail.as_str(), c.subscribers, c.views, c.video_count, c.uploads_playlist.as_str()), ("UC1", "@chan", "h", 10, 999, 3, "UU1"));
    }

    #[test]
    fn edit_keeps_untouched_fields_and_handles_schedule() {
        let orig = json!({
            "snippet": { "title": "Old", "description": "Desc", "tags": ["x"], "categoryId": "24", "defaultLanguage": "en" },
            "status": { "privacyStatus": "public", "license": "youtube", "embeddable": true, "uploadStatus": "processed" }
        });
        let b = apply_edit("id1", &orig, &VideoEdit { title: Some("New <b>".into()), ..Default::default() }).unwrap();
        assert_eq!(b["snippet"]["title"], "New b");
        assert_eq!(b["snippet"]["description"], "Desc");
        assert_eq!(b["snippet"]["categoryId"], "24");
        assert_eq!(b["snippet"]["defaultLanguage"], "en");
        assert_eq!(b["status"]["privacyStatus"], "public");
        assert!(b["status"].get("uploadStatus").is_none());

        let t = parse_rfc3339("2025-02-01T09:00:00Z").unwrap();
        let b = apply_edit("id1", &orig, &VideoEdit { publish_at: Some(Some(t)), ..Default::default() }).unwrap();
        assert_eq!(b["status"]["privacyStatus"], "private");
        assert_eq!(b["status"]["publishAt"], "2025-02-01T09:00:00Z");

        let sched = json!({ "snippet": { "title": "T", "categoryId": "1" }, "status": { "privacyStatus": "private", "publishAt": "2025-02-01T09:00:00Z" } });
        let b = apply_edit("id1", &sched, &VideoEdit { publish_at: Some(None), ..Default::default() }).unwrap();
        assert_eq!(b["status"]["privacyStatus"], "public");
        assert!(b["status"].get("publishAt").is_none());
        let b = apply_edit("id1", &sched, &VideoEdit { publish_at: Some(None), privacy: Some("unlisted".into()), ..Default::default() }).unwrap();
        assert_eq!(b["status"]["privacyStatus"], "unlisted");
        let b = apply_edit("id1", &sched, &VideoEdit { privacy: Some("public".into()), ..Default::default() }).unwrap();
        assert!(b["status"].get("publishAt").is_none());
        assert!(apply_edit("id1", &json!({}), &VideoEdit::default()).is_err());
    }

    #[test]
    fn upload_metadata_schedules_only_the_future() {
        let item = UploadItem { title: "Solo <Leveling> Ch. 3".into(), description: "d".into(), tags: vec!["#manga".into()], publish_at: Some(2_000_000_000), ..Default::default() };
        // New uploads are private by default and a release time never makes them public.
        let m = upload_metadata(&item, &Settings::default(), 1_000_000_000);
        assert_eq!(m["status"]["privacyStatus"], "private");
        assert!(m["status"].get("publishAt").is_none());
        let s = Settings { privacy: "public".into(), ..Default::default() };
        let m = upload_metadata(&item, &s, 1_000_000_000);
        assert_eq!(m["status"]["privacyStatus"], "private");
        assert_eq!(m["status"]["publishAt"], rfc3339_z(2_000_000_000));
        assert_eq!(m["status"]["selfDeclaredMadeForKids"], false);
        assert_eq!(m["snippet"]["title"], "Solo Leveling Ch. 3");
        assert_eq!(m["snippet"]["tags"][0], "manga");
        assert_eq!(m["snippet"]["categoryId"], "1");
        assert_eq!(m["snippet"]["defaultLanguage"], "en");
        let m = upload_metadata(&item, &s, 2_100_000_000);
        assert_eq!(m["status"]["privacyStatus"], "public");
        assert!(m["status"].get("publishAt").is_none());
    }

    #[test]
    fn analytics_rows_are_zero_filled() {
        let v = json!({
            "columnHeaders": [{"name":"day"},{"name":"views"},{"name":"estimatedMinutesWatched"},{"name":"averageViewDuration"},{"name":"subscribersGained"},{"name":"likes"}],
            "rows": [["2025-01-02", 10, 5.5, 33.0, 2, 1]]
        });
        let start = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let days = fill_days(start, NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(), parse_daily(&v));
        assert_eq!(days.len(), 3);
        assert_eq!(days[0], DayStat { date: "2025-01-01".into(), ..Default::default() });
        assert_eq!(days[1], DayStat { date: "2025-01-02".into(), views: 10, watch_minutes: 5.5, avg_view_secs: 33.0, subs_gained: 2, likes: 1 });
        assert_eq!(days[2].views, 0);
    }

    #[test]
    fn thumbnail_types() {
        assert_eq!(thumbnail_content_type(Path::new("a.PNG")), "image/png");
        assert_eq!(thumbnail_content_type(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(thumbnail_content_type(Path::new("a")), "image/jpeg");
    }
}
