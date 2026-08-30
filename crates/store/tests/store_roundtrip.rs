use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{TimeZone, Utc};
use crush_core::{
    job::{JobStatus, Stage},
    DEFAULT_OWNER_ID,
};
use crush_store::{
    AestheticAssessment, AssetFilter, Collection, CollectionItem, EditorialAnnotation,
    EmbeddingMeta, FeedbackEvent, FeedbackSignal, JobFilter, MediaKind, NewJob, NewRenderJob,
    Photo, PhotoProxyProvenance, PhotoSourceMetadata, PhotoStatus, Plan, PlanItem, PlanItemPatch,
    PlanOrigin, ProblemKind, ReferenceItemRole, ReferenceSet, ReferenceSetItem, ReferenceSetScope,
    ReferenceSetStatus, RenderJobStatus, RenderOutput, RenderRecipe, RenderRecipeKind, ReviewOp,
    SafetyFlags, SavedSearch, Shot, StackItem, StackItemRole, StackMediaKind, Store, StyleProfile,
    TranscriptSegment, VersionStack, Video, VideoSourceMetadata, VideoStatus,
};
use rusqlite::Connection;

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("crush-store-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).expect("test data directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn video(id: &str, sha256: &str) -> Video {
    Video {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: format!("/footage/{id}.mov"),
        sha256: sha256.to_owned(),
        duration_s: Some(12.5),
        fps: Some(24.0),
        width: Some(3840),
        height: Some(2160),
        has_audio: true,
        status: VideoStatus::Pending,
        indexed_at: None,
    }
}

fn shot(id: &str, video_id: &str, idx: i64) -> Shot {
    let start_s = idx as f64;
    Shot {
        id: id.to_owned(),
        video_id: video_id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        idx,
        start_s,
        end_s: start_s + 1.0,
        rep_frame_s: start_s + 0.4,
        thumb_rel: None,
        scene_score: Some(idx as f64 / 100.0),
    }
}

fn photo(id: &str, sha256: &str) -> Photo {
    Photo {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: format!("/photos/{id}.jpg"),
        sha256: sha256.to_owned(),
        width: 6000,
        height: 4000,
        format: "jpeg".to_owned(),
        orientation: Some(1),
        captured_at: None,
        camera_make: None,
        camera_model: None,
        lens: None,
        thumb_rel: None,
        status: PhotoStatus::Pending,
        indexed_at: None,
    }
}

fn feedback(
    id: &str,
    media_kind: MediaKind,
    media_id: &str,
    signal: FeedbackSignal,
) -> FeedbackEvent {
    FeedbackEvent {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind,
        media_id: media_id.to_owned(),
        signal,
        value: None,
        compared_media_kind: None,
        compared_media_id: None,
        context_json: "{}".to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn style_profile(id: &str) -> StyleProfile {
    StyleProfile {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: format!("style-{id}"),
        version: 1,
        algorithm_version: "pairwise-linear-v1".to_owned(),
        embedding_weights: vec![0.1, -0.2, 0.3],
        feature_weights_json: "{}".to_owned(),
        sample_count: 10,
        held_out_metric: None,
        baseline_metric: None,
        context_key: "default".to_owned(),
        metrics_json: "{}".to_owned(),
        learned: false,
        active: true,
        trained_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

#[test]
fn fresh_database_migrates_once_and_enforces_connection_pragmas() {
    let directory = TestDir::new("migration");
    let store = Store::open(directory.path()).expect("fresh database should open");
    assert_eq!(store.schema_version().unwrap(), 10);
    assert_eq!(store.db_path(), directory.path().join("library.db"));

    let missing_vector = store.put_vector(DEFAULT_OWNER_ID, "missing-shot", &[1.0]);
    assert!(
        missing_vector.is_err(),
        "foreign keys must be enabled on the Store connection"
    );
    drop(store);

    let reopened = Store::open(directory.path()).expect("second open should be a migration no-op");
    assert_eq!(reopened.schema_version().unwrap(), 10);
    let audit = Connection::open(reopened.db_path()).unwrap();
    let journal_mode: String = audit
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

#[test]
fn schema_v3_upgrades_to_strong_shot_components_without_losing_jobs() {
    let directory = TestDir::new("migration-v3-v4");
    let db = directory.path().join("library.db");
    std::fs::create_dir_all(directory.path()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
             ) STRICT;
             INSERT INTO schema_version VALUES (1, 0);",
        )
        .unwrap();
    for (version, migration) in [
        (1, include_str!("../migrations/0001_init.sql")),
        (2, include_str!("../migrations/0002_dam_feedback.sql")),
        (3, include_str!("../migrations/0003_source_fidelity.sql")),
    ] {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status
             ) VALUES ('legacy-video', 'local', '/legacy.mov', 'legacy-sha', 1.0, 24.0,
                       1920, 1080, 1, 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO jobs (
                id, owner_id, video_id, stage, status, started_at, finished_at, duration_ms
             ) VALUES ('legacy-job', 'local', 'legacy-video', 'embed', 'done',
                       '2026-08-28T00:00:00Z', '2026-08-28T00:00:01Z', 1000)",
            [],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);
    let jobs = store.jobs(DEFAULT_OWNER_ID, &JobFilter::default()).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, "legacy-job");
    assert_eq!(jobs[0].stage, Stage::Embed);
    assert_eq!(
        jobs[0].video_id.as_deref(),
        Some("legacy-video"),
        "video job ownership must survive the photo-jobs migration"
    );
    assert!(jobs[0].photo_id.is_none());
}

#[test]
fn schema_v4_jobs_gain_photo_support_without_losing_rows() {
    let directory = TestDir::new("migration-v4-v5");
    let db = directory.path().join("library.db");
    std::fs::create_dir_all(directory.path()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
             ) STRICT;
             INSERT INTO schema_version VALUES (1, 0);",
        )
        .unwrap();
    for (version, migration) in [
        (1, include_str!("../migrations/0001_init.sql")),
        (2, include_str!("../migrations/0002_dam_feedback.sql")),
        (3, include_str!("../migrations/0003_source_fidelity.sql")),
        (4, include_str!("../migrations/0004_strong_shot.sql")),
    ] {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status
             ) VALUES ('legacy-video', 'local', '/legacy.mov', 'legacy-sha', 1.0, 24.0,
                       1920, 1080, 1, 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO jobs (
                id, owner_id, video_id, stage, status, started_at
             ) VALUES ('legacy-job', 'local', 'legacy-video', 'split', 'done',
                       '2026-08-28T00:00:00Z')",
            [],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);
    let jobs = store.jobs(DEFAULT_OWNER_ID, &JobFilter::default()).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].stage, Stage::Split);

    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &Photo {
                id: "photo-job-target".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: "/photos/job-target.jpg".to_owned(),
                sha256: "photo-job-sha".to_owned(),
                width: 100,
                height: 100,
                format: "jpeg".to_owned(),
                orientation: Some(1),
                captured_at: None,
                camera_make: None,
                camera_model: None,
                lens: None,
                thumb_rel: None,
                status: PhotoStatus::Pending,
                indexed_at: None,
            },
        )
        .unwrap();
    let started_at = Utc.with_ymd_and_hms(2026, 8, 28, 19, 0, 0).unwrap();
    let photo_job = store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "photo-ingest-job".to_owned(),
                video_id: None,
                photo_id: Some("photo-job-target".to_owned()),
                stage: Stage::PhotoIngest,
                started_at,
                debug_dir: None,
            },
        )
        .unwrap();
    assert_eq!(photo_job.stage, Stage::PhotoIngest);
    assert_eq!(photo_job.photo_id.as_deref(), Some("photo-job-target"));
    assert!(photo_job.video_id.is_none());

    assert!(
        store
            .job_start(
                DEFAULT_OWNER_ID,
                &NewJob {
                    id: "ambiguous-job".to_owned(),
                    video_id: Some("legacy-video".to_owned()),
                    photo_id: Some("photo-job-target".to_owned()),
                    stage: Stage::Analyze,
                    started_at,
                    debug_dir: None,
                },
            )
            .is_err(),
        "a job must reference exactly one owned asset"
    );
    assert!(
        store
            .job_start(
                DEFAULT_OWNER_ID,
                &NewJob {
                    id: "detached-job".to_owned(),
                    video_id: None,
                    photo_id: None,
                    stage: Stage::Analyze,
                    started_at,
                    debug_dir: None,
                },
            )
            .is_err(),
        "a job must reference exactly one owned asset"
    );
}

#[test]
fn photos_editorial_feedback_and_style_round_trip() {
    let directory = TestDir::new("dam-feedback");
    let mut store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 18, 0, 0).unwrap();
    let first = Photo {
        id: "photo-a".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: "/photos/a.jpg".to_owned(),
        sha256: "photo-sha-a".to_owned(),
        width: 6000,
        height: 4000,
        format: "jpeg".to_owned(),
        orientation: Some(1),
        captured_at: Some(now),
        camera_make: Some("Example".to_owned()),
        camera_model: Some("Camera".to_owned()),
        lens: Some("35mm".to_owned()),
        thumb_rel: Some("photo-a.jpg".to_owned()),
        status: PhotoStatus::Pending,
        indexed_at: None,
    };
    let second = Photo {
        id: "photo-b".to_owned(),
        path: "/photos/b.png".to_owned(),
        sha256: "photo-sha-b".to_owned(),
        format: "png".to_owned(),
        thumb_rel: None,
        ..first.clone()
    };
    assert_eq!(store.upsert_photo(DEFAULT_OWNER_ID, &first).unwrap(), first);
    assert_eq!(
        store.upsert_photo(DEFAULT_OWNER_ID, &second).unwrap(),
        second
    );
    let photo_source = PhotoSourceMetadata {
        photo_id: "photo-a".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        source_format: "jpeg".to_owned(),
        decoder: "image-rs".to_owned(),
        proxy_rel: Some("photos/photo-a.jpg".to_owned()),
        proxy_width: Some(2560),
        proxy_height: Some(1707),
        proxy_sha256: Some("proxy-photo-sha".to_owned()),
        proxy_provenance: PhotoProxyProvenance::DecodedOriginal,
        orientation_applied: true,
        bit_depth: Some(8),
        color_space: Some("sRGB".to_owned()),
        icc_profile_name: Some("sRGB IEC61966-2.1".to_owned()),
        icc_profile_sha256: Some("icc-sha".to_owned()),
        exposure_json: r#"{"f_number":"2.8","iso":400}"#.to_owned(),
        gps_present: true,
        metadata_json: r#"{"gps_policy":"presence_only"}"#.to_owned(),
        original_size_bytes: 123_456,
        extracted_at: now,
    };
    store
        .upsert_photo_source_metadata(DEFAULT_OWNER_ID, &photo_source)
        .unwrap();
    assert_eq!(
        store
            .photo_source_metadata(DEFAULT_OWNER_ID, "photo-a")
            .unwrap(),
        Some(photo_source)
    );
    store
        .put_photo_vector(DEFAULT_OWNER_ID, "photo-a", &[1.0, -0.0, 0.25])
        .unwrap();
    let vector = store
        .vector_for_photo(DEFAULT_OWNER_ID, "photo-a")
        .unwrap()
        .unwrap();
    assert_eq!(vector[0].to_bits(), 1.0_f32.to_bits());
    assert_eq!(vector[1].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(
        store.load_all_photo_vectors(DEFAULT_OWNER_ID).unwrap().0,
        vec!["photo-a"]
    );
    store
        .set_photo_status(DEFAULT_OWNER_ID, "photo-a", PhotoStatus::Done)
        .unwrap();
    assert_eq!(
        store
            .photo_by_path(DEFAULT_OWNER_ID, "/photos/a.jpg")
            .unwrap()
            .unwrap()
            .status,
        PhotoStatus::Done
    );

    let annotation = EditorialAnnotation {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind: MediaKind::Photo,
        media_id: "photo-a".to_owned(),
        description: "quiet architectural portrait with deliberate negative space".to_owned(),
        subjects: "person, architecture".to_owned(),
        action: "standing".to_owned(),
        tags: "warm, geometric, campaign-a".to_owned(),
        quality: Some(5),
        standout: true,
        usable: true,
        faces_visible: true,
        nametags_visible: false,
        blur_required: false,
        crop_x: Some(0.38),
        grade_json: Some(r#"{"warmth":18,"contrast":6}"#.to_owned()),
        notes: "Prefer the asymmetry.".to_owned(),
        updated_at: now,
    };
    store
        .upsert_editorial_annotation(DEFAULT_OWNER_ID, &annotation)
        .unwrap();
    assert_eq!(
        store
            .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-a")
            .unwrap(),
        Some(annotation)
    );

    let assessment = AestheticAssessment {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind: MediaKind::Photo,
        media_id: "photo-a".to_owned(),
        sharpness: 0.92,
        exposure: 0.81,
        contrast: 0.74,
        color_harmony: 0.88,
        balance: 0.83,
        subject_placement: 0.91,
        negative_space: 0.95,
        visual_clarity: 0.86,
        technical_quality: 0.84,
        blur_control: 0.91,
        clipping_control: 0.88,
        noise_control: 0.82,
        compression_quality: 0.86,
        resolution_quality: 0.93,
        motion_stability: 0.5,
        duplicate_confidence: 0.0,
        composition_quality: 0.87,
        hierarchy: 0.9,
        leading_lines: 0.72,
        symmetry: 0.55,
        crop_potential: 0.8,
        moment_story: 0.7,
        expression: 0.5,
        gesture: 0.5,
        action: 0.5,
        novelty: 0.75,
        pacing: 0.5,
        repetition_risk: 0.0,
        overall: 0.89,
        confidence: 0.77,
        explanation_json: r#"{"strengths":["negative space","color harmony"]}"#.to_owned(),
        model_version: "design-baseline-v1".to_owned(),
        assessed_at: now,
    };
    store
        .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &assessment)
        .unwrap();
    assert_eq!(
        store
            .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-a")
            .unwrap(),
        Some(assessment)
    );

    let preference = FeedbackEvent {
        id: "feedback-1".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind: MediaKind::Photo,
        media_id: "photo-a".to_owned(),
        signal: FeedbackSignal::Prefer,
        value: Some(1.0),
        compared_media_kind: Some(MediaKind::Photo),
        compared_media_id: Some("photo-b".to_owned()),
        context_json: r#"{"collection":"homepage hero"}"#.to_owned(),
        created_at: now,
    };
    store
        .append_feedback(DEFAULT_OWNER_ID, &preference)
        .unwrap();
    assert_eq!(
        store.feedback_events(DEFAULT_OWNER_ID).unwrap(),
        vec![preference]
    );

    let profile = StyleProfile {
        id: "style-local-1".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: "default".to_owned(),
        version: 1,
        algorithm_version: "pairwise-linear-v1".to_owned(),
        embedding_weights: vec![0.1, -0.2, 0.3],
        feature_weights_json: r#"{"negative_space":0.8,"color_harmony":0.6}"#.to_owned(),
        sample_count: 42,
        held_out_metric: Some(0.73),
        baseline_metric: Some(0.61),
        context_key: "default".to_owned(),
        metrics_json: r#"{"learned":true}"#.to_owned(),
        learned: true,
        active: true,
        trained_at: now,
    };
    store.put_style_profile(DEFAULT_OWNER_ID, &profile).unwrap();
    assert_eq!(
        store.active_style_profile(DEFAULT_OWNER_ID).unwrap(),
        Some(profile)
    );

    assert!(store
        .upsert_editorial_annotation(
            DEFAULT_OWNER_ID,
            &EditorialAnnotation {
                media_id: "missing".to_owned(),
                ..store
                    .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-a")
                    .unwrap()
                    .unwrap()
            },
        )
        .is_err());
}

#[test]
fn video_source_metadata_round_trips_and_rejects_unsafe_proxies() {
    let directory = TestDir::new("video-source-metadata");
    let store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 18, 30, 0).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-source", "source-sha"))
        .unwrap();
    let metadata = VideoSourceMetadata {
        video_id: "video-source".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        container: "mov,mp4,m4a,3gp,3g2,mj2".to_owned(),
        video_codec: "hevc".to_owned(),
        codec_profile: Some("Main 10".to_owned()),
        pixel_format: Some("yuv420p10le".to_owned()),
        bit_depth: Some(10),
        color_space: Some("bt2020nc".to_owned()),
        color_primaries: Some("bt2020".to_owned()),
        color_transfer: Some("smpte2084".to_owned()),
        color_range: Some("tv".to_owned()),
        rotation: Some(90),
        proxy_rel: Some("videos/video-source.mp4".to_owned()),
        proxy_sha256: Some("proxy-video-sha".to_owned()),
        proxy_required: true,
        proxy_reason: Some("HEVC Main 10 requires a working proxy".to_owned()),
        original_size_bytes: 987_654,
        metadata_json: r#"{"policy_version":1}"#.to_owned(),
        probed_at: now,
    };
    store
        .upsert_video_source_metadata(DEFAULT_OWNER_ID, &metadata)
        .unwrap();
    assert_eq!(
        store
            .video_source_metadata(DEFAULT_OWNER_ID, "video-source")
            .unwrap(),
        Some(metadata.clone())
    );

    let unsafe_metadata = VideoSourceMetadata {
        proxy_rel: Some("../escape.mp4".to_owned()),
        ..metadata
    };
    assert!(store
        .upsert_video_source_metadata(DEFAULT_OWNER_ID, &unsafe_metadata)
        .is_err());
    assert!(store.proxy_path("../escape.mp4").is_err());
}

#[test]
fn videos_shots_and_delete_cascade_round_trip() {
    let directory = TestDir::new("video");
    let mut store = Store::open(directory.path()).unwrap();
    let first = video("video-1", "sha-one");
    assert_eq!(store.upsert_video(DEFAULT_OWNER_ID, &first).unwrap(), first);

    let mut updated = video("ignored-new-id", "sha-one");
    updated.path = "/renamed/source.mov".to_owned();
    updated.duration_s = Some(20.0);
    let returned = store.upsert_video(DEFAULT_OWNER_ID, &updated).unwrap();
    assert_eq!(returned.id, "video-1", "sha identity must retain its id");
    assert_eq!(returned.path, updated.path);
    assert_eq!(returned.duration_s, Some(20.0));

    store
        .set_video_status(DEFAULT_OWNER_ID, "video-1", VideoStatus::Done)
        .unwrap();
    let loaded = store
        .video_by_sha(DEFAULT_OWNER_ID, "sha-one")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, VideoStatus::Done);
    assert!(loaded.indexed_at.is_some());

    store
        .insert_shots(DEFAULT_OWNER_ID, &[shot("shot-1", "video-1", 0)])
        .unwrap();
    assert_eq!(
        store
            .shots_for_video(DEFAULT_OWNER_ID, "video-1")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .shot_by_id(DEFAULT_OWNER_ID, "shot-1")
            .unwrap()
            .unwrap()
            .video_id,
        "video-1"
    );
    assert!(
        store
            .put_vector(DEFAULT_OWNER_ID, "shot-1", &[1.0, -0.0, f32::NAN])
            .is_err(),
        "vectors must reject non-finite values"
    );
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-1", &[1.0, -0.0])
        .unwrap();
    let exact_vector = store
        .vector_for_shot(DEFAULT_OWNER_ID, "shot-1")
        .unwrap()
        .unwrap();
    assert_eq!(exact_vector[0].to_bits(), 1.0_f32.to_bits());
    assert_eq!(exact_vector[1].to_bits(), (-0.0_f32).to_bits());
    store
        .insert_transcript_segments(
            DEFAULT_OWNER_ID,
            &[TranscriptSegment {
                id: "segment-1".to_owned(),
                video_id: "video-1".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                start_s: 0.0,
                end_s: 1.0,
                text: "cascade keyword".to_owned(),
                confidence: Some(0.9),
            }],
        )
        .unwrap();
    store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "job-cascade".to_owned(),
                video_id: Some("video-1".to_owned()),
                photo_id: None,
                stage: Stage::Split,
                started_at: Utc.with_ymd_and_hms(2026, 8, 27, 11, 0, 0).unwrap(),
                debug_dir: None,
            },
        )
        .unwrap();
    store
        .upsert_photo(DEFAULT_OWNER_ID, &photo("photo-cascade", "cascade-sha"))
        .unwrap();
    let on_shot = feedback(
        "feedback-shot",
        MediaKind::Shot,
        "shot-1",
        FeedbackSignal::Pick,
    );
    store.append_feedback(DEFAULT_OWNER_ID, &on_shot).unwrap();
    let preference = FeedbackEvent {
        value: Some(1.0),
        compared_media_kind: Some(MediaKind::Shot),
        compared_media_id: Some("shot-1".to_owned()),
        ..feedback(
            "feedback-prefer",
            MediaKind::Photo,
            "photo-cascade",
            FeedbackSignal::Prefer,
        )
    };
    store
        .append_feedback(DEFAULT_OWNER_ID, &preference)
        .unwrap();

    assert!(store
        .delete_video_cascade(DEFAULT_OWNER_ID, "video-1")
        .unwrap());
    assert!(store
        .video_by_sha(DEFAULT_OWNER_ID, "sha-one")
        .unwrap()
        .is_none());
    assert!(store
        .shot_by_id(DEFAULT_OWNER_ID, "shot-1")
        .unwrap()
        .is_none());
    assert!(store
        .vector_for_shot(DEFAULT_OWNER_ID, "shot-1")
        .unwrap()
        .is_none());
    assert!(store
        .load_all_vectors(DEFAULT_OWNER_ID)
        .unwrap()
        .0
        .is_empty());
    assert!(store
        .search_transcripts(DEFAULT_OWNER_ID, "keyword", 10)
        .unwrap()
        .is_empty());
    assert!(store
        .jobs(DEFAULT_OWNER_ID, &JobFilter::default())
        .unwrap()
        .is_empty());
    assert!(
        store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty(),
        "shot cleanup must remove feedback on the deleted shot and its comparisons"
    );
    assert!(!store
        .delete_video_cascade(DEFAULT_OWNER_ID, "video-1")
        .unwrap());
}

#[test]
fn vectors_are_exact_and_load_as_a_contiguous_matrix_under_budget() {
    const ROWS: usize = 1000;
    const DIM: usize = 512;

    let directory = TestDir::new("vectors");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-vectors", "sha-vectors"))
        .unwrap();

    let shots = (0..ROWS)
        .map(|index| shot(&format!("shot-{index:04}"), "video-vectors", index as i64))
        .collect::<Vec<_>>();
    store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();

    let mut expected = Vec::with_capacity(ROWS * DIM);
    for index in 0..ROWS {
        let values = (0..DIM)
            .map(|column| {
                // Masked to subnormal bits so every generated value is finite; the store
                // rejects non-finite vectors and the round trip must stay bit-exact.
                f32::from_bits(
                    ((index * DIM + column) as u32).wrapping_mul(2_654_435_761) & 0x007F_FFFF,
                )
            })
            .collect::<Vec<_>>();
        store
            .put_vector(DEFAULT_OWNER_ID, &format!("shot-{index:04}"), &values)
            .unwrap();
        expected.extend_from_slice(&values);
    }

    let audit = Connection::open(store.db_path()).unwrap();
    let invalid_lengths: i64 = audit
        .query_row(
            "SELECT count(*) FROM shot_vectors WHERE length(vec) != dim * 4",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        invalid_lengths, 0,
        "each vector must contain exactly dim*4 bytes"
    );

    let started = Instant::now();
    let (ids, matrix) = store.load_all_vectors(DEFAULT_OWNER_ID).unwrap();
    let elapsed = started.elapsed();
    eprintln!("loaded {ROWS}x{DIM} vectors in {elapsed:?}");
    assert_eq!(ids.len(), ROWS);
    assert_eq!(matrix.len(), ROWS * DIM);
    assert!(
        elapsed < Duration::from_millis(50),
        "1000x512 vector load took {elapsed:?}"
    );
    assert!(
        matrix
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| actual.to_bits() == expected.to_bits()),
        "little-endian round trip must preserve every f32 bit"
    );
}

#[test]
fn transcript_overlap_and_fts_queries_return_typed_segments() {
    let directory = TestDir::new("transcripts");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-t", "sha-t"))
        .unwrap();
    let segments = vec![
        TranscriptSegment {
            id: "segment-a".to_owned(),
            video_id: "video-t".to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            start_s: 0.0,
            end_s: 2.0,
            text: "A lighthouse appears on the horizon".to_owned(),
            confidence: Some(0.95),
        },
        TranscriptSegment {
            id: "segment-b".to_owned(),
            video_id: "video-t".to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            start_s: 2.0,
            end_s: 4.0,
            text: "Ocean waves reach the rocks".to_owned(),
            confidence: None,
        },
    ];
    store
        .insert_transcript_segments(DEFAULT_OWNER_ID, &segments)
        .unwrap();

    assert_eq!(
        store
            .segments_overlapping(DEFAULT_OWNER_ID, "video-t", 1.5, 2.5)
            .unwrap(),
        segments
    );
    assert_eq!(
        store
            .segments_overlapping(DEFAULT_OWNER_ID, "video-t", 2.0, 3.0)
            .unwrap(),
        vec![segments[1].clone()],
        "touching half-open ranges must not overlap"
    );
    assert_eq!(
        store
            .search_transcripts(DEFAULT_OWNER_ID, "lighthouse", 10)
            .unwrap(),
        vec![segments[0].clone()]
    );
}

#[test]
fn replacing_transcripts_removes_stale_rows_from_table_and_fts() {
    let directory = TestDir::new("replace-transcripts");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-r", "sha-r"))
        .unwrap();
    let old = TranscriptSegment {
        id: "old-segment".to_owned(),
        video_id: "video-r".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        start_s: 0.0,
        end_s: 1.0,
        text: "obsolete lighthouse words".to_owned(),
        confidence: Some(0.5),
    };
    let replacement = TranscriptSegment {
        id: "new-segment".to_owned(),
        video_id: "video-r".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        start_s: 1.0,
        end_s: 2.0,
        text: "current rocket words".to_owned(),
        confidence: Some(0.9),
    };
    store
        .insert_transcript_segments(DEFAULT_OWNER_ID, &[old])
        .unwrap();
    store
        .replace_transcript_segments(
            DEFAULT_OWNER_ID,
            "video-r",
            std::slice::from_ref(&replacement),
        )
        .unwrap();

    assert!(store
        .search_transcripts(DEFAULT_OWNER_ID, "obsolete", 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .search_transcripts(DEFAULT_OWNER_ID, "rocket", 10)
            .unwrap(),
        vec![replacement]
    );
    assert_eq!(
        store
            .transcript_count_for_video(DEFAULT_OWNER_ID, "video-r")
            .unwrap(),
        1
    );
}

#[test]
fn jobs_and_embedding_metadata_round_trip_for_every_terminal_state() {
    let directory = TestDir::new("jobs");
    let store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-j", "sha-j"))
        .unwrap();
    let started_at = Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap();

    for (id, stage) in [
        ("job-done", Stage::Split),
        ("job-failed", Stage::Embed),
        ("job-cancelled", Stage::Transcribe),
    ] {
        let started = store
            .job_start(
                DEFAULT_OWNER_ID,
                &NewJob {
                    id: id.to_owned(),
                    video_id: Some("video-j".to_owned()),
                    photo_id: None,
                    stage,
                    started_at,
                    debug_dir: Some(format!("debug/{id}")),
                },
            )
            .unwrap();
        assert_eq!(started.status, JobStatus::Running);
    }

    store
        .job_finish(
            DEFAULT_OWNER_ID,
            "job-done",
            started_at + chrono::Duration::milliseconds(1250),
        )
        .unwrap();
    store
        .job_fail(
            DEFAULT_OWNER_ID,
            "job-failed",
            started_at + chrono::Duration::seconds(2),
            "model unavailable",
        )
        .unwrap();
    store
        .job_cancel(
            DEFAULT_OWNER_ID,
            "job-cancelled",
            started_at + chrono::Duration::milliseconds(750),
        )
        .unwrap();

    let done = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                status: Some(JobStatus::Done),
                ..JobFilter::default()
            },
        )
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].id, "job-done");
    assert_eq!(done[0].duration_ms, Some(1250));

    let failed_embed = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                video_id: Some("video-j".to_owned()),
                stage: Some(Stage::Embed),
                status: Some(JobStatus::Failed),
            },
        )
        .unwrap();
    assert_eq!(failed_embed.len(), 1);
    assert_eq!(failed_embed[0].error.as_deref(), Some("model unavailable"));

    let cancelled = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                status: Some(JobStatus::Cancelled),
                ..JobFilter::default()
            },
        )
        .unwrap();
    assert_eq!(cancelled[0].duration_ms, Some(750));

    assert!(store
        .embedding_meta_get(DEFAULT_OWNER_ID)
        .unwrap()
        .is_none());
    let mut metadata = EmbeddingMeta {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        model_name: "clip-vit-b-32".to_owned(),
        model_sha256: "abc123".to_owned(),
        dim: 512,
        preprocess_version: 1,
    };
    store
        .embedding_meta_set(DEFAULT_OWNER_ID, &metadata)
        .unwrap();
    assert_eq!(
        store.embedding_meta_get(DEFAULT_OWNER_ID).unwrap(),
        Some(metadata.clone())
    );
    metadata.preprocess_version = 2;
    store
        .embedding_meta_set(DEFAULT_OWNER_ID, &metadata)
        .unwrap();
    assert_eq!(
        store.embedding_meta_get(DEFAULT_OWNER_ID).unwrap(),
        Some(metadata)
    );
}

#[test]
fn interrupted_jobs_fail_and_failed_videos_restore_last_completed_stage() {
    let directory = TestDir::new("interrupted-job");
    let store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-i", "sha-i"))
        .unwrap();
    let started = Utc::now();
    store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "split-done".to_owned(),
                video_id: Some("video-i".to_owned()),
                photo_id: None,
                stage: Stage::Split,
                started_at: started,
                debug_dir: None,
            },
        )
        .unwrap();
    store
        .job_finish(DEFAULT_OWNER_ID, "split-done", started)
        .unwrap();
    store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "embed-running".to_owned(),
                video_id: Some("video-i".to_owned()),
                photo_id: None,
                stage: Stage::Embed,
                started_at: started,
                debug_dir: None,
            },
        )
        .unwrap();
    assert_eq!(
        store
            .fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .video_by_id(DEFAULT_OWNER_ID, "video-i")
            .unwrap()
            .unwrap()
            .status,
        VideoStatus::Failed
    );
    assert_eq!(
        store
            .restore_failed_video_status(DEFAULT_OWNER_ID, "video-i")
            .unwrap(),
        VideoStatus::Split
    );
    let failed = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                status: Some(JobStatus::Failed),
                ..JobFilter::default()
            },
        )
        .unwrap();
    assert_eq!(failed[0].error.as_deref(), Some("interrupted"));
}

#[test]
fn deep_integrity_reports_missing_vectors_and_thumbnail_files() {
    let directory = TestDir::new("integrity");
    let mut store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-i", "sha-i"))
        .unwrap();
    let mut with_thumb = shot("shot-vector", "video-i", 0);
    with_thumb.thumb_rel = Some("present.jpg".to_owned());
    let mut missing = shot("shot-missing", "video-i", 1);
    missing.thumb_rel = Some("missing.jpg".to_owned());
    store
        .insert_shots(DEFAULT_OWNER_ID, &[with_thumb, missing])
        .unwrap();
    std::fs::write(directory.path().join("thumbs/present.jpg"), b"jpeg").unwrap();
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-vector", &[0.0; 512])
        .unwrap();
    store
        .set_video_status(DEFAULT_OWNER_ID, "video-i", VideoStatus::Embedded)
        .unwrap();

    let mut integrity_photo = photo("photo-integrity", "sha-photo-i");
    integrity_photo.thumb_rel = Some("present.jpg".to_owned());
    store
        .upsert_photo(DEFAULT_OWNER_ID, &integrity_photo)
        .unwrap();
    store
        .set_photo_status(DEFAULT_OWNER_ID, "photo-integrity", PhotoStatus::Embedded)
        .unwrap();

    let problems = store.integrity().unwrap();
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingVector && problem.entity_id == "shot-missing"
    }));
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingThumbnail && problem.entity_id == "shot-missing"
    }));
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingVector && problem.entity_id == "photo-integrity"
    }));
    assert_eq!(
        problems
            .iter()
            .filter(|problem| problem.entity_id == "shot-vector")
            .count(),
        0
    );

    store
        .put_photo_vector(DEFAULT_OWNER_ID, "photo-integrity", &[0.0; 512])
        .unwrap();
    let metadata = PhotoSourceMetadata {
        photo_id: "photo-integrity".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        source_format: "jpeg".to_owned(),
        decoder: "image-rs".to_owned(),
        proxy_rel: Some("photos/photo-integrity.bin".to_owned()),
        proxy_width: Some(2560),
        proxy_height: Some(1707),
        proxy_sha256: Some("proxy-sha".to_owned()),
        proxy_provenance: PhotoProxyProvenance::FullRender,
        orientation_applied: true,
        bit_depth: Some(8),
        color_space: None,
        icc_profile_name: None,
        icc_profile_sha256: None,
        exposure_json: "{}".to_owned(),
        gps_present: false,
        metadata_json: "{}".to_owned(),
        original_size_bytes: 1_024,
        extracted_at: now,
    };
    store
        .upsert_photo_source_metadata(DEFAULT_OWNER_ID, &metadata)
        .unwrap();

    let problems = store.integrity().unwrap();
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingProxy && problem.entity_id == "photo-integrity"
    }));

    std::fs::create_dir_all(directory.path().join("proxies/photos")).unwrap();
    std::fs::write(
        directory.path().join("proxies/photos/photo-integrity.bin"),
        b"proxy",
    )
    .unwrap();

    // The API rejects unsafe proxy paths, so corrupt one through raw SQL.
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "UPDATE photo_source_metadata SET proxy_rel = '../escape.bin'
             WHERE photo_id = 'photo-integrity'",
            [],
        )
        .unwrap();
    let problems = store.integrity().unwrap();
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::UnsafeProxyPath && problem.entity_id == "photo-integrity"
    }));
    audit
        .execute(
            "UPDATE photo_source_metadata SET proxy_rel = 'photos/photo-integrity.bin'
             WHERE photo_id = 'photo-integrity'",
            [],
        )
        .unwrap();

    // Raw-SQL corruption the typed API cannot produce: an orphan photo vector and a
    // truncated shot vector blob. The orphan insert needs foreign keys off on the audit
    // connection because photo_vectors carries a composite FK — the row models a
    // pre-existing corrupted database, which is exactly what integrity() must catch.
    audit.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    audit
        .execute(
            "INSERT INTO photo_vectors (photo_id, owner_id, dim, vec)
             VALUES ('ghost-photo', 'local', 1, X'00000000')",
            [],
        )
        .unwrap();
    audit.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    audit
        .execute(
            "UPDATE shot_vectors SET vec = X'00' WHERE shot_id = 'shot-vector'",
            [],
        )
        .unwrap();
    let problems = store.integrity().unwrap();
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::OrphanVector && problem.entity_id == "ghost-photo"
    }));
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::InvalidVectorBytes && problem.entity_id == "shot-vector"
    }));
    audit
        .execute(
            "DELETE FROM photo_vectors WHERE photo_id = 'ghost-photo'",
            [],
        )
        .unwrap();

    // Style profile weights must match their declared embedding dimension.
    store
        .put_style_profile(DEFAULT_OWNER_ID, &style_profile("style-integrity"))
        .unwrap();
    audit
        .execute(
            "UPDATE style_profiles SET embedding_weights = X'00' WHERE id = 'style-integrity'",
            [],
        )
        .unwrap();
    let problems = store.integrity().unwrap();
    let weights = problems
        .iter()
        .find(|problem| {
            problem.kind == ProblemKind::InvalidVectorBytes
                && problem.entity_id == "style-integrity"
        })
        .expect("style profile weight corruption must be reported");
    assert!(weights.detail.contains("style-integrity"));
    audit
        .execute(
            "DELETE FROM style_profiles WHERE id = 'style-integrity'",
            [],
        )
        .unwrap();
    drop(audit);

    store
        .put_vector(DEFAULT_OWNER_ID, "shot-vector", &[0.0; 512])
        .unwrap();
    std::fs::write(directory.path().join("thumbs/missing.jpg"), b"jpeg").unwrap();
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-missing", &[0.0; 512])
        .unwrap();
    assert!(store.integrity().unwrap().is_empty());
}

#[test]
fn photos_for_analysis_returns_only_missing_or_stale_done_photos() {
    let directory = TestDir::new("photos-for-analysis");
    let store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 19, 30, 0).unwrap();
    let photo = |id: &str, path: &str, status: PhotoStatus| Photo {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: path.to_owned(),
        sha256: format!("sha-{id}"),
        width: 100,
        height: 100,
        format: "jpeg".to_owned(),
        orientation: Some(1),
        captured_at: None,
        camera_make: None,
        camera_model: None,
        lens: None,
        thumb_rel: None,
        status,
        indexed_at: None,
    };
    let current = photo("photo-current", "/photos/z-current.jpg", PhotoStatus::Done);
    let stale = photo("photo-stale", "/photos/a-stale.jpg", PhotoStatus::Done);
    let missing = photo("photo-missing", "/photos/m-missing.jpg", PhotoStatus::Done);
    let pending = photo(
        "photo-pending",
        "/photos/p-pending.jpg",
        PhotoStatus::Pending,
    );
    for candidate in [&current, &stale, &missing, &pending] {
        store.upsert_photo(DEFAULT_OWNER_ID, candidate).unwrap();
    }
    let assessment = |photo_id: &str, model_version: &str| AestheticAssessment {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind: MediaKind::Photo,
        media_id: photo_id.to_owned(),
        sharpness: 0.5,
        exposure: 0.5,
        contrast: 0.5,
        color_harmony: 0.5,
        balance: 0.5,
        subject_placement: 0.5,
        negative_space: 0.5,
        visual_clarity: 0.5,
        technical_quality: 0.5,
        blur_control: 0.5,
        clipping_control: 0.5,
        noise_control: 0.5,
        compression_quality: 0.5,
        resolution_quality: 0.5,
        motion_stability: 0.5,
        duplicate_confidence: 0.0,
        composition_quality: 0.5,
        hierarchy: 0.5,
        leading_lines: 0.5,
        symmetry: 0.5,
        crop_potential: 0.5,
        moment_story: 0.5,
        expression: 0.5,
        gesture: 0.5,
        action: 0.5,
        novelty: 0.5,
        pacing: 0.5,
        repetition_risk: 0.0,
        overall: 0.5,
        confidence: 0.5,
        explanation_json: "{}".to_owned(),
        model_version: model_version.to_owned(),
        assessed_at: now,
    };
    store
        .upsert_aesthetic_assessment(
            DEFAULT_OWNER_ID,
            &assessment("photo-current", "strong-shot-v1"),
        )
        .unwrap();
    store
        .upsert_aesthetic_assessment(
            DEFAULT_OWNER_ID,
            &assessment("photo-stale", "strong-shot-v0"),
        )
        .unwrap();

    let needed = store
        .photos_for_analysis(DEFAULT_OWNER_ID, "strong-shot-v1")
        .unwrap();
    assert_eq!(
        needed
            .iter()
            .map(|photo| photo.id.as_str())
            .collect::<Vec<_>>(),
        vec!["photo-stale", "photo-missing"],
        "only stale or missing assessments are backfilled, in photos() order (path, id)"
    );

    store
        .upsert_aesthetic_assessment(
            DEFAULT_OWNER_ID,
            &assessment("photo-stale", "strong-shot-v1"),
        )
        .unwrap();
    let needed = store
        .photos_for_analysis(DEFAULT_OWNER_ID, "strong-shot-v1")
        .unwrap();
    assert_eq!(
        needed
            .iter()
            .map(|photo| photo.id.as_str())
            .collect::<Vec<_>>(),
        vec!["photo-missing"]
    );
}

#[test]
fn embedded_preview_provenance_is_rejected_structurally() {
    let directory = TestDir::new("embedded-preview-rejected");
    let store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 19, 45, 0).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &Photo {
                id: "photo-preview".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: "/photos/preview.jpg".to_owned(),
                sha256: "sha-preview".to_owned(),
                width: 100,
                height: 100,
                format: "jpeg".to_owned(),
                orientation: Some(1),
                captured_at: None,
                camera_make: None,
                camera_model: None,
                lens: None,
                thumb_rel: None,
                status: PhotoStatus::Pending,
                indexed_at: None,
            },
        )
        .unwrap();
    let metadata = PhotoSourceMetadata {
        photo_id: "photo-preview".to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        source_format: "cr3".to_owned(),
        decoder: "macos-imageio".to_owned(),
        proxy_rel: Some("photos/photo-preview.jpg".to_owned()),
        proxy_width: Some(960),
        proxy_height: Some(640),
        proxy_sha256: Some("proxy-sha".to_owned()),
        proxy_provenance: PhotoProxyProvenance::EmbeddedPreview,
        orientation_applied: true,
        bit_depth: Some(8),
        color_space: None,
        icc_profile_name: None,
        icc_profile_sha256: None,
        exposure_json: "{}".to_owned(),
        gps_present: false,
        metadata_json: "{}".to_owned(),
        original_size_bytes: 1000,
        extracted_at: now,
    };
    let error = store
        .upsert_photo_source_metadata(DEFAULT_OWNER_ID, &metadata)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("embedded_preview provenance is not producible"),
        "unexpected error: {error}"
    );
}

#[test]
fn feedback_is_append_only_and_enforces_signal_rules_at_the_api() {
    let directory = TestDir::new("feedback-hardening");
    let store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(DEFAULT_OWNER_ID, &photo("photo-a", "sha-a"))
        .unwrap();
    store
        .upsert_photo(DEFAULT_OWNER_ID, &photo("photo-b", "sha-b"))
        .unwrap();
    let base = feedback(
        "feedback-base",
        MediaKind::Photo,
        "photo-a",
        FeedbackSignal::Pick,
    );
    store.append_feedback(DEFAULT_OWNER_ID, &base).unwrap();
    let on_photo_b = feedback(
        "feedback-photo-b",
        MediaKind::Photo,
        "photo-b",
        FeedbackSignal::Pick,
    );
    store
        .append_feedback(DEFAULT_OWNER_ID, &on_photo_b)
        .unwrap();
    let pair = FeedbackEvent {
        value: Some(1.0),
        compared_media_kind: Some(MediaKind::Photo),
        compared_media_id: Some("photo-b".to_owned()),
        ..feedback(
            "feedback-pair",
            MediaKind::Photo,
            "photo-a",
            FeedbackSignal::Prefer,
        )
    };
    store.append_feedback(DEFAULT_OWNER_ID, &pair).unwrap();

    // The '{}' context default round-trips through the typed API.
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO feedback_events (
                id, owner_id, media_kind, media_id, signal, created_at
             ) VALUES ('feedback-default', 'local', 'photo', 'photo-a', 'tag',
                       '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    let defaulted = store
        .feedback_events(DEFAULT_OWNER_ID)
        .unwrap()
        .into_iter()
        .find(|event| event.id == "feedback-default")
        .expect("feedback row written without context_json");
    assert_eq!(defaulted.context_json, "{}");

    // Rating values stay API-enforced and must fall within 1..=5.
    for rating in [0.0, 5.5, f64::NAN] {
        assert!(store
            .append_feedback(
                DEFAULT_OWNER_ID,
                &FeedbackEvent {
                    id: format!("feedback-rating-{rating}"),
                    signal: FeedbackSignal::Rating,
                    value: Some(rating),
                    ..base.clone()
                },
            )
            .is_err());
    }
    // Prefer requires a compared asset.
    assert!(store
        .append_feedback(
            DEFAULT_OWNER_ID,
            &feedback(
                "feedback-lone-prefer",
                MediaKind::Photo,
                "photo-a",
                FeedbackSignal::Prefer,
            ),
        )
        .is_err());
    // Only prefer feedback may compare two assets.
    assert!(store
        .append_feedback(
            DEFAULT_OWNER_ID,
            &FeedbackEvent {
                compared_media_kind: Some(MediaKind::Photo),
                compared_media_id: Some("photo-b".to_owned()),
                ..feedback(
                    "feedback-crossed",
                    MediaKind::Photo,
                    "photo-a",
                    FeedbackSignal::Pick,
                )
            },
        )
        .is_err());
    // Duplicate event ids are rejected and leave the original row untouched.
    assert!(store.append_feedback(DEFAULT_OWNER_ID, &base).is_err());
    // context_json must be a valid JSON object.
    let bad_context = FeedbackEvent {
        id: "feedback-context-bad".to_owned(),
        context_json: "not json".to_owned(),
        ..base.clone()
    };
    assert!(store
        .append_feedback(DEFAULT_OWNER_ID, &bad_context)
        .is_err());
    let array_context = FeedbackEvent {
        id: "feedback-context-array".to_owned(),
        context_json: "[1, 2]".to_owned(),
        ..base.clone()
    };
    assert!(store
        .append_feedback(DEFAULT_OWNER_ID, &array_context)
        .is_err());
    assert_eq!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(), 4);

    // Live feedback rows are immutable and cannot be deleted.
    assert!(audit
        .execute(
            "UPDATE feedback_events SET value = 9.0 WHERE id = 'feedback-base'",
            [],
        )
        .is_err());
    assert!(audit
        .execute("DELETE FROM feedback_events WHERE id = 'feedback-base'", [],)
        .is_err());
    assert_eq!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(), 4);

    // Cleanup paths still remove dependent feedback rows: deleting photo-b removes the pick
    // that targeted it and the preference that compared against it.
    audit
        .execute("DELETE FROM photos WHERE id = 'photo-b'", [])
        .unwrap();
    let remaining = store
        .feedback_events(DEFAULT_OWNER_ID)
        .unwrap()
        .into_iter()
        .map(|event| event.id)
        .collect::<Vec<_>>();
    assert_eq!(remaining, vec!["feedback-base", "feedback-default"]);
    // Deleting photo-a removes the remaining rows, both of which target it.
    audit
        .execute("DELETE FROM photos WHERE id = 'photo-a'", [])
        .unwrap();
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());
}

#[test]
fn schema_v4_upgrades_to_hardened_feedback_without_losing_data() {
    let directory = TestDir::new("migration-v4-v5");
    let db = directory.path().join("library.db");
    std::fs::create_dir_all(directory.path()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
             ) STRICT;
             INSERT INTO schema_version VALUES (1, 0);",
        )
        .unwrap();
    for (version, migration) in [
        (1, include_str!("../migrations/0001_init.sql")),
        (2, include_str!("../migrations/0002_dam_feedback.sql")),
        (3, include_str!("../migrations/0003_source_fidelity.sql")),
        (4, include_str!("../migrations/0004_strong_shot.sql")),
    ] {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO photos (
                id, owner_id, path, sha256, width, height, format, status
             ) VALUES ('legacy-photo', 'local', '/legacy.jpg', 'legacy-sha', 100, 100,
                       'jpeg', 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photo_vectors (photo_id, owner_id, dim, vec)
             VALUES ('legacy-photo', 'local', 2, X'0000803F0000803F')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO feedback_events (
                id, owner_id, media_kind, media_id, signal, value, context_json, created_at
             ) VALUES ('legacy-feedback', 'local', 'photo', 'legacy-photo', 'pick', NULL,
                       '{}', '2026-08-28T00:00:00+00:00')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO style_profiles (
                id, owner_id, name, version, algorithm_version, embedding_dim,
                embedding_weights, feature_weights_json, sample_count, active, trained_at
             ) VALUES ('legacy-style', 'local', 'legacy', 1, 'pairwise-linear-v1', 2,
                       X'0000803F0000803F', '{}', 3, 1, '2026-08-28T00:00:00+00:00')",
            [],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);
    assert!(store
        .photo_by_id(DEFAULT_OWNER_ID, "legacy-photo")
        .unwrap()
        .is_some());
    assert_eq!(
        store
            .vector_for_photo(DEFAULT_OWNER_ID, "legacy-photo")
            .unwrap(),
        Some(vec![1.0, 1.0])
    );
    let events = store.feedback_events(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "legacy-feedback");
    assert_eq!(events[0].context_json, "{}");
    let profile = store
        .active_style_profile(DEFAULT_OWNER_ID)
        .unwrap()
        .expect("legacy style profile should survive the upgrade");
    assert_eq!(profile.id, "legacy-style");
    assert_eq!(profile.embedding_weights, vec![1.0, 1.0]);

    // The append-only guards are enforced on the upgraded database too.
    let audit = Connection::open(store.db_path()).unwrap();
    assert!(audit
        .execute(
            "UPDATE feedback_events SET value = 4.0 WHERE id = 'legacy-feedback'",
            [],
        )
        .is_err());
    assert!(audit
        .execute(
            "DELETE FROM feedback_events WHERE id = 'legacy-feedback'",
            [],
        )
        .is_err());
}

#[test]
fn second_owner_rows_are_isolated_from_the_default_owner() {
    let directory = TestDir::new("owner-isolation");
    let mut store = Store::open(directory.path()).unwrap();
    const OWNER_B: &str = "editor-b";
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('editor-b', 'Editor B', '2026-08-28T00:00:00+00:00')",
            [],
        )
        .unwrap();

    // Photos.
    let mut photo_b = photo("photo-owner-b", "sha-photo-b");
    photo_b.owner_id = OWNER_B.to_owned();
    store.upsert_photo(OWNER_B, &photo_b).unwrap();
    let photo_a = photo("photo-owner-a", "sha-photo-a");
    store.upsert_photo(DEFAULT_OWNER_ID, &photo_a).unwrap();
    let mut id_collision = photo("placeholder", "sha-collision");
    id_collision.id = "photo-owner-b".to_owned();
    assert!(store.upsert_photo(DEFAULT_OWNER_ID, &id_collision).is_err());
    assert!(store
        .photo_by_path(DEFAULT_OWNER_ID, &photo_b.path)
        .unwrap()
        .is_none());
    assert!(store
        .photo_by_path(OWNER_B, &photo_a.path)
        .unwrap()
        .is_none());
    assert_eq!(
        store.photos(DEFAULT_OWNER_ID).unwrap(),
        vec![photo_a.clone()]
    );
    assert_eq!(store.photos(OWNER_B).unwrap(), vec![photo_b.clone()]);

    // Videos and shots.
    let mut video_b = video("video-owner-b", "sha-video-b");
    video_b.owner_id = OWNER_B.to_owned();
    store.upsert_video(OWNER_B, &video_b).unwrap();
    let video_a = video("video-owner-a", "sha-video-a");
    store.upsert_video(DEFAULT_OWNER_ID, &video_a).unwrap();
    let mut shot_b = shot("shot-owner-b", "video-owner-b", 0);
    shot_b.owner_id = OWNER_B.to_owned();
    store
        .insert_shots(OWNER_B, std::slice::from_ref(&shot_b))
        .unwrap();
    let shot_a = shot("shot-owner-a", "video-owner-a", 0);
    store
        .insert_shots(DEFAULT_OWNER_ID, std::slice::from_ref(&shot_a))
        .unwrap();
    assert!(store
        .video_by_id(DEFAULT_OWNER_ID, "video-owner-b")
        .unwrap()
        .is_none());
    assert!(store
        .shot_by_id(DEFAULT_OWNER_ID, "shot-owner-b")
        .unwrap()
        .is_none());
    assert_eq!(
        store.shots_for_video(OWNER_B, "video-owner-b").unwrap(),
        vec![shot_b.clone()]
    );
    assert!(store
        .insert_shots(DEFAULT_OWNER_ID, &[shot("cross-owner", "video-owner-b", 1)])
        .is_err());
    assert!(store
        .set_video_status(DEFAULT_OWNER_ID, "video-owner-b", VideoStatus::Done)
        .is_err());

    // Vectors.
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-owner-a", &[1.0, 2.0])
        .unwrap();
    store
        .put_vector(OWNER_B, "shot-owner-b", &[3.0, 4.0])
        .unwrap();
    store
        .put_photo_vector(DEFAULT_OWNER_ID, "photo-owner-a", &[5.0])
        .unwrap();
    store
        .put_photo_vector(OWNER_B, "photo-owner-b", &[6.0])
        .unwrap();
    assert!(store
        .put_vector(DEFAULT_OWNER_ID, "shot-owner-b", &[0.0])
        .is_err());
    assert!(store
        .put_photo_vector(DEFAULT_OWNER_ID, "photo-owner-b", &[0.0])
        .is_err());
    assert!(store
        .vector_for_shot(DEFAULT_OWNER_ID, "shot-owner-b")
        .unwrap()
        .is_none());
    assert!(store
        .vector_for_photo(DEFAULT_OWNER_ID, "photo-owner-b")
        .unwrap()
        .is_none());
    assert_eq!(
        store.load_all_vectors(DEFAULT_OWNER_ID).unwrap().0,
        vec!["shot-owner-a"]
    );
    assert_eq!(
        store.load_all_vectors(OWNER_B).unwrap().0,
        vec!["shot-owner-b"]
    );
    assert_eq!(
        store.load_all_photo_vectors(DEFAULT_OWNER_ID).unwrap().0,
        vec!["photo-owner-a"]
    );
    assert_eq!(
        store.load_all_photo_vectors(OWNER_B).unwrap().0,
        vec!["photo-owner-b"]
    );

    // Feedback.
    let event_a = feedback(
        "feedback-owner-a",
        MediaKind::Photo,
        "photo-owner-a",
        FeedbackSignal::Pick,
    );
    store.append_feedback(DEFAULT_OWNER_ID, &event_a).unwrap();
    let mut event_b = feedback(
        "feedback-owner-b",
        MediaKind::Photo,
        "photo-owner-b",
        FeedbackSignal::Pick,
    );
    event_b.owner_id = OWNER_B.to_owned();
    store.append_feedback(OWNER_B, &event_b).unwrap();
    assert!(store.append_feedback(DEFAULT_OWNER_ID, &event_b).is_err());
    let crossed = FeedbackEvent {
        media_id: "photo-owner-b".to_owned(),
        ..event_a.clone()
    };
    assert!(store.append_feedback(DEFAULT_OWNER_ID, &crossed).is_err());
    assert_eq!(
        store.feedback_events(DEFAULT_OWNER_ID).unwrap(),
        vec![event_a.clone()]
    );
    assert_eq!(
        store.feedback_events(OWNER_B).unwrap(),
        vec![event_b.clone()]
    );

    // Style profiles: same-owner upserts update, cross-owner id collisions fail closed.
    let profile_a = style_profile("style-owner-a");
    store
        .put_style_profile(DEFAULT_OWNER_ID, &profile_a)
        .unwrap();
    assert_eq!(
        store.active_style_profile(DEFAULT_OWNER_ID).unwrap(),
        Some(profile_a.clone())
    );
    let mut profile_b = style_profile("style-owner-b");
    profile_b.owner_id = OWNER_B.to_owned();
    store.put_style_profile(OWNER_B, &profile_b).unwrap();
    let mut stolen = style_profile("style-owner-a");
    stolen.owner_id = OWNER_B.to_owned();
    assert!(store.put_style_profile(OWNER_B, &stolen).is_err());
    assert_eq!(
        store.active_style_profile(DEFAULT_OWNER_ID).unwrap(),
        Some(profile_a.clone()),
        "the original owner's profile must be unchanged after a rejected upsert"
    );
    assert_eq!(
        store.active_style_profile(OWNER_B).unwrap().map(|p| p.id),
        Some("style-owner-b".to_owned())
    );
    let mut updated_a = profile_a.clone();
    updated_a.version = 2;
    store
        .put_style_profile(DEFAULT_OWNER_ID, &updated_a)
        .unwrap();
    assert_eq!(
        store.active_style_profile(DEFAULT_OWNER_ID).unwrap(),
        Some(updated_a)
    );
}

fn reference_set(id: &str, name: &str, status: ReferenceSetStatus) -> ReferenceSet {
    ReferenceSet {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: name.to_owned(),
        context_key: "default".to_owned(),
        description: "finished selects".to_owned(),
        scope: ReferenceSetScope::WholeSet,
        status,
        source_collection_id: None,
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
        confirmed_at: None,
    }
}

fn reference_photo(id: &str, sha256: &str) -> Photo {
    Photo {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        path: format!("/photos/{id}.jpg"),
        sha256: sha256.to_owned(),
        width: 100,
        height: 100,
        format: "jpeg".to_owned(),
        orientation: Some(1),
        captured_at: None,
        camera_make: None,
        camera_model: None,
        lens: None,
        thumb_rel: None,
        status: PhotoStatus::Done,
        indexed_at: None,
    }
}

#[test]
fn reference_sets_round_trip_with_owner_isolation_and_cascade() {
    const OWNER_B: &str = "editor-b";
    let directory = TestDir::new("reference-sets");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('editor-b', 'Editor B', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-ref-a", "ref-sha-a"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-ref-b", "ref-sha-b"),
        )
        .unwrap();

    let set = reference_set("set-a", "previous work", ReferenceSetStatus::Unconfirmed);
    store.reference_set_create(DEFAULT_OWNER_ID, &set).unwrap();
    assert_eq!(
        store.reference_set_get(DEFAULT_OWNER_ID, "set-a").unwrap(),
        Some(set.clone())
    );
    // Owner isolation: the same name is allowed for another owner, and neither sees the other.
    let mut other_owner = reference_set("set-b", "previous work", ReferenceSetStatus::Unconfirmed);
    other_owner.owner_id = OWNER_B.to_owned();
    store.reference_set_create(OWNER_B, &other_owner).unwrap();
    assert_eq!(store.reference_set_list(OWNER_B).unwrap().len(), 1);
    assert!(store.reference_set_get(OWNER_B, "set-a").unwrap().is_none());
    let crossed = ReferenceSetItem {
        owner_id: OWNER_B.to_owned(),
        set_id: "set-a".to_owned(),
        media_kind: MediaKind::Photo,
        media_id: "photo-ref-a".to_owned(),
        role: ReferenceItemRole::Positive,
        added_at: now,
    };
    assert!(store.reference_set_add_item(OWNER_B, &crossed).is_err());

    let item = |media_id: &str| ReferenceSetItem {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        set_id: "set-a".to_owned(),
        media_kind: MediaKind::Photo,
        media_id: media_id.to_owned(),
        role: ReferenceItemRole::Positive,
        added_at: now,
    };
    store
        .reference_set_add_item(DEFAULT_OWNER_ID, &item("photo-ref-a"))
        .unwrap();
    store
        .reference_set_add_item(DEFAULT_OWNER_ID, &item("photo-ref-b"))
        .unwrap();
    // The target-existence trigger refuses items for missing media.
    let mut missing = item("photo-missing");
    missing.role = ReferenceItemRole::Excluded;
    assert!(store
        .reference_set_add_item(DEFAULT_OWNER_ID, &missing)
        .is_err());
    assert_eq!(
        store
            .reference_set_items(DEFAULT_OWNER_ID, "set-a")
            .unwrap(),
        vec![item("photo-ref-a"), item("photo-ref-b")]
    );
    assert!(store
        .reference_set_remove_item(DEFAULT_OWNER_ID, "set-a", MediaKind::Photo, "photo-ref-b")
        .unwrap());
    assert_eq!(
        store
            .reference_set_items(DEFAULT_OWNER_ID, "set-a")
            .unwrap()
            .len(),
        1
    );

    assert!(store
        .reference_set_confirm(DEFAULT_OWNER_ID, "set-a")
        .unwrap());
    let confirmed = store
        .reference_set_get(DEFAULT_OWNER_ID, "set-a")
        .unwrap()
        .unwrap();
    assert_eq!(confirmed.status, ReferenceSetStatus::Confirmed);
    assert!(confirmed.confirmed_at.is_some());
    assert!(store
        .reference_set_disable(DEFAULT_OWNER_ID, "set-a")
        .unwrap());
    assert_eq!(
        store
            .reference_set_get(DEFAULT_OWNER_ID, "set-a")
            .unwrap()
            .unwrap()
            .status,
        ReferenceSetStatus::Disabled
    );

    // Deleting the media cleans up dangling items through the 0007 triggers.
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute("DELETE FROM photos WHERE id = 'photo-ref-a'", [])
        .unwrap();
    drop(audit);
    assert!(store
        .reference_set_items(DEFAULT_OWNER_ID, "set-a")
        .unwrap()
        .is_empty());

    // Deleting the set cascades its remaining items; a retrain reproduces from the rest.
    store
        .reference_set_add_item(DEFAULT_OWNER_ID, &item("photo-ref-b"))
        .unwrap();
    assert!(store
        .reference_set_delete(DEFAULT_OWNER_ID, "set-a")
        .unwrap());
    assert!(store
        .reference_set_get(DEFAULT_OWNER_ID, "set-a")
        .unwrap()
        .is_none());
    assert!(store
        .reference_set_items(DEFAULT_OWNER_ID, "set-a")
        .unwrap()
        .is_empty());
    // Owner scoping: deleting another owner's set by id touches nothing.
    assert!(!store
        .reference_set_delete(DEFAULT_OWNER_ID, "set-b")
        .unwrap());
    assert!(store.reference_set_get(OWNER_B, "set-b").unwrap().is_some());
}

#[test]
fn confirmed_items_read_only_confirmed_sets_in_one_context() {
    let directory = TestDir::new("reference-confirmed-items");
    let mut store = Store::open(directory.path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-curated", "curated-sha"),
        )
        .unwrap();
    store
        .reference_set_create(
            DEFAULT_OWNER_ID,
            &reference_set(
                "set-context",
                "hero selects",
                ReferenceSetStatus::Unconfirmed,
            ),
        )
        .unwrap();
    store
        .reference_set_add_item(
            DEFAULT_OWNER_ID,
            &ReferenceSetItem {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                set_id: "set-context".to_owned(),
                media_kind: MediaKind::Photo,
                media_id: "photo-curated".to_owned(),
                role: ReferenceItemRole::Positive,
                added_at: now,
            },
        )
        .unwrap();
    // Uncurated sets contribute nothing until confirmed.
    assert!(store
        .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
        .unwrap()
        .is_empty());
    store
        .reference_set_confirm(DEFAULT_OWNER_ID, "set-context")
        .unwrap();
    assert_eq!(
        store
            .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
            .unwrap(),
        vec![(MediaKind::Photo, "photo-curated".to_owned())]
    );
    // Context scoping: another context sees nothing.
    assert!(store
        .reference_set_confirmed_items(DEFAULT_OWNER_ID, "homepage-hero")
        .unwrap()
        .is_empty());
    // Disabling mutes without deleting.
    store
        .reference_set_disable(DEFAULT_OWNER_ID, "set-context")
        .unwrap();
    assert!(store
        .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
        .unwrap()
        .is_empty());
}

#[test]
fn style_profile_versions_list_activate_and_reset_round_trip() {
    let directory = TestDir::new("profile-versions");
    let mut store = Store::open(directory.path()).unwrap();
    let first = style_profile("style-v1");
    store.put_style_profile(DEFAULT_OWNER_ID, &first).unwrap();
    let mut second = style_profile("style-v2");
    second.version = 2;
    store.put_style_profile(DEFAULT_OWNER_ID, &second).unwrap();
    // The prior active version is deactivated, never deleted.
    let versions = store.style_profiles(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(versions.len(), 2);
    assert!(!versions[0].active);
    assert!(versions[1].active);
    assert_eq!(
        store
            .style_profiles_for_context(DEFAULT_OWNER_ID, "default")
            .unwrap()
            .len(),
        2
    );

    // Reversible activation flips between retained versions.
    assert!(store
        .activate_style_profile(DEFAULT_OWNER_ID, "style-v1")
        .unwrap());
    assert_eq!(
        store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .map(|p| p.id),
        Some("style-v1".to_owned())
    );
    assert!(!store
        .activate_style_profile(DEFAULT_OWNER_ID, "style-missing")
        .unwrap());

    // Named contexts activate independently of the default context.
    let mut hero = style_profile("style-hero");
    hero.name = "homepage-hero".to_owned();
    hero.context_key = "homepage-hero".to_owned();
    store.put_style_profile(DEFAULT_OWNER_ID, &hero).unwrap();
    assert!(hero.active);
    assert_eq!(
        store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .map(|p| p.id),
        Some("style-v1".to_owned())
    );
    assert!(store
        .active_style_profile_for_context(DEFAULT_OWNER_ID, "homepage-hero")
        .unwrap()
        .is_some());

    // Reset deactivates everything and is reversible through activation or a retrain.
    assert_eq!(store.reset_style_profiles(DEFAULT_OWNER_ID).unwrap(), 2);
    assert!(store
        .active_style_profile(DEFAULT_OWNER_ID)
        .unwrap()
        .is_none());
    assert!(store
        .active_style_profile_for_context(DEFAULT_OWNER_ID, "homepage-hero")
        .unwrap()
        .is_none());
    assert_eq!(store.style_profiles(DEFAULT_OWNER_ID).unwrap().len(), 3);
    assert_eq!(store.reset_style_profiles(DEFAULT_OWNER_ID).unwrap(), 0);
}

fn collection(id: &str, name: &str) -> Collection {
    Collection {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: name.to_owned(),
        description: "editorial selects".to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn plan(id: &str, name: &str) -> Plan {
    Plan {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: name.to_owned(),
        description: "hero selects".to_owned(),
        context_key: "default".to_owned(),
        brief: "warm family film".to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn plan_item(plan_id: &str, media_kind: MediaKind, media_id: &str) -> PlanItem {
    PlanItem {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        plan_id: plan_id.to_owned(),
        media_kind,
        media_id: media_id.to_owned(),
        // plan_add_item assigns the next dense position.
        position: 0,
        start_s: (media_kind == MediaKind::Shot).then_some(0.0),
        end_s: (media_kind == MediaKind::Shot).then_some(1.0),
        pacing: None,
        crop_x: None,
        grade_json: None,
        reason: String::new(),
        signals_json: "{}".to_owned(),
        origin: PlanOrigin::General,
        rank: None,
        profile_version: None,
        added_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn collection_item(collection_id: &str, media_kind: MediaKind, media_id: &str) -> CollectionItem {
    CollectionItem {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        collection_id: collection_id.to_owned(),
        media_kind,
        media_id: media_id.to_owned(),
        context_key: None,
        marked: false,
        added_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn version_stack(id: &str, name: &str) -> VersionStack {
    VersionStack {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: name.to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn stack_item(
    stack_id: &str,
    media_kind: StackMediaKind,
    media_id: &str,
    role: StackItemRole,
) -> StackItem {
    StackItem {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        stack_id: stack_id.to_owned(),
        media_kind,
        media_id: media_id.to_owned(),
        role,
        added_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

fn saved_search(id: &str, name: &str) -> SavedSearch {
    SavedSearch {
        id: id.to_owned(),
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        name: name.to_owned(),
        query: "quiet hero frames".to_owned(),
        context_key: "default".to_owned(),
        filters_json: "{}".to_owned(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
    }
}

#[test]
fn collections_round_trip_with_owner_isolation_and_cascades() {
    const OWNER_B: &str = "editor-b";
    let directory = TestDir::new("collections");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('editor-b', 'Editor B', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-col-a", "col-sha-a"),
        )
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-col", "col-video-sha"))
        .unwrap();
    store
        .insert_shots(
            DEFAULT_OWNER_ID,
            std::slice::from_ref(&shot("shot-col", "video-col", 0)),
        )
        .unwrap();

    let set = collection("col-a", "hero selects");
    store.collection_create(DEFAULT_OWNER_ID, &set).unwrap();
    assert_eq!(
        store.collection_get(DEFAULT_OWNER_ID, "col-a").unwrap(),
        Some(set.clone())
    );
    // Owner isolation: the same name is allowed for another owner and neither sees the other.
    let mut other_owner = collection("col-b", "hero selects");
    other_owner.owner_id = OWNER_B.to_owned();
    store.collection_create(OWNER_B, &other_owner).unwrap();
    assert_eq!(store.collection_list(OWNER_B).unwrap().len(), 1);
    assert!(store.collection_get(OWNER_B, "col-a").unwrap().is_none());
    // Cross-owner items are refused before SQL even runs.
    let mut crossed = collection_item("col-a", MediaKind::Photo, "photo-col-a");
    crossed.owner_id = OWNER_B.to_owned();
    assert!(store.collection_add_item(OWNER_B, &crossed).is_err());

    store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-a", MediaKind::Photo, "photo-col-a"),
        )
        .unwrap();
    let mut shot_item = collection_item("col-a", MediaKind::Shot, "shot-col");
    shot_item.context_key = Some("homepage-hero".to_owned());
    store
        .collection_add_item(DEFAULT_OWNER_ID, &shot_item)
        .unwrap();
    // The target-existence trigger refuses items for missing media.
    assert!(store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-a", MediaKind::Photo, "photo-missing"),
        )
        .is_err());
    // Duplicate membership and blank context keys are refused.
    assert!(store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-a", MediaKind::Photo, "photo-col-a"),
        )
        .is_err());
    let mut blank = collection_item("col-a", MediaKind::Photo, "photo-col-a");
    blank.context_key = Some("   ".to_owned());
    assert!(store.collection_add_item(DEFAULT_OWNER_ID, &blank).is_err());

    let mut items = store
        .collection_items(DEFAULT_OWNER_ID, "col-a")
        .unwrap()
        .into_iter()
        .map(|item| (item.media_kind, item.media_id))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.1.cmp(&right.1));
    assert_eq!(
        items,
        vec![
            (MediaKind::Photo, "photo-col-a".to_owned()),
            (MediaKind::Shot, "shot-col".to_owned()),
        ]
    );

    // Marking round-trips and refuses unknown items.
    store
        .collection_set_item_marked(
            DEFAULT_OWNER_ID,
            "col-a",
            MediaKind::Photo,
            "photo-col-a",
            true,
        )
        .unwrap();
    let marked = store
        .collection_items(DEFAULT_OWNER_ID, "col-a")
        .unwrap()
        .into_iter()
        .find(|item| item.media_id == "photo-col-a")
        .unwrap();
    assert!(marked.marked);
    assert!(store
        .collection_set_item_marked(
            DEFAULT_OWNER_ID,
            "col-a",
            MediaKind::Photo,
            "photo-missing",
            true,
        )
        .is_err());

    // Renaming works; unique (owner, name) still applies between two of this owner's
    // collections but never across owners.
    assert!(store
        .collection_rename(DEFAULT_OWNER_ID, "col-a", "renamed selects")
        .unwrap());
    store
        .collection_create(DEFAULT_OWNER_ID, &collection("col-c", "second set"))
        .unwrap();
    assert!(store
        .collection_rename(DEFAULT_OWNER_ID, "col-c", "renamed selects")
        .is_err());
    assert!(!store
        .collection_rename(OWNER_B, "col-a", "stolen name")
        .unwrap());

    // Deleting media scrubs dangling items through the 0008 cleanup triggers.
    audit
        .execute("DELETE FROM shots WHERE id = 'shot-col'", [])
        .unwrap();
    let remaining = store
        .collection_items(DEFAULT_OWNER_ID, "col-a")
        .unwrap()
        .into_iter()
        .map(|item| item.media_id)
        .collect::<Vec<_>>();
    assert_eq!(remaining, vec!["photo-col-a".to_owned()]);

    // Deleting the collection cascades its items; a second delete reports false.
    assert!(store.collection_delete(DEFAULT_OWNER_ID, "col-a").unwrap());
    assert!(store
        .collection_get(DEFAULT_OWNER_ID, "col-a")
        .unwrap()
        .is_none());
    assert!(store
        .collection_items(DEFAULT_OWNER_ID, "col-a")
        .unwrap()
        .is_empty());
    assert!(!store.collection_delete(DEFAULT_OWNER_ID, "col-a").unwrap());
    // Owner scoping: deleting another owner's collection by id touches nothing.
    assert!(!store.collection_delete(DEFAULT_OWNER_ID, "col-b").unwrap());
    assert!(store.collection_get(OWNER_B, "col-b").unwrap().is_some());
}

#[test]
fn collection_designation_materializes_items_and_survives_collection_delete() {
    let directory = TestDir::new("collection-designation");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-des-a", "des-sha-a"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-des-b", "des-sha-b"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-des-c", "des-sha-c"),
        )
        .unwrap();

    let set = collection("col-des", "previous work");
    store.collection_create(DEFAULT_OWNER_ID, &set).unwrap();
    let mut marked = collection_item("col-des", MediaKind::Photo, "photo-des-a");
    marked.marked = true;
    store
        .collection_add_item(DEFAULT_OWNER_ID, &marked)
        .unwrap();
    store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-des", MediaKind::Photo, "photo-des-b"),
        )
        .unwrap();

    // WholeSet snapshots every current collection item.
    let whole = store
        .collection_designate_as_reference_set(
            DEFAULT_OWNER_ID,
            "col-des",
            "whole evidence",
            "default",
            ReferenceSetScope::WholeSet,
        )
        .unwrap();
    assert_eq!(whole.status, ReferenceSetStatus::Unconfirmed);
    assert_eq!(whole.scope, ReferenceSetScope::WholeSet);
    assert_eq!(whole.source_collection_id.as_deref(), Some("col-des"));
    assert_eq!(
        store
            .reference_set_items(DEFAULT_OWNER_ID, &whole.id)
            .unwrap()
            .len(),
        2
    );
    // Uncurated designation contributes nothing until the explicit confirm.
    assert!(store
        .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
        .unwrap()
        .is_empty());
    store
        .reference_set_confirm(DEFAULT_OWNER_ID, &whole.id)
        .unwrap();
    assert_eq!(
        store
            .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
            .unwrap()
            .len(),
        2
    );
    // Later collection edits never rewrite the materialized evidence.
    store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-des", MediaKind::Photo, "photo-des-c"),
        )
        .unwrap();
    assert_eq!(
        store
            .reference_set_items(DEFAULT_OWNER_ID, &whole.id)
            .unwrap()
            .len(),
        2
    );

    // Selected snapshots only the marked rows.
    let selected = store
        .collection_designate_as_reference_set(
            DEFAULT_OWNER_ID,
            "col-des",
            "selected evidence",
            "default",
            ReferenceSetScope::Selected,
        )
        .unwrap();
    let selected_items = store
        .reference_set_items(DEFAULT_OWNER_ID, &selected.id)
        .unwrap();
    assert_eq!(selected_items.len(), 1);
    assert_eq!(selected_items[0].media_id, "photo-des-a");
    // Re-designation creates a *new* set; UNIQUE(owner_id, name) still applies.
    assert!(store
        .collection_designate_as_reference_set(
            DEFAULT_OWNER_ID,
            "col-des",
            "selected evidence",
            "default",
            ReferenceSetScope::Selected,
        )
        .is_err());

    // Deleting the collection unsets the designation but keeps the confirmed set and its
    // items: evidence survives and is reproducible from the remaining sets.
    assert!(store
        .collection_delete(DEFAULT_OWNER_ID, "col-des")
        .unwrap());
    let survivor = store
        .reference_set_get(DEFAULT_OWNER_ID, &whole.id)
        .unwrap()
        .unwrap();
    assert_eq!(survivor.source_collection_id, None);
    assert_eq!(
        store
            .reference_set_items(DEFAULT_OWNER_ID, &whole.id)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        store
            .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn version_stacks_enforce_one_original_with_cascades() {
    const OWNER_B: &str = "editor-b";
    let directory = TestDir::new("version-stacks");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('editor-b', 'Editor B', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-stack-a", "stack-sha-a"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-stack-b", "stack-sha-b"),
        )
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-stack", "stack-video-sha"))
        .unwrap();

    let stack = version_stack("stack-a", "hero photo versions");
    store.stack_create(DEFAULT_OWNER_ID, &stack).unwrap();
    assert_eq!(
        store.stack_get(DEFAULT_OWNER_ID, "stack-a").unwrap(),
        Some(stack.clone())
    );
    // Owner isolation: same name for another owner, ids never cross owners.
    let mut other_owner = version_stack("stack-b", "hero photo versions");
    other_owner.owner_id = OWNER_B.to_owned();
    store.stack_create(OWNER_B, &other_owner).unwrap();
    assert!(store.stack_get(OWNER_B, "stack-a").unwrap().is_none());
    let mut crossed = stack_item(
        "stack-a",
        StackMediaKind::Photo,
        "photo-stack-a",
        StackItemRole::Original,
    );
    crossed.owner_id = OWNER_B.to_owned();
    assert!(store.stack_add_item(OWNER_B, &crossed).is_err());

    store
        .stack_add_item(
            DEFAULT_OWNER_ID,
            &stack_item(
                "stack-a",
                StackMediaKind::Photo,
                "photo-stack-a",
                StackItemRole::Original,
            ),
        )
        .unwrap();
    store
        .stack_add_item(
            DEFAULT_OWNER_ID,
            &stack_item(
                "stack-a",
                StackMediaKind::Photo,
                "photo-stack-b",
                StackItemRole::Derived,
            ),
        )
        .unwrap();
    // Exactly one original per stack: a second original (even a video) is refused both by the
    // API and by the partial unique index.
    let mut second_original = stack_item(
        "stack-a",
        StackMediaKind::Video,
        "video-stack",
        StackItemRole::Original,
    );
    assert!(store
        .stack_add_item(DEFAULT_OWNER_ID, &second_original)
        .is_err());
    second_original.role = StackItemRole::Derived;
    store
        .stack_add_item(DEFAULT_OWNER_ID, &second_original)
        .unwrap();
    // Unknown media hits the target-existence trigger.
    let missing = stack_item(
        "stack-a",
        StackMediaKind::Photo,
        "photo-missing",
        StackItemRole::Derived,
    );
    assert!(store.stack_add_item(DEFAULT_OWNER_ID, &missing).is_err());

    assert_eq!(
        store
            .stack_items(DEFAULT_OWNER_ID, "stack-a")
            .unwrap()
            .len(),
        3
    );
    let stacks_for_photo = store
        .stacks_for_asset(DEFAULT_OWNER_ID, StackMediaKind::Photo, "photo-stack-a")
        .unwrap();
    assert_eq!(stacks_for_photo.len(), 1);
    assert_eq!(stacks_for_photo[0].id, "stack-a");

    // Deleting media scrubs dangling stack items through the 0008 cleanup triggers.
    audit
        .execute("DELETE FROM photos WHERE id = 'photo-stack-b'", [])
        .unwrap();
    assert_eq!(
        store
            .stack_items(DEFAULT_OWNER_ID, "stack-a")
            .unwrap()
            .len(),
        2
    );

    // Deleting the stack cascades its items and leaves media untouched.
    assert!(store.stack_delete(DEFAULT_OWNER_ID, "stack-a").unwrap());
    assert!(store
        .stack_get(DEFAULT_OWNER_ID, "stack-a")
        .unwrap()
        .is_none());
    assert!(store
        .stack_items(DEFAULT_OWNER_ID, "stack-a")
        .unwrap()
        .is_empty());
    assert!(store
        .photo_by_id(DEFAULT_OWNER_ID, "photo-stack-a")
        .unwrap()
        .is_some());
    assert!(!store.stack_delete(DEFAULT_OWNER_ID, "stack-b").unwrap());
}

#[test]
fn saved_searches_round_trip_and_validate_filters() {
    const OWNER_B: &str = "editor-b";
    let directory = TestDir::new("saved-searches");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('editor-b', 'Editor B', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();

    let mut search = saved_search("search-a", "quiet hero frames");
    search.context_key = "homepage-hero".to_owned();
    search.filters_json = r#"{"usable": false}"#.to_owned();
    store
        .saved_search_create(DEFAULT_OWNER_ID, &search)
        .unwrap();
    assert_eq!(
        store.saved_search_list(DEFAULT_OWNER_ID).unwrap(),
        vec![search.clone()]
    );
    // Name uniqueness per owner; other owners may reuse the name.
    let mut duplicate = saved_search("search-b", "quiet hero frames");
    duplicate.id = "search-b".to_owned();
    assert!(store
        .saved_search_create(DEFAULT_OWNER_ID, &duplicate)
        .is_err());
    let mut other_owner = saved_search("search-c", "quiet hero frames");
    other_owner.owner_id = OWNER_B.to_owned();
    store.saved_search_create(OWNER_B, &other_owner).unwrap();
    assert_eq!(store.saved_search_list(OWNER_B).unwrap().len(), 1);

    // filters_json must be a JSON object.
    let mut array_filters = saved_search("search-d", "array filters");
    array_filters.filters_json = "[1, 2]".to_owned();
    assert!(store
        .saved_search_create(DEFAULT_OWNER_ID, &array_filters)
        .is_err());
    let mut garbage_filters = saved_search("search-e", "garbage filters");
    garbage_filters.filters_json = "not json".to_owned();
    assert!(store
        .saved_search_create(DEFAULT_OWNER_ID, &garbage_filters)
        .is_err());

    assert!(store
        .saved_search_delete(DEFAULT_OWNER_ID, "search-a")
        .unwrap());
    assert!(store
        .saved_search_list(DEFAULT_OWNER_ID)
        .unwrap()
        .is_empty());
    assert!(!store
        .saved_search_delete(DEFAULT_OWNER_ID, "search-a")
        .unwrap());
    // Owner scoping: another owner's saved search is untouched.
    assert!(!store
        .saved_search_delete(DEFAULT_OWNER_ID, "search-c")
        .unwrap());
}

#[test]
fn safety_flags_write_path_is_state_only_and_never_appends_feedback() {
    let directory = TestDir::new("safety-flags");
    let store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-flag-a", "flag-sha-a"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-flag-b", "flag-sha-b"),
        )
        .unwrap();
    // Pre-existing editable state must survive a flag write untouched.
    store
        .upsert_editorial_annotation(
            DEFAULT_OWNER_ID,
            &EditorialAnnotation {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                media_kind: MediaKind::Photo,
                media_id: "photo-flag-a".to_owned(),
                description: "quiet portrait".to_owned(),
                subjects: String::new(),
                action: String::new(),
                tags: "portrait".to_owned(),
                quality: Some(4),
                standout: true,
                usable: true,
                faces_visible: false,
                nametags_visible: false,
                blur_required: false,
                crop_x: None,
                grade_json: None,
                notes: "keep".to_owned(),
                updated_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
            },
        )
        .unwrap();

    let flagged = store
        .set_safety_flags(
            DEFAULT_OWNER_ID,
            MediaKind::Photo,
            "photo-flag-a",
            SafetyFlags {
                usable: false,
                faces_visible: true,
                nametags_visible: false,
                blur_required: true,
            },
        )
        .unwrap();
    assert!(!flagged.usable);
    assert!(flagged.blur_required);
    assert_eq!(flagged.description, "quiet portrait");
    assert_eq!(flagged.tags, "portrait");
    assert_eq!(flagged.quality, Some(4));
    assert!(flagged.standout);
    // The dedicated write path is state-only: no feedback event may appear.
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());

    // Flags clear again through the same explicit path.
    let cleared = store
        .set_safety_flags(
            DEFAULT_OWNER_ID,
            MediaKind::Photo,
            "photo-flag-a",
            SafetyFlags::default(),
        )
        .unwrap();
    assert!(cleared.usable);
    assert!(!cleared.blur_required);
    assert!(!cleared.faces_visible);

    // An asset without an annotation gets the 0002 defaults plus the requested flags.
    let fresh = store
        .set_safety_flags(
            DEFAULT_OWNER_ID,
            MediaKind::Photo,
            "photo-flag-b",
            SafetyFlags {
                usable: false,
                faces_visible: false,
                nametags_visible: false,
                blur_required: true,
            },
        )
        .unwrap();
    assert!(!fresh.usable);
    assert!(fresh.blur_required);
    assert_eq!(fresh.description, "");

    // Unknown media is refused by the target-existence trigger.
    assert!(store
        .set_safety_flags(
            DEFAULT_OWNER_ID,
            MediaKind::Photo,
            "photo-missing",
            SafetyFlags::default(),
        )
        .is_err());
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());
}

#[test]
fn bulk_review_appends_events_and_updates_annotations_atomically() {
    let directory = TestDir::new("bulk-review");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-review-a", "review-sha-a"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-review-b", "review-sha-b"),
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-review-c", "review-sha-c"),
        )
        .unwrap();
    store
        .collection_create(DEFAULT_OWNER_ID, &collection("col-review", "review batch"))
        .unwrap();

    let applied = store
        .bulk_review(
            DEFAULT_OWNER_ID,
            &[
                ReviewOp::Pick {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-a".to_owned(),
                },
                ReviewOp::Reject {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-b".to_owned(),
                },
                ReviewOp::Rate {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-c".to_owned(),
                    rating: 4,
                },
                ReviewOp::SetFlags {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-a".to_owned(),
                    flags: SafetyFlags {
                        usable: false,
                        faces_visible: false,
                        nametags_visible: false,
                        blur_required: true,
                    },
                },
                ReviewOp::AddToCollection {
                    collection_id: "col-review".to_owned(),
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-a".to_owned(),
                    context_key: Some("homepage-hero".to_owned()),
                },
            ],
        )
        .unwrap();
    assert_eq!(applied, 5);

    let events = store.feedback_events(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(events.len(), 3, "flag and collection ops are state-only");
    assert_eq!(events[0].signal, FeedbackSignal::Pick);
    assert_eq!(events[0].value, Some(1.0));
    assert_eq!(events[1].signal, FeedbackSignal::Reject);
    assert_eq!(events[1].value, Some(-1.0));
    assert_eq!(events[2].signal, FeedbackSignal::Rating);
    assert_eq!(events[2].value, Some(4.0));
    let annotation_c = store
        .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-review-c")
        .unwrap()
        .unwrap();
    assert_eq!(annotation_c.quality, Some(4));
    let annotation_a = store
        .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-review-a")
        .unwrap()
        .unwrap();
    assert!(!annotation_a.usable);
    assert!(annotation_a.blur_required);
    let items = store
        .collection_items(DEFAULT_OWNER_ID, "col-review")
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].media_id, "photo-review-a");
    assert_eq!(items[0].context_key.as_deref(), Some("homepage-hero"));

    // One bad op aborts the whole batch: nothing is appended and no state changes.
    assert!(store
        .bulk_review(
            DEFAULT_OWNER_ID,
            &[
                ReviewOp::Pick {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-b".to_owned(),
                },
                ReviewOp::Rate {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-b".to_owned(),
                    rating: 9,
                },
            ],
        )
        .is_err());
    assert!(store
        .bulk_review(
            DEFAULT_OWNER_ID,
            &[
                ReviewOp::Pick {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-review-b".to_owned(),
                },
                ReviewOp::Pick {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-missing".to_owned(),
                },
            ],
        )
        .is_err());
    assert!(store
        .bulk_review(
            DEFAULT_OWNER_ID,
            &[ReviewOp::AddToCollection {
                collection_id: "col-missing".to_owned(),
                media_kind: MediaKind::Photo,
                media_id: "photo-review-b".to_owned(),
                context_key: None,
            }],
        )
        .is_err());
    assert_eq!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(), 3);
    assert!(store
        .collection_items(DEFAULT_OWNER_ID, "col-review")
        .unwrap()
        .iter()
        .all(|item| item.media_id == "photo-review-a"));
    let annotation_b = store
        .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "photo-review-b")
        .unwrap();
    assert!(
        annotation_b.is_none(),
        "the aborted batch must not leave annotation writes behind"
    );
}

#[test]
fn browse_assets_filters_and_counts() {
    let directory = TestDir::new("browse-assets");
    let mut store = Store::open(directory.path()).unwrap();
    let mut photo_flagged = reference_photo("photo-browse-a", "browse-sha-a");
    photo_flagged.status = PhotoStatus::Done;
    store
        .upsert_photo(DEFAULT_OWNER_ID, &photo_flagged)
        .unwrap();
    let mut photo_pending = reference_photo("photo-browse-b", "browse-sha-b");
    photo_pending.status = PhotoStatus::Pending;
    store
        .upsert_photo(DEFAULT_OWNER_ID, &photo_pending)
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-browse", "browse-video-sha"))
        .unwrap();
    store
        .insert_shots(
            DEFAULT_OWNER_ID,
            std::slice::from_ref(&shot("shot-browse", "video-browse", 0)),
        )
        .unwrap();
    store
        .upsert_editorial_annotation(
            DEFAULT_OWNER_ID,
            &EditorialAnnotation {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                media_kind: MediaKind::Photo,
                media_id: "photo-browse-a".to_owned(),
                description: String::new(),
                subjects: String::new(),
                action: String::new(),
                tags: "hero".to_owned(),
                quality: Some(2),
                standout: false,
                usable: false,
                faces_visible: false,
                nametags_visible: false,
                blur_required: true,
                crop_x: None,
                grade_json: None,
                notes: String::new(),
                updated_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
            },
        )
        .unwrap();
    store
        .upsert_editorial_annotation(
            DEFAULT_OWNER_ID,
            &EditorialAnnotation {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                media_kind: MediaKind::Shot,
                media_id: "shot-browse".to_owned(),
                description: String::new(),
                subjects: String::new(),
                action: String::new(),
                tags: String::new(),
                quality: Some(5),
                standout: true,
                usable: true,
                faces_visible: false,
                nametags_visible: false,
                blur_required: false,
                crop_x: None,
                grade_json: None,
                notes: String::new(),
                updated_at: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
            },
        )
        .unwrap();
    store
        .collection_create(DEFAULT_OWNER_ID, &collection("col-browse", "browse set"))
        .unwrap();
    let mut contextual = collection_item("col-browse", MediaKind::Photo, "photo-browse-a");
    contextual.context_key = Some("homepage-hero".to_owned());
    store
        .collection_add_item(DEFAULT_OWNER_ID, &contextual)
        .unwrap();
    store
        .stack_create(
            DEFAULT_OWNER_ID,
            &version_stack("stack-browse", "browse versions"),
        )
        .unwrap();
    store
        .stack_add_item(
            DEFAULT_OWNER_ID,
            &stack_item(
                "stack-browse",
                StackMediaKind::Photo,
                "photo-browse-b",
                StackItemRole::Original,
            ),
        )
        .unwrap();

    let ids = |assets: &[crush_store::LibraryAsset]| -> Vec<String> {
        assets.iter().map(|asset| asset.media_id.clone()).collect()
    };
    let all = store
        .browse_assets(DEFAULT_OWNER_ID, &AssetFilter::default())
        .unwrap();
    assert_eq!(
        ids(&all),
        vec![
            "photo-browse-a".to_owned(),
            "photo-browse-b".to_owned(),
            "shot-browse".to_owned(),
        ],
        "photos sort before shots when no timestamps exist"
    );
    let shot_row = all
        .iter()
        .find(|asset| asset.media_id == "shot-browse")
        .unwrap();
    assert_eq!(shot_row.video_id.as_deref(), Some("video-browse"));
    assert_eq!(shot_row.path, "/footage/video-browse.mov");
    assert_eq!(shot_row.quality, Some(5));
    let photo_b = all
        .iter()
        .find(|asset| asset.media_id == "photo-browse-b")
        .unwrap();
    assert_eq!(photo_b.stack_ids, vec!["stack-browse".to_owned()]);
    let photo_a = all
        .iter()
        .find(|asset| asset.media_id == "photo-browse-a")
        .unwrap();
    assert_eq!(photo_a.collection_ids, vec!["col-browse".to_owned()]);
    assert_eq!(photo_a.status, "done");

    let set_filter = |mutate: &dyn Fn(&mut AssetFilter)| {
        let mut filter = AssetFilter::default();
        mutate(&mut filter);
        ids(&store.browse_assets(DEFAULT_OWNER_ID, &filter).unwrap())
    };
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.kind = Some(MediaKind::Shot)),
        vec!["shot-browse".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.kind = Some(MediaKind::Photo)).len(),
        2
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.usable = Some(false)),
        vec!["photo-browse-a".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.blur_required = Some(true)),
        vec!["photo-browse-a".to_owned()]
    );
    assert!(set_filter(&|f: &mut AssetFilter| f.faces_visible = Some(true)).is_empty());
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.quality_min = Some(3)),
        vec!["shot-browse".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.collection_id = Some("col-browse".to_owned())),
        vec!["photo-browse-a".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.stack_id = Some("stack-browse".to_owned())),
        vec!["photo-browse-b".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.context_key = Some("homepage-hero".to_owned())),
        vec!["photo-browse-a".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.search = Some("video-browse".to_owned())),
        vec!["shot-browse".to_owned()]
    );
    assert_eq!(
        set_filter(&|f: &mut AssetFilter| f.status = Some("done".to_owned())),
        vec!["photo-browse-a".to_owned()]
    );

    let counts = store.library_counts(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(counts.photos, 2);
    assert_eq!(counts.shots, 1);
    assert_eq!(counts.picks, 0);
    assert_eq!(counts.rejects, 0);
    assert_eq!(counts.flagged, 1);
}

#[test]
fn schema_v7_upgrades_to_collections_without_losing_rows() {
    let directory = TestDir::new("migration-v7-v8");
    let db = directory.path().join("library.db");
    std::fs::create_dir_all(directory.path()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
             ) STRICT;
             INSERT INTO schema_version VALUES (1, 0);",
        )
        .unwrap();
    for (version, migration) in [
        (1, include_str!("../migrations/0001_init.sql")),
        (2, include_str!("../migrations/0002_dam_feedback.sql")),
        (3, include_str!("../migrations/0003_source_fidelity.sql")),
        (4, include_str!("../migrations/0004_strong_shot.sql")),
        (5, include_str!("../migrations/0005_feedback_hardening.sql")),
        (6, include_str!("../migrations/0006_photo_jobs.sql")),
        (7, include_str!("../migrations/0007_reference_sets.sql")),
    ] {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status
             ) VALUES ('legacy-video', 'local', '/legacy.mov', 'legacy-sha', 1.0, 24.0,
                       1920, 1080, 1, 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO shots (
                id, video_id, owner_id, idx, start_s, end_s, rep_frame_s
             ) VALUES ('legacy-shot', 'legacy-video', 'local', 0, 0.0, 1.0, 0.5)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photos (
                id, owner_id, path, sha256, width, height, format, status
             ) VALUES ('legacy-photo', 'local', '/legacy.jpg', 'legacy-photo-sha',
                       100, 100, 'jpeg', 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO editorial_annotations (
                owner_id, media_kind, media_id, quality, updated_at
             ) VALUES ('local', 'photo', 'legacy-photo', 4, '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO feedback_events (
                id, owner_id, media_kind, media_id, signal, value, created_at
             ) VALUES ('legacy-event', 'local', 'photo', 'legacy-photo', 'pick', 1.0,
                       '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO reference_sets (
                id, owner_id, name, context_key, description, scope, status, created_at
             ) VALUES ('legacy-set', 'local', 'previous work', 'default', '',
                       'whole_set', 'confirmed', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO reference_set_items (
                owner_id, set_id, media_kind, media_id, role, added_at
             ) VALUES ('local', 'legacy-set', 'photo', 'legacy-photo', 'positive',
                       '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);
    assert_eq!(store.videos(DEFAULT_OWNER_ID).unwrap().len(), 1);
    assert_eq!(
        store
            .shots_for_video(DEFAULT_OWNER_ID, "legacy-video")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, "legacy-photo")
            .unwrap()
            .unwrap()
            .quality,
        Some(4)
    );
    assert_eq!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(), 1);
    assert_eq!(
        store
            .reference_set_confirmed_items(DEFAULT_OWNER_ID, "default")
            .unwrap(),
        vec![(MediaKind::Photo, "legacy-photo".to_owned())]
    );
    // The v8 organization surfaces are live on the upgraded database.
    store
        .collection_create(DEFAULT_OWNER_ID, &collection("col-upgrade", "upgraded"))
        .unwrap();
    store
        .collection_add_item(
            DEFAULT_OWNER_ID,
            &collection_item("col-upgrade", MediaKind::Photo, "legacy-photo"),
        )
        .unwrap();
    assert_eq!(
        store
            .collection_items(DEFAULT_OWNER_ID, "col-upgrade")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn plans_round_trip_with_owner_isolation_and_cascades() {
    const OWNER_B: &str = "planner-b";
    let directory = TestDir::new("plans");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at)
             VALUES ('planner-b', 'Planner B', '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-plan-a", "plan-sha-a"),
        )
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-plan", "plan-video-sha"))
        .unwrap();
    store
        .insert_shots(
            DEFAULT_OWNER_ID,
            std::slice::from_ref(&shot("shot-plan", "video-plan", 0)),
        )
        .unwrap();

    let hero = plan("plan-a", "hero reel");
    store.plan_create(DEFAULT_OWNER_ID, &hero).unwrap();
    assert_eq!(
        store.plan_get(DEFAULT_OWNER_ID, "plan-a").unwrap(),
        Some(hero.clone())
    );
    // Owner isolation: another owner neither sees the plan nor may add items to it.
    let mut other_owner = plan("plan-b", "hero reel");
    other_owner.owner_id = OWNER_B.to_owned();
    store.plan_create(OWNER_B, &other_owner).unwrap();
    assert_eq!(store.plan_list(OWNER_B).unwrap().len(), 1);
    assert!(store.plan_get(OWNER_B, "plan-a").unwrap().is_none());
    let mut crossed = plan_item("plan-a", MediaKind::Photo, "photo-plan-a");
    crossed.owner_id = OWNER_B.to_owned();
    assert!(store.plan_add_item(OWNER_B, &crossed).is_err());
    // Duplicate names stay per-owner.
    assert!(store
        .plan_create(DEFAULT_OWNER_ID, &plan("plan-c", "hero reel"))
        .is_err());
    // Plans never append feedback events.
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());

    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-a", MediaKind::Photo, "photo-plan-a"),
        )
        .unwrap();
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-a", MediaKind::Shot, "shot-plan"),
        )
        .unwrap();
    // Duplicate membership is refused by the primary key.
    assert!(store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-a", MediaKind::Photo, "photo-plan-a"),
        )
        .is_err());
    // Missing media is refused by the target-existence trigger.
    assert!(store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-a", MediaKind::Photo, "photo-missing"),
        )
        .is_err());
    // Items are appended in dense positions.
    let items = store.plan_items(DEFAULT_OWNER_ID, "plan-a").unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| (item.position, item.media_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "photo-plan-a"), (1, "shot-plan")]
    );

    // Header updates round-trip and touch updated_at.
    let before = store.plan_get(DEFAULT_OWNER_ID, "plan-a").unwrap().unwrap();
    assert!(store
        .plan_update(
            DEFAULT_OWNER_ID,
            "plan-a",
            "renamed reel",
            "new",
            "quiet film"
        )
        .unwrap());
    let after = store.plan_get(DEFAULT_OWNER_ID, "plan-a").unwrap().unwrap();
    assert_eq!(after.name, "renamed reel");
    assert_eq!(after.brief, "quiet film");
    assert!(after.updated_at > before.updated_at);
    // Cross-owner renames and deletes touch nothing.
    assert!(!store
        .plan_update(OWNER_B, "plan-a", "stolen", "", "")
        .unwrap());
    assert!(!store.plan_delete(OWNER_B, "plan-a").unwrap());
    assert!(store
        .plan_get(DEFAULT_OWNER_ID, "plan-a")
        .unwrap()
        .is_some());

    // Duplicating copies header and items in order, under a new id.
    let copy = store
        .plan_duplicate(DEFAULT_OWNER_ID, "plan-a", "hero reel copy")
        .unwrap();
    assert_ne!(copy.id, "plan-a");
    let copied_items = store.plan_items(DEFAULT_OWNER_ID, &copy.id).unwrap();
    assert_eq!(copied_items.len(), items.len());
    assert_eq!(copied_items[1].media_id, "shot-plan");

    // Deleting media scrubs dangling plan items through the 0009 cleanup trigger.
    audit
        .execute("DELETE FROM shots WHERE id = 'shot-plan'", [])
        .unwrap();
    assert!(store
        .plan_items(DEFAULT_OWNER_ID, "plan-a")
        .unwrap()
        .iter()
        .all(|item| item.media_kind == MediaKind::Photo));

    // Deleting a plan cascades its items and revisions; a second delete reports false.
    store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-a", "checkpoint")
        .unwrap();
    assert!(store.plan_delete(DEFAULT_OWNER_ID, "plan-a").unwrap());
    assert!(store
        .plan_get(DEFAULT_OWNER_ID, "plan-a")
        .unwrap()
        .is_none());
    assert!(store
        .plan_items(DEFAULT_OWNER_ID, "plan-a")
        .unwrap()
        .is_empty());
    assert!(store
        .plan_revisions(DEFAULT_OWNER_ID, "plan-a")
        .unwrap()
        .is_empty());
    assert!(!store.plan_delete(DEFAULT_OWNER_ID, "plan-a").unwrap());
}

#[test]
fn plan_items_validate_shot_boundaries_and_reorder() {
    let directory = TestDir::new("plan-boundaries");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &reference_photo("photo-bound", "bound-sha"),
        )
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-bound", "bound-video-sha"))
        .unwrap();
    let shots = vec![
        shot("shot-bound-0", "video-bound", 0),
        shot("shot-bound-1", "video-bound", 1),
    ];
    store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
    store
        .plan_create(DEFAULT_OWNER_ID, &plan("plan-bound", "boundaries"))
        .unwrap();

    // A shot item clipped inside the source shot's interval is accepted.
    let mut clipped = plan_item("plan-bound", MediaKind::Shot, "shot-bound-1");
    clipped.start_s = Some(1.0);
    clipped.end_s = Some(2.0);
    store.plan_add_item(DEFAULT_OWNER_ID, &clipped).unwrap();

    // Out-of-bounds or degenerate clip boundaries are refused before SQL runs.
    let mut early = plan_item("plan-bound", MediaKind::Shot, "shot-bound-1");
    early.start_s = Some(0.5);
    early.end_s = Some(2.0);
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &early).is_err());
    let mut late = plan_item("plan-bound", MediaKind::Shot, "shot-bound-1");
    late.start_s = Some(1.0);
    late.end_s = Some(2.5);
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &late).is_err());
    let mut degenerate = plan_item("plan-bound", MediaKind::Shot, "shot-bound-1");
    degenerate.start_s = Some(1.5);
    degenerate.end_s = Some(1.5);
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &degenerate).is_err());
    // Shots require boundaries; photos must not carry any.
    let mut no_bounds = plan_item("plan-bound", MediaKind::Shot, "shot-bound-0");
    no_bounds.start_s = None;
    no_bounds.end_s = None;
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &no_bounds).is_err());
    // A photo item carrying clip boundaries violates the photos-carry-none rule.
    let mut photo_with_bounds = plan_item("plan-bound", MediaKind::Photo, "photo-bound");
    photo_with_bounds.start_s = Some(0.0);
    photo_with_bounds.end_s = Some(1.0);
    assert!(store
        .plan_add_item(DEFAULT_OWNER_ID, &photo_with_bounds)
        .is_err());
    // A plain photo item (no boundaries) is accepted.
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-bound", MediaKind::Photo, "photo-bound"),
        )
        .unwrap();

    // In-bounds edits are accepted and persisted; out-of-bounds edits are refused.
    store
        .plan_update_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Shot,
            "shot-bound-1",
            &PlanItemPatch {
                start_s: Some(1.25),
                end_s: Some(1.75),
                pacing: Some(0.4),
                crop_x: Some(0.5),
                grade_json: Some(r#"{"lift":[0,0,0.02]}"#.to_owned()),
                reason: Some("tighter on the gesture".to_owned()),
            },
        )
        .unwrap();
    let edited = &store.plan_items(DEFAULT_OWNER_ID, "plan-bound").unwrap()[0];
    assert_eq!(edited.start_s, Some(1.25));
    assert_eq!(edited.end_s, Some(1.75));
    assert_eq!(edited.pacing, Some(0.4));
    assert_eq!(edited.reason, "tighter on the gesture");
    assert!(store
        .plan_update_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Shot,
            "shot-bound-1",
            &PlanItemPatch {
                start_s: Some(0.25),
                ..PlanItemPatch::default()
            },
        )
        .is_err());
    // Photo items refuse treatment fields that imply clip boundaries.
    assert!(store
        .plan_update_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Photo,
            "photo-bound",
            &PlanItemPatch {
                start_s: Some(0.0),
                end_s: Some(1.0),
                ..PlanItemPatch::default()
            },
        )
        .is_err());
    // Unknown items are refused.
    assert!(store
        .plan_update_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Shot,
            "shot-missing",
            &PlanItemPatch::default(),
        )
        .is_err());

    // Reordering reassigns dense positions and accepts only full permutations.
    store
        .plan_reorder_items(
            DEFAULT_OWNER_ID,
            "plan-bound",
            &[
                (MediaKind::Shot, "shot-bound-1".to_owned()),
                (MediaKind::Photo, "photo-bound".to_owned()),
            ],
        )
        .unwrap();
    let ordered = store.plan_items(DEFAULT_OWNER_ID, "plan-bound").unwrap();
    assert_eq!(ordered[0].media_id, "shot-bound-1");
    assert_eq!(ordered[1].media_id, "photo-bound");
    assert!(store
        .plan_reorder_items(
            DEFAULT_OWNER_ID,
            "plan-bound",
            &[(MediaKind::Photo, "photo-bound".to_owned())],
        )
        .is_err());
    assert!(store
        .plan_reorder_items(
            DEFAULT_OWNER_ID,
            "plan-bound",
            &[
                (MediaKind::Photo, "photo-bound".to_owned()),
                (MediaKind::Photo, "photo-bound".to_owned()),
            ],
        )
        .is_err());
    assert!(store
        .plan_reorder_items(
            DEFAULT_OWNER_ID,
            "plan-bound",
            &[
                (MediaKind::Photo, "photo-bound".to_owned()),
                (MediaKind::Photo, "photo-missing".to_owned()),
            ],
        )
        .is_err());

    // Removing an item compacts the remaining positions.
    assert!(store
        .plan_remove_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Shot,
            "shot-bound-1"
        )
        .unwrap());
    let remaining = store.plan_items(DEFAULT_OWNER_ID, "plan-bound").unwrap();
    assert_eq!(
        remaining
            .iter()
            .map(|item| (item.position, item.media_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "photo-bound")]
    );
    assert!(!store
        .plan_remove_item(
            DEFAULT_OWNER_ID,
            "plan-bound",
            MediaKind::Shot,
            "shot-bound-1"
        )
        .unwrap());
}

#[test]
fn plan_revisions_are_append_only_and_restore_revalidates() {
    let directory = TestDir::new("plan-revisions");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    store
        .upsert_photo(DEFAULT_OWNER_ID, &reference_photo("photo-rev", "rev-sha"))
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-rev", "rev-video-sha"))
        .unwrap();
    store
        .insert_shots(
            DEFAULT_OWNER_ID,
            std::slice::from_ref(&shot("shot-rev", "video-rev", 0)),
        )
        .unwrap();
    store
        .plan_create(DEFAULT_OWNER_ID, &plan("plan-rev", "revisions"))
        .unwrap();
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-rev", MediaKind::Photo, "photo-rev"),
        )
        .unwrap();
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-rev", MediaKind::Shot, "shot-rev"),
        )
        .unwrap();

    let first = store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-rev", "v1")
        .unwrap();
    assert_eq!(first.revision, 1);

    // Mutate: swap the order, then drop the photo item entirely.
    store
        .plan_reorder_items(
            DEFAULT_OWNER_ID,
            "plan-rev",
            &[
                (MediaKind::Shot, "shot-rev".to_owned()),
                (MediaKind::Photo, "photo-rev".to_owned()),
            ],
        )
        .unwrap();
    store
        .plan_remove_item(DEFAULT_OWNER_ID, "plan-rev", MediaKind::Photo, "photo-rev")
        .unwrap();

    let second = store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-rev", "v2")
        .unwrap();
    assert_eq!(second.revision, 2);
    let revisions = store.plan_revisions(DEFAULT_OWNER_ID, "plan-rev").unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].label, "v1");

    // Snapshots are append-only at the schema level while the plan exists.
    assert!(audit
        .execute(
            "UPDATE plan_revisions SET label = 'rewritten' WHERE revision = 1",
            [],
        )
        .is_err());
    assert!(audit
        .execute("DELETE FROM plan_revisions WHERE revision = 1", [])
        .is_err());

    // Restore v1: both items come back in their original order.
    let restored_count = store
        .plan_restore_revision(DEFAULT_OWNER_ID, "plan-rev", 1)
        .unwrap();
    assert_eq!(restored_count, 2);
    let restored = store.plan_items(DEFAULT_OWNER_ID, "plan-rev").unwrap();
    assert_eq!(restored[0].media_id, "photo-rev");
    assert_eq!(restored[1].media_id, "shot-rev");
    // Restoring preserves provenance and frozen signals.
    assert_eq!(restored[0].position, 0);
    assert!(restored
        .iter()
        .all(|item| item.origin == PlanOrigin::General));
    // Unknown revisions are refused.
    assert!(store
        .plan_restore_revision(DEFAULT_OWNER_ID, "plan-rev", 99)
        .is_err());

    // Restore revalidates against current media: shrink the source shot, then restoring the
    // snapshot that still clips the old full interval must fail loudly.
    store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-rev", "v3")
        .unwrap();
    audit
        .execute(
            "UPDATE shots SET start_s = 0.5, end_s = 0.9, rep_frame_s = 0.7
             WHERE id = 'shot-rev'",
            [],
        )
        .unwrap();
    assert!(store
        .plan_restore_revision(DEFAULT_OWNER_ID, "plan-rev", 1)
        .is_err());
}

#[test]
fn plan_item_provenance_invariant_and_no_feedback_writes() {
    let directory = TestDir::new("plan-provenance");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_photo(DEFAULT_OWNER_ID, &reference_photo("photo-prov", "prov-sha"))
        .unwrap();
    store
        .upsert_video(DEFAULT_OWNER_ID, &video("video-prov", "prov-video-sha"))
        .unwrap();
    store
        .insert_shots(
            DEFAULT_OWNER_ID,
            std::slice::from_ref(&shot("shot-prov", "video-prov", 0)),
        )
        .unwrap();
    store
        .plan_create(DEFAULT_OWNER_ID, &plan("plan-prov", "provenance"))
        .unwrap();

    // A personalized item must carry the style-profile version that ranked it...
    let mut personal = plan_item("plan-prov", MediaKind::Shot, "shot-prov");
    personal.origin = PlanOrigin::Personal;
    personal.rank = Some(0.82);
    personal.profile_version = Some(3);
    personal.signals_json = r#"{"personal_affinity":0.12,"general_aesthetic":0.08}"#.to_owned();
    store.plan_add_item(DEFAULT_OWNER_ID, &personal).unwrap();
    // ...and a general item must not carry one.
    let mut forged = plan_item("plan-prov", MediaKind::Photo, "photo-prov");
    forged.origin = PlanOrigin::General;
    forged.profile_version = Some(3);
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &forged).is_err());
    let mut anonymous = plan_item("plan-prov", MediaKind::Photo, "photo-prov");
    anonymous.origin = PlanOrigin::Personal;
    assert!(store.plan_add_item(DEFAULT_OWNER_ID, &anonymous).is_err());

    let items = store.plan_items(DEFAULT_OWNER_ID, "plan-prov").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].origin, PlanOrigin::Personal);
    assert_eq!(items[0].profile_version, Some(3));
    assert_eq!(items[0].rank, Some(0.82));
    assert!(items[0].signals_json.contains("general_aesthetic"));

    // Plan writes are document state: nothing above appended feedback.
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());
    store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-prov", "checkpoint")
        .unwrap();
    store
        .plan_duplicate(DEFAULT_OWNER_ID, "plan-prov", "copy")
        .unwrap();
    store
        .plan_restore_revision(DEFAULT_OWNER_ID, "plan-prov", 1)
        .unwrap();
    assert!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().is_empty());
}

#[test]
fn schema_v8_upgrades_to_plans_without_losing_rows() {
    let directory = TestDir::new("migration-v8-v9");
    let db = directory.path().join("library.db");
    std::fs::create_dir_all(directory.path()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_version (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
             ) STRICT;
             INSERT INTO schema_version VALUES (1, 0);",
        )
        .unwrap();
    for (version, migration) in [
        (1, include_str!("../migrations/0001_init.sql")),
        (2, include_str!("../migrations/0002_dam_feedback.sql")),
        (3, include_str!("../migrations/0003_source_fidelity.sql")),
        (4, include_str!("../migrations/0004_strong_shot.sql")),
        (5, include_str!("../migrations/0005_feedback_hardening.sql")),
        (6, include_str!("../migrations/0006_photo_jobs.sql")),
        (7, include_str!("../migrations/0007_reference_sets.sql")),
        (8, include_str!("../migrations/0008_collections.sql")),
    ] {
        connection.execute_batch(migration).unwrap();
        connection
            .execute(
                "UPDATE schema_version SET version = ?1 WHERE singleton = 1",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO videos (
                id, owner_id, path, sha256, duration_s, fps, width, height, has_audio, status
             ) VALUES ('legacy-video', 'local', '/legacy.mov', 'legacy-sha', 1.0, 24.0,
                       1920, 1080, 1, 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO shots (
                id, video_id, owner_id, idx, start_s, end_s, rep_frame_s
             ) VALUES ('legacy-shot', 'legacy-video', 'local', 0, 0.0, 1.0, 0.5)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photos (
                id, owner_id, path, sha256, width, height, format, status
             ) VALUES ('legacy-photo', 'local', '/legacy.jpg', 'legacy-photo-sha',
                       100, 100, 'jpeg', 'done')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO feedback_events (
                id, owner_id, media_kind, media_id, signal, value, created_at
             ) VALUES ('legacy-event', 'local', 'photo', 'legacy-photo', 'pick', 1.0,
                       '2026-08-28T12:00:00+00:00')",
            [],
        )
        .unwrap();
    drop(connection);

    let mut store = Store::open(directory.path()).unwrap();
    assert_eq!(store.schema_version().unwrap(), 10);
    assert_eq!(store.videos(DEFAULT_OWNER_ID).unwrap().len(), 1);
    assert_eq!(store.feedback_events(DEFAULT_OWNER_ID).unwrap().len(), 1);
    // The v9 plan surfaces are live on the upgraded database.
    store
        .plan_create(DEFAULT_OWNER_ID, &plan("plan-upgrade", "upgraded"))
        .unwrap();
    store
        .plan_add_item(
            DEFAULT_OWNER_ID,
            &plan_item("plan-upgrade", MediaKind::Shot, "legacy-shot"),
        )
        .unwrap();
    let items = store.plan_items(DEFAULT_OWNER_ID, "plan-upgrade").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].start_s, Some(0.0));
    let revision = store
        .plan_save_revision(DEFAULT_OWNER_ID, "plan-upgrade", "after upgrade")
        .unwrap();
    assert_eq!(revision.revision, 1);
}

#[test]
fn render_jobs_freeze_portable_inputs_and_verified_outputs() {
    let directory = TestDir::new("render-contract");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 29, 19, 0, 0).unwrap();
    let source_hash = "a".repeat(64);
    let output_hash = "b".repeat(64);
    let manifest_hash = "c".repeat(64);
    let source_path = directory.path().join("original.jpg");
    let destination = directory.path().join("exports/hero.jpg");
    let staging = directory.path().join("exports/.crush-render/job-1.partial");

    let recipe = RenderRecipe {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        id: "photo-web".to_owned(),
        version: 1,
        kind: RenderRecipeKind::Photo,
        name: "Web JPEG".to_owned(),
        schema_json: serde_json::json!({
            "schema_version": 1,
            "kind": "photo",
            "crop": {"x": 0.1, "y": 0.0, "width": 0.8, "height": 1.0},
            "rotation_degrees": 90,
            "grade": {"mode": "none"},
            "output": {"preset": "jpeg-srgb-v1"}
        })
        .to_string(),
        created_at: now,
    };
    store
        .render_recipe_create(DEFAULT_OWNER_ID, &recipe)
        .unwrap();
    assert_eq!(
        store
            .render_recipe_get(DEFAULT_OWNER_ID, "photo-web", 1)
            .unwrap(),
        Some(recipe.clone())
    );

    let source_snapshot = serde_json::json!({
        "schema_version": 1,
        "context_key": "campaign",
        "selection_provenance": {"origin": "general", "rank": 0.91},
        "sources": [{
            "media_kind": "photo",
            "media_id": "photo-hero",
            "source_id": "photo-hero",
            "sha256": source_hash,
            "path": source_path
        }]
    })
    .to_string();
    let model_versions = serde_json::json!({
        "schema_version": 1,
        "models": {
            "clip": "models-v1",
            "aesthetic": "strong-shot-v1",
            "personal_style": "not_used"
        }
    })
    .to_string();
    let job = store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: "render-photo-1".to_owned(),
                recipe_id: recipe.id.clone(),
                recipe_version: recipe.version,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: source_snapshot.clone(),
                model_versions_json: model_versions.clone(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: now,
            },
        )
        .unwrap();
    assert_eq!(job.status, RenderJobStatus::Queued);
    assert_eq!(job.source_snapshot_json, source_snapshot);
    assert_eq!(job.model_versions_json, model_versions);
    assert!(job.frozen_recipe_json.contains("photo-web"));
    assert!(job.frozen_recipe_json.contains("jpeg-srgb-v1"));

    assert!(audit
        .execute(
            "UPDATE render_recipes SET name = 'rewritten' WHERE id = 'photo-web'",
            [],
        )
        .is_err());
    assert!(audit
        .execute(
            "UPDATE render_jobs SET destination_path = '/tmp/stolen.jpg' WHERE id = 'render-photo-1'",
            [],
        )
        .is_err());

    let attempt = store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-1",
            &staging.to_string_lossy(),
            now + chrono::Duration::seconds(1),
        )
        .unwrap();
    assert_eq!(attempt.attempt, 1);
    store
        .render_attempt_set_commands(
            DEFAULT_OWNER_ID,
            "render-photo-1",
            1,
            r#"[{"program":"photo-renderer","backend":"cpu"}]"#,
        )
        .unwrap();
    store
        .render_job_set_progress(DEFAULT_OWNER_ID, "render-photo-1", 0.75)
        .unwrap();
    assert!(store
        .render_job_set_progress(DEFAULT_OWNER_ID, "render-photo-1", 0.5)
        .is_err());
    assert!(store
        .render_job_set_progress(DEFAULT_OWNER_ID, "render-photo-1", 1.0)
        .is_err());
    store
        .render_job_mark_verifying(DEFAULT_OWNER_ID, "render-photo-1")
        .unwrap();
    let output = RenderOutput {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        id: "output-photo-1".to_owned(),
        job_id: "render-photo-1".to_owned(),
        attempt: 1,
        output_path: destination.to_string_lossy().into_owned(),
        output_sha256: output_hash,
        size_bytes: 42,
        media_type: "image/jpeg".to_owned(),
        width: Some(1200),
        height: Some(1500),
        duration_s: None,
        verification_json: r#"{"dimensions":true,"orientation":true,"color":"srgb"}"#.to_owned(),
        manifest_path: destination
            .with_extension("jpg.manifest.json")
            .to_string_lossy()
            .into_owned(),
        manifest_json: r#"{"schema_version":1,"verified":true}"#.to_owned(),
        manifest_sha256: manifest_hash,
        created_at: now + chrono::Duration::seconds(2),
    };
    store.render_job_finish(DEFAULT_OWNER_ID, &output).unwrap();
    let finished = store
        .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-1")
        .unwrap()
        .unwrap();
    assert_eq!(finished.status, RenderJobStatus::Done);
    assert_eq!(finished.progress, 1.0);
    assert_eq!(
        store
            .render_output_by_job(DEFAULT_OWNER_ID, "render-photo-1")
            .unwrap(),
        Some(output)
    );
    assert!(store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-1",
            &staging.to_string_lossy(),
            now + chrono::Duration::seconds(3),
        )
        .is_err());
    assert!(audit
        .execute("DELETE FROM render_jobs WHERE id = 'render-photo-1'", [])
        .is_err());
}

#[test]
fn render_contract_rejects_unsupported_or_cross_owner_intent_and_retries_safely() {
    const OWNER_B: &str = "render-owner-b";
    let directory = TestDir::new("render-validation");
    let mut store = Store::open(directory.path()).unwrap();
    let audit = Connection::open(store.db_path()).unwrap();
    audit
        .execute(
            "INSERT INTO owners (id, name, created_at) VALUES (?1, ?2, ?3)",
            [OWNER_B, "Other renderer", "2026-08-29T19:00:00Z"],
        )
        .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 29, 19, 0, 0).unwrap();
    let mut recipe = RenderRecipe {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        id: "clip-safe".to_owned(),
        version: 1,
        kind: RenderRecipeKind::VideoClip,
        name: "Portable clip".to_owned(),
        schema_json: serde_json::json!({
            "schema_version": 1,
            "kind": "video_clip",
            "in_s": 1.0,
            "out_s": 3.0,
            "crop": null,
            "grade": {"mode": "none"},
            "transition": {"kind": "cut"},
            "audio": {"mode": "source"},
            "output": {"preset": "mp4-h264-sdr-v1"}
        })
        .to_string(),
        created_at: now,
    };
    store
        .render_recipe_create(DEFAULT_OWNER_ID, &recipe)
        .unwrap();
    recipe.id = "unsupported".to_owned();
    recipe.schema_json = recipe
        .schema_json
        .replace(r#"{"mode":"none"}"#, r#"{"mode":"mystery"}"#);
    assert!(store
        .render_recipe_create(DEFAULT_OWNER_ID, &recipe)
        .is_err());

    let request = NewRenderJob {
        id: "render-retry".to_owned(),
        recipe_id: "clip-safe".to_owned(),
        recipe_version: 1,
        plan_id: None,
        plan_revision: None,
        source_snapshot_json: serde_json::json!({
            "schema_version": 1,
            "context_key": "default",
            "selection_provenance": {"origin": "general"},
            "sources": [{
                "media_kind": "video",
                "media_id": "video-1",
                "source_id": "video-1",
                "sha256": "d".repeat(64),
                "path": directory.path().join("source.mov")
            }]
        })
        .to_string(),
        model_versions_json: serde_json::json!({
            "schema_version": 1,
            "models": {"clip": "not_used", "aesthetic": "not_used", "personal_style": "not_used"}
        })
        .to_string(),
        destination_path: directory
            .path()
            .join("clip.mp4")
            .to_string_lossy()
            .into_owned(),
        created_at: now,
    };
    assert!(store.render_job_create(OWNER_B, &request).is_err());
    store.render_job_create(DEFAULT_OWNER_ID, &request).unwrap();
    let first_staging = directory.path().join(".render-retry-1/clip.partial");
    store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-retry",
            &first_staging.to_string_lossy(),
            now,
        )
        .unwrap();
    store
        .render_job_fail(
            DEFAULT_OWNER_ID,
            "render-retry",
            "encoder unavailable",
            now + chrono::Duration::seconds(1),
        )
        .unwrap();
    let second_staging = directory.path().join(".render-retry-2/clip.partial");
    let second = store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-retry",
            &second_staging.to_string_lossy(),
            now + chrono::Duration::seconds(2),
        )
        .unwrap();
    assert_eq!(second.attempt, 2);
    store
        .render_job_cancel(
            DEFAULT_OWNER_ID,
            "render-retry",
            now + chrono::Duration::seconds(3),
        )
        .unwrap();
    assert_eq!(
        store
            .render_attempts(DEFAULT_OWNER_ID, "render-retry")
            .unwrap()
            .iter()
            .map(|attempt| attempt.status)
            .collect::<Vec<_>>(),
        vec![RenderJobStatus::Failed, RenderJobStatus::Cancelled]
    );
    assert!(audit
        .execute(
            "UPDATE render_attempts SET progress = 0.0 WHERE job_id = 'render-retry' AND attempt = 2",
            [],
        )
        .is_err());
}
