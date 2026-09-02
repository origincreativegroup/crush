use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::job::{JobStatus, Stage};
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_pipeline::{sha256_file, Pipeline};
use crush_search::SearchEngine;
use crush_stage_embed::embedder::{Embedder, ProviderPreference};
use crush_stage_split::ffmpeg;
use crush_store::{
    EditorialAnnotation, FeedbackEvent, FeedbackSignal, JobFilter, MediaKind, NewJob,
    PhotoProxyProvenance, PhotoStatus, Plan, PlanItem, PlanOrigin, Store, VideoStatus,
};
use image::{Rgb, RgbImage};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn source_models() -> PathBuf {
    std::env::var_os("CRUSH_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("models"))
}

fn install_model_links(data_dir: &Path) -> bool {
    let source = source_models();
    let required = [
        "clip-image.onnx",
        "clip-text.onnx",
        "bpe_simple_vocab_16e6.txt.gz",
        "ggml-small.bin",
    ];
    if !required.iter().all(|name| source.join(name).is_file()) {
        eprintln!("skipping pipeline fixture test: complete models-v1 is not installed");
        return false;
    }
    let destination = data_dir.join("models");
    std::fs::create_dir_all(&destination).unwrap();
    for name in required {
        let source = source.join(name);
        let target = destination.join(name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(source, target).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(source, target).unwrap();
    }
    true
}

fn fixture_config(data_dir: &Path) -> Config {
    let mut config = Config {
        data_dir: Some(data_dir.to_path_buf()),
        ..Config::default()
    };
    config.embed.provider = "cpu".to_owned();
    config.asr.model = "small".to_owned();
    config.asr.language = Some("en".to_owned());
    config
}

#[test]
fn photo_ingest_is_idempotent_and_searchable() {
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("photos");
    std::fs::create_dir(&input).unwrap();
    let photo_path = input.join("warm-geometric-portrait.jpg");
    let image = RgbImage::from_fn(640, 426, |x, y| {
        if x > 380 && (90..350).contains(&y) {
            Rgb([230, 166, 72])
        } else {
            Rgb([34, 48, 78])
        }
    });
    image.save(&photo_path).unwrap();
    let original_sha256 = sha256_file(&photo_path).unwrap();
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let config = fixture_config(temp.path());
    let pipeline = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default());

    let first = pipeline.ingest(&input, false).unwrap();
    assert_eq!(first.discovered, 1);
    assert_eq!(first.discovered_photos, 1);
    assert_eq!(first.indexed, 1);
    assert_eq!(first.indexed_photos, 1);
    assert_eq!(first.failed, 0);
    assert_eq!(first.search_vectors, 1);

    let store = Store::open(temp.path()).unwrap();
    let photo = store.photos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
    assert_eq!(photo.status, PhotoStatus::Done);
    assert_eq!(photo.sha256, original_sha256);
    assert_eq!(sha256_file(&photo_path).unwrap(), original_sha256);
    assert!(store
        .thumbnail_path(photo.thumb_rel.as_deref().unwrap())
        .unwrap()
        .is_file());
    let source_metadata = store
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)
        .unwrap()
        .unwrap();
    assert_eq!(source_metadata.decoder, "image-rs");
    assert_eq!(
        source_metadata.proxy_provenance,
        PhotoProxyProvenance::DecodedOriginal
    );
    let proxy_path = store
        .proxy_path(source_metadata.proxy_rel.as_deref().unwrap())
        .unwrap();
    assert!(proxy_path.is_file());
    assert_eq!(
        sha256_file(&proxy_path).unwrap(),
        source_metadata.proxy_sha256.as_deref().unwrap()
    );
    assert_eq!(
        store
            .vector_for_photo(DEFAULT_OWNER_ID, &photo.id)
            .unwrap()
            .unwrap()
            .len(),
        512
    );
    assert!(store
        .active_style_profile(DEFAULT_OWNER_ID)
        .unwrap()
        .is_none());
    let assessment = store
        .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Photo, &photo.id)
        .unwrap()
        .expect("cold-start photo assessment");
    assert_eq!(assessment.model_version, "strong-shot-v1");
    assert!((0.0..=1.0).contains(&assessment.technical_quality));
    assert!((0.0..=1.0).contains(&assessment.composition_quality));
    let first_assessed_at = assessment.assessed_at;
    let evidence: serde_json::Value = serde_json::from_str(&assessment.explanation_json).unwrap();
    assert_eq!(evidence["independent_of_profile"], true);
    assert_eq!(evidence["identity_used"], false);
    // Fidelity metadata now records the thumbnail hash and the derivative recipes.
    let recorded_metadata: serde_json::Value =
        serde_json::from_str(&source_metadata.metadata_json).unwrap();
    assert!(recorded_metadata["thumbnail_sha256"].is_string());
    assert_eq!(
        recorded_metadata["proxy_recipe"]["proxy"]["max_dimension_px"],
        2560
    );
    assert_eq!(
        recorded_metadata["proxy_recipe"]["thumbnail"]["max_dimension_px"],
        960
    );
    let engine =
        SearchEngine::load(&store, DEFAULT_OWNER_ID, config.search.transcript_hit_boost).unwrap();
    let mut embedder = Embedder::new(paths.models(), ProviderPreference::Cpu, 2).unwrap();
    let results = engine
        .search_assets(
            &store,
            &mut |text: &str| embedder.embed_text(text),
            "warm geometric portrait",
            5,
            crush_search::SearchKind::All,
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].asset_type, "photo");
    assert_eq!(results[0].asset_id, photo.id);
    drop(store);

    let second = pipeline.ingest(&input, false).unwrap();
    assert_eq!(second.indexed, 0);
    assert_eq!(second.skipped, 1);
    assert_eq!(sha256_file(&photo_path).unwrap(), original_sha256);

    let store = Store::open(temp.path()).unwrap();
    let photo_id = store.photos(DEFAULT_OWNER_ID).unwrap().pop().unwrap().id;
    let assessment = store
        .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Photo, &photo_id)
        .unwrap()
        .expect("assessment survives a no-change re-ingest");
    assert_eq!(
        assessment.assessed_at, first_assessed_at,
        "second ingest of an unchanged library must perform zero re-analysis work"
    );
    drop(store);
    rusqlite::Connection::open(temp.path().join("library.db"))
        .unwrap()
        .execute(
            "DELETE FROM photo_source_metadata WHERE photo_id = ?1",
            [&photo_id],
        )
        .unwrap();
    let backfilled = pipeline.ingest(&input, false).unwrap();
    assert_eq!(
        backfilled.indexed, 1,
        "v2 photo rows must gain v3 fidelity metadata"
    );
    assert!(Store::open(temp.path())
        .unwrap()
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo_id)
        .unwrap()
        .is_some());
}

/// The full rename-survival posture (TASK-038): the same bytes at a new path keep the same
/// identity and every piece of shot-keyed evidence, whether the file returns via ingest
/// (reported honestly as moved/renamed) or through the explicit verified relink flow.
#[test]
fn renamed_and_moved_files_keep_identity_and_evidence() {
    if ffmpeg::resolve().is_err() {
        eprintln!("skipping pipeline fixture test: FFmpeg sidecars are not installed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let config = fixture_config(temp.path());
    let pipeline = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default());

    let media = temp.path().join("media");
    std::fs::create_dir(&media).unwrap();
    let original = media.join("launch-day.mp4");
    std::fs::copy(
        repo_root().join("fixtures/clips/earth-timelapse-silent.mp4"),
        &original,
    )
    .unwrap();

    let first = pipeline.ingest(&media, false).unwrap();
    assert_eq!(first.indexed, 1, "ingest errors: {:#?}", first.errors);
    assert_eq!(first.moved, 0);
    assert_eq!(first.renamed, 0);

    let store = Store::open(temp.path()).unwrap();
    let video = store.videos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
    assert_eq!(video.status, VideoStatus::Done);
    let shots = store.shots_for_video(DEFAULT_OWNER_ID, &video.id).unwrap();
    assert!(!shots.is_empty());
    let shot_ids = shots.iter().map(|shot| shot.id.clone()).collect::<Vec<_>>();
    let shot_id = shot_ids[0].clone();
    drop(store);

    // Shot-keyed evidence on a Done video, exactly as a user leaves it before a move.
    let mut store = Store::open(temp.path()).unwrap();
    store
        .append_feedback(
            DEFAULT_OWNER_ID,
            &FeedbackEvent {
                id: "ev-pick".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                media_kind: MediaKind::Shot,
                media_id: shot_id.clone(),
                signal: FeedbackSignal::Pick,
                value: None,
                compared_media_kind: None,
                compared_media_id: None,
                context_json: "{}".to_owned(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .upsert_editorial_annotation(
            DEFAULT_OWNER_ID,
            &EditorialAnnotation {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                media_kind: MediaKind::Shot,
                media_id: shot_id.clone(),
                description: "the decisive moment".to_owned(),
                subjects: String::new(),
                action: String::new(),
                tags: "hero".to_owned(),
                quality: Some(5),
                standout: true,
                usable: true,
                faces_visible: true,
                nametags_visible: false,
                blur_required: false,
                crop_x: None,
                grade_json: None,
                notes: String::new(),
                updated_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .plan_create(
            DEFAULT_OWNER_ID,
            &Plan {
                id: "plan-ev".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name: "Rename survival".to_owned(),
                description: String::new(),
                context_key: "default".to_owned(),
                brief: String::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &PlanItem {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                plan_id: "plan-ev".to_owned(),
                media_kind: MediaKind::Shot,
                media_id: shot_id.clone(),
                position: 0,
                start_s: Some(shots[0].start_s),
                end_s: Some(shots[0].end_s),
                pacing: None,
                crop_x: None,
                grade_json: None,
                reason: String::new(),
                signals_json: "{}".to_owned(),
                origin: PlanOrigin::General,
                rank: None,
                profile_version: None,
                provenance_json: "{}".to_owned(),
                added_at: Utc::now(),
            },
        )
        .unwrap();
    assert!(store
        .vector_for_shot(DEFAULT_OWNER_ID, &shot_id)
        .unwrap()
        .is_some());
    assert_eq!(store.videos(DEFAULT_OWNER_ID).unwrap().len(), 1);
    drop(store);

    let evidence_intact = |context: &str| {
        let store = Store::open(temp.path()).unwrap();
        assert_eq!(
            store.videos(DEFAULT_OWNER_ID).unwrap().len(),
            1,
            "{context}: a relink never duplicates rows"
        );
        let video = store.videos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
        let ids = store
            .shots_for_video(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .iter()
            .map(|shot| shot.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, shot_ids, "{context}: stable shot ids must not change");
        assert_eq!(
            store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(),
            1,
            "{context}: feedback survives"
        );
        assert!(
            store
                .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Shot, &shot_id)
                .unwrap()
                .is_some(),
            "{context}: annotation survives"
        );
        assert!(
            store
                .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &shot_id)
                .unwrap()
                .is_some(),
            "{context}: assessment survives"
        );
        assert!(
            store
                .vector_for_shot(DEFAULT_OWNER_ID, &shot_id)
                .unwrap()
                .is_some(),
            "{context}: vector survives"
        );
        assert_eq!(
            store.plan_items(DEFAULT_OWNER_ID, "plan-ev").unwrap().len(),
            1,
            "{context}: plan item survives"
        );
    };

    // 1) Rename in place: same directory, new file name → reported as renamed.
    let renamed = media.canonicalize().unwrap().join("launch-day-renamed.mp4");
    std::fs::rename(&original, &renamed).unwrap();
    let summary = pipeline.ingest(&media, false).unwrap();
    assert_eq!(summary.renamed, 1, "a same-directory rename is a rename");
    assert_eq!(summary.moved, 0);
    assert_eq!(
        summary.indexed, 0,
        "a moved file is relinked, not re-indexed"
    );
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.relinked.len(), 1);
    assert_eq!(summary.relinked[0].media_kind, "video");
    assert_eq!(summary.relinked[0].id, video.id);
    assert_eq!(
        summary.relinked[0].kind,
        crush_pipeline::RelinkKind::Renamed
    );
    assert_eq!(summary.relinked[0].to_path, renamed);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .video_by_id(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap()
            .path,
        renamed.to_string_lossy()
    );
    drop(store);
    evidence_intact("after a rename");

    // 2) Move to a different directory: reported as moved.
    let moved_dir = temp.path().join("remounted-drive");
    std::fs::create_dir(&moved_dir).unwrap();
    let moved = moved_dir
        .canonicalize()
        .unwrap()
        .join("launch-day-renamed.mp4");
    std::fs::rename(&renamed, &moved).unwrap();
    let summary = pipeline.ingest(&moved_dir, false).unwrap();
    assert_eq!(summary.moved, 1, "a new directory is a move");
    assert_eq!(summary.renamed, 0);
    assert_eq!(summary.relinked[0].kind, crush_pipeline::RelinkKind::Moved);
    assert_eq!(summary.relinked[0].from_path, renamed);
    assert_eq!(summary.relinked[0].to_path, moved);
    evidence_intact("after a move");

    // 2b) Duplicate copy: the same bytes appear at a second path while the old file is
    //     still on disk. Nothing moved or renamed — ingest re-points the catalog at the
    //     new copy and says so honestly instead of claiming a move.
    let copied = media.canonicalize().unwrap().join("launch-day-copy.mp4");
    std::fs::copy(&moved, &copied).unwrap();
    let summary = pipeline.ingest(&media, false).unwrap();
    assert_eq!(summary.moved, 0, "the old file still exists: not a move");
    assert_eq!(summary.renamed, 0);
    assert_eq!(
        summary.duplicated, 1,
        "an old copy left on disk is reported as a duplicate copy"
    );
    assert_eq!(
        summary.indexed, 0,
        "a duplicate copy is relinked, not re-indexed"
    );
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.relinked.len(), 1);
    assert_eq!(
        summary.relinked[0].kind,
        crush_pipeline::RelinkKind::DuplicateCopy
    );
    assert_eq!(summary.relinked[0].from_path, moved);
    assert_eq!(summary.relinked[0].to_path, copied);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .video_by_id(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap()
            .path,
        copied.to_string_lossy(),
        "the path update semantics are unchanged: the row points at the ingested copy"
    );
    drop(store);
    evidence_intact("after a duplicate copy");
    // Clean the copy up so the later phases see a single file on disk again.
    std::fs::remove_file(&copied).unwrap();
    // The row still points at the removed copy; restore it to the surviving file for the
    // explicit-relink phase below.
    let restored = pipeline.relink(&video.id, &moved).unwrap();
    assert_eq!(restored.new_path, moved.to_string_lossy());

    // 3) Explicit relink without re-adding any folder: hash verified, row re-pointed.
    let relocated_dir = temp.path().join("relocated");
    std::fs::create_dir(&relocated_dir).unwrap();
    let relocated = relocated_dir
        .canonicalize()
        .unwrap()
        .join("launch-day-renamed.mp4");
    std::fs::rename(&moved, &relocated).unwrap();
    let outcome = pipeline.relink(&video.id, &relocated).unwrap();
    assert_eq!(outcome.id, video.id);
    assert_eq!(outcome.media_kind, "video");
    assert_eq!(outcome.old_path, moved.to_string_lossy());
    assert_eq!(outcome.new_path, relocated.to_string_lossy());
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .video_by_id(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap()
            .path,
        relocated.to_string_lossy()
    );
    drop(store);
    evidence_intact("after an explicit relink");

    // 4) A different file at the new path is refused honestly and changes nothing.
    let tampered = relocated_dir.join("tampered.mp4");
    std::fs::copy(&relocated, &tampered).unwrap();
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(&tampered)
        .unwrap();
    std::io::Write::write_all(&mut handle, b"not-the-same-bytes").unwrap();
    drop(handle);
    let error = pipeline.relink(&video.id, &tampered).unwrap_err();
    assert!(
        format!("{error:#}").contains("SHA-256 mismatch"),
        "a different file must be refused with the honest reason: {error:#}"
    );
    assert!(
        pipeline.relink("video-missing", &relocated).is_err(),
        "an unknown target refuses too"
    );
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .video_by_id(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap()
            .path,
        relocated.to_string_lossy(),
        "a refused relink leaves the row untouched"
    );
    drop(store);
    evidence_intact("after a refused relink");
    // The original bytes were never modified anywhere along the way.
    assert_eq!(sha256_file(&relocated).unwrap(), video.sha256);
}

#[test]
fn fixture_ingest_is_idempotent_resumable_and_exports_a_clip() {
    if ffmpeg::resolve().is_err() {
        eprintln!("skipping pipeline fixture test: FFmpeg sidecars are not installed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let config = fixture_config(temp.path());
    let pipeline = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default());
    let clips = repo_root().join("fixtures/clips");

    let first = pipeline.ingest(&clips, true).unwrap();
    assert_eq!(first.discovered, 4);
    assert_eq!(first.indexed, 4, "ingest errors: {:#?}", first.errors);
    assert_eq!(first.failed, 0);
    assert!(first.search_vectors > 0);

    let store = Store::open(temp.path()).unwrap();
    let videos = store.videos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(videos.len(), 4);
    assert!(videos.iter().all(|video| video.status == VideoStatus::Done));
    for video in &videos {
        assert_eq!(sha256_file(Path::new(&video.path)).unwrap(), video.sha256);
        let metadata = store
            .video_source_metadata(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap();
        assert!(!metadata.container.is_empty());
        assert!(!metadata.video_codec.is_empty());
        if metadata.proxy_required {
            let proxy = store
                .proxy_path(metadata.proxy_rel.as_deref().unwrap())
                .unwrap();
            assert!(proxy.is_file());
            assert_eq!(
                sha256_file(&proxy).unwrap(),
                metadata.proxy_sha256.as_deref().unwrap()
            );
        }
        for shot in store.shots_for_video(DEFAULT_OWNER_ID, &video.id).unwrap() {
            let assessment = store
                .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &shot.id)
                .unwrap()
                .expect("video shot assessment");
            assert_eq!(assessment.model_version, "strong-shot-v1");
            assert!((0.0..=1.0).contains(&assessment.motion_stability));
            assert!((0.0..=1.0).contains(&assessment.pacing));
            assert!(
                serde_json::from_str::<serde_json::Value>(&assessment.explanation_json).unwrap()
                    ["groups"]["moment_sequence"]
                    .is_object()
            );
        }
    }
    let jobs = store.jobs(DEFAULT_OWNER_ID, &JobFilter::default()).unwrap();
    assert_eq!(jobs.len(), 16);
    assert!(jobs.iter().all(|job| job.status == JobStatus::Done));
    for job in &jobs {
        let directory = Path::new(job.debug_dir.as_deref().expect("debug directory recorded"));
        assert!(directory.is_dir());
        match job.stage {
            Stage::Split => {
                assert!(directory.join("frames").is_dir());
                assert!(directory.join("scores.csv").is_file());
                assert!(directory.join("commands.txt").is_file());
            }
            Stage::Embed => assert!(directory.join("vectors.json").is_file()),
            Stage::Analyze => assert!(directory.join("aesthetic-frames").is_dir()),
            Stage::Transcribe => {
                let video = store
                    .video_by_id(
                        DEFAULT_OWNER_ID,
                        job.video_id.as_deref().expect("video job has a video id"),
                    )
                    .unwrap()
                    .unwrap();
                if video.has_audio {
                    assert!(directory.join("audio.wav").is_file());
                    assert!(directory.join("commands.txt").is_file());
                }
            }
            Stage::PhotoIngest => {
                assert!(job.photo_id.is_some(), "photo jobs carry a photo id");
            }
        }
    }
    drop(store);

    let second = pipeline.ingest(&clips, false).unwrap();
    assert_eq!(second.skipped, 4);
    assert_eq!(second.indexed, 0);

    let store = Store::open(temp.path()).unwrap();
    let target = store
        .videos(DEFAULT_OWNER_ID)
        .unwrap()
        .into_iter()
        .find(|video| video.path.ends_with("synthetic-speech.mp4"))
        .unwrap();
    let before_ids = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .map(|shot| shot.id)
        .collect::<Vec<_>>();
    drop(store);
    rusqlite::Connection::open(temp.path().join("library.db"))
        .unwrap()
        .execute(
            "DELETE FROM aesthetic_assessments WHERE media_kind = 'shot' AND media_id = ?1",
            [&before_ids[0]],
        )
        .unwrap();
    let backfilled = pipeline.ingest(Path::new(&target.path), false).unwrap();
    assert_eq!(backfilled.indexed, 1);
    assert_eq!(backfilled.skipped, 0);
    assert!(Store::open(temp.path())
        .unwrap()
        .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &before_ids[0])
        .unwrap()
        .is_some());
    pipeline.resplit(&target.id, false).unwrap();
    let store = Store::open(temp.path()).unwrap();
    let after_ids = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .map(|shot| shot.id)
        .collect::<Vec<_>>();
    assert_eq!(
        before_ids, after_ids,
        "resplit must preserve stable shot ids"
    );

    let shot = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    drop(store);
    let exported = temp.path().join("exported-shot.mp4");
    let source_hash = sha256_file(Path::new(&target.path)).unwrap();
    assert!(pipeline
        .export_clip(&shot.id, Path::new(&target.path))
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    pipeline.export_clip(&shot.id, &exported).unwrap();
    let export_hash = sha256_file(&exported).unwrap();
    assert!(pipeline.export_clip(&shot.id, &exported).is_err());
    assert_eq!(sha256_file(&exported).unwrap(), export_hash);
    assert_eq!(sha256_file(Path::new(&target.path)).unwrap(), source_hash);
    let probe = ffmpeg::Runner::new(ffmpeg::resolve().unwrap(), 0, "verify-clip")
        .probe(&exported)
        .unwrap()
        .value;
    assert!(probe.duration_s > 0.0);
    assert!((probe.duration_s - (shot.end_s - shot.start_s)).abs() < 0.25);

    let store = Store::open(temp.path()).unwrap();
    store
        .set_video_status(DEFAULT_OWNER_ID, &target.id, VideoStatus::Split)
        .unwrap();
    let shots = store.shots_for_video(DEFAULT_OWNER_ID, &target.id).unwrap();
    let preserved = store
        .vector_for_shot(DEFAULT_OWNER_ID, &shots[0].id)
        .unwrap()
        .unwrap();
    store
        .delete_vectors_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap();
    store
        .put_vector(DEFAULT_OWNER_ID, &shots[0].id, &preserved)
        .unwrap();
    store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "killed-mid-embed".to_owned(),
                video_id: Some(target.id.clone()),
                photo_id: None,
                stage: Stage::Embed,
                started_at: Utc::now(),
                debug_dir: None,
            },
        )
        .unwrap();
    drop(store);

    let resumed = pipeline.ingest(Path::new(&target.path), false).unwrap();
    assert_eq!(resumed.recovered_jobs, 1);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .jobs(
                DEFAULT_OWNER_ID,
                &JobFilter {
                    status: Some(JobStatus::Failed),
                    ..JobFilter::default()
                },
            )
            .unwrap()
            .into_iter()
            .find(|job| job.id == "killed-mid-embed")
            .unwrap()
            .error
            .as_deref(),
        Some("interrupted")
    );
    assert!(store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .iter()
        .all(|shot| store
            .vector_for_shot(DEFAULT_OWNER_ID, &shot.id)
            .unwrap()
            .is_some()));
}

#[test]
fn bad_video_records_a_failed_job_and_does_not_block_other_files() {
    if ffmpeg::resolve().is_err() {
        eprintln!("skipping pipeline fixture test: FFmpeg sidecars are not installed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("00-bad.mp4"), b"not a video").unwrap();
    std::fs::copy(
        repo_root().join("fixtures/clips/earth-timelapse-silent.mp4"),
        input.join("01-valid.mp4"),
    )
    .unwrap();
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        paths,
        CancellationToken::default(),
    );
    let summary = pipeline.ingest(&input, false).unwrap();
    assert_eq!(summary.discovered, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.indexed, 1);
    let store = Store::open(temp.path()).unwrap();
    let failed = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                status: Some(JobStatus::Failed),
                ..JobFilter::default()
            },
        )
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]
        .error
        .as_deref()
        .is_some_and(|error| !error.is_empty()));
    assert_eq!(
        store
            .videos(DEFAULT_OWNER_ID)
            .unwrap()
            .into_iter()
            .find(|video| video.path.ends_with("01-valid.mp4"))
            .unwrap()
            .status,
        VideoStatus::Done
    );
}

#[test]
#[cfg(target_os = "macos")]
fn packaged_pipeline_indexes_representative_professional_sources() {
    let resolved = match ffmpeg::resolve() {
        Ok(resolved) => resolved,
        Err(_) => {
            eprintln!("skipping Task 016 packaged-pipeline test: sidecars are not installed");
            return;
        }
    };
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("professional-sources");
    std::fs::create_dir(&input).unwrap();
    let still = RgbImage::from_fn(96, 64, |x, y| Rgb([(x * 2) as u8, (y * 3) as u8, 112]));
    let png = input.join("still.png");
    still.save(&png).unwrap();
    still
        .save_with_format(input.join("still.jpg"), image::ImageFormat::Jpeg)
        .unwrap();
    still
        .save_with_format(input.join("still.tiff"), image::ImageFormat::Tiff)
        .unwrap();
    let heic_status = Command::new("/usr/bin/sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(input.join("still.heic"))
        .status()
        .unwrap();
    assert!(heic_status.success());

    let source = repo_root().join("fixtures/clips/synthetic-speech.mp4");
    let video_cases = [
        (
            "prores.mov",
            vec!["-an", "-c:v", "prores", "-profile:v", "2"],
        ),
        ("h264.m4v", vec!["-an", "-c:v", "h264_videotoolbox"]),
        (
            "dnx.mxf",
            vec![
                "-an",
                "-vf",
                "scale=1280:720",
                "-c:v",
                "dnxhd",
                "-b:v",
                "90M",
            ],
        ),
        (
            "hevc.mov",
            vec!["-an", "-c:v", "hevc_videotoolbox", "-tag:v", "hvc1"],
        ),
    ];
    for (name, arguments) in &video_cases {
        let status = Command::new(&resolved.path)
            .args(["-y", "-v", "error", "-i"])
            .arg(&source)
            .args(["-t", "1"])
            .args(arguments)
            .arg(input.join(name))
            .status()
            .unwrap();
        assert!(status.success(), "could not generate {name}");
    }

    let started = Instant::now();
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let summary = pipeline.ingest(&input, true).unwrap();
    let elapsed_ms = started.elapsed().as_millis();
    assert_eq!(summary.discovered, 8);
    assert_eq!(summary.discovered_photos, 4);
    assert_eq!(summary.indexed, 8);
    assert_eq!(summary.failed, 0, "{:?}", summary.errors);

    let store = Store::open(temp.path()).unwrap();
    let photos = store.photos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(photos.len(), 4);
    assert!(photos.iter().all(|photo| store
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)
        .unwrap()
        .is_some()));
    let videos = store.videos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(videos.len(), 4);
    let mut codec_policies = Vec::new();
    for video in videos {
        let metadata = store
            .video_source_metadata(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap();
        codec_policies.push(serde_json::json!({
            "source": Path::new(&video.path).file_name().unwrap().to_string_lossy(),
            "container": metadata.container,
            "codec": metadata.video_codec,
            "bit_depth": metadata.bit_depth,
            "color_space": metadata.color_space,
            "proxy_required": metadata.proxy_required,
            "proxy_reason": metadata.proxy_reason,
        }));
    }
    let report = serde_json::json!({
        "fixture_set": "Task 016 representative packaged pipeline",
        "elapsed_ms": elapsed_ms,
        "peak_resident_bytes": peak_resident_bytes(),
        "failures": summary.errors,
        "orientation_check": "EXIF orientation normalization covered by source_fidelity fixture",
        "color_check": "source color fields persisted and derivatives visually decoded",
        "photos": photos.len(),
        "videos": codec_policies,
    });
    std::fs::write(
        temp.path().join("task-016-packaged-pipeline-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
}

#[test]
fn pre_cancelled_pipeline_stops_without_creating_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        cancellation,
    );
    assert!(pipeline
        .ingest(&repo_root().join("fixtures/clips"), false)
        .is_err());
    let store = Store::open(temp.path()).unwrap();
    assert!(store
        .jobs(DEFAULT_OWNER_ID, &JobFilter::default())
        .unwrap()
        .is_empty());
}

#[cfg(target_os = "macos")]
fn peak_resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status == 0 {
        u64::try_from(unsafe { usage.assume_init() }.ru_maxrss).unwrap_or(0)
    } else {
        0
    }
}

#[test]
#[ignore = "10-minute local responsiveness smoke; run explicitly before release"]
fn ten_minute_silent_ingest_smoke() {
    let Ok(resolved) = ffmpeg::resolve() else {
        eprintln!("skipping 10-minute smoke: FFmpeg sidecars are not installed");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }

    let input = temp.path().join("ten-minute-silent.mp4");
    let status = Command::new(&resolved.path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-stream_loop",
            "59",
            "-i",
        ])
        .arg(repo_root().join("fixtures/clips/earth-timelapse-silent.mp4"))
        .args(["-t", "600", "-an", "-c:v", "copy"])
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());

    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let started = Instant::now();
    let summary = pipeline.ingest(&input, false).unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "10-minute ingest: {:.2}s wall time, {} vectors",
        elapsed.as_secs_f64(),
        summary.search_vectors
    );
    assert_eq!(summary.indexed, 1);
    assert_eq!(summary.failed, 0);

    let store = Store::open(temp.path()).unwrap();
    let video = store.videos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
    assert_eq!(video.status, VideoStatus::Done);
    assert!(video.duration_s.is_some_and(|duration| duration >= 599.0));
}

#[test]
fn known_unsupported_extensions_are_recorded_with_precise_reasons() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("shot.avif"), b"not really an avif").unwrap();
    std::fs::write(input.join("scan.erf"), b"not really an erf").unwrap();
    std::fs::write(input.join("notes.txt"), b"plain text").unwrap();
    std::fs::write(input.join(".DS_Store"), b"junk").unwrap();
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let summary = pipeline.ingest(&input, false).unwrap();
    assert_eq!(summary.discovered, 0);
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.failed, 0);
    assert_eq!(
        summary.errors.len(),
        2,
        "only known-unsupported media files are recorded: {:?}",
        summary.errors
    );
    for (path, reason) in &summary.errors {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap()
            .to_ascii_lowercase();
        assert!(extension == "avif" || extension == "erf");
        assert!(
            reason.contains("decode is disabled") && reason.contains("embedded-preview"),
            "reason must be precise: {reason}"
        );
    }
}

#[test]
fn unsupported_registry_never_flags_arbitrary_files() {
    use crush_pipeline::KNOWN_UNSUPPORTED_EXTENSIONS;
    use std::path::Path;

    let reason = |name: &str| {
        Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .and_then(|extension| {
                KNOWN_UNSUPPORTED_EXTENSIONS
                    .iter()
                    .find(|(known, _)| extension.eq_ignore_ascii_case(known))
                    .map(|(_, reason)| *reason)
            })
    };
    assert!(reason("a.avif").is_some());
    assert!(reason("a.jxl").is_some());
    assert!(reason("a.erf").is_some());
    assert!(reason("notes.txt").is_none());
    assert!(reason(".DS_Store").is_none());
    assert!(reason("no-extension").is_none());
}

#[test]
fn unsupported_registry_matches_the_checked_in_support_matrix() {
    use crush_pipeline::KNOWN_UNSUPPORTED_EXTENSIONS;

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/source-formats/support-matrix.json"
    ))
    .unwrap();
    let declared: Vec<&str> = fixture["known_unsupported"]
        .as_array()
        .expect("support matrix must declare known_unsupported entries")
        .iter()
        .filter_map(|entry| entry["extension"].as_str())
        .collect();
    for (extension, reason) in KNOWN_UNSUPPORTED_EXTENSIONS {
        assert!(
            declared.contains(extension),
            "support matrix is missing .{extension}"
        );
        assert!(!reason.trim().is_empty());
    }
    assert_eq!(
        declared.len(),
        KNOWN_UNSUPPORTED_EXTENSIONS.len(),
        "support matrix and code registry must agree exactly"
    );
}
