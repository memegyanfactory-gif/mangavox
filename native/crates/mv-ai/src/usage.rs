//! Token usage + subscription limits ("usage left") per provider.
//!
//! * Claude Code: `GET https://api.anthropic.com/api/oauth/usage` with the Claude CLI's OAuth token
//!   (`~/.claude/.credentials.json`) — the endpoint T3 Code also calls for Claude account limits.
//! * Codex: the newest `token_count` event with `rate_limits` in `~/.codex/sessions/**/rollout-*.jsonl`
//!   (T3 Code reads the same snapshot from the app-server `account/rateLimits` API).
//! * Session token totals are counted in-process from every finished step.
//!
//! Credentials are read only to authenticate one GET and are never logged or returned.
use crate::types::AiProviderId;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// "5h", "Week", "Month"...
    pub label: String,
    pub used_percent: f32,
    /// Unix seconds when the window resets.
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsage {
    pub provider: AiProviderId,
    pub windows: Vec<UsageWindow>,
    pub plan: Option<String>,
    pub note: Option<String>,
}

const TTL: Duration = Duration::from_secs(60);

type Cache = Mutex<HashMap<AiProviderId, (Instant, Option<ProviderUsage>)>>;
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}
type Totals = Mutex<HashMap<AiProviderId, (u64, u64, u32)>>;
fn totals() -> &'static Totals {
    static T: OnceLock<Totals> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// Counts one finished step and invalidates the cached limits so the next `fetch` is fresh.
pub fn record(provider: AiProviderId, _model: &str, input: u64, output: u64) {
    {
        let mut t = totals().lock().unwrap();
        let e = t.entry(provider).or_default();
        e.0 += input;
        e.1 += output;
        e.2 += 1;
    }
    cache().lock().unwrap().remove(&provider);
}

/// (input tokens, output tokens, calls) since the app started.
pub fn session_totals(provider: AiProviderId) -> (u64, u64, u32) {
    totals().lock().unwrap().get(&provider).copied().unwrap_or_default()
}

/// Current limits for `provider` (cached 60 s). `None` when the provider has no limit data.
pub async fn fetch(provider: AiProviderId) -> Option<ProviderUsage> {
    if !matches!(provider, AiProviderId::ClaudeCode | AiProviderId::Codex) {
        return None;
    }
    if let Some((at, v)) = cache().lock().unwrap().get(&provider) {
        if at.elapsed() < TTL {
            return v.clone();
        }
    }
    let v = match provider {
        AiProviderId::ClaudeCode => fetch_claude().await,
        _ => fetch_codex().await,
    };
    cache().lock().unwrap().insert(provider, (Instant::now(), v.clone()));
    v
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- Claude Code -------------------------------------------------------------------------------

fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
}

async fn fetch_claude() -> Option<ProviderUsage> {
    let raw = tokio::fs::read_to_string(claude_dir()?.join(".credentials.json")).await.ok()?;
    let creds: Value = serde_json::from_str(&raw).ok()?;
    let oauth = creds.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let plan = oauth.get("subscriptionType").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().ok()?;
    let res = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", "claude-cli/2.0 (external, cli)")
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body: Value = res.json().await.ok()?;
    let mut u = parse_claude_usage(&body);
    u.plan = plan;
    Some(u)
}

/// Parses the `/api/oauth/usage` body: `five_hour` / `seven_day` = `{utilization 0-100, resets_at ISO}`.
pub fn parse_claude_usage(body: &Value) -> ProviderUsage {
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "5h"), ("seven_day", "Week")] {
        let Some(w) = body.get(key).filter(|w| w.is_object()) else { continue };
        let Some(p) = w.get("utilization").and_then(Value::as_f64) else { continue };
        let resets_at = w.get("resets_at").and_then(Value::as_str).and_then(parse_iso);
        windows.push(window(label, p as f32, resets_at));
    }
    ProviderUsage { provider: AiProviderId::ClaudeCode, windows, plan: None, note: None }
}

fn window(label: &str, pct: f32, resets_at: Option<u64>) -> UsageWindow {
    // A window whose reset time already passed has started over.
    let expired = resets_at.is_some_and(|r| r <= now_secs());
    UsageWindow { label: label.into(), used_percent: if expired { 0. } else { pct.clamp(0., 100.) }, resets_at }
}

/// RFC 3339 -> unix seconds ("2025-11-04T04:59:59.943648+00:00", "...Z").
fn parse_iso(s: &str) -> Option<u64> {
    if s.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let off = match rest.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(&sign) if sign == b'+' || sign == b'-' => {
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
            let v = oh * 3600 + om * 60;
            if sign == b'+' {
                v
            } else {
                -v
            }
        }
        _ => return None,
    };
    // days from civil (Howard Hinnant)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + se - off).ok()
}

// ---- Codex -------------------------------------------------------------------------------------

async fn fetch_codex() -> Option<ProviderUsage> {
    let root = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?.join("sessions");
    tokio::task::spawn_blocking(move || latest_codex_usage(&root)).await.ok().flatten()
}

fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    v.sort();
    v.reverse();
    v
}

/// Scans the newest rollout files (newest first) for the last `token_count` event with rate limits.
fn latest_codex_usage(root: &Path) -> Option<ProviderUsage> {
    let mut files = Vec::new();
    'outer: for y in sorted_children(root) {
        for m in sorted_children(&y) {
            for d in sorted_children(&m) {
                for f in sorted_children(&d) {
                    if f.extension().is_some_and(|e| e == "jsonl") {
                        files.push(f);
                        if files.len() >= 6 {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let mtime = std::fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
        if let Some(u) = parse_codex_rollout(&text, mtime.unwrap_or_else(now_secs)) {
            return Some(u);
        }
    }
    None
}

/// Last `token_count` line with usable `rate_limits`. `base_secs` anchors `resets_in_seconds`.
pub fn parse_codex_rollout(text: &str, base_secs: u64) -> Option<ProviderUsage> {
    text.lines().rev().filter(|l| l.contains("token_count")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        let p = v.get("payload").unwrap_or(&v);
        if p.get("type").and_then(Value::as_str) != Some("token_count") {
            return None;
        }
        let u = parse_codex_rate_limits(p.get("rate_limits")?, base_secs);
        (!u.windows.is_empty()).then_some(u)
    })
}

/// Parses `{primary, secondary, plan_type}`; each window has `used_percent`, `window_minutes`,
/// and `resets_at` (unix secs) or `resets_in_seconds`.
pub fn parse_codex_rate_limits(rl: &Value, base_secs: u64) -> ProviderUsage {
    let mut windows = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let mins = w.get("window_minutes").and_then(Value::as_u64).unwrap_or(if key == "primary" { 300 } else { 10080 });
        let resets = w
            .get("resets_at")
            .and_then(Value::as_u64)
            .or_else(|| w.get("resets_in_seconds").and_then(Value::as_u64).map(|s| base_secs + s));
        windows.push(window(&window_label(mins), pct as f32, resets));
    }
    let plan = rl.get("plan_type").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    ProviderUsage { provider: AiProviderId::Codex, windows, plan, note: None }
}

fn window_label(mins: u64) -> String {
    match mins {
        m if m >= 30 * 24 * 60 => "Month".into(),
        m if m >= 7 * 24 * 60 => "Week".into(),
        m if m >= 24 * 60 => format!("{}d", m / (24 * 60)),
        m if m >= 60 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// Stashes limits seen in a live Codex event (so the next `fetch` need not read rollouts).
pub fn stash_codex_rate_limits(rl: &Value) {
    let u = parse_codex_rate_limits(rl, now_secs());
    if !u.windows.is_empty() {
        cache().lock().unwrap().insert(AiProviderId::Codex, (Instant::now(), Some(u)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_windows() {
        let body = json!({
            "five_hour": {"utilization": 34.0, "resets_at": "2099-11-04T04:59:59.943648+00:00"},
            "seven_day": {"utilization": 61.5, "resets_at": "2099-11-08T00:00:00Z"},
            "seven_day_opus": null
        });
        let u = parse_claude_usage(&body);
        assert_eq!(u.windows.len(), 2);
        assert_eq!(u.windows[0].label, "5h");
        assert_eq!(u.windows[0].used_percent, 34.0);
        assert_eq!(u.windows[1].label, "Week");
        assert_eq!(u.windows[1].resets_at, parse_iso("2099-11-08T00:00:00Z"));
    }

    #[test]
    fn iso_parsing() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_iso("2000-03-01T01:00:00+01:00"), Some(951_868_800));
        assert_eq!(parse_iso("garbage"), None);
    }

    #[test]
    fn codex_rollout_picks_last_token_count() {
        let text = concat!(
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":5.0,"window_minutes":300,"resets_at":4102444800}}}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#,
            "\n",
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"primary":{"used_percent":12.0,"window_minutes":300,"resets_in_seconds":600},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":4102444800},"plan_type":"plus"}}}"#,
            "\n",
        );
        let u = parse_codex_rollout(text, 4_102_000_000).unwrap();
        assert_eq!(u.plan.as_deref(), Some("plus"));
        assert_eq!(u.windows.len(), 2);
        assert_eq!(u.windows[0].label, "5h");
        assert_eq!(u.windows[0].used_percent, 12.0);
        assert_eq!(u.windows[0].resets_at, Some(4_102_000_600));
        assert_eq!(u.windows[1].label, "Week");
        assert_eq!(u.windows[1].used_percent, 40.0);
    }

    #[test]
    fn expired_window_reads_zero() {
        let u = parse_codex_rate_limits(&json!({"primary": {"used_percent": 90.0, "window_minutes": 300, "resets_at": 1}}), 0);
        assert_eq!(u.windows[0].used_percent, 0.0);
    }

    #[test]
    fn session_counter() {
        record(AiProviderId::Mistral, "m", 10, 5);
        record(AiProviderId::Mistral, "m", 1, 2);
        assert_eq!(session_totals(AiProviderId::Mistral), (11, 7, 2));
    }
}
