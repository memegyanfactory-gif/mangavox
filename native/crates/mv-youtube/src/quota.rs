//! The YouTube Data API's daily quota (10,000 units per Google Cloud project by default), counted
//! here so MangaVox never spends it on things that can wait. Every Data API call is charged its
//! documented cost as it is sent (`YouTube::request`); the count starts again at midnight Pacific
//! time, when Google resets it. Background reads stop while what is left would not cover an
//! upload; uploads and replies the user asks for are never held back by the estimate (only by
//! YouTube's own "quota exceeded").
//! Costs: <https://developers.google.com/youtube/v3/determine_quota_cost>
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// The default daily allowance of a Google Cloud project.
pub const DAILY: u32 = 10_000;
/// `videos.insert`. Google's table says 1,600; newer notes say less. The larger figure is kept so
/// the estimate never runs over.
pub const UPLOAD: u32 = 1_600;
/// `comments.insert`, `commentThreads.insert`, `videos.update`, `playlistItems.insert`...
pub const WRITE: u32 = 50;
/// `search.list`.
pub const SEARCH: u32 = 100;

/// What one request costs, from its method and URL. Analytics has a quota of its own (0 here);
/// the resumable upload's data chunks go to the session URL and cost nothing more.
pub fn cost(method: &str, url: &str) -> u32 {
    if url.contains("youtubeanalytics.googleapis.com") || url.contains("upload_id=") {
        return 0;
    }
    let get = method.eq_ignore_ascii_case("GET");
    if url.contains("/upload/youtube/v3/videos") {
        UPLOAD
    } else if url.contains("/thumbnails/set") {
        WRITE
    } else if url.contains("/youtube/v3/search") {
        SEARCH
    } else if get {
        1
    } else {
        WRITE
    }
}

/// Seconds east of UTC of US Pacific time at `ts` (PDT from the second Sunday of March 2:00 to
/// the first Sunday of November 2:00, PST otherwise).
pub fn pacific_offset(ts: i64) -> i32 {
    let Some(utc) = Utc.timestamp_opt(ts, 0).single() else { return -8 * 3600 };
    let year = utc.year();
    let nth_sunday = |month: u32, n: u32| -> NaiveDate {
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
        let to_sun = (7 - first.weekday().num_days_from_sunday()) % 7;
        first + chrono::Duration::days((to_sun + 7 * (n - 1)) as i64)
    };
    // 2:00 local = 10:00 UTC in March (PST), 09:00 UTC in November (PDT).
    let start = nth_sunday(3, 2).and_hms_opt(10, 0, 0).map(|d| d.and_utc().timestamp()).unwrap_or(0);
    let end = nth_sunday(11, 1).and_hms_opt(9, 0, 0).map(|d| d.and_utc().timestamp()).unwrap_or(0);
    if (start..end).contains(&ts) {
        -7 * 3600
    } else {
        -8 * 3600
    }
}

/// The quota day of `ts`: its date in Pacific time ("2026-11-02").
pub fn quota_day(ts: i64) -> String {
    let local = ts + pacific_offset(ts) as i64;
    Utc.timestamp_opt(local, 0).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// Unix seconds of the next quota reset (midnight Pacific) after `ts`.
pub fn next_reset(ts: i64) -> i64 {
    let off = pacific_offset(ts) as i64;
    let local = ts + off;
    let midnight = (local.div_euclid(86_400) + 1) * 86_400;
    midnight - off
}

/// `youtube/quota.json`: what today's calls have cost.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Usage {
    /// The Pacific date the count belongs to.
    pub day: String,
    pub used: u32,
    pub calls: u32,
    pub uploads: u32,
}

impl Usage {
    /// Starts a new count when the quota day has turned over.
    pub fn roll(&mut self, now: i64) {
        let day = quota_day(now);
        if self.day != day {
            *self = Usage { day, ..Default::default() };
        }
    }

    pub fn spend(&mut self, units: u32, now: i64) {
        self.roll(now);
        self.used = self.used.saturating_add(units);
        self.calls += 1;
        if units >= UPLOAD {
            self.uploads += 1;
        }
    }

    /// Units left today (by this count).
    pub fn left(&self, now: i64) -> u32 {
        if self.day != quota_day(now) {
            return DAILY;
        }
        DAILY.saturating_sub(self.used)
    }

    /// Background reads may run: one more upload would still fit afterwards.
    pub fn can_spare(&self, units: u32, now: i64) -> bool {
        self.left(now) >= units + UPLOAD
    }

    /// Uploads that still fit today.
    pub fn uploads_left(&self, now: i64) -> u32 {
        self.left(now) / UPLOAD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs() {
        assert_eq!(cost("GET", "https://www.googleapis.com/youtube/v3/videos?id=x"), 1);
        assert_eq!(cost("PUT", "https://www.googleapis.com/youtube/v3/videos?part=snippet"), WRITE);
        assert_eq!(cost("POST", "https://www.googleapis.com/youtube/v3/comments?part=snippet"), WRITE);
        assert_eq!(cost("POST", "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable"), UPLOAD);
        assert_eq!(cost("PUT", "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&upload_id=abc"), 0);
        assert_eq!(cost("POST", "https://www.googleapis.com/upload/youtube/v3/thumbnails/set?videoId=x"), WRITE);
        assert_eq!(cost("GET", "https://www.googleapis.com/youtube/v3/search?q=x"), SEARCH);
        assert_eq!(cost("GET", "https://youtubeanalytics.googleapis.com/v2/reports?ids=channel==MINE"), 0);
    }

    #[test]
    fn pacific_days_and_dst() {
        // 2026-07-01 06:00 UTC = June 30 23:00 PDT.
        let summer = Utc.with_ymd_and_hms(2026, 7, 1, 6, 0, 0).unwrap().timestamp();
        assert_eq!(pacific_offset(summer), -7 * 3600);
        assert_eq!(quota_day(summer), "2026-06-30");
        // 2026-12-01 07:30 UTC = Nov 30 23:30 PST.
        let winter = Utc.with_ymd_and_hms(2026, 12, 1, 7, 30, 0).unwrap().timestamp();
        assert_eq!(pacific_offset(winter), -8 * 3600);
        assert_eq!(quota_day(winter), "2026-11-30");
        // The reset is the next Pacific midnight: 08:00 UTC in winter.
        assert_eq!(next_reset(winter), Utc.with_ymd_and_hms(2026, 12, 1, 8, 0, 0).unwrap().timestamp());
    }

    #[test]
    fn usage_rolls_over() {
        let t = Utc.with_ymd_and_hms(2026, 11, 2, 18, 0, 0).unwrap().timestamp();
        let mut u = Usage::default();
        u.spend(UPLOAD, t);
        u.spend(1, t);
        assert_eq!((u.used, u.calls, u.uploads), (UPLOAD + 1, 2, 1));
        assert_eq!(u.left(t), DAILY - UPLOAD - 1);
        assert!(u.can_spare(1, t));
        assert_eq!(u.uploads_left(t), 5);
        let tomorrow = t + 86_400;
        assert_eq!(u.left(tomorrow), DAILY);
        u.spend(1, tomorrow);
        assert_eq!(u.used, 1);
    }
}
