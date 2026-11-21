//! The manga input (design 3.2, 3.3, 6.1): episode chapters as `mv_recap::ChapterRef`s, their
//! Studio Read (`read.json`), the text digest the story and bible prompts read, page and panel
//! files, and the series brain. Read-only: the engine never writes Studio's files, and reads
//! them only through `mv_recap`'s public functions.
//!
//! Page and panel numbers are 0-based everywhere (a page is its position in
//! `mv_recap::list_pages`, a panel its position in `PageRead::panels`), as in `SourceRef`.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use mv_recap::{Brain, ChapterRead, ChapterRef, PageRead};

use crate::model::ChapterPick;

/// Most characters of one digest sent in one LLM call.
pub const DIGEST_CAP: usize = 60_000;

/// A stored dir (root-relative or absolute) back to an absolute path. Empty stays empty.
pub fn resolve_dir(stored: &str) -> PathBuf {
    let stored = stored.trim();
    if stored.is_empty() {
        return PathBuf::new();
    }
    let p = Path::new(stored);
    if p.is_absolute() || p.has_root() {
        return p.to_path_buf();
    }
    let mut out = mv_core::paths::root();
    for part in stored.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".") {
        out.push(part);
    }
    out
}

/// An absolute dir to what is stored: relative to `mv_core::paths::root()` (forward slashes)
/// when inside it, else the absolute path as it is.
pub fn store_dir(abs: &Path) -> String {
    if abs.as_os_str().is_empty() {
        return String::new();
    }
    if let Ok(rel) = abs.strip_prefix(mv_core::paths::root()) {
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        // Only a plain path below the root is stored relative ("..", drive prefixes stay absolute).
        if !parts.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return parts.join("/");
        }
    }
    abs.to_string_lossy().into_owned()
}

/// The chapter as Studio knows it.
pub fn chapter_ref(pick: &ChapterPick, manga_title: &str) -> ChapterRef {
    ChapterRef {
        chapter_id: pick.chapter_id.clone(),
        manga_id: pick.manga_id.clone(),
        manga_title: manga_title.to_string(),
        label: pick.label.clone(),
        pages_dir: resolve_dir(&pick.pages_dir),
        output_dir: resolve_dir(&pick.output_dir),
    }
}

/// What an episode stores for a Studio chapter.
pub fn pick_from(chapter: &ChapterRef, number: Option<f64>) -> ChapterPick {
    ChapterPick {
        chapter_id: chapter.chapter_id.clone(),
        manga_id: chapter.manga_id.clone(),
        label: chapter.label.clone(),
        number: number.filter(|n| n.is_finite()),
        pages_dir: store_dir(&chapter.pages_dir),
        output_dir: store_dir(&chapter.output_dir),
    }
}

/// Studio's Read of the chapter, if any (`mv_recap::load_read`).
pub fn read(pick: &ChapterPick, manga_title: &str) -> Option<ChapterRead> {
    if pick.output_dir.trim().is_empty() {
        return None;
    }
    mv_recap::load_read(&chapter_ref(pick, manga_title))
}

/// (pages read, pages in the chapter); read < pages means partly read. A Read whose page
/// folder is gone counts its own pages as the chapter's.
pub fn read_progress(pick: &ChapterPick, manga_title: &str) -> (usize, usize) {
    let pages = pages(pick).len();
    let read = read(pick, manga_title).map(|r| {
        let mut idx: Vec<usize> = r.pages.iter().map(|p| p.index).filter(|i| pages == 0 || *i < pages).collect();
        idx.sort_unstable();
        idx.dedup();
        idx.len()
    });
    let read = read.unwrap_or(0);
    (read, pages.max(read))
}

/// One line of text: newlines and runs of spaces folded.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The digest block of one page (ends with a newline). Used by [`digest`]; the story step uses
/// it to cut a long chapter into chunks at page boundaries.
pub fn digest_page(page: &PageRead) -> String {
    let mut out = String::new();
    let s = &page.scene;
    let _ = writeln!(out, "[p{}] scene={}/{}/{} | {}", page.index, flat(&s.kind), flat(&s.mood), s.intensity, flat(&page.summary));
    for (n, panel) in page.panels.iter().enumerate() {
        let chars: Vec<String> = panel.characters.iter().map(|c| flat(&c.name)).filter(|n| !n.is_empty()).collect();
        let _ = write!(out, "  (k{n}) {}", flat(&panel.description));
        if !chars.is_empty() {
            let _ = write!(out, " | chars: {}", chars.join(", "));
        }
        out.push('\n');
        for line in &panel.lines {
            let text = flat(&line.text);
            if text.is_empty() {
                continue;
            }
            let speaker = line.speaker.as_deref().map(flat).filter(|s| !s.is_empty()).unwrap_or_else(|| "?".into());
            let kind = if line.kind.trim().is_empty() { "speech".to_string() } else { flat(&line.kind) };
            let _ = writeln!(out, "    {speaker} [{kind}]: {text}");
        }
    }
    out
}

/// The text digest of a Read: per page `[p{index}] scene={kind}/{mood}/{intensity} | {summary}`,
/// per panel `  (k{n}) {description} | chars: A, B`, per line `    {speaker or "?"} [{kind}]: {text}`,
/// cut at `max_chars` (characters) on a page boundary. A first page longer than the cap alone is
/// cut mid-page so the digest is never empty for a chapter that has text.
pub fn digest(read: &ChapterRead, max_chars: usize) -> String {
    let mut pages: Vec<&PageRead> = read.pages.iter().collect();
    pages.sort_by_key(|p| p.index);
    let (mut out, mut used) = (String::new(), 0usize);
    for page in pages {
        let block = digest_page(page);
        let n = block.chars().count();
        if used + n > max_chars {
            if out.is_empty() {
                out = block.chars().take(max_chars).collect();
            }
            break;
        }
        out.push_str(&block);
        used += n;
    }
    out
}

/// The chapter's page files in reading order (`mv_recap::list_pages`).
fn pages(pick: &ChapterPick) -> Vec<PathBuf> {
    let dir = resolve_dir(&pick.pages_dir);
    if dir.as_os_str().is_empty() {
        return Vec::new();
    }
    mv_recap::list_pages(&dir)
}

/// Full-resolution page file at index `page` (`mv_recap::list_pages`).
pub fn page_file(pick: &ChapterPick, page: usize) -> Option<PathBuf> {
    pages(pick).into_iter().nth(page)
}

/// Studio's panel crop (`mv_recap::panel_path`); the file may not exist (check before use).
pub fn panel_file(pick: &ChapterPick, manga_title: &str, page: usize, panel: usize) -> PathBuf {
    mv_recap::panel_path(&chapter_ref(pick, manga_title), page, panel)
}

/// The series brain (read only; Studio keeps the live copy).
pub fn brain(manga_id: &str, manga_title: &str) -> Brain {
    Brain::load(manga_id, manga_title)
}
