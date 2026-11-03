//! What the chat can read and do on the YouTube side, beyond the built-in `youtube_*` tools:
//! - `brain_youtube` (on every page): everything the app has learned, analysed in one answer: the
//!   channel, what its viewers say, the automation and its results, the release-time research and
//!   the competitors (who leads, what their best videos share, what they changed and when).
//! - `youtube_competitors`, `youtube_competitor_channel`, `youtube_competitor_video`: the hourly
//!   competitor research in detail (titles and thumbnails as they changed, views hour by hour).
//! - `youtube_comments` and `youtube_reply`: the channel's comments (cached, no quota) and answering one.
//! - `youtube_automation`: the automation's state, and starting, approving or switching full auto.
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use mv_youtube::competitors::{self, CompChannel, CompStore};
use serde_json::{json, Map, Value};
use ui_assistant::ext::{self, ToolProvider};
use ui_assistant::tools::{AppTool, Phrases, Risk, ToolOutput, ToolResult};

use crate::hub::{fmt_num, now, utc_offset};
use crate::when;

struct YouTubeBrain;

pub fn register() {
    ext::register("youtube-brain", Arc::new(YouTubeBrain));
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

fn tools() -> Vec<AppTool> {
    vec![
        AppTool {
            name: "brain_youtube",
            description: "Everything MangaVox has learned about the user's YouTube channel and its niche, analysed: the channel's numbers and best videos, what commenters say and ask for, the automation (what it is making, scheduled releases, results), the best release times (competitors' golden hours and the channel's own history), and the competitor research gathered every hour (top recap channels, what their best videos share in titles, length, tags and upload times, who changed titles or thumbnails and what happened after). Use it first for any question about growing the channel, what to make next, titles, thumbnails, timing or competitors.".into(),
            parameters: obj(json!({ "focus": { "type": "string", "description": "Optional: channel, comments, automation, timing or competitors to get only that part" } }), &[]),
            risk: Risk::Read,
        },
        AppTool {
            name: "youtube_competitors",
            description: "Recap channels the competitor research tracks, with subscribers, growth, upload rate and views. Filter by a word (name, keyword, country).".into(),
            parameters: obj(json!({ "query": { "type": "string" }, "sort": { "type": "string", "enum": ["subscribers", "growth", "newest"] }, "limit": { "type": "integer" } }), &[]),
            risk: Risk::Read,
        },
        AppTool {
            name: "youtube_competitor_channel",
            description: "One competitor channel in detail: description, keywords, country, subscriber history, uploads per week, upload hours and days (user's local time), top tags, and its recent videos with views, views per hour, exact publish time, and title/thumbnail change counts. Pass its name, @handle or channel id.".into(),
            parameters: obj(json!({ "channel": { "type": "string" } }), &["channel"]),
            risk: Risk::Read,
        },
        AppTool {
            name: "youtube_competitor_video",
            description: "One competitor video's full history: every title it had with the date it changed, every thumbnail version (dates and image files), description edits, tags, and views hour by hour.".into(),
            parameters: obj(json!({ "video_id": { "type": "string" } }), &["video_id"]),
            risk: Risk::Read,
        },
        AppTool {
            name: "youtube_comments",
            description: "Comments on the user's videos (newest first, with replies), as last read; optionally one video's or only unanswered ones.".into(),
            parameters: obj(json!({ "video_id": { "type": "string" }, "unanswered": { "type": "boolean" }, "limit": { "type": "integer" } }), &[]),
            risk: Risk::Read,
        },
        AppTool {
            name: "youtube_reply",
            description: "Posts a reply as the channel to a comment thread (thread id from youtube_comments). Costs 50 of the daily 10,000 API units.".into(),
            parameters: obj(json!({ "thread_id": { "type": "string" }, "text": { "type": "string" } }), &["thread_id", "text"]),
            risk: Risk::Act,
        },
        AppTool {
            name: "youtube_automation",
            description: "The Studio automation: with no action, its state (full auto on/off, videos in production and their stage, scheduled releases, results, discover picks, the latest log). Actions: discover (find hot manga now), start (make a recap of a discover pick: pass its title), approve (a finished render: pass the job's title), full_auto_on, full_auto_off.".into(),
            parameters: obj(json!({ "action": { "type": "string", "enum": ["status", "discover", "start", "approve", "full_auto_on", "full_auto_off"] }, "title": { "type": "string" } }), &[]),
            risk: Risk::Act,
        },
    ]
}

impl ToolProvider for YouTubeBrain {
    fn tools(&self) -> Vec<AppTool> {
        tools()
    }

    fn phrases(&self, name: &str, args: &Map<String, Value>) -> Option<Phrases> {
        Some(match name {
            "brain_youtube" => Phrases::new("Read", "Reading", "Read", "what MangaVox knows about your channel and niche"),
            "youtube_competitors" => Phrases::new("Read", "Reading", "Read", "the competitor channels"),
            "youtube_competitor_channel" => Phrases::new("Read", "Reading", "Read", arg(args, "channel").unwrap_or("the channel")),
            "youtube_competitor_video" => Phrases::new("Read", "Reading", "Read", "the video's history"),
            "youtube_comments" => Phrases::new("Read", "Reading", "Read", "your comments"),
            "youtube_reply" => Phrases::new("Reply", "Replying", "Replied", "to the comment"),
            "youtube_automation" => match arg(args, "action").unwrap_or("status") {
                "status" => Phrases::new("Check", "Checking", "Checked", "the automation"),
                "full_auto_on" => Phrases::new("Turn on", "Turning on", "Turned on", "full auto"),
                "full_auto_off" => Phrases::new("Turn off", "Turning off", "Turned off", "full auto"),
                a => Phrases::new("Run", "Running", "Ran", &format!("automation {a}")),
            },
            _ => return None,
        })
    }

    fn run(&self, name: &str, args: Map<String, Value>) -> Option<BoxFuture<'static, ToolResult>> {
        let name = name.to_string();
        if !tools().iter().any(|t| t.name == name) {
            return None;
        }
        Some(async move { execute(&name, &args).await.map(ToolOutput::Value) }.boxed())
    }

    fn prompt(&self) -> Option<String> {
        Some("For anything about the user's YouTube channel, what to recap next, titles, thumbnails, release timing or competitors, call brain_youtube first: it holds the analysed channel, comments, automation and hourly competitor data. Mention times in 12-hour local time (\"6:30 PM\").".into())
    }
}

async fn execute(name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    match name {
        "brain_youtube" => {
            let focus = arg(args, "focus").map(str::to_lowercase);
            let want_rivals = focus.as_deref().is_none_or(|f| f.contains("competitor"));
            let f2 = focus.clone();
            // The live state on the UI thread; the competitor files (hundreds of them) off it.
            let mut brief = ui_assistant::on_ui(move |_, cx| brain(f2.as_deref(), cx)).await?;
            if want_rivals {
                let rivals = mv_ui::bg::blocking(|| competitors::brief(&CompStore::default(), now(), utc_offset())).await.map_err(|e| e.to_string())?;
                brief.push_str("\n## Competitors (hourly research)\n");
                brief.push_str(&rivals);
                brief.push('\n');
            }
            Ok(json!({ "brain": brief }))
        }
        "youtube_competitors" => {
            let q = arg(args, "query").unwrap_or("").to_lowercase();
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(25).clamp(1, 100) as usize;
            let sort = arg(args, "sort").unwrap_or("subscribers").to_string();
            mv_ui::bg::blocking(move || {
                let store = CompStore::default();
                let mut list: Vec<CompChannel> = store.channels().into_iter().filter(|c| !c.hidden).filter(|c| q.is_empty() || c.title.to_lowercase().contains(&q) || c.handle.to_lowercase().contains(&q) || c.keywords.iter().any(|k| k.to_lowercase().contains(&q)) || c.country.to_lowercase().contains(&q)).collect();
                let gain = |c: &CompChannel| c.subs_history.last().map(|p| p.value as i64).unwrap_or(0) - c.subs_history.iter().find(|p| p.at >= now() - 7 * 86_400).map(|p| p.value as i64).unwrap_or(0);
                match sort.as_str() {
                    "growth" => list.sort_by_key(|c| std::cmp::Reverse(gain(c))),
                    "newest" => list.sort_by_key(|c| std::cmp::Reverse(c.first_seen)),
                    _ => list.sort_by_key(|c| std::cmp::Reverse(c.subscribers)),
                }
                let total = list.len();
                let rows: Vec<Value> = list.iter().take(limit).map(|c| json!({ "id": c.id, "title": c.title, "handle": c.handle, "subscribers": c.subscribers, "subscribers_7d": gain(c), "videos": c.video_count, "country": c.country, "keywords": c.keywords.iter().take(8).collect::<Vec<_>>(), "pinned": c.pinned })).collect();
                json!({ "total": total, "channels": rows })
            })
            .await
            .map_err(|e| e.to_string())
        }
        "youtube_competitor_channel" => {
            let want = arg(args, "channel").unwrap_or("").to_lowercase();
            mv_ui::bg::blocking(move || {
                let store = CompStore::default();
                let channels = store.channels();
                let c = channels
                    .iter()
                    .find(|c| c.id.to_lowercase() == want || c.handle.to_lowercase() == want || c.handle.to_lowercase() == format!("@{want}"))
                    .or_else(|| channels.iter().find(|c| c.title.to_lowercase() == want))
                    .or_else(|| channels.iter().find(|c| c.title.to_lowercase().contains(&want)))
                    .cloned()
                    .ok_or_else(|| format!("No tracked channel matches \"{want}\". youtube_competitors lists them."))?;
                let videos = store.videos(&c.id);
                let s = competitors::channel_stats(&c, &videos, now(), utc_offset());
                let mut recent = videos.clone();
                recent.sort_by_key(|v| std::cmp::Reverse(v.published_at));
                let rows: Vec<Value> = recent
                    .iter()
                    .take(25)
                    .map(|v| json!({ "id": v.id, "title": v.title, "published": when::when(v.published_at), "exact_time": v.published_exact, "views": v.views, "views_per_hour": competitors::velocity(v, now()).round(), "duration": crate::yt::duration(v.duration_secs), "title_changes": v.titles.len().saturating_sub(1), "thumbnail_changes": v.thumbnails.len().saturating_sub(1), "tags": v.tags.iter().take(10).collect::<Vec<_>>() }))
                    .collect();
                let busiest: Vec<String> = {
                    let mut h: Vec<(usize, u32)> = s.upload_hours.iter().copied().enumerate().filter(|(_, n)| *n > 0).collect();
                    h.sort_by_key(|x| std::cmp::Reverse(x.1));
                    h.into_iter().take(4).map(|(h, n)| format!("{} ({n})", when::hour(h))).collect()
                };
                Ok(json!({
                    "channel": { "id": c.id, "title": c.title, "handle": c.handle, "subscribers": c.subscribers, "videos": c.video_count, "country": c.country, "joined": c.joined, "keywords": c.keywords, "description": c.description.chars().take(800).collect::<String>(), "tracked_since": when::date(c.first_seen) },
                    "stats": { "uploads_per_week": s.uploads_per_week, "avg_views": s.avg_views.round(), "median_views": s.median_views, "views_last_24h": s.views_last_24h, "typical_length": crate::yt::duration(s.avg_duration_secs as u64), "subscribers_7d": s.subs_gained_7d, "title_changes": s.title_changes, "thumbnail_changes": s.thumb_changes, "upload_hours_local": busiest, "top_tags": s.top_tags.iter().take(15).collect::<Vec<_>>() },
                    "recent_videos": rows,
                }))
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "youtube_competitor_video" => {
            let id = arg(args, "video_id").unwrap_or("").to_string();
            mv_ui::bg::blocking(move || {
                let store = CompStore::default();
                for c in store.channels() {
                    if let Some(v) = store.videos(&c.id).into_iter().find(|v| v.id == id) {
                        let gains = competitors::hourly_gains(&v);
                        return Ok(json!({
                            "id": v.id, "channel": c.title, "title": v.title, "views": v.views, "likes": v.likes, "comments": v.comments,
                            "published": when::when(v.published_at), "exact_time": v.published_exact, "duration": crate::yt::duration(v.duration_secs),
                            "tags": v.tags, "description": v.description.chars().take(1500).collect::<String>(),
                            "titles": v.titles.iter().map(|t| json!({ "since": when::when(t.at), "title": t.value })).collect::<Vec<_>>(),
                            "thumbnails": v.thumbnails.iter().map(|t| json!({ "since": when::when(t.at), "file": store.thumb_path(t), "url": t.url })).collect::<Vec<_>>(),
                            "description_edits": v.descriptions.len().saturating_sub(1),
                            "views_gained_by_hour": gains.iter().rev().take(48).rev().map(|(at, n)| json!([when::when(*at), n])).collect::<Vec<_>>(),
                        }));
                    }
                }
                Err(format!("Video {id} is not in the competitor research"))
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "youtube_comments" => {
            let video = arg(args, "video_id").map(str::to_string);
            let unanswered = args.get("unanswered").and_then(Value::as_bool).unwrap_or(false);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(30).clamp(1, 100) as usize;
            ui_assistant::on_ui(move |_, cx| {
                let h = crate::hub::hub(cx);
                let h = h.read(cx);
                let rows: Vec<Value> = h
                    .threads
                    .iter()
                    .filter(|t| video.as_ref().is_none_or(|v| &t.video_id == v))
                    .filter(|t| !unanswered || (!t.top.mine && !t.answered()))
                    .take(limit)
                    .map(|t| json!({ "thread_id": t.id, "video": h.video(&t.video_id).map(|v| v.title.clone()).unwrap_or_else(|| t.video_id.clone()), "author": t.top.author, "when": when::when(t.top.published_at), "likes": t.top.likes, "text": t.top.text, "answered": t.answered(), "replies": t.replies.iter().map(|r| json!({ "author": r.author, "mine": r.mine, "text": r.text })).collect::<Vec<_>>() }))
                    .collect();
                json!({ "read_at": when::when(h.comments_at), "can_reply": h.account.can_write_comments(), "threads": rows })
            })
            .await
        }
        "youtube_reply" => {
            let (thread, text) = (arg(args, "thread_id").unwrap_or("").to_string(), arg(args, "text").unwrap_or("").to_string());
            mv_youtube::comments::check_reply(&text).map_err(|e| e.to_string())?;
            let yt = ui_assistant::on_ui(|_, cx| crate::hub::hub(cx).read(cx).yt.clone()).await?;
            let c = mv_ui::bg::run(async move { yt.reply(&thread, &text).await }).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))?;
            Ok(json!({ "posted": true, "comment_id": c.id }))
        }
        "youtube_automation" => {
            let action = arg(args, "action").unwrap_or("status").to_string();
            let title = arg(args, "title").unwrap_or("").to_lowercase();
            ui_assistant::on_ui(move |_, cx| {
                let r = crate::automation::runner(cx);
                match action.as_str() {
                    "discover" => r.update(cx, |r, cx| r.discover(cx)),
                    "full_auto_on" => r.update(cx, |r, cx| r.set_full_auto(true, cx)),
                    "full_auto_off" => r.update(cx, |r, cx| r.set_full_auto(false, cx)),
                    "start" => {
                        let pick = r.read(cx).state.picks.iter().find(|p| p.title.to_lowercase().contains(&title)).cloned();
                        match pick {
                            Some(p) => r.update(cx, |r, cx| r.start_job(p, false, cx)),
                            None => return Err(format!("No discover pick matches \"{title}\"; run discover first")),
                        }
                    }
                    "approve" => {
                        let id = r.read(cx).state.jobs.iter().find(|j| j.state == mv_youtube::automation::JobState::Waiting && (title.is_empty() || j.pick.title.to_lowercase().contains(&title))).map(|j| j.id.clone());
                        match id {
                            Some(id) => r.update(cx, |r, cx| r.approve(&id, cx)),
                            None => return Err("No render is waiting for approval".into()),
                        }
                    }
                    _ => {}
                }
                Ok(automation_json(&r.read(cx).state))
            })
            .await?
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

fn automation_json(s: &mv_youtube::automation::Automation) -> Value {
    json!({
        "full_auto": s.full_auto,
        "approve_renders": s.approve_renders,
        "chapters_per_video": s.max_chapters,
        "release_every_days": s.cadence_days,
        "jobs": s.jobs.iter().rev().take(10).map(|j| json!({ "title": j.pick.title, "stage": j.stage.label(), "state": format!("{:?}", j.state), "step": j.step, "chapters": j.chapters.len(), "release": j.publish_at.map(when::when), "why_that_time": j.why_time, "video": j.video_id.as_ref().map(|v| format!("https://youtu.be/{v}")), "results": j.results.as_ref().map(|r| json!({ "views": r.views, "likes": r.likes, "comments": r.comments, "retention": format!("{:.0}%", r.retention * 100.) })), "error": j.error })).collect::<Vec<_>>(),
        "discover_picks": s.picks.iter().take(8).map(|p| json!({ "title": p.title, "score": p.score, "why": p.reason, "asked_by_viewers": p.asked })).collect::<Vec<_>>(),
        "viewer_requests": s.requests,
        "log": s.log.iter().rev().take(15).map(|e| format!("{}: {}", when::when(e.at), e.text)).collect::<Vec<_>>(),
    })
}

/// The analysed brief `brain_youtube` returns (on the UI thread: it reads the live state).
fn brain(focus: Option<&str>, cx: &mut gpui_kit::App) -> String {
    let want = |k: &str| focus.is_none_or(|f| f.contains(k));
    let mut out = String::new();
    let h = crate::hub::hub(cx);
    let h = h.read(cx);
    if want("channel") {
        out.push_str("## Your channel\n");
        match &h.channel {
            Some(c) => {
                out.push_str(&format!("{} ({}): {} subscribers, {} views, {} videos.\n", c.title, c.handle, fmt_num(c.subscribers), fmt_num(c.views), c.video_count));
                let v28: u64 = h.channel_daily.iter().rev().take(28).map(|d| d.views).sum();
                let s28: i64 = h.channel_daily.iter().rev().take(28).map(|d| d.subs_gained).sum();
                out.push_str(&format!("Last 28 days: {} views, {s28:+} subscribers.\n", fmt_num(v28)));
                let mut top: Vec<_> = h.videos.iter().filter(|v| v.privacy == "public").collect();
                top.sort_by_key(|v| std::cmp::Reverse(v.views));
                for v in top.iter().take(5) {
                    out.push_str(&format!("- \"{}\": {} views, {} likes, {} comments, {} long, released {}\n", v.title, fmt_num(v.views), v.likes, v.comments, crate::yt::duration(v.duration_secs), when::when(v.published_at)));
                }
            }
            None => out.push_str("No channel connected.\n"),
        }
    }
    if want("comment") {
        let f = mv_youtube::comments::feedback(&h.threads);
        out.push_str(&format!("\n## What viewers say ({} comments read {})\n", f.comments, if h.comments_at > 0 { when::when(h.comments_at) } else { "never".into() }));
        out.push_str(&format!("{} unanswered, {} questions, {} positive, {} critical.\n", f.unanswered, f.questions, f.positive, f.negative));
        if !f.requests.is_empty() {
            let mut r: Vec<_> = f.requests.into_iter().collect();
            r.sort_by_key(|x| std::cmp::Reverse(x.1));
            out.push_str(&format!("Asked to recap: {}.\n", r.iter().take(8).map(|(t, n)| format!("{t} ({n})")).collect::<Vec<_>>().join(", ")));
        }
        for t in h.threads.iter().filter(|t| !t.top.mine).take(5) {
            out.push_str(&format!("- {}: \"{}\"\n", t.top.author, t.top.text.chars().take(160).collect::<String>()));
        }
    }
    if want("timing") {
        let a = h.timing_analysis();
        out.push_str("\n## Release timing (your local time)\n");
        if a.golden.is_empty() {
            out.push_str("No upload-time research yet (YouTube › Schedule).\n");
        } else {
            out.push_str(&format!(
                "From {} recap uploads by {} channels: best windows {}.\n",
                a.uploads,
                a.channels,
                a.golden.iter().take(3).map(|g| format!("{} ({:.1}× their usual views)", when::hour_span(g.start_hour, g.hours), g.cell.lift())).collect::<Vec<_>>().join(", ")
            ));
        }
        let own = mv_youtube::automation::own_hour_lift(&h.videos, now(), utc_offset());
        let mut best: Vec<(usize, f64, usize)> = own.iter().enumerate().filter(|(_, (_, n))| *n > 0).map(|(i, (l, n))| (i, *l, *n)).collect();
        best.sort_by(|a, b| b.1.total_cmp(&a.1));
        if !best.is_empty() {
            out.push_str(&format!("Your own videos by release hour: {}.\n", best.iter().take(4).map(|(h, l, n)| format!("{} {l:.1}× ({n} videos)", when::hour(*h))).collect::<Vec<_>>().join(", ")));
        }
        out.push_str("Studies of long-form uploads: Sunday and Saturday slightly above average, Wednesday slightly below; uploads 2-3 hours before the evening peak get indexed in time.\n");
    }
    let r = crate::automation::runner(cx).read(cx);
    if want("automation") {
        let s = &r.state;
        out.push_str(&format!("\n## Automation (full auto {})\n", if s.full_auto { "ON" } else { "off" }));
        for j in s.jobs.iter().rev().take(6) {
            out.push_str(&format!("- {}: {} · {}{}{}\n", j.pick.title, j.stage.label(), j.step, j.publish_at.map(|t| format!(" · release {}", when::when(t))).unwrap_or_default(), j.results.as_ref().map(|r| format!(" · {} views, {:.0}% retention", fmt_num(r.views), r.retention * 100.)).unwrap_or_default()));
        }
        if !s.picks.is_empty() {
            out.push_str(&format!("Discover picks: {}.\n", s.picks.iter().take(5).map(|p| format!("{} ({}/100)", p.title, p.score)).collect::<Vec<_>>().join(", ")));
        }
    }
    out
}
