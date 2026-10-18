//! Read-write database access plus the migration runner.
//!
//! Reproduces the schema of `src/main/db/database.ts` (inline migrations 1..15 tracked in
//! `_migrations`) and the CREATE-IF-NOT-EXISTS schemas other TS services register. It is
//! idempotent and only creates what is missing; existing data is never dropped or rewritten
//! (the data-fix migrations only run when the id is not yet recorded in `_migrations`).
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

enum Step {
    Sql(&'static str),
    /// ALTER TABLE ADD COLUMN, skipped when the column already exists.
    AddColumn(&'static str, &'static str, &'static str),
}

struct Migration {
    id: i64,
    name: &'static str,
    steps: &'static [Step],
}

use Step::{AddColumn, Sql};

const MIGRATIONS: &[Migration] = &[
    Migration { id: 1, name: "001_initial", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS manga (
        id            TEXT PRIMARY KEY,
        source_plugin TEXT NOT NULL,
        source_id     TEXT NOT NULL,
        title         TEXT NOT NULL,
        author        TEXT,
        artist        TEXT,
        description   TEXT,
        cover_path    TEXT,
        status        TEXT,
        genres        TEXT DEFAULT '[]',
        is_favorite   INTEGER DEFAULT 0,
        notes         TEXT,
        added_at      TEXT NOT NULL DEFAULT (datetime('now')),
        last_read_at  TEXT,
        UNIQUE(source_plugin, source_id)
      );
      CREATE TABLE IF NOT EXISTS chapter (
        id              TEXT PRIMARY KEY,
        manga_id        TEXT NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
        source_id       TEXT NOT NULL,
        number          REAL,
        title           TEXT,
        language        TEXT NOT NULL DEFAULT 'en',
        page_count      INTEGER,
        download_status TEXT NOT NULL DEFAULT 'none',
        ocr_status      TEXT NOT NULL DEFAULT 'none',
        tts_status      TEXT NOT NULL DEFAULT 'none',
        video_status    TEXT NOT NULL DEFAULT 'none',
        read_progress   INTEGER NOT NULL DEFAULT 0,
        is_read         INTEGER NOT NULL DEFAULT 0,
        pages_dir       TEXT,
        added_at        TEXT NOT NULL DEFAULT (datetime('now')),
        UNIQUE(manga_id, source_id)
      );
      CREATE TABLE IF NOT EXISTS pipeline_job (
        id            TEXT PRIMARY KEY,
        manga_id      TEXT REFERENCES manga(id),
        chapter_id    TEXT REFERENCES chapter(id) ON DELETE CASCADE,
        status        TEXT NOT NULL DEFAULT 'queued',
        current_stage TEXT,
        progress      REAL DEFAULT 0,
        config_json   TEXT NOT NULL,
        result_json   TEXT,
        error_message TEXT,
        created_at    TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE INDEX IF NOT EXISTS idx_chapter_manga ON chapter(manga_id);
      CREATE INDEX IF NOT EXISTS idx_pipeline_job_status ON pipeline_job(status);
      CREATE INDEX IF NOT EXISTS idx_pipeline_job_chapter ON pipeline_job(chapter_id);
    "#)] },
    Migration { id: 2, name: "002_history_and_bookmarks", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS reading_history (
        id          TEXT PRIMARY KEY,
        manga_id    TEXT NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
        chapter_id  TEXT NOT NULL REFERENCES chapter(id) ON DELETE CASCADE,
        page        INTEGER NOT NULL DEFAULT 0,
        read_at     TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE INDEX IF NOT EXISTS idx_history_manga ON reading_history(manga_id);
      CREATE INDEX IF NOT EXISTS idx_history_read_at ON reading_history(read_at DESC);
    "#)] },
    Migration { id: 3, name: "003_chapter_source_plugin", steps: &[AddColumn("chapter", "source_plugin", "TEXT")] },
    Migration { id: 4, name: "004_fix_source_plugin_data", steps: &[Sql(r#"
      UPDATE chapter SET source_plugin = 'mangadex'
        WHERE source_plugin IS NULL
          AND length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';
      UPDATE manga SET source_plugin = 'mangadex'
        WHERE length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';
      UPDATE chapter SET source_plugin = (
        SELECT manga.source_plugin FROM manga WHERE manga.id = chapter.manga_id
      ) WHERE source_plugin IS NULL;
    "#)] },
    Migration { id: 5, name: "005_fix_empty_source_plugin", steps: &[Sql(r#"
      UPDATE chapter SET source_plugin = 'mangadex'
        WHERE (source_plugin IS NULL OR source_plugin = '')
          AND length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';
      UPDATE chapter SET source_plugin = (
        SELECT manga.source_plugin FROM manga WHERE manga.id = chapter.manga_id
      ) WHERE source_plugin IS NULL OR source_plugin = '';
      UPDATE chapter SET download_status = 'none'
        WHERE download_status = 'failed' AND pages_dir IS NULL;
    "#)] },
    Migration { id: 6, name: "006_anime_tables", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS anime_addon (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL,
        version      TEXT NOT NULL,
        url          TEXT NOT NULL,
        manifest     TEXT NOT NULL,
        installed_at TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE TABLE IF NOT EXISTS anime_watch_history (
        id             TEXT PRIMARY KEY,
        addon_id       TEXT NOT NULL,
        anime_id       TEXT NOT NULL,
        anime_title    TEXT NOT NULL,
        cover_url      TEXT,
        episode_num    INTEGER,
        episode_title  TEXT,
        watched_at     TEXT NOT NULL DEFAULT (datetime('now')),
        progress_secs  REAL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS idx_anime_history_watched ON anime_watch_history(watched_at DESC);
    "#)] },
    Migration { id: 7, name: "007_character_cast", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS character (
        id              TEXT PRIMARY KEY,
        manga_id        TEXT NOT NULL,
        name            TEXT NOT NULL,
        description     TEXT,
        voice           TEXT,
        face_crop_path  TEXT,
        color           TEXT,
        created_at      TEXT NOT NULL DEFAULT (datetime('now')),
        FOREIGN KEY (manga_id) REFERENCES manga(id) ON DELETE CASCADE
      );
      CREATE INDEX IF NOT EXISTS idx_character_manga ON character(manga_id);
    "#)] },
    Migration { id: 8, name: "008_media_chat", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS media_chat_conversation (
        id             TEXT PRIMARY KEY,
        title          TEXT NOT NULL,
        model          TEXT NOT NULL,
        online_enabled INTEGER NOT NULL DEFAULT 0,
        created_at     TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE TABLE IF NOT EXISTS media_chat_message (
        id              TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES media_chat_conversation(id) ON DELETE CASCADE,
        role            TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
        content         TEXT NOT NULL,
        created_at      TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE INDEX IF NOT EXISTS idx_media_chat_updated ON media_chat_conversation(updated_at DESC);
      CREATE INDEX IF NOT EXISTS idx_media_chat_messages ON media_chat_message(conversation_id, created_at ASC);
    "#)] },
    Migration { id: 9, name: "009_media_chat_modes", steps: &[
        AddColumn("media_chat_conversation", "chat_mode", "TEXT NOT NULL DEFAULT 'mangavox'"),
        AddColumn("media_chat_conversation", "persona_id", "TEXT NOT NULL DEFAULT 'nova'"),
        AddColumn("media_chat_conversation", "adult_enabled", "INTEGER NOT NULL DEFAULT 0"),
    ] },
    Migration { id: 10, name: "010_media_intelligence", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS media_title (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL CHECK(kind IN ('anime','movie','series')),
        title TEXT NOT NULL,
        imdb_id TEXT,
        tmdb_id INTEGER,
        mal_id INTEGER,
        cover_url TEXT,
        banner_url TEXT,
        year INTEGER,
        genres_json TEXT NOT NULL DEFAULT '[]',
        languages_json TEXT NOT NULL DEFAULT '[]',
        description TEXT,
        score REAL,
        updated_at INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_media_title_kind ON media_title(kind);
      CREATE INDEX IF NOT EXISTS idx_media_title_imdb ON media_title(imdb_id);
      CREATE TABLE IF NOT EXISTS media_event (
        id TEXT PRIMARY KEY,
        media_id TEXT,
        kind TEXT NOT NULL,
        event_type TEXT NOT NULL,
        query TEXT,
        provider_id TEXT,
        episode_number INTEGER,
        position_secs REAL,
        duration_secs REAL,
        created_at INTEGER NOT NULL,
        FOREIGN KEY(media_id) REFERENCES media_title(id) ON DELETE CASCADE
      );
      CREATE INDEX IF NOT EXISTS idx_media_event_kind_time ON media_event(kind, created_at DESC);
      CREATE INDEX IF NOT EXISTS idx_media_event_media_time ON media_event(media_id, created_at DESC);
      CREATE INDEX IF NOT EXISTS idx_media_event_type ON media_event(event_type);
      CREATE TABLE IF NOT EXISTS media_playback (
        media_id TEXT PRIMARY KEY REFERENCES media_title(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        provider_id TEXT,
        episode_number INTEGER,
        position_secs REAL NOT NULL DEFAULT 0,
        duration_secs REAL NOT NULL DEFAULT 0,
        completed INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS streaming_provider (
        id TEXT PRIMARY KEY,
        payload_json TEXT NOT NULL,
        refreshed_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS media_import_batch (
        id TEXT PRIMARY KEY,
        source TEXT NOT NULL,
        filename TEXT,
        imported_at INTEGER NOT NULL,
        event_ids_json TEXT NOT NULL DEFAULT '[]'
      );
    "#)] },
    Migration { id: 11, name: "011_playback_reliability", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS playback_session_state (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        payload_json TEXT NOT NULL,
        recoverable INTEGER NOT NULL DEFAULT 1,
        updated_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS playback_source_failure (
        id TEXT PRIMARY KEY,
        media_id TEXT,
        source_name TEXT NOT NULL,
        failure_code TEXT NOT NULL,
        failed_at INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_playback_source_failure_media ON playback_source_failure(media_id, failed_at DESC);
      CREATE TABLE IF NOT EXISTS subtitle_cache (
        cache_key TEXT PRIMARY KEY,
        media_id TEXT,
        season_number INTEGER,
        episode_number INTEGER,
        language TEXT NOT NULL,
        release_name TEXT,
        file_path TEXT NOT NULL,
        source TEXT NOT NULL,
        created_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS player_preferences (
        id INTEGER PRIMARY KEY CHECK(id = 1),
        payload_json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
      );
      UPDATE media_playback
      SET duration_secs = 0, completed = 0
      WHERE duration_secs > 0
        AND duration_secs < 120
        AND ABS(position_secs - duration_secs) < 1;
    "#)] },
    Migration { id: 12, name: "012_chat_smart_titles", steps: &[
        AddColumn("media_chat_conversation", "icon", "TEXT NOT NULL DEFAULT ''"),
        AddColumn("media_chat_conversation", "subtitle", "TEXT NOT NULL DEFAULT ''"),
    ] },
    Migration { id: 13, name: "013_custom_skills", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS custom_skill (
        id            TEXT PRIMARY KEY,
        slug          TEXT NOT NULL,
        name          TEXT NOT NULL,
        icon          TEXT NOT NULL DEFAULT '🛠️',
        description   TEXT NOT NULL DEFAULT '',
        instructions  TEXT NOT NULL DEFAULT '',
        knowledge     TEXT NOT NULL DEFAULT '',
        triggers_json TEXT NOT NULL DEFAULT '[]',
        sources_json  TEXT NOT NULL DEFAULT '[]',
        usage_count   INTEGER NOT NULL DEFAULT 0,
        created_at    INTEGER NOT NULL,
        updated_at    INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_custom_skill_slug ON custom_skill(slug);
    "#)] },
    Migration { id: 14, name: "014_message_meta", steps: &[AddColumn("media_chat_message", "meta_json", "TEXT NOT NULL DEFAULT ''")] },
    Migration { id: 15, name: "015_library_categories", steps: &[Sql(r#"
      CREATE TABLE IF NOT EXISTS library_category (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE TABLE IF NOT EXISTS manga_category (
        manga_id    TEXT NOT NULL,
        category_id TEXT NOT NULL,
        PRIMARY KEY (manga_id, category_id)
      );
      CREATE INDEX IF NOT EXISTS idx_manga_category_cat ON manga_category(category_id);
    "#)] },
];

/// Tables the TS services create with `registerSchema` (run on every start; all idempotent).
const SERVICE_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS watchlist_status (
      media_id   TEXT PRIMARY KEY,
      kind       TEXT NOT NULL,
      status     TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_watchlist_kind_status ON watchlist_status(kind, status);
    CREATE TABLE IF NOT EXISTS agent_memory (
      user_id TEXT PRIMARY KEY,
      data TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS assistant_conv_memory (
      conversation_id TEXT PRIMARY KEY,
      summary TEXT NOT NULL DEFAULT '',
      facts_json TEXT NOT NULL DEFAULT '[]',
      message_count INTEGER NOT NULL DEFAULT 0,
      topic TEXT NOT NULL DEFAULT '',
      updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS assistant_memory_buckets (
      user_id TEXT PRIMARY KEY,
      data TEXT NOT NULL DEFAULT '{}',
      updated_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS ai_token_usage (
      request_id TEXT PRIMARY KEY,
      conversation_id TEXT REFERENCES media_chat_conversation(id) ON DELETE CASCADE,
      tokens INTEGER NOT NULL,
      private INTEGER NOT NULL DEFAULT 0,
      mode TEXT NOT NULL DEFAULT 'chat',
      label TEXT NOT NULL DEFAULT '',
      status TEXT NOT NULL DEFAULT 'done',
      provider TEXT NOT NULL DEFAULT '',
      created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_ai_token_usage_created ON ai_token_usage(created_at DESC);
"#;

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|n| n == column))
}

fn add_column_if_missing(conn: &Connection, table: &str, column: &str, def: &str) -> Result<()> {
    if !has_column(conn, table, column)? {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {def}"))?;
    }
    Ok(())
}

/// Applies pending migrations and the service schemas. Safe to call any number of times.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
           id   INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         )",
    )?;
    let applied: std::collections::HashSet<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM _migrations")?;
        let rows = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        rows
    };
    for m in MIGRATIONS.iter().filter(|m| !applied.contains(&m.id)) {
        let tx = conn.transaction()?;
        for step in m.steps {
            match step {
                Sql(sql) => tx.execute_batch(sql)?,
                AddColumn(t, c, d) => add_column_if_missing(&tx, t, c, d)?,
            }
        }
        tx.execute("INSERT INTO _migrations (id, name) VALUES (?1, ?2)", rusqlite::params![m.id, m.name])?;
        tx.commit()?;
    }
    conn.execute_batch(SERVICE_SCHEMA)?;
    add_column_if_missing(conn, "assistant_conv_memory", "topic", "TEXT NOT NULL DEFAULT ''")?;
    add_column_if_missing(conn, "ai_token_usage", "provider", "TEXT NOT NULL DEFAULT ''")?;
    Ok(())
}

/// Download workers are process-local: rows left queued/downloading/failed by a previous run
/// cannot be active and must be retryable (same as `initDatabase` in the TS app).
pub fn reset_stale_downloads(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE chapter SET download_status = 'none' WHERE download_status IN ('queued', 'downloading', 'failed')",
        [],
    )?)
}

/// Opens (creating if needed) a read-write connection with WAL, foreign keys and a busy timeout.
/// Does not migrate; see [`open_and_migrate`].
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// `open` + `migrate` + `reset_stale_downloads` — the equivalent of `initDatabase()`.
pub fn open_and_migrate(path: &Path) -> Result<Connection> {
    let mut conn = open(path)?;
    migrate(&mut conn)?;
    reset_stale_downloads(&conn)?;
    Ok(conn)
}

/// Opens the user's real database (`paths::database()`) read-write and migrates it.
pub fn open_default() -> Result<Connection> {
    open_and_migrate(&crate::paths::database())
}

/// Test helper: a migrated database in a temp dir (keep the guard alive).
#[cfg(test)]
pub(crate) fn temp_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_and_migrate(&dir.path().join("t.db")).unwrap();
    (dir, conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let (_d, mut conn) = temp_db();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0)).unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
        assert!(has_column(&conn, "media_chat_message", "meta_json").unwrap());
        assert!(has_column(&conn, "chapter", "source_plugin").unwrap());
        let fk: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn existing_columns_and_data_survive() {
        // A DB whose tracking rows are missing but whose columns/data already exist.
        let (_d, mut conn) = temp_db();
        conn.execute("INSERT INTO manga (id, source_plugin, source_id, title) VALUES ('m','p','s','T')", []).unwrap();
        conn.execute("DELETE FROM _migrations WHERE id IN (3, 9, 12, 14)", []).unwrap();
        migrate(&mut conn).unwrap();
        let t: String = conn.query_row("SELECT title FROM manga WHERE id='m'", [], |r| r.get(0)).unwrap();
        assert_eq!(t, "T");
    }

    #[test]
    fn stale_downloads_reset_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        {
            let conn = open_and_migrate(&p).unwrap();
            conn.execute("INSERT INTO manga (id, source_plugin, source_id, title) VALUES ('m','p','s','T')", []).unwrap();
            conn.execute("INSERT INTO chapter (id, manga_id, source_id, download_status) VALUES ('c','m','x','downloading')", []).unwrap();
        }
        let conn = open_and_migrate(&p).unwrap();
        let s: String = conn.query_row("SELECT download_status FROM chapter WHERE id='c'", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "none");
    }

    #[test]
    fn real_db_copy_migrates_and_lists() {
        let src = crate::paths::database();
        if !src.exists() {
            eprintln!("skipping: no real database");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("copy.db");
        // SQLite backup API from a read-only handle: includes WAL contents, leaves the original untouched.
        {
            let ro = Connection::open_with_flags(&src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            ro.busy_timeout(Duration::from_secs(5)).unwrap();
            let mut out = Connection::open(&dst).unwrap();
            let b = rusqlite::backup::Backup::new(&ro, &mut out).unwrap();
            b.run_to_completion(100, Duration::from_millis(5), None).unwrap();
        }
        let mut conn = open(&dst).unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let list = crate::library::list_manga(&conn).unwrap();
        let repo_list = crate::repo::manga::list(&conn).unwrap();
        assert_eq!(list.len(), repo_list.len());
        eprintln!("real db copy: {} manga", list.len());
    }
}
