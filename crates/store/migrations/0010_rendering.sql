-- Crush schema v10: immutable render recipes, durable attempts, and verified outputs.
--
-- Recipes and successful artifacts are append-only evidence. Jobs and their active attempt are
-- mutable state machines so interrupted work can be retried from the frozen recipe/source/plan
-- snapshots without consulting the current mutable plan.

CREATE TABLE render_recipes (
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  id          TEXT NOT NULL,
  version     INTEGER NOT NULL CHECK (version > 0),
  kind        TEXT NOT NULL CHECK (kind IN ('photo', 'video_clip', 'reel')),
  name        TEXT NOT NULL,
  schema_json TEXT NOT NULL,
  created_at  TEXT NOT NULL,
  PRIMARY KEY(owner_id, id, version)
) STRICT;

CREATE INDEX render_recipes_owner_created
ON render_recipes(owner_id, created_at, id, version);

CREATE TRIGGER render_recipes_no_update BEFORE UPDATE ON render_recipes BEGIN
  SELECT RAISE(ABORT, 'render_recipes is append-only');
END;

CREATE TRIGGER render_recipes_no_delete BEFORE DELETE ON render_recipes BEGIN
  SELECT RAISE(ABORT, 'render_recipes is append-only');
END;

CREATE TABLE render_jobs (
  owner_id             TEXT NOT NULL REFERENCES owners(id),
  id                   TEXT NOT NULL,
  recipe_id            TEXT NOT NULL,
  recipe_version       INTEGER NOT NULL CHECK (recipe_version > 0),
  recipe_kind          TEXT NOT NULL CHECK (recipe_kind IN ('photo', 'video_clip', 'reel')),
  frozen_recipe_json   TEXT NOT NULL,
  plan_id              TEXT,
  plan_revision        INTEGER CHECK (plan_revision IS NULL OR plan_revision > 0),
  frozen_plan_json     TEXT,
  source_snapshot_json TEXT NOT NULL,
  model_versions_json  TEXT NOT NULL,
  destination_path     TEXT NOT NULL,
  status               TEXT NOT NULL CHECK (
                         status IN ('queued', 'running', 'verifying', 'done', 'failed', 'cancelled')
                       ),
  progress             REAL NOT NULL DEFAULT 0.0 CHECK (progress BETWEEN 0.0 AND 1.0),
  current_attempt      INTEGER NOT NULL DEFAULT 0 CHECK (current_attempt >= 0),
  error                TEXT,
  created_at           TEXT NOT NULL,
  started_at           TEXT,
  finished_at          TEXT,
  PRIMARY KEY(owner_id, id),
  FOREIGN KEY(owner_id, recipe_id, recipe_version)
    REFERENCES render_recipes(owner_id, id, version),
  CHECK ((plan_id IS NULL) = (plan_revision IS NULL)),
  CHECK ((plan_id IS NULL) = (frozen_plan_json IS NULL)),
  CHECK ((status = 'done') = (progress = 1.0)),
  CHECK ((status IN ('done', 'failed', 'cancelled')) = (finished_at IS NOT NULL))
) STRICT;

CREATE INDEX render_jobs_owner_status
ON render_jobs(owner_id, status, created_at, id);

CREATE TRIGGER render_jobs_frozen_inputs_no_update
BEFORE UPDATE OF recipe_id, recipe_version, recipe_kind, frozen_recipe_json,
                 plan_id, plan_revision, frozen_plan_json, source_snapshot_json,
                 model_versions_json, destination_path, created_at
ON render_jobs
BEGIN
  SELECT RAISE(ABORT, 'render job inputs are immutable');
END;

CREATE TRIGGER render_jobs_no_delete BEFORE DELETE ON render_jobs BEGIN
  SELECT RAISE(ABORT, 'render jobs are durable history');
END;

CREATE TABLE render_attempts (
  owner_id     TEXT NOT NULL,
  job_id       TEXT NOT NULL,
  attempt      INTEGER NOT NULL CHECK (attempt > 0),
  status       TEXT NOT NULL CHECK (status IN ('running', 'verifying', 'done', 'failed', 'cancelled')),
  staging_path TEXT NOT NULL,
  progress     REAL NOT NULL DEFAULT 0.0 CHECK (progress BETWEEN 0.0 AND 1.0),
  command_json TEXT NOT NULL DEFAULT '[]',
  error        TEXT,
  started_at   TEXT NOT NULL,
  finished_at  TEXT,
  PRIMARY KEY(owner_id, job_id, attempt),
  FOREIGN KEY(owner_id, job_id) REFERENCES render_jobs(owner_id, id),
  CHECK ((status IN ('done', 'failed', 'cancelled')) = (finished_at IS NOT NULL))
) STRICT;

CREATE INDEX render_attempts_active
ON render_attempts(owner_id, status, started_at, job_id);

CREATE TRIGGER render_attempts_no_delete BEFORE DELETE ON render_attempts BEGIN
  SELECT RAISE(ABORT, 'render_attempts is append-only');
END;

CREATE TRIGGER render_attempts_identity_no_update
BEFORE UPDATE OF owner_id, job_id, attempt, staging_path, started_at
ON render_attempts
BEGIN
  SELECT RAISE(ABORT, 'render attempt identity is immutable');
END;

CREATE TRIGGER render_attempts_terminal_no_update
BEFORE UPDATE ON render_attempts
WHEN OLD.status IN ('done', 'failed', 'cancelled')
BEGIN
  SELECT RAISE(ABORT, 'finished render attempts are immutable');
END;

CREATE TABLE render_outputs (
  owner_id         TEXT NOT NULL,
  id               TEXT NOT NULL,
  job_id           TEXT NOT NULL,
  attempt           INTEGER NOT NULL CHECK (attempt > 0),
  output_path       TEXT NOT NULL,
  output_sha256     TEXT NOT NULL,
  size_bytes        INTEGER NOT NULL CHECK (size_bytes >= 0),
  media_type        TEXT NOT NULL,
  width             INTEGER CHECK (width IS NULL OR width > 0),
  height            INTEGER CHECK (height IS NULL OR height > 0),
  duration_s        REAL CHECK (duration_s IS NULL OR duration_s > 0),
  verification_json TEXT NOT NULL,
  created_at        TEXT NOT NULL,
  PRIMARY KEY(owner_id, id),
  UNIQUE(owner_id, job_id),
  FOREIGN KEY(owner_id, job_id, attempt)
    REFERENCES render_attempts(owner_id, job_id, attempt)
) STRICT;

CREATE TABLE render_manifests (
  owner_id       TEXT NOT NULL,
  output_id      TEXT NOT NULL,
  manifest_path  TEXT NOT NULL,
  manifest_json  TEXT NOT NULL,
  manifest_sha256 TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  PRIMARY KEY(owner_id, output_id),
  FOREIGN KEY(owner_id, output_id) REFERENCES render_outputs(owner_id, id)
) STRICT;

CREATE TRIGGER render_outputs_no_update BEFORE UPDATE ON render_outputs BEGIN
  SELECT RAISE(ABORT, 'render_outputs is append-only');
END;

CREATE TRIGGER render_outputs_no_delete BEFORE DELETE ON render_outputs BEGIN
  SELECT RAISE(ABORT, 'render_outputs is append-only');
END;

CREATE TRIGGER render_manifests_no_update BEFORE UPDATE ON render_manifests BEGIN
  SELECT RAISE(ABORT, 'render_manifests is append-only');
END;

CREATE TRIGGER render_manifests_no_delete BEFORE DELETE ON render_manifests BEGIN
  SELECT RAISE(ABORT, 'render_manifests is append-only');
END;
