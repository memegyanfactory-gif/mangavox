//! Read access to the manga library in the existing `mangavox.db`.
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};

#[derive(Debug, Clone)]
pub struct MangaEntry {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub status: Option<String>,
    pub cover_url: Option<String>,
    pub is_favorite: bool,
    pub unread: i64,
}

/// Opens the shared database read-only so the Electron app can keep running alongside.
pub fn open_readonly() -> Result<Connection> {
    let conn = Connection::open_with_flags(
        crate::paths::database(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(conn)
}

pub fn list_manga(conn: &Connection) -> Result<Vec<MangaEntry>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.title, m.author, m.status, m.cover_path, COALESCE(m.is_favorite, 0),
                (SELECT COUNT(*) FROM chapter c WHERE c.manga_id = m.id AND COALESCE(c.is_read, 0) = 0)
         FROM manga m ORDER BY COALESCE(m.last_read_at, m.added_at) DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(MangaEntry {
            id: r.get(0)?,
            title: r.get(1)?,
            author: r.get(2)?,
            status: r.get(3)?,
            cover_url: r.get(4)?,
            is_favorite: r.get::<_, i64>(5)? != 0,
            unread: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
