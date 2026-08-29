-- Crush schema v8: DAM organization — collections, version stacks, saved searches, and the
-- safety-flag write path's organizational tables (docs/dam-feedback-blueprint.md).
--
-- These tables carry current organizational state only. Every review *signal* still flows
-- through the append-only feedback_events table (0002/0005); nothing here mutates or deletes a
-- feedback row. Collections carry no training meaning until a reference set is explicitly
-- derived from one, and originals (photos/videos/shots rows) stay immutable: stacks and
-- collections are separate owner-scoped rows kept consistent by cleanup triggers.

CREATE TABLE collections (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX collections_owner ON collections(owner_id, name);

CREATE TABLE collection_items (
  owner_id      TEXT NOT NULL REFERENCES owners(id),
  collection_id TEXT NOT NULL,
  media_kind    TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id      TEXT NOT NULL,
  -- Optional per-item context key (e.g. 'homepage-hero'); NULL inherits the set/collection
  -- level. Feeds saved-search and reference-set context defaults.
  context_key   TEXT,
  -- Marks the item as a user-selected example for 'selected'-scope designation.
  marked        INTEGER NOT NULL DEFAULT 0 CHECK (marked IN (0, 1)),
  added_at      TEXT NOT NULL,
  PRIMARY KEY(owner_id, collection_id, media_kind, media_id),
  FOREIGN KEY(collection_id, owner_id) REFERENCES collections(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX collection_items_set ON collection_items(collection_id);

-- Collection items must point at real media, mirroring the 0002/0007 target-existence triggers.
CREATE TRIGGER collection_item_target_insert
BEFORE INSERT ON collection_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'collection item photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'collection item shot does not exist')
  END;
END;

-- Media removal cannot leave dangling collection items (mirrors the 0007 cleanup triggers).
CREATE TRIGGER photo_collection_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM collection_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER shot_collection_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM collection_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
END;

-- Version stacks: one original + derived/alternate versions. Purely organizational metadata;
-- no API mutates the underlying media rows (originals stay immutable).
CREATE TABLE version_stacks (
  id         TEXT PRIMARY KEY,
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE TABLE stack_items (
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  stack_id   TEXT NOT NULL,
  media_kind TEXT NOT NULL CHECK (media_kind IN ('photo', 'video')),
  media_id   TEXT NOT NULL,
  role       TEXT NOT NULL CHECK (role IN ('original', 'derived')),
  added_at   TEXT NOT NULL,
  PRIMARY KEY(owner_id, stack_id, media_kind, media_id),
  FOREIGN KEY(stack_id, owner_id) REFERENCES version_stacks(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX stack_items_stack ON stack_items(stack_id);

-- Exactly one original per stack; everything else is a derived/alternate version.
CREATE UNIQUE INDEX stack_one_original
ON stack_items(owner_id, stack_id) WHERE role = 'original';

-- Stack items must point at real media.
CREATE TRIGGER stack_item_target_insert
BEFORE INSERT ON stack_items
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'stack item photo does not exist')
    WHEN NEW.media_kind = 'video' AND NOT EXISTS (
      SELECT 1 FROM videos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'stack item video does not exist')
  END;
END;

-- Media removal cannot leave dangling stack items.
CREATE TRIGGER photo_stack_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM stack_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER video_stack_cleanup AFTER DELETE ON videos BEGIN
  DELETE FROM stack_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'video' AND media_id = OLD.id;
END;

CREATE TABLE saved_searches (
  id           TEXT PRIMARY KEY,
  owner_id     TEXT NOT NULL REFERENCES owners(id),
  name         TEXT NOT NULL,
  query        TEXT NOT NULL,
  context_key  TEXT NOT NULL DEFAULT 'default',
  filters_json TEXT NOT NULL DEFAULT '{}',   -- AssetFilter projection, validated as JSON object
  created_at   TEXT NOT NULL,
  UNIQUE(owner_id, name)
) STRICT;

-- Reference sets may designate a collection as their source (0007 reserved the column). SQLite
-- cannot ALTER TABLE ... ADD CONSTRAINT, so composite-FK validation is trigger-based, and a
-- deleted collection unsets the designation while its confirmed set keeps its materialized
-- items (removal stays reproducible from the remaining evidence).
CREATE TRIGGER reference_set_source_collection_insert
BEFORE INSERT ON reference_sets
BEGIN
  SELECT CASE
    WHEN NEW.source_collection_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM collections
      WHERE id = NEW.source_collection_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'reference set source collection does not exist')
  END;
END;

CREATE TRIGGER reference_set_source_collection_update
BEFORE UPDATE OF source_collection_id ON reference_sets
BEGIN
  SELECT CASE
    WHEN NEW.source_collection_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM collections
      WHERE id = NEW.source_collection_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'reference set source collection does not exist')
  END;
END;

CREATE TRIGGER collection_reference_unset AFTER DELETE ON collections BEGIN
  UPDATE reference_sets SET source_collection_id = NULL
  WHERE owner_id = OLD.owner_id AND source_collection_id = OLD.id;
END;
