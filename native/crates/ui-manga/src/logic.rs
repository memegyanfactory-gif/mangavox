//! Pure, UI-free logic (filtering, labels, cache names) so it can be unit-tested.
use std::path::PathBuf;

use chrono::NaiveDateTime;
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::Manga;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibraryFilter {
    All,
    Favorites,
    Ongoing,
    Completed,
}

impl LibraryFilter {
    pub const ALL: [LibraryFilter; 4] = [Self::All, Self::Favorites, Self::Ongoing, Self::Completed];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Favorites => "Favorites",
            Self::Ongoing => "Ongoing",
            Self::Completed => "Completed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    Recent,
    Title,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Title => "Title",
        }
    }
    pub fn toggled(self) -> Self {
        match self {
            Self::Recent => Self::Title,
            Self::Title => Self::Recent,
        }
    }
}

fn status_is(m: &Manga, word: &str) -> bool {
    m.status.as_deref().is_some_and(|s| s.to_lowercase().contains(word))
}

/// Indices into `items`, filtered by chip + query and sorted. `Recent` = last read, then added.
pub fn filter_library(items: &[Manga], filter: LibraryFilter, query: &str, sort: SortMode) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut idx: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, m)| match filter {
            LibraryFilter::All => true,
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
    }
    idx
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
        Some((i, true)) => format!("Continue - {}", chapter_short(&chapters[i])),
        Some((i, false)) => format!("Start reading - {}", chapter_short(&chapters[i])),
    }
}

fn chapter_short(c: &Chapter) -> String {
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

/// Number of grid columns that fit `width` px (cards are 150 wide with a 20 px gap).
pub fn grid_columns(width: f32) -> usize {
    (((width + 20.0) / 170.0).floor() as usize).max(1)
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
        assert_eq!(filter_library(&items, LibraryFilter::All, "", SortMode::Title), vec![1, 0, 2]);
        assert_eq!(filter_library(&items, LibraryFilter::All, "", SortMode::Recent), vec![1, 2, 0]);
        assert_eq!(filter_library(&items, LibraryFilter::Favorites, "", SortMode::Recent), vec![1]);
        assert_eq!(filter_library(&items, LibraryFilter::Ongoing, "", SortMode::Title), vec![1, 2]);
        assert_eq!(filter_library(&items, LibraryFilter::Completed, "", SortMode::Title), vec![0]);
        assert_eq!(filter_library(&items, LibraryFilter::All, " nar", SortMode::Title), vec![2]);
        assert_eq!(filter_library(&items, LibraryFilter::All, "oda", SortMode::Title).len(), 3);
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
        assert_eq!(continue_label(&fresh), "Start reading - Ch. 1");
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
        assert_eq!(grid_columns(100.0), 1);
        assert_eq!(grid_columns(850.0), 5);
        assert_eq!(clamp_page(99, 10), 9);
        assert_eq!(clamp_page(-1, 10), 0);
        assert_eq!(clamp_page(3, 0), 0);
        assert_eq!(preload_window(0, 3, 1, 2), vec![0, 1, 2]);
        assert_eq!(preload_window(5, 10, 1, 2), vec![5, 6, 4, 7]);
    }
}
