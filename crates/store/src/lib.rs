//! SQLite persistence for Crush.
//!
//! This crate is the only place in the product that contains SQL. It owns migrations, typed
//! records, vector serialization, FTS synchronization, job history, and deep integrity checks.

use std::{
    convert::TryFrom,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, Utc};
use crush_core::{job::JobRecord, job::JobStatus, job::Stage};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row, TransactionBehavior};

const CURRENT_SCHEMA_VERSION: i64 = 6;
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_dam_feedback.sql")),
    (3, include_str!("../migrations/0003_source_fidelity.sql")),
    (4, include_str!("../migrations/0004_strong_shot.sql")),
    (5, include_str!("../migrations/0005_feedback_hardening.sql")),
    (6, include_str!("../migrations/0006_photo_jobs.sql")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoStatus {
    Pending,
    Split,
    Embedded,
    Transcribed,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub owner_id: String,
    pub path: String,
    pub sha256: String,
    pub duration_s: Option<f64>,
    pub fps: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub has_audio: bool,
    pub status: VideoStatus,
    pub indexed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoStatus {
    Pending,
    Embedded,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Photo {
    pub id: String,
    pub owner_id: String,
    pub path: String,
    pub sha256: String,
    pub width: i64,
    pub height: i64,
    pub format: String,
    pub orientation: Option<i64>,
    pub captured_at: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    /// Path relative to `<data_dir>/thumbs`.
    pub thumb_rel: Option<String>,
    pub status: PhotoStatus,
    pub indexed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoProxyProvenance {
    DecodedOriginal,
    FullRender,
    EmbeddedPreview,
}

/// Source facts needed to reproduce trustworthy photo derivatives without changing the original.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoSourceMetadata {
    pub photo_id: String,
    pub owner_id: String,
    pub source_format: String,
    pub decoder: String,
    /// Path relative to `<data_dir>/proxies`.
    pub proxy_rel: Option<String>,
    pub proxy_width: Option<i64>,
    pub proxy_height: Option<i64>,
    pub proxy_sha256: Option<String>,
    pub proxy_provenance: PhotoProxyProvenance,
    pub orientation_applied: bool,
    pub bit_depth: Option<i64>,
    pub color_space: Option<String>,
    pub icc_profile_name: Option<String>,
    pub icc_profile_sha256: Option<String>,
    pub exposure_json: String,
    /// Coordinates are deliberately not stored by the default privacy policy.
    pub gps_present: bool,
    pub metadata_json: String,
    pub original_size_bytes: i64,
    pub extracted_at: DateTime<Utc>,
}

/// Source facts and edit-proxy policy for production video.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSourceMetadata {
    pub video_id: String,
    pub owner_id: String,
    pub container: String,
    pub video_codec: String,
    pub codec_profile: Option<String>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<i64>,
    pub color_space: Option<String>,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub color_range: Option<String>,
    pub rotation: Option<i64>,
    /// Path relative to `<data_dir>/proxies`.
    pub proxy_rel: Option<String>,
    pub proxy_sha256: Option<String>,
    pub proxy_required: bool,
    pub proxy_reason: Option<String>,
    pub original_size_bytes: i64,
    pub metadata_json: String,
    pub probed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Shot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorialAnnotation {
    pub owner_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    pub description: String,
    pub subjects: String,
    pub action: String,
    pub tags: String,
    pub quality: Option<i64>,
    pub standout: bool,
    pub usable: bool,
    pub faces_visible: bool,
    pub nametags_visible: bool,
    pub blur_required: bool,
    pub crop_x: Option<f64>,
    pub grade_json: Option<String>,
    pub notes: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AestheticAssessment {
    pub owner_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    pub sharpness: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub color_harmony: f64,
    pub balance: f64,
    pub subject_placement: f64,
    pub negative_space: f64,
    pub visual_clarity: f64,
    pub technical_quality: f64,
    pub blur_control: f64,
    pub clipping_control: f64,
    pub noise_control: f64,
    pub compression_quality: f64,
    pub resolution_quality: f64,
    pub motion_stability: f64,
    /// Probability that a sequence neighbor is a duplicate; high is risky.
    pub duplicate_confidence: f64,
    pub composition_quality: f64,
    pub hierarchy: f64,
    pub leading_lines: f64,
    pub symmetry: f64,
    pub crop_potential: f64,
    pub moment_story: f64,
    pub expression: f64,
    pub gesture: f64,
    pub action: f64,
    pub novelty: f64,
    pub pacing: f64,
    /// Sequence repetition risk; high is risky.
    pub repetition_risk: f64,
    pub overall: f64,
    pub confidence: f64,
    pub explanation_json: String,
    pub model_version: String,
    pub assessed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackSignal {
    Pick,
    Reject,
    Rating,
    Prefer,
    Crop,
    Grade,
    Export,
    Publish,
    Tag,
    Edit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackEvent {
    pub id: String,
    pub owner_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    pub signal: FeedbackSignal,
    pub value: Option<f64>,
    pub compared_media_kind: Option<MediaKind>,
    pub compared_media_id: Option<String>,
    pub context_json: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleProfile {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub version: i64,
    pub algorithm_version: String,
    pub embedding_weights: Vec<f32>,
    pub feature_weights_json: String,
    pub sample_count: i64,
    pub held_out_metric: Option<f64>,
    pub active: bool,
    pub trained_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub id: String,
    pub video_id: String,
    pub owner_id: String,
    pub idx: i64,
    pub start_s: f64,
    pub end_s: f64,
    pub rep_frame_s: f64,
    /// Path relative to `<data_dir>/thumbs`.
    pub thumb_rel: Option<String>,
    pub scene_score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub id: String,
    pub video_id: String,
    pub owner_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub confidence: Option<f64>,
}

/// Store-owned projection used to hydrate a ranked in-memory vector match.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchShotContext {
    pub shot_id: String,
    pub video_id: String,
    pub video_path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub thumb_rel: Option<String>,
}

/// A matching transcript segment joined to every shot whose interval it overlaps.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptShotHit {
    pub shot_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingMeta {
    pub owner_id: String,
    pub model_name: String,
    pub model_sha256: String,
    pub dim: usize,
    pub preprocess_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub id: String,
    /// Exactly one of `video_id` and `photo_id` must be set.
    pub video_id: Option<String>,
    /// Exactly one of `video_id` and `photo_id` must be set.
    pub photo_id: Option<String>,
    pub stage: Stage,
    pub started_at: DateTime<Utc>,
    pub debug_dir: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFilter {
    pub video_id: Option<String>,
    pub stage: Option<Stage>,
    pub status: Option<JobStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    MissingVector,
    MissingThumbnail,
    UnsafeThumbnailPath,
    MissingProxy,
    UnsafeProxyPath,
    OrphanVector,
    InvalidVectorBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub entity_id: String,
    pub detail: String,
}

pub struct Store {
    connection: Connection,
    data_dir: PathBuf,
    db_path: PathBuf,
}

impl Store {
    /// Open `<data_dir>/library.db`, applying pending migrations before returning.
    pub fn open(data_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
        std::fs::create_dir_all(data_dir.join("thumbs"))?;
        std::fs::create_dir_all(data_dir.join("proxies"))?;
        let db_path = data_dir.join("library.db");
        let mut connection = Connection::open(&db_path)
            .with_context(|| format!("failed to open SQLite database {}", db_path.display()))?;

        configure_connection(&connection)?;
        apply_migrations(&mut connection)?;

        Ok(Self {
            connection,
            data_dir,
            db_path,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn thumbnail_path(&self, relative: &str) -> anyhow::Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            safe_relative_path(relative),
            "thumbnail path must be a safe path relative to the thumbs directory"
        );
        Ok(self.data_dir.join("thumbs").join(relative))
    }

    pub fn proxy_path(&self, relative: &str) -> anyhow::Result<PathBuf> {
        let relative = Path::new(relative);
        ensure!(
            safe_relative_path(relative),
            "proxy path must be a safe path relative to the proxies directory"
        );
        Ok(self.data_dir.join("proxies").join(relative))
    }

    pub fn schema_version(&self) -> anyhow::Result<i64> {
        self.connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .context("failed to read schema version")
    }

    pub fn upsert_photo(&self, owner_id: &str, photo: &Photo) -> anyhow::Result<Photo> {
        ensure_owner_matches(owner_id, &photo.owner_id, "photo")?;
        validate_photo(photo)?;
        self.connection.execute(
            "INSERT INTO photos (
                id, owner_id, path, sha256, width, height, format, orientation, captured_at,
                camera_make, camera_model, lens, thumb_rel, status, indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(owner_id, sha256) DO UPDATE SET
                path = excluded.path,
                width = excluded.width,
                height = excluded.height,
                format = excluded.format,
                orientation = excluded.orientation,
                captured_at = excluded.captured_at,
                camera_make = excluded.camera_make,
                camera_model = excluded.camera_model,
                lens = excluded.lens,
                thumb_rel = excluded.thumb_rel,
                status = excluded.status,
                indexed_at = excluded.indexed_at",
            params![
                photo.id,
                owner_id,
                photo.path,
                photo.sha256,
                photo.width,
                photo.height,
                photo.format,
                photo.orientation,
                photo.captured_at.map(|value| value.to_rfc3339()),
                photo.camera_make,
                photo.camera_model,
                photo.lens,
                photo.thumb_rel,
                photo_status_to_str(photo.status),
                photo.indexed_at.map(|value| value.to_rfc3339()),
            ],
        )?;
        self.photo_by_sha(owner_id, &photo.sha256)?
            .context("upserted photo could not be read back")
    }

    pub fn photo_by_sha(&self, owner_id: &str, sha256: &str) -> anyhow::Result<Option<Photo>> {
        self.photo_query(
            "SELECT id, owner_id, path, sha256, width, height, format, orientation, captured_at,
                    camera_make, camera_model, lens, thumb_rel, status, indexed_at
             FROM photos WHERE owner_id = ?1 AND sha256 = ?2",
            owner_id,
            sha256,
        )
    }

    pub fn photo_by_id(&self, owner_id: &str, photo_id: &str) -> anyhow::Result<Option<Photo>> {
        self.photo_query(
            "SELECT id, owner_id, path, sha256, width, height, format, orientation, captured_at,
                    camera_make, camera_model, lens, thumb_rel, status, indexed_at
             FROM photos WHERE owner_id = ?1 AND id = ?2",
            owner_id,
            photo_id,
        )
    }

    pub fn photo_by_path(&self, owner_id: &str, path: &str) -> anyhow::Result<Option<Photo>> {
        self.photo_query(
            "SELECT id, owner_id, path, sha256, width, height, format, orientation, captured_at,
                    camera_make, camera_model, lens, thumb_rel, status, indexed_at
             FROM photos WHERE owner_id = ?1 AND path = ?2",
            owner_id,
            path,
        )
    }

    pub fn photos(&self, owner_id: &str) -> anyhow::Result<Vec<Photo>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, path, sha256, width, height, format, orientation, captured_at,
                    camera_make, camera_model, lens, thumb_rel, status, indexed_at
             FROM photos WHERE owner_id = ?1 ORDER BY path, id",
        )?;
        let rows = statement.query_map(params![owner_id], photo_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list photos")
    }

    pub fn set_photo_status(
        &self,
        owner_id: &str,
        photo_id: &str,
        status: PhotoStatus,
    ) -> anyhow::Result<()> {
        let indexed_at = (status == PhotoStatus::Done).then(|| Utc::now().to_rfc3339());
        let changed = self.connection.execute(
            "UPDATE photos
             SET status = ?3,
                 indexed_at = CASE WHEN ?3 = 'done' THEN COALESCE(indexed_at, ?4)
                                   ELSE indexed_at END
             WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, photo_id, photo_status_to_str(status), indexed_at],
        )?;
        ensure_changed(changed, "photo", photo_id)
    }

    pub fn upsert_photo_source_metadata(
        &self,
        owner_id: &str,
        metadata: &PhotoSourceMetadata,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &metadata.owner_id, "photo source metadata")?;
        validate_photo_source_metadata(metadata)?;
        self.connection.execute(
            "INSERT INTO photo_source_metadata (
                photo_id, owner_id, source_format, decoder, proxy_rel, proxy_width, proxy_height,
                proxy_sha256, proxy_provenance, orientation_applied, bit_depth, color_space,
                icc_profile_name, icc_profile_sha256, exposure_json, gps_present, metadata_json,
                original_size_bytes, extracted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19)
             ON CONFLICT(photo_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                source_format = excluded.source_format,
                decoder = excluded.decoder,
                proxy_rel = excluded.proxy_rel,
                proxy_width = excluded.proxy_width,
                proxy_height = excluded.proxy_height,
                proxy_sha256 = excluded.proxy_sha256,
                proxy_provenance = excluded.proxy_provenance,
                orientation_applied = excluded.orientation_applied,
                bit_depth = excluded.bit_depth,
                color_space = excluded.color_space,
                icc_profile_name = excluded.icc_profile_name,
                icc_profile_sha256 = excluded.icc_profile_sha256,
                exposure_json = excluded.exposure_json,
                gps_present = excluded.gps_present,
                metadata_json = excluded.metadata_json,
                original_size_bytes = excluded.original_size_bytes,
                extracted_at = excluded.extracted_at",
            params![
                metadata.photo_id,
                owner_id,
                metadata.source_format,
                metadata.decoder,
                metadata.proxy_rel,
                metadata.proxy_width,
                metadata.proxy_height,
                metadata.proxy_sha256,
                photo_proxy_provenance_to_str(metadata.proxy_provenance),
                metadata.orientation_applied,
                metadata.bit_depth,
                metadata.color_space,
                metadata.icc_profile_name,
                metadata.icc_profile_sha256,
                metadata.exposure_json,
                metadata.gps_present,
                metadata.metadata_json,
                metadata.original_size_bytes,
                metadata.extracted_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn photo_source_metadata(
        &self,
        owner_id: &str,
        photo_id: &str,
    ) -> anyhow::Result<Option<PhotoSourceMetadata>> {
        self.connection
            .query_row(
                "SELECT photo_id, owner_id, source_format, decoder, proxy_rel, proxy_width,
                        proxy_height, proxy_sha256, proxy_provenance, orientation_applied,
                        bit_depth, color_space, icc_profile_name, icc_profile_sha256, exposure_json,
                        gps_present, metadata_json, original_size_bytes, extracted_at
                 FROM photo_source_metadata WHERE owner_id = ?1 AND photo_id = ?2",
                params![owner_id, photo_id],
                photo_source_metadata_from_row,
            )
            .optional()
            .context("failed to query photo source metadata")
    }

    pub fn put_photo_vector(
        &self,
        owner_id: &str,
        photo_id: &str,
        values: &[f32],
    ) -> anyhow::Result<()> {
        ensure!(!values.is_empty(), "vector must not be empty");
        ensure!(
            values.iter().all(|value| value.is_finite()),
            "vector contains non-finite values"
        );
        self.connection.execute(
            "INSERT INTO photo_vectors (photo_id, owner_id, dim, vec)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(photo_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                dim = excluded.dim,
                vec = excluded.vec",
            params![
                photo_id,
                owner_id,
                values.len() as i64,
                vector_bytes(values)
            ],
        )?;
        Ok(())
    }

    pub fn vector_for_photo(
        &self,
        owner_id: &str,
        photo_id: &str,
    ) -> anyhow::Result<Option<Vec<f32>>> {
        self.vector_row(
            "SELECT dim, vec FROM photo_vectors WHERE owner_id = ?1 AND photo_id = ?2",
            owner_id,
            photo_id,
        )
    }

    pub fn load_all_photo_vectors(
        &self,
        owner_id: &str,
    ) -> anyhow::Result<(Vec<String>, Vec<f32>)> {
        self.load_vector_matrix(
            "SELECT photo_id, dim, vec FROM photo_vectors WHERE owner_id = ?1 ORDER BY photo_id",
            owner_id,
        )
    }

    pub fn upsert_editorial_annotation(
        &self,
        owner_id: &str,
        annotation: &EditorialAnnotation,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &annotation.owner_id, "editorial annotation")?;
        if let Some(quality) = annotation.quality {
            ensure!(
                (1..=5).contains(&quality),
                "quality must be between 1 and 5"
            );
        }
        if let Some(crop_x) = annotation.crop_x {
            ensure_unit_score(crop_x, "crop_x")?;
        }
        self.connection.execute(
            "INSERT INTO editorial_annotations (
                owner_id, media_kind, media_id, description, subjects, action, tags, quality,
                standout, usable, faces_visible, nametags_visible, blur_required, crop_x,
                grade_json, notes, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(owner_id, media_kind, media_id) DO UPDATE SET
                description = excluded.description,
                subjects = excluded.subjects,
                action = excluded.action,
                tags = excluded.tags,
                quality = excluded.quality,
                standout = excluded.standout,
                usable = excluded.usable,
                faces_visible = excluded.faces_visible,
                nametags_visible = excluded.nametags_visible,
                blur_required = excluded.blur_required,
                crop_x = excluded.crop_x,
                grade_json = excluded.grade_json,
                notes = excluded.notes,
                updated_at = excluded.updated_at",
            params![
                owner_id,
                media_kind_to_str(annotation.media_kind),
                annotation.media_id,
                annotation.description,
                annotation.subjects,
                annotation.action,
                annotation.tags,
                annotation.quality,
                annotation.standout,
                annotation.usable,
                annotation.faces_visible,
                annotation.nametags_visible,
                annotation.blur_required,
                annotation.crop_x,
                annotation.grade_json,
                annotation.notes,
                annotation.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn editorial_annotation(
        &self,
        owner_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<Option<EditorialAnnotation>> {
        self.connection
            .query_row(
                "SELECT owner_id, media_kind, media_id, description, subjects, action, tags,
                        quality, standout, usable, faces_visible, nametags_visible, blur_required,
                        crop_x, grade_json, notes, updated_at
                 FROM editorial_annotations
                 WHERE owner_id = ?1 AND media_kind = ?2 AND media_id = ?3",
                params![owner_id, media_kind_to_str(media_kind), media_id],
                editorial_annotation_from_row,
            )
            .optional()
            .context("failed to read editorial annotation")
    }

    pub fn upsert_aesthetic_assessment(
        &self,
        owner_id: &str,
        assessment: &AestheticAssessment,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &assessment.owner_id, "aesthetic assessment")?;
        for (name, score) in [
            ("sharpness", assessment.sharpness),
            ("exposure", assessment.exposure),
            ("contrast", assessment.contrast),
            ("color_harmony", assessment.color_harmony),
            ("balance", assessment.balance),
            ("subject_placement", assessment.subject_placement),
            ("negative_space", assessment.negative_space),
            ("visual_clarity", assessment.visual_clarity),
            ("technical_quality", assessment.technical_quality),
            ("blur_control", assessment.blur_control),
            ("clipping_control", assessment.clipping_control),
            ("noise_control", assessment.noise_control),
            ("compression_quality", assessment.compression_quality),
            ("resolution_quality", assessment.resolution_quality),
            ("motion_stability", assessment.motion_stability),
            ("duplicate_confidence", assessment.duplicate_confidence),
            ("composition_quality", assessment.composition_quality),
            ("hierarchy", assessment.hierarchy),
            ("leading_lines", assessment.leading_lines),
            ("symmetry", assessment.symmetry),
            ("crop_potential", assessment.crop_potential),
            ("moment_story", assessment.moment_story),
            ("expression", assessment.expression),
            ("gesture", assessment.gesture),
            ("action", assessment.action),
            ("novelty", assessment.novelty),
            ("pacing", assessment.pacing),
            ("repetition_risk", assessment.repetition_risk),
            ("overall", assessment.overall),
            ("confidence", assessment.confidence),
        ] {
            ensure_unit_score(score, name)?;
        }
        self.connection.execute(
            "INSERT INTO aesthetic_assessments (
                owner_id, media_kind, media_id, sharpness, exposure, contrast, color_harmony,
                balance, subject_placement, negative_space, visual_clarity, overall, confidence,
                explanation_json, model_version, assessed_at, technical_quality, blur_control,
                clipping_control, noise_control, compression_quality, resolution_quality,
                motion_stability, duplicate_confidence, composition_quality, hierarchy,
                leading_lines, symmetry, crop_potential, moment_story, expression, gesture,
                action, novelty, pacing, repetition_risk
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                       ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30,
                       ?31, ?32, ?33, ?34, ?35, ?36)
             ON CONFLICT(owner_id, media_kind, media_id) DO UPDATE SET
                sharpness = excluded.sharpness,
                exposure = excluded.exposure,
                contrast = excluded.contrast,
                color_harmony = excluded.color_harmony,
                balance = excluded.balance,
                subject_placement = excluded.subject_placement,
                negative_space = excluded.negative_space,
                visual_clarity = excluded.visual_clarity,
                overall = excluded.overall,
                confidence = excluded.confidence,
                explanation_json = excluded.explanation_json,
                model_version = excluded.model_version,
                assessed_at = excluded.assessed_at,
                technical_quality = excluded.technical_quality,
                blur_control = excluded.blur_control,
                clipping_control = excluded.clipping_control,
                noise_control = excluded.noise_control,
                compression_quality = excluded.compression_quality,
                resolution_quality = excluded.resolution_quality,
                motion_stability = excluded.motion_stability,
                duplicate_confidence = excluded.duplicate_confidence,
                composition_quality = excluded.composition_quality,
                hierarchy = excluded.hierarchy,
                leading_lines = excluded.leading_lines,
                symmetry = excluded.symmetry,
                crop_potential = excluded.crop_potential,
                moment_story = excluded.moment_story,
                expression = excluded.expression,
                gesture = excluded.gesture,
                action = excluded.action,
                novelty = excluded.novelty,
                pacing = excluded.pacing,
                repetition_risk = excluded.repetition_risk",
            params![
                owner_id,
                media_kind_to_str(assessment.media_kind),
                assessment.media_id,
                assessment.sharpness,
                assessment.exposure,
                assessment.contrast,
                assessment.color_harmony,
                assessment.balance,
                assessment.subject_placement,
                assessment.negative_space,
                assessment.visual_clarity,
                assessment.overall,
                assessment.confidence,
                assessment.explanation_json,
                assessment.model_version,
                assessment.assessed_at.to_rfc3339(),
                assessment.technical_quality,
                assessment.blur_control,
                assessment.clipping_control,
                assessment.noise_control,
                assessment.compression_quality,
                assessment.resolution_quality,
                assessment.motion_stability,
                assessment.duplicate_confidence,
                assessment.composition_quality,
                assessment.hierarchy,
                assessment.leading_lines,
                assessment.symmetry,
                assessment.crop_potential,
                assessment.moment_story,
                assessment.expression,
                assessment.gesture,
                assessment.action,
                assessment.novelty,
                assessment.pacing,
                assessment.repetition_risk,
            ],
        )?;
        Ok(())
    }

    pub fn aesthetic_assessment(
        &self,
        owner_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<Option<AestheticAssessment>> {
        self.connection
            .query_row(
                "SELECT owner_id, media_kind, media_id, sharpness, exposure, contrast,
                        color_harmony, balance, subject_placement, negative_space, visual_clarity,
                        overall, confidence, explanation_json, model_version, assessed_at,
                        technical_quality, blur_control, clipping_control, noise_control,
                        compression_quality, resolution_quality, motion_stability,
                        duplicate_confidence, composition_quality, hierarchy, leading_lines,
                        symmetry, crop_potential, moment_story, expression, gesture, action,
                        novelty, pacing, repetition_risk
                 FROM aesthetic_assessments
                 WHERE owner_id = ?1 AND media_kind = ?2 AND media_id = ?3",
                params![owner_id, media_kind_to_str(media_kind), media_id],
                aesthetic_assessment_from_row,
            )
            .optional()
            .context("failed to read aesthetic assessment")
    }

    pub fn append_feedback(&self, owner_id: &str, event: &FeedbackEvent) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &event.owner_id, "feedback event")?;
        let has_comparison =
            event.compared_media_kind.is_some() && event.compared_media_id.is_some();
        ensure!(
            event.compared_media_kind.is_some() == event.compared_media_id.is_some(),
            "compared media kind and id must be supplied together"
        );
        ensure!(
            event.signal == FeedbackSignal::Prefer || !has_comparison,
            "only prefer feedback may compare two assets"
        );
        ensure!(
            event.signal != FeedbackSignal::Prefer || has_comparison,
            "prefer feedback requires a compared asset"
        );
        if event.signal == FeedbackSignal::Rating {
            ensure!(
                event
                    .value
                    .is_some_and(|value| (1.0..=5.0).contains(&value)),
                "rating feedback requires a value from 1 to 5"
            );
        }
        validate_json_object(&event.context_json, "context_json")?;
        self.connection.execute(
            "INSERT INTO feedback_events (
                id, owner_id, media_kind, media_id, signal, value, compared_media_kind,
                compared_media_id, context_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.id,
                owner_id,
                media_kind_to_str(event.media_kind),
                event.media_id,
                feedback_signal_to_str(event.signal),
                event.value,
                event.compared_media_kind.map(media_kind_to_str),
                event.compared_media_id,
                event.context_json,
                event.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn feedback_events(&self, owner_id: &str) -> anyhow::Result<Vec<FeedbackEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, media_kind, media_id, signal, value, compared_media_kind,
                    compared_media_id, context_json, created_at
             FROM feedback_events WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], feedback_event_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list feedback events")
    }

    pub fn put_style_profile(
        &mut self,
        owner_id: &str,
        profile: &StyleProfile,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &profile.owner_id, "style profile")?;
        ensure!(
            profile.version > 0,
            "style profile version must be positive"
        );
        ensure!(
            !profile.embedding_weights.is_empty(),
            "style profile embedding weights must not be empty"
        );
        ensure!(
            profile.sample_count >= 0,
            "sample count must be non-negative"
        );
        ensure!(
            profile
                .embedding_weights
                .iter()
                .all(|value| value.is_finite()),
            "style profile contains non-finite embedding weights"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if profile.active {
            transaction.execute(
                "UPDATE style_profiles SET active = 0 WHERE owner_id = ?1",
                params![owner_id],
            )?;
        }
        transaction.execute(
            "INSERT INTO style_profiles (
                id, owner_id, name, version, algorithm_version, embedding_dim,
                embedding_weights, feature_weights_json, sample_count, held_out_metric, active,
                trained_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id, owner_id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                algorithm_version = excluded.algorithm_version,
                embedding_dim = excluded.embedding_dim,
                embedding_weights = excluded.embedding_weights,
                feature_weights_json = excluded.feature_weights_json,
                sample_count = excluded.sample_count,
                held_out_metric = excluded.held_out_metric,
                active = excluded.active,
                trained_at = excluded.trained_at",
            params![
                profile.id,
                owner_id,
                profile.name,
                profile.version,
                profile.algorithm_version,
                profile.embedding_weights.len() as i64,
                vector_bytes(&profile.embedding_weights),
                profile.feature_weights_json,
                profile.sample_count,
                profile.held_out_metric,
                profile.active,
                profile.trained_at.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        let stored_owner: String = self
            .connection
            .query_row(
                "SELECT owner_id FROM style_profiles WHERE id = ?1",
                params![profile.id],
                |row| row.get(0),
            )
            .context("upserted style profile could not be read back")?;
        ensure_owner_matches(owner_id, &stored_owner, "style profile")
    }

    pub fn active_style_profile(&self, owner_id: &str) -> anyhow::Result<Option<StyleProfile>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, version, algorithm_version, embedding_dim,
                        embedding_weights, feature_weights_json, sample_count, held_out_metric,
                        active, trained_at
                 FROM style_profiles WHERE owner_id = ?1 AND active = 1",
                params![owner_id],
                style_profile_from_row,
            )
            .optional()
            .context("failed to read active style profile")
    }

    fn photo_query(&self, sql: &str, owner_id: &str, value: &str) -> anyhow::Result<Option<Photo>> {
        self.connection
            .query_row(sql, params![owner_id, value], photo_from_row)
            .optional()
            .context("failed to query photo")
    }

    fn vector_row(
        &self,
        sql: &str,
        owner_id: &str,
        media_id: &str,
    ) -> anyhow::Result<Option<Vec<f32>>> {
        let row = self
            .connection
            .query_row(sql, params![owner_id, media_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .optional()?;
        row.map(|(dim, bytes)| decode_vector(dim, bytes, media_id))
            .transpose()
    }

    fn load_vector_matrix(
        &self,
        sql: &str,
        owner_id: &str,
    ) -> anyhow::Result<(Vec<String>, Vec<f32>)> {
        let mut statement = self.connection.prepare(sql)?;
        let mut rows = statement.query(params![owner_id])?;
        let mut ids = Vec::new();
        let mut matrix = Vec::new();
        let mut expected_dim = None;
        while let Some(row) = rows.next()? {
            let media_id: String = row.get(0)?;
            let dim: i64 = row.get(1)?;
            let values = decode_vector(dim, row.get(2)?, &media_id)?;
            if let Some(previous) = expected_dim {
                ensure!(
                    previous == values.len(),
                    "vector {media_id} has inconsistent dimension"
                );
            } else {
                expected_dim = Some(values.len());
            }
            ids.push(media_id);
            matrix.extend(values);
        }
        Ok((ids, matrix))
    }

    pub fn upsert_video(&self, owner_id: &str, video: &Video) -> anyhow::Result<Video> {
        ensure_owner_matches(owner_id, &video.owner_id, "video")?;
        self.connection.execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status,
                indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(owner_id, sha256) DO UPDATE SET
                path = excluded.path,
                duration_s = excluded.duration_s,
                fps = excluded.fps,
                width = excluded.width,
                height = excluded.height,
                has_audio = excluded.has_audio,
                status = excluded.status,
                indexed_at = excluded.indexed_at",
            params![
                video.id,
                owner_id,
                video.path,
                video.sha256,
                video.duration_s,
                video.fps,
                video.width,
                video.height,
                video.has_audio,
                video_status_to_str(video.status),
                video.indexed_at.map(|value| value.to_rfc3339()),
            ],
        )?;

        self.video_by_sha(owner_id, &video.sha256)?
            .context("upserted video could not be read back")
    }

    pub fn video_by_sha(&self, owner_id: &str, sha256: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND sha256 = ?2",
                params![owner_id, sha256],
                video_from_row,
            )
            .optional()
            .context("failed to query video by sha256")
    }

    pub fn video_by_id(&self, owner_id: &str, video_id: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, video_id],
                video_from_row,
            )
            .optional()
            .context("failed to query video by id")
    }

    pub fn video_by_path(&self, owner_id: &str, path: &str) -> anyhow::Result<Option<Video>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                        status, indexed_at
                 FROM videos
                 WHERE owner_id = ?1 AND path = ?2",
                params![owner_id, path],
                video_from_row,
            )
            .optional()
            .context("failed to query video by path")
    }

    pub fn videos(&self, owner_id: &str) -> anyhow::Result<Vec<Video>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, path, sha256, duration_s, fps, width, height, has_audio,
                    status, indexed_at
             FROM videos
             WHERE owner_id = ?1
             ORDER BY path, id",
        )?;
        let rows = statement.query_map(params![owner_id], video_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list videos")
    }

    pub fn set_video_status(
        &self,
        owner_id: &str,
        video_id: &str,
        status: VideoStatus,
    ) -> anyhow::Result<()> {
        let status_text = video_status_to_str(status);
        let indexed_at = (status == VideoStatus::Done).then(|| Utc::now().to_rfc3339());
        let changed = self.connection.execute(
            "UPDATE videos
             SET status = ?3,
                 indexed_at = CASE WHEN ?3 = 'done' THEN COALESCE(indexed_at, ?4)
                                   ELSE indexed_at END
             WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id, status_text, indexed_at],
        )?;
        ensure_changed(changed, "video", video_id)
    }

    pub fn upsert_video_source_metadata(
        &self,
        owner_id: &str,
        metadata: &VideoSourceMetadata,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &metadata.owner_id, "video source metadata")?;
        validate_video_source_metadata(metadata)?;
        self.connection.execute(
            "INSERT INTO video_source_metadata (
                video_id, owner_id, container, video_codec, codec_profile, pixel_format,
                bit_depth, color_space, color_primaries, color_transfer, color_range, rotation,
                proxy_rel, proxy_sha256, proxy_required, proxy_reason, original_size_bytes,
                metadata_json, probed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19)
             ON CONFLICT(video_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                container = excluded.container,
                video_codec = excluded.video_codec,
                codec_profile = excluded.codec_profile,
                pixel_format = excluded.pixel_format,
                bit_depth = excluded.bit_depth,
                color_space = excluded.color_space,
                color_primaries = excluded.color_primaries,
                color_transfer = excluded.color_transfer,
                color_range = excluded.color_range,
                rotation = excluded.rotation,
                proxy_rel = excluded.proxy_rel,
                proxy_sha256 = excluded.proxy_sha256,
                proxy_required = excluded.proxy_required,
                proxy_reason = excluded.proxy_reason,
                original_size_bytes = excluded.original_size_bytes,
                metadata_json = excluded.metadata_json,
                probed_at = excluded.probed_at",
            params![
                metadata.video_id,
                owner_id,
                metadata.container,
                metadata.video_codec,
                metadata.codec_profile,
                metadata.pixel_format,
                metadata.bit_depth,
                metadata.color_space,
                metadata.color_primaries,
                metadata.color_transfer,
                metadata.color_range,
                metadata.rotation,
                metadata.proxy_rel,
                metadata.proxy_sha256,
                metadata.proxy_required,
                metadata.proxy_reason,
                metadata.original_size_bytes,
                metadata.metadata_json,
                metadata.probed_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn video_source_metadata(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<Option<VideoSourceMetadata>> {
        self.connection
            .query_row(
                "SELECT video_id, owner_id, container, video_codec, codec_profile, pixel_format,
                        bit_depth, color_space, color_primaries, color_transfer, color_range,
                        rotation, proxy_rel, proxy_sha256, proxy_required, proxy_reason,
                        original_size_bytes, metadata_json, probed_at
                 FROM video_source_metadata WHERE owner_id = ?1 AND video_id = ?2",
                params![owner_id, video_id],
                video_source_metadata_from_row,
            )
            .optional()
            .context("failed to query video source metadata")
    }

    pub fn insert_shots(&mut self, owner_id: &str, shots: &[Shot]) -> anyhow::Result<()> {
        for shot in shots {
            ensure_owner_matches(owner_id, &shot.owner_id, "shot")?;
            validate_shot(shot)?;
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO shots (
                    id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for shot in shots {
                statement.execute(params![
                    shot.id,
                    shot.video_id,
                    owner_id,
                    shot.idx,
                    shot.start_s,
                    shot.end_s,
                    shot.rep_frame_s,
                    shot.thumb_rel,
                    shot.scene_score,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Replace a video's shot graph and advance it to `split` in one transaction.
    pub fn replace_shots(
        &mut self,
        owner_id: &str,
        video_id: &str,
        shots: &[Shot],
    ) -> anyhow::Result<()> {
        ensure!(!shots.is_empty(), "cannot replace with an empty shot list");
        for shot in shots {
            ensure_owner_matches(owner_id, &shot.owner_id, "shot")?;
            ensure!(
                shot.video_id == video_id,
                "shot belongs to a different video"
            );
            validate_shot(shot)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM shots WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
        )?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO shots (
                    id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for shot in shots {
                statement.execute(params![
                    shot.id,
                    video_id,
                    owner_id,
                    shot.idx,
                    shot.start_s,
                    shot.end_s,
                    shot.rep_frame_s,
                    shot.thumb_rel,
                    shot.scene_score,
                ])?;
            }
        }
        let changed = transaction.execute(
            "UPDATE videos SET status = 'split' WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id],
        )?;
        ensure_changed(changed, "video", video_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_vectors_for_video(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<usize> {
        self.connection
            .execute(
                "DELETE FROM shot_vectors
                 WHERE owner_id = ?1 AND shot_id IN (
                     SELECT id FROM shots WHERE owner_id = ?1 AND video_id = ?2
                 )",
                params![owner_id, video_id],
            )
            .context("failed to delete video vectors")
    }

    pub fn shots_for_video(&self, owner_id: &str, video_id: &str) -> anyhow::Result<Vec<Shot>> {
        let mut statement = self.connection.prepare(
            "SELECT id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
             FROM shots
             WHERE owner_id = ?1 AND video_id = ?2
             ORDER BY idx",
        )?;
        let rows = statement.query_map(params![owner_id, video_id], shot_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to load shots for video")
    }

    pub fn shot_by_id(&self, owner_id: &str, shot_id: &str) -> anyhow::Result<Option<Shot>> {
        self.connection
            .query_row(
                "SELECT id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                        scene_score
                 FROM shots
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, shot_id],
                shot_from_row,
            )
            .optional()
            .context("failed to query shot by id")
    }

    pub fn put_vector(&self, owner_id: &str, shot_id: &str, values: &[f32]) -> anyhow::Result<()> {
        ensure!(!values.is_empty(), "vector must not be empty");
        ensure!(
            values.iter().all(|value| value.is_finite()),
            "vector contains non-finite values"
        );

        self.connection.execute(
            "INSERT INTO shot_vectors (shot_id, owner_id, dim, vec)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(shot_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                dim = excluded.dim,
                vec = excluded.vec",
            params![shot_id, owner_id, values.len() as i64, vector_bytes(values),],
        )?;
        Ok(())
    }

    /// Load one vector, preserving its exact little-endian f32 representation.
    pub fn vector_for_shot(
        &self,
        owner_id: &str,
        shot_id: &str,
    ) -> anyhow::Result<Option<Vec<f32>>> {
        let row = self
            .connection
            .query_row(
                "SELECT dim, vec FROM shot_vectors WHERE owner_id = ?1 AND shot_id = ?2",
                params![owner_id, shot_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;
        let Some((dim_i64, bytes)) = row else {
            return Ok(None);
        };
        let dim = usize::try_from(dim_i64).context("vector dimension was negative")?;
        ensure!(
            bytes.len() == dim * size_of::<f32>(),
            "vector {shot_id} contains {} bytes; expected {}",
            bytes.len(),
            dim * size_of::<f32>()
        );
        Ok(Some(
            bytes
                .as_chunks::<{ size_of::<f32>() }>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect(),
        ))
    }

    /// Return shot ids plus one contiguous row-major vector matrix.
    pub fn load_all_vectors(&self, owner_id: &str) -> anyhow::Result<(Vec<String>, Vec<f32>)> {
        let mut statement = self.connection.prepare(
            "SELECT shot_id, dim, vec
             FROM shot_vectors
             WHERE owner_id = ?1
             ORDER BY shot_id",
        )?;
        let mut rows = statement.query(params![owner_id])?;
        let mut ids = Vec::new();
        let mut matrix = Vec::new();
        let mut expected_dim = None;

        while let Some(row) = rows.next()? {
            let shot_id: String = row.get(0)?;
            let dim_i64: i64 = row.get(1)?;
            let dim = usize::try_from(dim_i64).context("vector dimension was negative")?;
            let bytes: Vec<u8> = row.get(2)?;
            let expected_bytes = dim
                .checked_mul(size_of::<f32>())
                .context("vector byte length overflowed usize")?;
            ensure!(
                bytes.len() == expected_bytes,
                "vector {shot_id} contains {} bytes; expected {} for dim {dim}",
                bytes.len(),
                expected_bytes
            );
            if let Some(previous) = expected_dim {
                ensure!(
                    previous == dim,
                    "vector {shot_id} has dim {dim}; expected uniform dim {previous}"
                );
            } else {
                expected_dim = Some(dim);
                matrix
                    .try_reserve(dim)
                    .context("failed to reserve the vector matrix")?;
            }

            // Length was validated above, so every chunk contains exactly one little-endian f32.
            for chunk in bytes.chunks(size_of::<f32>()) {
                matrix.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            ids.push(shot_id);
        }

        Ok((ids, matrix))
    }

    pub fn insert_transcript_segments(
        &mut self,
        owner_id: &str,
        segments: &[TranscriptSegment],
    ) -> anyhow::Result<()> {
        for segment in segments {
            ensure_owner_matches(owner_id, &segment.owner_id, "transcript segment")?;
            ensure!(
                segment.end_s > segment.start_s,
                "segment end must exceed start"
            );
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for segment in segments {
            transaction.execute(
                "INSERT INTO transcripts (
                    id, video_id, owner_id, start_s, end_s, text, confidence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    segment.video_id,
                    owner_id,
                    segment.start_s,
                    segment.end_s,
                    segment.text,
                    segment.confidence,
                ],
            )?;
            let rowid = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO transcripts_fts(rowid, text) VALUES (?1, ?2)",
                params![rowid, segment.text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Atomically replace one video's transcript and its external-content FTS rows.
    pub fn replace_transcript_segments(
        &mut self,
        owner_id: &str,
        video_id: &str,
        segments: &[TranscriptSegment],
    ) -> anyhow::Result<()> {
        for segment in segments {
            ensure_owner_matches(owner_id, &segment.owner_id, "transcript segment")?;
            ensure!(
                segment.video_id == video_id,
                "transcript segment belongs to a different video"
            );
            ensure!(
                segment.end_s > segment.start_s,
                "segment end must exceed start"
            );
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM transcripts_fts
             WHERE rowid IN (
                 SELECT rowid FROM transcripts WHERE owner_id = ?1 AND video_id = ?2
             )",
            params![owner_id, video_id],
        )?;
        transaction.execute(
            "DELETE FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
        )?;
        for segment in segments {
            transaction.execute(
                "INSERT INTO transcripts (
                    id, video_id, owner_id, start_s, end_s, text, confidence
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    video_id,
                    owner_id,
                    segment.start_s,
                    segment.end_s,
                    segment.text,
                    segment.confidence,
                ],
            )?;
            let rowid = transaction.last_insert_rowid();
            transaction.execute(
                "INSERT INTO transcripts_fts(rowid, text) VALUES (?1, ?2)",
                params![rowid, segment.text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn transcript_count_for_video(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            params![owner_id, video_id],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("transcript count was negative")
    }

    pub fn segments_overlapping(
        &self,
        owner_id: &str,
        video_id: &str,
        start_s: f64,
        end_s: f64,
    ) -> anyhow::Result<Vec<TranscriptSegment>> {
        ensure!(end_s > start_s, "query end must exceed start");
        let mut statement = self.connection.prepare(
            "SELECT id, video_id, owner_id, start_s, end_s, text, confidence
             FROM transcripts
             WHERE owner_id = ?1 AND video_id = ?2 AND start_s < ?4 AND end_s > ?3
             ORDER BY start_s, id",
        )?;
        let rows = statement.query_map(
            params![owner_id, video_id, start_s, end_s],
            transcript_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to load overlapping transcript segments")
    }

    pub fn search_transcripts(
        &self,
        owner_id: &str,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<TranscriptSegment>> {
        ensure!(!query.trim().is_empty(), "FTS query must not be empty");
        let mut statement = self.connection.prepare(
            "SELECT t.id, t.video_id, t.owner_id, t.start_s, t.end_s, t.text, t.confidence
             FROM transcripts_fts
             JOIN transcripts AS t ON t.rowid = transcripts_fts.rowid
             WHERE transcripts_fts MATCH ?2 AND t.owner_id = ?1
             ORDER BY bm25(transcripts_fts), t.start_s
             LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![owner_id, query, limit as i64], transcript_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to search transcripts")
    }

    pub fn search_shot_context(
        &self,
        owner_id: &str,
        shot_id: &str,
    ) -> anyhow::Result<Option<SearchShotContext>> {
        self.connection
            .query_row(
                "SELECT s.id, s.video_id, v.path, s.start_s, s.end_s, s.thumb_rel
                 FROM shots AS s
                 JOIN videos AS v ON v.id = s.video_id AND v.owner_id = s.owner_id
                 WHERE s.owner_id = ?1 AND s.id = ?2",
                params![owner_id, shot_id],
                |row| {
                    Ok(SearchShotContext {
                        shot_id: row.get(0)?,
                        video_id: row.get(1)?,
                        video_path: row.get(2)?,
                        start_s: row.get(3)?,
                        end_s: row.get(4)?,
                        thumb_rel: row.get(5)?,
                    })
                },
            )
            .optional()
            .context("failed to load search shot context")
    }

    pub fn transcript_shot_hits(
        &self,
        owner_id: &str,
        fts_query: &str,
    ) -> anyhow::Result<Vec<TranscriptShotHit>> {
        ensure!(!fts_query.trim().is_empty(), "FTS query must not be empty");
        let mut statement = self.connection.prepare(
            "SELECT s.id, t.text
             FROM transcripts_fts
             JOIN transcripts AS t ON t.rowid = transcripts_fts.rowid
             JOIN shots AS s
               ON s.video_id = t.video_id
              AND s.owner_id = t.owner_id
              AND t.start_s < s.end_s
              AND t.end_s > s.start_s
             WHERE transcripts_fts MATCH ?2 AND t.owner_id = ?1
             ORDER BY bm25(transcripts_fts), t.start_s, s.id",
        )?;
        let rows = statement.query_map(params![owner_id, fts_query], |row| {
            Ok(TranscriptShotHit {
                shot_id: row.get(0)?,
                text: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to join transcript matches to shots")
    }

    pub fn job_start(&self, owner_id: &str, job: &NewJob) -> anyhow::Result<JobRecord> {
        ensure!(
            job.video_id.is_some() ^ job.photo_id.is_some(),
            "job {} must reference exactly one of video_id or photo_id",
            job.id
        );
        if let Some(photo_id) = &job.photo_id {
            ensure!(
                job.stage == Stage::PhotoIngest || job.stage == Stage::Analyze,
                "photo job {} stage must be photo_ingest or analyze",
                job.id
            );
            ensure!(
                self.photo_by_id(owner_id, photo_id)?.is_some(),
                "job photo {photo_id} does not exist"
            );
        }
        if let Some(video_id) = &job.video_id {
            ensure!(
                job.stage != Stage::PhotoIngest,
                "video job {} cannot use the photo_ingest stage",
                job.id
            );
            ensure!(
                self.video_by_id(owner_id, video_id)?.is_some(),
                "job video {video_id} does not exist"
            );
        }
        self.connection.execute(
            "INSERT INTO jobs (
                id, owner_id, video_id, photo_id, stage, status, started_at, debug_dir
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7)",
            params![
                job.id,
                owner_id,
                job.video_id,
                job.photo_id,
                stage_to_str(job.stage),
                job.started_at.to_rfc3339(),
                job.debug_dir,
            ],
        )?;
        self.job_by_id(owner_id, &job.id)?
            .context("started job could not be read back")
    }

    pub fn job_finish(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.complete_job(owner_id, job_id, JobStatus::Done, finished_at, None)
    }

    pub fn job_fail(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
        error: &str,
    ) -> anyhow::Result<()> {
        ensure!(!error.trim().is_empty(), "failed job must include an error");
        self.complete_job(
            owner_id,
            job_id,
            JobStatus::Failed,
            finished_at,
            Some(error),
        )
    }

    pub fn job_cancel(
        &self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.complete_job(owner_id, job_id, JobStatus::Cancelled, finished_at, None)
    }

    pub fn jobs(&self, owner_id: &str, filter: &JobFilter) -> anyhow::Result<Vec<JobRecord>> {
        let stage = filter.stage.map(stage_to_str);
        let status = filter.status.map(job_status_to_str);
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, video_id, photo_id, stage, status, started_at, finished_at,
                    duration_ms, error, debug_dir
             FROM jobs
             WHERE owner_id = ?1
               AND (?2 IS NULL OR video_id = ?2)
               AND (?3 IS NULL OR stage = ?3)
               AND (?4 IS NULL OR status = ?4)
             ORDER BY started_at DESC, id DESC",
        )?;
        let rows = statement.query_map(
            params![owner_id, filter.video_id, stage, status],
            job_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list jobs")
    }

    pub fn fail_running_jobs_as_interrupted(&self, owner_id: &str) -> anyhow::Result<usize> {
        let now = Utc::now();
        let jobs = self.jobs(
            owner_id,
            &JobFilter {
                status: Some(JobStatus::Running),
                ..JobFilter::default()
            },
        )?;
        // The whole pass is atomic: either every running job and its video/photo end up
        // failed, or nothing changes. `transaction_with_behavior` requires `&mut self` and
        // this API stays shared-access, so the Immediate transaction is driven directly on
        // the connection.
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let marked = mark_jobs_interrupted(&self.connection, owner_id, now, &jobs);
        if let Err(error) = marked {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(error);
        }
        if let Err(error) = self.connection.execute_batch("COMMIT") {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(error.into());
        }
        Ok(jobs.len())
    }

    /// Recover the last completed stage after a process or stage marked the video failed.
    pub fn restore_failed_video_status(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<VideoStatus> {
        let video = self
            .video_by_id(owner_id, video_id)?
            .with_context(|| format!("video {video_id} was not found"))?;
        if video.status != VideoStatus::Failed {
            return Ok(video.status);
        }
        let failed = self.jobs(
            owner_id,
            &JobFilter {
                video_id: Some(video_id.to_owned()),
                status: Some(JobStatus::Failed),
                ..JobFilter::default()
            },
        )?;
        let status = match failed.first().map(|job| job.stage) {
            Some(Stage::Split) | None => VideoStatus::Pending,
            Some(Stage::Embed) => VideoStatus::Split,
            Some(Stage::PhotoIngest) | Some(Stage::Analyze) | Some(Stage::Transcribe) => {
                VideoStatus::Embedded
            }
        };
        self.set_video_status(owner_id, video_id, status)?;
        Ok(status)
    }

    /// Done photos that have no aesthetic assessment for `model_version`, or whose stored
    /// assessment was produced by a different model version, ordered like `photos()`
    /// (path, id) so backfill analysis order is stable across runs.
    pub fn photos_for_analysis(
        &self,
        owner_id: &str,
        model_version: &str,
    ) -> anyhow::Result<Vec<Photo>> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.owner_id, p.path, p.sha256, p.width, p.height, p.format,
                    p.orientation, p.captured_at, p.camera_make, p.camera_model, p.lens,
                    p.thumb_rel, p.status, p.indexed_at
             FROM photos AS p
             LEFT JOIN aesthetic_assessments AS a
               ON a.owner_id = p.owner_id
              AND a.media_kind = 'photo'
              AND a.media_id = p.id
              AND a.model_version = ?2
             WHERE p.owner_id = ?1
               AND p.status = 'done'
               AND a.owner_id IS NULL
             ORDER BY p.path, p.id",
        )?;
        let rows = statement.query_map(params![owner_id, model_version], photo_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list photos needing analysis")
    }

    pub fn embedding_meta_get(&self, owner_id: &str) -> anyhow::Result<Option<EmbeddingMeta>> {
        self.connection
            .query_row(
                "SELECT owner_id, model_name, model_sha256, dim, preprocess_version
                 FROM embedding_meta
                 WHERE owner_id = ?1",
                params![owner_id],
                |row| {
                    let dim: i64 = row.get(3)?;
                    Ok(EmbeddingMeta {
                        owner_id: row.get(0)?,
                        model_name: row.get(1)?,
                        model_sha256: row.get(2)?,
                        dim: usize::try_from(dim).map_err(|error| conversion_error(3, error))?,
                        preprocess_version: row.get(4)?,
                    })
                },
            )
            .optional()
            .context("failed to read embedding metadata")
    }

    pub fn embedding_meta_set(
        &self,
        owner_id: &str,
        metadata: &EmbeddingMeta,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &metadata.owner_id, "embedding metadata")?;
        ensure!(metadata.dim > 0, "embedding dimension must be positive");
        ensure!(
            metadata.preprocess_version > 0,
            "preprocess version must be positive"
        );
        self.connection.execute(
            "INSERT INTO embedding_meta (
                owner_id, model_name, model_sha256, dim, preprocess_version
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(owner_id) DO UPDATE SET
                model_name = excluded.model_name,
                model_sha256 = excluded.model_sha256,
                dim = excluded.dim,
                preprocess_version = excluded.preprocess_version",
            params![
                owner_id,
                metadata.model_name,
                metadata.model_sha256,
                metadata.dim as i64,
                metadata.preprocess_version,
            ],
        )?;
        Ok(())
    }

    pub fn delete_video_cascade(&mut self, owner_id: &str, video_id: &str) -> anyhow::Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let fts_rows = {
            let mut statement = transaction.prepare(
                "SELECT rowid, text FROM transcripts WHERE owner_id = ?1 AND video_id = ?2",
            )?;
            let rows = statement.query_map(params![owner_id, video_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (rowid, text) in fts_rows {
            transaction.execute(
                "INSERT INTO transcripts_fts(transcripts_fts, rowid, text)
                 VALUES ('delete', ?1, ?2)",
                params![rowid, text],
            )?;
        }
        let changed = transaction.execute(
            "DELETE FROM videos WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, video_id],
        )?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    /// Scan all owners because `doctor --deep` validates the complete local database.
    pub fn integrity(&self) -> anyhow::Result<Vec<Problem>> {
        let mut problems = Vec::new();

        collect_string_pairs(
            &self.connection,
            "SELECT s.id, s.video_id
             FROM shots AS s
             JOIN videos AS v ON v.id = s.video_id AND v.owner_id = s.owner_id
             LEFT JOIN shot_vectors AS sv ON sv.shot_id = s.id AND sv.owner_id = s.owner_id
             WHERE v.status IN ('embedded', 'transcribed', 'done') AND sv.shot_id IS NULL
             ORDER BY s.id",
            |shot_id, video_id| Problem {
                kind: ProblemKind::MissingVector,
                entity_id: shot_id,
                detail: format!("embedded video {video_id} has a shot without a vector"),
            },
            &mut problems,
        )?;

        collect_string_pairs(
            &self.connection,
            "SELECT p.id, p.status
             FROM photos AS p
             LEFT JOIN photo_vectors AS pv ON pv.photo_id = p.id AND pv.owner_id = p.owner_id
             WHERE p.status IN ('embedded', 'done') AND pv.photo_id IS NULL
             ORDER BY p.id",
            |photo_id, status| Problem {
                kind: ProblemKind::MissingVector,
                entity_id: photo_id,
                detail: format!("{status} photo has no vector"),
            },
            &mut problems,
        )?;

        let mut statement = self
            .connection
            .prepare("SELECT id, thumb_rel FROM shots WHERE thumb_rel IS NOT NULL ORDER BY id")?;
        let thumbs = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for thumb in thumbs {
            let (shot_id, relative) = thumb?;
            let relative_path = Path::new(&relative);
            if !safe_relative_path(relative_path) {
                problems.push(Problem {
                    kind: ProblemKind::UnsafeThumbnailPath,
                    entity_id: shot_id,
                    detail: format!("thumbnail path is not a safe relative path: {relative}"),
                });
            } else {
                let path = self.data_dir.join("thumbs").join(relative_path);
                if !path.is_file() {
                    problems.push(Problem {
                        kind: ProblemKind::MissingThumbnail,
                        entity_id: shot_id,
                        detail: format!("thumbnail does not exist: {}", path.display()),
                    });
                }
            }
        }
        drop(statement);

        let mut statement = self
            .connection
            .prepare("SELECT id, thumb_rel FROM photos WHERE thumb_rel IS NOT NULL ORDER BY id")?;
        let thumbs = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for thumb in thumbs {
            let (photo_id, relative) = thumb?;
            let relative_path = Path::new(&relative);
            if !safe_relative_path(relative_path) {
                problems.push(Problem {
                    kind: ProblemKind::UnsafeThumbnailPath,
                    entity_id: photo_id,
                    detail: format!("photo thumbnail path is not a safe relative path: {relative}"),
                });
            } else {
                let path = self.data_dir.join("thumbs").join(relative_path);
                if !path.is_file() {
                    problems.push(Problem {
                        kind: ProblemKind::MissingThumbnail,
                        entity_id: photo_id,
                        detail: format!("thumbnail does not exist: {}", path.display()),
                    });
                }
            }
        }
        drop(statement);

        let mut statement = self.connection.prepare(
            "SELECT 'photo', photo_id, proxy_rel FROM photo_source_metadata WHERE proxy_rel IS NOT NULL
             UNION ALL
             SELECT 'video', video_id, proxy_rel FROM video_source_metadata WHERE proxy_rel IS NOT NULL
             ORDER BY 1, 2",
        )?;
        let proxies = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for proxy in proxies {
            let (kind, id, relative) = proxy?;
            let relative_path = Path::new(&relative);
            if !safe_relative_path(relative_path) {
                problems.push(Problem {
                    kind: ProblemKind::UnsafeProxyPath,
                    entity_id: id,
                    detail: format!("{kind} proxy path is not a safe relative path: {relative}"),
                });
            } else {
                let path = self.data_dir.join("proxies").join(relative_path);
                if !path.is_file() {
                    problems.push(Problem {
                        kind: ProblemKind::MissingProxy,
                        entity_id: id,
                        detail: format!("{kind} proxy does not exist: {}", path.display()),
                    });
                }
            }
        }
        drop(statement);

        collect_string_pairs(
            &self.connection,
            "SELECT sv.shot_id, sv.owner_id
             FROM shot_vectors AS sv
             LEFT JOIN shots AS s ON s.id = sv.shot_id AND s.owner_id = sv.owner_id
             WHERE s.id IS NULL
             ORDER BY sv.shot_id",
            |shot_id, owner_id| Problem {
                kind: ProblemKind::OrphanVector,
                entity_id: shot_id,
                detail: format!("vector for owner {owner_id} has no matching shot"),
            },
            &mut problems,
        )?;

        collect_string_pairs(
            &self.connection,
            "SELECT pv.photo_id, pv.owner_id
             FROM photo_vectors AS pv
             LEFT JOIN photos AS p ON p.id = pv.photo_id AND p.owner_id = pv.owner_id
             WHERE p.id IS NULL
             ORDER BY pv.photo_id",
            |photo_id, owner_id| Problem {
                kind: ProblemKind::OrphanVector,
                entity_id: photo_id,
                detail: format!("vector for owner {owner_id} has no matching photo"),
            },
            &mut problems,
        )?;

        let mut statement = self.connection.prepare(
            "SELECT shot_id, dim, length(vec)
             FROM shot_vectors
             WHERE length(vec) != dim * 4
             ORDER BY shot_id",
        )?;
        let invalid = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for item in invalid {
            let (shot_id, dim, byte_len) = item?;
            problems.push(Problem {
                kind: ProblemKind::InvalidVectorBytes,
                entity_id: shot_id,
                detail: format!("dim {dim} requires {} bytes, found {byte_len}", dim * 4),
            });
        }
        drop(statement);

        let mut statement = self.connection.prepare(
            "SELECT photo_id, dim, length(vec)
             FROM photo_vectors
             WHERE length(vec) != dim * 4
             ORDER BY photo_id",
        )?;
        let invalid = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for item in invalid {
            let (photo_id, dim, byte_len) = item?;
            problems.push(Problem {
                kind: ProblemKind::InvalidVectorBytes,
                entity_id: photo_id,
                detail: format!("dim {dim} requires {} bytes, found {byte_len}", dim * 4),
            });
        }
        drop(statement);

        let mut statement = self.connection.prepare(
            "SELECT id, embedding_dim, length(embedding_weights)
             FROM style_profiles
             WHERE length(embedding_weights) % 4 != 0
                OR length(embedding_weights) != embedding_dim * 4
             ORDER BY id",
        )?;
        let invalid_weights = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for item in invalid_weights {
            let (profile_id, dim, byte_len) = item?;
            problems.push(Problem {
                kind: ProblemKind::InvalidVectorBytes,
                entity_id: profile_id.clone(),
                detail: format!(
                    "style profile {profile_id} with dim {dim} requires {} bytes, found {byte_len}",
                    dim * 4
                ),
            });
        }
        drop(statement);

        Ok(problems)
    }

    fn job_by_id(&self, owner_id: &str, job_id: &str) -> anyhow::Result<Option<JobRecord>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, video_id, photo_id, stage, status, started_at, finished_at,
                        duration_ms, error, debug_dir
                 FROM jobs
                 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, job_id],
                job_from_row,
            )
            .optional()
            .context("failed to query job by id")
    }

    fn complete_job(
        &self,
        owner_id: &str,
        job_id: &str,
        status: JobStatus,
        finished_at: DateTime<Utc>,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        let job = self
            .job_by_id(owner_id, job_id)?
            .with_context(|| format!("job {job_id} was not found for owner {owner_id}"))?;
        ensure!(
            job.status == JobStatus::Running,
            "job {job_id} is {:?}, not running",
            job.status
        );
        let duration_ms = finished_at
            .signed_duration_since(job.started_at)
            .num_milliseconds();
        ensure!(duration_ms >= 0, "job finish time precedes its start time");

        let changed = self.connection.execute(
            "UPDATE jobs
             SET status = ?3, finished_at = ?4, duration_ms = ?5, error = ?6
             WHERE owner_id = ?1 AND id = ?2 AND status = 'running'",
            params![
                owner_id,
                job_id,
                job_status_to_str(status),
                finished_at.to_rfc3339(),
                duration_ms,
                error,
            ],
        )?;
        ensure_changed(changed, "running job", job_id)
    }
}

fn configure_connection(connection: &Connection) -> anyhow::Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    ensure!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "SQLite refused WAL mode; active mode is {journal_mode}"
    );
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    ensure!(
        foreign_keys == 1,
        "SQLite foreign_keys pragma is not enabled"
    );
    Ok(())
}

fn apply_migrations(connection: &mut Connection) -> anyhow::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version INTEGER NOT NULL CHECK (version >= 0)
         ) STRICT;
         INSERT OR IGNORE INTO schema_version (singleton, version) VALUES (1, 0);",
    )?;
    let mut current: i64 = connection.query_row(
        "SELECT version FROM schema_version WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    ensure!(
        current <= CURRENT_SCHEMA_VERSION,
        "database schema version {current} is newer than supported version {CURRENT_SCHEMA_VERSION}"
    );

    for &(version, migration) in MIGRATIONS {
        if version <= current {
            continue;
        }
        ensure!(
            version == current + 1,
            "migration sequence jumps from version {current} to {version}"
        );
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute_batch(migration)
            .with_context(|| format!("failed to apply schema migration {version}"))?;
        transaction.execute(
            "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
            params![version],
        )?;
        transaction.commit()?;
        tracing::info!(schema_version = version, "applied SQLite migration");
        current = version;
    }

    ensure!(
        current == CURRENT_SCHEMA_VERSION,
        "database stopped at schema version {current}; expected {CURRENT_SCHEMA_VERSION}"
    );
    Ok(())
}

fn video_from_row(row: &Row<'_>) -> rusqlite::Result<Video> {
    let status: String = row.get(9)?;
    let indexed_at: Option<String> = row.get(10)?;
    Ok(Video {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        path: row.get(2)?,
        sha256: row.get(3)?,
        duration_s: row.get(4)?,
        fps: row.get(5)?,
        width: row.get(6)?,
        height: row.get(7)?,
        has_audio: row.get(8)?,
        status: video_status_from_str(&status)
            .map_err(|error| conversion_message(9, error.to_string()))?,
        indexed_at: indexed_at
            .map(|value| timestamp_from_str(&value, 10))
            .transpose()?,
    })
}

fn photo_from_row(row: &Row<'_>) -> rusqlite::Result<Photo> {
    let captured_at: Option<String> = row.get(8)?;
    let status: String = row.get(13)?;
    let indexed_at: Option<String> = row.get(14)?;
    Ok(Photo {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        path: row.get(2)?,
        sha256: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        format: row.get(6)?,
        orientation: row.get(7)?,
        captured_at: captured_at
            .map(|value| timestamp_from_str(&value, 8))
            .transpose()?,
        camera_make: row.get(9)?,
        camera_model: row.get(10)?,
        lens: row.get(11)?,
        thumb_rel: row.get(12)?,
        status: photo_status_from_str(&status)
            .map_err(|error| conversion_message(13, error.to_string()))?,
        indexed_at: indexed_at
            .map(|value| timestamp_from_str(&value, 14))
            .transpose()?,
    })
}

fn photo_source_metadata_from_row(row: &Row<'_>) -> rusqlite::Result<PhotoSourceMetadata> {
    let provenance: String = row.get(8)?;
    let extracted_at: String = row.get(18)?;
    Ok(PhotoSourceMetadata {
        photo_id: row.get(0)?,
        owner_id: row.get(1)?,
        source_format: row.get(2)?,
        decoder: row.get(3)?,
        proxy_rel: row.get(4)?,
        proxy_width: row.get(5)?,
        proxy_height: row.get(6)?,
        proxy_sha256: row.get(7)?,
        proxy_provenance: photo_proxy_provenance_from_str(&provenance)
            .map_err(|error| conversion_message(8, error.to_string()))?,
        orientation_applied: row.get(9)?,
        bit_depth: row.get(10)?,
        color_space: row.get(11)?,
        icc_profile_name: row.get(12)?,
        icc_profile_sha256: row.get(13)?,
        exposure_json: row.get(14)?,
        gps_present: row.get(15)?,
        metadata_json: row.get(16)?,
        original_size_bytes: row.get(17)?,
        extracted_at: timestamp_from_str(&extracted_at, 18)?,
    })
}

fn video_source_metadata_from_row(row: &Row<'_>) -> rusqlite::Result<VideoSourceMetadata> {
    let probed_at: String = row.get(18)?;
    Ok(VideoSourceMetadata {
        video_id: row.get(0)?,
        owner_id: row.get(1)?,
        container: row.get(2)?,
        video_codec: row.get(3)?,
        codec_profile: row.get(4)?,
        pixel_format: row.get(5)?,
        bit_depth: row.get(6)?,
        color_space: row.get(7)?,
        color_primaries: row.get(8)?,
        color_transfer: row.get(9)?,
        color_range: row.get(10)?,
        rotation: row.get(11)?,
        proxy_rel: row.get(12)?,
        proxy_sha256: row.get(13)?,
        proxy_required: row.get(14)?,
        proxy_reason: row.get(15)?,
        original_size_bytes: row.get(16)?,
        metadata_json: row.get(17)?,
        probed_at: timestamp_from_str(&probed_at, 18)?,
    })
}

fn editorial_annotation_from_row(row: &Row<'_>) -> rusqlite::Result<EditorialAnnotation> {
    let media_kind: String = row.get(1)?;
    let updated_at: String = row.get(16)?;
    Ok(EditorialAnnotation {
        owner_id: row.get(0)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(1, error.to_string()))?,
        media_id: row.get(2)?,
        description: row.get(3)?,
        subjects: row.get(4)?,
        action: row.get(5)?,
        tags: row.get(6)?,
        quality: row.get(7)?,
        standout: row.get(8)?,
        usable: row.get(9)?,
        faces_visible: row.get(10)?,
        nametags_visible: row.get(11)?,
        blur_required: row.get(12)?,
        crop_x: row.get(13)?,
        grade_json: row.get(14)?,
        notes: row.get(15)?,
        updated_at: timestamp_from_str(&updated_at, 16)?,
    })
}

fn aesthetic_assessment_from_row(row: &Row<'_>) -> rusqlite::Result<AestheticAssessment> {
    let media_kind: String = row.get(1)?;
    let assessed_at: String = row.get(15)?;
    Ok(AestheticAssessment {
        owner_id: row.get(0)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(1, error.to_string()))?,
        media_id: row.get(2)?,
        sharpness: row.get(3)?,
        exposure: row.get(4)?,
        contrast: row.get(5)?,
        color_harmony: row.get(6)?,
        balance: row.get(7)?,
        subject_placement: row.get(8)?,
        negative_space: row.get(9)?,
        visual_clarity: row.get(10)?,
        technical_quality: row.get(16)?,
        blur_control: row.get(17)?,
        clipping_control: row.get(18)?,
        noise_control: row.get(19)?,
        compression_quality: row.get(20)?,
        resolution_quality: row.get(21)?,
        motion_stability: row.get(22)?,
        duplicate_confidence: row.get(23)?,
        composition_quality: row.get(24)?,
        hierarchy: row.get(25)?,
        leading_lines: row.get(26)?,
        symmetry: row.get(27)?,
        crop_potential: row.get(28)?,
        moment_story: row.get(29)?,
        expression: row.get(30)?,
        gesture: row.get(31)?,
        action: row.get(32)?,
        novelty: row.get(33)?,
        pacing: row.get(34)?,
        repetition_risk: row.get(35)?,
        overall: row.get(11)?,
        confidence: row.get(12)?,
        explanation_json: row.get(13)?,
        model_version: row.get(14)?,
        assessed_at: timestamp_from_str(&assessed_at, 15)?,
    })
}

fn feedback_event_from_row(row: &Row<'_>) -> rusqlite::Result<FeedbackEvent> {
    let media_kind: String = row.get(2)?;
    let signal: String = row.get(4)?;
    let compared_media_kind: Option<String> = row.get(6)?;
    let created_at: String = row.get(9)?;
    Ok(FeedbackEvent {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(2, error.to_string()))?,
        media_id: row.get(3)?,
        signal: feedback_signal_from_str(&signal)
            .map_err(|error| conversion_message(4, error.to_string()))?,
        value: row.get(5)?,
        compared_media_kind: compared_media_kind
            .map(|value| {
                media_kind_from_str(&value)
                    .map_err(|error| conversion_message(6, error.to_string()))
            })
            .transpose()?,
        compared_media_id: row.get(7)?,
        context_json: row.get(8)?,
        created_at: timestamp_from_str(&created_at, 9)?,
    })
}

fn style_profile_from_row(row: &Row<'_>) -> rusqlite::Result<StyleProfile> {
    let dim: i64 = row.get(5)?;
    let bytes: Vec<u8> = row.get(6)?;
    let trained_at: String = row.get(11)?;
    let id: String = row.get(0)?;
    let embedding_weights =
        decode_vector(dim, bytes, &id).map_err(|error| conversion_message(6, error.to_string()))?;
    Ok(StyleProfile {
        id,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        algorithm_version: row.get(4)?,
        embedding_weights,
        feature_weights_json: row.get(7)?,
        sample_count: row.get(8)?,
        held_out_metric: row.get(9)?,
        active: row.get(10)?,
        trained_at: timestamp_from_str(&trained_at, 11)?,
    })
}

fn shot_from_row(row: &Row<'_>) -> rusqlite::Result<Shot> {
    Ok(Shot {
        id: row.get(0)?,
        video_id: row.get(1)?,
        owner_id: row.get(2)?,
        idx: row.get(3)?,
        start_s: row.get(4)?,
        end_s: row.get(5)?,
        rep_frame_s: row.get(6)?,
        thumb_rel: row.get(7)?,
        scene_score: row.get(8)?,
    })
}

fn transcript_from_row(row: &Row<'_>) -> rusqlite::Result<TranscriptSegment> {
    Ok(TranscriptSegment {
        id: row.get(0)?,
        video_id: row.get(1)?,
        owner_id: row.get(2)?,
        start_s: row.get(3)?,
        end_s: row.get(4)?,
        text: row.get(5)?,
        confidence: row.get(6)?,
    })
}

fn job_from_row(row: &Row<'_>) -> rusqlite::Result<JobRecord> {
    let stage: String = row.get(4)?;
    let status: String = row.get(5)?;
    let started_at: String = row.get(6)?;
    let finished_at: Option<String> = row.get(7)?;
    Ok(JobRecord {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        video_id: row.get(2)?,
        photo_id: row.get(3)?,
        stage: stage_from_str(&stage).map_err(|error| conversion_message(4, error.to_string()))?,
        status: job_status_from_str(&status)
            .map_err(|error| conversion_message(5, error.to_string()))?,
        started_at: timestamp_from_str(&started_at, 6)?,
        finished_at: finished_at
            .map(|value| timestamp_from_str(&value, 7))
            .transpose()?,
        duration_ms: row.get(8)?,
        error: row.get(9)?,
        debug_dir: row.get(10)?,
    })
}

fn video_status_to_str(status: VideoStatus) -> &'static str {
    match status {
        VideoStatus::Pending => "pending",
        VideoStatus::Split => "split",
        VideoStatus::Embedded => "embedded",
        VideoStatus::Transcribed => "transcribed",
        VideoStatus::Done => "done",
        VideoStatus::Failed => "failed",
    }
}

fn video_status_from_str(value: &str) -> anyhow::Result<VideoStatus> {
    match value {
        "pending" => Ok(VideoStatus::Pending),
        "split" => Ok(VideoStatus::Split),
        "embedded" => Ok(VideoStatus::Embedded),
        "transcribed" => Ok(VideoStatus::Transcribed),
        "done" => Ok(VideoStatus::Done),
        "failed" => Ok(VideoStatus::Failed),
        _ => bail!("unknown video status {value:?}"),
    }
}

fn photo_status_to_str(status: PhotoStatus) -> &'static str {
    match status {
        PhotoStatus::Pending => "pending",
        PhotoStatus::Embedded => "embedded",
        PhotoStatus::Done => "done",
        PhotoStatus::Failed => "failed",
    }
}

fn photo_status_from_str(value: &str) -> anyhow::Result<PhotoStatus> {
    match value {
        "pending" => Ok(PhotoStatus::Pending),
        "embedded" => Ok(PhotoStatus::Embedded),
        "done" => Ok(PhotoStatus::Done),
        "failed" => Ok(PhotoStatus::Failed),
        _ => bail!("unknown photo status {value:?}"),
    }
}

fn photo_proxy_provenance_to_str(value: PhotoProxyProvenance) -> &'static str {
    match value {
        PhotoProxyProvenance::DecodedOriginal => "decoded_original",
        PhotoProxyProvenance::FullRender => "full_render",
        PhotoProxyProvenance::EmbeddedPreview => "embedded_preview",
    }
}

fn photo_proxy_provenance_from_str(value: &str) -> anyhow::Result<PhotoProxyProvenance> {
    match value {
        "decoded_original" => Ok(PhotoProxyProvenance::DecodedOriginal),
        "full_render" => Ok(PhotoProxyProvenance::FullRender),
        "embedded_preview" => Ok(PhotoProxyProvenance::EmbeddedPreview),
        other => bail!("unknown photo proxy provenance {other:?}"),
    }
}

fn media_kind_to_str(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Shot => "shot",
    }
}

fn media_kind_from_str(value: &str) -> anyhow::Result<MediaKind> {
    match value {
        "photo" => Ok(MediaKind::Photo),
        "shot" => Ok(MediaKind::Shot),
        _ => bail!("unknown media kind {value:?}"),
    }
}

fn feedback_signal_to_str(signal: FeedbackSignal) -> &'static str {
    match signal {
        FeedbackSignal::Pick => "pick",
        FeedbackSignal::Reject => "reject",
        FeedbackSignal::Rating => "rating",
        FeedbackSignal::Prefer => "prefer",
        FeedbackSignal::Crop => "crop",
        FeedbackSignal::Grade => "grade",
        FeedbackSignal::Export => "export",
        FeedbackSignal::Publish => "publish",
        FeedbackSignal::Tag => "tag",
        FeedbackSignal::Edit => "edit",
    }
}

fn feedback_signal_from_str(value: &str) -> anyhow::Result<FeedbackSignal> {
    match value {
        "pick" => Ok(FeedbackSignal::Pick),
        "reject" => Ok(FeedbackSignal::Reject),
        "rating" => Ok(FeedbackSignal::Rating),
        "prefer" => Ok(FeedbackSignal::Prefer),
        "crop" => Ok(FeedbackSignal::Crop),
        "grade" => Ok(FeedbackSignal::Grade),
        "export" => Ok(FeedbackSignal::Export),
        "publish" => Ok(FeedbackSignal::Publish),
        "tag" => Ok(FeedbackSignal::Tag),
        "edit" => Ok(FeedbackSignal::Edit),
        _ => bail!("unknown feedback signal {value:?}"),
    }
}

fn stage_to_str(stage: Stage) -> &'static str {
    match stage {
        Stage::Split => "split",
        Stage::Embed => "embed",
        Stage::Analyze => "analyze",
        Stage::Transcribe => "transcribe",
        Stage::PhotoIngest => "photo_ingest",
    }
}

fn stage_from_str(value: &str) -> anyhow::Result<Stage> {
    match value {
        "split" => Ok(Stage::Split),
        "embed" => Ok(Stage::Embed),
        "analyze" => Ok(Stage::Analyze),
        "transcribe" => Ok(Stage::Transcribe),
        "photo_ingest" => Ok(Stage::PhotoIngest),
        _ => bail!("unknown job stage {value:?}"),
    }
}

fn job_status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn job_status_from_str(value: &str) -> anyhow::Result<JobStatus> {
    match value {
        "queued" => Ok(JobStatus::Queued),
        "running" => Ok(JobStatus::Running),
        "done" => Ok(JobStatus::Done),
        "failed" => Ok(JobStatus::Failed),
        "cancelled" => Ok(JobStatus::Cancelled),
        _ => bail!("unknown job status {value:?}"),
    }
}

fn timestamp_from_str(value: &str, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| conversion_error(column, error))
}

fn conversion_error(
    column: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
}

fn conversion_message(column: usize, message: String) -> rusqlite::Error {
    conversion_error(
        column,
        std::io::Error::new(std::io::ErrorKind::InvalidData, message),
    )
}

fn ensure_owner_matches(owner_id: &str, record_owner: &str, kind: &str) -> anyhow::Result<()> {
    ensure!(
        owner_id == record_owner,
        "{kind} owner {record_owner:?} does not match requested owner {owner_id:?}"
    );
    Ok(())
}

fn ensure_changed(changed: usize, kind: &str, id: &str) -> anyhow::Result<()> {
    ensure!(changed == 1, "{kind} {id} was not found");
    Ok(())
}

/// Inlines the `job_fail` and `set_video_status`/`set_photo_status` writes for one
/// interrupted pass so they can share a single transaction; statuses, error text, and counts
/// match the standalone APIs.
fn mark_jobs_interrupted(
    connection: &Connection,
    owner_id: &str,
    finished_at: DateTime<Utc>,
    jobs: &[JobRecord],
) -> anyhow::Result<()> {
    for job in jobs {
        let duration_ms = finished_at
            .signed_duration_since(job.started_at)
            .num_milliseconds();
        ensure!(duration_ms >= 0, "job finish time precedes its start time");
        let changed = connection.execute(
            "UPDATE jobs
             SET status = 'failed', finished_at = ?3, duration_ms = ?4, error = ?5
             WHERE owner_id = ?1 AND id = ?2 AND status = 'running'",
            params![
                owner_id,
                job.id,
                finished_at.to_rfc3339(),
                duration_ms,
                "interrupted",
            ],
        )?;
        ensure_changed(changed, "running job", &job.id)?;
        connection.execute(
            "UPDATE videos SET status = 'failed' WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, job.video_id],
        )?;
        connection.execute(
            "UPDATE photos SET status = 'failed' WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, job.photo_id],
        )?;
    }
    Ok(())
}

fn validate_shot(shot: &Shot) -> anyhow::Result<()> {
    ensure!(shot.idx >= 0, "shot index must be non-negative");
    ensure!(shot.start_s >= 0.0, "shot start must be non-negative");
    ensure!(shot.end_s > shot.start_s, "shot end must exceed start");
    ensure!(
        shot.rep_frame_s >= shot.start_s && shot.rep_frame_s <= shot.end_s,
        "representative frame must be within the shot"
    );
    if let Some(relative) = &shot.thumb_rel {
        ensure!(
            safe_relative_path(Path::new(relative)),
            "thumbnail path must be a safe path relative to the thumbs directory"
        );
    }
    Ok(())
}

fn validate_photo(photo: &Photo) -> anyhow::Result<()> {
    ensure!(photo.width > 0, "photo width must be positive");
    ensure!(photo.height > 0, "photo height must be positive");
    ensure!(!photo.format.trim().is_empty(), "photo format is required");
    if let Some(orientation) = photo.orientation {
        ensure!(
            (1..=8).contains(&orientation),
            "EXIF orientation must be 1 through 8"
        );
    }
    if let Some(relative) = &photo.thumb_rel {
        ensure!(
            safe_relative_path(Path::new(relative)),
            "thumbnail path must be a safe path relative to the thumbs directory"
        );
    }
    Ok(())
}

fn validate_photo_source_metadata(metadata: &PhotoSourceMetadata) -> anyhow::Result<()> {
    ensure!(
        metadata.proxy_provenance != PhotoProxyProvenance::EmbeddedPreview,
        "embedded_preview provenance is not producible by any pipeline decoder; thumbnails must never be recorded as full decodes"
    );
    ensure!(
        !metadata.source_format.trim().is_empty(),
        "source format is required"
    );
    ensure!(
        !metadata.decoder.trim().is_empty(),
        "photo decoder is required"
    );
    ensure!(
        metadata.original_size_bytes >= 0,
        "original size must be non-negative"
    );
    validate_json_object(&metadata.exposure_json, "exposure_json")?;
    validate_json_object(&metadata.metadata_json, "metadata_json")?;
    validate_proxy_fields(
        metadata.proxy_rel.as_deref(),
        metadata.proxy_sha256.as_deref(),
        metadata.proxy_width,
        metadata.proxy_height,
    )?;
    if let Some(bit_depth) = metadata.bit_depth {
        ensure!(bit_depth > 0, "photo bit depth must be positive");
    }
    Ok(())
}

fn validate_video_source_metadata(metadata: &VideoSourceMetadata) -> anyhow::Result<()> {
    ensure!(
        !metadata.container.trim().is_empty(),
        "video container is required"
    );
    ensure!(
        !metadata.video_codec.trim().is_empty(),
        "video codec is required"
    );
    ensure!(
        metadata.original_size_bytes >= 0,
        "original size must be non-negative"
    );
    validate_json_object(&metadata.metadata_json, "metadata_json")?;
    if let Some(bit_depth) = metadata.bit_depth {
        ensure!(bit_depth > 0, "video bit depth must be positive");
    }
    match (&metadata.proxy_rel, &metadata.proxy_sha256) {
        (Some(relative), Some(hash)) => {
            ensure!(
                safe_relative_path(Path::new(relative)),
                "proxy path must be a safe relative path"
            );
            ensure!(
                !hash.trim().is_empty(),
                "proxy SHA-256 is required when a proxy path is set"
            );
        }
        (None, None) => {}
        _ => bail!("video proxy path and SHA-256 must either both be set or both be absent"),
    }
    ensure!(
        metadata.proxy_required || metadata.proxy_reason.is_none(),
        "proxy reason requires proxy_required"
    );
    Ok(())
}

fn validate_proxy_fields(
    relative: Option<&str>,
    sha256: Option<&str>,
    width: Option<i64>,
    height: Option<i64>,
) -> anyhow::Result<()> {
    match (relative, sha256, width, height) {
        (Some(relative), Some(hash), Some(width), Some(height)) => {
            ensure!(
                safe_relative_path(Path::new(relative)),
                "proxy path must be a safe relative path"
            );
            ensure!(!hash.trim().is_empty(), "proxy SHA-256 is required");
            ensure!(width > 0 && height > 0, "proxy dimensions must be positive");
        }
        (None, None, None, None) => {}
        _ => bail!("photo proxy path, SHA-256, width, and height must be set together"),
    }
    Ok(())
}

fn validate_json_object(value: &str, name: &str) -> anyhow::Result<()> {
    let parsed: serde_json::Value =
        serde_json::from_str(value).with_context(|| format!("{name} must be valid JSON"))?;
    ensure!(parsed.is_object(), "{name} must be a JSON object");
    Ok(())
}

fn ensure_unit_score(value: f64, name: &str) -> anyhow::Result<()> {
    ensure!(
        value.is_finite() && (0.0..=1.0).contains(&value),
        "{name} must be finite and between 0 and 1"
    );
    Ok(())
}

fn vector_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_vector(dim: i64, bytes: Vec<u8>, media_id: &str) -> anyhow::Result<Vec<f32>> {
    let dim = usize::try_from(dim).context("vector dimension was negative")?;
    let expected_bytes = dim
        .checked_mul(size_of::<f32>())
        .context("vector byte length overflowed usize")?;
    ensure!(
        bytes.len() == expected_bytes,
        "vector {media_id} contains {} bytes; expected {expected_bytes} for dim {dim}",
        bytes.len()
    );
    Ok(bytes
        .as_chunks::<{ size_of::<f32>() }>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn collect_string_pairs(
    connection: &Connection,
    sql: &str,
    problem: impl Fn(String, String) -> Problem,
    output: &mut Vec<Problem>,
) -> anyhow::Result<()> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (first, second) = row?;
        output.push(problem(first, second));
    }
    Ok(())
}
