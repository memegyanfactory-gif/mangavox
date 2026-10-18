use super::new_id;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Mirrors `MediaChatConversation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub subtitle: String,
    pub model: String,
    pub online_enabled: bool,
    /// `mangavox` | `open`
    pub chat_mode: String,
    pub persona_id: String,
    pub adult_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Mirrors `MediaChatMessage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: Option<String>,
    /// `user` | `assistant`
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub meta_json: String,
}

const CONV_COLS: &str = "id, title, icon, subtitle, model, online_enabled, chat_mode, persona_id, adult_enabled, created_at, updated_at";
const MSG_COLS: &str = "id, conversation_id, role, content, created_at, meta_json";

fn row_to_conv(r: &Row) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: r.get(0)?,
        title: r.get(1)?,
        icon: r.get(2)?,
        subtitle: r.get(3)?,
        model: r.get(4)?,
        online_enabled: r.get::<_, i64>(5)? != 0,
        chat_mode: r.get(6)?,
        persona_id: r.get(7)?,
        adult_enabled: r.get::<_, i64>(8)? != 0,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

fn row_to_msg(r: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        created_at: r.get(4)?,
        meta_json: r.get(5)?,
    })
}

fn meta_to_string(meta: Option<&serde_json::Value>) -> String {
    match meta {
        Some(m) if !m.is_null() => serde_json::to_string(m).unwrap_or_default(),
        _ => String::new(),
    }
}

pub fn list_conversations(conn: &Connection) -> Result<Vec<Conversation>> {
    let mut stmt = conn.prepare(&format!("SELECT {CONV_COLS} FROM media_chat_conversation ORDER BY updated_at DESC"))?;
    let rows = stmt.query_map([], row_to_conv)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_conversation(conn: &Connection, id: &str) -> Result<Option<Conversation>> {
    Ok(conn
        .query_row(&format!("SELECT {CONV_COLS} FROM media_chat_conversation WHERE id = ?1"), [id], row_to_conv)
        .optional()?)
}

pub fn create_conversation(conn: &Connection, title: &str, model: &str, online: bool, chat_mode: &str, persona_id: &str, adult: bool) -> Result<Conversation> {
    let id = new_id();
    conn.execute(
        "INSERT INTO media_chat_conversation (id, title, model, online_enabled, chat_mode, persona_id, adult_enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, title, model, online as i64, chat_mode, persona_id, adult as i64],
    )?;
    Ok(get_conversation(conn, &id)?.expect("just inserted"))
}

pub fn list_messages(conn: &Connection, conversation_id: &str) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MSG_COLS} FROM media_chat_message WHERE conversation_id = ?1 ORDER BY created_at ASC, rowid ASC"
    ))?;
    let rows = stmt.query_map([conversation_id], row_to_msg)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn get_message(conn: &Connection, id: &str) -> Result<Option<Message>> {
    Ok(conn.query_row(&format!("SELECT {MSG_COLS} FROM media_chat_message WHERE id = ?1"), [id], row_to_msg).optional()?)
}

pub fn add_message(conn: &mut Connection, conversation_id: &str, role: &str, content: &str, meta: Option<&serde_json::Value>) -> Result<Message> {
    let id = new_id();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO media_chat_message (id, conversation_id, role, content, meta_json) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, conversation_id, role, content, meta_to_string(meta)],
    )?;
    tx.execute("UPDATE media_chat_conversation SET updated_at = datetime('now') WHERE id = ?1", [conversation_id])?;
    tx.commit()?;
    Ok(get_message(conn, &id)?.expect("just inserted"))
}

pub fn update_message(conn: &mut Connection, id: &str, content: &str, meta: Option<&serde_json::Value>) -> Result<Option<Message>> {
    let tx = conn.transaction()?;
    tx.execute("UPDATE media_chat_message SET content = ?1, meta_json = ?2 WHERE id = ?3", params![content, meta_to_string(meta), id])?;
    tx.execute(
        "UPDATE media_chat_conversation SET updated_at = datetime('now')
         WHERE id = (SELECT conversation_id FROM media_chat_message WHERE id = ?1)",
        [id],
    )?;
    tx.commit()?;
    get_message(conn, id)
}

pub fn update_conversation_settings(conn: &Connection, id: &str, model: &str, online: bool, chat_mode: &str, persona_id: &str, adult: bool) -> Result<()> {
    conn.execute(
        "UPDATE media_chat_conversation
         SET model = ?1, online_enabled = ?2, chat_mode = ?3, persona_id = ?4, adult_enabled = ?5, updated_at = datetime('now')
         WHERE id = ?6",
        params![model, online as i64, chat_mode, persona_id, adult as i64, id],
    )?;
    Ok(())
}

pub fn update_conversation_meta(conn: &Connection, id: &str, title: &str, icon: &str, subtitle: &str) -> Result<()> {
    conn.execute("UPDATE media_chat_conversation SET title = ?1, icon = ?2, subtitle = ?3 WHERE id = ?4", params![title, icon, subtitle, id])?;
    Ok(())
}

pub fn delete_conversation(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM media_chat_conversation WHERE id = ?1", [id])?;
    Ok(())
}

pub fn clear_conversations(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM media_chat_conversation", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::temp_db;
    use serde_json::json;

    #[test]
    fn conversation_and_messages_roundtrip() {
        let (_d, mut conn) = temp_db();
        let c = create_conversation(&conn, "Hello", "gpt", true, "mangavox", "nova", false).unwrap();
        assert!(c.online_enabled && !c.adult_enabled);
        assert_eq!((c.icon.as_str(), c.subtitle.as_str()), ("", ""));
        let m1 = add_message(&mut conn, &c.id, "user", "hi", None).unwrap();
        let m2 = add_message(&mut conn, &c.id, "assistant", "yo", Some(&json!({"a":1}))).unwrap();
        assert_eq!((m1.meta_json.as_str(), m2.meta_json.as_str()), ("", r#"{"a":1}"#));
        assert_eq!(list_messages(&conn, &c.id).unwrap().iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["hi", "yo"]);
        let u = update_message(&mut conn, &m2.id, "yo!", None).unwrap().unwrap();
        assert_eq!((u.content.as_str(), u.meta_json.as_str()), ("yo!", ""));
        assert!(add_message(&mut conn, &c.id, "system", "x", None).is_err()); // CHECK constraint

        update_conversation_settings(&conn, &c.id, "m2", false, "open", "p", true).unwrap();
        update_conversation_meta(&conn, &c.id, "T", "🎬", "sub").unwrap();
        let g = get_conversation(&conn, &c.id).unwrap().unwrap();
        assert_eq!((g.model.as_str(), g.chat_mode.as_str(), g.persona_id.as_str(), g.online_enabled, g.adult_enabled), ("m2", "open", "p", false, true));
        assert_eq!((g.title.as_str(), g.icon.as_str(), g.subtitle.as_str()), ("T", "🎬", "sub"));
        assert_eq!(list_conversations(&conn).unwrap().len(), 1);
        delete_conversation(&conn, &c.id).unwrap();
        assert!(list_messages(&conn, &c.id).unwrap().is_empty()); // cascade
        create_conversation(&conn, "x", "m", false, "mangavox", "nova", false).unwrap();
        clear_conversations(&conn).unwrap();
        assert!(list_conversations(&conn).unwrap().is_empty());
    }
}
