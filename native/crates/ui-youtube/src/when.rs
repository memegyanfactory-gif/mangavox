//! Times as people read them: local time, 12-hour clock ("6:30 PM"), with the day or date only
//! when it is not obvious. Every time shown on the YouTube and Automation pages goes through here.
use chrono::{DateTime, Datelike, Local, TimeZone};

fn local(t: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&Local))
}

/// "6:30 PM" / "11:05 AM".
pub fn clock(t: i64) -> String {
    local(t).map(|d| clock_of(d.format("%H").to_string().parse().unwrap_or(0), d.format("%M").to_string().parse().unwrap_or(0))).unwrap_or_else(|| "—".into())
}

/// "6:30 PM" for an hour (0-23) and minute.
pub fn clock_of(hour: u32, minute: u32) -> String {
    let (h, ampm) = match hour % 24 {
        0 => (12, "AM"),
        h @ 1..=11 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!("{h}:{minute:02} {ampm}")
}

/// "6 PM" for an hour (0-23), for charts and heatmaps.
pub fn hour(h: usize) -> String {
    let h = (h % 24) as u32;
    clock_of(h, 0).replace(":00", "")
}

/// "6–8 PM", "11 PM–1 AM".
pub fn hour_span(start: usize, hours: usize) -> String {
    let (a, b) = (hour(start), hour(start + hours));
    let (a_num, a_ap) = a.split_once(' ').unwrap_or((&a, ""));
    let (_, b_ap) = b.split_once(' ').unwrap_or((&b, ""));
    if a_ap == b_ap {
        format!("{a_num}–{b}")
    } else {
        format!("{a}–{b}")
    }
}

/// "Today, 6:30 PM", "Tomorrow, 9:00 AM", "Yesterday, 11:41 PM", "Sat, 6:30 PM" within a week,
/// "Nov 12, 6:30 PM" this year, "Nov 12, 2027" beyond.
pub fn when(t: i64) -> String {
    when_from(t, chrono::Utc::now().timestamp())
}

pub fn when_from(t: i64, now: i64) -> String {
    let (Some(d), Some(n)) = (local(t), local(now)) else { return "—".into() };
    let days = d.date_naive().signed_duration_since(n.date_naive()).num_days();
    let time = clock(t);
    match days {
        0 => format!("Today, {time}"),
        1 => format!("Tomorrow, {time}"),
        -1 => format!("Yesterday, {time}"),
        2..=6 | -6..=-2 => format!("{}, {time}", d.format("%a")),
        _ if d.year() == n.year() => format!("{}, {time}", d.format("%b %-d")),
        _ => d.format("%b %-d, %Y").to_string(),
    }
}

/// "Nov 3, 2026" (YouTube Studio's date column).
pub fn date(t: i64) -> String {
    local(t).map(|d| d.format("%b %-d, %Y").to_string()).unwrap_or_else(|| "—".into())
}

/// The local zone ("IST", or "UTC+5:30" when the system gives no name).
pub fn zone() -> String {
    let off = Local::now().offset().local_minus_utc();
    let sign = if off < 0 { '-' } else { '+' };
    let m = off.abs() / 60;
    if m == 0 {
        "UTC".into()
    } else if m % 60 == 0 {
        format!("UTC{sign}{}", m / 60)
    } else {
        format!("UTC{sign}{}:{:02}", m / 60, m % 60)
    }
}

/// Parses what people type for a release time: "2026-11-03 18:30", "2026-11-03 6:30 PM",
/// "Nov 3 6:30 pm" (this year), local time.
pub fn parse(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    let up = s.to_uppercase();
    let year = Local::now().year();
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%d %I:%M %p", "%Y-%m-%d %I %p", "%b %d %Y %I:%M %p", "%b %d %Y %H:%M"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&up, f) {
            return Local.from_local_datetime(&n).single().map(|d| d.timestamp());
        }
    }
    // No year: this year.
    let parts: Vec<&str> = up.split(' ').collect();
    let with_year = if parts.len() >= 3 { format!("{} {} {year} {}", parts[0], parts[1], parts[2..].join(" ")) } else { String::new() };
    for f in ["%b %d %Y %I:%M %p", "%b %d %Y %H:%M"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&with_year, f) {
            return Local.from_local_datetime(&n).single().map(|d| d.timestamp());
        }
    }
    None
}

/// What the release-time field shows: "2026-11-03 6:30 PM" (what [`parse`] reads back).
pub fn input(t: i64) -> String {
    local(t).map(|d| format!("{} {}", d.format("%Y-%m-%d"), clock(t))).unwrap_or_default()
}

/// "in 5 h 12 min" / "in 3 days".
pub fn until(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("in {m} min"),
        (h, _) if h >= 48 => format!("in {} days", h / 24),
        (h, 0) => format!("in {h} h"),
        (h, m) => format!("in {h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::{clock_of, hour, hour_span, input, parse, until, when_from};

    #[test]
    fn twelve_hour_clock() {
        assert_eq!(clock_of(23, 41), "11:41 PM");
        assert_eq!(clock_of(0, 5), "12:05 AM");
        assert_eq!(clock_of(12, 0), "12:00 PM");
        assert_eq!(clock_of(9, 30), "9:30 AM");
        assert_eq!(hour(18), "6 PM");
        assert_eq!(hour(0), "12 AM");
        assert_eq!(hour_span(18, 2), "6–8 PM");
        assert_eq!(hour_span(23, 2), "11 PM–1 AM");
    }

    #[test]
    fn relative_days() {
        let t = parse("2026-11-03 18:30").unwrap();
        assert_eq!(when_from(t, t - 3600), "Today, 6:30 PM");
        assert_eq!(when_from(t, t - 86_400), "Tomorrow, 6:30 PM");
        assert!(when_from(t, t - 20 * 86_400).starts_with("Nov 3, "));
        assert_eq!(until(3600 * 5 + 12 * 60), "in 5 h 12 min");
    }

    #[test]
    fn typed_times_round_trip() {
        let t = parse("2026-11-03 6:30 PM").unwrap();
        assert_eq!(parse("2026-11-03 18:30"), Some(t));
        assert_eq!(parse(&input(t)), Some(t));
        assert!(parse("tomorrow").is_none());
    }
}
