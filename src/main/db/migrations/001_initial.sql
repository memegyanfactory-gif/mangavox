CREATE TABLE manga (
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

CREATE TABLE chapter (
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

CREATE TABLE pipeline_job (
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

CREATE INDEX idx_chapter_manga ON chapter(manga_id);
CREATE INDEX idx_pipeline_job_status ON pipeline_job(status);
CREATE INDEX idx_pipeline_job_chapter ON pipeline_job(chapter_id);
