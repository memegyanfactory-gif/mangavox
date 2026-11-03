//! Comments on the channel's videos: threads with their replies (`commentThreads.list`, 1 unit a
//! page of up to 100), more replies of one thread (`comments.list`, 1 unit), and answering from
//! the app (`comments.insert`, 50 units). Writing needs the `youtube.force-ssl` scope, which
//! channels connected before it was added must grant by connecting again.
//! <https://developers.google.com/youtube/v3/docs/commentThreads>
use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::{YouTube, DATA_API};

/// One comment or reply.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    /// "@handle" or display name.
    pub author: String,
    pub author_channel_id: String,
    pub author_avatar: String,
    /// Plain text (`textFormat=plainText`).
    pub text: String,
    pub likes: u64,
    pub published_at: i64,
    pub updated_at: i64,
    /// Written by the connected channel itself.
    pub mine: bool,
    /// The top-level comment this replies to.
    pub parent_id: Option<String>,
}

/// A top-level comment and its replies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub video_id: String,
    pub top: Comment,
    /// Oldest first. Up to five come with the thread; [`YouTube::replies`] reads the rest.
    pub replies: Vec<Comment>,
    pub reply_count: u64,
    pub can_reply: bool,
}

impl Thread {
    /// The channel has answered in this thread.
    pub fn answered(&self) -> bool {
        self.replies.iter().any(|r| r.mine)
    }

    /// Every reply is loaded.
    pub fn complete(&self) -> bool {
        self.replies.len() as u64 >= self.reply_count
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadPage {
    pub threads: Vec<Thread>,
    pub next_page: Option<String>,
}

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn n(v: &Value, ptr: &str) -> u64 {
    v.pointer(ptr).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0)
}

fn time(v: &Value, ptr: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&s(v, ptr)).map(|d| d.timestamp()).unwrap_or(0)
}

/// A `youtube#comment` resource.
pub fn parse_comment(item: &Value, channel_id: &str) -> Comment {
    let author_channel_id = s(item, "/snippet/authorChannelId/value");
    let text = {
        let o = s(item, "/snippet/textOriginal");
        if o.is_empty() {
            s(item, "/snippet/textDisplay")
        } else {
            o
        }
    };
    Comment {
        id: s(item, "/id"),
        author: s(item, "/snippet/authorDisplayName"),
        mine: !channel_id.is_empty() && author_channel_id == channel_id,
        author_channel_id,
        author_avatar: s(item, "/snippet/authorProfileImageUrl"),
        text,
        likes: n(item, "/snippet/likeCount"),
        published_at: time(item, "/snippet/publishedAt"),
        updated_at: time(item, "/snippet/updatedAt"),
        parent_id: Some(s(item, "/snippet/parentId")).filter(|p| !p.is_empty()),
    }
}

/// A `youtube#commentThread` resource (with `part=snippet,replies`).
pub fn parse_thread(item: &Value, channel_id: &str) -> Thread {
    let top = item.pointer("/snippet/topLevelComment").map(|c| parse_comment(c, channel_id)).unwrap_or_default();
    let mut replies: Vec<Comment> = item.pointer("/replies/comments").and_then(Value::as_array).into_iter().flatten().map(|c| parse_comment(c, channel_id)).collect();
    replies.sort_by_key(|r| r.published_at);
    Thread {
        id: s(item, "/id"),
        video_id: s(item, "/snippet/videoId"),
        top,
        replies,
        reply_count: n(item, "/snippet/totalReplyCount"),
        can_reply: item.pointer("/snippet/canReply").and_then(Value::as_bool).unwrap_or(true),
    }
}

/// YouTube's comment limit is 10,000 characters; empty replies are refused.
pub fn check_reply(text: &str) -> Result<String> {
    let t = text.trim();
    if t.is_empty() {
        bail!("Write a reply first");
    }
    if t.chars().count() > 10_000 {
        bail!("A comment can be at most 10,000 characters");
    }
    Ok(t.to_string())
}

/// What viewers say across many threads, for the suggestions: how much of it is a question,
/// how warm it is, and which titles they ask for ("do Omniscient Reader next").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Feedback {
    pub comments: u64,
    pub questions: u64,
    pub positive: u64,
    pub negative: u64,
    pub unanswered: u64,
    /// Text after "do", "recap", "next", "please make"... with how often it came up.
    pub requests: BTreeMap<String, u32>,
}

const POSITIVE: &[&str] = &["love", "great", "amazing", "best", "awesome", "thank", "goat", "fire", "peak", "good", "nice", "keep it up", "❤", "🔥"];
const NEGATIVE: &[&str] = &["boring", "bad", "worst", "wrong", "skip", "ai voice", "robot", "too fast", "too slow", "hate", "spoiler", "mistake", "dislike"];
const ASKS: &[&str] = &["do a recap of ", "make a recap of ", "recap of ", "please do ", "pls do ", "can you do ", "could you do ", "next do ", "please make ", "do "];
/// Words after a title that are not part of it ("do Solo Leveling next pls").
const FILLER: &[&str] = &["next", "pls", "plz", "please", "too", "also", "bro", "sir", "now"];

/// The title a viewer asks for in `text`, cleaned ("please do Solo Leveling!" -> "solo leveling").
pub fn requested_title(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for a in ASKS {
        if let Some(pos) = lower.find(a) {
            if *a == "do " && pos != 0 {
                continue;
            }
            let rest = &lower[pos + a.len()..];
            let title: String = rest.split(|c: char| matches!(c, '.' | '!' | '?' | ',' | '\n' | '(' | ')')).next().unwrap_or("").trim().chars().take(60).collect();
            let mut words: Vec<&str> = title.trim_start_matches("the manga ").trim_start_matches("the manhwa ").split_whitespace().collect();
            while words.last().is_some_and(|w| FILLER.contains(w)) {
                words.pop();
            }
            let title = words.join(" ");
            let words = title.split_whitespace().count();
            if (1..=8).contains(&words) && !["it", "this", "that", "more", "next", "part 2", "the next one"].contains(&title.as_str()) {
                return Some(title);
            }
        }
    }
    None
}

/// Tallies [`Feedback`] over `threads` (top-level comments and others' replies).
pub fn feedback(threads: &[Thread]) -> Feedback {
    let mut f = Feedback::default();
    for t in threads {
        if !t.answered() && !t.top.mine {
            f.unanswered += 1;
        }
        for c in std::iter::once(&t.top).chain(t.replies.iter()).filter(|c| !c.mine) {
            f.comments += 1;
            let l = c.text.to_lowercase();
            if l.contains('?') {
                f.questions += 1;
            }
            if POSITIVE.iter().any(|w| l.contains(w)) {
                f.positive += 1;
            }
            if NEGATIVE.iter().any(|w| l.contains(w)) {
                f.negative += 1;
            }
            if let Some(title) = requested_title(&c.text) {
                *f.requests.entry(title).or_default() += 1;
            }
        }
    }
    f
}

/// YouTube refused because the video has comments turned off.
fn comments_disabled(e: &anyhow::Error) -> bool {
    crate::api::error_reason(e).as_deref() == Some("commentsDisabled") || {
        let text = format!("{e:#}");
        text.contains("commentsDisabled") || text.contains("disabled comments")
    }
}

impl YouTube {
    /// Newest threads first: of one video, or of the whole channel when `video_id` is None.
    /// `max` 1..=100 per page (1 unit a page).
    pub async fn comment_threads(&self, video_id: Option<&str>, page: Option<&str>, max: u32) -> Result<ThreadPage> {
        let channel_id = self.account().await.channel_id.unwrap_or_default();
        let mut q: Vec<(&str, String)> = vec![("part", "snippet,replies".into()), ("order", "time".into()), ("textFormat", "plainText".into()), ("maxResults", max.clamp(1, 100).to_string())];
        match video_id {
            Some(v) => q.push(("videoId", v.to_string())),
            None if !channel_id.is_empty() => q.push(("allThreadsRelatedToChannelId", channel_id.clone())),
            None => bail!("Connect your channel first"),
        }
        if let Some(p) = page {
            q.push(("pageToken", p.to_string()));
        }
        let v = match self.get_json(&format!("{DATA_API}/commentThreads"), &q).await {
            Ok(v) => v,
            // Comments turned off on the video: no threads rather than an error.
            Err(e) if comments_disabled(&e) => return Ok(ThreadPage::default()),
            Err(e) => return Err(e),
        };
        Ok(ThreadPage {
            threads: v.get("items").and_then(Value::as_array).into_iter().flatten().map(|i| parse_thread(i, &channel_id)).collect(),
            next_page: v.get("nextPageToken").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// Every reply of a thread, oldest first (`comments.list?parentId`, 1 unit per 100).
    pub async fn replies(&self, thread_id: &str) -> Result<Vec<Comment>> {
        let channel_id = self.account().await.channel_id.unwrap_or_default();
        let mut out = vec![];
        let mut page: Option<String> = None;
        loop {
            let mut q: Vec<(&str, String)> = vec![("part", "snippet".into()), ("parentId", thread_id.to_string()), ("textFormat", "plainText".into()), ("maxResults", "100".into())];
            if let Some(p) = &page {
                q.push(("pageToken", p.clone()));
            }
            let v = self.get_json(&format!("{DATA_API}/comments"), &q).await?;
            out.extend(v.get("items").and_then(Value::as_array).into_iter().flatten().map(|c| parse_comment(c, &channel_id)));
            page = v.get("nextPageToken").and_then(Value::as_str).map(str::to_string);
            if page.is_none() || out.len() >= 500 {
                break;
            }
        }
        out.sort_by_key(|r| r.published_at);
        Ok(out)
    }

    /// Answers a thread as the channel (`comments.insert`, 50 units). Needs `youtube.force-ssl`.
    pub async fn reply(&self, thread_id: &str, text: &str) -> Result<Comment> {
        let text = check_reply(text)?;
        if !self.account().await.can_write_comments() {
            bail!("Replying needs one more permission. Connect your channel again in YouTube › Settings to allow managing comments.");
        }
        let channel_id = self.account().await.channel_id.unwrap_or_default();
        let body = json!({ "snippet": { "parentId": thread_id, "textOriginal": text } });
        let v = self.send_json(reqwest::Method::POST, &format!("{DATA_API}/comments"), &[("part", "snippet".into())], &body).await?;
        let mut c = parse_comment(&v, &channel_id);
        if c.id.is_empty() {
            return Err(anyhow!("YouTube accepted the reply but sent no comment back"));
        }
        c.mine = true;
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_disabled_is_detected_by_reason_and_wording() {
        let body = r#"{"error":{"message":"The video identified by the videoId parameter has disabled comments.","errors":[{"reason":"commentsDisabled"}]}}"#;
        assert!(comments_disabled(&crate::api::test_error(403, body)));
        let no_reason = r#"{"error":{"message":"The video identified by the videoId parameter has disabled comments."}}"#;
        assert!(comments_disabled(&crate::api::test_error(403, no_reason)));
        assert!(!comments_disabled(&crate::api::test_error(403, r#"{"error":{"message":"Forbidden","errors":[{"reason":"forbidden"}]}}"#)));
    }

    fn thread_json() -> Value {
        json!({
            "id": "Ug1",
            "snippet": {
                "videoId": "vid1",
                "canReply": true,
                "totalReplyCount": 2,
                "topLevelComment": {
                    "id": "Ug1",
                    "snippet": {
                        "authorDisplayName": "@reader",
                        "authorProfileImageUrl": "https://yt3.ggpht.com/a.jpg",
                        "authorChannelId": { "value": "UCviewer" },
                        "textDisplay": "Please do Omniscient Reader next!",
                        "textOriginal": "Please do Omniscient Reader next!",
                        "likeCount": 12,
                        "publishedAt": "2026-10-30T18:00:00Z",
                        "updatedAt": "2026-10-30T18:00:00Z"
                    }
                }
            },
            "replies": { "comments": [
                { "id": "Ug1.b", "snippet": { "authorDisplayName": "@me", "authorChannelId": { "value": "UCme" }, "textOriginal": "On it!", "parentId": "Ug1", "likeCount": 1, "publishedAt": "2026-10-30T20:00:00Z" } },
                { "id": "Ug1.a", "snippet": { "authorDisplayName": "@other", "authorChannelId": { "value": "UCx" }, "textOriginal": "Same, it's the best", "parentId": "Ug1", "likeCount": "3", "publishedAt": "2026-10-30T19:00:00Z" } }
            ]}
        })
    }

    #[test]
    fn parses_threads() {
        let t = parse_thread(&thread_json(), "UCme");
        assert_eq!(t.id, "Ug1");
        assert_eq!(t.video_id, "vid1");
        assert_eq!(t.top.author, "@reader");
        assert_eq!(t.top.likes, 12);
        assert_eq!(t.top.published_at, 1_793_383_200);
        assert_eq!(t.reply_count, 2);
        // Oldest reply first, string counts read, the channel's own reply marked.
        assert_eq!(t.replies[0].id, "Ug1.a");
        assert_eq!(t.replies[0].likes, 3);
        assert!(t.replies[1].mine && t.answered() && t.complete());
        assert_eq!(t.replies[1].parent_id.as_deref(), Some("Ug1"));
    }

    #[test]
    fn reply_text_is_checked() {
        assert!(check_reply("   ").is_err());
        assert_eq!(check_reply("  thanks! ").unwrap(), "thanks!");
        assert!(check_reply(&"a".repeat(10_001)).is_err());
    }

    #[test]
    fn requests_and_feedback() {
        assert_eq!(requested_title("Please do Omniscient Reader next!").as_deref(), Some("omniscient reader"));
        assert_eq!(requested_title("do solo leveling pls").as_deref(), Some("solo leveling"));
        assert_eq!(requested_title("Can you do a recap of Nano Machine?").as_deref(), Some("nano machine"));
        assert_eq!(requested_title("I do love this"), None);
        assert_eq!(requested_title("great video"), None);
        let f = feedback(&[parse_thread(&thread_json(), "UCme")]);
        assert_eq!(f.comments, 2);
        assert_eq!(f.positive, 1);
        assert_eq!(f.unanswered, 0);
        assert_eq!(f.requests.values().sum::<u32>(), 1);
    }
}
