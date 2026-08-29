-- Crush schema v4: inspectable cold-start strong-shot components.

ALTER TABLE aesthetic_assessments ADD COLUMN technical_quality REAL NOT NULL DEFAULT 0.5 CHECK (technical_quality BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN blur_control REAL NOT NULL DEFAULT 0.5 CHECK (blur_control BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN clipping_control REAL NOT NULL DEFAULT 0.5 CHECK (clipping_control BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN noise_control REAL NOT NULL DEFAULT 0.5 CHECK (noise_control BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN compression_quality REAL NOT NULL DEFAULT 0.5 CHECK (compression_quality BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN resolution_quality REAL NOT NULL DEFAULT 0.5 CHECK (resolution_quality BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN motion_stability REAL NOT NULL DEFAULT 0.5 CHECK (motion_stability BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN duplicate_confidence REAL NOT NULL DEFAULT 0.0 CHECK (duplicate_confidence BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN composition_quality REAL NOT NULL DEFAULT 0.5 CHECK (composition_quality BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN hierarchy REAL NOT NULL DEFAULT 0.5 CHECK (hierarchy BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN leading_lines REAL NOT NULL DEFAULT 0.5 CHECK (leading_lines BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN symmetry REAL NOT NULL DEFAULT 0.5 CHECK (symmetry BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN crop_potential REAL NOT NULL DEFAULT 0.5 CHECK (crop_potential BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN moment_story REAL NOT NULL DEFAULT 0.5 CHECK (moment_story BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN expression REAL NOT NULL DEFAULT 0.5 CHECK (expression BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN gesture REAL NOT NULL DEFAULT 0.5 CHECK (gesture BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN action REAL NOT NULL DEFAULT 0.5 CHECK (action BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN novelty REAL NOT NULL DEFAULT 0.5 CHECK (novelty BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN pacing REAL NOT NULL DEFAULT 0.5 CHECK (pacing BETWEEN 0.0 AND 1.0);
ALTER TABLE aesthetic_assessments ADD COLUMN repetition_risk REAL NOT NULL DEFAULT 0.0 CHECK (repetition_risk BETWEEN 0.0 AND 1.0);

CREATE INDEX aesthetic_assessments_strongest
ON aesthetic_assessments(owner_id, overall DESC, confidence DESC, media_kind, media_id);

-- Analysis is independently resumable and visible in the job log.
DROP INDEX jobs_video;
DROP INDEX jobs_owner_status;
ALTER TABLE jobs RENAME TO jobs_v3;
CREATE TABLE jobs (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  video_id    TEXT NOT NULL,
  stage       TEXT NOT NULL CHECK (stage IN ('split', 'embed', 'analyze', 'transcribe')),
  status      TEXT NOT NULL CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  error       TEXT,
  debug_dir   TEXT,
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;
INSERT INTO jobs SELECT * FROM jobs_v3;
DROP TABLE jobs_v3;
CREATE INDEX jobs_video ON jobs(video_id, started_at);
CREATE INDEX jobs_owner_status ON jobs(owner_id, status, started_at);
