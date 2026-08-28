-- Crush schema v3: non-destructive source metadata and deterministic working proxies.

CREATE TABLE photo_source_metadata (
  photo_id                    TEXT PRIMARY KEY,
  owner_id                    TEXT NOT NULL REFERENCES owners(id),
  source_format               TEXT NOT NULL,
  decoder                     TEXT NOT NULL,
  proxy_rel                   TEXT,
  proxy_width                 INTEGER CHECK (proxy_width > 0),
  proxy_height                INTEGER CHECK (proxy_height > 0),
  proxy_sha256                TEXT,
  proxy_provenance            TEXT NOT NULL CHECK (proxy_provenance IN (
                                'decoded_original', 'full_render', 'embedded_preview'
                              )),
  orientation_applied         INTEGER NOT NULL CHECK (orientation_applied IN (0, 1)),
  bit_depth                   INTEGER CHECK (bit_depth > 0),
  color_space                 TEXT,
  icc_profile_name            TEXT,
  icc_profile_sha256          TEXT,
  exposure_json               TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(exposure_json)),
  gps_present                 INTEGER NOT NULL DEFAULT 0 CHECK (gps_present IN (0, 1)),
  metadata_json               TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  original_size_bytes         INTEGER NOT NULL CHECK (original_size_bytes >= 0),
  extracted_at                TEXT NOT NULL,
  CHECK ((proxy_rel IS NULL) = (proxy_sha256 IS NULL)),
  CHECK ((proxy_rel IS NULL) = (proxy_width IS NULL)),
  CHECK ((proxy_rel IS NULL) = (proxy_height IS NULL)),
  FOREIGN KEY(photo_id, owner_id) REFERENCES photos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX photo_source_metadata_owner ON photo_source_metadata(owner_id, photo_id);

CREATE TABLE video_source_metadata (
  video_id             TEXT PRIMARY KEY,
  owner_id             TEXT NOT NULL REFERENCES owners(id),
  container            TEXT NOT NULL,
  video_codec          TEXT NOT NULL,
  codec_profile        TEXT,
  pixel_format         TEXT,
  bit_depth            INTEGER CHECK (bit_depth > 0),
  color_space          TEXT,
  color_primaries      TEXT,
  color_transfer       TEXT,
  color_range          TEXT,
  rotation             INTEGER,
  proxy_rel            TEXT,
  proxy_sha256         TEXT,
  proxy_required       INTEGER NOT NULL CHECK (proxy_required IN (0, 1)),
  proxy_reason         TEXT,
  original_size_bytes  INTEGER NOT NULL CHECK (original_size_bytes >= 0),
  metadata_json        TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  probed_at            TEXT NOT NULL,
  CHECK ((proxy_rel IS NULL) = (proxy_sha256 IS NULL)),
  CHECK (proxy_required = 1 OR proxy_reason IS NULL),
  FOREIGN KEY(video_id, owner_id) REFERENCES videos(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX video_source_metadata_owner ON video_source_metadata(owner_id, video_id);
