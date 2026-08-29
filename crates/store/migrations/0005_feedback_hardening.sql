-- Crush schema v5: append-only feedback at the schema level.
--
-- docs/dam-feedback-blueprint.md declares the feedback store append-only. Two rules remain
-- enforced only by append_feedback because adding CHECK constraints to feedback_events would
-- require a table rebuild (feedback_events_owner_time and feedback_events_media index the
-- table and four existing triggers reference it): the rating 1..5 range and JSON-object
-- context_json. Every new write goes through the API, and the no-update trigger below freezes
-- rows that predate those API checks.

CREATE TRIGGER feedback_events_no_update
BEFORE UPDATE ON feedback_events
BEGIN
  SELECT RAISE(ABORT, 'feedback_events is append-only');
END;

-- photo_editorial_cleanup / shot_editorial_cleanup (0002_dam_feedback.sql) delete feedback
-- rows after their media row is already gone, including through FK-cascade chains such as
-- delete_video_cascade (videos -> shots -> shot_editorial_cleanup). Abort only when every
-- media row referenced by OLD still exists; permit when any referenced target is missing so
-- cleanup-trigger and orphan deletes pass, including the mixed photo/shot-comparison case
-- where one referenced asset survives the other.
CREATE TRIGGER feedback_events_no_delete
BEFORE DELETE ON feedback_events
BEGIN
  SELECT CASE
    WHEN (
      (OLD.media_kind = 'photo' AND EXISTS (
        SELECT 1 FROM photos WHERE id = OLD.media_id AND owner_id = OLD.owner_id
      ))
      OR (OLD.media_kind = 'shot' AND EXISTS (
        SELECT 1 FROM shots WHERE id = OLD.media_id AND owner_id = OLD.owner_id
      ))
    ) AND (
      OLD.compared_media_id IS NULL
      OR (OLD.compared_media_kind = 'photo' AND EXISTS (
        SELECT 1 FROM photos WHERE id = OLD.compared_media_id AND owner_id = OLD.owner_id
      ))
      OR (OLD.compared_media_kind = 'shot' AND EXISTS (
        SELECT 1 FROM shots WHERE id = OLD.compared_media_id AND owner_id = OLD.owner_id
      ))
    ) THEN RAISE(ABORT, 'feedback_events is append-only')
  END;
END;
