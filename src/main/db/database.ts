import Database from 'better-sqlite3'
import { AppPaths } from '../services/app-paths'

let db: Database.Database | null = null

// Services outside the migration system (memory buckets, conversation memory,
// agent memory, …) register their CREATE TABLE IF NOT EXISTS here. They run on
// every initDatabase() call, so a factory reset that recreates the DB file
// mid-session restores these tables too — not just the migrated ones.
type SchemaInitializer = (db: Database.Database) => void
const schemaInitializers: SchemaInitializer[] = []

export function registerSchema(init: SchemaInitializer): void {
  schemaInitializers.push(init)
  if (db) {
    try { init(db) } catch (err) { console.error('[db] schema init failed:', err) }
  }
}

export function getDb(): Database.Database {
  if (!db) {
    console.warn('Database was not initialized yet — auto-initializing now')
    initDatabase()
  }
  return db!
}

export function initDatabase(): Database.Database {
  db = new Database(AppPaths.db)

  db.pragma('journal_mode = WAL')
  db.pragma('foreign_keys = ON')

  // Create migrations tracking table
  db.exec(`
    CREATE TABLE IF NOT EXISTS _migrations (
      id   INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      applied_at TEXT NOT NULL DEFAULT (datetime('now'))
    )
  `)

  runMigrations(db)
  for (const init of schemaInitializers) {
    try { init(db) } catch (err) { console.error('[db] schema init failed:', err) }
  }
  // Download workers are process-local. If the app was closed or crashed,
  // queued/downloading rows cannot still be active and must be retryable.
  db.prepare(`
    UPDATE chapter
    SET download_status = 'none'
    WHERE download_status IN ('queued', 'downloading', 'failed')
  `).run()
  return db
}

// Inline migrations — more reliable than reading .sql files in bundled Electron apps
const migrations: { id: number; name: string; sql: string }[] = [
  {
    id: 1,
    name: '001_initial',
    sql: `
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
    `,
  },
  {
    id: 2,
    name: '002_history_and_bookmarks',
    sql: `
      CREATE TABLE IF NOT EXISTS reading_history (
        id          TEXT PRIMARY KEY,
        manga_id    TEXT NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
        chapter_id  TEXT NOT NULL REFERENCES chapter(id) ON DELETE CASCADE,
        page        INTEGER NOT NULL DEFAULT 0,
        read_at     TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE INDEX IF NOT EXISTS idx_history_manga ON reading_history(manga_id);
      CREATE INDEX IF NOT EXISTS idx_history_read_at ON reading_history(read_at DESC);
    `,
  },
  {
    id: 3,
    name: '003_chapter_source_plugin',
    sql: `
      ALTER TABLE chapter ADD COLUMN source_plugin TEXT;
    `,
  },
  {
    id: 4,
    name: '004_fix_source_plugin_data',
    sql: `
      -- MangaDex source_ids are UUIDs (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
      -- Fix chapters: if source_id looks like a UUID, it's from MangaDex
      UPDATE chapter SET source_plugin = 'mangadex'
        WHERE source_plugin IS NULL
          AND length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';

      -- Fix manga: if source_id is a UUID, reset source_plugin to mangadex
      -- (it may have been corrupted by the old source-switching code)
      UPDATE manga SET source_plugin = 'mangadex'
        WHERE length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';

      -- For any remaining chapters with no source_plugin, inherit from manga
      UPDATE chapter SET source_plugin = (
        SELECT manga.source_plugin FROM manga WHERE manga.id = chapter.manga_id
      ) WHERE source_plugin IS NULL;
    `,
  },
  {
    id: 5,
    name: '005_fix_empty_source_plugin',
    sql: `
      -- Migration 004 only fixed NULL source_plugin, not empty string ''
      -- Fix chapters with empty source_plugin using same UUID detection
      UPDATE chapter SET source_plugin = 'mangadex'
        WHERE (source_plugin IS NULL OR source_plugin = '')
          AND length(source_id) = 36
          AND source_id LIKE '________-____-____-____-____________';

      -- For any remaining chapters with empty source_plugin, inherit from manga
      UPDATE chapter SET source_plugin = (
        SELECT manga.source_plugin FROM manga WHERE manga.id = chapter.manga_id
      ) WHERE source_plugin IS NULL OR source_plugin = '';

      -- Reset failed downloads so users can retry (failed from batch rate-limiting)
      UPDATE chapter SET download_status = 'none'
        WHERE download_status = 'failed' AND pages_dir IS NULL;
    `,
  },
  {
    id: 6,
    name: '006_anime_tables',
    sql: `
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
    `,
  },
  {
    id: 7,
    name: '007_character_cast',
    sql: `
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
    `,
  },
  {
    id: 8,
    name: '008_media_chat',
    sql: `
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

      CREATE INDEX IF NOT EXISTS idx_media_chat_updated
        ON media_chat_conversation(updated_at DESC);
      CREATE INDEX IF NOT EXISTS idx_media_chat_messages
        ON media_chat_message(conversation_id, created_at ASC);
    `,
  },
  {
    id: 9,
    name: '009_media_chat_modes',
    sql: `
      ALTER TABLE media_chat_conversation ADD COLUMN chat_mode TEXT NOT NULL DEFAULT 'mangavox';
      ALTER TABLE media_chat_conversation ADD COLUMN persona_id TEXT NOT NULL DEFAULT 'nova';
      ALTER TABLE media_chat_conversation ADD COLUMN adult_enabled INTEGER NOT NULL DEFAULT 0;
    `,
  },
  {
    id: 10,
    name: '010_media_intelligence',
    sql: `
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
    `,
  },
  {
    id: 11,
    name: '011_playback_reliability',
    sql: `
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
      CREATE INDEX IF NOT EXISTS idx_playback_source_failure_media
        ON playback_source_failure(media_id, failed_at DESC);

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
    `,
  },
  {
    id: 12,
    name: '012_chat_smart_titles',
    sql: `
      ALTER TABLE media_chat_conversation ADD COLUMN icon TEXT NOT NULL DEFAULT '';
      ALTER TABLE media_chat_conversation ADD COLUMN subtitle TEXT NOT NULL DEFAULT '';
    `,
  },
  {
    id: 13,
    name: '013_custom_skills',
    sql: `
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
    `,
  },
  {
    id: 14,
    name: '014_message_meta',
    sql: `
      ALTER TABLE media_chat_message ADD COLUMN meta_json TEXT NOT NULL DEFAULT '';
    `,
  },
  {
    id: 15,
    name: '015_library_categories',
    sql: `
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
    `,
  },
]

function runMigrations(db: Database.Database): void {
  const applied = new Set(
    db
      .prepare('SELECT id FROM _migrations')
      .all()
      .map((r: any) => r.id)
  )

  for (const migration of migrations) {
    if (applied.has(migration.id)) continue

    db.transaction(() => {
      db.exec(migration.sql)
      db.prepare('INSERT INTO _migrations (id, name) VALUES (?, ?)').run(
        migration.id,
        migration.name
      )
    })()

    console.log(`Migration applied: ${migration.name}`)
  }
}

export function closeDatabase(): void {
  if (db) {
    db.close()
    db = null
  }
}
