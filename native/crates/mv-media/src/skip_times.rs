//! AniSkip skip-time lookup (intro, outro, recap). Mirrors `src/main/services/skip-times.ts`.
//!
//! Unlike the TypeScript version, failures are not hidden: the lookup returns `Result` so the caller can
//! tell a miss (`Ok` with `found == false`) from a failed request. `Err` never comes from a 404; a 404 is cached as a miss.
//! The TypeScript `getSkipTimes` never throws, so `unwrap_or_default()` gives its behaviour.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{MediaError, Result};
use crate::types::{SkipSegment, SkipTimesResult, SkipType};
use crate::util::value_to_f64;

pub const ANISKIP_BASE: &str = "https://api.aniskip.com/v2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const TTL_OK: Duration = Duration::from_secs(24 * 60 * 60);
const TTL_EMPTY: Duration = Duration::from_secs(10 * 60);

/// URL for one episode. `episode` is floored, and `episode_length` is in whole seconds.
pub fn skip_times_url(base: &str, mal_id: u64, episode: u64, episode_length: u64) -> String {
    let types: String = SkipType::ALL
        .iter()
        .map(|t| format!("types={}&", skip_type_str(*t)))
        .collect();
    format!("{base}/skip-times/{mal_id}/{episode}?{types}episodeLength={episode_length}")
}

fn skip_type_str(t: SkipType) -> &'static str {
    match t {
        SkipType::Op => "op",
        SkipType::Ed => "ed",
        SkipType::Recap => "recap",
        SkipType::MixedOp => "mixed-op",
        SkipType::MixedEd => "mixed-ed",
    }
}

fn skip_type_from_str(s: &str) -> Option<SkipType> {
    SkipType::ALL
        .iter()
        .copied()
        .find(|t| skip_type_str(*t) == s)
}

/// Parse an AniSkip v2 response body: keeps valid, non-empty intervals and sorts them by start time.
pub fn parse_skip_body(body: &Value) -> SkipTimesResult {
    let mut segments: Vec<SkipSegment> = body
        .get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|r| {
                    let kind = skip_type_from_str(r.get("skipType")?.as_str()?)?;
                    let interval = r.get("interval")?;
                    let start = value_to_f64(interval.get("startTime"))?;
                    let end = value_to_f64(interval.get("endTime"))?;
                    (end > start).then_some(SkipSegment { kind, start, end })
                })
                .collect()
        })
        .unwrap_or_default();
    segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    SkipTimesResult {
        found: !segments.is_empty(),
        segments,
    }
}

struct CacheEntry {
    at: Instant,
    ttl: Duration,
    value: SkipTimesResult,
}

pub struct SkipTimesClient {
    http: reqwest::Client,
    base: String,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl SkipTimesClient {
    pub fn new() -> Result<Self> {
        Self::with_base_url(ANISKIP_BASE)
    }

    pub fn with_base_url(base: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "MangaVox/",
                env!("CARGO_PKG_VERSION"),
                " (mv-media)"
            ))
            .build()?;
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Skip segments for one episode. Invalid ids or episode numbers return an empty result without a request.
    /// Hits are cached for 24 hours and misses for 10 minutes.
    pub async fn get(
        &self,
        mal_id: u64,
        episode: f64,
        episode_length: f64,
    ) -> Result<SkipTimesResult> {
        if mal_id == 0 || !episode.is_finite() || episode <= 0.0 {
            return Ok(SkipTimesResult::default());
        }
        let length = if episode_length.is_finite() {
            episode_length.round().max(0.0)
        } else {
            0.0
        };
        // Length is bucketed to 5 s so small differences between releases share one cache entry.
        let key = format!("{mal_id}:{episode}:{}", (length / 5.0).round());
        if let Some(hit) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            if hit.at.elapsed() < hit.ttl {
                return Ok(hit.value.clone());
            }
        }

        let url = skip_times_url(&self.base, mal_id, episode.floor() as u64, length as u64);
        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?;
        let status = resp.status();
        let value = if status == reqwest::StatusCode::NOT_FOUND {
            let empty = SkipTimesResult::default();
            self.store(key, TTL_EMPTY, empty.clone());
            return Ok(empty);
        } else if !status.is_success() {
            return Err(MediaError::Status {
                status: status.as_u16(),
                url,
            });
        } else {
            let text = resp.text().await?;
            let body: Value = serde_json::from_str(&text).map_err(|source| MediaError::Json {
                url: url.clone(),
                source,
            })?;
            parse_skip_body(&body)
        };
        let ttl = if value.found { TTL_OK } else { TTL_EMPTY };
        self.store(key, ttl, value.clone());
        Ok(value)
    }

    fn store(&self, key: String, ttl: Duration, value: SkipTimesResult) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(
            key,
            CacheEntry {
                at: Instant::now(),
                ttl,
                value,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_lists_all_types_and_length() {
        assert_eq!(
            skip_times_url(ANISKIP_BASE, 16498, 1, 1440),
            "https://api.aniskip.com/v2/skip-times/16498/1?types=op&types=ed&types=recap&types=mixed-op&types=mixed-ed&episodeLength=1440"
        );
    }

    #[test]
    fn parses_recorded_aniskip_response() {
        let body: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/aniskip_skip_times_16498_ep1.json"
        ))
        .unwrap();
        let result = parse_skip_body(&body);
        assert!(result.found);
        let kinds: Vec<SkipType> = result.segments.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SkipType::MixedOp,
                SkipType::Op,
                SkipType::Ed,
                SkipType::MixedEd
            ]
        );
        assert_eq!(result.segments[1].start, 128.406);
        assert_eq!(result.segments[1].end, 218.406);
    }

    #[test]
    fn drops_invalid_intervals_and_unknown_types() {
        let body = serde_json::json!({"results": [
            {"skipType": "op", "interval": {"startTime": 10, "endTime": 5}},
            {"skipType": "credits", "interval": {"startTime": 1, "endTime": 2}},
            {"skipType": "recap", "interval": {"startTime": "3", "endTime": "9"}}
        ]});
        let result = parse_skip_body(&body);
        assert_eq!(
            result.segments,
            vec![SkipSegment {
                kind: SkipType::Recap,
                start: 3.0,
                end: 9.0
            }]
        );
        assert!(!parse_skip_body(&serde_json::json!({})).found);
    }

    #[tokio::test]
    async fn rejects_invalid_ids_without_request() {
        let client = SkipTimesClient::with_base_url("http://127.0.0.1:1").unwrap();
        assert_eq!(
            client.get(0, 1.0, 1440.0).await.unwrap(),
            SkipTimesResult::default()
        );
        assert_eq!(
            client.get(5, 0.0, 1440.0).await.unwrap(),
            SkipTimesResult::default()
        );
        assert_eq!(
            client.get(5, f64::NAN, 1440.0).await.unwrap(),
            SkipTimesResult::default()
        );
    }
}
