-- Crush schema v1. Applied transactionally by the store crate.
-- Never edit an applied migration; add a new numbered file instead.

CREATE TABLE owners (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;

INSERT INTO owners (id, name, created_at)
VALUES ('local', 'Local user', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE videos (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  path        TEXT NOT NULL,
  sha256      TEXT NOT NULL,
  duration_s  REAL,
  fps         REAL,
  width       INTEGER,
  height      INTEGER,
  has_audio   INTEGER NOT NULL DEFAULT 1 CHECK (has_audio IN (0, 1)),
  status      TEXT NOT NULL DEFAULT 'pending'
              CHECK (status IN ('pending', 'split', 'embedded', 'transcribed', 'done', 'failed')),
  indexed_at  TEXT,
  UNIQUE(owner_id, sha256),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX videos_owner ON videos(owner_id);

CREATE TABLE shots (
  id           TEXT PRIMARY KEY,
  video_id     TEXT NOT NULL,
  owner_id     TEXT NOT NULL REFERENCES owners(id),
  idx          INTEGER NOT NULL CHECK (idx >= 0),
  start_s      REAL NOT NULL CHECK (start_s >= 0),
  end_s        REAL NOT NULL CHECK (end_s > start_s),
  rep_frame_s  REAL NOT NULL CHECK (rep_frame_s >= start_s AND rep_frame_s <= end_s),
  thumb_rel    TEXT,
  scene_score  REAL,
  UNIQUE(video_id, idx),
  UNIQUE(id, owner_id),
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX shots_video ON shots(video_id, idx);
CREATE INDEX shots_owner ON shots(owner_id);

CREATE TABLE shot_vectors (
  shot_id   TEXT PRIMARY KEY,
  owner_id  TEXT NOT NULL REFERENCES owners(id),
  dim       INTEGER NOT NULL CHECK (dim > 0),
  vec       BLOB NOT NULL,
  FOREIGN KEY(shot_id, owner_id) REFERENCES shots(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX shot_vectors_owner ON shot_vectors(owner_id, shot_id);

CREATE TABLE transcripts (
  id         TEXT PRIMARY KEY,
  video_id   TEXT NOT NULL,
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  start_s    REAL NOT NULL CHECK (start_s >= 0),
  end_s      REAL NOT NULL CHECK (end_s > start_s),
  text       TEXT NOT NULL,
  confidence REAL,
  UNIQUE(id, owner_id),
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX transcripts_video_time ON transcripts(video_id, start_s, end_s);
CREATE INDEX transcripts_owner ON transcripts(owner_id);
CREATE VIRTUAL TABLE transcripts_fts USING fts5(
  text,
  content='transcripts',
  content_rowid='rowid'
);

CREATE TABLE jobs (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  video_id    TEXT NOT NULL,
  stage       TEXT NOT NULL CHECK (stage IN ('split', 'embed', 'transcribe')),
  status      TEXT NOT NULL CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  error       TEXT,
  debug_dir   TEXT,
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX jobs_video ON jobs(video_id, started_at);
CREATE INDEX jobs_owner_status ON jobs(owner_id, status, started_at);

CREATE TABLE embedding_meta (
  owner_id           TEXT PRIMARY KEY REFERENCES owners(id),
  model_name         TEXT NOT NULL,
  model_sha256       TEXT NOT NULL,
  dim                INTEGER NOT NULL CHECK (dim > 0),
  preprocess_version INTEGER NOT NULL CHECK (preprocess_version > 0)
) STRICT;
