//! Repositories mirroring `src/main/db/*.repo.ts`. All functions take a `&Connection`.
pub mod category;
pub mod chapter;
pub mod history;
pub mod manga;
pub mod media;
pub mod media_chat;
pub mod watchlist;

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
pub(crate) fn seed_manga(conn: &rusqlite::Connection, src: &str) -> manga::Manga {
    manga::add(
        conn,
        &manga::AddMangaInput {
            source_plugin: "mangadex".into(),
            source_id: src.into(),
            title: format!("Title {src}"),
            ..Default::default()
        },
    )
    .unwrap()
}
