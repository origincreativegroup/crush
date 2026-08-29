-- Crush schema v9: editorial plans (Task 020a).
--
-- Plans are editable documents that record which assets were selected for a deliverable, in
-- what order, and why. They are normal mutable state: plan writes are state APIs, never
-- feedback signals — feedback_events (0002/0005) stays the only training evidence and remains
-- append-only. plan_revisions are append-only snapshots a plan can be reopened from.
--
-- Provenance columns make the baseline/personalized distinction explicit at the data level:
-- an item ranked by the cold-start general strong-shot model carries origin 'general' and no
-- profile version; an item ranked under a personal style profile carries origin 'personal'
-- plus the exact profile version that produced its rank, and freezes the explainability
-- signals (signals_json) observed at selection time.

CREATE TABLE plans (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  context_key TEXT NOT NULL DEFAULT 'default',
  brief       TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX plans_owner ON plans(owner_id, created_at);

CREATE TABLE plan_items (
  owner_id        TEXT NOT NULL REFERENCES owners(id),
  plan_id         TEXT NOT NULL,
  media_kind      TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id        TEXT NOT NULL,
  position        INTEGER NOT NULL CHECK (position >= 0),
  -- Editable clip in/out points for shots; they must stay inside the source shot interval
  -- (plan_item_boundaries triggers below). Photos carry none.
  start_s         REAL,
  end_s           REAL,
  pacing          REAL CHECK (pacing IS NULL OR pacing BETWEEN 0.0 AND 1.0),
  crop_x          REAL CHECK (crop_x IS NULL OR crop_x BETWEEN 0.0 AND 1.0),
  grade_json      TEXT,
  reason          TEXT NOT NULL DEFAULT '',
  -- Frozen explainability evidence at selection time: the score-breakdown components and
  -- general-versus-personal signals the UI showed when the item was chosen.
  signals_json    TEXT NOT NULL DEFAULT '{}',
  -- Provenance: 'general' = cold-start strong-shot ranking, 'personal' = ranked under the
  -- style profile version recorded in profile_version.
  origin          TEXT NOT NULL CHECK (origin IN ('general', 'personal')),
  rank            REAL,
  profile_version INTEGER CHECK (profile_version IS NULL OR profile_version > 0),
  added_at        TEXT NOT NULL,
  PRIMARY KEY(owner_id, plan_id, media_kind, media_id),
  FOREIGN KEY(plan_id, owner_id) REFERENCES plans(id, owner_id) ON DELETE CASCADE,
  CHECK ((origin = 'personal') = (profile_version IS NOT NULL)),
  CHECK ((media_kind = 'shot') = (start_s IS NOT NULL)),
  CHECK ((media_kind = 'shot') = (end_s IS NOT NULL))
) STRICT;

-- Positions are dense and unique within a plan; the store APIs keep them 0..n.
CREATE UNIQUE INDEX plan_items_position ON plan_items(owner_id, plan_id, position);

CREATE INDEX plans_items_updated ON plans(owner_id, updated_at);

-- Items must point at real media (mirrors the 0002/0008 target-existence triggers).
CREATE TRIGGER plan_item_target_insert
BEFORE INSERT ON plan_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item shot does not exist')
  END;
END;

CREATE TRIGGER plan_item_target_update
BEFORE UPDATE OF media_kind, media_id ON plan_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item shot does not exist')
  END;
END;

-- Boundary safety for shots: editable in/out points must stay inside the source shot interval
-- and keep end after start. With a missing shot the comparisons are NULL and this trigger
-- passes; the target-existence trigger aborts instead.
CREATE TRIGGER plan_item_boundaries_insert
BEFORE INSERT ON plan_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'shot' AND (
      NEW.start_s < (SELECT start_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s > (SELECT end_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the source shot')
  END;
END;

CREATE TRIGGER plan_item_boundaries_update
BEFORE UPDATE OF start_s, end_s, media_kind, media_id ON plan_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'shot' AND (
      NEW.start_s < (SELECT start_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s > (SELECT end_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the source shot')
  END;
END;

-- Media removal cannot leave dangling plan items (mirrors the 0002/0008 cleanup triggers).
CREATE TRIGGER photo_plan_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM plan_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER shot_plan_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM plan_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
END;

-- Append-only revision history for every plan. Snapshots freeze while the plan exists; the
-- delete guard permits FK-cascade removal once the plan row itself is already gone.
CREATE TABLE plan_revisions (
  owner_id      TEXT NOT NULL REFERENCES owners(id),
  plan_id       TEXT NOT NULL,
  revision      INTEGER NOT NULL CHECK (revision > 0),
  label         TEXT NOT NULL DEFAULT '',
  snapshot_json TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  PRIMARY KEY(owner_id, plan_id, revision),
  FOREIGN KEY(plan_id, owner_id) REFERENCES plans(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE TRIGGER plan_revisions_no_update
BEFORE UPDATE ON plan_revisions
BEGIN
  SELECT RAISE(ABORT, 'plan_revisions is append-only');
END;

CREATE TRIGGER plan_revisions_no_delete
BEFORE DELETE ON plan_revisions
BEGIN
  SELECT CASE
    WHEN EXISTS (
      SELECT 1 FROM plans WHERE id = OLD.plan_id AND owner_id = OLD.owner_id
    ) THEN RAISE(ABORT, 'plan_revisions is append-only')
  END;
END;
