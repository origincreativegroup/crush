-- Crush schema v12: imported span plan items clamp to the source video, not the span
-- (Task 037 — first-class spans, Reel Studio unification step 1).
--
-- Why: an imported span's boundaries are the catalogue's clip — the item's *default*,
-- not a physical limit. The source video is in the library, so a span plan item's In/Out
-- must be adjustable anywhere inside the video (0..duration_s). Migration 0011's
-- plan_item_boundaries_insert/_update enforced the frozen-container clamp in SQL, so a
-- store-side loosening alone was defeated by the database; the span arms are rebuilt here
-- to validate against the span's source video (manual_spans → videos) instead, with the
-- same +0.001 s probing slack the manual_span_bounds_* triggers use. When the video's
-- duration_s is NULL the SQL upper bound cannot be evaluated; the store API refuses span
-- item edits for unknown-duration videos, and the trigger still enforces a non-negative,
-- non-empty interval. The shot arm is re-created byte-identically: shot items stay clamped
-- to their source shot (the approved render paths). No data is copied or rewritten.
--
-- Rollback: restore migration 0011's trigger bodies (span arms clamped to
-- manual_spans.start_s/end_s). Rows whose boundaries were extended past the span while on
-- v12 would then violate the restored triggers on their next boundary UPDATE; they render
-- correctly until edited.

DROP TRIGGER plan_item_boundaries_insert;
DROP TRIGGER plan_item_boundaries_update;

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
      NEW.start_s < 0
      OR NEW.end_s > (SELECT duration_s FROM videos WHERE id = (
           SELECT video_id FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id
         ) AND owner_id = NEW.owner_id) + 0.001
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the source video')
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
      NEW.start_s < 0
      OR NEW.end_s > (SELECT duration_s FROM videos WHERE id = (
           SELECT video_id FROM manual_spans WHERE id = NEW.media_id AND owner_id = NEW.owner_id
         ) AND owner_id = NEW.owner_id) + 0.001
      OR NEW.end_s <= NEW.start_s
    ) THEN RAISE(ABORT, 'plan item boundaries must stay inside the source video')
  END;
END;
