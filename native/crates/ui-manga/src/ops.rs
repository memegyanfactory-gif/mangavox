//! Multi-step async operations that combine a source with the database.
use anyhow::{anyhow, Result};
use mv_core::repo::chapter::Chapter;
use mv_core::repo::manga::{self, Manga};
use mv_sources::MangaSearchResult;
use mv_ui::bg;

use crate::{backend, store};

/// Open a source result: details + chapter sync into the database, and return the entry.
/// Titles seen for the first time are marked "browse only" (not shown in the Library until the
/// user adds them). Nothing is written when the chapter list cannot be fetched.
pub async fn prepare_from_source(source_id: String, r: MangaSearchResult) -> Result<Manga> {
    let src = backend::source(&source_id).await?;
    let details = src.get_manga_details(&r.source_id).await.ok();
    let chapters = src.get_chapter_list(&r.source_id).await.map_err(|e| anyhow!("{e}"))?;
    let lang = backend::preferred_language();
    let m = bg::blocking(move || {
        backend::with_db(|c| {
            let existed = manga::find_by_source(c, &source_id, &r.source_id)?.is_some();
            let m = store::ensure_manga(c, &source_id, &r, details.as_ref())?;
            store::apply_chapters(c, &m.id, &source_id, &chapters, &lang)?;
            if let Some(d) = &details {
                store::apply_details(c, &m.id, d)?;
            }
            backend::store_extras(&m.id, &chapters);
            if !existed {
                backend::set_in_library(&m.id, false);
            }
            Ok(manga::get(c, &m.id)?.unwrap_or(m))
        })
    })
    .await??;
    Ok(m)
}

/// Re-read details and chapters from the manga's source.
pub async fn refresh_manga(manga_id: String) -> Result<(Manga, Vec<Chapter>)> {
    let id = manga_id.clone();
    let m = bg::blocking(move || backend::with_db(|c| manga::get(c, &id)?.ok_or_else(|| anyhow!("Manga not found")))).await??;
    let src = backend::source(&m.source_plugin).await?;
    let details = src.get_manga_details(&m.source_id).await.ok();
    let chapters = src.get_chapter_list(&m.source_id).await.map_err(|e| anyhow!("{e}"))?;
    let lang = backend::preferred_language();
    let res = bg::blocking(move || {
        backend::with_db(|c| {
            let list = store::apply_chapters(c, &m.id, &m.source_plugin, &chapters, &lang)?;
            if let Some(d) = &details {
                store::apply_details(c, &m.id, d)?;
            }
            backend::store_extras(&m.id, &chapters);
            Ok((manga::get(c, &m.id)?.unwrap_or(m), list))
        })
    })
    .await?;
    backend::bump_data();
    res
}
