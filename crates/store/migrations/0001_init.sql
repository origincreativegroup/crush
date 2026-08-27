-- Crush schema v1. Applied by the store crate at startup. Never edit an applied migration; add a new file.
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS owners (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL
);
INSERT OR IGNORE INTO owners (id, name, created_at) VALUES ('local', 'Local user', strftime('%Y-%m-%dT%H:%M:%fZ','now'));

CREATE TABLE IF NOT EXISTS videos (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  path        TEXT NOT NULL,
  sha256      TEXT NOT NULL,
  duration_s  REAL,
  fps         REAL,
  width       INTEGER,
  height      INTEGER,
  has_audio   INTEGER NOT NULL DEFAULT 1,
  status      TEXT NOT NULL DEFAULT 'pending',  -- pending | split | embedded | transcribed | done | failed
  indexed_at  TEXT,
  UNIQUE(owner_id, sha256)
);
CREATE INDEX IF NOT EXISTS videos_owner ON videos(owner_id);

CREATE TABLE IF NOT EXISTS shots (
  id           TEXT PRIMARY KEY,
  video_id     TEXT NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  owner_id     TEXT NOT NULL REFERENCES owners(id),
  idx          INTEGER NOT NULL,
  start_s      REAL NOT NULL,
  end_s        REAL NOT NULL,
  rep_frame_s  REAL NOT NULL,
  thumb_rel    TEXT,               -- relative to <data_dir>/thumbs
  scene_score  REAL,               -- detector value at the cut that opened this shot
  UNIQUE(video_id, idx)
);
CREATE INDEX IF NOT EXISTS shots_video ON shots(video_id);
CREATE INDEX IF NOT EXISTS shots_owner ON shots(owner_id);

CREATE TABLE IF NOT EXISTS shot_vectors (
  shot_id   TEXT PRIMARY KEY REFERENCES shots(id) ON DELETE CASCADE,
  owner_id  TEXT NOT NULL,
  dim       INTEGER NOT NULL,
  vec       BLOB NOT NULL          -- dim x f32 little-endian, L2-normalized
);

CREATE TABLE IF NOT EXISTS transcripts (
  id         TEXT PRIMARY KEY,
  video_id   TEXT NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  start_s    REAL NOT NULL,
  end_s      REAL NOT NULL,
  text       TEXT NOT NULL,
  confidence REAL
);
CREATE INDEX IF NOT EXISTS transcripts_video_time ON transcripts(video_id, start_s);
CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(text, content='transcripts', content_rowid='rowid');

CREATE TABLE IF NOT EXISTS jobs (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL,
  video_id    TEXT NOT NULL,
  stage       TEXT NOT NULL,       -- split | embed | transcribe
  status      TEXT NOT NULL,       -- queued | running | done | failed | cancelled
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER,
  error       TEXT,
  debug_dir   TEXT
);
CREATE INDEX IF NOT EXISTS jobs_video ON jobs(video_id);
CREATE INDEX IF NOT EXISTS jobs_status ON jobs(status);

CREATE TABLE IF NOT EXISTS embedding_meta (
  id                 INTEGER PRIMARY KEY CHECK (id = 1),
  model_name         TEXT NOT NULL,
  model_sha256       TEXT NOT NULL,
  dim                INTEGER NOT NULL,
  preprocess_version INTEGER NOT NULL
);

INSERT INTO schema_version (version) VALUES (1);
