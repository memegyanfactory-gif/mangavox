use super::new_id;
use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Mirrors `LibraryCategory` in `src/shared/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryCategory {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

pub fn list(conn: &Connection) -> Result<Vec<LibraryCategory>> {
    let mut stmt =
        conn.prepare("SELECT id, name, sort_order FROM library_category ORDER BY sort_order ASC, created_at ASC")?;
    let rows = stmt
        .query_map([], |r| Ok(LibraryCategory { id: r.get(0)?, name: r.get(1)?, sort_order: r.get(2)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn create(conn: &Connection, name: &str) -> Result<LibraryCategory> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Category name required");
    }
    let max: i64 = conn.query_row("SELECT COALESCE(MAX(sort_order), -1) FROM library_category", [], |r| r.get(0))?;
    let id = new_id();
    conn.execute("INSERT INTO library_category (id, name, sort_order) VALUES (?1, ?2, ?3)", params![id, name, max + 1])?;
    Ok(LibraryCategory { id, name: name.to_string(), sort_order: max + 1 })
}

pub fn rename(conn: &Connection, id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Category name required");
    }
    conn.execute("UPDATE library_category SET name = ?1 WHERE id = ?2", [name, id])?;
    Ok(())
}

pub fn delete(conn: &mut Connection, id: &str) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM manga_category WHERE category_id = ?1", [id])?;
    tx.execute("DELETE FROM library_category WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(())
}

pub fn set_manga_categories(conn: &mut Connection, manga_id: &str, category_ids: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM manga_category WHERE manga_id = ?1", [manga_id])?;
    let mut seen = HashSet::new();
    for cid in category_ids.iter().filter(|c| seen.insert(c.as_str())) {
        tx.execute("INSERT OR IGNORE INTO manga_category (manga_id, category_id) VALUES (?1, ?2)", [manga_id, cid])?;
    }
    tx.commit()?;
    Ok(())
}

/// mangaId -> categoryIds for the whole library.
pub fn manga_category_map(conn: &Connection) -> Result<BTreeMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT mc.manga_id, mc.category_id FROM manga_category mc JOIN manga m ON m.id = mc.manga_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let (m, c) = row?;
        map.entry(m).or_default().push(c);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;
    use crate::repo::seed_manga;

    #[test]
    fn crud_and_assignment() {
        let (_d, mut conn) = temp_db();
        assert!(create(&conn, "  ").is_err());
        let a = create(&conn, " Reading ").unwrap();
        let b = create(&conn, "Done").unwrap();
        assert_eq!((a.name.as_str(), a.sort_order, b.sort_order), ("Reading", 0, 1));
        rename(&conn, &b.id, "Finished").unwrap();
        assert_eq!(list(&conn).unwrap()[1].name, "Finished");
        let m = seed_manga(&conn, "x");
        set_manga_categories(&mut conn, &m.id, &[a.id.clone(), a.id.clone(), b.id.clone()]).unwrap();
        assert_eq!(manga_category_map(&conn).unwrap()[&m.id].len(), 2);
        delete(&mut conn, &a.id).unwrap();
        assert_eq!(manga_category_map(&conn).unwrap()[&m.id], vec![b.id.clone()]);
        set_manga_categories(&mut conn, &m.id, &[]).unwrap();
        assert!(manga_category_map(&conn).unwrap().is_empty());
    }
}
