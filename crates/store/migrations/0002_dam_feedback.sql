-- Crush schema v2: photo assets, editorial judgment, feedback, and personal style models.

CREATE TABLE photos (
  id           TEXT PRIMARY KEY,
  owner_id     TEXT NOT NULL REFERENCES owners(id),
  path         TEXT NOT NULL,
  sha256       TEXT NOT NULL,
  width        INTEGER NOT NULL CHECK (width > 0),
  height       INTEGER NOT NULL CHECK (height > 0),
  format       TEXT NOT NULL,
  orientation  INTEGER CHECK (orientation BETWEEN 1 AND 8),
  captured_at  TEXT,
  camera_make  TEXT,
  camera_model TEXT,
  lens         TEXT,
  thumb_rel    TEXT,
  status       TEXT NOT NULL DEFAULT 'pending'
               CHECK (status IN ('pending', 'embedded', 'done', 'failed')),
  indexed_at   TEXT,
  UNIQUE(owner_id, sha256),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX photos_owner_path ON photos(owner_id, path);

CREATE TABLE photo_vectors (
  photo_id  TEXT PRIMARY KEY,
  owner_id  TEXT NOT NULL REFERENCES owners(id),
  dim       INTEGER NOT NULL CHECK (dim > 0),
  vec       BLOB NOT NULL,
  FOREIGN KEY(photo_id, owner_id) REFERENCES photos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX photo_vectors_owner ON photo_vectors(owner_id, photo_id);

-- Carries the useful editorial language of Reel Studio without tying the DAM to one archive.
CREATE TABLE editorial_annotations (
  owner_id         TEXT NOT NULL REFERENCES owners(id),
  media_kind       TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id         TEXT NOT NULL,
  description      TEXT NOT NULL DEFAULT '',
  subjects         TEXT NOT NULL DEFAULT '',
  action           TEXT NOT NULL DEFAULT '',
  tags              TEXT NOT NULL DEFAULT '',
  quality           INTEGER CHECK (quality BETWEEN 1 AND 5),
  standout          INTEGER NOT NULL DEFAULT 0 CHECK (standout IN (0, 1)),
  usable            INTEGER NOT NULL DEFAULT 1 CHECK (usable IN (0, 1)),
  faces_visible     INTEGER NOT NULL DEFAULT 0 CHECK (faces_visible IN (0, 1)),
  nametags_visible  INTEGER NOT NULL DEFAULT 0 CHECK (nametags_visible IN (0, 1)),
  blur_required     INTEGER NOT NULL DEFAULT 0 CHECK (blur_required IN (0, 1)),
  crop_x            REAL CHECK (crop_x BETWEEN 0.0 AND 1.0),
  grade_json        TEXT,
  notes             TEXT NOT NULL DEFAULT '',
  updated_at        TEXT NOT NULL,
  PRIMARY KEY(owner_id, media_kind, media_id)
) STRICT;

CREATE INDEX editorial_annotations_media ON editorial_annotations(media_kind, media_id);

CREATE TABLE aesthetic_assessments (
  owner_id          TEXT NOT NULL REFERENCES owners(id),
  media_kind        TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id          TEXT NOT NULL,
  sharpness         REAL NOT NULL CHECK (sharpness BETWEEN 0.0 AND 1.0),
  exposure          REAL NOT NULL CHECK (exposure BETWEEN 0.0 AND 1.0),
  contrast          REAL NOT NULL CHECK (contrast BETWEEN 0.0 AND 1.0),
  color_harmony     REAL NOT NULL CHECK (color_harmony BETWEEN 0.0 AND 1.0),
  balance           REAL NOT NULL CHECK (balance BETWEEN 0.0 AND 1.0),
  subject_placement REAL NOT NULL CHECK (subject_placement BETWEEN 0.0 AND 1.0),
  negative_space    REAL NOT NULL CHECK (negative_space BETWEEN 0.0 AND 1.0),
  visual_clarity    REAL NOT NULL CHECK (visual_clarity BETWEEN 0.0 AND 1.0),
  overall           REAL NOT NULL CHECK (overall BETWEEN 0.0 AND 1.0),
  confidence        REAL NOT NULL CHECK (confidence BETWEEN 0.0 AND 1.0),
  explanation_json TEXT NOT NULL,
  model_version     TEXT NOT NULL,
  assessed_at       TEXT NOT NULL,
  PRIMARY KEY(owner_id, media_kind, media_id)
) STRICT;

CREATE TABLE feedback_events (
  id                 TEXT PRIMARY KEY,
  owner_id           TEXT NOT NULL REFERENCES owners(id),
  media_kind         TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id           TEXT NOT NULL,
  signal             TEXT NOT NULL CHECK (signal IN (
                       'pick', 'reject', 'rating', 'prefer', 'crop', 'grade', 'export',
                       'publish', 'tag', 'edit'
                     )),
  value              REAL,
  compared_media_kind TEXT CHECK (compared_media_kind IN ('photo', 'shot')),
  compared_media_id  TEXT,
  context_json       TEXT NOT NULL DEFAULT '{}',
  created_at         TEXT NOT NULL,
  CHECK ((compared_media_kind IS NULL) = (compared_media_id IS NULL)),
  CHECK (signal = 'prefer' OR compared_media_id IS NULL)
) STRICT;

CREATE INDEX feedback_events_owner_time ON feedback_events(owner_id, created_at, id);
CREATE INDEX feedback_events_media ON feedback_events(owner_id, media_kind, media_id);

CREATE TABLE style_profiles (
  id                   TEXT PRIMARY KEY,
  owner_id             TEXT NOT NULL REFERENCES owners(id),
  name                 TEXT NOT NULL,
  version              INTEGER NOT NULL CHECK (version > 0),
  algorithm_version    TEXT NOT NULL,
  embedding_dim        INTEGER NOT NULL CHECK (embedding_dim > 0),
  embedding_weights    BLOB NOT NULL,
  feature_weights_json TEXT NOT NULL,
  sample_count         INTEGER NOT NULL CHECK (sample_count >= 0),
  held_out_metric      REAL,
  active               INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0, 1)),
  trained_at           TEXT NOT NULL,
  UNIQUE(id, owner_id),
  UNIQUE(owner_id, name, version)
) STRICT;

CREATE UNIQUE INDEX style_profiles_one_active
ON style_profiles(owner_id) WHERE active = 1;

-- Polymorphic media references stay strongly checked without forcing photos and shots into one
-- table before the existing video pipeline is migrated.
CREATE TRIGGER editorial_annotation_target_insert
BEFORE INSERT ON editorial_annotations
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'editorial annotation photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'editorial annotation shot does not exist')
  END;
END;

CREATE TRIGGER aesthetic_assessment_target_insert
BEFORE INSERT ON aesthetic_assessments
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'aesthetic assessment photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'aesthetic assessment shot does not exist')
  END;
END;

CREATE TRIGGER feedback_event_target_insert
BEFORE INSERT ON feedback_events
BEGIN
  SELECT CASE
    WHEN NEW.media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'feedback photo does not exist')
    WHEN NEW.media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'feedback shot does not exist')
    WHEN NEW.compared_media_kind = 'photo' AND NOT EXISTS (
      SELECT 1 FROM photos WHERE id = NEW.compared_media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'compared feedback photo does not exist')
    WHEN NEW.compared_media_kind = 'shot' AND NOT EXISTS (
      SELECT 1 FROM shots WHERE id = NEW.compared_media_id AND owner_id = NEW.owner_id
    ) THEN RAISE(ABORT, 'compared feedback shot does not exist')
  END;
END;

CREATE TRIGGER photo_editorial_cleanup AFTER DELETE ON photos BEGIN
  DELETE FROM editorial_annotations WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
  DELETE FROM aesthetic_assessments WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
  DELETE FROM feedback_events WHERE owner_id = OLD.owner_id AND media_kind = 'photo' AND media_id = OLD.id;
  DELETE FROM feedback_events WHERE owner_id = OLD.owner_id AND compared_media_kind = 'photo' AND compared_media_id = OLD.id;
END;

CREATE TRIGGER shot_editorial_cleanup AFTER DELETE ON shots BEGIN
  DELETE FROM editorial_annotations WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
  DELETE FROM aesthetic_assessments WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
  DELETE FROM feedback_events WHERE owner_id = OLD.owner_id AND media_kind = 'shot' AND media_id = OLD.id;
  DELETE FROM feedback_events WHERE owner_id = OLD.owner_id AND compared_media_kind = 'shot' AND compared_media_id = OLD.id;
END;
