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
    AestheticAssessment, EditorialAnnotation, EmbeddingMeta, FeedbackEvent, FeedbackSignal,
    JobFilter, MediaKind, NewJob, Photo, PhotoProxyProvenance, PhotoSourceMetadata, PhotoStatus,
    ProblemKind, Shot, Store, StyleProfile, TranscriptSegment, Video, VideoSourceMetadata,
    VideoStatus,
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

#[test]
fn fresh_database_migrates_once_and_enforces_connection_pragmas() {
    let directory = TestDir::new("migration");
    let store = Store::open(directory.path()).expect("fresh database should open");
    assert_eq!(store.schema_version().unwrap(), 3);
    assert_eq!(store.db_path(), directory.path().join("library.db"));

    let missing_vector = store.put_vector(DEFAULT_OWNER_ID, "missing-shot", &[1.0]);
    assert!(
        missing_vector.is_err(),
        "foreign keys must be enabled on the Store connection"
    );
    drop(store);

    let reopened = Store::open(directory.path()).expect("second open should be a migration no-op");
    assert_eq!(reopened.schema_version().unwrap(), 3);
    let audit = Connection::open(reopened.db_path()).unwrap();
    let journal_mode: String = audit
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
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
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-1", &[1.0, -0.0, f32::NAN])
        .unwrap();
    let exact_vector = store
        .vector_for_shot(DEFAULT_OWNER_ID, "shot-1")
        .unwrap()
        .unwrap();
    assert_eq!(exact_vector[0].to_bits(), 1.0_f32.to_bits());
    assert_eq!(exact_vector[1].to_bits(), (-0.0_f32).to_bits());
    assert!(exact_vector[2].is_nan());
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
                video_id: "video-1".to_owned(),
                stage: Stage::Split,
                started_at: Utc.with_ymd_and_hms(2026, 8, 27, 11, 0, 0).unwrap(),
                debug_dir: None,
            },
        )
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
                f32::from_bits(((index * DIM + column) as u32).wrapping_mul(2_654_435_761))
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
                    video_id: "video-j".to_owned(),
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
                video_id: "video-i".to_owned(),
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
                video_id: "video-i".to_owned(),
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

    let problems = store.integrity().unwrap();
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingVector && problem.entity_id == "shot-missing"
    }));
    assert!(problems.iter().any(|problem| {
        problem.kind == ProblemKind::MissingThumbnail && problem.entity_id == "shot-missing"
    }));
    assert_eq!(
        problems
            .iter()
            .filter(|problem| problem.entity_id == "shot-vector")
            .count(),
        0
    );

    std::fs::write(directory.path().join("thumbs/missing.jpg"), b"jpeg").unwrap();
    store
        .put_vector(DEFAULT_OWNER_ID, "shot-missing", &[0.0; 512])
        .unwrap();
    assert!(store.integrity().unwrap().is_empty());
}
