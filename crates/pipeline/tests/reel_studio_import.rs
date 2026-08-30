//! Task 022: Reel Studio importer against a synthetic catalogue over the repo's own fixtures.
//! No real catalogue rows or private media are used; the schema is Reel Studio's `schema.sql`.

use std::fs;
use std::path::{Path, PathBuf};

use crush_core::DEFAULT_OWNER_ID;
use crush_pipeline::reel_studio_import::{import_reel_studio, ImportOptions};
use crush_pipeline::sha256_file;
use crush_store::{
    MediaKind, PlanOrigin, RenderRecipeKind, Shot, SpanBoundaryBasis, Store, Video, VideoStatus,
};
use rusqlite::Connection;

#[cfg(target_os = "macos")]
use crush_core::{cancellation::CancellationToken, paths::AppPaths, Config};
#[cfg(target_os = "macos")]
use crush_pipeline::Pipeline;
#[cfg(target_os = "macos")]
use crush_store::{NewRenderJob, RenderRecipe};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/clips")
        .join(name)
        .canonicalize()
        .unwrap()
}

fn indexed_video(id: &str, path: &Path, duration_s: Option<f64>) -> Video {
    Video {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: path.to_string_lossy().into_owned(),
        sha256: sha256_file(path).unwrap(),
        duration_s,
        fps: Some(30.0),
        width: Some(1920),
        height: Some(1080),
        has_audio: true,
        status: VideoStatus::Done,
        indexed_at: None,
    }
}

/// (segment_id, clip_id, tc_in, tc_out, description, quality, standout, used_in, crop_x)
type SegmentRow<'a> = (
    &'a str,
    &'a str,
    f64,
    f64,
    &'a str,
    i64,
    i64,
    &'a str,
    Option<f64>,
);

/// Reel Studio's published schema, verbatim from `schema/schema.sql`.
fn write_catalogue(path: &Path, rows: &[SegmentRow<'_>]) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE segments (
              segment_id TEXT PRIMARY KEY, clip_id TEXT REFERENCES source_clips(clip_id),
              tc_in REAL, tc_out REAL, description TEXT, shot_type TEXT, camera_move TEXT,
              subjects TEXT, action TEXT, tags TEXT, quality INTEGER, standout INTEGER DEFAULT 0,
              faces_visible INTEGER DEFAULT 0, nametags_visible INTEGER DEFAULT 0,
              blur_required INTEGER DEFAULT 0, usable INTEGER DEFAULT 1, used_in TEXT DEFAULT '',
              library_file TEXT, thumb TEXT, preview TEXT, notes TEXT, crop_x REAL DEFAULT 0.5,
              vertical_file TEXT);
             CREATE VIRTUAL TABLE segments_fts USING fts5(segment_id, description, subjects, action, tags, content='');
             CREATE TABLE source_clips (
              clip_id TEXT PRIMARY KEY, source_file TEXT NOT NULL, suggested_name TEXT,
              duration REAL, resolution TEXT, fps TEXT, size_bytes INTEGER, exhibit TEXT, theme TEXT,
              has_audio INTEGER, avg_sharpness REAL, logged_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO source_clips (clip_id, source_file, exhibit, theme) VALUES
             ('V1-0001', 'earth-timelapse-silent.mp4', 'Healthy Earth', 'earth'),
             ('V1-0002', 'synthetic-speech.mp4', 'Voices', 'speech'),
             ('V1-0003', 'not-on-this-machine.mov', 'Missing', 'missing')",
            [],
        )
        .unwrap();
    for (segment_id, clip_id, tc_in, tc_out, description, quality, standout, used_in, crop_x) in
        rows
    {
        connection
            .execute(
                "INSERT INTO segments (segment_id, clip_id, tc_in, tc_out, description, subjects,
                                       action, tags, quality, standout, faces_visible, usable,
                                       used_in, crop_x)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'earth', 'turning', 'space,exhibit', ?6, ?7, 0, 1, ?8, ?9)",
                rusqlite::params![segment_id, clip_id, tc_in, tc_out, description, quality, standout, used_in, crop_x],
            )
            .unwrap();
    }
}

fn recipe_json(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn dry_run_reports_mappings_then_apply_is_idempotent_and_honest() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let mut store = Store::open(&data_dir).unwrap();
    let earth = fixture("earth-timelapse-silent.mp4");
    let speech = fixture("synthetic-speech.mp4");
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &indexed_video("video-earth", &earth, None),
        )
        .unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &indexed_video("video-speech", &speech, Some(5.0)),
        )
        .unwrap();

    let catalogue = temp.path().join("clips.db");
    write_catalogue(
        &catalogue,
        &[
            (
                "V1-0001_S1",
                "V1-0001",
                0.25,
                1.75,
                "earth turning, wide",
                4,
                1,
                "reel-01",
                Some(0.42),
            ),
            (
                "V1-0002_S1",
                "V1-0002",
                0.5,
                2.5,
                "speaker mid shot",
                3,
                0,
                "",
                Some(0.5),
            ),
            (
                "V1-0002_S2",
                "V1-0002",
                4.0,
                9.0,
                "runs past the end of the indexed source",
                2,
                0,
                "",
                Some(0.5),
            ),
            (
                "V1-0003_S1",
                "V1-0003",
                1.0,
                2.0,
                "source is not on this machine",
                5,
                1,
                "reel-02",
                Some(0.5),
            ),
        ],
    );
    let originals = temp.path().join("originals");
    fs::create_dir_all(&originals).unwrap();
    fs::copy(&earth, originals.join("earth-timelapse-silent.mp4")).unwrap();
    fs::copy(&speech, originals.join("synthetic-speech.mp4")).unwrap();
    // Originals were copied: paths differ from the indexed rows, so matching must fall back to hash.
    let recipes_dir = temp.path().join("recipes");
    fs::create_dir_all(&recipes_dir).unwrap();
    let good = recipe_json(
        &recipes_dir,
        "healthy-earth.json",
        r#"{"reel": {"theme": "Healthy Earth", "vibe": "bright", "beat_snap": true, "format": "9:16",
             "music_volume": 100, "watermark": "br", "cover": {"id": "V1-0001_S1", "time": 0.4},
             "sequence": [
               {"id": "V1-0001_S1", "in": 0.25, "out": 1.25, "crop_x": 0.42,
                "crop_kf": [{"t": 0.25, "x": 0.42}, {"t": 1.0, "x": 0.61}],
                "caption": "A short warm opening line", "transition": "mix", "motion": "in",
                "grade": {"b": 103, "c": 104, "s": 106, "t": 26}},
               {"id": "V1-0002_S1", "in": 0.0, "out": 1.5}
             ],
             "crops": {"V1-0001_S1": 0.42, "V1-0002_S1": 0.5}}}"#,
    );
    let unsupported = recipe_json(
        &recipes_dir,
        "legacy.json",
        r#"{"reel": {"sequence": [{"id": "V1-0002_S1", "in": 0.0, "out": 1.0, "lut": "kodak.cube"}]}}"#,
    );
    let unknown_segment = recipe_json(
        &recipes_dir,
        "missing.json",
        r#"{"reel": {"sequence": [{"id": "V1-0003_S1", "in": 0.0, "out": 1.0}]}}"#,
    );

    let mut options = ImportOptions::dry_run(&catalogue);
    options.originals = vec![originals.clone()];
    options.recipes = vec![good.clone(), unsupported.clone(), unknown_segment.clone()];
    options.match_by_hash = true;

    // ---- dry run ----
    let report = import_reel_studio(&mut store, &options).unwrap();
    assert_eq!(report.mode, "dry_run");
    let by_clip = |id: &str| {
        report
            .sources
            .iter()
            .find(|s| s.clip_id == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_clip("V1-0001").matched_by, "sha256");
    assert_eq!(by_clip("V1-0001").video_id.as_deref(), Some("video-earth"));
    assert_eq!(by_clip("V1-0003").matched_by, "missing_file");
    let seg = |id: &str| report.segments.iter().find(|s| s.segment_id == id).cloned();
    assert_eq!(seg("V1-0001_S1").unwrap().outcome, "new");
    assert_eq!(seg("V1-0001_S1").unwrap().boundary_basis, "catalogue_tc");
    assert_eq!(seg("V1-0003_S1").unwrap().outcome, "skipped");
    assert!(
        seg("V1-0002_S2").is_none(),
        "out-of-range segment is an issue, not a span"
    );
    let kinds: Vec<&str> = report.issues.iter().map(|i| i.kind.as_str()).collect();
    assert!(kinds.contains(&"missing_source"));
    assert!(kinds.contains(&"out_of_range"));
    assert!(
        kinds.iter().filter(|k| **k == "unsupported").count() >= 2,
        "{kinds:?}"
    );
    assert!(report
        .issues
        .iter()
        .any(|i| i.detail.contains("lut") && i.detail.contains("refusing to discard")));
    assert!(report
        .issues
        .iter()
        .any(|i| i.detail.contains("unknown_segment")));
    let good_recipe = report
        .recipes
        .iter()
        .find(|r| r.file.ends_with("healthy-earth.json"))
        .unwrap();
    assert_eq!(good_recipe.outcome, "new");
    assert_eq!(good_recipe.items, 2);
    assert!(good_recipe.finished_project, "V1-0001_S1 carries used_in");
    assert_eq!(
        report.reference_set_candidates,
        vec!["Reel Studio · Healthy Earth".to_owned()]
    );
    assert_eq!(report.planned_writes.manual_spans_insert, 2);
    assert_eq!(report.planned_writes.render_recipes_insert, 1);
    assert_eq!(report.planned_writes.plan_items_insert, 2);
    assert_eq!(report.planned_writes.feedback_events_insert, 0);
    assert_eq!(report.planned_writes.reference_sets_insert, 0);
    // Nothing was written.
    assert!(store.manual_spans(DEFAULT_OWNER_ID).unwrap().is_empty());
    assert!(store.plan_list(DEFAULT_OWNER_ID).unwrap().is_empty());
    assert_eq!(store.catalogue_imports(DEFAULT_OWNER_ID).unwrap().len(), 1);
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());

    // ---- apply ----
    options.apply = true;
    let applied = import_reel_studio(&mut store, &options).unwrap();
    assert_eq!(applied.mode, "apply");
    let spans = store.manual_spans(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(spans.len(), 2);
    let earth_span = spans
        .iter()
        .find(|s| s.external_id == "V1-0001_S1")
        .unwrap();
    assert_eq!(earth_span.video_id, "video-earth");
    assert_eq!((earth_span.start_s, earth_span.end_s), (0.25, 1.75));
    assert_eq!(earth_span.boundary_basis, SpanBoundaryBasis::CatalogueTc);
    assert!(earth_span.boundary_tolerance_s > 0.0);
    assert_eq!(earth_span.quality, Some(4));
    assert!(earth_span.standout);
    assert_eq!(earth_span.used_in, "reel-01");
    assert_eq!(earth_span.crop_x, Some(0.42));

    let plans = store.plan_list(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].name, "Reel Studio · Healthy Earth");
    let items = store.plan_items(DEFAULT_OWNER_ID, &plans[0].id).unwrap();
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|item| item.origin == PlanOrigin::Historical));
    assert!(items.iter().all(|item| item.media_kind == MediaKind::Span));
    assert!(items.iter().all(|item| item.profile_version.is_none()));
    // Segment-relative in/out became original-source seconds inside the span.
    assert_eq!(items[0].media_id, earth_span.id);
    assert_eq!((items[0].start_s, items[0].end_s), (Some(0.5), Some(1.5)));
    assert!(items[0]
        .provenance_json
        .contains("\"external_id\":\"V1-0001_S1\""));
    let recipes = store
        .render_recipes(DEFAULT_OWNER_ID, Some(RenderRecipeKind::Reel))
        .unwrap();
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].id, "reel-studio-healthy-earth");
    assert!(recipes[0].schema_json.contains("\"origin\":\"historical\""));
    assert!(
        store
            .plan_revisions(DEFAULT_OWNER_ID, &plans[0].id)
            .unwrap()
            .len()
            == 1
    );
    assert!(
        store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty(),
        "discovery never trains"
    );

    // ---- re-apply: idempotent ----
    let again = import_reel_studio(&mut store, &options).unwrap();
    assert!(again
        .segments
        .iter()
        .filter(|s| s.video_id.is_some())
        .all(|s| s.outcome == "unchanged"));
    assert_eq!(again.planned_writes.manual_spans_insert, 0);
    assert_eq!(again.planned_writes.render_recipes_insert, 0);
    let good_again = again
        .recipes
        .iter()
        .find(|r| r.file.ends_with("healthy-earth.json"))
        .unwrap();
    assert_eq!(good_again.outcome, "unchanged");
    assert_eq!(store.manual_spans(DEFAULT_OWNER_ID).unwrap().len(), 2);
    assert_eq!(store.plan_list(DEFAULT_OWNER_ID).unwrap().len(), 1);
    assert_eq!(
        store
            .render_recipes(DEFAULT_OWNER_ID, Some(RenderRecipeKind::Reel))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.catalogue_imports(DEFAULT_OWNER_ID).unwrap().len(), 3);

    // ---- catalogue edit re-imports evidence without touching the user's plan ----
    Connection::open(&catalogue)
        .unwrap()
        .execute(
            "UPDATE segments SET quality = 5 WHERE segment_id = 'V1-0001_S1'",
            [],
        )
        .unwrap();
    let updated = import_reel_studio(&mut store, &options).unwrap();
    assert_eq!(
        updated
            .segments
            .iter()
            .find(|s| s.segment_id == "V1-0001_S1")
            .unwrap()
            .outcome,
        "updated"
    );
    let refreshed = store
        .manual_span_by_external_id(DEFAULT_OWNER_ID, "reel_studio", "V1-0001_S1")
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.id, earth_span.id,
        "span id is stable across re-imports"
    );
    assert_eq!(refreshed.quality, Some(5));
    assert_eq!(
        store
            .plan_items(DEFAULT_OWNER_ID, &plans[0].id)
            .unwrap()
            .len(),
        2
    );

    // The spans survive a shot rebuild (resplit) and the catalogue file was never modified by us.
    store
        .replace_shots(
            DEFAULT_OWNER_ID,
            "video-earth",
            &[Shot {
                id: "shot-earth-rebuilt".to_owned(),
                video_id: "video-earth".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: 0,
                start_s: 0.0,
                end_s: 2.0,
                rep_frame_s: 1.0,
                thumb_rel: None,
                scene_score: None,
            }],
        )
        .unwrap();
    assert_eq!(
        store
            .manual_spans_for_video(DEFAULT_OWNER_ID, "video-earth")
            .unwrap()
            .len(),
        1
    );
}

/// An imported historical project renders through Task 021's durable ordered-reel path using
/// span sources. Uses the bundled FFmpeg, so it runs on macOS only.
#[cfg(target_os = "macos")]
#[test]
fn imported_span_project_renders_through_the_reel_executor() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let mut store = Store::open(&data_dir).unwrap();
    let speech = fixture("synthetic-speech.mp4");
    let source_hash = sha256_file(&speech).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &indexed_video("video-speech", &speech, Some(5.0)),
        )
        .unwrap();
    let catalogue = temp.path().join("clips.db");
    write_catalogue(
        &catalogue,
        &[
            (
                "V1-0002_S1",
                "V1-0002",
                0.0,
                2.0,
                "opening",
                4,
                1,
                "reel-01",
                None,
            ),
            (
                "V1-0002_S2",
                "V1-0002",
                3.0,
                5.0,
                "closing",
                4,
                0,
                "reel-01",
                None,
            ),
        ],
    );
    let recipes_dir = temp.path().join("recipes");
    fs::create_dir_all(&recipes_dir).unwrap();
    let recipe = recipe_json(
        &recipes_dir,
        "two-cuts.json",
        r#"{"reel": {"theme": "Two cuts", "sequence": [
            {"id": "V1-0002_S1", "in": 0.25, "out": 1.25},
            {"id": "V1-0002_S2", "in": 0.25, "out": 1.25}
        ]}}"#,
    );
    let mut options = ImportOptions::dry_run(&catalogue);
    options.originals = vec![speech.parent().unwrap().to_path_buf()];
    options.recipes = vec![recipe];
    options.apply = true;
    let report = import_reel_studio(&mut store, &options).unwrap();
    // The shared catalogue helper also lists an unindexed and a missing source; the speech clip
    // and its recipe themselves import cleanly.
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.subject == "V1-0001" || issue.subject == "V1-0003"),
        "{:?}",
        report.issues
    );
    let plan = store.plan_list(DEFAULT_OWNER_ID).unwrap().remove(0);
    let items = store.plan_items(DEFAULT_OWNER_ID, &plan.id).unwrap();
    assert_eq!(items.len(), 2);
    let revision = store.plan_revisions(DEFAULT_OWNER_ID, &plan.id).unwrap()[0].revision;

    // Queue an ordered reel (v1 executor contract) over the imported span items.
    let now = chrono::Utc::now();
    store
        .render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: "imported-reel-mp4".to_owned(),
                version: 1,
                kind: RenderRecipeKind::Reel,
                name: "Imported reel MP4".to_owned(),
                schema_json: serde_json::json!({
                    "schema_version": 1, "kind": "reel",
                    "transition": {"kind": "cut"}, "audio": {"mode": "source"},
                    "output": {"preset": "mp4-h264-sdr-v1"}
                })
                .to_string(),
                created_at: now,
            },
        )
        .unwrap();
    let destination = temp.path().join("imported-reel.mp4");
    store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: "render-imported-reel".to_owned(),
                recipe_id: "imported-reel-mp4".to_owned(),
                recipe_version: 1,
                plan_id: Some(plan.id.clone()),
                plan_revision: Some(revision),
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": plan.context_key,
                    "selection_provenance": {"origin": "historical"},
                    "sources": items.iter().map(|item| serde_json::json!({
                        "media_kind": "span",
                        "media_id": item.media_id,
                        "source_id": "video-speech",
                        "sha256": source_hash,
                        "path": speech.to_string_lossy(),
                    })).collect::<Vec<_>>()
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {"clip": "not_used", "aesthetic": "not_used", "personal_style": "not_used"}
                })
                .to_string(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: now,
            },
        )
        .unwrap();
    drop(store);
    let config = Config {
        data_dir: Some(data_dir.clone()),
        ..Config::default()
    };
    let paths = AppPaths::resolve(config.data_dir.as_ref()).unwrap();
    let output = Pipeline::new(config, paths, CancellationToken::default())
        .execute_render_job(DEFAULT_OWNER_ID, "render-imported-reel")
        .unwrap();
    assert!(Path::new(&output.output_path).is_file());
    assert!(Path::new(&output.manifest_path).is_file());
    let duration = output.duration_s.expect("reel duration");
    assert!(
        (duration - 2.0).abs() < 0.2,
        "two one-second cuts, got {duration}"
    );
    assert_eq!(
        sha256_file(&speech).unwrap(),
        source_hash,
        "source untouched"
    );
}
