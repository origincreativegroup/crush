-- Crush schema v13: catalogue unification — span text joins search, spans enter Review,
-- and imported evidence becomes preference evidence only through the explicit confirmation
-- flow (Task 034, Reel Studio unification step 2).
--
-- WHY span confirmation is a reference set and NOT a feedback_events row (the recorded
-- schema decision, ratified in .tasks/backlog/TASK-034-impl-plan.md): span evidence becomes
-- preference evidence only through the user's explicit confirmation, and confirmation must
-- be reversible. feedback_events is append-only (0005) and therefore cannot be the vehicle —
-- a confirmed-then-withdrawn signal could never be withdrawn. Confirmed imported evidence is
-- instead a named previous-work reference set whose items are the spans themselves;
-- reference_set_items admits media_kind 'span' so the evidence keeps its true identity and
-- provenance instead of being mapped onto whatever shots happen to overlap the interval
-- today (a mapping that both fabricates evidence location and silently evaporates when a
-- resplit rebuilds shots — the cleanup triggers delete shot-keyed rows). The existing
-- confirm/disable/delete withdrawal machinery (with its transactional profile
-- invalidation) then covers reversibility with no new withdrawal code. feedback_events
-- stays photo/shot: direct span pick/rate signals are deferred until span interval analysis
-- exists, because the trainer can only consume media with vectors — and spans have none, so
-- confirmed span evidence is catalogued but inert for the current learner (disclosed in the
-- UI; never implied to train).
--
-- Three additions, all owner-scoped:
--   * reference_set_items rebuilt with media_kind 'span' admitted. The photo/shot cleanup
--     triggers on photos/shots name this table, so they are dropped around the rebuild and
--     re-created (the 0011 pattern); a new span_reference_cleanup trigger mirrors them for
--     manual_spans. The target-existence trigger gains a manual_spans arm, so a reference
--     item can only ever point at a span that really exists for its owner.
--   * manual_spans_fts — an FTS5 index over the span catalogue text (description, subjects,
--     action, tags, shot_type, camera_move), external content on manual_spans. Span text
--     becomes text-MATCH-ONLY search evidence: spans have no embedding vectors, so span
--     results never join the cosine ranking — they are bm25 text hits beside the semantic
--     results, plainly labeled. Three triggers keep the index in sync with every span
--     write, including the foreign-key cascade from videos, so the index can never drift
--     from the span rows.
--   * No change to feedback_events, editorial_annotations, or aesthetic_assessments: they
--     stay photo/shot, and span catalogue evidence keeps living on the span row.
--
-- Rollback: drop manual_spans_fts and its triggers; rebuild reference_set_items with the
-- 0007 CHECK (photo/shot only) after deleting its 'span' rows. Reference sets that contained
-- span items would lose those items; photo/shot evidence is unaffected.

-- 1. Rebuild reference_set_items with 'span' admitted (SQLite cannot widen a CHECK in place).
DROP TRIGGER reference_set_item_target_insert;
DROP TRIGGER photo_reference_cleanup;
DROP TRIGGER shot_reference_cleanup;

CREATE TABLE reference_set_items_v13 (
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  set_id      TEXT NOT NULL,
  media_kind  TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot', 'span')),
  media_id    TEXT NOT NULL,
  role        TEXT NOT NULL DEFAULT 'positive' CHECK (role IN ('positive', 'excluded')),
  added_at    TEXT NOT NULL,
  PRIMARY KEY(owner_id, set_id, media_kind, media_id),
  FOREIGN KEY(set_id, owner_id) REFERENCES reference_sets(id, owner_id) ON DELETE CASCADE
) STRICT;

INSERT INTO reference_set_items_v13 (owner_id, set_id, media_kind, media_id, role, added_at)
SELECT owner_id, set_id, media_kind, media_id, role, added_at FROM reference_set_items;

DROP TABLE reference_set_items;
ALTER TABLE reference_set_items_v13 RENAME TO reference_set_items;

CREATE INDEX reference_set_items_set ON reference_set_items(set_id);

CREATE TRIGGER reference_set_item_target_insert
BEFORE INSERT ON reference_set_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'reference set item photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'reference set item shot does not exist')
    WHEN NEW.media_kind = 'span' AND NOT EXISTS (
      SELECT 1 FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'reference set item span does not exist')
  END;
END;

CREATE TRIGGER photo_reference_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM reference_set_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER shot_reference_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM reference_set_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
END;

CREATE TRIGGER span_reference_cleanup AFTER DELETE ON manual_spans BEGIN
  DELETE FROM reference_set_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'span' AND media_id = OLD.id;
END;

-- 2. Span catalogue text joins the search index (text-match only; spans carry no vectors).
CREATE VIRTUAL TABLE manual_spans_fts USING fts5(
  description,
  subjects,
  action,
  tags,
  shot_type,
  camera_move,
  content='manual_spans',
  content_rowid='rowid'
);

-- Backfill every span that already exists (v11/v12 imports keep their catalogue text).
INSERT INTO manual_spans_fts (rowid, description, subjects, action, tags, shot_type, camera_move)
SELECT rowid, description, subjects, action, tags, shot_type, camera_move FROM manual_spans;

-- The three sync triggers below are the FTS5-documented external-content pattern; they keep
-- the index true through the store API AND through the videos → manual_spans ON DELETE
-- CASCADE, where no Rust code runs.
CREATE TRIGGER manual_spans_fts_insert
AFTER INSERT ON manual_spans BEGIN
  INSERT INTO manual_spans_fts(rowid, description, subjects, action, tags, shot_type, camera_move)
  VALUES (new.rowid, new.description, new.subjects, new.action, new.tags, new.shot_type,
          new.camera_move);
END;

CREATE TRIGGER manual_spans_fts_delete
AFTER DELETE ON manual_spans BEGIN
  INSERT INTO manual_spans_fts(manual_spans_fts, rowid, description, subjects, action, tags,
                               shot_type, camera_move)
  VALUES ('delete', old.rowid, old.description, old.subjects, old.action, old.tags,
          old.shot_type, old.camera_move);
END;

CREATE TRIGGER manual_spans_fts_update
AFTER UPDATE OF description, subjects, action, tags, shot_type, camera_move ON manual_spans BEGIN
  INSERT INTO manual_spans_fts(manual_spans_fts, rowid, description, subjects, action, tags,
                               shot_type, camera_move)
  VALUES ('delete', old.rowid, old.description, old.subjects, old.action, old.tags,
          old.shot_type, old.camera_move);
  INSERT INTO manual_spans_fts(rowid, description, subjects, action, tags, shot_type, camera_move)
  VALUES (new.rowid, new.description, new.subjects, new.action, new.tags, new.shot_type,
          new.camera_move);
END;
