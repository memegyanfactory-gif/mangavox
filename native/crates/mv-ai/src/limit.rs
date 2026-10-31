//! Provider errors that arrive dressed as answers, and usage / rate limits.
//!
//! CLI agents (Claude Code, Codex, Gemini CLI, …) and some HTTP gateways print "You've hit your
//! session limit · resets 8am" or "Quota exceeded" as ordinary reply text. [`provider_error_in`]
//! spots such a reply so it becomes an error instead of narration, and [`limit_of`] says whether
//! an error is a limit worth waiting out, and until when.

use std::sync::OnceLock;

use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// The provider is refusing work for now: a usage, rate or quota limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiLimit {
    pub message: String,
    /// When the provider said it resets (unix seconds), if it said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
}

impl AiLimit {
    /// Seconds to wait before trying again: until the reset (plus a minute), else a backoff by
    /// attempt (2, 5, 10, then every 15 minutes). Never under 30 s or over 6 h.
    pub fn wait_secs(&self, attempt: u32, now: i64) -> u64 {
        let backoff = [120, 300, 600, 900][attempt.min(3) as usize];
        let secs = match self.retry_at {
            Some(t) if t > now => t - now + 60,
            _ => backoff,
        };
        secs.clamp(30, 6 * 3600) as u64
    }
}

fn limit_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            hit\ your\ (?:\w+\ )?limit | (?:usage|session|weekly|daily|monthly|token|rate|request)[\s_-]?limits?\ (?:reached|exceeded|hit) |
            limit\ (?:reached|exceeded) | rate[\s_-]?limit(?:ed|ing)? | too\ many\ requests | \b429\b | quota | resource[\s_-]?exhausted |
            insufficient[\s_-]?quota | out\ of\ (?:credits|tokens|quota) | credit\ balance\ is\ too\ low | overloaded | capacity\ (?:exceeded|reached) |
            upgrade\ (?:to|your\ plan)\ .{0,40}(?:limit|usage) | try\ again\ (?:later|in\ \d) | resets?\ (?:at\ )?\d",
        )
        .unwrap()
    })
}

/// What only a provider says about itself, never a story or a transcript: a reply matching this is
/// an error. Stricter than [`limit_re`], which judges messages already known to be errors.
fn reply_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            hit\ your\ (?:\w+\ )?limit | (?:usage|session|weekly|daily|monthly)\ limit | limit\ reached\s*\|\s*\d{10} |
            rate[\s_-]?limit(?:ed)?\ (?:reached|exceeded|for) | rate_limit_error | too\ many\ requests | exceeded\ your\ current\ quota |
            quota\ (?:exceeded|metric) | resource[\s_-]?exhausted | insufficient[\s_-]?quota | credit\ balance\ is\ too\ low |
            overloaded_error | ^\s*api\ error\b | \b(?:429|500|502|503|529)\ (?:too\ many|internal|bad\ gateway|service\ unavailable|overloaded) |
            invalid\ api\ key | please\ run\ /login | not\ logged\ in\ ·",
        )
        .unwrap()
    })
}

/// True when `msg` (an error, or a reply) is a usage / rate / quota limit.
pub fn is_limit(msg: &str) -> bool {
    limit_re().is_match(msg)
}

/// The limit in an error message, with its reset time when the message names one.
pub fn limit_of(msg: &str) -> Option<AiLimit> {
    is_limit(msg).then(|| AiLimit { message: msg.trim().to_string(), retry_at: retry_at(msg, Local::now()) })
}

/// A reply that is really the provider talking about itself (a limit or an error), not an answer.
/// Only short plain-text replies are judged: a JSON answer, or real prose, is left alone.
pub fn provider_error_in(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.len() > 600 || t.starts_with('{') && !t.contains("\"error\"") || t.starts_with('[') || t.starts_with("```") {
        return None;
    }
    reply_re().is_match(t).then(|| t.to_string())
}

/// When a limit message says it resets: "try again in 20s", "resets in 3 hours", "resets 8am",
/// "resets at 3:45 pm", "Retry after 60 seconds", or Claude Code's "limit reached|1730000000".
pub fn retry_at(msg: &str, now: DateTime<Local>) -> Option<i64> {
    static IN: OnceLock<Regex> = OnceLock::new();
    static AT: OnceLock<Regex> = OnceLock::new();
    static STAMP: OnceLock<Regex> = OnceLock::new();
    let stamp = STAMP.get_or_init(|| Regex::new(r"\|\s*(\d{10})\b").unwrap());
    if let Some(t) = stamp.captures(msg).and_then(|c| c[1].parse::<i64>().ok()) {
        return Some(t);
    }
    let within = IN.get_or_init(|| {
        Regex::new(r"(?i)(?:in|after)\s+(\d+(?:\.\d+)?)\s*(s|secs?|seconds?|m|mins?|minutes?|h|hrs?|hours?)\b").unwrap()
    });
    if let Some(c) = within.captures(msg) {
        let n: f64 = c[1].parse().ok()?;
        let unit = c[2].to_lowercase();
        let secs = match unit.chars().next() {
            Some('h') => n * 3600.,
            Some('m') => n * 60.,
            _ => n,
        };
        return Some(now.timestamp() + secs.ceil() as i64);
    }
    let at = AT.get_or_init(|| Regex::new(r"(?i)resets?\s+(?:at\s+|on\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?").unwrap());
    let c = at.captures(msg)?;
    let mut h: u32 = c[1].parse().ok()?;
    let m: u32 = c.get(2).map_or(Some(0), |m| m.as_str().parse().ok())?;
    match c.get(3).map(|x| x.as_str().to_lowercase()).as_deref() {
        Some("pm") if h < 12 => h += 12,
        Some("am") if h == 12 => h = 0,
        _ => {}
    }
    let time = NaiveTime::from_hms_opt(h, m, 0)?;
    let mut day = now.date_naive();
    if day.and_time(time) <= now.naive_local() {
        day += Duration::days(1);
    }
    Local.from_local_datetime(&day.and_time(time)).earliest().map(|t| t.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.from_local_datetime(&chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap().and_hms_opt(h, m, 0).unwrap()).unwrap()
    }

    #[test]
    fn limit_replies_are_spotted_and_stories_are_not() {
        for s in [
            "You've hit your session limit · resets 8am (Asia/Kolkata)",
            "Claude AI usage limit reached|1793390400",
            "429 Too Many Requests",
            "Quota exceeded for quota metric 'Generate Content API requests per minute'",
            "RESOURCE_EXHAUSTED: Please retry in 34.5s.",
            "You exceeded your current quota, please check your plan and billing details.",
            "Rate limit reached for gpt-5 in organization org-x on tokens per min.",
            "Credit balance is too low",
            "API Error: 529 Overloaded",
        ] {
            assert!(provider_error_in(s).is_some(), "{s}");
        }
        assert_eq!(provider_error_in("Invalid API key · Please run /login").as_deref(), Some("Invalid API key · Please run /login"));
        for s in [
            "Arhan bites down on the blade, and the power floods through him.",
            "Mira: We used up our quota of luck today.\nKai: Then try again later!",
            "Error: the gate will not open.",
            r#"{"title":"Duel","segments":[{"page":1,"text":"He has no limit to his rage."}]}"#,
            "",
        ] {
            assert!(provider_error_in(s).is_none(), "{s}");
        }
        assert!(provider_error_in(&"A long story. ".repeat(60)).is_none());
    }

    #[test]
    fn reset_times_are_read() {
        let now = at(23, 10);
        assert_eq!(retry_at("You've hit your session limit · resets 8am (Asia/Kolkata)", now), Some(at(8, 0).timestamp() + 86_400));
        assert_eq!(retry_at("limit · resets at 11:45 pm", now), Some(at(23, 45).timestamp()));
        assert_eq!(retry_at("Please retry in 34.5s.", now), Some(now.timestamp() + 35));
        assert_eq!(retry_at("Rate limited. Try again in 2 minutes", now), Some(now.timestamp() + 120));
        assert_eq!(retry_at("usage limit reached|1793390400", now), Some(1_793_390_400));
        assert_eq!(retry_at("429 Too Many Requests", now), None);
    }

    #[test]
    fn waits_until_the_reset_or_backs_off() {
        let l = AiLimit { message: "x".into(), retry_at: Some(1_000) };
        assert_eq!(l.wait_secs(0, 400), 660);
        let none = AiLimit { message: "x".into(), retry_at: None };
        assert_eq!((none.wait_secs(0, 0), none.wait_secs(1, 0), none.wait_secs(9, 0)), (120, 300, 900));
        assert_eq!(AiLimit { message: "x".into(), retry_at: Some(10_000_000) }.wait_secs(0, 0), 6 * 3600);
    }
}
