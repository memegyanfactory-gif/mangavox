use super::now_ms;
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub const WATCH_STATUSES: [&str; 5] = ["watching", "planned", "completed", "on_hold", "dropped"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistEntry {
    pub media_id: String,
    pub kind: String,
    pub status: String,
    pub updated_at: i64,
}

pub fn list(conn: &Connection, kind: Option<&str>) -> Result<Vec<WatchlistEntry>> {
    let mut stmt = conn.prepare(
        "SELECT media_id, kind, status, updated_at FROM watchlist_status WHERE (?1 IS NULL OR kind = ?1)",
    )?;
    let rows = stmt
        .query_map([kind], |r| Ok(WatchlistEntry { media_id: r.get(0)?, kind: r.get(1)?, status: r.get(2)?, updated_at: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_status(conn: &Connection, media_id: &str, kind: &str, status: &str) -> Result<()> {
    if !WATCH_STATUSES.contains(&status) {
        bail!("Invalid watch status: {status}");
    }
    conn.execute(
        "INSERT INTO watchlist_status (media_id, kind, status, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(media_id) DO UPDATE SET status = excluded.status, kind = excluded.kind, updated_at = excluded.updated_at",
        params![media_id, kind, status, now_ms()],
    )?;
    Ok(())
}

pub fn remove_status(conn: &Connection, media_id: &str) -> Result<()> {
    conn.execute("DELETE FROM watchlist_status WHERE media_id = ?1", [media_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;

    #[test]
    fn crud() {
        let (_d, conn) = temp_db();
        set_status(&conn, "m1", "anime", "watching").unwrap();
        set_status(&conn, "m2", "movie", "planned").unwrap();
        set_status(&conn, "m1", "anime", "completed").unwrap();
        assert!(set_status(&conn, "m1", "anime", "bogus").is_err());
        assert_eq!(list(&conn, None).unwrap().len(), 2);
        let a = list(&conn, Some("anime")).unwrap();
        assert_eq!((a.len(), a[0].status.as_str()), (1, "completed"));
        remove_status(&conn, "m1").unwrap();
        assert_eq!(list(&conn, None).unwrap().len(), 1);
    }
}
