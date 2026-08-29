-- Crush schema v5: photo ingest and photo analysis share the resumable job log.
--
-- Rebuilds `jobs` because SQLite cannot alter FOREIGN KEY clauses: video jobs keep their
-- composite FK to videos(id, owner_id), photo jobs gain the same pattern against
-- photos(id, owner_id), and the stage CHECK gains 'photo_ingest' ('analyze' was already
-- legal and is reused for photo analysis). An exactly-one-of CHECK keeps every row attached
-- to precisely one owned asset.

DROP INDEX jobs_video;
DROP INDEX jobs_owner_status;
ALTER TABLE jobs RENAME TO jobs_v4;
CREATE TABLE jobs (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  video_id    TEXT,
  photo_id    TEXT,
  stage       TEXT NOT NULL CHECK (stage IN ('split', 'embed', 'analyze', 'transcribe', 'photo_ingest')),
  status      TEXT NOT NULL CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  error       TEXT,
  debug_dir   TEXT,
  CHECK ((video_id IS NULL) <> (photo_id IS NULL)),
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE,
  FOREIGN KEY(photo_id, owner_id) REFERENCES photos(id, owner_id) ON DELETE CASCADE
) STRICT;
INSERT INTO jobs (id, owner_id, video_id, stage, status, started_at, finished_at, duration_ms, error, debug_dir)
SELECT id, owner_id, video_id, stage, status, started_at, finished_at, duration_ms, error, debug_dir
FROM jobs_v4;
DROP TABLE jobs_v4;
CREATE INDEX jobs_video ON jobs(video_id, started_at);
CREATE INDEX jobs_owner_status ON jobs(owner_id, status, started_at);
CREATE INDEX jobs_photo ON jobs(photo_id, started_at);
