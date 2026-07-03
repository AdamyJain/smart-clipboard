-- Schema v1 — architecture doc §3.3
CREATE TABLE IF NOT EXISTS captures (
  id TEXT PRIMARY KEY,              -- ULID
  session_id TEXT REFERENCES sessions(id),
  captured_at INTEGER NOT NULL,
  content_type TEXT,                -- code | url | text | secret | image
  entity_type TEXT,
  raw_text TEXT,
  source_app TEXT,
  source_url TEXT,
  page_title TEXT,
  context_before TEXT,
  context_after TEXT,
  sensitivity TEXT DEFAULT 'unknown',
  dedupe_hash TEXT,
  asset_id TEXT REFERENCES assets(id),
  enrichment_status TEXT DEFAULT 'pending',
  embedded INTEGER DEFAULT 0,       -- 1 once written to captures_vec
  deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_captures_at ON captures(captured_at DESC);
CREATE INDEX IF NOT EXISTS idx_captures_dedupe ON captures(dedupe_hash);

CREATE VIRTUAL TABLE IF NOT EXISTS captures_fts USING fts5(
  raw_text, context_before, context_after,
  content=captures, content_rowid=rowid,
  tokenize='trigram'
);

CREATE VIRTUAL TABLE IF NOT EXISTS captures_vec USING vec0(
  capture_id TEXT PRIMARY KEY,
  embedding FLOAT[384]
);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  topic TEXT,
  started_at INTEGER,
  last_activity_at INTEGER,
  ended_at INTEGER,
  status TEXT DEFAULT 'open',
  boundary_source TEXT,
  affinity_hint TEXT,
  summary TEXT,
  deleted_at INTEGER
);

CREATE TABLE IF NOT EXISTS tags (id TEXT PRIMARY KEY, label TEXT, kind TEXT);
CREATE TABLE IF NOT EXISTS capture_tags (capture_id TEXT, tag_id TEXT);

CREATE TABLE IF NOT EXISTS assets (
  id TEXT PRIMARY KEY,
  file_path TEXT,
  ocr_text TEXT,
  ocr_source TEXT
);

CREATE TABLE IF NOT EXISTS enrichment_queue (
  capture_id TEXT PRIMARY KEY,
  attempts INTEGER DEFAULT 0,
  next_attempt_at INTEGER
);

CREATE TABLE IF NOT EXISTS access_log (
  id TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  ref_id TEXT,
  bytes_sent INTEGER
);

CREATE TABLE IF NOT EXISTS session_corrections (
  id TEXT PRIMARY KEY,
  ts INTEGER NOT NULL,
  kind TEXT NOT NULL,
  from_session TEXT,
  to_session TEXT,
  capture_id TEXT
);
