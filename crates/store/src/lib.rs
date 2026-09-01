//! SQLite persistence for Crush.
//!
//! This crate is the only place in the product that contains SQL. It owns migrations, typed
//! records, vector serialization, FTS synchronization, job history, and deep integrity checks.

use std::{
    collections::HashSet,
    convert::TryFrom,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, Utc};
use crush_core::{job::JobRecord, job::JobStatus, job::Stage};
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row, TransactionBehavior};

const CURRENT_SCHEMA_VERSION: i64 = 13;
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_dam_feedback.sql")),
    (3, include_str!("../migrations/0003_source_fidelity.sql")),
    (4, include_str!("../migrations/0004_strong_shot.sql")),
    (5, include_str!("../migrations/0005_feedback_hardening.sql")),
    (6, include_str!("../migrations/0006_photo_jobs.sql")),
    (7, include_str!("../migrations/0007_reference_sets.sql")),
    (8, include_str!("../migrations/0008_collections.sql")),
    (9, include_str!("../migrations/0009_plans.sql")),
    (10, include_str!("../migrations/0010_rendering.sql")),
    (
        11,
        include_str!("../migrations/0011_reel_studio_import.sql"),
    ),
    (
        12,
        include_str!("../migrations/0012_span_item_video_range.sql"),
    ),
    (
        13,
        include_str!("../migrations/0013_span_reference_evidence.sql"),
    ),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum MediaKind {
    Photo,
    Shot,
    /// An imported or manual video span (Task 022); references `manual_spans`, never `shots`.
    Span,
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
    /// Non-personalized baseline accuracy measured on the same held-out split.
    pub baseline_metric: Option<f64>,
    pub context_key: String,
    pub metrics_json: String,
    pub learned: bool,
    pub active: bool,
    pub trained_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSetScope {
    WholeSet,
    Selected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSetStatus {
    Unconfirmed,
    Confirmed,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceItemRole {
    Positive,
    Excluded,
}

/// A named, context-scoped collection of previous-work examples the owner designated as style
/// evidence. Uncurated sets are inert: the trainer reads items only through confirmed sets.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSet {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub context_key: String,
    pub description: String,
    pub scope: ReferenceSetScope,
    pub status: ReferenceSetStatus,
    /// Reserved for TASK-019 collection designation.
    pub source_collection_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSetItem {
    pub owner_id: String,
    pub set_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    pub role: ReferenceItemRole,
    pub added_at: DateTime<Utc>,
}

/// An owner-scoped named grouping of photos and shots. Purely organizational: a collection
/// carries no training meaning until it is explicitly designated as a reference set.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectionItem {
    pub owner_id: String,
    pub collection_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    /// Optional per-item context key; `None` inherits the collection/set level.
    pub context_key: Option<String>,
    /// User-marked example for `selected`-scope designation.
    pub marked: bool,
    pub added_at: DateTime<Utc>,
}

/// The media a version stack groups: photos and whole videos (shots stay scene units).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackMediaKind {
    Photo,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackItemRole {
    Original,
    Derived,
}

/// One original plus its derived/alternate versions. Metadata only; underlying media rows are
/// never mutated by stack APIs.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionStack {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StackItem {
    pub owner_id: String,
    pub stack_id: String,
    pub media_kind: StackMediaKind,
    pub media_id: String,
    pub role: StackItemRole,
    pub added_at: DateTime<Utc>,
}

/// A persisted `(query, context_key, filters)` triple the UI can replay through search and
/// [`Store::browse_assets`] without changing ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSearch {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub query: String,
    pub context_key: String,
    pub filters_json: String,
    pub created_at: DateTime<Utc>,
}

/// Where a plan item's rank came from. `General` is the cold-start strong-shot model;
/// `Personal` carries the exact style-profile version that produced the rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PlanOrigin {
    General,
    Personal,
    /// A prior human choice reproduced from an imported recipe/project (Task 022).
    Historical,
    /// A catalogue-driven selection imported from another tool (Task 022).
    Imported,
}

/// An owner-scoped editorial document: an ordered, editable selection of photos and video
/// clips for a deliverable. Plans are mutable state — writing one never appends a feedback
/// event ([`Store::plan_create`] and friends write `plans`/`plan_items` only).
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub description: String,
    pub context_key: String,
    pub brief: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One entry in a plan. Video items carry boundary-safe `start_s`/`end_s` points that must
/// stay inside the source shot, plus pacing, crop, and grade treatment. Every item records
/// its provenance (origin, rank, and profile version) and freezes the explainability signals
/// observed when it was chosen in `signals_json`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlanItem {
    pub owner_id: String,
    pub plan_id: String,
    pub media_kind: MediaKind,
    pub media_id: String,
    /// Dense 0-based sequence position within the plan; the store APIs keep positions 0..n.
    pub position: i64,
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub pacing: Option<f64>,
    pub crop_x: Option<f64>,
    pub grade_json: Option<String>,
    pub reason: String,
    pub signals_json: String,
    pub origin: PlanOrigin,
    pub rank: Option<f64>,
    pub profile_version: Option<i64>,
    /// `{}` for general/personal items. Historical/imported items carry
    /// `{source, external_id, import_id, boundary_basis, boundary_tolerance_s}`. Span items
    /// whose boundaries were moved away from the imported span's gain a derived
    /// `adjusted: true` + `adjusted_at` (Task 037); the store writes and clears the marker
    /// itself so it cannot drift from the stored boundaries or be spoofed by a caller.
    pub provenance_json: String,
    pub added_at: DateTime<Utc>,
}

/// Field-wise edit for [`Store::plan_update_item`]; `None` leaves the field unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanItemPatch {
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub pacing: Option<f64>,
    pub crop_x: Option<f64>,
    pub grade_json: Option<String>,
    pub reason: Option<String>,
}

/// One append-only revision snapshot of a plan (header plus items) saved through
/// [`Store::plan_save_revision`] and restorable through [`Store::plan_restore_revision`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRevision {
    pub owner_id: String,
    pub plan_id: String,
    pub revision: i64,
    pub label: String,
    pub snapshot_json: String,
    pub created_at: DateTime<Utc>,
}

/// The media contract described by an immutable version of a render recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRecipeKind {
    Photo,
    VideoClip,
    Reel,
}

/// One immutable version of a non-destructive render recipe. `schema_json` is validated on
/// insertion and is frozen again into every queued job, so later recipe versions cannot change
/// work that is already queued.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRecipe {
    pub owner_id: String,
    pub id: String,
    pub version: i64,
    pub kind: RenderRecipeKind,
    pub name: String,
    pub schema_json: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderJobStatus {
    Queued,
    Running,
    Verifying,
    Done,
    Failed,
    Cancelled,
}

/// Input for queueing a render. The recipe and optional plan revision are looked up under the
/// supplied owner and frozen by [`Store::render_job_create`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewRenderJob {
    pub id: String,
    pub recipe_id: String,
    pub recipe_version: i64,
    pub plan_id: Option<String>,
    pub plan_revision: Option<i64>,
    pub source_snapshot_json: String,
    pub model_versions_json: String,
    pub destination_path: String,
    pub created_at: DateTime<Utc>,
}

/// Durable render state. Source, recipe, and plan snapshots are immutable after creation; only
/// lifecycle fields change while an attempt runs.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderJob {
    pub owner_id: String,
    pub id: String,
    pub recipe_id: String,
    pub recipe_version: i64,
    pub recipe_kind: RenderRecipeKind,
    pub frozen_recipe_json: String,
    pub plan_id: Option<String>,
    pub plan_revision: Option<i64>,
    pub frozen_plan_json: Option<String>,
    pub source_snapshot_json: String,
    pub model_versions_json: String,
    pub destination_path: String,
    pub status: RenderJobStatus,
    pub progress: f64,
    pub current_attempt: i64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderAttempt {
    pub owner_id: String,
    pub job_id: String,
    pub attempt: i64,
    pub status: RenderJobStatus,
    pub staging_path: String,
    pub progress: f64,
    pub command_json: String,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Verified output facts and its immutable, separately checksummed manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutput {
    pub owner_id: String,
    pub id: String,
    pub job_id: String,
    pub attempt: i64,
    pub output_path: String,
    pub output_sha256: String,
    pub size_bytes: i64,
    pub media_type: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_s: Option<f64>,
    pub verification_json: String,
    pub manifest_path: String,
    pub manifest_json: String,
    pub manifest_sha256: String,
    pub created_at: DateTime<Utc>,
}

/// A strong-shot row read through the `aesthetic_assessments_strongest` index: the general
/// cold-start ranking of candidate assets before any personalization.
#[derive(Debug, Clone, PartialEq)]
pub struct StrongAsset {
    pub media_kind: MediaKind,
    pub media_id: String,
    pub overall: f64,
    pub confidence: f64,
}

/// The safety columns of an editorial annotation. Writable only through
/// [`Store::set_safety_flags`] (or a review op) after an explicit user action; machine paths
/// have no API that writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyFlags {
    pub usable: bool,
    pub faces_visible: bool,
    pub nametags_visible: bool,
    pub blur_required: bool,
}

impl Default for SafetyFlags {
    fn default() -> Self {
        Self {
            usable: true,
            faces_visible: false,
            nametags_visible: false,
            blur_required: false,
        }
    }
}

/// Filters for the unified mixed-media grid. `None` fields are wide open; annotation-derived
/// booleans match the annotation defaults for assets without an annotation row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetFilter {
    pub kind: Option<MediaKind>,
    pub status: Option<String>,
    pub usable: Option<bool>,
    pub faces_visible: Option<bool>,
    pub blur_required: Option<bool>,
    pub quality_min: Option<i64>,
    /// Match assets with at least one recorded feedback event of this signal
    /// ("pick", "reject", or "rating"). Append-only; a later opposite event does not yet
    /// reverse earlier history, matching the tile's event counts.
    pub feedback: Option<String>,
    pub collection_id: Option<String>,
    pub stack_id: Option<String>,
    pub context_key: Option<String>,
    /// Case-insensitive file-name substring over the photo path or the parent video path.
    pub search: Option<String>,
}

/// One row of the unified library grid: a photo or a shot with its parent video, annotation
/// summary, and organizational memberships.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryAsset {
    pub media_kind: MediaKind,
    pub media_id: String,
    pub owner_id: String,
    /// The photo path, or the parent video's path for shots.
    pub path: String,
    pub thumb_rel: Option<String>,
    /// The photo status, or the parent video's status for shots.
    pub status: String,
    pub indexed_at: Option<DateTime<Utc>>,
    /// Shot parent; `None` for photos.
    pub video_id: Option<String>,
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub quality: Option<i64>,
    pub usable: bool,
    pub standout: bool,
    pub faces_visible: bool,
    pub nametags_visible: bool,
    pub blur_required: bool,
    pub tags: String,
    pub collection_ids: Vec<String>,
    pub stack_ids: Vec<String>,
    /// Catalogue provenance for imported spans (Task 034); `None` for photos and shots.
    /// `source` is `reel_studio` or `manual`; `import_id` links the span to its import
    /// ledger row so Review pills can say where the evidence came from.
    pub source: Option<String>,
    pub external_id: Option<String>,
    pub import_id: Option<String>,
    pub imported_at: Option<DateTime<Utc>>,
}

/// Dashboard counters for the library view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LibraryCounts {
    pub photos: i64,
    pub shots: i64,
    pub picks: i64,
    pub rejects: i64,
    /// Annotations flagged unsafe: `usable = 0` or `blur_required = 1`.
    pub flagged: i64,
}

/// One explicit user review action for [`Store::bulk_review`]. Every op runs inside one
/// transaction: a bad op aborts the whole batch.
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewOp {
    Pick {
        media_kind: MediaKind,
        media_id: String,
    },
    Reject {
        media_kind: MediaKind,
        media_id: String,
    },
    Rate {
        media_kind: MediaKind,
        media_id: String,
        rating: i64,
    },
    SetFlags {
        media_kind: MediaKind,
        media_id: String,
        flags: SafetyFlags,
    },
    AddToCollection {
        collection_id: String,
        media_kind: MediaKind,
        media_id: String,
        context_key: Option<String>,
    },
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

/// The mutable columns of a stored shot row, used by `replace_shots` to decide whether a
/// surviving shot actually changed before writing it back in place.
#[derive(Debug, Clone, PartialEq)]
struct StoredShot {
    id: String,
    idx: i64,
    start_s: f64,
    end_s: f64,
    rep_frame_s: f64,
    thumb_rel: Option<String>,
    scene_score: Option<f64>,
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

/// One text-match hit from the span catalogue FTS index (Task 034). Spans have no embedding
/// vectors, so these hits are always TEXT-MATCH-ONLY results beside the semantic ranking —
/// never a cosine score. The matched catalogue text and the full provenance travel with the
/// hit so no consumer can mistake it for an embedded asset.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanTextHit {
    pub span_id: String,
    pub video_id: String,
    pub video_path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub description: String,
    pub subjects: String,
    pub action: String,
    pub tags: String,
    pub shot_type: String,
    pub camera_move: String,
    /// `reel_studio` or `manual` — the catalogue provenance, exported verbatim.
    pub source: String,
    pub external_id: String,
    pub import_id: Option<String>,
    pub imported_at: DateTime<Utc>,
    /// The catalogue text that matched, for an honest snippet without fabricating content.
    pub matched_text: String,
    /// FTS5 bm25 rank: lower (more negative) is a better text match.
    pub rank: f64,
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
        let had_database = std::fs::metadata(&db_path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false);
        let mut connection = Connection::open(&db_path)
            .with_context(|| format!("failed to open SQLite database {}", db_path.display()))?;

        configure_connection(&connection)?;
        if had_database {
            write_pre_migration_snapshot(&connection, &data_dir, &db_path)?;
        }
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

    /// Cheap change marker for cache-reload decisions, e.g. the app's vector index.
    ///
    /// `PRAGMA data_version` is the simplest correct choice here: it stays stable while only
    /// this connection reads and changes whenever any other connection commits. Every Crush
    /// command opens its own short-lived `Store`, so comparing the value across commands
    /// detects every commit made in between — by the pipeline, other commands, `crushctl`,
    /// or another process — without tracking writers in Rust. Writes made on this same
    /// connection do not move the value, which is fine: callers that write (like a retrain)
    /// only touch non-vector tables, and the next command's fresh connection sees the bump.
    pub fn data_version(&self) -> anyhow::Result<i64> {
        self.connection
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .context("failed to read the database data version")
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

    pub fn delete_photo_vector(&self, owner_id: &str, photo_id: &str) -> anyhow::Result<()> {
        self.connection
            .execute(
                "DELETE FROM photo_vectors WHERE owner_id = ?1 AND photo_id = ?2",
                params![owner_id, photo_id],
            )
            .context("failed to delete photo vector")?;
        Ok(())
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
        upsert_editorial_annotation_on(&self.connection, owner_id, annotation)
    }

    pub fn editorial_annotation(
        &self,
        owner_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<Option<EditorialAnnotation>> {
        editorial_annotation_on(&self.connection, owner_id, media_kind, media_id)
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
        append_feedback_on(&self.connection, owner_id, event)
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
        ensure!(
            !profile.context_key.trim().is_empty(),
            "style profile context key must not be empty"
        );
        validate_json_object(&profile.metrics_json, "metrics_json")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if profile.active {
            transaction.execute(
                "UPDATE style_profiles SET active = 0 WHERE owner_id = ?1 AND context_key = ?2",
                params![owner_id, profile.context_key],
            )?;
        }
        transaction.execute(
            "INSERT INTO style_profiles (
                id, owner_id, name, version, algorithm_version, embedding_dim,
                embedding_weights, feature_weights_json, sample_count, held_out_metric, active,
                trained_at, context_key, baseline_metric, metrics_json, learned
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                trained_at = excluded.trained_at,
                context_key = excluded.context_key,
                baseline_metric = excluded.baseline_metric,
                metrics_json = excluded.metrics_json,
                learned = excluded.learned",
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
                profile.context_key,
                profile.baseline_metric,
                profile.metrics_json,
                i64::from(profile.learned),
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

    /// The active profile for the default context. Named contexts are read through
    /// [`Store::active_style_profile_for_context`].
    pub fn active_style_profile(&self, owner_id: &str) -> anyhow::Result<Option<StyleProfile>> {
        self.active_style_profile_for_context(owner_id, "default")
    }

    pub fn active_style_profile_for_context(
        &self,
        owner_id: &str,
        context_key: &str,
    ) -> anyhow::Result<Option<StyleProfile>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, version, algorithm_version, embedding_dim,
                        embedding_weights, feature_weights_json, sample_count, held_out_metric,
                        active, trained_at, context_key, baseline_metric, metrics_json, learned
                 FROM style_profiles
                 WHERE owner_id = ?1 AND active = 1 AND context_key = ?2",
                params![owner_id, context_key],
                style_profile_from_row,
            )
            .optional()
            .context("failed to read active style profile")
    }

    /// Every retained profile version, ordered for auditability; rows are never deleted.
    pub fn style_profiles(&self, owner_id: &str) -> anyhow::Result<Vec<StyleProfile>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, version, algorithm_version, embedding_dim,
                    embedding_weights, feature_weights_json, sample_count, held_out_metric,
                    active, trained_at, context_key, baseline_metric, metrics_json, learned
             FROM style_profiles WHERE owner_id = ?1
             ORDER BY context_key, version",
        )?;
        let rows = statement.query_map(params![owner_id], style_profile_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list style profiles")
    }

    pub fn style_profiles_for_context(
        &self,
        owner_id: &str,
        context_key: &str,
    ) -> anyhow::Result<Vec<StyleProfile>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, version, algorithm_version, embedding_dim,
                    embedding_weights, feature_weights_json, sample_count, held_out_metric,
                    active, trained_at, context_key, baseline_metric, metrics_json, learned
             FROM style_profiles WHERE owner_id = ?1 AND context_key = ?2
             ORDER BY version",
        )?;
        let rows = statement.query_map(params![owner_id, context_key], style_profile_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list style profiles for context")
    }

    /// Reversibly activate one retained version: the prior active row for the same
    /// (owner, context) is deactivated inside one transaction and nothing is deleted.
    pub fn activate_style_profile(
        &mut self,
        owner_id: &str,
        profile_id: &str,
    ) -> anyhow::Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let context_key: Option<String> = transaction
            .query_row(
                "SELECT context_key FROM style_profiles WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, profile_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read style profile for activation")?;
        let Some(context_key) = context_key else {
            return Ok(false);
        };
        transaction.execute(
            "UPDATE style_profiles SET active = 0
             WHERE owner_id = ?1 AND context_key = ?2 AND active = 1",
            params![owner_id, context_key],
        )?;
        transaction.execute(
            "UPDATE style_profiles SET active = 1 WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, profile_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Deactivate every profile for the owner. Reversible through
    /// [`Store::activate_style_profile`] or a retrain; returns the deactivated count.
    pub fn reset_style_profiles(&mut self, owner_id: &str) -> anyhow::Result<usize> {
        let changed = self
            .connection
            .execute(
                "UPDATE style_profiles SET active = 0 WHERE owner_id = ?1 AND active = 1",
                params![owner_id],
            )
            .context("failed to reset style profiles")?;
        Ok(changed)
    }

    pub fn reference_set_create(&self, owner_id: &str, set: &ReferenceSet) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &set.owner_id, "reference set")?;
        ensure!(
            !set.name.trim().is_empty(),
            "reference set name must not be empty"
        );
        ensure!(
            !set.context_key.trim().is_empty(),
            "reference set context key must not be empty"
        );
        self.connection
            .execute(
                "INSERT INTO reference_sets (
                    id, owner_id, name, context_key, description, scope, status,
                    source_collection_id, created_at, confirmed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    set.id,
                    owner_id,
                    set.name,
                    set.context_key,
                    set.description,
                    reference_scope_to_str(set.scope),
                    reference_status_to_str(set.status),
                    set.source_collection_id,
                    set.created_at.to_rfc3339(),
                    set.confirmed_at.map(|value| value.to_rfc3339()),
                ],
            )
            .context("failed to create reference set")?;
        Ok(())
    }

    pub fn reference_set_list(&self, owner_id: &str) -> anyhow::Result<Vec<ReferenceSet>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, context_key, description, scope, status,
                    source_collection_id, created_at, confirmed_at
             FROM reference_sets WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], reference_set_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list reference sets")
    }

    pub fn reference_set_get(
        &self,
        owner_id: &str,
        set_id: &str,
    ) -> anyhow::Result<Option<ReferenceSet>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, context_key, description, scope, status,
                        source_collection_id, created_at, confirmed_at
                 FROM reference_sets WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, set_id],
                reference_set_from_row,
            )
            .optional()
            .context("failed to read reference set")
    }

    /// Look a set up by its (owner-unique) name — used by the imported-evidence confirmation
    /// flow so re-confirming extends the existing set instead of failing the unique name.
    pub fn reference_set_by_name(
        &self,
        owner_id: &str,
        name: &str,
    ) -> anyhow::Result<Option<ReferenceSet>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, context_key, description, scope, status,
                        source_collection_id, created_at, confirmed_at
                 FROM reference_sets WHERE owner_id = ?1 AND name = ?2",
                params![owner_id, name],
                reference_set_from_row,
            )
            .optional()
            .context("failed to read reference set by name")
    }

    /// Flip a set to `confirmed`; confirmed sets are the only ones the trainer reads.
    pub fn reference_set_confirm(&mut self, owner_id: &str, set_id: &str) -> anyhow::Result<bool> {
        self.reference_set_set_status(owner_id, set_id, ReferenceSetStatus::Confirmed)
    }

    /// Mute a set without deleting it; the trainer stops reading its items. Deactivating a
    /// confirmed set also invalidates the affected context's active profile so withdrawn
    /// evidence stops influencing ranking (retrain-or-fallback).
    pub fn reference_set_disable(&mut self, owner_id: &str, set_id: &str) -> anyhow::Result<bool> {
        self.reference_set_set_status(owner_id, set_id, ReferenceSetStatus::Disabled)
    }

    fn reference_set_set_status(
        &mut self,
        owner_id: &str,
        set_id: &str,
        status: ReferenceSetStatus,
    ) -> anyhow::Result<bool> {
        // The status change and the profile invalidation it triggers must land in ONE
        // transaction: as separate autocommitted statements, a crash between them would
        // leave withdrawn evidence still influencing an active profile — the exact bug
        // class this withdrawal path exists to close.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Read the prior status and context BEFORE the update (consistent with delete) so
        // the invalidation decision uses the pre-mutation state.
        let prior: Option<(String, String)> = transaction
            .query_row(
                "SELECT status, context_key FROM reference_sets WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, set_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .context("failed to read reference set status")?;
        let changed = transaction
            .execute(
                "UPDATE reference_sets SET status = ?3, confirmed_at = ?4
                 WHERE owner_id = ?1 AND id = ?2",
                params![
                    owner_id,
                    set_id,
                    reference_status_to_str(status),
                    (status == ReferenceSetStatus::Confirmed).then(|| Utc::now().to_rfc3339())
                ],
            )
            .context("failed to update reference set status")?;
        // Withdrawing confirmed evidence must invalidate the profile it trained
        // (docs/review-2026-08-29.md finding 3): the trainer intentionally retains the
        // previous profile below the sample floor, so deactivation is what makes the ranker
        // fall back to the general model until a retrain re-proves learning.
        if changed == 1
            && status == ReferenceSetStatus::Disabled
            && prior.as_ref().is_some_and(|(prior_status, _)| {
                prior_status == reference_status_to_str(ReferenceSetStatus::Confirmed)
            })
        {
            if let Some((_, context_key)) = prior {
                Self::deactivate_style_profiles_for_context(&transaction, owner_id, &context_key)?;
            }
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    /// Deactivate every active style profile for one (owner, context): withdrawn evidence
    /// invalidates what it trained. Profile rows are versioned and never deleted. Takes the
    /// connection (or an open transaction, via `Deref`) so a withdrawal mutation and this
    /// invalidation land in one atomic unit.
    fn deactivate_style_profiles_for_context(
        connection: &Connection,
        owner_id: &str,
        context_key: &str,
    ) -> anyhow::Result<()> {
        connection
            .execute(
                "UPDATE style_profiles SET active = 0
                 WHERE owner_id = ?1 AND context_key = ?2 AND active = 1",
                params![owner_id, context_key],
            )
            .context("failed to deactivate style profiles after evidence withdrawal")?;
        Ok(())
    }

    /// Delete a set; its items cascade and the next retrain reproduces the profile from the
    /// remaining evidence. Deleting a confirmed set invalidates the affected context's active
    /// profile the same way disabling does, in the same transaction as the delete.
    pub fn reference_set_delete(&mut self, owner_id: &str, set_id: &str) -> anyhow::Result<bool> {
        // One transaction for the delete and the invalidation it triggers: a crash between
        // separate autocommitted statements would leave withdrawn evidence still
        // influencing an active profile.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<(String, String)> = transaction
            .query_row(
                "SELECT status, context_key FROM reference_sets WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, set_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .context("failed to read reference set before delete")?;
        let changed = transaction
            .execute(
                "DELETE FROM reference_sets WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, set_id],
            )
            .context("failed to delete reference set")?;
        if changed == 1
            && prior.as_ref().is_some_and(|(status, _)| {
                status == reference_status_to_str(ReferenceSetStatus::Confirmed)
            })
        {
            if let Some((_, context_key)) = prior {
                Self::deactivate_style_profiles_for_context(&transaction, owner_id, &context_key)?;
            }
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn reference_set_add_item(
        &self,
        owner_id: &str,
        item: &ReferenceSetItem,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &item.owner_id, "reference set item")?;
        ensure!(
            self.reference_set_get(owner_id, &item.set_id)?.is_some(),
            "reference set {} does not exist for this owner",
            item.set_id
        );
        self.connection
            .execute(
                "INSERT INTO reference_set_items (owner_id, set_id, media_kind, media_id, role, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    owner_id,
                    item.set_id,
                    media_kind_to_str(item.media_kind),
                    item.media_id,
                    reference_role_to_str(item.role),
                    item.added_at.to_rfc3339(),
                ],
            )
            .context("failed to add reference set item")?;
        Ok(())
    }

    pub fn reference_set_remove_item(
        &mut self,
        owner_id: &str,
        set_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<bool> {
        // Removing an item from a confirmed set withdraws evidence: the removal and the
        // profile invalidation it triggers must land in ONE transaction, like disable and
        // delete.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Read the set's status and context BEFORE the removal (consistent with delete).
        let prior: Option<(String, String)> = transaction
            .query_row(
                "SELECT status, context_key FROM reference_sets WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, set_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .context("failed to read reference set before item removal")?;
        let changed = transaction
            .execute(
                "DELETE FROM reference_set_items
                 WHERE owner_id = ?1 AND set_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                params![owner_id, set_id, media_kind_to_str(media_kind), media_id],
            )
            .context("failed to remove reference set item")?;
        if changed == 1
            && prior.as_ref().is_some_and(|(status, _)| {
                status == reference_status_to_str(ReferenceSetStatus::Confirmed)
            })
        {
            if let Some((_, context_key)) = prior {
                Self::deactivate_style_profiles_for_context(&transaction, owner_id, &context_key)?;
            }
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn reference_set_items(
        &self,
        owner_id: &str,
        set_id: &str,
    ) -> anyhow::Result<Vec<ReferenceSetItem>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, set_id, media_kind, media_id, role, added_at
             FROM reference_set_items WHERE owner_id = ?1 AND set_id = ?2
             ORDER BY added_at, media_kind, media_id",
        )?;
        let rows = statement.query_map(params![owner_id, set_id], reference_set_item_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list reference set items")
    }

    /// Positive examples from confirmed, non-disabled sets in one context. This is the only
    /// read path the trainer uses, so uncurated sets can never contribute positive signal.
    pub fn reference_set_confirmed_items(
        &self,
        owner_id: &str,
        context_key: &str,
    ) -> anyhow::Result<Vec<(MediaKind, String)>> {
        let mut statement = self.connection.prepare(
            "SELECT i.media_kind, i.media_id
             FROM reference_set_items AS i
             JOIN reference_sets AS s ON s.id = i.set_id AND s.owner_id = i.owner_id
             WHERE i.owner_id = ?1 AND s.context_key = ?2 AND s.status = 'confirmed'
                   AND i.role = 'positive'
             ORDER BY i.set_id, i.media_kind, i.media_id",
        )?;
        let rows = statement.query_map(params![owner_id, context_key], |row| {
            let kind: String = row.get(0)?;
            Ok((
                media_kind_from_str(&kind)
                    .map_err(|error| conversion_message(0, error.to_string()))?,
                row.get::<_, String>(1)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list confirmed reference items")
    }

    pub fn collection_create(&self, owner_id: &str, collection: &Collection) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &collection.owner_id, "collection")?;
        ensure!(
            !collection.name.trim().is_empty(),
            "collection name must not be empty"
        );
        self.connection
            .execute(
                "INSERT INTO collections (id, owner_id, name, description, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    collection.id,
                    owner_id,
                    collection.name,
                    collection.description,
                    collection.created_at.to_rfc3339(),
                ],
            )
            .context("failed to create collection")?;
        Ok(())
    }

    pub fn collection_list(&self, owner_id: &str) -> anyhow::Result<Vec<Collection>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, description, created_at
             FROM collections WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], collection_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list collections")
    }

    pub fn collection_get(
        &self,
        owner_id: &str,
        collection_id: &str,
    ) -> anyhow::Result<Option<Collection>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, description, created_at
                 FROM collections WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, collection_id],
                collection_from_row,
            )
            .optional()
            .context("failed to read collection")
    }

    /// Rename a collection; `UNIQUE(owner_id, name)` still applies.
    pub fn collection_rename(
        &mut self,
        owner_id: &str,
        collection_id: &str,
        name: &str,
    ) -> anyhow::Result<bool> {
        ensure!(!name.trim().is_empty(), "collection name must not be empty");
        let changed = self
            .connection
            .execute(
                "UPDATE collections SET name = ?3 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, collection_id, name],
            )
            .context("failed to rename collection")?;
        Ok(changed == 1)
    }

    /// Delete a collection; items cascade and designation triggers unset the
    /// `source_collection_id` of derived reference sets while those sets keep their items.
    pub fn collection_delete(
        &mut self,
        owner_id: &str,
        collection_id: &str,
    ) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM collections WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, collection_id],
            )
            .context("failed to delete collection")?;
        Ok(changed == 1)
    }

    pub fn collection_add_item(&self, owner_id: &str, item: &CollectionItem) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &item.owner_id, "collection item")?;
        ensure!(
            self.collection_get(owner_id, &item.collection_id)?
                .is_some(),
            "collection {} does not exist for this owner",
            item.collection_id
        );
        if let Some(context_key) = &item.context_key {
            ensure!(
                !context_key.trim().is_empty(),
                "collection item context key must not be blank"
            );
        }
        self.connection
            .execute(
                "INSERT INTO collection_items (
                    owner_id, collection_id, media_kind, media_id, context_key, marked, added_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    owner_id,
                    item.collection_id,
                    media_kind_to_str(item.media_kind),
                    item.media_id,
                    item.context_key,
                    i64::from(item.marked),
                    item.added_at.to_rfc3339(),
                ],
            )
            .context("failed to add collection item")?;
        Ok(())
    }

    pub fn collection_remove_item(
        &self,
        owner_id: &str,
        collection_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM collection_items
                 WHERE owner_id = ?1 AND collection_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                params![
                    owner_id,
                    collection_id,
                    media_kind_to_str(media_kind),
                    media_id
                ],
            )
            .context("failed to remove collection item")?;
        Ok(changed == 1)
    }

    /// Mark (or unmark) an item as a user-selected example for `selected`-scope designation.
    pub fn collection_set_item_marked(
        &self,
        owner_id: &str,
        collection_id: &str,
        media_kind: MediaKind,
        media_id: &str,
        marked: bool,
    ) -> anyhow::Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE collection_items SET marked = ?5
                 WHERE owner_id = ?1 AND collection_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                params![
                    owner_id,
                    collection_id,
                    media_kind_to_str(media_kind),
                    media_id,
                    i64::from(marked),
                ],
            )
            .context("failed to mark collection item")?;
        ensure_changed(changed, "collection item", media_id)
    }

    pub fn collection_items(
        &self,
        owner_id: &str,
        collection_id: &str,
    ) -> anyhow::Result<Vec<CollectionItem>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, collection_id, media_kind, media_id, context_key, marked, added_at
             FROM collection_items WHERE owner_id = ?1 AND collection_id = ?2
             ORDER BY added_at, media_kind, media_id",
        )?;
        let rows =
            statement.query_map(params![owner_id, collection_id], collection_item_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list collection items")
    }

    /// Designate a collection as style evidence by creating a *new* `unconfirmed` reference set
    /// with `source_collection_id` filled in. For `WholeSet` every current collection item is
    /// materialized into `reference_set_items` as positive examples; for `Selected` only the
    /// rows the user marked copy. The snapshot happens here, so later collection edits never
    /// rewrite confirmed training evidence, and the trainer's confirmed-only read path is
    /// untouched: the set contributes nothing until `reference_set_confirm`.
    pub fn collection_designate_as_reference_set(
        &mut self,
        owner_id: &str,
        collection_id: &str,
        name: &str,
        context_key: &str,
        scope: ReferenceSetScope,
    ) -> anyhow::Result<ReferenceSet> {
        ensure!(
            !name.trim().is_empty(),
            "reference set name must not be empty"
        );
        ensure!(
            !context_key.trim().is_empty(),
            "reference set context key must not be empty"
        );
        let collection = self
            .collection_get(owner_id, collection_id)?
            .with_context(|| format!("collection {collection_id} was not found for this owner"))?;
        let now = Utc::now();
        let set = ReferenceSet {
            id: generated_id("refset", 0),
            owner_id: owner_id.to_owned(),
            name: name.to_owned(),
            context_key: context_key.to_owned(),
            description: String::new(),
            scope,
            status: ReferenceSetStatus::Unconfirmed,
            source_collection_id: Some(collection.id.clone()),
            created_at: now,
            confirmed_at: None,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute(
                "INSERT INTO reference_sets (
                    id, owner_id, name, context_key, description, scope, status,
                    source_collection_id, created_at, confirmed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    set.id,
                    owner_id,
                    set.name,
                    set.context_key,
                    set.description,
                    reference_scope_to_str(set.scope),
                    reference_status_to_str(set.status),
                    set.source_collection_id,
                    set.created_at.to_rfc3339(),
                    set.confirmed_at.map(|value| value.to_rfc3339()),
                ],
            )
            .context("failed to create reference set from collection")?;
        transaction
            .execute(
                "INSERT INTO reference_set_items (owner_id, set_id, media_kind, media_id, role, added_at)
                 SELECT owner_id, ?2, media_kind, media_id, 'positive', ?3
                 FROM collection_items
                 WHERE owner_id = ?1 AND collection_id = ?4 AND (?5 = 0 OR marked = 1)",
                params![
                    owner_id,
                    set.id,
                    now.to_rfc3339(),
                    collection.id,
                    i64::from(scope == ReferenceSetScope::Selected),
                ],
            )
            .context("failed to materialize collection items into reference set items")?;
        transaction.commit()?;
        Ok(set)
    }

    pub fn stack_create(&self, owner_id: &str, stack: &VersionStack) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &stack.owner_id, "version stack")?;
        ensure!(
            !stack.name.trim().is_empty(),
            "version stack name must not be empty"
        );
        self.connection
            .execute(
                "INSERT INTO version_stacks (id, owner_id, name, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    stack.id,
                    owner_id,
                    stack.name,
                    stack.created_at.to_rfc3339(),
                ],
            )
            .context("failed to create version stack")?;
        Ok(())
    }

    pub fn stack_list(&self, owner_id: &str) -> anyhow::Result<Vec<VersionStack>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, created_at
             FROM version_stacks WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], version_stack_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list version stacks")
    }

    pub fn stack_get(
        &self,
        owner_id: &str,
        stack_id: &str,
    ) -> anyhow::Result<Option<VersionStack>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, created_at
                 FROM version_stacks WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, stack_id],
                version_stack_from_row,
            )
            .optional()
            .context("failed to read version stack")
    }

    /// Delete a stack; its items cascade. Underlying media rows are never touched.
    pub fn stack_delete(&mut self, owner_id: &str, stack_id: &str) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM version_stacks WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, stack_id],
            )
            .context("failed to delete version stack")?;
        Ok(changed == 1)
    }

    pub fn stack_add_item(&self, owner_id: &str, item: &StackItem) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &item.owner_id, "stack item")?;
        ensure!(
            self.stack_get(owner_id, &item.stack_id)?.is_some(),
            "version stack {} does not exist for this owner",
            item.stack_id
        );
        if item.role == StackItemRole::Original {
            let existing: Option<i64> = self
                .connection
                .query_row(
                    "SELECT 1 FROM stack_items
                     WHERE owner_id = ?1 AND stack_id = ?2 AND role = 'original' LIMIT 1",
                    params![owner_id, item.stack_id],
                    |row| row.get(0),
                )
                .optional()
                .context("failed to check stack original")?;
            ensure!(
                existing.is_none(),
                "version stack {} already has an original",
                item.stack_id
            );
        }
        self.connection
            .execute(
                "INSERT INTO stack_items (owner_id, stack_id, media_kind, media_id, role, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    owner_id,
                    item.stack_id,
                    stack_media_kind_to_str(item.media_kind),
                    item.media_id,
                    stack_role_to_str(item.role),
                    item.added_at.to_rfc3339(),
                ],
            )
            .context("failed to add stack item")?;
        Ok(())
    }

    pub fn stack_remove_item(
        &self,
        owner_id: &str,
        stack_id: &str,
        media_kind: StackMediaKind,
        media_id: &str,
    ) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM stack_items
                 WHERE owner_id = ?1 AND stack_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                params![
                    owner_id,
                    stack_id,
                    stack_media_kind_to_str(media_kind),
                    media_id
                ],
            )
            .context("failed to remove stack item")?;
        Ok(changed == 1)
    }

    pub fn stack_items(&self, owner_id: &str, stack_id: &str) -> anyhow::Result<Vec<StackItem>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, stack_id, media_kind, media_id, role, added_at
             FROM stack_items WHERE owner_id = ?1 AND stack_id = ?2
             ORDER BY added_at, media_kind, media_id",
        )?;
        let rows = statement.query_map(params![owner_id, stack_id], stack_item_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list stack items")
    }

    /// Every stack the asset belongs to, for the grid and detail drawer.
    pub fn stacks_for_asset(
        &self,
        owner_id: &str,
        media_kind: StackMediaKind,
        media_id: &str,
    ) -> anyhow::Result<Vec<VersionStack>> {
        let mut statement = self.connection.prepare(
            "SELECT s.id, s.owner_id, s.name, s.created_at
             FROM version_stacks AS s
             JOIN stack_items AS i ON i.stack_id = s.id AND i.owner_id = s.owner_id
             WHERE s.owner_id = ?1 AND i.media_kind = ?2 AND i.media_id = ?3
             ORDER BY s.created_at, s.id",
        )?;
        let rows = statement.query_map(
            params![owner_id, stack_media_kind_to_str(media_kind), media_id],
            version_stack_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list stacks for asset")
    }

    pub fn saved_search_create(&self, owner_id: &str, search: &SavedSearch) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &search.owner_id, "saved search")?;
        ensure!(
            !search.name.trim().is_empty(),
            "saved search name must not be empty"
        );
        ensure!(
            !search.query.trim().is_empty(),
            "saved search query must not be empty"
        );
        ensure!(
            !search.context_key.trim().is_empty(),
            "saved search context key must not be empty"
        );
        validate_json_object(&search.filters_json, "filters_json")?;
        self.connection
            .execute(
                "INSERT INTO saved_searches (id, owner_id, name, query, context_key, filters_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    search.id,
                    owner_id,
                    search.name,
                    search.query,
                    search.context_key,
                    search.filters_json,
                    search.created_at.to_rfc3339(),
                ],
            )
            .context("failed to create saved search")?;
        Ok(())
    }

    pub fn saved_search_list(&self, owner_id: &str) -> anyhow::Result<Vec<SavedSearch>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, query, context_key, filters_json, created_at
             FROM saved_searches WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], saved_search_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list saved searches")
    }

    pub fn saved_search_delete(
        &mut self,
        owner_id: &str,
        saved_search_id: &str,
    ) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM saved_searches WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, saved_search_id],
            )
            .context("failed to delete saved search")?;
        Ok(changed == 1)
    }

    // ---- Editorial plans (Task 020a) ----
    //
    // Plans are documents (mutable state): none of these APIs appends a feedback event.
    // feedback_events stays the only training evidence and remains append-only.

    pub fn plan_create(&self, owner_id: &str, plan: &Plan) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &plan.owner_id, "plan")?;
        ensure!(!plan.name.trim().is_empty(), "plan name must not be empty");
        ensure!(
            !plan.context_key.trim().is_empty(),
            "plan context key must not be empty"
        );
        self.connection
            .execute(
                "INSERT INTO plans (id, owner_id, name, description, context_key, brief,
                                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    plan.id,
                    owner_id,
                    plan.name,
                    plan.description,
                    plan.context_key,
                    plan.brief,
                    plan.created_at.to_rfc3339(),
                    plan.updated_at.to_rfc3339(),
                ],
            )
            .context("failed to create plan")?;
        Ok(())
    }

    pub fn plan_list(&self, owner_id: &str) -> anyhow::Result<Vec<Plan>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, name, description, context_key, brief, created_at, updated_at
             FROM plans WHERE owner_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![owner_id], plan_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list plans")
    }

    pub fn plan_get(&self, owner_id: &str, plan_id: &str) -> anyhow::Result<Option<Plan>> {
        self.connection
            .query_row(
                "SELECT id, owner_id, name, description, context_key, brief, created_at, updated_at
                 FROM plans WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, plan_id],
                plan_from_row,
            )
            .optional()
            .context("failed to read plan")
    }

    /// Update a plan's editable header. `UNIQUE(owner_id, name)` still applies.
    pub fn plan_update(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        name: &str,
        description: &str,
        brief: &str,
    ) -> anyhow::Result<bool> {
        ensure!(!name.trim().is_empty(), "plan name must not be empty");
        let changed = self
            .connection
            .execute(
                "UPDATE plans SET name = ?3, description = ?4, brief = ?5, updated_at = ?6
                 WHERE owner_id = ?1 AND id = ?2",
                params![
                    owner_id,
                    plan_id,
                    name,
                    description,
                    brief,
                    Utc::now().to_rfc3339(),
                ],
            )
            .context("failed to update plan")?;
        Ok(changed == 1)
    }

    /// Delete a plan; its items and revision snapshots cascade with it.
    pub fn plan_delete(&mut self, owner_id: &str, plan_id: &str) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM plans WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, plan_id],
            )
            .context("failed to delete plan")?;
        Ok(changed == 1)
    }

    /// Append an item at the end of the plan (`item.position` is ignored; the next dense
    /// position is assigned). Shot boundaries are validated against the source shot and span
    /// boundaries against the source video here and again by the SQL triggers, so raw SQL
    /// cannot smuggle an out-of-bounds clip in. Span items whose boundaries differ from the
    /// imported span's get the derived `adjusted` provenance marker.
    pub fn plan_add_item(&mut self, owner_id: &str, item: &PlanItem) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &item.owner_id, "plan item")?;
        ensure!(
            self.plan_get(owner_id, &item.plan_id)?.is_some(),
            "plan {} does not exist for this owner",
            item.plan_id
        );
        let validated = self.validate_plan_item_against_media(owner_id, item)?;
        let mut item = item.clone();
        if let Some(span) = &validated {
            derive_span_adjusted_provenance(&mut item, span, Utc::now())?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(position) + 1, 0) FROM plan_items
                 WHERE owner_id = ?1 AND plan_id = ?2",
                params![owner_id, item.plan_id],
                |row| row.get(0),
            )
            .context("failed to read the next plan position")?;
        transaction
            .execute(
                "INSERT INTO plan_items (
                    owner_id, plan_id, media_kind, media_id, position, start_s, end_s, pacing,
                    crop_x, grade_json, reason, signals_json, origin, rank, profile_version,
                    provenance_json, added_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    owner_id,
                    item.plan_id,
                    media_kind_to_str(item.media_kind),
                    item.media_id,
                    next,
                    item.start_s,
                    item.end_s,
                    item.pacing,
                    item.crop_x,
                    item.grade_json,
                    item.reason,
                    item.signals_json,
                    plan_origin_to_str(item.origin),
                    item.rank,
                    item.profile_version,
                    item.provenance_json,
                    item.added_at.to_rfc3339(),
                ],
            )
            .context("failed to add plan item")?;
        transaction
            .execute(
                "UPDATE plans SET updated_at = ?3 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, item.plan_id, Utc::now().to_rfc3339()],
            )
            .context("failed to touch plan")?;
        transaction.commit()?;
        Ok(())
    }

    /// Apply a field patch to one plan item. `None` fields are left unchanged. New shot
    /// boundaries are validated against the source shot and new span boundaries against the
    /// source video; the position never changes here (use [`Store::plan_reorder_items`]).
    /// Span items gain or lose the derived `adjusted` provenance marker as their boundaries
    /// move away from or back to the imported span's.
    pub fn plan_update_item(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        media_kind: MediaKind,
        media_id: &str,
        patch: &PlanItemPatch,
    ) -> anyhow::Result<()> {
        let mut current = self
            .plan_items(owner_id, plan_id)?
            .into_iter()
            .find(|item| item.media_kind == media_kind && item.media_id == media_id)
            .with_context(|| format!("plan item {media_id} was not found in plan {plan_id}"))?;
        if let Some(start_s) = patch.start_s {
            current.start_s = Some(start_s);
        }
        if let Some(end_s) = patch.end_s {
            current.end_s = Some(end_s);
        }
        if let Some(pacing) = patch.pacing {
            current.pacing = Some(pacing);
        }
        if let Some(crop_x) = patch.crop_x {
            current.crop_x = Some(crop_x);
        }
        if let Some(grade_json) = &patch.grade_json {
            current.grade_json = Some(grade_json.clone());
        }
        if let Some(reason) = &patch.reason {
            current.reason = reason.clone();
        }
        let validated = self.validate_plan_item_against_media(owner_id, &current)?;
        if let Some(span) = &validated {
            derive_span_adjusted_provenance(&mut current, span, Utc::now())?;
        }
        self.connection
            .execute(
                "UPDATE plan_items SET start_s = ?4, end_s = ?5, pacing = ?6, crop_x = ?7,
                        grade_json = ?8, reason = ?9, provenance_json = ?11
                 WHERE owner_id = ?1 AND plan_id = ?2 AND media_kind = ?3 AND media_id = ?10",
                params![
                    owner_id,
                    plan_id,
                    media_kind_to_str(media_kind),
                    current.start_s,
                    current.end_s,
                    current.pacing,
                    current.crop_x,
                    current.grade_json,
                    current.reason,
                    media_id,
                    current.provenance_json,
                ],
            )
            .context("failed to update plan item")?;
        self.connection
            .execute(
                "UPDATE plans SET updated_at = ?3 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, plan_id, Utc::now().to_rfc3339()],
            )
            .context("failed to touch plan")?;
        Ok(())
    }

    /// Remove one item and re-compact the remaining positions to 0..n.
    pub fn plan_remove_item(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        media_kind: MediaKind,
        media_id: &str,
    ) -> anyhow::Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction
            .execute(
                "DELETE FROM plan_items
                 WHERE owner_id = ?1 AND plan_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                params![owner_id, plan_id, media_kind_to_str(media_kind), media_id],
            )
            .context("failed to remove plan item")?;
        if changed == 1 {
            compact_plan_positions(&transaction, owner_id, plan_id)?;
            transaction.execute(
                "UPDATE plans SET updated_at = ?3 WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, plan_id, Utc::now().to_rfc3339()],
            )?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn plan_items(&self, owner_id: &str, plan_id: &str) -> anyhow::Result<Vec<PlanItem>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, plan_id, media_kind, media_id, position, start_s, end_s, pacing,
                    crop_x, grade_json, reason, signals_json, origin, rank, profile_version,
                    provenance_json, added_at
             FROM plan_items WHERE owner_id = ?1 AND plan_id = ?2
             ORDER BY position, media_kind, media_id",
        )?;
        let rows = statement.query_map(params![owner_id, plan_id], plan_item_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list plan items")
    }

    /// Assign dense sequence positions 0..n exactly in the given order. The ordered list must
    /// contain every current plan item exactly once; boundaries and treatment fields do not
    /// move with a reorder.
    pub fn plan_reorder_items(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        ordered: &[(MediaKind, String)],
    ) -> anyhow::Result<()> {
        let current = self.plan_items(owner_id, plan_id)?;
        ensure!(
            current.len() == ordered.len(),
            "reorder lists {} item(s) but the plan holds {}",
            ordered.len(),
            current.len()
        );
        let mut wanted = HashSet::new();
        for (kind, id) in ordered {
            ensure!(
                wanted.insert((media_kind_to_str(*kind), id.clone())),
                "reorder lists media {id:?} more than once"
            );
            ensure!(
                current
                    .iter()
                    .any(|item| item.media_kind == *kind && item.media_id == *id),
                "reorder lists media {id:?} that is not in the plan"
            );
        }
        for item in &current {
            ensure!(
                wanted.contains(&(media_kind_to_str(item.media_kind), item.media_id.clone())),
                "reorder omits plan item {:?}",
                item.media_id
            );
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Pass 1 moves every row above the dense range so pass 2 never trips the unique
        // position index while values are being swapped.
        transaction.execute(
            "UPDATE plan_items SET position = position + 1000000000
             WHERE owner_id = ?1 AND plan_id = ?2",
            params![owner_id, plan_id],
        )?;
        for (index, (kind, id)) in ordered.iter().enumerate() {
            let index = i64::try_from(index).context("plan position overflowed i64")?;
            transaction
                .execute(
                    "UPDATE plan_items SET position = ?5
                     WHERE owner_id = ?1 AND plan_id = ?2 AND media_kind = ?3 AND media_id = ?4",
                    params![owner_id, plan_id, media_kind_to_str(*kind), id, index,],
                )
                .context("failed to reorder plan item")?;
        }
        transaction.execute(
            "UPDATE plans SET updated_at = ?3 WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, plan_id, Utc::now().to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Freeze the plan header plus its items into one append-only revision snapshot and
    /// return it. Revisions number from 1 and never rewrite (0009 triggers refuse
    /// UPDATE/DELETE while the plan exists).
    pub fn plan_save_revision(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        label: &str,
    ) -> anyhow::Result<PlanRevision> {
        let plan = self
            .plan_get(owner_id, plan_id)?
            .with_context(|| format!("plan {plan_id} was not found for this owner"))?;
        let items = self.plan_items(owner_id, plan_id)?;
        let now = Utc::now();
        let snapshot = serde_json::json!({
            "description": plan.description,
            "context_key": plan.context_key,
            "brief": plan.brief,
            "items": items
                .iter()
                .map(plan_item_snapshot_value)
                .collect::<Vec<_>>(),
        });
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(revision) + 1, 1) FROM plan_revisions
                 WHERE owner_id = ?1 AND plan_id = ?2",
                params![owner_id, plan_id],
                |row| row.get(0),
            )
            .context("failed to read the next plan revision number")?;
        let revision = PlanRevision {
            owner_id: owner_id.to_owned(),
            plan_id: plan_id.to_owned(),
            revision: next,
            label: label.to_owned(),
            snapshot_json: snapshot.to_string(),
            created_at: now,
        };
        transaction
            .execute(
                "INSERT INTO plan_revisions (owner_id, plan_id, revision, label, snapshot_json,
                                             created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    owner_id,
                    plan_id,
                    revision.revision,
                    revision.label,
                    revision.snapshot_json,
                    revision.created_at.to_rfc3339(),
                ],
            )
            .context("failed to save plan revision")?;
        transaction.commit()?;
        Ok(revision)
    }

    pub fn plan_revisions(
        &self,
        owner_id: &str,
        plan_id: &str,
    ) -> anyhow::Result<Vec<PlanRevision>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, plan_id, revision, label, snapshot_json, created_at
             FROM plan_revisions WHERE owner_id = ?1 AND plan_id = ?2 ORDER BY revision",
        )?;
        let rows = statement.query_map(params![owner_id, plan_id], plan_revision_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list plan revisions")
    }

    /// Replace the plan's editable state with a saved revision snapshot: header fields
    /// (description, context_key, brief) and the item list, re-validated against the current
    /// media so a snapshot whose shot no longer covers its boundaries fails loudly instead of
    /// restoring something unrenderable. Returns the restored item count.
    pub fn plan_restore_revision(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        revision: i64,
    ) -> anyhow::Result<usize> {
        let saved = self
            .plan_revisions(owner_id, plan_id)?
            .into_iter()
            .find(|saved| saved.revision == revision)
            .with_context(|| format!("revision {revision} of plan {plan_id} was not found"))?;
        let snapshot: serde_json::Value = serde_json::from_str(&saved.snapshot_json)
            .context("plan revision snapshot is not valid JSON")?;
        let items = snapshot
            .get("items")
            .and_then(serde_json::Value::as_array)
            .context("plan revision snapshot has no items array")?;
        let mut restored = Vec::with_capacity(items.len());
        for (index, value) in items.iter().enumerate() {
            let mut item = plan_item_from_snapshot(owner_id, plan_id, value)
                .with_context(|| format!("snapshot item {index} is invalid"))?;
            // Re-derive the span `adjusted` marker against the current span rows so a
            // restored item never carries a marker its boundaries no longer justify (the
            // span may have been refreshed since the snapshot was saved).
            let validated = self.validate_plan_item_against_media(owner_id, &item)?;
            if let Some(span) = &validated {
                derive_span_adjusted_provenance(&mut item, span, Utc::now())?;
            }
            restored.push(item);
        }
        let description = snapshot
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let context_key = snapshot
            .get("context_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_owned();
        ensure!(
            !context_key.trim().is_empty(),
            "snapshot context key must not be empty"
        );
        let brief = snapshot
            .get("brief")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute(
                "DELETE FROM plan_items WHERE owner_id = ?1 AND plan_id = ?2",
                params![owner_id, plan_id],
            )
            .context("failed to clear plan items for restore")?;
        for item in &restored {
            transaction
                .execute(
                    "INSERT INTO plan_items (
                        owner_id, plan_id, media_kind, media_id, position, start_s, end_s,
                        pacing, crop_x, grade_json, reason, signals_json, origin, rank,
                        profile_version, provenance_json, added_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        owner_id,
                        plan_id,
                        media_kind_to_str(item.media_kind),
                        item.media_id,
                        item.position,
                        item.start_s,
                        item.end_s,
                        item.pacing,
                        item.crop_x,
                        item.grade_json,
                        item.reason,
                        item.signals_json,
                        plan_origin_to_str(item.origin),
                        item.rank,
                        item.profile_version,
                        item.provenance_json,
                        item.added_at.to_rfc3339(),
                    ],
                )
                .context("failed to restore plan item")?;
        }
        transaction
            .execute(
                "UPDATE plans SET description = ?3, context_key = ?4, brief = ?5, updated_at = ?6
                 WHERE owner_id = ?1 AND id = ?2",
                params![
                    owner_id,
                    plan_id,
                    description,
                    context_key,
                    brief,
                    Utc::now().to_rfc3339(),
                ],
            )
            .context("failed to restore plan header")?;
        transaction.commit()?;
        Ok(restored.len())
    }

    /// Copy a plan (header plus items, same order and provenance) into a new plan with the
    /// given name. Revision history is not copied.
    pub fn plan_duplicate(
        &mut self,
        owner_id: &str,
        plan_id: &str,
        new_name: &str,
    ) -> anyhow::Result<Plan> {
        ensure!(!new_name.trim().is_empty(), "plan name must not be empty");
        let source = self
            .plan_get(owner_id, plan_id)?
            .with_context(|| format!("plan {plan_id} was not found for this owner"))?;
        let now = Utc::now();
        let copy = Plan {
            id: generated_id("plan", 0),
            owner_id: owner_id.to_owned(),
            name: new_name.to_owned(),
            description: source.description.clone(),
            context_key: source.context_key.clone(),
            brief: source.brief.clone(),
            created_at: now,
            updated_at: now,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute(
                "INSERT INTO plans (id, owner_id, name, description, context_key, brief,
                                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    copy.id,
                    owner_id,
                    copy.name,
                    copy.description,
                    copy.context_key,
                    copy.brief,
                    copy.created_at.to_rfc3339(),
                    copy.updated_at.to_rfc3339(),
                ],
            )
            .context("failed to duplicate plan")?;
        transaction
            .execute(
                "INSERT INTO plan_items (
                    owner_id, plan_id, media_kind, media_id, position, start_s, end_s, pacing,
                    crop_x, grade_json, reason, signals_json, origin, rank, profile_version,
                    provenance_json, added_at
                 ) SELECT owner_id, ?3, media_kind, media_id, position, start_s, end_s, pacing,
                          crop_x, grade_json, reason, signals_json, origin, rank,
                          profile_version, provenance_json, ?4
                 FROM plan_items WHERE owner_id = ?1 AND plan_id = ?2",
                params![owner_id, plan_id, copy.id, now.to_rfc3339()],
            )
            .context("failed to duplicate plan items")?;
        transaction.commit()?;
        Ok(copy)
    }

    // ---- Non-destructive render recipes and durable outputs (Task 021) ----

    /// Insert one immutable recipe version. Existing versions are never overwritten.
    pub fn render_recipe_create(
        &self,
        owner_id: &str,
        recipe: &RenderRecipe,
    ) -> anyhow::Result<()> {
        validate_render_recipe_record(owner_id, recipe)?;
        self.connection
            .execute(
                "INSERT INTO render_recipes
                 (owner_id, id, version, kind, name, schema_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    owner_id,
                    recipe.id,
                    recipe.version,
                    render_recipe_kind_to_str(recipe.kind),
                    recipe.name,
                    recipe.schema_json,
                    recipe.created_at.to_rfc3339(),
                ],
            )
            .context("failed to create immutable render recipe")?;
        Ok(())
    }

    pub fn render_recipe_get(
        &self,
        owner_id: &str,
        recipe_id: &str,
        version: i64,
    ) -> anyhow::Result<Option<RenderRecipe>> {
        self.connection
            .query_row(
                "SELECT owner_id, id, version, kind, name, schema_json, created_at
                 FROM render_recipes WHERE owner_id = ?1 AND id = ?2 AND version = ?3",
                params![owner_id, recipe_id, version],
                render_recipe_from_row,
            )
            .optional()
            .context("failed to read render recipe")
    }

    pub fn render_recipes(
        &self,
        owner_id: &str,
        kind: Option<RenderRecipeKind>,
    ) -> anyhow::Result<Vec<RenderRecipe>> {
        let kind = kind.map(render_recipe_kind_to_str);
        let mut statement = self.connection.prepare(
            "SELECT owner_id, id, version, kind, name, schema_json, created_at
             FROM render_recipes
             WHERE owner_id = ?1 AND (?2 IS NULL OR kind = ?2)
             ORDER BY created_at, id, version",
        )?;
        let rows = statement.query_map(params![owner_id, kind], render_recipe_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list render recipes")
    }

    /// Queue a render from immutable owner-scoped inputs. A recipe version and optional saved
    /// plan revision are copied into the job in the same transaction as creation.
    pub fn render_job_create(
        &mut self,
        owner_id: &str,
        job: &NewRenderJob,
    ) -> anyhow::Result<RenderJob> {
        let recipe = self
            .render_recipe_get(owner_id, &job.recipe_id, job.recipe_version)?
            .with_context(|| {
                format!(
                    "render recipe {} version {} was not found for this owner",
                    job.recipe_id, job.recipe_version
                )
            })?;
        let (frozen_plan, frozen_recipe) = self.prepare_render_job(owner_id, job, &recipe)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_render_job(
            &transaction,
            owner_id,
            job,
            &recipe,
            frozen_plan,
            frozen_recipe,
        )?;
        transaction.commit()?;
        self.render_job_by_id(owner_id, &job.id)?
            .context("queued render job could not be read back")
    }

    /// Atomically insert a one-off immutable recipe and queue its first render job. If queueing
    /// fails, the recipe insert is rolled back so append-only storage cannot accumulate recipes
    /// that no job has ever referenced.
    pub fn render_recipe_and_job_create(
        &mut self,
        owner_id: &str,
        recipe: &RenderRecipe,
        job: &NewRenderJob,
    ) -> anyhow::Result<RenderJob> {
        validate_render_recipe_record(owner_id, recipe)?;
        ensure!(
            job.recipe_id == recipe.id && job.recipe_version == recipe.version,
            "render job must reference the recipe inserted with it"
        );
        let (frozen_plan, frozen_recipe) = self.prepare_render_job(owner_id, job, recipe)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction
            .execute(
                "INSERT INTO render_recipes
                 (owner_id, id, version, kind, name, schema_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    owner_id,
                    recipe.id,
                    recipe.version,
                    render_recipe_kind_to_str(recipe.kind),
                    recipe.name,
                    recipe.schema_json,
                    recipe.created_at.to_rfc3339(),
                ],
            )
            .context("failed to create immutable render recipe")?;
        insert_render_job(
            &transaction,
            owner_id,
            job,
            recipe,
            frozen_plan,
            frozen_recipe,
        )?;
        transaction.commit()?;
        self.render_job_by_id(owner_id, &job.id)?
            .context("queued render job could not be read back")
    }

    fn prepare_render_job(
        &self,
        owner_id: &str,
        job: &NewRenderJob,
        recipe: &RenderRecipe,
    ) -> anyhow::Result<(Option<String>, String)> {
        ensure!(!job.id.trim().is_empty(), "render job id must not be empty");
        ensure!(
            job.plan_id.is_some() == job.plan_revision.is_some(),
            "plan id and revision must be supplied together"
        );
        validate_source_snapshot_json(&job.source_snapshot_json)?;
        validate_model_versions_json(&job.model_versions_json)?;
        validate_destination_path(&job.destination_path)?;
        ensure!(
            job.recipe_id == recipe.id && job.recipe_version == recipe.version,
            "render job recipe identity does not match the resolved recipe"
        );
        self.validate_render_source_ownership(owner_id, recipe, &job.source_snapshot_json)?;
        let frozen_plan = match (&job.plan_id, job.plan_revision) {
            (Some(plan_id), Some(revision)) => Some(
                self.connection
                    .query_row(
                        "SELECT snapshot_json FROM plan_revisions
                         WHERE owner_id = ?1 AND plan_id = ?2 AND revision = ?3",
                        params![owner_id, plan_id, revision],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .with_context(|| {
                        format!("plan {plan_id} revision {revision} was not found for this owner")
                    })?,
            ),
            (None, None) => None,
            _ => unreachable!("validated optional plan fields"),
        };
        ensure!(
            (recipe.kind == RenderRecipeKind::Reel) == frozen_plan.is_some(),
            "reel recipes require a frozen plan revision; photo and clip recipes must not carry one"
        );
        let recipe_schema: serde_json::Value = serde_json::from_str(&recipe.schema_json)
            .context("validated render recipe could not be parsed for freezing")?;
        let frozen_recipe = serde_json::json!({
            "id": recipe.id,
            "version": recipe.version,
            "kind": render_recipe_kind_to_str(recipe.kind),
            "name": recipe.name,
            "schema": recipe_schema,
        })
        .to_string();
        Ok((frozen_plan, frozen_recipe))
    }

    /// Bind a queued render to media that actually belongs to this owner. At queue time the IDs,
    /// stored SHA-256, and current library path must all match so the frozen audit path cannot be
    /// forged. A later legitimate relink may update the library row; execution resolves that
    /// current row by ID and rechecks both its stored hash and the file's bytes.
    fn validate_render_source_ownership(
        &self,
        owner_id: &str,
        recipe: &RenderRecipe,
        snapshot_json: &str,
    ) -> anyhow::Result<()> {
        let parsed: serde_json::Value = serde_json::from_str(snapshot_json)
            .context("validated render source snapshot could not be parsed")?;
        let sources = parsed
            .get("sources")
            .and_then(serde_json::Value::as_array)
            .context("validated render source snapshot has no sources array")?;
        match recipe.kind {
            RenderRecipeKind::Photo => ensure!(
                sources.len() == 1
                    && sources[0]
                        .get("media_kind")
                        .and_then(serde_json::Value::as_str)
                        == Some("photo"),
                "photo recipes require exactly one owned photo source"
            ),
            RenderRecipeKind::VideoClip => ensure!(
                sources.len() == 1
                    && matches!(
                        sources[0]
                            .get("media_kind")
                            .and_then(serde_json::Value::as_str),
                        Some("video" | "shot")
                    ),
                "video clip recipes require exactly one owned video or shot source"
            ),
            RenderRecipeKind::Reel => {
                let schema_version =
                    serde_json::from_str::<serde_json::Value>(&recipe.schema_json)?
                        .get("schema_version")
                        .and_then(serde_json::Value::as_u64)
                        .context("reel recipe schema_version is missing")?;
                if schema_version == 1 {
                    ensure!(
                        sources.iter().all(|source| matches!(
                            source.get("media_kind").and_then(serde_json::Value::as_str),
                            Some("shot" | "span")
                        )),
                        "ordered reel v1 accepts shot and imported span sources only; photo holds need a versioned duration and framing contract"
                    );
                } else {
                    ensure!(
                        sources.iter().all(|source| matches!(
                            source.get("media_kind").and_then(serde_json::Value::as_str),
                            Some("photo" | "shot" | "span")
                        )),
                        "reel recipes require owned photo, shot, or span sources"
                    );
                }
            }
        }

        for (index, source) in sources.iter().enumerate() {
            let source = source
                .as_object()
                .with_context(|| format!("validated render source {index} is not an object"))?;
            let media_kind = required_json_string(source, "media_kind", "render source")?;
            let media_id = required_json_string(source, "media_id", "render source")?;
            let source_id = required_json_string(source, "source_id", "render source")?;
            let snapshot_hash = required_json_string(source, "sha256", "render source")?;
            let snapshot_path = required_json_string(source, "path", "render source")?;
            let (stored_hash, stored_path) = match media_kind {
                "photo" => {
                    let photo = self.photo_by_id(owner_id, media_id)?.with_context(|| {
                        format!("render photo {media_id} is not owned by {owner_id}")
                    })?;
                    ensure!(
                        source_id == photo.id,
                        "render photo source_id must match its media_id"
                    );
                    (photo.sha256, photo.path)
                }
                "video" => {
                    let video = self.video_by_id(owner_id, media_id)?.with_context(|| {
                        format!("render video {media_id} is not owned by {owner_id}")
                    })?;
                    ensure!(
                        source_id == video.id,
                        "render video source_id must match its media_id"
                    );
                    (video.sha256, video.path)
                }
                "shot" => {
                    let shot = self.shot_by_id(owner_id, media_id)?.with_context(|| {
                        format!("render shot {media_id} is not owned by {owner_id}")
                    })?;
                    ensure!(
                        source_id == shot.video_id,
                        "render shot source_id must identify its owning video"
                    );
                    let video = self
                        .video_by_id(owner_id, &shot.video_id)?
                        .with_context(|| {
                            format!("render shot {media_id} has no owned source video")
                        })?;
                    (video.sha256, video.path)
                }
                "span" => {
                    let span = self
                        .manual_span_by_id(owner_id, media_id)?
                        .with_context(|| {
                            format!("render span {media_id} is not owned by {owner_id}")
                        })?;
                    ensure!(
                        source_id == span.video_id,
                        "render span source_id must identify its owning video"
                    );
                    let video = self
                        .video_by_id(owner_id, &span.video_id)?
                        .with_context(|| {
                            format!("render span {media_id} has no owned source video")
                        })?;
                    (video.sha256, video.path)
                }
                other => bail!("unsupported render source media_kind {other:?}"),
            };
            ensure!(
                stored_hash.eq_ignore_ascii_case(snapshot_hash),
                "render source {media_kind}:{media_id} SHA-256 does not match the owned library record"
            );
            ensure!(
                stored_path == snapshot_path,
                "render source {media_kind}:{media_id} path does not match the current owned library record"
            );
        }
        Ok(())
    }

    pub fn render_job_by_id(
        &self,
        owner_id: &str,
        job_id: &str,
    ) -> anyhow::Result<Option<RenderJob>> {
        self.connection
            .query_row(
                "SELECT owner_id, id, recipe_id, recipe_version, recipe_kind,
                        frozen_recipe_json, plan_id, plan_revision, frozen_plan_json,
                        source_snapshot_json, model_versions_json, destination_path, status,
                        progress, current_attempt, error, created_at, started_at, finished_at
                 FROM render_jobs WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, job_id],
                render_job_from_row,
            )
            .optional()
            .context("failed to read render job")
    }

    pub fn render_jobs(
        &self,
        owner_id: &str,
        status: Option<RenderJobStatus>,
    ) -> anyhow::Result<Vec<RenderJob>> {
        let status = status.map(render_job_status_to_str);
        let mut statement = self.connection.prepare(
            "SELECT owner_id, id, recipe_id, recipe_version, recipe_kind,
                    frozen_recipe_json, plan_id, plan_revision, frozen_plan_json,
                    source_snapshot_json, model_versions_json, destination_path, status,
                    progress, current_attempt, error, created_at, started_at, finished_at
             FROM render_jobs
             WHERE owner_id = ?1 AND (?2 IS NULL OR status = ?2)
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = statement.query_map(params![owner_id, status], render_job_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list render jobs")
    }

    /// Start or retry a queued/failed/cancelled render. Every retry gets a new attempt row and
    /// caller-provided managed staging path; completed outputs can never be retried or replaced.
    pub fn render_job_start(
        &mut self,
        owner_id: &str,
        job_id: &str,
        staging_path: &str,
        started_at: DateTime<Utc>,
    ) -> anyhow::Result<RenderAttempt> {
        validate_destination_path(staging_path)
            .context("render staging path must be an absolute managed path")?;
        let current = self
            .render_job_by_id(owner_id, job_id)?
            .with_context(|| format!("render job {job_id} was not found"))?;
        ensure!(
            matches!(
                current.status,
                RenderJobStatus::Queued | RenderJobStatus::Failed | RenderJobStatus::Cancelled
            ),
            "render job {job_id} cannot start from {}",
            render_job_status_to_str(current.status)
        );
        ensure!(
            self.render_output_by_job(owner_id, job_id)?.is_none(),
            "render job {job_id} already has a verified output"
        );
        let attempt = current
            .current_attempt
            .checked_add(1)
            .context("render attempt number overflowed i64")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO render_attempts
             (owner_id, job_id, attempt, status, staging_path, progress, started_at)
             VALUES (?1, ?2, ?3, 'running', ?4, 0.0, ?5)",
            params![
                owner_id,
                job_id,
                attempt,
                staging_path,
                started_at.to_rfc3339()
            ],
        )?;
        let changed = transaction.execute(
            "UPDATE render_jobs
             SET status = 'running', progress = 0.0, current_attempt = ?3, error = NULL,
                 started_at = ?4, finished_at = NULL
             WHERE owner_id = ?1 AND id = ?2 AND current_attempt = ?5",
            params![
                owner_id,
                job_id,
                attempt,
                started_at.to_rfc3339(),
                current.current_attempt
            ],
        )?;
        ensure!(
            changed == 1,
            "render job changed while its attempt was starting"
        );
        transaction.commit()?;
        self.render_attempt(owner_id, job_id, attempt)?
            .context("started render attempt could not be read back")
    }

    pub fn render_job_set_progress(
        &mut self,
        owner_id: &str,
        job_id: &str,
        progress: f64,
    ) -> anyhow::Result<()> {
        ensure_unit_score(progress, "render progress")?;
        ensure!(
            progress < 1.0,
            "render progress reaches 1.0 only after verification"
        );
        // One guarded UPDATE per table inside a single immediate transaction: the status and
        // monotonicity guards live in the WHERE clause, so the hot path performs no row read.
        // A rejected update re-derives the precise error from a minimal status/progress read.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE render_jobs SET progress = ?3
             WHERE owner_id = ?1 AND id = ?2
               AND status IN ('running', 'verifying')
               AND progress <= ?3",
            params![owner_id, job_id, progress],
        )?;
        if changed == 0 {
            let existing = transaction
                .query_row(
                    "SELECT status, progress FROM render_jobs WHERE owner_id = ?1 AND id = ?2",
                    params![owner_id, job_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
                )
                .optional()
                .context("failed to read render job")?;
            let Some((status, current)) = existing else {
                bail!("render job {job_id} was not found");
            };
            ensure!(
                matches!(status.as_str(), "running" | "verifying"),
                "render progress can only update a running or verifying job"
            );
            ensure!(progress >= current, "render progress cannot move backwards");
            bail!("render job {job_id} progress update changed no rows");
        }
        let changed_attempts = transaction.execute(
            "UPDATE render_attempts SET progress = ?3
             WHERE owner_id = ?1 AND job_id = ?2
               AND attempt = (SELECT current_attempt FROM render_jobs
                              WHERE owner_id = ?1 AND id = ?2)",
            params![owner_id, job_id, progress],
        )?;
        ensure!(
            changed_attempts == 1,
            "render job {job_id} has no current attempt to record progress on"
        );
        transaction.commit()?;
        Ok(())
    }

    pub fn render_attempt_set_commands(
        &self,
        owner_id: &str,
        job_id: &str,
        attempt: i64,
        command_json: &str,
    ) -> anyhow::Result<()> {
        let parsed: serde_json::Value =
            serde_json::from_str(command_json).context("render command_json must be valid JSON")?;
        ensure!(
            parsed.is_array(),
            "render command_json must be a JSON array"
        );
        let changed = self.connection.execute(
            "UPDATE render_attempts SET command_json = ?4
             WHERE owner_id = ?1 AND job_id = ?2 AND attempt = ?3
               AND status IN ('running', 'verifying')",
            params![owner_id, job_id, attempt, command_json],
        )?;
        ensure!(changed == 1, "active render attempt was not found");
        Ok(())
    }

    pub fn render_job_mark_verifying(
        &mut self,
        owner_id: &str,
        job_id: &str,
    ) -> anyhow::Result<()> {
        let job = self
            .render_job_by_id(owner_id, job_id)?
            .with_context(|| format!("render job {job_id} was not found"))?;
        ensure!(
            job.status == RenderJobStatus::Running,
            "only a running render can begin verification"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE render_attempts SET status = 'verifying'
             WHERE owner_id = ?1 AND job_id = ?2 AND attempt = ?3 AND status = 'running'",
            params![owner_id, job_id, job.current_attempt],
        )?;
        ensure!(changed == 1, "active render attempt was not running");
        let changed = transaction.execute(
            "UPDATE render_jobs SET status = 'verifying'
             WHERE owner_id = ?1 AND id = ?2 AND status = 'running'",
            params![owner_id, job_id],
        )?;
        ensure!(changed == 1, "render job changed before verification began");
        transaction.commit()?;
        Ok(())
    }

    /// Atomically record verified output+manifest evidence and finish the active attempt.
    pub fn render_job_finish(
        &mut self,
        owner_id: &str,
        output: &RenderOutput,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &output.owner_id, "render output")?;
        ensure!(
            !output.id.trim().is_empty(),
            "render output id must not be empty"
        );
        ensure!(
            !output.output_path.trim().is_empty(),
            "render output path is required"
        );
        ensure!(
            !output.output_sha256.trim().is_empty(),
            "render output SHA-256 is required"
        );
        validate_sha256(&output.output_sha256, "render output SHA-256")?;
        ensure!(
            output.size_bytes >= 0,
            "render output size cannot be negative"
        );
        ensure!(
            !output.media_type.trim().is_empty(),
            "render output media type is required"
        );
        ensure!(
            !output.manifest_path.trim().is_empty(),
            "render manifest path is required"
        );
        ensure!(
            !output.manifest_sha256.trim().is_empty(),
            "render manifest SHA-256 is required"
        );
        validate_sha256(&output.manifest_sha256, "render manifest SHA-256")?;
        validate_json_object(&output.verification_json, "render verification_json")?;
        validate_json_object(&output.manifest_json, "render manifest_json")?;
        let job = self
            .render_job_by_id(owner_id, &output.job_id)?
            .with_context(|| format!("render job {} was not found", output.job_id))?;
        ensure!(
            job.status == RenderJobStatus::Verifying,
            "render job is not verifying"
        );
        ensure!(
            output.attempt == job.current_attempt,
            "output attempt does not match the active render attempt"
        );
        ensure!(
            output.output_path == job.destination_path,
            "verified output path differs from the frozen destination"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO render_outputs (
                owner_id, id, job_id, attempt, output_path, output_sha256, size_bytes,
                media_type, width, height, duration_s, verification_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                owner_id,
                output.id,
                output.job_id,
                output.attempt,
                output.output_path,
                output.output_sha256,
                output.size_bytes,
                output.media_type,
                output.width,
                output.height,
                output.duration_s,
                output.verification_json,
                output.created_at.to_rfc3339(),
            ],
        )?;
        transaction.execute(
            "INSERT INTO render_manifests
             (owner_id, output_id, manifest_path, manifest_json, manifest_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                owner_id,
                output.id,
                output.manifest_path,
                output.manifest_json,
                output.manifest_sha256,
                output.created_at.to_rfc3339(),
            ],
        )?;
        let changed = transaction.execute(
            "UPDATE render_attempts
             SET status = 'done', progress = 1.0, finished_at = ?4
             WHERE owner_id = ?1 AND job_id = ?2 AND attempt = ?3 AND status = 'verifying'",
            params![
                owner_id,
                output.job_id,
                output.attempt,
                output.created_at.to_rfc3339()
            ],
        )?;
        ensure!(
            changed == 1,
            "verifying render attempt changed before completion"
        );
        let changed = transaction.execute(
            "UPDATE render_jobs
             SET status = 'done', progress = 1.0, error = NULL, finished_at = ?3
             WHERE owner_id = ?1 AND id = ?2 AND status = 'verifying'",
            params![owner_id, output.job_id, output.created_at.to_rfc3339()],
        )?;
        ensure!(
            changed == 1,
            "verifying render job changed before completion"
        );
        transaction.commit()?;
        Ok(())
    }

    pub fn render_job_fail(
        &mut self,
        owner_id: &str,
        job_id: &str,
        error: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        ensure!(
            !error.trim().is_empty(),
            "failed render must include an error"
        );
        self.finish_render_attempt(
            owner_id,
            job_id,
            RenderJobStatus::Failed,
            Some(error),
            finished_at,
        )
    }

    pub fn render_job_cancel(
        &mut self,
        owner_id: &str,
        job_id: &str,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.finish_render_attempt(
            owner_id,
            job_id,
            RenderJobStatus::Cancelled,
            None,
            finished_at,
        )
    }

    pub fn render_attempt(
        &self,
        owner_id: &str,
        job_id: &str,
        attempt: i64,
    ) -> anyhow::Result<Option<RenderAttempt>> {
        self.connection
            .query_row(
                "SELECT owner_id, job_id, attempt, status, staging_path, progress, command_json,
                        error, started_at, finished_at
                 FROM render_attempts
                 WHERE owner_id = ?1 AND job_id = ?2 AND attempt = ?3",
                params![owner_id, job_id, attempt],
                render_attempt_from_row,
            )
            .optional()
            .context("failed to read render attempt")
    }

    pub fn render_attempts(
        &self,
        owner_id: &str,
        job_id: &str,
    ) -> anyhow::Result<Vec<RenderAttempt>> {
        let mut statement = self.connection.prepare(
            "SELECT owner_id, job_id, attempt, status, staging_path, progress, command_json,
                    error, started_at, finished_at
             FROM render_attempts WHERE owner_id = ?1 AND job_id = ?2 ORDER BY attempt",
        )?;
        let rows = statement.query_map(params![owner_id, job_id], render_attempt_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list render attempts")
    }

    pub fn render_output_by_job(
        &self,
        owner_id: &str,
        job_id: &str,
    ) -> anyhow::Result<Option<RenderOutput>> {
        self.connection
            .query_row(
                "SELECT o.owner_id, o.id, o.job_id, o.attempt, o.output_path,
                        o.output_sha256, o.size_bytes, o.media_type, o.width, o.height,
                        o.duration_s, o.verification_json, m.manifest_path, m.manifest_json,
                        m.manifest_sha256, o.created_at
                 FROM render_outputs AS o
                 JOIN render_manifests AS m
                   ON m.owner_id = o.owner_id AND m.output_id = o.id
                 WHERE o.owner_id = ?1 AND o.job_id = ?2",
                params![owner_id, job_id],
                render_output_from_row,
            )
            .optional()
            .context("failed to read render output")
    }

    fn finish_render_attempt(
        &mut self,
        owner_id: &str,
        job_id: &str,
        status: RenderJobStatus,
        error: Option<&str>,
        finished_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        ensure!(
            matches!(status, RenderJobStatus::Failed | RenderJobStatus::Cancelled),
            "render terminal helper only accepts failed or cancelled"
        );
        let job = self
            .render_job_by_id(owner_id, job_id)?
            .with_context(|| format!("render job {job_id} was not found"))?;
        ensure!(
            matches!(
                job.status,
                RenderJobStatus::Running | RenderJobStatus::Verifying
            ),
            "only an active render can be failed or cancelled"
        );
        let status_text = render_job_status_to_str(status);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE render_attempts SET status = ?4, error = ?5, finished_at = ?6
             WHERE owner_id = ?1 AND job_id = ?2 AND attempt = ?3
               AND status IN ('running', 'verifying')",
            params![
                owner_id,
                job_id,
                job.current_attempt,
                status_text,
                error,
                finished_at.to_rfc3339(),
            ],
        )?;
        ensure!(
            changed == 1,
            "active render attempt changed before it finished"
        );
        let changed = transaction.execute(
            "UPDATE render_jobs SET status = ?3, error = ?4, finished_at = ?5
             WHERE owner_id = ?1 AND id = ?2 AND status IN ('running', 'verifying')",
            params![
                owner_id,
                job_id,
                status_text,
                error,
                finished_at.to_rfc3339()
            ],
        )?;
        ensure!(changed == 1, "active render job changed before it finished");
        transaction.commit()?;
        Ok(())
    }

    /// The general cold-start strong-shot list, read through the
    /// `aesthetic_assessments_strongest` index (overall DESC, confidence DESC). Assets the
    /// owner marked unusable or blur-required never surface as candidates: machine scores
    /// never clear a privacy flag.
    pub fn strongest_assets(
        &self,
        owner_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StrongAsset>> {
        let limit = i64::try_from(limit).context("strongest-asset limit overflowed i64")?;
        let mut statement = self.connection.prepare(
            "SELECT a.media_kind, a.media_id, a.overall, a.confidence
             FROM aesthetic_assessments AS a
             WHERE a.owner_id = ?1
               AND NOT EXISTS (
                 SELECT 1 FROM editorial_annotations AS e
                 WHERE e.owner_id = a.owner_id AND e.media_kind = a.media_kind
                   AND e.media_id = a.media_id AND (e.usable = 0 OR e.blur_required = 1)
               )
             ORDER BY a.overall DESC, a.confidence DESC, a.media_kind, a.media_id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![owner_id, limit], |row| {
            let media_kind: String = row.get(0)?;
            Ok(StrongAsset {
                media_kind: media_kind_from_str(&media_kind)
                    .map_err(|error| conversion_message(0, error.to_string()))?,
                media_id: row.get(1)?,
                overall: row.get(2)?,
                confidence: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to read strongest assets")
    }

    /// Shared validation for plan items: field shapes, treatment ranges, JSON payloads, the
    /// provenance invariant, and — for shots and spans — the boundary check against the
    /// source media. Returns the referenced span when `item` is one, so write paths can
    /// derive the honest `adjusted` provenance marker without re-reading the span.
    fn validate_plan_item_against_media(
        &self,
        owner_id: &str,
        item: &PlanItem,
    ) -> anyhow::Result<Option<ManualSpan>> {
        validate_plan_item_fields(item)?;
        match item.media_kind {
            MediaKind::Photo => Ok(None),
            MediaKind::Shot => {
                let shot = self
                    .shot_by_id(owner_id, &item.media_id)?
                    .with_context(|| {
                        format!("shot {} does not exist for this owner", item.media_id)
                    })?;
                let start_s = item.start_s.context("plan item shot start is required")?;
                let end_s = item.end_s.context("plan item shot end is required")?;
                ensure!(
                    start_s >= shot.start_s && end_s <= shot.end_s && end_s > start_s,
                    "plan item boundaries {start_s}..{end_s} must stay inside shot {} \
                     ({:.3}..{:.3})",
                    item.media_id,
                    shot.start_s,
                    shot.end_s
                );
                Ok(None)
            }
            MediaKind::Span => {
                let span = self
                    .manual_span_by_id(owner_id, &item.media_id)?
                    .with_context(|| {
                        format!("span {} does not exist for this owner", item.media_id)
                    })?;
                let start_s = item.start_s.context("plan item span start is required")?;
                let end_s = item.end_s.context("plan item span end is required")?;
                // Span items are adjustable clips (Task 037): the imported span boundaries
                // are the item's default, not a physical limit, so the clamp is the SOURCE
                // VIDEO's range (0..duration). The importer only matches indexed videos, so
                // an unknown duration is a degenerate case — refuse it instead of silently
                // re-freezing the item inside the span. The +0.001 s slack mirrors the
                // manual_span_bounds_* and plan_item_boundaries_* SQL triggers.
                let video = self
                    .video_by_id(owner_id, &span.video_id)?
                    .with_context(|| format!("span {} has no owned source video", item.media_id))?;
                let duration = video.duration_s.with_context(|| {
                    format!(
                        "span {} source video duration is unknown; re-index the video \
                         before editing its plan items",
                        item.media_id
                    )
                })?;
                ensure!(
                    start_s >= 0.0 && end_s <= duration + 0.001,
                    "plan item boundaries {start_s}..{end_s} must stay inside the source \
                     video {} (0..{duration:.3}); the imported span {:.3}..{:.3} is the \
                     default, not a limit",
                    span.video_id,
                    span.start_s,
                    span.end_s
                );
                Ok(Some(span))
            }
        }
    }

    /// Overwrites only the safety columns of the editorial annotation. This is the single
    /// write path for the privacy/safety flags: it is called only after an explicit user
    /// action, never appends a feedback event, and no machine path has an API that writes
    /// these columns from scores.
    pub fn set_safety_flags(
        &self,
        owner_id: &str,
        media_kind: MediaKind,
        media_id: &str,
        flags: SafetyFlags,
    ) -> anyhow::Result<EditorialAnnotation> {
        let now = Utc::now();
        let mut annotation =
            load_annotation_or_default(&self.connection, owner_id, media_kind, media_id, now)?;
        annotation.usable = flags.usable;
        annotation.faces_visible = flags.faces_visible;
        annotation.nametags_visible = flags.nametags_visible;
        annotation.blur_required = flags.blur_required;
        annotation.updated_at = now;
        upsert_editorial_annotation_on(&self.connection, owner_id, &annotation)?;
        Ok(annotation)
    }

    /// Apply a batch of explicit review actions in one immediate transaction: a bad op aborts
    /// the whole batch. Pick/reject/rate append append-only `feedback_events` rows through the
    /// `append_feedback` invariants; rate also updates the annotation's editable quality.
    /// Flag ops write state only (privacy flags never produce events); add-to-collection
    /// writes organizational state only.
    pub fn bulk_review(&mut self, owner_id: &str, ops: &[ReviewOp]) -> anyhow::Result<usize> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now();
        for (index, op) in ops.iter().enumerate() {
            match op {
                ReviewOp::Pick {
                    media_kind,
                    media_id,
                } => {
                    append_feedback_on(
                        &transaction,
                        owner_id,
                        &FeedbackEvent {
                            id: generated_id("review", index),
                            owner_id: owner_id.to_owned(),
                            media_kind: *media_kind,
                            media_id: media_id.clone(),
                            signal: FeedbackSignal::Pick,
                            value: Some(1.0),
                            compared_media_kind: None,
                            compared_media_id: None,
                            context_json: "{}".to_owned(),
                            created_at: now,
                        },
                    )?;
                }
                ReviewOp::Reject {
                    media_kind,
                    media_id,
                } => {
                    append_feedback_on(
                        &transaction,
                        owner_id,
                        &FeedbackEvent {
                            id: generated_id("review", index),
                            owner_id: owner_id.to_owned(),
                            media_kind: *media_kind,
                            media_id: media_id.clone(),
                            signal: FeedbackSignal::Reject,
                            value: Some(-1.0),
                            compared_media_kind: None,
                            compared_media_id: None,
                            context_json: "{}".to_owned(),
                            created_at: now,
                        },
                    )?;
                }
                ReviewOp::Rate {
                    media_kind,
                    media_id,
                    rating,
                } => {
                    ensure!((1..=5).contains(rating), "rating must be between 1 and 5");
                    let mut annotation = load_annotation_or_default(
                        &transaction,
                        owner_id,
                        *media_kind,
                        media_id,
                        now,
                    )?;
                    annotation.quality = Some(*rating);
                    annotation.updated_at = now;
                    upsert_editorial_annotation_on(&transaction, owner_id, &annotation)?;
                    append_feedback_on(
                        &transaction,
                        owner_id,
                        &FeedbackEvent {
                            id: generated_id("review", index),
                            owner_id: owner_id.to_owned(),
                            media_kind: *media_kind,
                            media_id: media_id.clone(),
                            signal: FeedbackSignal::Rating,
                            value: Some(*rating as f64),
                            compared_media_kind: None,
                            compared_media_id: None,
                            context_json: "{}".to_owned(),
                            created_at: now,
                        },
                    )?;
                }
                ReviewOp::SetFlags {
                    media_kind,
                    media_id,
                    flags,
                } => {
                    let mut annotation = load_annotation_or_default(
                        &transaction,
                        owner_id,
                        *media_kind,
                        media_id,
                        now,
                    )?;
                    annotation.usable = flags.usable;
                    annotation.faces_visible = flags.faces_visible;
                    annotation.nametags_visible = flags.nametags_visible;
                    annotation.blur_required = flags.blur_required;
                    annotation.updated_at = now;
                    upsert_editorial_annotation_on(&transaction, owner_id, &annotation)?;
                }
                ReviewOp::AddToCollection {
                    collection_id,
                    media_kind,
                    media_id,
                    context_key,
                } => {
                    let exists: Option<i64> = transaction
                        .query_row(
                            "SELECT 1 FROM collections WHERE owner_id = ?1 AND id = ?2",
                            params![owner_id, collection_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .context("failed to check collection")?;
                    ensure!(
                        exists.is_some(),
                        "collection {collection_id} does not exist for this owner"
                    );
                    if let Some(context_key) = context_key {
                        ensure!(
                            !context_key.trim().is_empty(),
                            "collection item context key must not be blank"
                        );
                    }
                    transaction.execute(
                        "INSERT INTO collection_items (
                            owner_id, collection_id, media_kind, media_id, context_key, marked, added_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            owner_id,
                            collection_id,
                            media_kind_to_str(*media_kind),
                            media_id,
                            context_key,
                            0,
                            now.to_rfc3339(),
                        ],
                    )?;
                }
            }
        }
        transaction.commit()?;
        Ok(ops.len())
    }

    /// The unified mixed-media grid query: one projection over photos and shots (joined to
    /// their parent video and annotation row), filterable by the `AssetFilter` fields. This is
    /// the only new read path in 019a; ranked search stays in the search crate.
    pub fn browse_assets(
        &self,
        owner_id: &str,
        filter: &AssetFilter,
    ) -> anyhow::Result<Vec<LibraryAsset>> {
        // All three branches bind the same eleven parameters in the same order, so the
        // clause set is rendered once per branch with branch-specific columns. The stack
        // clause in the shot branch can never match because stack_items CHECK-constrains
        // media_kind to photo/video, and the collection/context clauses in the span branch
        // can never match because collection_items CHECK-constrains media_kind to
        // photo/shot — which keeps the parameter list aligned across branches.
        //
        // Task 034: the span branch aliases manual_spans as `a` so the evidence clauses
        // (quality/usable/faces/blur) read the span row's own catalogue columns, and the
        // feedback arm maps to confirmed reference-set membership instead of feedback
        // events — per the v13 schema decision, spans have no feedback_events rows
        // (that table stays photo/shot); "editorial" filtering for spans means the span is
        // confirmed evidence in some reference set.
        let clause = |owner_col: &str,
                      status_col: &str,
                      path_col: &str,
                      media: &str,
                      id_col: &str,
                      feedback_via_reference_set: bool|
         -> String {
            let feedback_arm = if feedback_via_reference_set {
                format!(
                    "AND (?11 IS NULL OR EXISTS (
           SELECT 1 FROM reference_set_items rsi
           JOIN reference_sets rs ON rs.id = rsi.set_id AND rs.owner_id = rsi.owner_id
           WHERE rsi.owner_id = {owner_col} AND rsi.media_kind = '{media}'
             AND rsi.media_id = {id_col} AND rs.status = 'confirmed'
             AND rsi.role = 'positive'))"
                )
            } else {
                format!(
                    "AND (?11 IS NULL OR EXISTS (
           SELECT 1 FROM feedback_events fe
           WHERE fe.owner_id = {owner_col} AND fe.media_kind = '{media}'
             AND fe.media_id = {id_col} AND fe.signal = ?11))"
                )
            };
            format!(
                "
     AND (?2 IS NULL OR {status_col} = ?2)
     AND (?3 IS NULL OR COALESCE(a.usable, 1) = ?3)
     AND (?4 IS NULL OR COALESCE(a.faces_visible, 0) = ?4)
     AND (?5 IS NULL OR COALESCE(a.blur_required, 0) = ?5)
     AND (?6 IS NULL OR (a.quality IS NOT NULL AND a.quality >= ?6))
     AND (?7 IS NULL OR EXISTS (
           SELECT 1 FROM collection_items cf
           WHERE cf.owner_id = {owner_col} AND cf.collection_id = ?7
             AND cf.media_kind = '{media}' AND cf.media_id = {id_col}))
     AND (?8 IS NULL OR EXISTS (
           SELECT 1 FROM stack_items sf
           WHERE sf.owner_id = {owner_col} AND sf.stack_id = ?8
             AND sf.media_kind = '{media}' AND sf.media_id = {id_col}))
     AND (?9 IS NULL OR EXISTS (
           SELECT 1 FROM collection_items cx
           WHERE cx.owner_id = {owner_col} AND cx.media_kind = '{media}'
             AND cx.media_id = {id_col} AND cx.context_key = ?9))
     AND (?10 IS NULL OR {path_col} LIKE '%' || ?10 || '%')
     {feedback_arm}",
            )
        };
        let photo_clause = clause("p.owner_id", "p.status", "p.path", "photo", "p.id", false);
        let shot_clause = clause("s.owner_id", "v.status", "v.path", "shot", "s.id", false);
        let span_clause = clause("a.owner_id", "v.status", "v.path", "span", "a.id", true);
        let photo_select = format!(
            "SELECT 'photo' AS media_kind, p.id AS media_id, p.owner_id, p.path, p.thumb_rel,
                    p.status, p.indexed_at,
                    NULL AS video_id, NULL AS start_s, NULL AS end_s, p.width, p.height,
                    a.quality, COALESCE(a.usable, 1), a.standout,
                    COALESCE(a.faces_visible, 0), COALESCE(a.nametags_visible, 0),
                    COALESCE(a.blur_required, 0), COALESCE(a.tags, ''),
                    (SELECT GROUP_CONCAT(ci.collection_id) FROM collection_items ci
                     WHERE ci.owner_id = p.owner_id AND ci.media_kind = 'photo'
                       AND ci.media_id = p.id) AS collection_ids,
                    (SELECT GROUP_CONCAT(si.stack_id) FROM stack_items si
                     WHERE si.owner_id = p.owner_id AND si.media_kind = 'photo'
                       AND si.media_id = p.id) AS stack_ids,
                    NULL AS source, NULL AS external_id, NULL AS import_id, NULL AS imported_at,
                    COALESCE(p.captured_at, p.indexed_at) AS sort_at
             FROM photos p
             LEFT JOIN editorial_annotations a
               ON a.owner_id = p.owner_id AND a.media_kind = 'photo' AND a.media_id = p.id
             WHERE p.owner_id = ?1{photo_clause}",
        );
        let shot_select = format!(
            "SELECT 'shot' AS media_kind, s.id AS media_id, s.owner_id, v.path, s.thumb_rel,
                    v.status, v.indexed_at,
                    s.video_id, s.start_s, s.end_s, v.width, v.height,
                    a.quality, COALESCE(a.usable, 1), a.standout,
                    COALESCE(a.faces_visible, 0), COALESCE(a.nametags_visible, 0),
                    COALESCE(a.blur_required, 0), COALESCE(a.tags, ''),
                    (SELECT GROUP_CONCAT(ci.collection_id) FROM collection_items ci
                     WHERE ci.owner_id = s.owner_id AND ci.media_kind = 'shot'
                       AND ci.media_id = s.id) AS collection_ids,
                    NULL AS stack_ids,
                    NULL AS source, NULL AS external_id, NULL AS import_id, NULL AS imported_at,
                    v.indexed_at AS sort_at
             FROM shots s
             JOIN videos v ON v.id = s.video_id AND v.owner_id = s.owner_id
             LEFT JOIN editorial_annotations a
               ON a.owner_id = s.owner_id AND a.media_kind = 'shot' AND a.media_id = s.id
             WHERE s.owner_id = ?1{shot_clause}",
        );
        let span_select = format!(
            "SELECT 'span' AS media_kind, a.id AS media_id, a.owner_id, v.path, NULL AS thumb_rel,
                    v.status, v.indexed_at,
                    a.video_id, a.start_s, a.end_s, v.width, v.height,
                    a.quality, a.usable, a.standout,
                    a.faces_visible, a.nametags_visible, a.blur_required, a.tags,
                    NULL AS collection_ids, NULL AS stack_ids,
                    a.source, a.external_id, a.import_id, a.imported_at,
                    a.imported_at AS sort_at
             FROM manual_spans a
             JOIN videos v ON v.id = a.video_id AND v.owner_id = a.owner_id
             WHERE a.owner_id = ?1{span_clause}",
        );
        let mut sql = match filter.kind {
            Some(MediaKind::Photo) => photo_select,
            Some(MediaKind::Shot) => shot_select,
            Some(MediaKind::Span) => span_select,
            None => format!("{photo_select}\nUNION ALL\n{shot_select}\nUNION ALL\n{span_select}"),
        };
        sql.push_str("\nORDER BY sort_at, media_kind, media_id");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                owner_id,
                filter.status,
                filter.usable,
                filter.faces_visible,
                filter.blur_required,
                filter.quality_min,
                filter.collection_id,
                filter.stack_id,
                filter.context_key,
                filter.search,
                filter.feedback,
            ],
            library_asset_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to browse assets")
    }

    /// Counter totals for the library dashboard.
    pub fn library_counts(&self, owner_id: &str) -> anyhow::Result<LibraryCounts> {
        self.connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM photos WHERE owner_id = ?1),
                    (SELECT COUNT(*) FROM shots WHERE owner_id = ?1),
                    (SELECT COUNT(*) FROM feedback_events
                     WHERE owner_id = ?1 AND signal = 'pick'),
                    (SELECT COUNT(*) FROM feedback_events
                     WHERE owner_id = ?1 AND signal = 'reject'),
                    (SELECT COUNT(*) FROM editorial_annotations
                     WHERE owner_id = ?1 AND (usable = 0 OR blur_required = 1))",
                params![owner_id],
                |row| {
                    Ok(LibraryCounts {
                        photos: row.get(0)?,
                        shots: row.get(1)?,
                        picks: row.get(2)?,
                        rejects: row.get(3)?,
                        flagged: row.get(4)?,
                    })
                },
            )
            .context("failed to count library assets")
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

    /// Relink a video row at a new path — the first-class moved/renamed-file flow. The
    /// caller hashes the file at the new path first; this method re-checks that hash
    /// against the row's recorded sha256 inside the same transaction that writes the
    /// path, so a stale verification can never land and a mismatch refuses without
    /// touching anything. Only the catalog path changes: no duplicate row is created and
    /// the original file is never modified.
    pub fn relink_video(
        &mut self,
        owner_id: &str,
        video_id: &str,
        new_path: &str,
        verified_sha256: &str,
    ) -> anyhow::Result<Video> {
        self.relink_row(
            owner_id,
            "videos",
            "video",
            video_id,
            new_path,
            verified_sha256,
        )?;
        self.video_by_id(owner_id, video_id)?
            .context("relinked video could not be read back")
    }

    /// Photo counterpart of [`Store::relink_video`]; the same verification, transaction,
    /// and no-duplicate-row guarantees apply.
    pub fn relink_photo(
        &mut self,
        owner_id: &str,
        photo_id: &str,
        new_path: &str,
        verified_sha256: &str,
    ) -> anyhow::Result<Photo> {
        self.relink_row(
            owner_id,
            "photos",
            "photo",
            photo_id,
            new_path,
            verified_sha256,
        )?;
        self.photo_by_id(owner_id, photo_id)?
            .context("relinked photo could not be read back")
    }

    fn relink_row(
        &mut self,
        owner_id: &str,
        table: &str,
        kind: &str,
        id: &str,
        new_path: &str,
        verified_sha256: &str,
    ) -> anyhow::Result<()> {
        ensure!(
            !verified_sha256.is_empty(),
            "refusing to relink without a verified sha256 (fail closed)"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored_sha256: Option<String> = transaction
            .query_row(
                &format!("SELECT sha256 FROM {table} WHERE owner_id = ?1 AND id = ?2"),
                params![owner_id, id],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("failed to read the recorded hash for {kind} {id}"))?;
        let Some(stored_sha256) = stored_sha256 else {
            bail!("{kind} {id} was not found for this owner; nothing was relinked")
        };
        ensure!(
            !stored_sha256.is_empty(),
            "{kind} {id} has no recorded sha256; refusing to relink (fail closed)"
        );
        ensure!(
            stored_sha256 == verified_sha256,
            "relink refused: the file at {new_path} is not the same media Crush indexed \
             (SHA-256 mismatch for {kind} {id}). Nothing was changed."
        );
        let changed = transaction.execute(
            &format!("UPDATE {table} SET path = ?3 WHERE owner_id = ?1 AND id = ?2"),
            params![owner_id, id, new_path],
        )?;
        ensure_changed(changed, kind, id)?;
        transaction.commit()?;
        Ok(())
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
    ///
    /// This is a diffing replace, not a delete-and-refill. Shot ids are content-addressed
    /// (`stable_shot_id` from video sha256 + index + start), so a resplit normally returns
    /// the same ids: those shots are updated in place and are never deleted, so the AFTER
    /// DELETE evidence-cleanup triggers (editorial annotations, aesthetic assessments,
    /// feedback as media and as compared media, reference-set items, plan items, and the
    /// cascading vectors) fire only when a shot genuinely vanished. Evidence attached to
    /// surviving ids therefore survives a resplit. Plan items on a vanished shot are
    /// removed by the cleanup trigger like every other kind of evidence; they are never
    /// silently rewritten, and the render-time clamp still refuses an item that no longer
    /// fits its shot.
    ///
    /// One deliberate exception: the stable id covers the index and start but NOT `end_s`
    /// or `rep_frame_s`, so a re-cut that moves a cut boundary changes what a surviving
    /// shot shows while its id survives. When a survivor's `end_s` or `rep_frame_s`
    /// changed, its `shot_vectors` row is deleted in the same transaction (the same
    /// discipline as the vanished-shot cascade), so the next embed pass re-embeds it —
    /// `embed_missing_shots` skips shots that already have a vector. No other evidence is
    /// touched; see the honest assessment note in the store test.
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
        let existing = {
            let mut statement = transaction.prepare(
                "SELECT id, idx, start_s, end_s, rep_frame_s, thumb_rel, scene_score
                 FROM shots
                 WHERE owner_id = ?1 AND video_id = ?2",
            )?;
            let rows = statement.query_map(params![owner_id, video_id], |row| {
                Ok(StoredShot {
                    id: row.get(0)?,
                    idx: row.get(1)?,
                    start_s: row.get(2)?,
                    end_s: row.get(3)?,
                    rep_frame_s: row.get(4)?,
                    thumb_rel: row.get(5)?,
                    scene_score: row.get(6)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let incoming_ids: HashSet<&str> = shots.iter().map(|shot| shot.id.as_str()).collect();
        // 1) Delete only the shots whose stable id did not return. The evidence-cleanup
        //    triggers and the vector cascade fire for these real removals only.
        for stored in &existing {
            if !incoming_ids.contains(stored.id.as_str()) {
                transaction.execute(
                    "DELETE FROM shots WHERE owner_id = ?1 AND video_id = ?2 AND id = ?3",
                    params![owner_id, video_id, stored.id],
                )?;
            }
        }
        // 2) Survivors update in place — only rows whose stored values actually changed
        //    are written. A surviving id keeps its index by construction (the index is
        //    part of the stable id), so these updates cannot collide on
        //    UNIQUE(video_id, idx); a direct API call that re-indexes a surviving id
        //    fails the transaction honestly instead of deleting evidence.
        {
            let mut statement = transaction.prepare(
                "UPDATE shots
                 SET idx = ?3, start_s = ?4, end_s = ?5, rep_frame_s = ?6,
                     thumb_rel = ?7, scene_score = ?8
                 WHERE owner_id = ?1 AND id = ?2",
            )?;
            for shot in shots {
                let Some(stored) = existing.iter().find(|stored| stored.id == shot.id) else {
                    continue;
                };
                let candidate = StoredShot {
                    id: shot.id.clone(),
                    idx: shot.idx,
                    start_s: shot.start_s,
                    end_s: shot.end_s,
                    rep_frame_s: shot.rep_frame_s,
                    thumb_rel: shot.thumb_rel.clone(),
                    scene_score: shot.scene_score,
                };
                if *stored == candidate {
                    continue;
                }
                // The stable id does not cover end_s or rep_frame_s: a re-cut that moves a
                // cut boundary or rep frame changes what this shot shows while its id
                // survives, so the stored vector describes the pre-recut rep frame. Drop
                // it in this same transaction (the same discipline as the vanished-shot
                // cascade) so the next embed pass re-embeds the shot instead of skipping
                // it as "already vectorized".
                if stored.end_s != shot.end_s || stored.rep_frame_s != shot.rep_frame_s {
                    transaction.execute(
                        "DELETE FROM shot_vectors WHERE owner_id = ?1 AND shot_id = ?2",
                        params![owner_id, shot.id],
                    )?;
                }
                statement.execute(params![
                    owner_id,
                    shot.id,
                    shot.idx,
                    shot.start_s,
                    shot.end_s,
                    shot.rep_frame_s,
                    shot.thumb_rel,
                    shot.scene_score,
                ])?;
            }
        }
        // 3) Newcomers insert.
        {
            let mut statement = transaction.prepare(
                "INSERT INTO shots (
                    id, video_id, owner_id, idx, start_s, end_s, rep_frame_s, thumb_rel,
                    scene_score
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for shot in shots
                .iter()
                .filter(|shot| !existing.iter().any(|stored| stored.id == shot.id))
            {
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

    /// Text-match hits over the span catalogue FTS index (Task 034), bm25-ordered. Spans
    /// carry no vectors, so these are text-match-only results — the search layer ranks them
    /// among themselves by this order and never folds them into the cosine ranking.
    pub fn span_text_hits(
        &self,
        owner_id: &str,
        fts_query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SpanTextHit>> {
        ensure!(!fts_query.trim().is_empty(), "FTS query must not be empty");
        let mut statement = self.connection.prepare(
            "SELECT ms.id, ms.video_id, v.path, ms.start_s, ms.end_s,
                    ms.description, ms.subjects, ms.action, ms.tags, ms.shot_type, ms.camera_move,
                    ms.source, ms.external_id, ms.import_id, ms.imported_at,
                    bm25(manual_spans_fts),
                    NULLIF(
                      TRIM(
                        COALESCE(NULLIF(ms.description, ''), '') || ' ' ||
                        COALESCE(NULLIF(ms.subjects, ''), '') || ' ' ||
                        COALESCE(NULLIF(ms.action, ''), '') || ' ' ||
                        COALESCE(NULLIF(ms.tags, ''), '') || ' ' ||
                        COALESCE(NULLIF(ms.shot_type, ''), '') || ' ' ||
                        COALESCE(NULLIF(ms.camera_move, ''), '')
                      ), ''
                    ) AS catalogue_text
             FROM manual_spans_fts
             JOIN manual_spans AS ms ON ms.rowid = manual_spans_fts.rowid
             JOIN videos AS v ON v.id = ms.video_id AND v.owner_id = ms.owner_id
             WHERE manual_spans_fts MATCH ?2 AND ms.owner_id = ?1
             ORDER BY bm25(manual_spans_fts), ms.start_s, ms.id
             LIMIT ?3",
        )?;
        let rows = statement.query_map(params![owner_id, fts_query, limit as i64], |row| {
            let imported_at: String = row.get(14)?;
            Ok(SpanTextHit {
                span_id: row.get(0)?,
                video_id: row.get(1)?,
                video_path: row.get(2)?,
                start_s: row.get(3)?,
                end_s: row.get(4)?,
                description: row.get(5)?,
                subjects: row.get(6)?,
                action: row.get(7)?,
                tags: row.get(8)?,
                shot_type: row.get(9)?,
                camera_move: row.get(10)?,
                source: row.get(11)?,
                external_id: row.get(12)?,
                import_id: row.get(13)?,
                imported_at: timestamp_from_str(&imported_at, 14)?,
                rank: row.get(15)?,
                matched_text: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to search span catalogue text")
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

    /// One latest failure error per video, newest failed job first.
    /// Lets callers annotate the library with a single query instead of one per video.
    pub fn failed_job_errors(&self, owner_id: &str) -> anyhow::Result<Vec<(String, String)>> {
        let mut statement = self.connection.prepare(
            "SELECT video_id, error
             FROM jobs
             WHERE owner_id = ?1
               AND status = 'failed'
               AND video_id IS NOT NULL
               AND error IS NOT NULL
             ORDER BY started_at DESC, id DESC",
        )?;
        let rows = statement.query_map(params![owner_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        for row in rows {
            let (video_id, error) = row.context("failed to list failed job errors")?;
            if seen.insert(video_id.clone()) {
                output.push((video_id, error));
            }
        }
        Ok(output)
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

    /// Delete a video and everything that references it: shots, transcripts (FTS), vectors,
    /// assessments, annotations, feedback, plan items, stacks, collections, and reference-set
    /// items all cascade through the schema triggers and foreign keys.
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

    /// Delete a photo and everything that references it. The AFTER DELETE triggers on `photos`
    /// (schema 0002/0007/0009) clean annotations, assessments, feedback, reference-set items,
    /// plan items, and stack/collection memberships; the vectors/proxy/thumb FKs cascade.
    pub fn delete_photo_cascade(&mut self, owner_id: &str, photo_id: &str) -> anyhow::Result<bool> {
        let changed = self.connection.execute(
            "DELETE FROM photos WHERE owner_id = ?1 AND id = ?2",
            params![owner_id, photo_id],
        )?;
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

/// Copy `library.db` to `backups/library-pre-v<N>-<timestamp>.db` before pending migrations run.
///
/// `<N>` is the schema version the database is at before the upgrade. The snapshot is a plain
/// file copy taken after `PRAGMA wal_checkpoint(TRUNCATE)`, so committed frames in the `-wal`
/// sidecar are folded into the main file first and the copy is a complete, self-contained
/// database. Callers skip this entirely on a first run, where there is nothing to back up.
fn write_pre_migration_snapshot(
    connection: &Connection,
    data_dir: &Path,
    db_path: &Path,
) -> anyhow::Result<()> {
    let has_version_table = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("failed to inspect the schema before the pre-migration snapshot")?
        == 1;
    let current = if has_version_table {
        connection
            .query_row(
                "SELECT version FROM schema_version WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read the schema version before the pre-migration snapshot")?
    } else {
        None
    }
    .unwrap_or(0);
    if current >= CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    let (busy, _log, _checkpointed): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .context("failed to checkpoint the WAL before the pre-migration snapshot")?;
    if busy != 0 {
        tracing::warn!(
            busy,
            "the WAL checkpoint was busy; the pre-migration snapshot may miss the most recent commits"
        );
    }

    let backups_dir = data_dir.join("backups");
    std::fs::create_dir_all(&backups_dir).with_context(|| {
        format!(
            "failed to create backups directory {}",
            backups_dir.display()
        )
    })?;
    let snapshot_path = backups_dir.join(format!(
        "library-pre-v{current}-{}.db",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    std::fs::copy(db_path, &snapshot_path).with_context(|| {
        format!(
            "failed to write the pre-migration snapshot {}",
            snapshot_path.display()
        )
    })?;
    tracing::info!(
        schema_version = current,
        snapshot = %snapshot_path.display(),
        "wrote pre-migration database snapshot"
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
        context_key: row.get(12)?,
        baseline_metric: row.get(13)?,
        metrics_json: row.get(14)?,
        learned: row.get::<_, i64>(15)? != 0,
    })
}

fn reference_set_from_row(row: &Row<'_>) -> rusqlite::Result<ReferenceSet> {
    let scope: String = row.get(5)?;
    let status: String = row.get(6)?;
    let created_at: String = row.get(8)?;
    let confirmed_at: Option<String> = row.get(9)?;
    Ok(ReferenceSet {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        context_key: row.get(3)?,
        description: row.get(4)?,
        scope: reference_scope_from_str(&scope)
            .map_err(|error| conversion_message(5, error.to_string()))?,
        status: reference_status_from_str(&status)
            .map_err(|error| conversion_message(6, error.to_string()))?,
        source_collection_id: row.get(7)?,
        created_at: timestamp_from_str(&created_at, 8)?,
        confirmed_at: confirmed_at
            .map(|value| timestamp_from_str(&value, 9))
            .transpose()?,
    })
}

fn reference_set_item_from_row(row: &Row<'_>) -> rusqlite::Result<ReferenceSetItem> {
    let media_kind: String = row.get(2)?;
    let role: String = row.get(4)?;
    let added_at: String = row.get(5)?;
    Ok(ReferenceSetItem {
        owner_id: row.get(0)?,
        set_id: row.get(1)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(2, error.to_string()))?,
        media_id: row.get(3)?,
        role: reference_role_from_str(&role)
            .map_err(|error| conversion_message(4, error.to_string()))?,
        added_at: timestamp_from_str(&added_at, 5)?,
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

fn collection_from_row(row: &Row<'_>) -> rusqlite::Result<Collection> {
    let created_at: String = row.get(4)?;
    Ok(Collection {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        created_at: timestamp_from_str(&created_at, 4)?,
    })
}

fn collection_item_from_row(row: &Row<'_>) -> rusqlite::Result<CollectionItem> {
    let media_kind: String = row.get(2)?;
    let added_at: String = row.get(6)?;
    Ok(CollectionItem {
        owner_id: row.get(0)?,
        collection_id: row.get(1)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(2, error.to_string()))?,
        media_id: row.get(3)?,
        context_key: row.get(4)?,
        marked: row.get::<_, i64>(5)? != 0,
        added_at: timestamp_from_str(&added_at, 6)?,
    })
}

fn version_stack_from_row(row: &Row<'_>) -> rusqlite::Result<VersionStack> {
    let created_at: String = row.get(3)?;
    Ok(VersionStack {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        created_at: timestamp_from_str(&created_at, 3)?,
    })
}

fn stack_item_from_row(row: &Row<'_>) -> rusqlite::Result<StackItem> {
    let media_kind: String = row.get(2)?;
    let role: String = row.get(4)?;
    let added_at: String = row.get(5)?;
    Ok(StackItem {
        owner_id: row.get(0)?,
        stack_id: row.get(1)?,
        media_kind: stack_media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(2, error.to_string()))?,
        media_id: row.get(3)?,
        role: stack_role_from_str(&role)
            .map_err(|error| conversion_message(4, error.to_string()))?,
        added_at: timestamp_from_str(&added_at, 5)?,
    })
}

fn saved_search_from_row(row: &Row<'_>) -> rusqlite::Result<SavedSearch> {
    let created_at: String = row.get(6)?;
    Ok(SavedSearch {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        query: row.get(3)?,
        context_key: row.get(4)?,
        filters_json: row.get(5)?,
        created_at: timestamp_from_str(&created_at, 6)?,
    })
}

fn plan_from_row(row: &Row<'_>) -> rusqlite::Result<Plan> {
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;
    Ok(Plan {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        context_key: row.get(4)?,
        brief: row.get(5)?,
        created_at: timestamp_from_str(&created_at, 6)?,
        updated_at: timestamp_from_str(&updated_at, 7)?,
    })
}

fn plan_item_from_row(row: &Row<'_>) -> rusqlite::Result<PlanItem> {
    let media_kind: String = row.get(2)?;
    let origin: String = row.get(12)?;
    let added_at: String = row.get(16)?;
    Ok(PlanItem {
        owner_id: row.get(0)?,
        plan_id: row.get(1)?,
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(2, error.to_string()))?,
        media_id: row.get(3)?,
        position: row.get(4)?,
        start_s: row.get(5)?,
        end_s: row.get(6)?,
        pacing: row.get(7)?,
        crop_x: row.get(8)?,
        grade_json: row.get(9)?,
        reason: row.get(10)?,
        signals_json: row.get(11)?,
        origin: plan_origin_from_str(&origin)
            .map_err(|error| conversion_message(12, error.to_string()))?,
        rank: row.get(13)?,
        profile_version: row.get(14)?,
        provenance_json: row.get(15)?,
        added_at: timestamp_from_str(&added_at, 16)?,
    })
}

fn plan_revision_from_row(row: &Row<'_>) -> rusqlite::Result<PlanRevision> {
    let created_at: String = row.get(5)?;
    Ok(PlanRevision {
        owner_id: row.get(0)?,
        plan_id: row.get(1)?,
        revision: row.get(2)?,
        label: row.get(3)?,
        snapshot_json: row.get(4)?,
        created_at: timestamp_from_str(&created_at, 5)?,
    })
}

fn render_recipe_from_row(row: &Row<'_>) -> rusqlite::Result<RenderRecipe> {
    let kind: String = row.get(3)?;
    let created_at: String = row.get(6)?;
    Ok(RenderRecipe {
        owner_id: row.get(0)?,
        id: row.get(1)?,
        version: row.get(2)?,
        kind: render_recipe_kind_from_str(&kind)
            .map_err(|error| conversion_message(3, error.to_string()))?,
        name: row.get(4)?,
        schema_json: row.get(5)?,
        created_at: timestamp_from_str(&created_at, 6)?,
    })
}

fn validate_render_recipe_record(owner_id: &str, recipe: &RenderRecipe) -> anyhow::Result<()> {
    ensure_owner_matches(owner_id, &recipe.owner_id, "render recipe")?;
    ensure!(!recipe.id.trim().is_empty(), "recipe id must not be empty");
    ensure!(recipe.version > 0, "recipe version must be positive");
    ensure!(
        !recipe.name.trim().is_empty(),
        "recipe name must not be empty"
    );
    validate_render_recipe_json(recipe.kind, &recipe.schema_json)
}

fn insert_render_job(
    connection: &Connection,
    owner_id: &str,
    job: &NewRenderJob,
    recipe: &RenderRecipe,
    frozen_plan: Option<String>,
    frozen_recipe: String,
) -> anyhow::Result<()> {
    connection
        .execute(
            "INSERT INTO render_jobs (
                owner_id, id, recipe_id, recipe_version, recipe_kind, frozen_recipe_json,
                plan_id, plan_revision, frozen_plan_json, source_snapshot_json,
                model_versions_json, destination_path, status, progress, current_attempt,
                created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                       'queued', 0.0, 0, ?13)",
            params![
                owner_id,
                job.id,
                recipe.id,
                recipe.version,
                render_recipe_kind_to_str(recipe.kind),
                frozen_recipe,
                job.plan_id,
                job.plan_revision,
                frozen_plan,
                job.source_snapshot_json,
                job.model_versions_json,
                job.destination_path,
                job.created_at.to_rfc3339(),
            ],
        )
        .context("failed to queue render job")?;
    Ok(())
}

fn render_job_from_row(row: &Row<'_>) -> rusqlite::Result<RenderJob> {
    let kind: String = row.get(4)?;
    let status: String = row.get(12)?;
    let created_at: String = row.get(16)?;
    let started_at: Option<String> = row.get(17)?;
    let finished_at: Option<String> = row.get(18)?;
    Ok(RenderJob {
        owner_id: row.get(0)?,
        id: row.get(1)?,
        recipe_id: row.get(2)?,
        recipe_version: row.get(3)?,
        recipe_kind: render_recipe_kind_from_str(&kind)
            .map_err(|error| conversion_message(4, error.to_string()))?,
        frozen_recipe_json: row.get(5)?,
        plan_id: row.get(6)?,
        plan_revision: row.get(7)?,
        frozen_plan_json: row.get(8)?,
        source_snapshot_json: row.get(9)?,
        model_versions_json: row.get(10)?,
        destination_path: row.get(11)?,
        status: render_job_status_from_str(&status)
            .map_err(|error| conversion_message(12, error.to_string()))?,
        progress: row.get(13)?,
        current_attempt: row.get(14)?,
        error: row.get(15)?,
        created_at: timestamp_from_str(&created_at, 16)?,
        started_at: started_at
            .map(|value| timestamp_from_str(&value, 17))
            .transpose()?,
        finished_at: finished_at
            .map(|value| timestamp_from_str(&value, 18))
            .transpose()?,
    })
}

fn render_attempt_from_row(row: &Row<'_>) -> rusqlite::Result<RenderAttempt> {
    let status: String = row.get(3)?;
    let started_at: String = row.get(8)?;
    let finished_at: Option<String> = row.get(9)?;
    Ok(RenderAttempt {
        owner_id: row.get(0)?,
        job_id: row.get(1)?,
        attempt: row.get(2)?,
        status: render_job_status_from_str(&status)
            .map_err(|error| conversion_message(3, error.to_string()))?,
        staging_path: row.get(4)?,
        progress: row.get(5)?,
        command_json: row.get(6)?,
        error: row.get(7)?,
        started_at: timestamp_from_str(&started_at, 8)?,
        finished_at: finished_at
            .map(|value| timestamp_from_str(&value, 9))
            .transpose()?,
    })
}

fn render_output_from_row(row: &Row<'_>) -> rusqlite::Result<RenderOutput> {
    let created_at: String = row.get(15)?;
    Ok(RenderOutput {
        owner_id: row.get(0)?,
        id: row.get(1)?,
        job_id: row.get(2)?,
        attempt: row.get(3)?,
        output_path: row.get(4)?,
        output_sha256: row.get(5)?,
        size_bytes: row.get(6)?,
        media_type: row.get(7)?,
        width: row.get(8)?,
        height: row.get(9)?,
        duration_s: row.get(10)?,
        verification_json: row.get(11)?,
        manifest_path: row.get(12)?,
        manifest_json: row.get(13)?,
        manifest_sha256: row.get(14)?,
        created_at: timestamp_from_str(&created_at, 15)?,
    })
}

fn library_asset_from_row(row: &Row<'_>) -> rusqlite::Result<LibraryAsset> {
    let media_kind: String = row.get(0)?;
    let indexed_at: Option<String> = row.get(6)?;
    let collection_ids: Option<String> = row.get(19)?;
    let stack_ids: Option<String> = row.get(20)?;
    let source: Option<String> = row.get(21)?;
    let external_id: Option<String> = row.get(22)?;
    let import_id: Option<String> = row.get(23)?;
    let imported_at: Option<String> = row.get(24)?;
    Ok(LibraryAsset {
        media_kind: media_kind_from_str(&media_kind)
            .map_err(|error| conversion_message(0, error.to_string()))?,
        media_id: row.get(1)?,
        owner_id: row.get(2)?,
        path: row.get(3)?,
        thumb_rel: row.get(4)?,
        status: row.get(5)?,
        indexed_at: indexed_at
            .map(|value| timestamp_from_str(&value, 6))
            .transpose()?,
        video_id: row.get(7)?,
        start_s: row.get(8)?,
        end_s: row.get(9)?,
        width: row.get(10)?,
        height: row.get(11)?,
        quality: row.get(12)?,
        usable: row.get::<_, i64>(13)? != 0,
        standout: row
            .get::<_, Option<i64>>(14)?
            .is_some_and(|value| value != 0),
        faces_visible: row.get::<_, i64>(15)? != 0,
        nametags_visible: row.get::<_, i64>(16)? != 0,
        blur_required: row.get::<_, i64>(17)? != 0,
        tags: row.get(18)?,
        collection_ids: collection_ids
            .map(|value| value.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        stack_ids: stack_ids
            .map(|value| value.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
        source,
        external_id,
        import_id,
        imported_at: imported_at
            .map(|value| timestamp_from_str(&value, 24))
            .transpose()?,
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
        MediaKind::Span => "span",
    }
}

fn media_kind_from_str(value: &str) -> anyhow::Result<MediaKind> {
    match value {
        "photo" => Ok(MediaKind::Photo),
        "shot" => Ok(MediaKind::Shot),
        "span" => Ok(MediaKind::Span),
        _ => bail!("unknown media kind {value:?}"),
    }
}

pub fn plan_origin_to_str(origin: PlanOrigin) -> &'static str {
    match origin {
        PlanOrigin::General => "general",
        PlanOrigin::Personal => "personal",
        PlanOrigin::Historical => "historical",
        PlanOrigin::Imported => "imported",
    }
}

pub fn plan_origin_from_str(value: &str) -> anyhow::Result<PlanOrigin> {
    match value {
        "general" => Ok(PlanOrigin::General),
        "personal" => Ok(PlanOrigin::Personal),
        "historical" => Ok(PlanOrigin::Historical),
        "imported" => Ok(PlanOrigin::Imported),
        _ => bail!("unknown plan item origin {value:?}"),
    }
}

fn render_recipe_kind_to_str(kind: RenderRecipeKind) -> &'static str {
    match kind {
        RenderRecipeKind::Photo => "photo",
        RenderRecipeKind::VideoClip => "video_clip",
        RenderRecipeKind::Reel => "reel",
    }
}

fn render_recipe_kind_from_str(value: &str) -> anyhow::Result<RenderRecipeKind> {
    match value {
        "photo" => Ok(RenderRecipeKind::Photo),
        "video_clip" => Ok(RenderRecipeKind::VideoClip),
        "reel" => Ok(RenderRecipeKind::Reel),
        _ => bail!("unknown render recipe kind {value:?}"),
    }
}

fn render_job_status_to_str(status: RenderJobStatus) -> &'static str {
    match status {
        RenderJobStatus::Queued => "queued",
        RenderJobStatus::Running => "running",
        RenderJobStatus::Verifying => "verifying",
        RenderJobStatus::Done => "done",
        RenderJobStatus::Failed => "failed",
        RenderJobStatus::Cancelled => "cancelled",
    }
}

fn render_job_status_from_str(value: &str) -> anyhow::Result<RenderJobStatus> {
    match value {
        "queued" => Ok(RenderJobStatus::Queued),
        "running" => Ok(RenderJobStatus::Running),
        "verifying" => Ok(RenderJobStatus::Verifying),
        "done" => Ok(RenderJobStatus::Done),
        "failed" => Ok(RenderJobStatus::Failed),
        "cancelled" => Ok(RenderJobStatus::Cancelled),
        _ => bail!("unknown render job status {value:?}"),
    }
}

fn stack_media_kind_to_str(kind: StackMediaKind) -> &'static str {
    match kind {
        StackMediaKind::Photo => "photo",
        StackMediaKind::Video => "video",
    }
}

fn stack_media_kind_from_str(value: &str) -> anyhow::Result<StackMediaKind> {
    match value {
        "photo" => Ok(StackMediaKind::Photo),
        "video" => Ok(StackMediaKind::Video),
        _ => bail!("unknown stack media kind {value:?}"),
    }
}

fn stack_role_to_str(role: StackItemRole) -> &'static str {
    match role {
        StackItemRole::Original => "original",
        StackItemRole::Derived => "derived",
    }
}

fn stack_role_from_str(value: &str) -> anyhow::Result<StackItemRole> {
    match value {
        "original" => Ok(StackItemRole::Original),
        "derived" => Ok(StackItemRole::Derived),
        _ => bail!("unknown stack item role {value:?}"),
    }
}

pub fn reference_scope_to_str(scope: ReferenceSetScope) -> &'static str {
    match scope {
        ReferenceSetScope::WholeSet => "whole_set",
        ReferenceSetScope::Selected => "selected",
    }
}

pub fn reference_scope_from_str(value: &str) -> anyhow::Result<ReferenceSetScope> {
    match value {
        "whole_set" => Ok(ReferenceSetScope::WholeSet),
        "selected" => Ok(ReferenceSetScope::Selected),
        _ => bail!("unknown reference set scope {value:?}"),
    }
}

pub fn reference_status_to_str(status: ReferenceSetStatus) -> &'static str {
    match status {
        ReferenceSetStatus::Unconfirmed => "unconfirmed",
        ReferenceSetStatus::Confirmed => "confirmed",
        ReferenceSetStatus::Disabled => "disabled",
    }
}

pub fn reference_status_from_str(value: &str) -> anyhow::Result<ReferenceSetStatus> {
    match value {
        "unconfirmed" => Ok(ReferenceSetStatus::Unconfirmed),
        "confirmed" => Ok(ReferenceSetStatus::Confirmed),
        "disabled" => Ok(ReferenceSetStatus::Disabled),
        _ => bail!("unknown reference set status {value:?}"),
    }
}

pub fn reference_role_to_str(role: ReferenceItemRole) -> &'static str {
    match role {
        ReferenceItemRole::Positive => "positive",
        ReferenceItemRole::Excluded => "excluded",
    }
}

pub fn reference_role_from_str(value: &str) -> anyhow::Result<ReferenceItemRole> {
    match value {
        "positive" => Ok(ReferenceItemRole::Positive),
        "excluded" => Ok(ReferenceItemRole::Excluded),
        _ => bail!("unknown reference set item role {value:?}"),
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

/// Collision-resistant record id without adding a dependency: nanosecond timestamp plus a
/// caller-supplied nonce (the operation index inside a batch). A collision would fail the
/// primary key loudly rather than corrupt data.
fn generated_id(prefix: &str, nonce: usize) -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default().max(0);
    format!("{prefix}-{nanos}-{nonce}")
}

/// Connection-taking core of [`Store::upsert_editorial_annotation`] so transactional review
/// writes can reuse the exact same validation and SQL.
fn upsert_editorial_annotation_on(
    connection: &Connection,
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
    connection.execute(
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

/// Connection-taking core of [`Store::editorial_annotation`].
fn editorial_annotation_on(
    connection: &Connection,
    owner_id: &str,
    media_kind: MediaKind,
    media_id: &str,
) -> anyhow::Result<Option<EditorialAnnotation>> {
    connection
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

/// The stored annotation, or the 0002 column defaults when none exists yet. Callers that
/// upsert the result back rely on the target-existence triggers to refuse missing media.
fn load_annotation_or_default(
    connection: &Connection,
    owner_id: &str,
    media_kind: MediaKind,
    media_id: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<EditorialAnnotation> {
    Ok(
        editorial_annotation_on(connection, owner_id, media_kind, media_id)?.unwrap_or_else(|| {
            EditorialAnnotation {
                owner_id: owner_id.to_owned(),
                media_kind,
                media_id: media_id.to_owned(),
                description: String::new(),
                subjects: String::new(),
                action: String::new(),
                tags: String::new(),
                quality: None,
                standout: false,
                usable: true,
                faces_visible: false,
                nametags_visible: false,
                blur_required: false,
                crop_x: None,
                grade_json: None,
                notes: String::new(),
                updated_at: now,
            }
        }),
    )
}

/// Connection-taking core of [`Store::append_feedback`] so `bulk_review` can append events on
/// the same transaction as its annotation writes; invariants and SQL stay identical.
fn append_feedback_on(
    connection: &Connection,
    owner_id: &str,
    event: &FeedbackEvent,
) -> anyhow::Result<()> {
    ensure_owner_matches(owner_id, &event.owner_id, "feedback event")?;
    let has_comparison = event.compared_media_kind.is_some() && event.compared_media_id.is_some();
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
    connection.execute(
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

fn validate_render_recipe_json(kind: RenderRecipeKind, value: &str) -> anyhow::Result<()> {
    let parsed: serde_json::Value =
        serde_json::from_str(value).context("render recipe schema must be valid JSON")?;
    let object = parsed
        .as_object()
        .context("render recipe schema must be a JSON object")?;
    let schema_version = object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .context("render recipe schema_version must be an unsigned integer")?;
    match schema_version {
        1 => validate_render_recipe_v1(kind, object),
        2 if kind == RenderRecipeKind::Reel => validate_reel_recipe_v2(object),
        2 => bail!("render recipe schema_version 2 is supported only for reel recipes"),
        other => bail!("unsupported render recipe schema_version {other}"),
    }
}

/// The original Task 021 recipe contract remains frozen and accepted exactly as shipped. New
/// reel features belong to a new schema version so adding importer compatibility cannot silently
/// reinterpret an already-queued v1 job.
fn validate_render_recipe_v1(
    kind: RenderRecipeKind,
    object: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    ensure_json_keys(
        object,
        match kind {
            RenderRecipeKind::Photo => &[
                "schema_version",
                "kind",
                "crop",
                "rotation_degrees",
                "grade",
                "output",
            ],
            RenderRecipeKind::VideoClip => &[
                "schema_version",
                "kind",
                "in_s",
                "out_s",
                "crop",
                "grade",
                "transition",
                "audio",
                "output",
            ],
            RenderRecipeKind::Reel => &["schema_version", "kind", "transition", "audio", "output"],
        },
        "render recipe",
    )?;
    ensure!(
        object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(1),
        "render recipe schema_version must be 1"
    );
    ensure!(
        object.get("kind").and_then(serde_json::Value::as_str)
            == Some(render_recipe_kind_to_str(kind)),
        "render recipe kind does not match its record"
    );

    match kind {
        RenderRecipeKind::Photo => {
            validate_crop(object.get("crop").expect("key set checked"))?;
            let rotation = object
                .get("rotation_degrees")
                .and_then(serde_json::Value::as_i64)
                .context("photo rotation_degrees must be an integer")?;
            ensure!(
                matches!(rotation, 0 | 90 | 180 | 270),
                "photo rotation_degrees must be 0, 90, 180, or 270"
            );
            validate_grade(object.get("grade").expect("key set checked"))?;
            validate_output_preset(
                object.get("output").expect("key set checked"),
                &["jpeg-srgb-v1", "png-srgb-v1", "tiff-srgb-v1"],
            )?;
        }
        RenderRecipeKind::VideoClip => {
            let start = finite_json_number(object, "in_s", "video clip")?;
            let end = finite_json_number(object, "out_s", "video clip")?;
            ensure!(
                start >= 0.0 && end > start,
                "video clip out_s must exceed non-negative in_s"
            );
            validate_crop(object.get("crop").expect("key set checked"))?;
            validate_grade(object.get("grade").expect("key set checked"))?;
            validate_transition(object.get("transition").expect("key set checked"))?;
            validate_audio(object.get("audio").expect("key set checked"))?;
            validate_output_preset(
                object.get("output").expect("key set checked"),
                &["mp4-h264-sdr-v1", "mov-h264-sdr-v1"],
            )?;
        }
        RenderRecipeKind::Reel => {
            validate_transition(object.get("transition").expect("key set checked"))?;
            validate_audio(object.get("audio").expect("key set checked"))?;
            validate_output_preset(
                object.get("output").expect("key set checked"),
                &["mp4-h264-sdr-v1", "mov-h264-sdr-v1"],
            )?;
        }
    }
    Ok(())
}

/// Closed, explicit Reel Studio-compatible reel recipe. Optional Reel Studio values are frozen
/// as JSON nulls instead of omitted, which keeps manifests deterministic and makes unsupported
/// intent fail at recipe creation rather than disappear during rendering.
fn validate_reel_recipe_v2(
    object: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    ensure_json_keys(
        object,
        &[
            "schema_version",
            "kind",
            "provenance",
            "theme",
            "vibe",
            "music",
            "target_seconds",
            "beat_snap",
            "format",
            "music_volume",
            "watermark",
            "cover",
            "sequence",
            "crops",
            "output",
        ],
        "reel recipe v2",
    )?;
    ensure!(
        object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(2),
        "reel recipe v2 schema_version must be 2"
    );
    ensure!(
        object.get("kind").and_then(serde_json::Value::as_str) == Some("reel"),
        "reel recipe v2 kind must be reel"
    );
    validate_reel_provenance(object.get("provenance").expect("key set checked"))?;
    validate_nullable_string(object.get("theme").expect("key set checked"), "reel theme")?;
    validate_nullable_enum(
        object.get("vibe").expect("key set checked"),
        &["bright", "electro", "trap"],
        "reel vibe",
    )?;
    validate_nullable_relative_path(object.get("music").expect("key set checked"), "reel music")?;
    validate_nullable_positive_number(
        object.get("target_seconds").expect("key set checked"),
        "reel target_seconds",
    )?;
    ensure!(
        object
            .get("beat_snap")
            .is_some_and(serde_json::Value::is_boolean),
        "reel beat_snap must be a boolean"
    );
    ensure!(
        matches!(
            object.get("format").and_then(serde_json::Value::as_str),
            Some("9:16" | "4:5" | "1:1" | "16:9")
        ),
        "reel format must be 9:16, 4:5, 1:1, or 16:9"
    );
    validate_percentage(
        object.get("music_volume").expect("key set checked"),
        "reel music_volume",
    )?;
    validate_nullable_enum(
        object.get("watermark").expect("key set checked"),
        &["tl", "tr", "bl", "br"],
        "reel watermark",
    )?;
    validate_output_preset(
        object.get("output").expect("key set checked"),
        &["mp4-h264-sdr-v1", "mov-h264-sdr-v1"],
    )?;

    let sequence = object
        .get("sequence")
        .and_then(serde_json::Value::as_array)
        .context("reel sequence must be an array")?;
    ensure!(!sequence.is_empty(), "reel sequence must not be empty");
    let mut segment_ids = HashSet::new();
    let mut item_crops = std::collections::HashMap::new();
    let mut item_spans = std::collections::HashMap::new();
    for (index, item) in sequence.iter().enumerate() {
        let item = item
            .as_object()
            .with_context(|| format!("reel sequence item {index} must be an object"))?;
        ensure_json_keys(
            item,
            &[
                "id",
                "in",
                "out",
                "crop_x",
                "crop_kf",
                "caption",
                "cap_pos",
                "transition",
                "speed",
                "motion",
                "clip_volume",
                "grade",
            ],
            "reel sequence item",
        )?;
        let segment_id = required_json_string(item, "id", "reel sequence item")?;
        ensure!(
            segment_ids.insert(segment_id.to_owned()),
            "reel sequence contains duplicate id {segment_id:?}"
        );
        let start = finite_json_number(item, "in", "reel sequence item")?;
        let end = finite_json_number(item, "out", "reel sequence item")?;
        ensure!(
            start >= 0.0 && end > start,
            "reel sequence item out must exceed non-negative in"
        );
        let crop_x = finite_json_number(item, "crop_x", "reel sequence item")?;
        ensure_unit_score(crop_x, "reel sequence item crop_x")?;
        validate_crop_keyframes(item.get("crop_kf").expect("key set checked"), start, end)?;
        validate_nullable_string(
            item.get("caption").expect("key set checked"),
            "reel sequence item caption",
        )?;
        ensure!(
            matches!(
                item.get("cap_pos").and_then(serde_json::Value::as_str),
                Some("low" | "mid" | "high")
            ),
            "reel sequence item cap_pos must be low, mid, or high"
        );
        ensure!(
            matches!(
                item.get("transition").and_then(serde_json::Value::as_str),
                Some(
                    "cut"
                        | "mix"
                        | "fade"
                        | "white"
                        | "slideL"
                        | "slideR"
                        | "slideU"
                        | "wipeL"
                        | "circle"
                        | "blurmix"
                        | "whip"
                        | "zoom"
                )
            ),
            "reel sequence item transition is unsupported"
        );
        let speed = finite_json_number(item, "speed", "reel sequence item")?;
        ensure!(
            (0.5..=2.0).contains(&speed),
            "reel sequence item speed must be between 0.5 and 2"
        );
        ensure!(
            matches!(
                item.get("motion").and_then(serde_json::Value::as_str),
                Some("none" | "in" | "out" | "left" | "right")
            ),
            "reel sequence item motion must be none, in, out, left, or right"
        );
        validate_percentage(
            item.get("clip_volume").expect("key set checked"),
            "reel sequence item clip_volume",
        )?;
        validate_reel_grade(item.get("grade").expect("key set checked"))?;
        item_crops.insert(segment_id.to_owned(), crop_x);
        item_spans.insert(segment_id.to_owned(), (start, end));
    }

    validate_reel_crops(object.get("crops").expect("key set checked"), &item_crops)?;
    validate_reel_cover(
        object.get("cover").expect("key set checked"),
        &segment_ids,
        &item_spans,
    )
}

fn validate_reel_provenance(value: &serde_json::Value) -> anyhow::Result<()> {
    let object = value
        .as_object()
        .context("reel provenance must be an object")?;
    ensure_json_keys(
        object,
        &["origin", "source", "external_id", "profile_version"],
        "reel provenance",
    )?;
    let origin = required_json_string(object, "origin", "reel provenance")?;
    ensure!(
        matches!(origin, "general" | "personal" | "historical" | "imported"),
        "reel provenance origin must be general, personal, historical, or imported"
    );
    required_json_string(object, "source", "reel provenance")?;
    validate_nullable_string(
        object.get("external_id").expect("key set checked"),
        "reel provenance external_id",
    )?;
    let profile_version = object.get("profile_version").expect("key set checked");
    if origin == "personal" {
        ensure!(
            profile_version.as_i64().is_some_and(|version| version > 0),
            "personal reel provenance requires a positive profile_version"
        );
    } else {
        ensure!(
            profile_version.is_null(),
            "only personal reel provenance may carry profile_version"
        );
    }
    if matches!(origin, "historical" | "imported") {
        ensure!(
            object
                .get("external_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| !id.trim().is_empty()),
            "historical or imported reel provenance requires external_id"
        );
    }
    Ok(())
}

fn validate_crop_keyframes(value: &serde_json::Value, start: f64, end: f64) -> anyhow::Result<()> {
    let keyframes = value
        .as_array()
        .context("reel sequence item crop_kf must be an array")?;
    let mut previous = None;
    for (index, keyframe) in keyframes.iter().enumerate() {
        let object = keyframe
            .as_object()
            .with_context(|| format!("crop keyframe {index} must be an object"))?;
        ensure_json_keys(object, &["t", "x"], "crop keyframe")?;
        let time = finite_json_number(object, "t", "crop keyframe")?;
        let x = finite_json_number(object, "x", "crop keyframe")?;
        ensure!(
            (start..=end).contains(&time),
            "crop keyframe time must stay inside the item in/out span"
        );
        ensure_unit_score(x, "crop keyframe x")?;
        if let Some(previous) = previous {
            ensure!(
                time > previous,
                "crop keyframe times must be strictly increasing"
            );
        }
        previous = Some(time);
    }
    Ok(())
}

fn validate_reel_grade(value: &serde_json::Value) -> anyhow::Result<()> {
    let object = value
        .as_object()
        .context("reel sequence item grade must be an object")?;
    ensure_json_keys(
        object,
        &["b", "c", "s", "t", "h", "v", "sh", "hl"],
        "reel sequence item grade",
    )?;
    for (key, minimum, maximum) in [
        ("b", 60.0, 140.0),
        ("c", 60.0, 140.0),
        ("s", 0.0, 180.0),
        ("t", -50.0, 50.0),
        ("h", -30.0, 30.0),
        ("v", -50.0, 50.0),
        ("sh", -50.0, 50.0),
        ("hl", -50.0, 50.0),
    ] {
        let value = finite_json_number(object, key, "reel sequence item grade")?;
        ensure!(
            (minimum..=maximum).contains(&value),
            "reel sequence item grade {key} must be between {minimum} and {maximum}"
        );
    }
    Ok(())
}

fn validate_reel_crops(
    value: &serde_json::Value,
    item_crops: &std::collections::HashMap<String, f64>,
) -> anyhow::Result<()> {
    let crops = value.as_object().context("reel crops must be an object")?;
    for (segment_id, value) in crops {
        let crop_x = value
            .as_f64()
            .with_context(|| format!("reel crop for {segment_id:?} must be a number"))?;
        ensure_unit_score(crop_x, "reel crop")?;
        let item_crop = item_crops
            .get(segment_id)
            .with_context(|| format!("reel crops references unknown sequence id {segment_id:?}"))?;
        ensure!(
            crop_x == *item_crop,
            "reel crop for {segment_id:?} must match the sequence item crop_x"
        );
    }
    Ok(())
}

fn validate_reel_cover(
    value: &serde_json::Value,
    segment_ids: &HashSet<String>,
    item_spans: &std::collections::HashMap<String, (f64, f64)>,
) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let object = value
        .as_object()
        .context("reel cover must be null or an object")?;
    ensure_json_keys(object, &["id", "time"], "reel cover")?;
    let segment_id = required_json_string(object, "id", "reel cover")?;
    ensure!(
        segment_ids.contains(segment_id),
        "reel cover references unknown sequence id {segment_id:?}"
    );
    let (start, end) = item_spans
        .get(segment_id)
        .with_context(|| format!("reel cover references unknown sequence id {segment_id:?}"))?;
    let time = finite_json_number(object, "time", "reel cover")?;
    ensure!(
        (*start..=*end).contains(&time),
        "reel cover time must stay inside the item in/out span"
    );
    Ok(())
}

fn validate_nullable_string(value: &serde_json::Value, name: &str) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    ensure!(
        value.as_str().is_some_and(|value| !value.trim().is_empty()),
        "{name} must be null or a non-empty string"
    );
    Ok(())
}

fn validate_nullable_enum(
    value: &serde_json::Value,
    allowed: &[&str],
    name: &str,
) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_str()
        .with_context(|| format!("{name} must be null or a string"))?;
    ensure!(allowed.contains(&value), "unsupported {name} {value:?}");
    Ok(())
}

fn validate_nullable_relative_path(value: &serde_json::Value, name: &str) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_str()
        .with_context(|| format!("{name} must be null or a string"))?;
    ensure!(
        safe_portable_relative_path(value),
        "{name} must be a safe portable relative path"
    );
    Ok(())
}

/// Validate recipe-owned relative paths independent of the host running the import. In
/// particular, Unix `Path` treats Windows backslashes and drive prefixes as ordinary characters,
/// so relying on host parsing alone would accept traversal that becomes meaningful after relink.
fn safe_portable_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\0')
        && !value.starts_with(['/', '\\'])
        && !value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        && value
            .split(['/', '\\'])
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn validate_nullable_positive_number(value: &serde_json::Value, name: &str) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_f64()
        .with_context(|| format!("{name} must be null or a number"))?;
    ensure!(value.is_finite() && value > 0.0, "{name} must be positive");
    Ok(())
}

fn validate_percentage(value: &serde_json::Value, name: &str) -> anyhow::Result<()> {
    let value = value
        .as_f64()
        .with_context(|| format!("{name} must be a number"))?;
    ensure!(
        value.is_finite() && (0.0..=100.0).contains(&value),
        "{name} must be between 0 and 100"
    );
    Ok(())
}

fn validate_crop(value: &serde_json::Value) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let object = value
        .as_object()
        .context("crop must be null or an object")?;
    ensure_json_keys(object, &["x", "y", "width", "height"], "crop")?;
    let x = finite_json_number(object, "x", "crop")?;
    let y = finite_json_number(object, "y", "crop")?;
    let width = finite_json_number(object, "width", "crop")?;
    let height = finite_json_number(object, "height", "crop")?;
    ensure!(x >= 0.0 && y >= 0.0, "crop origin must be non-negative");
    ensure!(
        width > 0.0 && height > 0.0,
        "crop dimensions must be positive"
    );
    ensure!(
        x + width <= 1.0 && y + height <= 1.0,
        "crop must stay inside normalized source bounds"
    );
    Ok(())
}

fn validate_grade(value: &serde_json::Value) -> anyhow::Result<()> {
    let object = value.as_object().context("grade must be an object")?;
    let mode = object
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .context("grade mode is required")?;
    match mode {
        "none" => ensure_json_keys(object, &["mode"], "grade"),
        "basic" => {
            ensure_json_keys(
                object,
                &[
                    "mode",
                    "exposure_ev",
                    "contrast",
                    "saturation",
                    "temperature",
                    "tint",
                ],
                "grade",
            )?;
            let exposure = finite_json_number(object, "exposure_ev", "grade")?;
            let contrast = finite_json_number(object, "contrast", "grade")?;
            let saturation = finite_json_number(object, "saturation", "grade")?;
            let temperature = finite_json_number(object, "temperature", "grade")?;
            let tint = finite_json_number(object, "tint", "grade")?;
            ensure!(
                (-5.0..=5.0).contains(&exposure),
                "grade exposure_ev must be between -5 and 5"
            );
            ensure!(
                (-1.0..=1.0).contains(&contrast),
                "grade contrast must be between -1 and 1"
            );
            ensure!(
                (0.0..=2.0).contains(&saturation),
                "grade saturation must be between 0 and 2"
            );
            ensure!(
                (-1.0..=1.0).contains(&temperature),
                "grade temperature must be between -1 and 1"
            );
            ensure!(
                (-1.0..=1.0).contains(&tint),
                "grade tint must be between -1 and 1"
            );
            Ok(())
        }
        other => bail!("unsupported render grade mode {other:?}"),
    }
}

fn validate_transition(value: &serde_json::Value) -> anyhow::Result<()> {
    let object = value.as_object().context("transition must be an object")?;
    ensure_json_keys(object, &["kind"], "transition")?;
    ensure!(
        object.get("kind").and_then(serde_json::Value::as_str) == Some("cut"),
        "only the cut transition is currently supported"
    );
    Ok(())
}

fn validate_audio(value: &serde_json::Value) -> anyhow::Result<()> {
    let object = value.as_object().context("audio must be an object")?;
    ensure_json_keys(object, &["mode"], "audio")?;
    ensure!(
        matches!(
            object.get("mode").and_then(serde_json::Value::as_str),
            Some("source" | "mute")
        ),
        "audio mode must be source or mute"
    );
    Ok(())
}

fn validate_output_preset(value: &serde_json::Value, allowed: &[&str]) -> anyhow::Result<()> {
    let object = value.as_object().context("output must be an object")?;
    ensure_json_keys(object, &["preset"], "output")?;
    let preset = object
        .get("preset")
        .and_then(serde_json::Value::as_str)
        .context("output preset is required")?;
    ensure!(
        allowed.contains(&preset),
        "unsupported output preset {preset:?}"
    );
    Ok(())
}

fn validate_source_snapshot_json(value: &str) -> anyhow::Result<()> {
    let parsed: serde_json::Value =
        serde_json::from_str(value).context("render source snapshot must be valid JSON")?;
    let object = parsed
        .as_object()
        .context("render source snapshot must be a JSON object")?;
    ensure_json_keys(
        object,
        &[
            "schema_version",
            "context_key",
            "selection_provenance",
            "sources",
        ],
        "render source snapshot",
    )?;
    ensure!(
        object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(1),
        "render source snapshot schema_version must be 1"
    );
    ensure!(
        object
            .get("context_key")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()),
        "render source snapshot context_key is required"
    );
    ensure!(
        object
            .get("selection_provenance")
            .is_some_and(serde_json::Value::is_object),
        "render source snapshot selection_provenance must be an object"
    );
    let sources = object
        .get("sources")
        .and_then(serde_json::Value::as_array)
        .context("render source snapshot sources must be an array")?;
    ensure!(
        !sources.is_empty(),
        "render source snapshot requires at least one source"
    );
    let mut identities = HashSet::new();
    for (index, source) in sources.iter().enumerate() {
        let source = source
            .as_object()
            .with_context(|| format!("render source {index} must be an object"))?;
        ensure_json_keys(
            source,
            &["media_kind", "media_id", "source_id", "sha256", "path"],
            "render source",
        )?;
        let media_kind = source
            .get("media_kind")
            .and_then(serde_json::Value::as_str)
            .context("render source media_kind is required")?;
        ensure!(
            matches!(media_kind, "photo" | "shot" | "video" | "span"),
            "unsupported render source media_kind {media_kind:?}"
        );
        let media_id = required_json_string(source, "media_id", "render source")?;
        let source_id = required_json_string(source, "source_id", "render source")?;
        ensure!(
            identities.insert((media_kind.to_owned(), media_id.to_owned())),
            "render source snapshot contains duplicate media {media_kind}:{media_id}"
        );
        ensure!(
            !source_id.trim().is_empty(),
            "render source source_id is required"
        );
        validate_sha256(
            required_json_string(source, "sha256", "render source")?,
            "render source SHA-256",
        )?;
        validate_destination_path(required_json_string(source, "path", "render source")?)
            .context("render source path must be absolute")?;
    }
    Ok(())
}

fn validate_model_versions_json(value: &str) -> anyhow::Result<()> {
    let parsed: serde_json::Value =
        serde_json::from_str(value).context("render model versions must be valid JSON")?;
    let object = parsed
        .as_object()
        .context("render model versions must be a JSON object")?;
    ensure_json_keys(
        object,
        &["schema_version", "models"],
        "render model versions",
    )?;
    ensure!(
        object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            == Some(1),
        "render model versions schema_version must be 1"
    );
    let models = object
        .get("models")
        .and_then(serde_json::Value::as_object)
        .context("render model versions models must be an object")?;
    ensure_json_keys(
        models,
        &["clip", "aesthetic", "personal_style"],
        "render models",
    )?;
    for key in ["clip", "aesthetic", "personal_style"] {
        required_json_string(models, key, "render models")?;
    }
    Ok(())
}

fn validate_destination_path(value: &str) -> anyhow::Result<()> {
    ensure!(!value.trim().is_empty(), "path must not be empty");
    ensure!(!value.contains('\0'), "path must not contain NUL");
    let path = Path::new(value);
    ensure!(path.is_absolute(), "path must be absolute");
    ensure!(path.file_name().is_some(), "path must name a file");
    Ok(())
}

fn validate_sha256(value: &str, name: &str) -> anyhow::Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{name} must contain exactly 64 hexadecimal characters"
    );
    Ok(())
}

fn ensure_json_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    expected: &[&str],
    name: &str,
) -> anyhow::Result<()> {
    for key in expected {
        ensure!(object.contains_key(*key), "{name} is missing {key:?}");
    }
    for key in object.keys() {
        ensure!(
            expected.contains(&key.as_str()),
            "{name} contains unsupported field {key:?}"
        );
    }
    Ok(())
}

fn finite_json_number(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    name: &str,
) -> anyhow::Result<f64> {
    let value = object
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .with_context(|| format!("{name} {key} must be a number"))?;
    ensure!(value.is_finite(), "{name} {key} must be finite");
    Ok(value)
}

fn required_json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    name: &str,
) -> anyhow::Result<&'a str> {
    let value = object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("{name} {key} must be a string"))?;
    ensure!(!value.trim().is_empty(), "{name} {key} must not be empty");
    Ok(value)
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

/// Field-level validation shared by every plan-item write path (add, patch, restore). The
/// provenance invariant lives here and in the 0009 CHECK constraints: a `personal` item must
/// carry the style-profile version that ranked it, and a `general` item must carry none.
fn validate_plan_item_fields(item: &PlanItem) -> anyhow::Result<()> {
    ensure!(
        !item.media_id.trim().is_empty(),
        "plan item media id must not be empty"
    );
    ensure!(
        item.position >= 0,
        "plan item position must be non-negative"
    );
    match item.media_kind {
        MediaKind::Photo => {
            ensure!(
                item.start_s.is_none() && item.end_s.is_none(),
                "photo plan items carry no clip boundaries"
            );
        }
        MediaKind::Shot | MediaKind::Span => {
            let start_s = item.start_s.context("plan item shot start is required")?;
            let end_s = item.end_s.context("plan item shot end is required")?;
            ensure!(
                start_s.is_finite() && end_s.is_finite() && end_s > start_s,
                "plan item clip end must exceed its start"
            );
        }
    }
    validate_plan_item_provenance(item)?;
    if let Some(pacing) = item.pacing {
        ensure_unit_score(pacing, "plan item pacing")?;
    }
    if let Some(crop_x) = item.crop_x {
        ensure_unit_score(crop_x, "plan item crop_x")?;
    }
    if let Some(grade_json) = &item.grade_json {
        validate_json_object(grade_json, "plan item grade_json")?;
    }
    validate_json_object(&item.signals_json, "plan item signals_json")?;
    ensure!(
        (item.origin == PlanOrigin::Personal) == item.profile_version.is_some(),
        "personal plan items must record the style profile version; general items must not"
    );
    if let Some(rank) = item.rank {
        ensure!(rank.is_finite(), "plan item rank must be finite");
    }
    if let Some(profile_version) = item.profile_version {
        ensure!(
            profile_version > 0,
            "plan item profile version must be positive"
        );
    }
    Ok(())
}

/// One plan item serialized into a revision snapshot.
fn plan_item_snapshot_value(item: &PlanItem) -> serde_json::Value {
    serde_json::json!({
        "media_kind": media_kind_to_str(item.media_kind),
        "media_id": item.media_id,
        "position": item.position,
        "start_s": item.start_s,
        "end_s": item.end_s,
        "pacing": item.pacing,
        "crop_x": item.crop_x,
        "grade_json": item.grade_json,
        "reason": item.reason,
        "signals_json": item.signals_json,
        "origin": plan_origin_to_str(item.origin),
        "rank": item.rank,
        "profile_version": item.profile_version,
        "provenance_json": item.provenance_json,
        "added_at": item.added_at.to_rfc3339(),
    })
}

/// One plan item deserialized from a revision snapshot value.
fn plan_item_from_snapshot(
    owner_id: &str,
    plan_id: &str,
    value: &serde_json::Value,
) -> anyhow::Result<PlanItem> {
    let media_kind = media_kind_from_str(
        value
            .get("media_kind")
            .and_then(serde_json::Value::as_str)
            .context("snapshot item has no media_kind")?,
    )?;
    let added_at = value
        .get("added_at")
        .and_then(serde_json::Value::as_str)
        .map(DateTime::parse_from_rfc3339)
        .transpose()?
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let item = PlanItem {
        owner_id: owner_id.to_owned(),
        plan_id: plan_id.to_owned(),
        media_kind,
        media_id: value
            .get("media_id")
            .and_then(serde_json::Value::as_str)
            .context("snapshot item has no media_id")?
            .to_owned(),
        position: value
            .get("position")
            .and_then(serde_json::Value::as_i64)
            .context("snapshot item has no position")?,
        start_s: value.get("start_s").and_then(serde_json::Value::as_f64),
        end_s: value.get("end_s").and_then(serde_json::Value::as_f64),
        pacing: value.get("pacing").and_then(serde_json::Value::as_f64),
        crop_x: value.get("crop_x").and_then(serde_json::Value::as_f64),
        grade_json: value
            .get("grade_json")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        reason: value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        signals_json: value
            .get("signals_json")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}")
            .to_owned(),
        origin: plan_origin_from_str(
            value
                .get("origin")
                .and_then(serde_json::Value::as_str)
                .context("snapshot item has no origin")?,
        )?,
        rank: value.get("rank").and_then(serde_json::Value::as_f64),
        profile_version: value
            .get("profile_version")
            .and_then(serde_json::Value::as_i64),
        provenance_json: value
            .get("provenance_json")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("{}")
            .to_owned(),
        added_at,
    };
    validate_plan_item_fields(&item)?;
    Ok(item)
}

/// Re-assign dense 0..n positions to the remaining items of a plan, ordered by their current
/// position. Runs inside the caller's transaction; the offset pass keeps the unique
/// position index satisfied while values shift down.
fn compact_plan_positions(
    transaction: &rusqlite::Transaction<'_>,
    owner_id: &str,
    plan_id: &str,
) -> anyhow::Result<()> {
    transaction.execute(
        "UPDATE plan_items SET position = position + 1000000000
         WHERE owner_id = ?1 AND plan_id = ?2",
        params![owner_id, plan_id],
    )?;
    let ordered = transaction
        .prepare(
            "SELECT media_kind, media_id FROM plan_items
             WHERE owner_id = ?1 AND plan_id = ?2
             ORDER BY position - 1000000000, media_kind, media_id",
        )?
        .query_map(params![owner_id, plan_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (kind, id)) in ordered.iter().enumerate() {
        let index = i64::try_from(index).context("plan position overflowed i64")?;
        transaction.execute(
            "UPDATE plan_items SET position = ?4
             WHERE owner_id = ?1 AND plan_id = ?2 AND media_kind = ?3 AND media_id = ?5",
            params![owner_id, plan_id, kind, index, id],
        )?;
    }
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

// ---- Reel Studio import: manual spans and catalogue ledger (Task 022) ----

/// Where an imported/manual span's boundaries came from. Imported spans are honest about
/// whether the catalogue timecodes were taken literally or corrected from a measured library clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanBoundaryBasis {
    CatalogueTc,
    LibraryProbe,
    User,
}

pub fn span_boundary_basis_to_str(basis: SpanBoundaryBasis) -> &'static str {
    match basis {
        SpanBoundaryBasis::CatalogueTc => "catalogue_tc",
        SpanBoundaryBasis::LibraryProbe => "library_probe",
        SpanBoundaryBasis::User => "user",
    }
}

pub fn span_boundary_basis_from_str(value: &str) -> anyhow::Result<SpanBoundaryBasis> {
    match value {
        "catalogue_tc" => Ok(SpanBoundaryBasis::CatalogueTc),
        "library_probe" => Ok(SpanBoundaryBasis::LibraryProbe),
        "user" => Ok(SpanBoundaryBasis::User),
        _ => bail!("unknown span boundary basis {value:?}"),
    }
}

/// A first-class human-decided video span on the original source timeline. Survives resplit.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ManualSpan {
    pub id: String,
    pub owner_id: String,
    pub video_id: String,
    /// `reel_studio` or `manual`.
    pub source: String,
    /// The external identifier (Reel Studio `segment_id`) or a Crush-generated id for manual spans.
    pub external_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub boundary_basis: SpanBoundaryBasis,
    pub boundary_tolerance_s: f64,
    /// `source_t = start_s + library_relative_offset_s + t` for library-clip-relative seconds.
    pub library_relative_offset_s: f64,
    pub description: String,
    pub shot_type: String,
    pub camera_move: String,
    pub subjects: String,
    pub action: String,
    pub tags: String,
    pub quality: Option<i64>,
    pub standout: bool,
    pub usable: bool,
    pub faces_visible: bool,
    pub nametags_visible: bool,
    pub blur_required: bool,
    pub used_in: String,
    pub crop_x: Option<f64>,
    pub notes: String,
    pub import_id: Option<String>,
    pub imported_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One append-only row in the import ledger.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CatalogueImport {
    pub id: String,
    pub owner_id: String,
    pub source: String,
    /// `dry_run` or `apply`.
    pub mode: String,
    pub catalogue_path: String,
    pub catalogue_sha256: String,
    pub recipes_json: String,
    pub report_json: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

const MANUAL_SPAN_COLUMNS: &str = "id, owner_id, video_id, source, external_id, start_s, end_s, \
     boundary_basis, boundary_tolerance_s, library_relative_offset_s, description, shot_type, \
     camera_move, subjects, action, tags, quality, standout, usable, faces_visible, \
     nametags_visible, blur_required, used_in, crop_x, notes, import_id, imported_at, updated_at";

/// One span surfaced by the Preferences "confirm imported evidence" flow (Task 034): a span
/// with import lineage or catalogue evidence (quality/standout/used_in) that is awaiting —
/// or has already received — the user's explicit confirmation decision. `sets` carries the
/// reference sets currently holding the span so the UI can show confirmed evidence and its
/// withdrawal path instead of guessing.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedEvidenceSpan {
    pub span_id: String,
    pub external_id: String,
    pub source: String,
    pub import_id: Option<String>,
    pub video_path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub description: String,
    pub quality: Option<i64>,
    pub standout: bool,
    pub used_in: String,
    pub imported_at: DateTime<Utc>,
    /// Reference sets (any status) that contain this span, by name.
    pub sets: Vec<String>,
    /// True when at least one containing set is `confirmed` — i.e. the explicit
    /// confirmation step already happened for this span.
    pub confirmed: bool,
}

fn manual_span_from_row(row: &Row<'_>) -> rusqlite::Result<ManualSpan> {
    let basis: String = row.get(7)?;
    let imported_at: String = row.get(26)?;
    let updated_at: String = row.get(27)?;
    Ok(ManualSpan {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        video_id: row.get(2)?,
        source: row.get(3)?,
        external_id: row.get(4)?,
        start_s: row.get(5)?,
        end_s: row.get(6)?,
        boundary_basis: span_boundary_basis_from_str(&basis)
            .map_err(|error| conversion_message(7, error.to_string()))?,
        boundary_tolerance_s: row.get(8)?,
        library_relative_offset_s: row.get(9)?,
        description: row.get(10)?,
        shot_type: row.get(11)?,
        camera_move: row.get(12)?,
        subjects: row.get(13)?,
        action: row.get(14)?,
        tags: row.get(15)?,
        quality: row.get(16)?,
        standout: row.get::<_, i64>(17)? != 0,
        usable: row.get::<_, i64>(18)? != 0,
        faces_visible: row.get::<_, i64>(19)? != 0,
        nametags_visible: row.get::<_, i64>(20)? != 0,
        blur_required: row.get::<_, i64>(21)? != 0,
        used_in: row.get(22)?,
        crop_x: row.get(23)?,
        notes: row.get(24)?,
        import_id: row.get(25)?,
        imported_at: timestamp_from_str(&imported_at, 26)?,
        updated_at: timestamp_from_str(&updated_at, 27)?,
    })
}

fn catalogue_import_from_row(row: &Row<'_>) -> rusqlite::Result<CatalogueImport> {
    let started_at: String = row.get(8)?;
    let finished_at: String = row.get(9)?;
    Ok(CatalogueImport {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        source: row.get(2)?,
        mode: row.get(3)?,
        catalogue_path: row.get(4)?,
        catalogue_sha256: row.get(5)?,
        recipes_json: row.get(6)?,
        report_json: row.get(7)?,
        started_at: timestamp_from_str(&started_at, 8)?,
        finished_at: timestamp_from_str(&finished_at, 9)?,
    })
}

/// Historical/imported items must say where they came from and never claim a profile.
fn validate_plan_item_provenance(item: &PlanItem) -> anyhow::Result<()> {
    let provenance: serde_json::Value = serde_json::from_str(&item.provenance_json)
        .context("plan item provenance_json must be valid JSON")?;
    ensure!(
        provenance.is_object(),
        "plan item provenance_json must be a JSON object"
    );
    match item.origin {
        PlanOrigin::General | PlanOrigin::Personal => Ok(()),
        PlanOrigin::Historical | PlanOrigin::Imported => {
            ensure!(
                item.profile_version.is_none(),
                "historical or imported plan items never carry a profile version"
            );
            let object = provenance.as_object().expect("checked above");
            for key in ["source", "external_id"] {
                ensure!(
                    object
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                    "historical or imported plan items require provenance {key}"
                );
            }
            Ok(())
        }
    }
}
/// Task 037: derive the honest `adjusted` provenance marker for span plan items. The marker
/// is derived here — inside the store's item write paths — so it can neither drift from the
/// stored boundaries nor be spoofed by a caller. The comparison default is the item's
/// import-time boundaries (`imported_start_s`/`imported_end_s`, recorded by the importer,
/// because a recipe's trim of a catalogue segment is the imported default even though it
/// differs from the span row); items without those keys (manual spans, legacy imports)
/// compare against the span row. When the item's In/Out differ from that default the
/// provenance gains `adjusted: true` + `adjusted_at`; returning to the default removes
/// both. Lineage keys (`source`, `external_id`, `import_id`, boundary basis/tolerance) are
/// preserved. Call only after `validate_plan_item_fields`, which guarantees a JSON object.
fn derive_span_adjusted_provenance(
    item: &mut PlanItem,
    span: &ManualSpan,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    if item.media_kind != MediaKind::Span {
        return Ok(());
    }
    let mut provenance: serde_json::Value = serde_json::from_str(&item.provenance_json)
        .context("plan item provenance_json must be valid JSON")?;
    let object = provenance
        .as_object_mut()
        .context("plan item provenance_json must be a JSON object")?;
    let imported_start = object
        .get("imported_start_s")
        .and_then(serde_json::Value::as_f64);
    let imported_end = object
        .get("imported_end_s")
        .and_then(serde_json::Value::as_f64);
    let (default_start, default_end) = match (imported_start, imported_end) {
        (Some(start), Some(end)) => (start, end),
        // No recorded import boundaries: the span row is the only honest default.
        _ => (span.start_s, span.end_s),
    };
    let matches_default = item.start_s == Some(default_start) && item.end_s == Some(default_end);
    if matches_default {
        object.remove("adjusted");
        object.remove("adjusted_at");
    } else {
        object.insert("adjusted".to_owned(), serde_json::Value::Bool(true));
        object.insert(
            "adjusted_at".to_owned(),
            serde_json::Value::String(now.to_rfc3339()),
        );
    }
    item.provenance_json = provenance.to_string();
    Ok(())
}

fn validate_manual_span(span: &ManualSpan) -> anyhow::Result<()> {
    ensure!(
        !span.id.trim().is_empty(),
        "manual span id must not be empty"
    );
    ensure!(
        matches!(span.source.as_str(), "reel_studio" | "manual"),
        "manual span source must be reel_studio or manual"
    );
    ensure!(
        !span.external_id.trim().is_empty(),
        "manual span external id must not be empty"
    );
    ensure!(
        span.start_s.is_finite()
            && span.end_s.is_finite()
            && span.start_s >= 0.0
            && span.end_s > span.start_s,
        "manual span end must exceed its finite non-negative start"
    );
    ensure!(
        span.boundary_tolerance_s.is_finite() && span.boundary_tolerance_s >= 0.0,
        "manual span boundary tolerance must be finite and non-negative"
    );
    ensure!(
        span.library_relative_offset_s.is_finite(),
        "manual span library offset must be finite"
    );
    if let Some(quality) = span.quality {
        ensure!(
            (1..=5).contains(&quality),
            "manual span quality must be 1..=5"
        );
    }
    if let Some(crop_x) = span.crop_x {
        ensure_unit_score(crop_x, "manual span crop_x")?;
    }
    Ok(())
}

impl Store {
    /// Insert or refresh one imported/manual span keyed by (owner, source, external_id). The
    /// span id is stable across re-imports: an existing row keeps its id (so plan items that
    /// reference it survive) and only its evidence/boundaries are updated. Returns the stored row.
    pub fn manual_span_upsert(
        &self,
        owner_id: &str,
        span: &ManualSpan,
    ) -> anyhow::Result<ManualSpan> {
        ensure_owner_matches(owner_id, &span.owner_id, "manual span")?;
        validate_manual_span(span)?;
        let existing =
            self.manual_span_by_external_id(owner_id, &span.source, &span.external_id)?;
        let id = existing
            .as_ref()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| span.id.clone());
        self.connection
            .execute(
                &format!(
                    "INSERT INTO manual_spans ({MANUAL_SPAN_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                             ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
                     ON CONFLICT(owner_id, source, external_id) DO UPDATE SET
                        video_id = excluded.video_id,
                        start_s = excluded.start_s,
                        end_s = excluded.end_s,
                        boundary_basis = excluded.boundary_basis,
                        boundary_tolerance_s = excluded.boundary_tolerance_s,
                        library_relative_offset_s = excluded.library_relative_offset_s,
                        description = excluded.description,
                        shot_type = excluded.shot_type,
                        camera_move = excluded.camera_move,
                        subjects = excluded.subjects,
                        action = excluded.action,
                        tags = excluded.tags,
                        quality = excluded.quality,
                        standout = excluded.standout,
                        usable = excluded.usable,
                        faces_visible = excluded.faces_visible,
                        nametags_visible = excluded.nametags_visible,
                        blur_required = excluded.blur_required,
                        used_in = excluded.used_in,
                        crop_x = excluded.crop_x,
                        notes = excluded.notes,
                        import_id = excluded.import_id,
                        updated_at = excluded.updated_at"
                ),
                params![
                    id,
                    owner_id,
                    span.video_id,
                    span.source,
                    span.external_id,
                    span.start_s,
                    span.end_s,
                    span_boundary_basis_to_str(span.boundary_basis),
                    span.boundary_tolerance_s,
                    span.library_relative_offset_s,
                    span.description,
                    span.shot_type,
                    span.camera_move,
                    span.subjects,
                    span.action,
                    span.tags,
                    span.quality,
                    i64::from(span.standout),
                    i64::from(span.usable),
                    i64::from(span.faces_visible),
                    i64::from(span.nametags_visible),
                    i64::from(span.blur_required),
                    span.used_in,
                    span.crop_x,
                    span.notes,
                    span.import_id,
                    span.imported_at.to_rfc3339(),
                    span.updated_at.to_rfc3339(),
                ],
            )
            .context("failed to upsert manual span")?;
        self.manual_span_by_id(owner_id, &id)?
            .context("manual span disappeared after upsert")
    }

    pub fn manual_span_by_id(
        &self,
        owner_id: &str,
        id: &str,
    ) -> anyhow::Result<Option<ManualSpan>> {
        self.connection
            .query_row(
                &format!("SELECT {MANUAL_SPAN_COLUMNS} FROM manual_spans WHERE owner_id = ?1 AND id = ?2"),
                params![owner_id, id],
                manual_span_from_row,
            )
            .optional()
            .context("failed to read manual span")
    }

    pub fn manual_span_by_external_id(
        &self,
        owner_id: &str,
        source: &str,
        external_id: &str,
    ) -> anyhow::Result<Option<ManualSpan>> {
        self.connection
            .query_row(
                &format!(
                    "SELECT {MANUAL_SPAN_COLUMNS} FROM manual_spans
                     WHERE owner_id = ?1 AND source = ?2 AND external_id = ?3"
                ),
                params![owner_id, source, external_id],
                manual_span_from_row,
            )
            .optional()
            .context("failed to read manual span by external id")
    }

    pub fn manual_spans_for_video(
        &self,
        owner_id: &str,
        video_id: &str,
    ) -> anyhow::Result<Vec<ManualSpan>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {MANUAL_SPAN_COLUMNS} FROM manual_spans
             WHERE owner_id = ?1 AND video_id = ?2 ORDER BY start_s, end_s, id"
        ))?;
        let rows = statement.query_map(params![owner_id, video_id], manual_span_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list manual spans for video")
    }

    pub fn manual_spans(&self, owner_id: &str) -> anyhow::Result<Vec<ManualSpan>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {MANUAL_SPAN_COLUMNS} FROM manual_spans
             WHERE owner_id = ?1 ORDER BY video_id, start_s, end_s, id"
        ))?;
        let rows = statement.query_map(params![owner_id], manual_span_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list manual spans")
    }

    pub fn manual_span_delete(&self, owner_id: &str, id: &str) -> anyhow::Result<bool> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM manual_spans WHERE owner_id = ?1 AND id = ?2",
                params![owner_id, id],
            )
            .context("failed to delete manual span")?;
        Ok(changed == 1)
    }

    /// Spans that are candidates for the explicit "confirm imported evidence" decision
    /// (Task 034): spans with import lineage or catalogue evidence (quality/standout/
    /// used_in). This is derived from the span rows themselves — never from a stored dry-run
    /// report, which goes stale. Each row reports which reference sets already hold it and
    /// whether any of them is confirmed, so the Preferences panel shows the real state.
    pub fn imported_evidence_spans(
        &self,
        owner_id: &str,
    ) -> anyhow::Result<Vec<ImportedEvidenceSpan>> {
        let mut statement = self.connection.prepare(
            "SELECT ms.id, ms.external_id, ms.source, ms.import_id, v.path, ms.start_s, ms.end_s,
                    ms.description, ms.quality, ms.standout, ms.used_in, ms.imported_at
             FROM manual_spans ms
             JOIN videos v ON v.id = ms.video_id AND v.owner_id = ms.owner_id
             WHERE ms.owner_id = ?1
               AND (ms.import_id IS NOT NULL OR ms.quality IS NOT NULL OR ms.standout = 1
                    OR ms.used_in <> '')
             ORDER BY ms.imported_at, ms.video_id, ms.start_s, ms.id",
        )?;
        let mut spans = statement
            .query_map(params![owner_id], |row| {
                let imported_at: String = row.get(11)?;
                Ok(ImportedEvidenceSpan {
                    span_id: row.get(0)?,
                    external_id: row.get(1)?,
                    source: row.get(2)?,
                    import_id: row.get(3)?,
                    video_path: row.get(4)?,
                    start_s: row.get(5)?,
                    end_s: row.get(6)?,
                    description: row.get(7)?,
                    quality: row.get(8)?,
                    standout: row.get::<_, i64>(9)? != 0,
                    used_in: row.get(10)?,
                    imported_at: timestamp_from_str(&imported_at, 11)?,
                    sets: Vec::new(),
                    confirmed: false,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("failed to list imported evidence spans")?;

        // Attach reference-set membership (any status) in one pass; the confirmed flag
        // reflects whether the explicit confirmation step has actually happened.
        let mut membership = self.connection.prepare(
            "SELECT i.media_id, s.name, s.status
                 FROM reference_set_items i
                 JOIN reference_sets s ON s.id = i.set_id AND s.owner_id = i.owner_id
                 WHERE i.owner_id = ?1 AND i.media_kind = 'span'
                 ORDER BY s.name",
        )?;
        let rows = membership.query_map(params![owner_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut by_span: std::collections::HashMap<String, Vec<(String, String)>> =
            std::collections::HashMap::new();
        for row in rows {
            let (media_id, name, status) = row.context("failed to read span set membership")?;
            by_span.entry(media_id).or_default().push((name, status));
        }
        for span in &mut spans {
            if let Some(members) = by_span.get(&span.span_id) {
                span.sets = members.iter().map(|(name, _)| name.clone()).collect();
                span.confirmed = members.iter().any(|(_, status)| {
                    status == reference_status_to_str(ReferenceSetStatus::Confirmed)
                });
            }
        }
        Ok(spans)
    }

    /// Append one ledger row. The ledger is append-only at the database layer.
    pub fn catalogue_import_append(
        &self,
        owner_id: &str,
        import: &CatalogueImport,
    ) -> anyhow::Result<()> {
        ensure_owner_matches(owner_id, &import.owner_id, "catalogue import")?;
        ensure!(
            !import.id.trim().is_empty(),
            "catalogue import id must not be empty"
        );
        ensure!(
            matches!(import.mode.as_str(), "dry_run" | "apply"),
            "catalogue import mode must be dry_run or apply"
        );
        self.connection
            .execute(
                "INSERT INTO catalogue_imports (
                    id, owner_id, source, mode, catalogue_path, catalogue_sha256, recipes_json,
                    report_json, started_at, finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    import.id,
                    owner_id,
                    import.source,
                    import.mode,
                    import.catalogue_path,
                    import.catalogue_sha256,
                    import.recipes_json,
                    import.report_json,
                    import.started_at.to_rfc3339(),
                    import.finished_at.to_rfc3339(),
                ],
            )
            .context("failed to append catalogue import")?;
        Ok(())
    }

    pub fn catalogue_imports(&self, owner_id: &str) -> anyhow::Result<Vec<CatalogueImport>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner_id, source, mode, catalogue_path, catalogue_sha256, recipes_json,
                    report_json, started_at, finished_at
             FROM catalogue_imports WHERE owner_id = ?1 ORDER BY started_at DESC, id",
        )?;
        let rows = statement.query_map(params![owner_id], catalogue_import_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .context("failed to list catalogue imports")
    }
}
