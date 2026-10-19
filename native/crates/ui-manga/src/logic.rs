//! Pure, UI-free logic (filtering, labels, cache names) so it can be unit-tested.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::Manga;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibraryFilter {
    All,
    Unread,
    Downloaded,
    Favorites,
    Ongoing,
    Completed,
}

impl LibraryFilter {
    pub const ALL: [LibraryFilter; 6] = [Self::All, Self::Unread, Self::Downloaded, Self::Favorites, Self::Ongoing, Self::Completed];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Downloaded => "Downloaded",
            Self::Favorites => "Favorites",
            Self::Ongoing => "Ongoing",
            Self::Completed => "Completed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    /// Last read, then date added (newest first).
    Recent,
    Title,
    /// Most unread chapters first.
    Unread,
    /// Newest additions first.
    Added,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "Last read",
            Self::Title => "Title",
            Self::Unread => "Unread count",
            Self::Added => "Date added",
        }
    }
    /// Next mode in the sort cycle.
    pub fn toggled(self) -> Self {
        match self {
            Self::Recent => Self::Title,
            Self::Title => Self::Unread,
            Self::Unread => Self::Added,
            Self::Added => Self::Recent,
        }
    }
}

/// Per-title chapter counts for the library grid (already narrowed to the preferred language).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LibStats {
    pub total: usize,
    pub unread: usize,
    pub downloaded: usize,
}

fn status_is(m: &Manga, word: &str) -> bool {
    m.status.as_deref().is_some_and(|s| s.to_lowercase().contains(word))
}

/// Indices into `items`, filtered by chip + category + query and sorted.
/// `only` restricts to a set of manga ids (a category tab); `None` = every title.
pub fn filter_library(
    items: &[Manga],
    stats: &HashMap<String, LibStats>,
    filter: LibraryFilter,
    only: Option<&HashSet<String>>,
    query: &str,
    sort: SortMode,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let st = |m: &Manga| stats.get(&m.id).copied().unwrap_or_default();
    let mut idx: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, m)| only.is_none_or(|ids| ids.contains(&m.id)))
        .filter(|(_, m)| match filter {
            LibraryFilter::All => true,
            LibraryFilter::Unread => st(m).unread > 0,
            LibraryFilter::Downloaded => st(m).downloaded > 0,
            LibraryFilter::Favorites => m.is_favorite,
            LibraryFilter::Ongoing => status_is(m, "ongoing"),
            LibraryFilter::Completed => status_is(m, "complete"),
        })
        .filter(|(_, m)| {
            q.is_empty()
                || m.title.to_lowercase().contains(&q)
                || m.author.as_deref().is_some_and(|a| a.to_lowercase().contains(&q))
        })
        .map(|(i, _)| i)
        .collect();
    match sort {
        SortMode::Title => idx.sort_by_key(|&i| items[i].title.to_lowercase()),
        SortMode::Recent => idx.sort_by(|&a, &b| {
            let ka = items[a].last_read_at.as_deref().unwrap_or(&items[a].added_at);
            let kb = items[b].last_read_at.as_deref().unwrap_or(&items[b].added_at);
            kb.cmp(ka)
        }),
        SortMode::Unread => idx.sort_by(|&a, &b| st(&items[b]).unread.cmp(&st(&items[a]).unread).then_with(|| items[a].title.cmp(&items[b].title))),
        SortMode::Added => idx.sort_by(|&a, &b| items[b].added_at.cmp(&items[a].added_at)),
    }
    idx
}

/// Does a chapter in `lang` match the preferred language `pref`? Case-insensitive and
/// region-tolerant ("en" matches "en-us"); an unknown (empty) language always matches.
pub fn lang_matches(lang: &str, pref: &str) -> bool {
    let (l, p) = (lang.trim().to_lowercase(), pref.trim().to_lowercase());
    if l.is_empty() || p.is_empty() || p == "all" {
        return true;
    }
    let base = |s: &str| s.split(['-', '_']).next().unwrap_or("").to_string();
    l == p || base(&l) == base(&p)
}

/// Keep only items in the preferred language; when none match, keep everything
/// (a title that is only available in other languages stays readable).
pub fn keep_language<T>(items: Vec<T>, pref: &str, lang: impl Fn(&T) -> &str) -> Vec<T> {
    if !items.iter().any(|it| lang_matches(lang(it), pref)) {
        return items;
    }
    items.into_iter().filter(|it| lang_matches(lang(it), pref)).collect()
}

/// Chapter list options on the manga page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChapterView {
    pub unread_only: bool,
    pub downloaded_only: bool,
    pub newest_first: bool,
}

impl Default for ChapterView {
    fn default() -> Self {
        Self { unread_only: false, downloaded_only: false, newest_first: false }
    }
}

/// Indices of `chapters` (ascending) to show, in display order.
pub fn visible_chapters(chapters: &[Chapter], v: ChapterView) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..chapters.len())
        .filter(|&i| !v.unread_only || !chapters[i].is_read)
        .filter(|&i| !v.downloaded_only || chapters[i].download_status == "complete")
        .collect();
    if v.newest_first {
        idx.reverse();
    }
    idx
}

/// The chapter to open after (`dir > 0`) or before (`dir < 0`) chapter `i`, skipping other
/// uploads of the same chapter number (several scanlation groups often post the same chapter).
pub fn step_chapter_index(chapters: &[Chapter], i: usize, dir: i32) -> Option<usize> {
    let cur = chapters.get(i)?.number;
    let mut j = i as i64;
    loop {
        j += dir.signum() as i64;
        if j < 0 || j as usize >= chapters.len() {
            return None;
        }
        let n = chapters[j as usize].number;
        match (cur, n) {
            (Some(a), Some(b)) if (a - b).abs() < 1e-9 => continue,
            _ => return Some(j as usize),
        }
    }
}

/// 12.0 -> "12", 12.5 -> "12.5".
pub fn fmt_number(n: f64) -> String {
    if (n - n.round()).abs() < 1e-9 {
        format!("{}", n.round() as i64)
    } else {
        format!("{n}")
    }
}

/// "Ch. 12", "Ch. 12 - Title" or just the title for unnumbered chapters.
pub fn chapter_label(c: &Chapter) -> String {
    let title = c.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    match (c.number, title) {
        (Some(n), Some(t)) => format!("Ch. {} - {t}", fmt_number(n)),
        (Some(n), None) => format!("Ch. {}", fmt_number(n)),
        (None, Some(t)) => t.to_string(),
        (None, None) => "Oneshot".to_string(),
    }
}

/// Which chapter the primary button opens, and whether it resumes (`true`) or starts fresh.
/// Chapters are in ascending order. Order of preference: started-but-unfinished (latest),
/// first unread, else the first chapter (re-read).
pub fn continue_target(chapters: &[Chapter]) -> Option<(usize, bool)> {
    if chapters.is_empty() {
        return None;
    }
    if let Some(i) = chapters.iter().rposition(|c| !c.is_read && c.read_progress > 0) {
        return Some((i, true));
    }
    if let Some(i) = chapters.iter().position(|c| !c.is_read) {
        let any_read = chapters.iter().any(|c| c.is_read);
        return Some((i, any_read));
    }
    Some((0, false))
}

/// Short label for the primary button.
pub fn continue_label(chapters: &[Chapter]) -> String {
    match continue_target(chapters) {
        None => "No chapters".into(),
        Some((i, true)) => format!("Resume {}", chapter_short(&chapters[i])),
        Some((i, false)) => format!("Start {}", chapter_short(&chapters[i])),
    }
}

pub fn chapter_short(c: &Chapter) -> String {
    c.number.map(|n| format!("Ch. {}", fmt_number(n))).unwrap_or_else(|| "Oneshot".into())
}

/// Stable file stem for a downloaded page (`md5(url + referer)`).
pub fn page_cache_name(clean_url: &str, referer: Option<&str>) -> String {
    let mut s = clean_url.to_string();
    if let Some(r) = referer {
        s.push('\n');
        s.push_str(r);
    }
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// Image extension from magic bytes; `None` when the body is not an image (HTML/JSON error page).
pub fn sniff_ext(b: &[u8]) -> Option<&'static str> {
    if b.len() < 12 {
        return None;
    }
    if b[0] == 0xff && b[1] == 0xd8 {
        Some("jpg")
    } else if b.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        Some("png")
    } else if b.starts_with(b"GIF") {
        Some("gif")
    } else if b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Existing cached file for `stem` in `dir` with any known extension.
pub fn find_cached(dir: &std::path::Path, stem: &str) -> Option<PathBuf> {
    ["jpg", "webp", "png", "gif", "jpeg"].iter().map(|e| dir.join(format!("{stem}.{e}"))).find(|p| p.exists())
}

/// "just now" / "5m ago" / "3h ago" / "2d ago" / date, from SQLite's `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn time_ago(read_at: &str, now: NaiveDateTime) -> String {
    let Ok(t) = NaiveDateTime::parse_from_str(read_at, "%Y-%m-%d %H:%M:%S") else { return read_at.to_string() };
    let m = (now - t).num_minutes();
    if m < 1 {
        "just now".into()
    } else if m < 60 {
        format!("{m}m ago")
    } else if m < 60 * 24 {
        format!("{}h ago", m / 60)
    } else if m < 60 * 24 * 7 {
        format!("{}d ago", m / (60 * 24))
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

/// Clamp a page index into `0..total`.
pub fn clamp_page(page: i64, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        page.clamp(0, total as i64 - 1) as usize
    }
}

/// Pages to keep downloaded around `current`, nearest first (`behind` pages back, `ahead` forward).
pub fn preload_window(current: usize, total: usize, behind: usize, ahead: usize) -> Vec<usize> {
    if total == 0 {
        return vec![];
    }
    let mut v = vec![current.min(total - 1)];
    for d in 1..=ahead.max(behind) {
        if d <= ahead && current + d < total {
            v.push(current + d);
        }
        if d <= behind && current >= d {
            v.push(current - d);
        }
    }
    v
}

/// Columns and card width for a cover grid that fills `width`: as many cards of at least
/// `min_card` px as fit, then stretched so the row ends flush with the edge.
pub fn grid_layout(width: f32, min_card: f32, gap: f32) -> (usize, f32) {
    let width = width.max(min_card);
    let cols = (((width + gap) / (min_card + gap)).floor() as usize).max(1);
    let card = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    (cols, card.floor())
}

/// SQLite `YYYY-MM-DD HH:MM:SS` (UTC) -> local time.
pub fn local_time(utc: &str) -> Option<NaiveDateTime> {
    let t = NaiveDateTime::parse_from_str(utc, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(chrono::TimeZone::from_utc_datetime(&chrono::Local, &t).naive_local())
}

/// History group heading: "Today", "Yesterday", a weekday within the last week, else a date.
pub fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    let d = (today - day).num_days();
    match d {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%B %-d").to_string(),
        _ => day.format("%B %-d, %Y").to_string(),
    }
}

/// Web page for a title, when the source has a predictable URL.
/// Source descriptions often carry markdown: `[text](url)` links become their text, and
/// separator lines (`---`, `___`, dashes) and trailing blank lines are dropped.
pub fn clean_description(desc: &str) -> String {
    let mut out = String::with_capacity(desc.len());
    let mut rest = desc;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let mut text = String::new();
    let mut blank = false;
    for l in out.lines().map(str::trim_end) {
        let t = l.trim();
        if !t.is_empty() && t.chars().all(|c| matches!(c, '-' | '_' | '*' | '=' | '—' | '–')) {
            continue;
        }
        // Keep at most one blank line between paragraphs.
        if t.is_empty() {
            blank = true;
            continue;
        }
        if !text.is_empty() {
            text.push_str(if blank { "\n\n" } else { "\n" });
        }
        blank = false;
        text.push_str(l);
    }
    text
}

pub fn web_url(source: &str, source_id: &str) -> Option<String> {
    if source_id.starts_with("http://") || source_id.starts_with("https://") {
        return Some(source_id.to_string());
    }
    match source {
        "mangadex" if !source_id.is_empty() => Some(format!("https://mangadex.org/title/{source_id}")),
        _ => None,
    }
}

/// Reader layout. Stored in the config as `readerMode` ("paged" / "rtl" / "vertical").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReaderMode {
    Ltr,
    Rtl,
    Strip,
}

impl ReaderMode {
    pub const ALL: [ReaderMode; 3] = [Self::Ltr, Self::Rtl, Self::Strip];
    pub fn from_config(s: &str) -> Self {
        match s {
            "rtl" => Self::Rtl,
            "vertical" | "webtoon" | "strip" => Self::Strip,
            _ => Self::Ltr,
        }
    }
    pub fn config(self) -> &'static str {
        match self {
            Self::Ltr => "paged",
            Self::Rtl => "rtl",
            Self::Strip => "vertical",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ltr => "Left to right",
            Self::Rtl => "Right to left",
            Self::Strip => "Long strip",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Self::Ltr => "LTR",
            Self::Rtl => "RTL",
            Self::Strip => "Strip",
        }
    }
}

/// How a page is scaled in paged mode. Stored as `readerFit`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Whole page visible (fit to screen height and width).
    Screen,
    /// Fill the width, scroll tall pages.
    Width,
    /// Natural size.
    Original,
}

impl Fit {
    pub const ALL: [Fit; 3] = [Self::Screen, Self::Width, Self::Original];
    pub fn from_config(s: &str) -> Self {
        match s {
            "width" => Self::Width,
            "original" => Self::Original,
            _ => Self::Screen,
        }
    }
    pub fn config(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Width => "width",
            Self::Original => "original",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Screen => "Fit screen",
            Self::Width => "Fit width",
            Self::Original => "Original",
        }
    }
}

/// Which third of the page a click landed in, in reading order: -1 previous, 0 menu, 1 next.
pub fn tap_zone(x: f32, width: f32, mode: ReaderMode) -> i32 {
    if width <= 0.0 {
        return 0;
    }
    let f = x / width;
    let z = if f < 0.3 {
        -1
    } else if f > 0.7 {
        1
    } else {
        0
    };
    if mode == ReaderMode::Rtl {
        -z
    } else {
        z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manga(title: &str, status: Option<&str>, fav: bool, added: &str, read: Option<&str>) -> Manga {
        Manga {
            id: title.into(),
            source_plugin: "s".into(),
            source_id: title.into(),
            title: title.into(),
            author: Some("Oda".into()),
            artist: None,
            description: None,
            cover_path: None,
            status: status.map(Into::into),
            genres: vec![],
            is_favorite: fav,
            notes: None,
            added_at: added.into(),
            last_read_at: read.map(Into::into),
            chapter_count: None,
            unread_count: None,
        }
    }

    fn chapter(n: Option<f64>, read: bool, progress: i64) -> Chapter {
        Chapter {
            id: format!("{n:?}"),
            manga_id: "m".into(),
            source_plugin: "s".into(),
            source_id: format!("{n:?}"),
            number: n,
            title: None,
            language: "en".into(),
            page_count: None,
            download_status: "none".into(),
            ocr_status: "none".into(),
            tts_status: "none".into(),
            video_status: "none".into(),
            read_progress: progress,
            is_read: read,
            pages_dir: None,
            added_at: String::new(),
        }
    }

    #[test]
    fn filters_and_sorts() {
        let items = vec![
            manga("Bleach", Some("Completed"), false, "2024-01-01", None),
            manga("Akira", Some("ongoing"), true, "2024-02-01", Some("2024-03-01")),
            manga("Naruto", Some("Ongoing"), false, "2024-01-15", None),
        ];
        let mut stats = HashMap::new();
        stats.insert("Naruto".to_string(), LibStats { total: 10, unread: 4, downloaded: 0 });
        stats.insert("Bleach".to_string(), LibStats { total: 5, unread: 1, downloaded: 2 });
        let f = |filter, only, q, sort| filter_library(&items, &stats, filter, only, q, sort);
        assert_eq!(f(LibraryFilter::All, None, "", SortMode::Title), vec![1, 0, 2]);
        assert_eq!(f(LibraryFilter::All, None, "", SortMode::Recent), vec![1, 2, 0]);
        assert_eq!(f(LibraryFilter::All, None, "", SortMode::Added), vec![1, 2, 0]);
        assert_eq!(f(LibraryFilter::All, None, "", SortMode::Unread), vec![2, 0, 1]);
        assert_eq!(f(LibraryFilter::Favorites, None, "", SortMode::Recent), vec![1]);
        assert_eq!(f(LibraryFilter::Ongoing, None, "", SortMode::Title), vec![1, 2]);
        assert_eq!(f(LibraryFilter::Completed, None, "", SortMode::Title), vec![0]);
        assert_eq!(f(LibraryFilter::Unread, None, "", SortMode::Title), vec![0, 2]);
        assert_eq!(f(LibraryFilter::Downloaded, None, "", SortMode::Title), vec![0]);
        assert_eq!(f(LibraryFilter::All, None, " nar", SortMode::Title), vec![2]);
        assert_eq!(f(LibraryFilter::All, None, "oda", SortMode::Title).len(), 3);
        let cat: HashSet<String> = ["Akira".to_string()].into();
        assert_eq!(f(LibraryFilter::All, Some(&cat), "", SortMode::Title), vec![1]);
    }

    #[test]
    fn language_filter() {
        assert!(lang_matches("en", "en"));
        assert!(lang_matches("EN-us", "en"));
        assert!(lang_matches("", "en"));
        assert!(!lang_matches("es-la", "en"));
        assert!(lang_matches("es-la", "all"));
        let mixed = vec!["en", "es", "en-us", "fr"];
        assert_eq!(keep_language(mixed, "en", |s| s), vec!["en", "en-us"]);
        // nothing in English: keep everything rather than an empty list
        assert_eq!(keep_language(vec!["es", "fr"], "en", |s| s), vec!["es", "fr"]);
    }

    #[test]
    fn chapter_views_and_steps() {
        let mut cs = vec![chapter(Some(1.), true, 0), chapter(Some(2.), false, 3), chapter(Some(2.), false, 0), chapter(Some(3.), false, 0)];
        cs[3].download_status = "complete".into();
        assert_eq!(visible_chapters(&cs, ChapterView::default()), vec![0, 1, 2, 3]);
        let v = ChapterView { newest_first: true, ..Default::default() };
        assert_eq!(visible_chapters(&cs, v), vec![3, 2, 1, 0]);
        let v = ChapterView { unread_only: true, downloaded_only: false, newest_first: false };
        assert_eq!(visible_chapters(&cs, v), vec![1, 2, 3]);
        let v = ChapterView { unread_only: false, downloaded_only: true, newest_first: false };
        assert_eq!(visible_chapters(&cs, v), vec![3]);
        // duplicate uploads of chapter 2 are skipped
        assert_eq!(step_chapter_index(&cs, 1, 1), Some(3));
        assert_eq!(step_chapter_index(&cs, 2, -1), Some(0));
        assert_eq!(step_chapter_index(&cs, 0, -1), None);
        assert_eq!(step_chapter_index(&cs, 3, 1), None);
        let unnumbered = vec![chapter(None, false, 0), chapter(None, false, 0)];
        assert_eq!(step_chapter_index(&unnumbered, 0, 1), Some(1));
    }

    #[test]
    fn layout_days_and_reader() {
        assert_eq!(grid_layout(100.0, 140.0, 16.0).0, 1);
        let (cols, w) = grid_layout(1000.0, 140.0, 16.0);
        assert_eq!(cols, 6);
        assert!(w >= 140.0 && w * 6.0 + 16.0 * 5.0 <= 1000.0);
        let today = NaiveDate::from_ymd_opt(2024, 5, 10).unwrap();
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(today.pred_opt().unwrap(), today), "Yesterday");
        assert_eq!(day_label(NaiveDate::from_ymd_opt(2024, 5, 7).unwrap(), today), "Tuesday");
        assert_eq!(day_label(NaiveDate::from_ymd_opt(2024, 3, 2).unwrap(), today), "March 2");
        assert_eq!(day_label(NaiveDate::from_ymd_opt(2023, 3, 2).unwrap(), today), "March 2, 2023");
        assert_eq!(clean_description("Intro\n\n---\n- [Official LN](https://x.y/z) here\n\n\n"), "Intro\n\n- Official LN here");
        assert_eq!(clean_description("no [link"), "no [link");
        assert_eq!(web_url("mangadex", "abc").as_deref(), Some("https://mangadex.org/title/abc"));
        assert_eq!(web_url("x", "https://a/b").as_deref(), Some("https://a/b"));
        assert_eq!(web_url("x", "id"), None);
        for m in ReaderMode::ALL {
            assert_eq!(ReaderMode::from_config(m.config()), m);
        }
        for f in Fit::ALL {
            assert_eq!(Fit::from_config(f.config()), f);
        }
        assert_eq!(tap_zone(10.0, 100.0, ReaderMode::Ltr), -1);
        assert_eq!(tap_zone(10.0, 100.0, ReaderMode::Rtl), 1);
        assert_eq!(tap_zone(50.0, 100.0, ReaderMode::Rtl), 0);
        assert_eq!(tap_zone(90.0, 100.0, ReaderMode::Ltr), 1);
    }

    #[test]
    fn labels() {
        assert_eq!(fmt_number(12.0), "12");
        assert_eq!(fmt_number(12.5), "12.5");
        let mut c = chapter(Some(3.0), false, 0);
        assert_eq!(chapter_label(&c), "Ch. 3");
        c.title = Some(" Hello ".into());
        assert_eq!(chapter_label(&c), "Ch. 3 - Hello");
        assert_eq!(chapter_label(&chapter(None, false, 0)), "Oneshot");
    }

    #[test]
    fn continue_picks() {
        assert_eq!(continue_target(&[]), None);
        let fresh = vec![chapter(Some(1.), false, 0), chapter(Some(2.), false, 0)];
        assert_eq!(continue_target(&fresh), Some((0, false)));
        assert_eq!(continue_label(&fresh), "Start Ch. 1");
        let mid = vec![chapter(Some(1.), true, 9), chapter(Some(2.), false, 4), chapter(Some(3.), false, 0)];
        assert_eq!(continue_target(&mid), Some((1, true)));
        let between = vec![chapter(Some(1.), true, 9), chapter(Some(2.), false, 0)];
        assert_eq!(continue_target(&between), Some((1, true)));
        let done = vec![chapter(Some(1.), true, 9)];
        assert_eq!(continue_target(&done), Some((0, false)));
    }

    #[test]
    fn cache_and_sniff() {
        assert_eq!(page_cache_name("u", None), page_cache_name("u", None));
        assert_ne!(page_cache_name("u", None), page_cache_name("u", Some("r")));
        let jpg = [0xff, 0xd8, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(sniff_ext(&jpg), Some("jpg"));
        assert_eq!(sniff_ext(b"<html><body>blocked</body>"), None);
        assert_eq!(sniff_ext(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
    }

    #[test]
    fn ago_and_paging() {
        let now = NaiveDateTime::parse_from_str("2024-05-10 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(time_ago("2024-05-10 11:59:30", now), "just now");
        assert_eq!(time_ago("2024-05-10 11:30:00", now), "30m ago");
        assert_eq!(time_ago("2024-05-10 07:00:00", now), "5h ago");
        assert_eq!(time_ago("2024-05-08 12:00:00", now), "2d ago");
        assert_eq!(time_ago("2024-04-01 12:00:00", now), "2024-04-01");
        assert_eq!(clamp_page(99, 10), 9);
        assert_eq!(clamp_page(-1, 10), 0);
        assert_eq!(clamp_page(3, 0), 0);
        assert_eq!(preload_window(0, 3, 1, 2), vec![0, 1, 2]);
        assert_eq!(preload_window(5, 10, 1, 2), vec![5, 6, 4, 7]);
    }
}
