//! The `youtube_*` tools: the chat reads the connected channel and fixes titles, descriptions,
//! tags and release times (the YouTube page shows the result after its next refresh).
use mv_youtube::{planner, seo, store::Store, UploadState, VideoEdit, YouTube};
use serde_json::{json, Map, Value};

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn offset() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

fn local(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn trend_word(t: mv_youtube::Trend) -> String {
    match t {
        mv_youtube::Trend::TooNew => "too new".into(),
        mv_youtube::Trend::Rising => "rising".into(),
        mv_youtube::Trend::Steady => "steady".into(),
        mv_youtube::Trend::Dipping { drop } => format!("dipping {:.0}% from its best day", drop * 100.),
    }
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub async fn execute(name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    let yt = YouTube::new(Store::default());
    if !yt.account().await.connected() {
        return Err("No YouTube channel is connected. The user connects it on the YouTube page → Settings.".into());
    }
    let settings = yt.store().settings();
    match name {
        "youtube_channel" => {
            let c = yt.channel().await.map_err(err)?;
            let videos = yt.videos(15).await.map_err(err)?;
            let daily = yt.channel_daily(28).await.unwrap_or_default();
            let views28: u64 = daily.iter().map(|d| d.views).sum();
            let subs28: i64 = daily.iter().map(|d| d.subs_gained).sum();
            let mut per_video = std::collections::HashMap::new();
            let mut rows = vec![];
            for v in &videos {
                let mut trend = None;
                if v.privacy != "private" && now() - v.published_at < 14 * 86_400 {
                    let from = chrono::DateTime::from_timestamp(v.published_at, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
                    if let Ok(d) = yt.video_daily(&v.id, &from).await {
                        trend = Some(trend_word(planner::trend(&d.iter().map(|s| s.views).collect::<Vec<_>>(), settings.dip_threshold)));
                        per_video.insert(v.id.clone(), d);
                    }
                }
                rows.push(json!({
                    "id": v.id, "title": v.title, "privacy": v.privacy, "published": local(v.published_at),
                    "publishAt": v.publish_at.map(local), "views": v.views, "likes": v.likes, "comments": v.comments, "trend": trend,
                }));
            }
            let queue = yt.store().queue();
            let q: Vec<Value> = queue
                .iter()
                .filter(|i| i.state != UploadState::Published)
                .map(|i| json!({ "id": i.id, "title": i.title, "manga": i.manga_title, "episode": i.episode_label(), "releaseAt": i.publish_at.map(local), "state": i.state.label(), "videoId": i.video_id }))
                .collect();
            let tips: Vec<Value> = planner::suggestions(now(), &videos, &per_video, &queue, &[], &settings, offset()).into_iter().map(|s| json!({ "title": s.title, "detail": s.body })).collect();
            Ok(json!({
                "channel": { "title": c.title, "handle": c.handle, "subscribers": c.subscribers, "totalViews": c.views, "videos": c.video_count },
                "last28Days": { "views": views28, "subscribersGained": subs28 },
                "videos": rows, "queue": q, "suggestions": tips,
                "releasePlan": { "everyHours": settings.cadence_hours, "atLocalHour": settings.publish_hour, "autoReschedule": settings.auto_reschedule },
            }))
        }
        "youtube_video" => {
            let id = arg(args, "id").ok_or("id is required")?;
            let videos = yt.videos(50).await.map_err(err)?;
            let v = videos.into_iter().find(|v| v.id == id).ok_or_else(|| format!("No video {id} on the channel"))?;
            let keyword = yt.store().queue().into_iter().find(|i| i.video_id.as_deref() == Some(id)).and_then(|i| i.manga_title).unwrap_or_default();
            let from = chrono::DateTime::from_timestamp(v.published_at, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            let daily: Vec<u64> = yt.video_daily(id, &from).await.map(|d| d.iter().map(|s| s.views).collect()).unwrap_or_default();
            let (score, checks) = seo::score(&v.title, &v.description, &v.tags, &keyword);
            Ok(json!({
                "id": v.id, "title": v.title, "description": v.description, "tags": v.tags, "privacy": v.privacy,
                "published": local(v.published_at), "publishAt": v.publish_at.map(local),
                "views": v.views, "likes": v.likes, "comments": v.comments, "durationSecs": v.duration_secs,
                "viewsPerDaySinceRelease": daily, "trend": trend_word(planner::trend(&daily, settings.dip_threshold)),
                "seoScore": score, "seoToFix": checks.into_iter().filter(|c| !c.ok).map(|c| c.hint).collect::<Vec<_>>(),
            }))
        }
        "youtube_update_video" => {
            let id = arg(args, "id").ok_or("id is required")?;
            let edit = VideoEdit {
                title: arg(args, "title").map(seo::clean_title),
                description: arg(args, "description").map(seo::clean_description),
                tags: arg(args, "tags").map(|t| seo::clean_tags(&t.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())),
                ..Default::default()
            };
            if edit.title.is_none() && edit.description.is_none() && edit.tags.is_none() {
                return Err("Give a title, description or tags to change".into());
            }
            let v = yt.update_video(id, &edit).await.map_err(err)?;
            Ok(json!({ "ok": true, "title": v.title, "note": "Saved on YouTube. The YouTube page shows it after Refresh." }))
        }
        "youtube_schedule" => {
            let id = arg(args, "id").ok_or("id is required")?;
            let at = arg(args, "at").ok_or("at is required")?;
            let t = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M")
                .ok()
                .and_then(|n| n.and_local_timezone(chrono::Local).single())
                .map(|d| d.timestamp())
                .ok_or("Write `at` as YYYY-MM-DD HH:MM (local time)")?;
            if t < now() + 15 * 60 {
                return Err("Pick a time at least 15 minutes from now".into());
            }
            let store = yt.store().clone();
            let item = store.queue().into_iter().find(|i| i.id == id || i.video_id.as_deref() == Some(id));
            // Continuity: an episode never goes out before an earlier chapter of its series.
            if let Some(i) = &item {
                if let (Some(m), Some(c)) = (&i.manga_id, i.chapter) {
                    if let Some(prev) = store.queue().iter().filter(|q| q.manga_id.as_ref() == Some(m) && q.chapter.is_some_and(|qc| qc < c) && q.state != UploadState::Failed).filter_map(|q| q.publish_at).max() {
                        if prev >= t {
                            return Err(format!("An earlier chapter of this series goes out at {}; schedule this one after it.", local(prev)));
                        }
                    }
                }
            }
            let video_id = item.as_ref().and_then(|i| i.video_id.clone()).or_else(|| item.is_none().then(|| id.to_string()));
            if let Some(i) = &item {
                let iid = i.id.clone();
                store
                    .update_queue(|q| {
                        if let Some(i) = q.iter_mut().find(|i| i.id == iid) {
                            i.publish_at = Some(t);
                            i.rescheduled_from = None;
                        }
                    })
                    .map_err(err)?;
            }
            if let Some(vid) = video_id.filter(|_| item.as_ref().map(|i| i.state != UploadState::Planned).unwrap_or(true)) {
                yt.update_video(&vid, &VideoEdit { publish_at: Some(Some(t)), ..Default::default() }).await.map_err(err)?;
            }
            Ok(json!({ "ok": true, "releaseAt": local(t) }))
        }
        other => Err(format!("Unknown tool {other}")),
    }
}
