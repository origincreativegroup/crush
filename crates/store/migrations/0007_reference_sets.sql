-- Crush schema v7: curated previous-work reference sets and style-profile versioning.
--
-- Reference sets carry explicitly confirmed style evidence (docs/dam-feedback-blueprint.md,
-- "Previous work as style evidence"): a set is inert until the user confirms it, disabling
-- mutes it without deleting, and removal is a real delete whose items cascade. Style profiles
-- gain context scoping, the held-out evaluation gate (learned flag plus baseline comparison),
-- and reversible activation per (owner, context).

CREATE TABLE reference_sets (
  id            TEXT PRIMARY KEY,
  owner_id      TEXT NOT NULL REFERENCES owners(id),
  name          TEXT NOT NULL,
  context_key   TEXT NOT NULL,
  description   TEXT NOT NULL DEFAULT '',
  scope         TEXT NOT NULL CHECK (scope IN ('whole_set', 'selected')),
  -- 'unconfirmed' until the user explicitly confirms; 'disabled' mutes without deleting;
  -- removal is a real DELETE.
  status        TEXT NOT NULL DEFAULT 'unconfirmed'
                CHECK (status IN ('unconfirmed', 'confirmed', 'disabled')),
  source_collection_id TEXT,
  created_at    TEXT NOT NULL,
  confirmed_at  TEXT,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX reference_sets_owner_context ON reference_sets(owner_id, context_key);

CREATE TABLE reference_set_items (
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  set_id      TEXT NOT NULL,
  media_kind  TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id    TEXT NOT NULL,
  role        TEXT NOT NULL DEFAULT 'positive' CHECK (role IN ('positive', 'excluded')),
  added_at    TEXT NOT NULL,
  PRIMARY KEY(owner_id, set_id, media_kind, media_id),
  FOREIGN KEY(set_id, owner_id) REFERENCES reference_sets(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX reference_set_items_set ON reference_set_items(set_id);

-- Reference items must point at real media, mirroring the 0002 target-existence triggers.
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
  END;
END;

-- Media removal cannot leave dangling reference items (mirrors the 0002 cleanup triggers).
CREATE TRIGGER photo_reference_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM reference_set_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
END;

CREATE TRIGGER shot_reference_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM reference_set_items
  WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
END;

-- Style-profile extensions: context scoping plus the held-out evaluation gate.
ALTER TABLE style_profiles ADD COLUMN context_key TEXT NOT NULL DEFAULT 'default';
ALTER TABLE style_profiles ADD COLUMN baseline_metric REAL;
ALTER TABLE style_profiles ADD COLUMN metrics_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE style_profiles ADD COLUMN learned INTEGER NOT NULL DEFAULT 0 CHECK (learned IN (0, 1));
CREATE INDEX style_profiles_owner_context ON style_profiles(owner_id, context_key, version);

-- One active profile per (owner, context) replaces the owner-wide rule from 0002: a preference
-- in one context must not silently become a universal rule.
DROP INDEX style_profiles_one_active;
CREATE UNIQUE INDEX style_profiles_one_active_per_context
ON style_profiles(owner_id, context_key) WHERE active = 1;
