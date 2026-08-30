//! Reel Studio historical-evidence importer (Task 022).
//!
//! Reads a Reel Studio `clips.db` catalogue plus exported reel recipes and maps them onto the
//! unified DAM schema: catalogue segments become owner-scoped [`ManualSpan`]s on the ORIGINAL
//! source timeline, recipes become immutable reel v2 [`RenderRecipe`]s with `historical`
//! provenance and a Projects plan whose items carry that provenance explicitly.
//!
//! Honesty rules baked in here:
//! * nothing from the private catalogue or media is ever copied into the repository; only paths,
//!   hashes and evidence go into the owner's local store;
//! * discovering finished projects never creates reference sets or feedback events — the user
//!   confirms those separately;
//! * dry-run reports every mapping, gap and planned write before anything is stored;
//! * re-running is idempotent: spans are keyed by external id, recipes by content, plans by name;
//! * segment timing that Reel Studio measured against a keyframe-aligned library copy is stored
//!   with its tolerance instead of being presented as frame-exact.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use crush_core::DEFAULT_OWNER_ID;
use crush_stage_split::ffmpeg;
use crush_store::{
    CatalogueImport, ManualSpan, MediaKind, Plan, PlanItem, PlanOrigin, RenderRecipe,
    RenderRecipeKind, SpanBoundaryBasis, Store, Video,
};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    reel_recipe::{parse_frozen_reel_recipe_v2, resolve_reel_recipe, SegmentSourceSpan},
    sha256_file,
};

pub const IMPORT_SOURCE: &str = "reel_studio";

/// Default slack for segments whose library copy was a keyframe-aligned stream copy.
pub const DEFAULT_KEYFRAME_TOLERANCE_S: f64 = 1.0;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub catalogue: PathBuf,
    /// Directories searched for original source files (`source_clips.source_file` or
    /// `<clip_id>.mp4`).
    pub originals: Vec<PathBuf>,
    /// Optional Reel Studio library folder holding `clips/<segment_id>.mp4`.
    pub library: Option<PathBuf>,
    pub recipes: Vec<PathBuf>,
    pub context_key: String,
    pub apply: bool,
    /// Hash original files to match videos whose stored path differs (slow on 4K footage).
    pub match_by_hash: bool,
    pub keyframe_tolerance_s: f64,
    pub threads: usize,
}

impl ImportOptions {
    pub fn dry_run(catalogue: impl Into<PathBuf>) -> Self {
        Self {
            catalogue: catalogue.into(),
            originals: Vec::new(),
            library: None,
            recipes: Vec::new(),
            context_key: "default".to_owned(),
            apply: false,
            match_by_hash: false,
            keyframe_tolerance_s: DEFAULT_KEYFRAME_TOLERANCE_S,
            threads: 0,
        }
    }
}

// ---- Catalogue rows -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogueSourceClip {
    pub clip_id: String,
    pub source_file: String,
    pub duration_s: Option<f64>,
    pub exhibit: Option<String>,
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogueSegment {
    pub segment_id: String,
    pub clip_id: String,
    pub tc_in_s: f64,
    pub tc_out_s: f64,
    pub description: String,
    pub shot_type: String,
    pub camera_move: String,
    pub subjects: String,
    pub action: String,
    pub tags: String,
    pub quality: Option<i64>,
    pub standout: bool,
    pub faces_visible: bool,
    pub nametags_visible: bool,
    pub blur_required: bool,
    pub usable: bool,
    pub used_in: String,
    pub library_file: Option<String>,
    pub notes: String,
    pub crop_x: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catalogue {
    pub clips: Vec<CatalogueSourceClip>,
    pub segments: Vec<CatalogueSegment>,
}

/// Read the catalogue read-only. The file is never modified.
pub fn read_catalogue(path: &Path) -> anyhow::Result<Catalogue> {
    ensure!(path.is_file(), "catalogue {} is not a file", path.display());
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open catalogue {}", path.display()))?;
    let mut clips = Vec::new();
    {
        let mut statement = connection
            .prepare("SELECT clip_id, source_file, duration, exhibit, theme FROM source_clips ORDER BY clip_id")
            .context("catalogue has no source_clips table")?;
        let rows = statement.query_map([], |row| {
            Ok(CatalogueSourceClip {
                clip_id: row.get(0)?,
                source_file: row.get(1)?,
                duration_s: row.get(2)?,
                exhibit: row.get(3)?,
                theme: row.get(4)?,
            })
        })?;
        for row in rows {
            clips.push(row?);
        }
    }
    let mut segments = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT segment_id, clip_id, tc_in, tc_out, description, shot_type, camera_move,
                        subjects, action, tags, quality, standout, faces_visible, nametags_visible,
                        blur_required, usable, used_in, library_file, notes, crop_x
                 FROM segments ORDER BY segment_id",
            )
            .context("catalogue has no segments table")?;
        let rows = statement.query_map([], |row| {
            Ok(CatalogueSegment {
                segment_id: row.get(0)?,
                clip_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                tc_in_s: row.get::<_, Option<f64>>(2)?.unwrap_or(f64::NAN),
                tc_out_s: row.get::<_, Option<f64>>(3)?.unwrap_or(f64::NAN),
                description: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                shot_type: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                camera_move: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                subjects: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                action: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                tags: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                quality: row.get(10)?,
                standout: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
                faces_visible: row.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
                nametags_visible: row.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0,
                blur_required: row.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0,
                usable: row.get::<_, Option<i64>>(15)?.unwrap_or(1) != 0,
                used_in: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
                library_file: row.get(17)?,
                notes: row.get::<_, Option<String>>(18)?.unwrap_or_default(),
                crop_x: row.get(19)?,
            })
        })?;
        for row in rows {
            segments.push(row?);
        }
    }
    Ok(Catalogue { clips, segments })
}

// ---- Report ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceMapping {
    pub clip_id: String,
    pub source_file: String,
    pub resolved_path: Option<String>,
    pub video_id: Option<String>,
    /// `path`, `sha256`, `missing_file`, `not_indexed`
    pub matched_by: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SegmentMapping {
    pub segment_id: String,
    pub clip_id: String,
    pub video_id: Option<String>,
    pub start_s: f64,
    pub end_s: f64,
    pub boundary_basis: String,
    pub boundary_tolerance_s: f64,
    /// `new`, `updated`, `unchanged`, `skipped`
    pub outcome: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RecipeMapping {
    pub file: String,
    pub recipe_id: String,
    pub plan_name: String,
    pub items: usize,
    pub finished_project: bool,
    /// `new`, `unchanged`, `skipped`
    pub outcome: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportIssue {
    /// `missing_source`, `not_indexed`, `duplicate`, `unsupported`, `out_of_range`, `unknown_segment`
    pub kind: String,
    pub subject: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct PlannedWrites {
    pub manual_spans_insert: usize,
    pub manual_spans_update: usize,
    pub render_recipes_insert: usize,
    pub plans_insert: usize,
    pub plan_items_insert: usize,
    pub plan_revisions_insert: usize,
    pub feedback_events_insert: usize,
    pub reference_sets_insert: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportReport {
    pub import_id: String,
    pub mode: String,
    pub catalogue_path: String,
    pub catalogue_sha256: String,
    pub context_key: String,
    pub sources: Vec<SourceMapping>,
    pub segments: Vec<SegmentMapping>,
    pub recipes: Vec<RecipeMapping>,
    pub issues: Vec<ImportIssue>,
    pub planned_writes: PlannedWrites,
    /// Finished projects (recipes whose segments carry `used_in`) that the user may promote to a
    /// named previous-work reference set. Discovery alone never trains the personal model.
    pub reference_set_candidates: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl ImportReport {
    pub fn summary_line(&self) -> String {
        let matched = self.sources.iter().filter(|s| s.video_id.is_some()).count();
        let spans_new = self.segments.iter().filter(|s| s.outcome == "new").count();
        let spans_updated = self
            .segments
            .iter()
            .filter(|s| s.outcome == "updated")
            .count();
        let spans_unchanged = self
            .segments
            .iter()
            .filter(|s| s.outcome == "unchanged")
            .count();
        let spans_skipped = self
            .segments
            .iter()
            .filter(|s| s.outcome == "skipped")
            .count();
        let recipes_new = self.recipes.iter().filter(|r| r.outcome == "new").count();
        format!(
            "{}: sources {}/{} matched · spans new={} updated={} unchanged={} skipped={} · recipes new={}/{} · issues={}",
            self.mode,
            matched,
            self.sources.len(),
            spans_new,
            spans_updated,
            spans_unchanged,
            spans_skipped,
            recipes_new,
            self.recipes.len(),
            self.issues.len()
        )
    }
}

// ---- Importer --------------------------------------------------------------------------------

struct MatchedSource {
    video: Video,
    resolved_path: PathBuf,
}

struct DerivedSpan {
    span: ManualSpan,
    mapping: SegmentMapping,
}

struct NormalizedRecipe {
    file: PathBuf,
    recipe_id: String,
    plan_name: String,
    schema_json: String,
    /// segment id -> (span id, span) for the sequence, in order.
    sequence: Vec<(String, ManualSpan, f64, f64)>,
    finished_project: bool,
}

/// Run a dry-run or apply. Reads the catalogue and recipes, matches originals against the owner's
/// indexed videos, derives spans and recipes, and (when `apply`) writes them. Both modes append a
/// ledger row so the history of what was inspected is auditable.
pub fn import_reel_studio(
    store: &mut Store,
    options: &ImportOptions,
) -> anyhow::Result<ImportReport> {
    let started_at = Utc::now();
    let import_id = format!("import-{}", Uuid::new_v4());
    let catalogue_sha256 = sha256_file(&options.catalogue)
        .with_context(|| format!("failed to hash catalogue {}", options.catalogue.display()))?;
    let catalogue = read_catalogue(&options.catalogue)?;
    ensure!(
        options.keyframe_tolerance_s.is_finite() && options.keyframe_tolerance_s >= 0.0,
        "keyframe tolerance must be finite and non-negative"
    );
    let mut issues = Vec::new();

    // 1. Sources -> indexed videos.
    let mut sources = Vec::new();
    let mut matched: BTreeMap<String, MatchedSource> = BTreeMap::new();
    let mut seen_clips = BTreeSet::new();
    for clip in &catalogue.clips {
        if !seen_clips.insert(clip.clip_id.clone()) {
            issues.push(ImportIssue {
                kind: "duplicate".to_owned(),
                subject: clip.clip_id.clone(),
                detail: "source clip id appears more than once in the catalogue".to_owned(),
            });
            continue;
        }
        let resolved = resolve_original(&options.originals, clip);
        let (video, matched_by) = match &resolved {
            None => {
                issues.push(ImportIssue {
                    kind: "missing_source".to_owned(),
                    subject: clip.clip_id.clone(),
                    detail: format!(
                        "{} was not found under the given originals directories",
                        clip.source_file
                    ),
                });
                (None, "missing_file")
            }
            Some(path) => match find_video(store, path, options.match_by_hash)? {
                Some((video, by)) => (Some(video), by),
                None => {
                    issues.push(ImportIssue {
                        kind: "not_indexed".to_owned(),
                        subject: clip.clip_id.clone(),
                        detail: format!(
                            "{} exists but Crush has not indexed it; add its folder first",
                            path.display()
                        ),
                    });
                    (None, "not_indexed")
                }
            },
        };
        sources.push(SourceMapping {
            clip_id: clip.clip_id.clone(),
            source_file: clip.source_file.clone(),
            resolved_path: resolved.as_ref().map(|path| path.display().to_string()),
            video_id: video.as_ref().map(|video| video.id.clone()),
            matched_by: matched_by.to_owned(),
        });
        if let (Some(video), Some(path)) = (video, resolved) {
            matched.insert(
                clip.clip_id.clone(),
                MatchedSource {
                    video,
                    resolved_path: path,
                },
            );
        }
    }

    // 2. Segments -> manual spans.
    let runner = if options.library.is_some() {
        Some(ffmpeg::Runner::new(
            ffmpeg::resolve()?,
            options.threads,
            "reel-studio-import",
        ))
    } else {
        None
    };
    let mut derived: Vec<DerivedSpan> = Vec::new();
    let mut spans_by_segment: BTreeMap<String, ManualSpan> = BTreeMap::new();
    let mut seen_segments = BTreeSet::new();
    let now = Utc::now();
    for segment in &catalogue.segments {
        if !seen_segments.insert(segment.segment_id.clone()) {
            issues.push(ImportIssue {
                kind: "duplicate".to_owned(),
                subject: segment.segment_id.clone(),
                detail: "segment id appears more than once in the catalogue".to_owned(),
            });
            continue;
        }
        let Some(source) = matched.get(&segment.clip_id) else {
            derived.push(DerivedSpan {
                mapping: SegmentMapping {
                    segment_id: segment.segment_id.clone(),
                    clip_id: segment.clip_id.clone(),
                    video_id: None,
                    start_s: segment.tc_in_s,
                    end_s: segment.tc_out_s,
                    boundary_basis: "catalogue_tc".to_owned(),
                    boundary_tolerance_s: 0.0,
                    outcome: "skipped".to_owned(),
                    reason: Some("source clip is not matched to an indexed video".to_owned()),
                },
                span: placeholder_span(segment, now),
            });
            continue;
        };
        if !(segment.tc_in_s.is_finite()
            && segment.tc_out_s.is_finite()
            && segment.tc_in_s >= 0.0
            && segment.tc_out_s > segment.tc_in_s)
        {
            issues.push(ImportIssue {
                kind: "unsupported".to_owned(),
                subject: segment.segment_id.clone(),
                detail: format!(
                    "tc_in/tc_out {:?}..{:?} are not a valid interval",
                    segment.tc_in_s, segment.tc_out_s
                ),
            });
            continue;
        }
        if let Some(duration) = source.video.duration_s {
            if segment.tc_out_s > duration + 0.001 {
                issues.push(ImportIssue {
                    kind: "out_of_range".to_owned(),
                    subject: segment.segment_id.clone(),
                    detail: format!(
                        "tc_out {:.3} exceeds the indexed source duration {:.3}",
                        segment.tc_out_s, duration
                    ),
                });
                continue;
            }
        }
        let (basis, tolerance, basis_note) =
            boundary_basis(options, runner.as_ref(), segment, &source.video);
        let span = ManualSpan {
            id: format!("span-{}", Uuid::new_v4()),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            video_id: source.video.id.clone(),
            source: IMPORT_SOURCE.to_owned(),
            external_id: segment.segment_id.clone(),
            start_s: segment.tc_in_s,
            end_s: segment.tc_out_s,
            boundary_basis: basis,
            boundary_tolerance_s: tolerance,
            library_relative_offset_s: 0.0,
            description: segment.description.clone(),
            shot_type: segment.shot_type.clone(),
            camera_move: segment.camera_move.clone(),
            subjects: segment.subjects.clone(),
            action: segment.action.clone(),
            tags: segment.tags.clone(),
            quality: segment.quality.filter(|quality| (1..=5).contains(quality)),
            standout: segment.standout,
            usable: segment.usable,
            faces_visible: segment.faces_visible,
            nametags_visible: segment.nametags_visible,
            blur_required: segment.blur_required,
            used_in: segment.used_in.clone(),
            crop_x: segment.crop_x.filter(|x| (0.0..=1.0).contains(x)),
            notes: segment.notes.clone(),
            import_id: Some(import_id.clone()),
            imported_at: now,
            updated_at: now,
        };
        if segment
            .quality
            .is_some_and(|quality| !(1..=5).contains(&quality))
        {
            issues.push(ImportIssue {
                kind: "unsupported".to_owned(),
                subject: segment.segment_id.clone(),
                detail: format!(
                    "quality {:?} is outside 1..=5 and was dropped",
                    segment.quality
                ),
            });
        }
        let existing = store.manual_span_by_external_id(
            DEFAULT_OWNER_ID,
            IMPORT_SOURCE,
            &segment.segment_id,
        )?;
        let outcome = match &existing {
            None => "new",
            Some(existing) if span_evidence_equal(existing, &span) => "unchanged",
            Some(_) => "updated",
        };
        let mapping = SegmentMapping {
            segment_id: segment.segment_id.clone(),
            clip_id: segment.clip_id.clone(),
            video_id: Some(source.video.id.clone()),
            start_s: span.start_s,
            end_s: span.end_s,
            boundary_basis: crush_store::span_boundary_basis_to_str(basis).to_owned(),
            boundary_tolerance_s: tolerance,
            outcome: outcome.to_owned(),
            reason: basis_note,
        };
        let effective = existing
            .map(|existing| ManualSpan {
                id: existing.id,
                ..span.clone()
            })
            .unwrap_or_else(|| span.clone());
        spans_by_segment.insert(segment.segment_id.clone(), effective);
        derived.push(DerivedSpan { span, mapping });
        let _ = &source.resolved_path;
    }

    // 3. Recipes -> reel v2 recipes + historical plans.
    let mut recipes = Vec::new();
    let mut reference_set_candidates = Vec::new();
    for file in &options.recipes {
        match normalize_recipe(store, file, &spans_by_segment, &options.context_key) {
            Ok(recipe) => {
                let existing_versions =
                    store.render_recipes(DEFAULT_OWNER_ID, Some(RenderRecipeKind::Reel))?;
                let unchanged = existing_versions.iter().any(|existing| {
                    existing.id == recipe.recipe_id && existing.schema_json == recipe.schema_json
                });
                let plan_exists = store
                    .plan_list(DEFAULT_OWNER_ID)?
                    .iter()
                    .any(|plan| plan.name == recipe.plan_name);
                let (outcome, reason) = if unchanged && plan_exists {
                    ("unchanged", None)
                } else if plan_exists {
                    (
                        "skipped",
                        Some(format!(
                            "project {:?} already exists; edit it in Projects or rename the recipe file",
                            recipe.plan_name
                        )),
                    )
                } else {
                    ("new", None)
                };
                if recipe.finished_project {
                    reference_set_candidates.push(recipe.plan_name.clone());
                }
                recipes.push((
                    RecipeMapping {
                        file: file.display().to_string(),
                        recipe_id: recipe.recipe_id.clone(),
                        plan_name: recipe.plan_name.clone(),
                        items: recipe.sequence.len(),
                        finished_project: recipe.finished_project,
                        outcome: outcome.to_owned(),
                        reason,
                    },
                    Some(recipe),
                ));
            }
            Err(error) => {
                issues.push(ImportIssue {
                    kind: "unsupported".to_owned(),
                    subject: file.display().to_string(),
                    detail: format!("{error:#}"),
                });
                recipes.push((
                    RecipeMapping {
                        file: file.display().to_string(),
                        recipe_id: String::new(),
                        plan_name: String::new(),
                        items: 0,
                        finished_project: false,
                        outcome: "skipped".to_owned(),
                        reason: Some(format!("{error:#}")),
                    },
                    None,
                ));
            }
        }
    }

    // 4. Planned writes.
    let mut planned = PlannedWrites::default();
    for item in &derived {
        match item.mapping.outcome.as_str() {
            "new" => planned.manual_spans_insert += 1,
            "updated" => planned.manual_spans_update += 1,
            _ => {}
        }
    }
    for (mapping, recipe) in &recipes {
        if mapping.outcome == "new" {
            if let Some(recipe) = recipe {
                planned.render_recipes_insert += 1;
                planned.plans_insert += 1;
                planned.plan_items_insert += recipe.sequence.len();
                planned.plan_revisions_insert += 1;
            }
        }
    }

    // 5. Apply.
    if options.apply {
        for item in &derived {
            if matches!(item.mapping.outcome.as_str(), "new" | "updated") {
                store.manual_span_upsert(DEFAULT_OWNER_ID, &item.span)?;
            }
        }
        for (mapping, recipe) in &recipes {
            let Some(recipe) = recipe else { continue };
            if mapping.outcome != "new" {
                continue;
            }
            // Re-resolve span ids after the upserts above (new spans now have stored ids).
            let mut resolved_sequence = Vec::with_capacity(recipe.sequence.len());
            for (segment_id, _, start_s, end_s) in &recipe.sequence {
                let span = store
                    .manual_span_by_external_id(DEFAULT_OWNER_ID, IMPORT_SOURCE, segment_id)?
                    .with_context(|| {
                        format!("span {segment_id} was not stored before its recipe")
                    })?;
                resolved_sequence.push((segment_id.clone(), span, *start_s, *end_s));
            }
            let version = store
                .render_recipes(DEFAULT_OWNER_ID, Some(RenderRecipeKind::Reel))?
                .iter()
                .filter(|existing| existing.id == recipe.recipe_id)
                .map(|existing| existing.version)
                .max()
                .unwrap_or(0)
                + 1;
            store.render_recipe_create(
                DEFAULT_OWNER_ID,
                &RenderRecipe {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    id: recipe.recipe_id.clone(),
                    version,
                    kind: RenderRecipeKind::Reel,
                    name: recipe.plan_name.clone(),
                    schema_json: recipe.schema_json.clone(),
                    created_at: now,
                },
            )?;
            let plan_id = format!("plan-{}", Uuid::new_v4());
            store.plan_create(
                DEFAULT_OWNER_ID,
                &Plan {
                    id: plan_id.clone(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    name: recipe.plan_name.clone(),
                    description: format!(
                        "Imported from Reel Studio recipe {} (historical; recipe {} v{})",
                        recipe.file.display(),
                        recipe.recipe_id,
                        version
                    ),
                    context_key: options.context_key.clone(),
                    brief: String::new(),
                    created_at: now,
                    updated_at: now,
                },
            )?;
            for (segment_id, span, start_s, end_s) in &resolved_sequence {
                store.plan_add_item(
                    DEFAULT_OWNER_ID,
                    &PlanItem {
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        plan_id: plan_id.clone(),
                        media_kind: MediaKind::Span,
                        media_id: span.id.clone(),
                        position: 0,
                        start_s: Some(*start_s),
                        end_s: Some(*end_s),
                        pacing: None,
                        crop_x: span.crop_x,
                        grade_json: None,
                        reason: format!("Reel Studio segment {segment_id} (historical choice)"),
                        signals_json: "{}".to_owned(),
                        origin: PlanOrigin::Historical,
                        rank: None,
                        profile_version: None,
                        provenance_json: json!({
                            "source": IMPORT_SOURCE,
                            "external_id": segment_id,
                            "import_id": import_id,
                            "recipe_id": recipe.recipe_id,
                            "recipe_version": version,
                            "boundary_basis": crush_store::span_boundary_basis_to_str(span.boundary_basis),
                            "boundary_tolerance_s": span.boundary_tolerance_s,
                        })
                        .to_string(),
                        added_at: now,
                    },
                )?;
            }
            store.plan_save_revision(DEFAULT_OWNER_ID, &plan_id, "imported from Reel Studio")?;
        }
    }

    let finished_at = Utc::now();
    let report = ImportReport {
        import_id: import_id.clone(),
        mode: if options.apply { "apply" } else { "dry_run" }.to_owned(),
        catalogue_path: options.catalogue.display().to_string(),
        catalogue_sha256: catalogue_sha256.clone(),
        context_key: options.context_key.clone(),
        sources,
        segments: derived.into_iter().map(|item| item.mapping).collect(),
        recipes: recipes.into_iter().map(|(mapping, _)| mapping).collect(),
        issues,
        planned_writes: planned,
        reference_set_candidates,
        started_at,
        finished_at,
    };
    store.catalogue_import_append(
        DEFAULT_OWNER_ID,
        &CatalogueImport {
            id: import_id,
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            source: IMPORT_SOURCE.to_owned(),
            mode: report.mode.clone(),
            catalogue_path: report.catalogue_path.clone(),
            catalogue_sha256,
            recipes_json: serde_json::to_string(
                &options
                    .recipes
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            )?,
            report_json: serde_json::to_string(&report)?,
            started_at,
            finished_at,
        },
    )?;
    Ok(report)
}

// ---- helpers ---------------------------------------------------------------------------------

fn resolve_original(originals: &[PathBuf], clip: &CatalogueSourceClip) -> Option<PathBuf> {
    let file_name = Path::new(&clip.source_file)
        .file_name()
        .map(|name| name.to_owned())?;
    let mut candidates = vec![
        PathBuf::from(&file_name),
        PathBuf::from(format!("{}.mp4", clip.clip_id)),
    ];
    if Path::new(&clip.source_file).is_absolute() {
        candidates.insert(0, PathBuf::from(&clip.source_file));
    }
    for candidate in &candidates {
        if candidate.is_absolute() && candidate.is_file() {
            return Some(candidate.clone());
        }
        for directory in originals {
            let path = directory.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn find_video(
    store: &Store,
    path: &Path,
    match_by_hash: bool,
) -> anyhow::Result<Option<(Video, &'static str)>> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for candidate in [path.to_path_buf(), canonical] {
        if let Some(video) = store.video_by_path(DEFAULT_OWNER_ID, &candidate.to_string_lossy())? {
            return Ok(Some((video, "path")));
        }
    }
    if match_by_hash {
        let sha256 = sha256_file(path)?;
        if let Some(video) = store.video_by_sha(DEFAULT_OWNER_ID, &sha256)? {
            return Ok(Some((video, "sha256")));
        }
    }
    Ok(None)
}

/// Decide how much to trust the catalogue timecodes. Reel Studio's 4K library clips were cut
/// with `-ss <tc_in> -c copy`, i.e. keyframe-aligned, so recipe times measured against them can
/// drift from `tc_in + t` by up to one GOP. 1080p browse copies were re-encoded and are exact.
fn boundary_basis(
    options: &ImportOptions,
    runner: Option<&ffmpeg::Runner>,
    segment: &CatalogueSegment,
    video: &Video,
) -> (SpanBoundaryBasis, f64, Option<String>) {
    let frame = video
        .fps
        .filter(|fps| *fps > 0.0)
        .map_or(0.05, |fps| 1.0 / fps);
    let Some(library) = options.library.as_ref() else {
        return (
            SpanBoundaryBasis::CatalogueTc,
            options.keyframe_tolerance_s,
            Some(
                "no library folder; catalogue timecodes taken literally with keyframe tolerance"
                    .to_owned(),
            ),
        );
    };
    let relative = segment
        .library_file
        .clone()
        .unwrap_or_else(|| format!("clips/{}.mp4", segment.segment_id));
    let library_file = library.join(&relative);
    let Some(runner) = runner else {
        return (
            SpanBoundaryBasis::CatalogueTc,
            options.keyframe_tolerance_s,
            None,
        );
    };
    match runner.probe(&library_file) {
        Ok(probe) => {
            let probe = probe.value;
            let expected = segment.tc_out_s - segment.tc_in_s;
            let delta = (probe.duration_s - expected).abs();
            if probe.width <= 1920 && delta <= frame + 0.05 {
                (
                    SpanBoundaryBasis::LibraryProbe,
                    frame,
                    Some(format!(
                        "re-encoded {}x{} library copy matches the catalogue interval within {:.3}s",
                        probe.width, probe.height, delta
                    )),
                )
            } else {
                (
                    SpanBoundaryBasis::CatalogueTc,
                    options.keyframe_tolerance_s.max(delta),
                    Some(format!(
                        "{}x{} library copy is a keyframe-aligned stream copy (duration delta {:.3}s); recipe times may be offset by up to the tolerance",
                        probe.width, probe.height, delta
                    )),
                )
            }
        }
        Err(error) => (
            SpanBoundaryBasis::CatalogueTc,
            options.keyframe_tolerance_s,
            Some(format!(
                "library copy {} could not be probed ({error}); catalogue timecodes taken literally",
                library_file.display()
            )),
        ),
    }
}

fn placeholder_span(segment: &CatalogueSegment, now: DateTime<Utc>) -> ManualSpan {
    ManualSpan {
        id: String::new(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        video_id: String::new(),
        source: IMPORT_SOURCE.to_owned(),
        external_id: segment.segment_id.clone(),
        start_s: segment.tc_in_s,
        end_s: segment.tc_out_s,
        boundary_basis: SpanBoundaryBasis::CatalogueTc,
        boundary_tolerance_s: 0.0,
        library_relative_offset_s: 0.0,
        description: String::new(),
        shot_type: String::new(),
        camera_move: String::new(),
        subjects: String::new(),
        action: String::new(),
        tags: String::new(),
        quality: None,
        standout: false,
        usable: true,
        faces_visible: false,
        nametags_visible: false,
        blur_required: false,
        used_in: String::new(),
        crop_x: None,
        notes: String::new(),
        import_id: None,
        imported_at: now,
        updated_at: now,
    }
}

fn span_evidence_equal(a: &ManualSpan, b: &ManualSpan) -> bool {
    a.video_id == b.video_id
        && (a.start_s - b.start_s).abs() < 1e-9
        && (a.end_s - b.end_s).abs() < 1e-9
        && a.boundary_basis == b.boundary_basis
        && (a.boundary_tolerance_s - b.boundary_tolerance_s).abs() < 1e-9
        && a.description == b.description
        && a.shot_type == b.shot_type
        && a.camera_move == b.camera_move
        && a.subjects == b.subjects
        && a.action == b.action
        && a.tags == b.tags
        && a.quality == b.quality
        && a.standout == b.standout
        && a.usable == b.usable
        && a.faces_visible == b.faces_visible
        && a.nametags_visible == b.nametags_visible
        && a.blur_required == b.blur_required
        && a.used_in == b.used_in
        && a.crop_x == b.crop_x
        && a.notes == b.notes
}

/// Turn a raw Reel Studio recipe export into the strict frozen reel v2 schema, verify it parses
/// and resolves against imported spans, and derive the plan it represents.
fn normalize_recipe(
    store: &Store,
    file: &Path,
    spans: &BTreeMap<String, ManualSpan>,
    _context_key: &str,
) -> anyhow::Result<NormalizedRecipe> {
    let text = fs::read_to_string(file)
        .with_context(|| format!("failed to read recipe {}", file.display()))?;
    let raw: Value = serde_json::from_str(&text).context("recipe is not valid JSON")?;
    let reel = raw
        .get("reel")
        .and_then(Value::as_object)
        .context("recipe has no top-level \"reel\" object")?;
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .context("recipe file needs a name")?;
    let allowed_reel = [
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
    ];
    for key in reel.keys() {
        ensure!(
            allowed_reel.contains(&key.as_str()),
            "unsupported reel field {key:?}; refusing to discard it"
        );
    }
    let raw_sequence = reel
        .get("sequence")
        .and_then(Value::as_array)
        .context("recipe reel.sequence is required")?;
    let crops = reel.get("crops").and_then(Value::as_object);
    let mut sequence = Vec::with_capacity(raw_sequence.len());
    let mut crops_out = Map::new();
    for (index, item) in raw_sequence.iter().enumerate() {
        let item = item
            .as_object()
            .with_context(|| format!("sequence item {index} is not an object"))?;
        let allowed_item = [
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
        ];
        for key in item.keys() {
            ensure!(
                allowed_item.contains(&key.as_str()),
                "sequence item {index} has unsupported field {key:?}; refusing to discard it"
            );
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .with_context(|| format!("sequence item {index} has no id"))?;
        let crop_x = item
            .get("crop_x")
            .and_then(Value::as_f64)
            .or_else(|| {
                crops
                    .and_then(|crops| crops.get(id))
                    .and_then(Value::as_f64)
            })
            .or_else(|| spans.get(id).and_then(|span| span.crop_x))
            .unwrap_or(0.5);
        let grade = item
            .get("grade")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut grade_out = Map::new();
        for (key, default) in [
            ("b", 100.0),
            ("c", 100.0),
            ("s", 100.0),
            ("t", 0.0),
            ("h", 0.0),
            ("v", 0.0),
            ("sh", 0.0),
            ("hl", 0.0),
        ] {
            grade_out.insert(
                key.to_owned(),
                json!(grade.get(key).and_then(Value::as_f64).unwrap_or(default)),
            );
        }
        for key in grade.keys() {
            ensure!(
                grade_out.contains_key(key),
                "sequence item {index} grade has unsupported key {key:?}"
            );
        }
        let mut normalized = Map::new();
        normalized.insert("id".into(), json!(id));
        normalized.insert(
            "in".into(),
            item.get("in").cloned().context("sequence item needs in")?,
        );
        normalized.insert(
            "out".into(),
            item.get("out")
                .cloned()
                .context("sequence item needs out")?,
        );
        normalized.insert("crop_x".into(), json!(crop_x));
        normalized.insert(
            "crop_kf".into(),
            item.get("crop_kf").cloned().unwrap_or_else(|| json!([])),
        );
        normalized.insert(
            "caption".into(),
            item.get("caption").cloned().unwrap_or(Value::Null),
        );
        normalized.insert(
            "cap_pos".into(),
            item.get("cap_pos").cloned().unwrap_or_else(|| json!("low")),
        );
        normalized.insert(
            "transition".into(),
            item.get("transition")
                .cloned()
                .unwrap_or_else(|| json!("cut")),
        );
        normalized.insert(
            "speed".into(),
            item.get("speed").cloned().unwrap_or_else(|| json!(1.0)),
        );
        normalized.insert(
            "motion".into(),
            item.get("motion").cloned().unwrap_or_else(|| json!("none")),
        );
        normalized.insert(
            "clip_volume".into(),
            item.get("clip_volume")
                .cloned()
                .unwrap_or_else(|| json!(0.0)),
        );
        normalized.insert("grade".into(), Value::Object(grade_out));
        crops_out.insert(id.to_owned(), json!(crop_x));
        sequence.push(Value::Object(normalized));
    }
    if let Some(crops) = crops {
        for key in crops.keys() {
            ensure!(
                crops_out.contains_key(key),
                "crops entry {key:?} does not belong to a sequence item"
            );
        }
    }
    let recipe_id = format!("reel-studio-{}", slug(stem));
    let theme = reel
        .get("theme")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let plan_name = format!("Reel Studio · {}", theme.unwrap_or(stem));
    let schema = json!({
        "schema_version": 2,
        "kind": "reel",
        "provenance": {
            "origin": "historical",
            "source": IMPORT_SOURCE,
            "external_id": stem,
            "profile_version": null,
        },
        "theme": theme,
        "vibe": reel.get("vibe").cloned().unwrap_or(Value::Null),
        "music": reel.get("music").cloned().unwrap_or(Value::Null),
        "target_seconds": reel.get("target_seconds").cloned().unwrap_or(Value::Null),
        "beat_snap": reel.get("beat_snap").and_then(Value::as_bool).unwrap_or(false),
        "format": reel.get("format").cloned().unwrap_or_else(|| json!("9:16")),
        "music_volume": reel.get("music_volume").cloned().unwrap_or_else(|| json!(100.0)),
        "watermark": reel.get("watermark").cloned().unwrap_or(Value::Null),
        "cover": reel.get("cover").cloned().unwrap_or(Value::Null),
        "sequence": sequence,
        "crops": Value::Object(crops_out),
        "output": {"preset": "mp4-h264-sdr-v1"},
    });
    let schema_json = serde_json::to_string(&schema)?;
    let parsed = parse_frozen_reel_recipe_v2(&schema_json)
        .context("normalized recipe does not satisfy the frozen reel v2 contract")?;

    // Resolve every item against an imported span on the original timeline.
    let mut segments = BTreeMap::new();
    let mut used_in_any = false;
    let mut spans_in_order = Vec::new();
    for item in &parsed.sequence {
        let span = spans.get(&item.id).with_context(|| {
            format!(
                "sequence item {:?} is not a segment matched to an indexed video (unknown_segment)",
                item.id
            )
        })?;
        let video = store
            .video_by_id(DEFAULT_OWNER_ID, &span.video_id)?
            .with_context(|| format!("video {} for segment {} vanished", span.video_id, item.id))?;
        segments.insert(
            item.id.clone(),
            SegmentSourceSpan::new(
                video.id.clone(),
                video.path.clone(),
                span.start_s + span.library_relative_offset_s,
                span.end_s + span.library_relative_offset_s,
            )?,
        );
        used_in_any |= !span.used_in.trim().is_empty();
        spans_in_order.push((
            item.id.clone(),
            span.clone(),
            item.segment_in_s,
            item.segment_out_s,
        ));
    }
    let resolved = resolve_reel_recipe(&parsed, &segments)?;
    let mut sequence_out = Vec::with_capacity(resolved.sequence.len());
    for (resolved_item, (id, span, _, _)) in resolved.sequence.iter().zip(spans_in_order) {
        ensure!(
            resolved_item.source_in_s >= span.start_s - 1e-6
                && resolved_item.source_out_s <= span.end_s + 1e-6,
            "sequence item {id:?} in/out fall outside its imported span"
        );
        sequence_out.push((
            id,
            span.clone(),
            resolved_item.source_in_s.max(span.start_s),
            resolved_item.source_out_s.min(span.end_s),
        ));
    }
    Ok(NormalizedRecipe {
        file: file.to_path_buf(),
        recipe_id,
        plan_name,
        schema_json,
        sequence: sequence_out,
        finished_project: used_in_any,
    })
}

fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "recipe".to_owned()
    } else {
        trimmed.to_owned()
    }
}
