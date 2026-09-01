-- Crush schema v11: Reel Studio historical-evidence import (Task 022).
--
-- Three additions, all owner-scoped:
--   * manual_spans      — first-class imported/manual video spans. A Reel Studio segment is a human
--                         decision about where a shot starts and ends on the ORIGINAL source; it may
--                         cross Crush's auto-detected scene cuts. Spans reference videos, never shots,
--                         so resplit/re-index (which rebuilds shots) cannot erase them. Catalogue
--                         evidence (quality/standout/usable/safety/tags/used_in) lives on the span row
--                         as historical evidence, separate from the user's own Crush annotations.
--   * catalogue_imports — an append-only ledger of dry-run and applied imports for idempotency.
--   * plan_items        — rebuilt to admit media_kind 'span' and the honest origins 'historical'
--                         (a prior human choice reproduced from an imported recipe) and 'imported'
--                         (a catalogue-driven selection). Both carry NULL profile_version and a
--                         provenance_json describing source, external id and boundary basis. They are
--                         never labelled general or personal.

CREATE TABLE manual_spans (
  id                          TEXT PRIMARY KEY,
  owner_id                    TEXT NOT NULL REFERENCES owners(id),
  video_id                    TEXT NOT NULL,
  source                      TEXT NOT NULL CHECK (source IN ('reel_studio', 'manual')),
  external_id                 TEXT NOT NULL,
  start_s                     REAL NOT NULL CHECK (start_s >= 0),
  end_s                       REAL NOT NULL CHECK (end_s > start_s),
  -- How start_s/end_s were established. 'catalogue_tc' = catalogue tc_in/tc_out taken literally;
  -- 'library_probe' = corrected from the measured library clip; 'user' = set in Crush.
  boundary_basis              TEXT NOT NULL CHECK (boundary_basis IN ('catalogue_tc', 'library_probe', 'user')),
  boundary_tolerance_s        REAL NOT NULL DEFAULT 0.0 CHECK (boundary_tolerance_s >= 0),
  -- Offset that converts library-clip-relative seconds (recipe in/out, crop_kf t, cover time)
  -- to original-source seconds: source_t = start_s + library_relative_offset_s + t.
  library_relative_offset_s   REAL NOT NULL DEFAULT 0.0,
  description                 TEXT NOT NULL DEFAULT '',
  shot_type                   TEXT NOT NULL DEFAULT '',
  camera_move                 TEXT NOT NULL DEFAULT '',
  subjects                    TEXT NOT NULL DEFAULT '',
  action                      TEXT NOT NULL DEFAULT '',
  tags                        TEXT NOT NULL DEFAULT '',
  quality                     INTEGER CHECK (quality IS NULL OR quality BETWEEN 1 AND 5),
  standout                    INTEGER NOT NULL DEFAULT 0 CHECK (standout IN (0, 1)),
  usable                      INTEGER NOT NULL DEFAULT 1 CHECK (usable IN (0, 1)),
  faces_visible               INTEGER NOT NULL DEFAULT 0 CHECK (faces_visible IN (0, 1)),
  nametags_visible            INTEGER NOT NULL DEFAULT 0 CHECK (nametags_visible IN (0, 1)),
  blur_required               INTEGER NOT NULL DEFAULT 0 CHECK (blur_required IN (0, 1)),
  used_in                     TEXT NOT NULL DEFAULT '',
  crop_x                      REAL CHECK (crop_x IS NULL OR crop_x BETWEEN 0.0 AND 1.0),
  notes                       TEXT NOT NULL DEFAULT '',
  import_id                   TEXT,
  imported_at                 TEXT NOT NULL,
  updated_at                  TEXT NOT NULL,
  UNIQUE(owner_id, source, external_id),
  UNIQUE(id, owner_id),
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX manual_spans_video ON manual_spans(owner_id, video_id, start_s);
CREATE INDEX manual_spans_import ON manual_spans(owner_id, import_id);

-- Spans must lie inside their source video when its duration is known.
CREATE TRIGGER manual_span_bounds_insert
BEFORE INSERT ON manual_spans
BEGIN
  SELECT CASE
    WHEN NOT EXISTS (
      SELECT 1 FROM videos WHERE id = NEW.video_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'manual span video does not exist')
    WHEN NEW.end_s > (SELECT duration_s FROM videos WHERE id = NEW.video_id AND owner_id = NEW.owner_id) + 0.001
      THEN RAISE(ABORT, 'manual span exceeds the source video duration')
  END;
END;

CREATE TRIGGER manual_span_bounds_update
BEFORE UPDATE OF start_s, end_s, video_id ON manual_spans
BEGIN
  SELECT CASE
    WHEN NOT EXISTS (
      SELECT 1 FROM videos WHERE id = NEW.video_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'manual span video does not exist')
    WHEN NEW.end_s > (SELECT duration_s FROM videos WHERE id = NEW.video_id AND owner_id = NEW.owner_id) + 0.001
      THEN RAISE(ABORT, 'manual span exceeds the source video duration')
  END;
END;

CREATE TABLE catalogue_imports (
  id               TEXT PRIMARY KEY,
  owner_id         TEXT NOT NULL REFERENCES owners(id),
  source           TEXT NOT NULL CHECK (source IN ('reel_studio')),
  mode             TEXT NOT NULL CHECK (mode IN ('dry_run', 'apply')),
  catalogue_path   TEXT NOT NULL,
  catalogue_sha256 TEXT NOT NULL,
  recipes_json     TEXT NOT NULL DEFAULT '[]',
  report_json      TEXT NOT NULL,
  started_at       TEXT NOT NULL,
  finished_at      TEXT NOT NULL
) STRICT;

CREATE INDEX catalogue_imports_owner ON catalogue_imports(owner_id, started_at);

CREATE TRIGGER catalogue_imports_no_update
BEFORE UPDATE ON catalogue_imports
BEGIN
  SELECT RAISE(ABORT, 'catalogue_imports is append-only');
END;

-- Rebuild plan_items (SQLite cannot widen a CHECK in place). Triggers on plan_items are recreated
-- below; the photo/shot cleanup triggers on photos/shots (0009) keep working unchanged.
CREATE TABLE plan_items_v11 (
  owner_id        TEXT NOT NULL REFERENCES owners(id),
  plan_id         TEXT NOT NULL,
  media_kind      TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot', 'span')),
  media_id        TEXT NOT NULL,
  position        INTEGER NOT NULL CHECK (position >= 0),
  start_s         REAL,
  end_s           REAL,
  pacing          REAL CHECK (pacing IS NULL OR pacing BETWEEN 0.0 AND 1.0),
  crop_x          REAL CHECK (crop_x IS NULL OR crop_x BETWEEN 0.0 AND 1.0),
  grade_json      TEXT,
  reason          TEXT NOT NULL DEFAULT '',
  signals_json    TEXT NOT NULL DEFAULT '{}',
  origin          TEXT NOT NULL CHECK (origin IN ('general', 'personal', 'historical', 'imported')),
  rank            REAL,
  profile_version INTEGER CHECK (profile_version IS NULL OR profile_version > 0),
  -- {source, external_id, import_id, boundary_basis, boundary_tolerance_s} for historical/imported.
  provenance_json TEXT NOT NULL DEFAULT '{}',
  added_at        TEXT NOT NULL,
  PRIMARY KEY(owner_id, plan_id, media_kind, media_id),
  FOREIGN KEY(plan_id, owner_id) REFERENCES plans(id, owner_id) ON DELETE CASCADE,
  CHECK ((origin = 'personal') = (profile_version IS NOT NULL)),
  CHECK ((media_kind IN ('shot', 'span')) = (start_s IS NOT NULL)),
  CHECK ((media_kind IN ('shot', 'span')) = (end_s IS NOT NULL)),
  CHECK (origin IN ('general', 'personal') OR provenance_json <> '{}')
) STRICT;

INSERT INTO plan_items_v11 (
  owner_id, plan_id, media_kind, media_id, position, start_s, end_s, pacing, crop_x, grade_json,
  reason, signals_json, origin, rank, profile_version, provenance_json, added_at
)
SELECT owner_id, plan_id, media_kind, media_id, position, start_s, end_s, pacing, crop_x, grade_json,
  reason, signals_json, origin, rank, profile_version, '{}', added_at
FROM plan_items;

-- The 0009 cleanup triggers on photos/shots name plan_items; drop them around the rebuild.
DROP TRIGGER photo_plan_cleanup;
DROP TRIGGER shot_plan_cleanup;
DROP TABLE plan_items;
ALTER TABLE plan_items_v11 RENAME TO plan_items;

CREATE TRIGGER photo_plan_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM plan_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER shot_plan_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM plan_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
END;

CREATE UNIQUE INDEX plan_items_position ON plan_items(owner_id, plan_id, position);

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
    WHEN NEW.media_kind = 'span' AND NOT EXISTS (
      SELECT 1 FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item span does not exist')
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
    WHEN NEW.media_kind = 'span' AND NOT EXISTS (
      SELECT 1 FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'plan item span does not exist')
  END;
END;

CREATE TRIGGER plan_item_boundaries_insert
BEFORE INSERT ON plan_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'shot' AND (
      NEW.start_s < (SELECT start_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s > (SELECT end_s FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the source shot')
    WHEN NEW.media_kind = 'span' AND (
      NEW.start_s < (SELECT start_s FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s > (SELECT end_s FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the imported span')
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
    WHEN NEW.media_kind = 'span' AND (
      NEW.start_s < (SELECT start_s FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s > (SELECT end_s FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id)
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the imported span')
  END;
END;

CREATE TRIGGER span_plan_cleanup AFTER DELETE ON manual_spans BEGIN
  DELETE FROM plan_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'span' AND media_id = OLD.id;
END;
