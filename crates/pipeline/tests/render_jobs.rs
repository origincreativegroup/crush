use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_pipeline::{sha256_file, Pipeline};
#[cfg(target_os = "macos")]
use crush_store::{MediaKind, Plan, PlanItem, PlanOrigin, Shot, Video, VideoStatus};
use crush_store::{
    NewRenderJob, Photo, PhotoStatus, RenderJobStatus, RenderRecipe, RenderRecipeKind, Store,
};
use image::{Rgb, RgbImage};

fn setup_photo_job(
    root: &Path,
    preset: &str,
    destination: &Path,
    job_id: &str,
) -> (Pipeline, PathBuf) {
    let source = root.join(format!("{job_id}-source.png"));
    let identity_tint = job_id.bytes().fold(0_u8, u8::wrapping_add);
    RgbImage::from_fn(12, 8, |x, y| {
        if x < 6 {
            Rgb([220, (y * 10) as u8, identity_tint])
        } else {
            Rgb([20, 80, (x * 10) as u8])
        }
    })
    .save(&source)
    .unwrap();
    let source_hash = sha256_file(&source).unwrap();
    let photo_id = format!("{job_id}-photo");
    let recipe_id = format!("{job_id}-recipe");
    let mut store = Store::open(root).unwrap();
    store
        .upsert_photo(
            DEFAULT_OWNER_ID,
            &Photo {
                id: photo_id.clone(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: source.to_string_lossy().into_owned(),
                sha256: source_hash.clone(),
                width: 12,
                height: 8,
                format: "png".to_owned(),
                orientation: None,
                captured_at: None,
                camera_make: None,
                camera_model: None,
                lens: None,
                thumb_rel: None,
                status: PhotoStatus::Done,
                indexed_at: Some(Utc::now()),
            },
        )
        .unwrap();
    store
        .render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: recipe_id.clone(),
                version: 1,
                kind: RenderRecipeKind::Photo,
                name: "Test derivative".to_owned(),
                schema_json: serde_json::json!({
                    "schema_version": 1,
                    "kind": "photo",
                    "crop": {"x": 0.0, "y": 0.0, "width": 0.5, "height": 1.0},
                    "rotation_degrees": 90,
                    "grade": {"mode": "none"},
                    "output": {"preset": preset}
                })
                .to_string(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: job_id.to_owned(),
                recipe_id,
                recipe_version: 1,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": "render-test",
                    "selection_provenance": {"origin": "general"},
                    "sources": [{
                        "media_kind": "photo",
                        "media_id": photo_id,
                        "source_id": format!("{job_id}-photo"),
                        "sha256": source_hash,
                        "path": source,
                    }]
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used"
                    }
                })
                .to_string(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    (
        Pipeline::new(
            Config {
                data_dir: Some(root.to_path_buf()),
                ..Config::default()
            },
            AppPaths {
                root: root.to_path_buf(),
            },
            CancellationToken::default(),
        ),
        source,
    )
}

#[cfg(target_os = "macos")]
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Generate a deterministic 60 fps source with AAC audio in-test (the TASK-036 fixture
/// pattern) — no tracked fixture, so no license/determinism review. Pinned lavfi parameters
/// keep generation reproducible. This is the fps/AAC-priming shape that exposed the
/// pass-then-fail duration-tolerance window (encoder ≈0.067 s vs executor 0.05 s).
#[cfg(target_os = "macos")]
fn generate_60fps_aac_source(ffmpeg: &Path, output: &Path) {
    let status = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x180:rate=60:duration=1",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=44100:duration=1",
        ])
        .args(["-map", "0:v", "-map", "1:a"])
        .args(["-c:v", "h264_videotoolbox", "-pix_fmt", "yuv420p"])
        .args([
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-tag:v",
            "avc1",
        ])
        .args(["-c:a", "aac", "-b:a", "192k"])
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success());
}

/// TASK-035 item 1: a 60 fps AAC-priming source renders through the durable clip path with
/// the executor's re-check using the ONE shared duration-tolerance rule
/// (`frame_tolerance + 0.05`), so nothing the encoder accepted can fail the executor. The
/// manifest pins the shared value (1/60 + 0.05 ≈ 0.0667 s) for this 60 fps source.
#[cfg(target_os = "macos")]
#[test]
fn sixty_fps_aac_clip_uses_the_shared_duration_tolerance_rule() {
    if crush_stage_split::ffmpeg::resolve().is_err() {
        eprintln!("skipping video render: bundled/development FFmpeg is unavailable");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("sixty-fps-aac.mp4");
    let resolved = crush_stage_split::ffmpeg::resolve().unwrap();
    generate_60fps_aac_source(&resolved.path, &source);
    let source_hash = sha256_file(&source).unwrap();
    let destination = directory.path().join("exports/sixty-fps-clip.mp4");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: "video-60fps".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: source.to_string_lossy().into_owned(),
                sha256: source_hash.clone(),
                duration_s: Some(1.0),
                fps: Some(60.0),
                width: Some(320),
                height: Some(180),
                has_audio: true,
                status: VideoStatus::Done,
                indexed_at: Some(Utc::now()),
            },
        )
        .unwrap();
    store
        .render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: "clip-60fps".to_owned(),
                version: 1,
                kind: RenderRecipeKind::VideoClip,
                name: "60 fps clip".to_owned(),
                schema_json: serde_json::json!({
                    "schema_version": 1,
                    "kind": "video_clip",
                    "in_s": 0.25,
                    "out_s": 1.0,
                    "crop": null,
                    "grade": {"mode": "none"},
                    "transition": {"kind": "cut"},
                    "audio": {"mode": "source"},
                    "output": {"preset": "mp4-h264-sdr-v1"}
                })
                .to_string(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: "render-clip-60fps".to_owned(),
                recipe_id: "clip-60fps".to_owned(),
                recipe_version: 1,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": "render-test",
                    "selection_provenance": {"origin": "general"},
                    "sources": [{
                        "media_kind": "video",
                        "media_id": "video-60fps",
                        "source_id": "video-60fps",
                        "sha256": source_hash,
                        "path": source,
                    }]
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used"
                    }
                })
                .to_string(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    drop(store);
    let pipeline = Pipeline::new(
        Config {
            data_dir: Some(directory.path().to_path_buf()),
            ..Config::default()
        },
        AppPaths {
            root: directory.path().to_path_buf(),
        },
        CancellationToken::default(),
    );

    let output = pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-clip-60fps")
        .unwrap();

    assert!(destination.is_file());
    let manifest: serde_json::Value = serde_json::from_str(&output.manifest_json).unwrap();
    let tolerance = manifest["verification"]["duration_tolerance_s"]
        .as_f64()
        .unwrap();
    let expected = 1.0 / 60.0 + 0.05;
    assert!(
        (tolerance - expected).abs() < 1e-9,
        "executor tolerance must equal the shared rule's value {expected}, got {tolerance}"
    );
    assert_eq!(manifest["verification"]["fps"], 60.0);
    assert_eq!(manifest["verification"]["source_unchanged"], true);
    assert!((output.duration_s.unwrap() - 0.75).abs() <= expected);
}

/// Decode one frame by index to raw YUV420P planes. Raw planes avoid color-matrix
/// assumptions: the untagged MPEG-4 fixture and the BT.709-tagged H.264 reel would
/// otherwise round-trip through different RGB matrices and hide the frame identity.
#[cfg(target_os = "macos")]
fn frame_yuv420p(ffmpeg: &Path, input: &Path, frame_index: i64) -> Vec<u8> {
    let output = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(input)
        .args([
            "-vf",
            &format!("select=eq(n\\,{frame_index})"),
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout
}

#[cfg(target_os = "macos")]
fn mean_abs_diff(left: &[u8], right: &[u8]) -> f64 {
    assert_eq!(left.len(), right.len(), "frames must share dimensions");
    left.iter()
        .zip(right)
        .map(|(a, b)| (*a as i64 - *b as i64).abs() as f64)
        .sum::<f64>()
        / left.len() as f64
}

/// The synthetic-speech fixture carries a burned-in source frame counter, so the reel's
/// boundary frames can be identified by content: the rendered frame must be nearest (by
/// mean absolute plane difference) to the expected source frame and clearly separated
/// from its neighbours. This is the automated form of the 021 review's ground truth.
#[cfg(target_os = "macos")]
fn assert_frame_identity(
    ffmpeg: &Path,
    source: &Path,
    reel: &Path,
    reel_frame: i64,
    expected_source_frame: i64,
) {
    let rendered = frame_yuv420p(ffmpeg, reel, reel_frame);
    let mut scores = Vec::new();
    for candidate in [
        expected_source_frame - 1,
        expected_source_frame,
        expected_source_frame + 1,
    ] {
        if candidate < 0 {
            continue;
        }
        let reference = frame_yuv420p(ffmpeg, source, candidate);
        scores.push((candidate, mean_abs_diff(&rendered, &reference)));
    }
    let (best_frame, best_score) = scores
        .iter()
        .copied()
        .min_by(|a, b| a.1.partial_cmp(&b.1).expect("finite frame distances"))
        .expect("at least the expected frame is compared");
    assert_eq!(
        best_frame, expected_source_frame,
        "reel frame {reel_frame} must be source frame {expected_source_frame}; scores: {scores:?}"
    );
    let neighbour = scores
        .iter()
        .find(|(frame, _)| *frame != expected_source_frame)
        .map(|(_, score)| *score)
        .unwrap_or_default();
    assert!(
        best_score * 4.0 < neighbour,
        "reel frame {reel_frame} match to source frame {expected_source_frame} must be \
         unambiguous; best {best_score:.3} vs neighbour {neighbour:.3}"
    );
}

/// Presentation timestamps of every video packet, in order.
#[cfg(target_os = "macos")]
fn video_packet_pts(ffprobe: &Path, input: &Path) -> Vec<f64> {
    let output = std::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_packets",
            "-select_streams",
            "v",
        ])
        .arg(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    document["packets"]
        .as_array()
        .expect("video packets")
        .iter()
        .filter_map(|packet| packet["pts_time"].as_str())
        .filter_map(|value| value.parse::<f64>().ok())
        .collect()
}

fn preserve_review_artifact(source: &Path, name: &str) {
    let Ok(directory) = std::env::var("CRUSH_RENDER_REVIEW_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory).unwrap();
    let destination = directory.join(name);
    assert!(
        !destination.exists(),
        "review artifact already exists: {}",
        destination.display()
    );
    fs::copy(source, &destination).unwrap();
}

fn preserve_review_output(output: &crush_store::RenderOutput, name: &str) {
    preserve_review_artifact(Path::new(&output.output_path), name);
    preserve_review_artifact(
        Path::new(&output.manifest_path),
        &format!("{name}.crush-manifest.json"),
    );
}

#[test]
fn frozen_photo_job_publishes_verified_output_and_manifest_without_touching_source() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("exports/hero.png");
    let (pipeline, source) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-success",
    );
    let source_hash = sha256_file(&source).unwrap();

    let output = pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-photo-success")
        .unwrap();

    assert!(destination.is_file());
    assert_eq!(sha256_file(&source).unwrap(), source_hash);
    assert_eq!(sha256_file(&destination).unwrap(), output.output_sha256);
    assert_eq!(output.width, Some(4));
    assert_eq!(output.height, Some(12));
    let manifest_path = Path::new(&output.manifest_path);
    assert!(manifest_path.is_file());
    assert_eq!(sha256_file(manifest_path).unwrap(), output.manifest_sha256);
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["source"]["sha256"], source_hash);
    assert_eq!(manifest["render"]["preset"], "png-srgb-v1");
    assert_eq!(manifest["verification"]["source_unchanged"], true);
    assert_eq!(
        Store::open(directory.path())
            .unwrap()
            .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-success")
            .unwrap()
            .unwrap()
            .status,
        RenderJobStatus::Done
    );
}

#[test]
fn documented_photo_presets_are_deterministic_through_the_durable_executor() {
    for (preset, extension, media_type) in [
        ("jpeg-srgb-v1", "jpg", "image/jpeg"),
        ("png-srgb-v1", "png", "image/png"),
        ("tiff-srgb-v1", "tiff", "image/tiff"),
    ] {
        let first_directory = tempfile::tempdir().unwrap();
        let first_destination = first_directory
            .path()
            .join(format!("exports/review.{extension}"));
        let (first_pipeline, first_source) = setup_photo_job(
            first_directory.path(),
            preset,
            &first_destination,
            "render-photo-preset",
        );
        let first_source_hash = sha256_file(&first_source).unwrap();
        let first = first_pipeline
            .execute_render_job(DEFAULT_OWNER_ID, "render-photo-preset")
            .unwrap();

        let second_directory = tempfile::tempdir().unwrap();
        let second_destination = second_directory
            .path()
            .join(format!("exports/review.{extension}"));
        let (second_pipeline, second_source) = setup_photo_job(
            second_directory.path(),
            preset,
            &second_destination,
            "render-photo-preset",
        );
        let second_source_hash = sha256_file(&second_source).unwrap();
        let second = second_pipeline
            .execute_render_job(DEFAULT_OWNER_ID, "render-photo-preset")
            .unwrap();

        assert_eq!(first_source_hash, second_source_hash, "{preset} source");
        assert_eq!(first.media_type, media_type);
        assert_eq!((first.width, first.height), (Some(4), Some(12)));
        assert_eq!(first.output_sha256, second.output_sha256, "{preset}");
        assert_eq!(sha256_file(&first_source).unwrap(), first_source_hash);
        assert_eq!(sha256_file(&second_source).unwrap(), second_source_hash);
        assert!(first_destination.is_file());
        assert!(second_destination.is_file());
        preserve_review_output(&first, &format!("photo-derivative.{extension}"));
        if preset == "jpeg-srgb-v1" {
            preserve_review_artifact(&first_source, "photo-source.png");
        }
    }
}

#[test]
fn existing_destination_and_stale_source_fail_before_an_attempt_or_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("existing.png");
    fs::write(&destination, b"keep me").unwrap();
    let (pipeline, source) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-guard",
    );
    assert!(pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-photo-guard")
        .is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"keep me");
    assert_eq!(
        Store::open(directory.path())
            .unwrap()
            .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-guard")
            .unwrap()
            .unwrap()
            .current_attempt,
        0
    );

    let manifest_destination = directory.path().join("manifest-collision.png");
    let manifest_path = PathBuf::from(format!(
        "{}.crush-manifest.json",
        manifest_destination.to_string_lossy()
    ));
    fs::write(&manifest_path, b"user manifest").unwrap();
    let (manifest_pipeline, _) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &manifest_destination,
        "render-photo-manifest-guard",
    );
    assert!(manifest_pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-photo-manifest-guard")
        .is_err());
    assert_eq!(fs::read(&manifest_path).unwrap(), b"user manifest");
    assert!(!manifest_destination.exists());

    let stale_destination = directory.path().join("stale.png");
    let (stale_pipeline, stale_source) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &stale_destination,
        "render-photo-stale",
    );
    fs::write(&stale_source, b"changed after queue").unwrap();
    assert!(stale_pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-photo-stale")
        .is_err());
    assert!(!stale_destination.exists());
    assert!(source.is_file());
}

#[test]
fn startup_recovery_removes_only_owned_marked_staging_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("exports/recovered.png");
    let (pipeline, _) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-recovery",
    );
    let staging_dir = destination
        .parent()
        .unwrap()
        .join(".crush-render-interrupted");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_output = staging_dir.join("recovered.png");
    fs::write(&staging_output, b"unverified").unwrap();
    let attempt = Store::open(directory.path())
        .unwrap()
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-recovery",
            &staging_output.to_string_lossy(),
            Utc::now(),
        )
        .unwrap();
    fs::write(
        staging_dir.join("marker.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "owner_id": DEFAULT_OWNER_ID,
            "job_id": "render-photo-recovery",
            "attempt": attempt.attempt,
            "destination": destination,
        }))
        .unwrap(),
    )
    .unwrap();

    let first = pipeline
        .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
        .unwrap();
    assert_eq!(first.failed, 1);
    assert_eq!(first.staging_removed, 1);
    assert!(!staging_dir.exists());
    assert_eq!(
        Store::open(directory.path())
            .unwrap()
            .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-recovery")
            .unwrap()
            .unwrap()
            .status,
        RenderJobStatus::Failed
    );
    assert_eq!(
        pipeline
            .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
            .unwrap(),
        Default::default()
    );
}

#[test]
fn recovery_preserves_unmarked_directories_even_with_a_managed_looking_name() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("exports/preserved.png");
    let (pipeline, _) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-preserve",
    );
    let staging_dir = destination
        .parent()
        .unwrap()
        .join(".crush-render-user-files");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_output = staging_dir.join("preserved.png");
    fs::write(&staging_output, b"not owned by Crush").unwrap();
    Store::open(directory.path())
        .unwrap()
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-preserve",
            &staging_output.to_string_lossy(),
            Utc::now(),
        )
        .unwrap();

    let recovery = pipeline
        .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
        .unwrap();
    assert_eq!(recovery.failed, 1);
    assert_eq!(recovery.staging_removed, 0);
    assert_eq!(fs::read(&staging_output).unwrap(), b"not owned by Crush");
}

#[test]
fn pre_cancelled_render_does_not_create_an_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("cancelled.png");
    setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-cancelled",
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let pipeline = Pipeline::new(
        Config {
            data_dir: Some(directory.path().to_path_buf()),
            ..Config::default()
        },
        AppPaths {
            root: directory.path().to_path_buf(),
        },
        cancellation,
    );

    assert!(pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-photo-cancelled")
        .is_err());
    let job = Store::open(directory.path())
        .unwrap()
        .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-cancelled")
        .unwrap()
        .unwrap();
    assert_eq!(job.status, RenderJobStatus::Queued);
    assert_eq!(job.current_attempt, 0);
    assert!(!destination.exists());
}

#[test]
fn recovery_finishes_a_fully_published_verifying_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("exports/finished-after-crash.png");
    let (pipeline, _) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-finalize",
    );
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let staging_dir = destination
        .parent()
        .unwrap()
        .join(".crush-render-published");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_output = staging_dir.join("finished-after-crash.png");
    let mut store = Store::open(directory.path()).unwrap();
    let attempt = store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-finalize",
            &staging_output.to_string_lossy(),
            Utc::now(),
        )
        .unwrap();
    fs::write(
        staging_dir.join("marker.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "owner_id": DEFAULT_OWNER_ID,
            "job_id": "render-photo-finalize",
            "attempt": attempt.attempt,
            "destination": destination,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(&destination, b"verified output after crash").unwrap();
    let output_hash = sha256_file(&destination).unwrap();
    let manifest_path = PathBuf::from(format!(
        "{}.crush-manifest.json",
        destination.to_string_lossy()
    ));
    let manifest_json = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "job": {"id": "render-photo-finalize", "attempt": attempt.attempt},
        "verification": {"source_unchanged": true}
    }))
    .unwrap();
    fs::write(&manifest_path, &manifest_json).unwrap();
    let manifest_hash = sha256_file(&manifest_path).unwrap();
    let created_at = Utc::now();
    let command = serde_json::json!([{
        "executor": "crush-photo-cpu-v1",
        "phase": "verified_staging",
        "staging_manifest": staging_dir.join("manifest.partial.json"),
        "output": {
            "owner_id": DEFAULT_OWNER_ID,
            "id": "render-output-recovered",
            "job_id": "render-photo-finalize",
            "attempt": attempt.attempt,
            "output_path": destination,
            "output_sha256": output_hash,
            "size_bytes": fs::metadata(&destination).unwrap().len(),
            "media_type": "image/png",
            "width": 12,
            "height": 8,
            "duration_s": null,
            "verification_json": "{\"source_unchanged\":true}",
            "manifest_path": manifest_path,
            "manifest_json": manifest_json,
            "manifest_sha256": manifest_hash,
            "created_at": created_at,
        }
    }]);
    store
        .render_attempt_set_commands(
            DEFAULT_OWNER_ID,
            "render-photo-finalize",
            attempt.attempt,
            &command.to_string(),
        )
        .unwrap();
    store
        .render_job_mark_verifying(DEFAULT_OWNER_ID, "render-photo-finalize")
        .unwrap();
    drop(store);

    let recovery = pipeline
        .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
        .unwrap();
    assert_eq!(recovery.finalized, 1);
    assert_eq!(recovery.staging_removed, 1);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store
            .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-finalize")
            .unwrap()
            .unwrap()
            .status,
        RenderJobStatus::Done
    );
    assert_eq!(
        store
            .render_output_by_job(DEFAULT_OWNER_ID, "render-photo-finalize")
            .unwrap()
            .unwrap()
            .output_sha256,
        output_hash
    );
}

/// TASK-035 item 2: a verifying publication whose output no longer matches the checksummed
/// size short-circuits before any full SHA-256 — recovery fails the job and never finalizes
/// or deletes the mismatching files. (The intact counterpart is
/// `recovery_finishes_a_fully_published_verifying_attempt`.)
#[test]
fn recovery_rejects_a_truncated_publication_by_size_without_finalizing() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("exports/truncated-after-crash.png");
    let (pipeline, _) = setup_photo_job(
        directory.path(),
        "png-srgb-v1",
        &destination,
        "render-photo-truncated",
    );
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let staging_dir = destination
        .parent()
        .unwrap()
        .join(".crush-render-published");
    fs::create_dir_all(&staging_dir).unwrap();
    let staging_output = staging_dir.join("truncated-after-crash.png");
    let mut store = Store::open(directory.path()).unwrap();
    let attempt = store
        .render_job_start(
            DEFAULT_OWNER_ID,
            "render-photo-truncated",
            &staging_output.to_string_lossy(),
            Utc::now(),
        )
        .unwrap();
    fs::write(
        staging_dir.join("marker.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "owner_id": DEFAULT_OWNER_ID,
            "job_id": "render-photo-truncated",
            "attempt": attempt.attempt,
            "destination": destination,
        }))
        .unwrap(),
    )
    .unwrap();
    // The published output is truncated relative to the checksummed evidence (10 bytes on
    // disk, 4096 claimed), so the size short-circuit decides without hashing.
    fs::write(&destination, b"truncated!").unwrap();
    let manifest_path = PathBuf::from(format!(
        "{}.crush-manifest.json",
        destination.to_string_lossy()
    ));
    let manifest_json = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "job": {"id": "render-photo-truncated", "attempt": attempt.attempt}
    }))
    .unwrap();
    fs::write(&manifest_path, &manifest_json).unwrap();
    let command = serde_json::json!([{
        "executor": "crush-photo-cpu-v1",
        "phase": "verified_staging",
        "staging_manifest": staging_dir.join("manifest.partial.json"),
        "output": {
            "owner_id": DEFAULT_OWNER_ID,
            "id": "render-output-truncated",
            "job_id": "render-photo-truncated",
            "attempt": attempt.attempt,
            "output_path": destination,
            "output_sha256": "f".repeat(64),
            "size_bytes": 4096,
            "media_type": "image/png",
            "width": 12,
            "height": 8,
            "duration_s": null,
            "verification_json": "{}",
            "manifest_path": manifest_path,
            "manifest_json": manifest_json,
            "manifest_sha256": sha256_file(&manifest_path).unwrap(),
            "created_at": Utc::now(),
        }
    }]);
    store
        .render_attempt_set_commands(
            DEFAULT_OWNER_ID,
            "render-photo-truncated",
            attempt.attempt,
            &command.to_string(),
        )
        .unwrap();
    store
        .render_job_mark_verifying(DEFAULT_OWNER_ID, "render-photo-truncated")
        .unwrap();
    drop(store);

    let recovery = pipeline
        .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
        .unwrap();

    assert_eq!(recovery.finalized, 0);
    assert_eq!(recovery.failed, 1);
    let job = Store::open(directory.path())
        .unwrap()
        .render_job_by_id(DEFAULT_OWNER_ID, "render-photo-truncated")
        .unwrap()
        .unwrap();
    assert_eq!(job.status, RenderJobStatus::Failed);
    // A size-mismatched publication is unverified, never finalized — but it is also not
    // owned-managed staging, so the on-disk bytes stay for human inspection.
    assert_eq!(fs::read(&destination).unwrap(), b"truncated!");
}

#[cfg(target_os = "macos")]
#[test]
fn frozen_video_clip_job_encodes_and_publishes_measured_manifest() {
    if crush_stage_split::ffmpeg::resolve().is_err() {
        eprintln!("skipping video render: bundled/development FFmpeg is unavailable");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = repo_root().join("fixtures/clips/earth-timelapse-silent.mp4");
    let source_hash = sha256_file(&source).unwrap();
    let destination = directory.path().join("exports/earth-clip.mp4");
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: "video-earth".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: source.to_string_lossy().into_owned(),
                sha256: source_hash.clone(),
                duration_s: Some(6.0),
                fps: Some(30.0),
                width: Some(1280),
                height: Some(720),
                has_audio: false,
                status: VideoStatus::Done,
                indexed_at: Some(Utc::now()),
            },
        )
        .unwrap();
    store
        .render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: "clip-mp4".to_owned(),
                version: 1,
                kind: RenderRecipeKind::VideoClip,
                name: "MP4 clip".to_owned(),
                schema_json: serde_json::json!({
                    "schema_version": 1,
                    "kind": "video_clip",
                    "in_s": 0.25,
                    "out_s": 1.25,
                    "crop": {"x": 0.1, "y": 0.1, "width": 0.8, "height": 0.8},
                    "grade": {"mode": "none"},
                    "transition": {"kind": "cut"},
                    "audio": {"mode": "mute"},
                    "output": {"preset": "mp4-h264-sdr-v1"}
                })
                .to_string(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: "render-video-clip".to_owned(),
                recipe_id: "clip-mp4".to_owned(),
                recipe_version: 1,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": "render-test",
                    "selection_provenance": {"origin": "general"},
                    "sources": [{
                        "media_kind": "video",
                        "media_id": "video-earth",
                        "source_id": "video-earth",
                        "sha256": source_hash,
                        "path": source,
                    }]
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used"
                    }
                })
                .to_string(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: Utc::now(),
            },
        )
        .unwrap();
    drop(store);
    let pipeline = Pipeline::new(
        Config {
            data_dir: Some(directory.path().to_path_buf()),
            ..Config::default()
        },
        AppPaths {
            root: directory.path().to_path_buf(),
        },
        CancellationToken::default(),
    );

    let output = pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-video-clip")
        .unwrap();

    assert_eq!(output.media_type, "video/mp4");
    assert!(output.duration_s.unwrap() >= 0.95);
    assert!(output.duration_s.unwrap() <= 1.05);
    assert_eq!(sha256_file(&source).unwrap(), source_hash);
    assert!(destination.is_file());
    let manifest: serde_json::Value = serde_json::from_str(&output.manifest_json).unwrap();
    assert_eq!(manifest["render"]["preset"], "mp4-h264-sdr-v1");
    assert_eq!(manifest["tool_versions"]["backend"], "videotoolbox");
    assert_eq!(manifest["verification"]["source_unchanged"], true);
    // The approved clip properties are test-enforced, not just documented (review LOW-2):
    // exactly 15 frames, 1.000 s, 512x288 (the declared 10% inset of 640x360), no audio.
    let resolved = crush_stage_split::ffmpeg::resolve().unwrap();
    let probe = crush_stage_split::ffmpeg::Runner::new(resolved, 2, "clip-earth-golden")
        .probe(&destination)
        .unwrap()
        .value;
    assert_eq!(probe.video_frame_count, Some(15));
    assert_eq!((probe.width, probe.height), (512, 288));
    assert!(!probe.has_audio);
    assert!((probe.duration_s - 1.0).abs() <= 0.05);
    assert!((probe.video_duration_s.unwrap() - 1.0).abs() <= 0.002);
    preserve_review_output(&output, "clip-earth.mp4");
}

#[cfg(target_os = "macos")]
#[test]
fn frozen_ordered_reel_job_renders_project_order_and_publishes_one_manifest() {
    if crush_stage_split::ffmpeg::resolve().is_err() {
        eprintln!("skipping reel render: bundled/development FFmpeg is unavailable");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = repo_root().join("fixtures/clips/synthetic-speech.mp4");
    let source_hash = sha256_file(&source).unwrap();
    let destination = directory.path().join("exports/ordered-reel.mp4");
    let now = Utc::now();
    let mut store = Store::open(directory.path()).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: "video-reel".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: source.to_string_lossy().into_owned(),
                sha256: source_hash.clone(),
                duration_s: Some(12.0),
                fps: Some(30.0),
                width: Some(640),
                height: Some(360),
                has_audio: true,
                status: VideoStatus::Done,
                indexed_at: Some(now),
            },
        )
        .unwrap();
    let shots = [
        Shot {
            id: "reel-shot-a".to_owned(),
            video_id: "video-reel".to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            idx: 0,
            start_s: 0.0,
            end_s: 2.0,
            rep_frame_s: 1.0,
            thumb_rel: None,
            scene_score: None,
        },
        Shot {
            id: "reel-shot-b".to_owned(),
            video_id: "video-reel".to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            idx: 1,
            start_s: 3.0,
            end_s: 5.0,
            rep_frame_s: 4.0,
            thumb_rel: None,
            scene_score: None,
        },
    ];
    store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
    store
        .plan_create(
            DEFAULT_OWNER_ID,
            &Plan {
                id: "reel-project".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name: "Ordered reel".to_owned(),
                description: String::new(),
                context_key: "render-test".to_owned(),
                brief: String::new(),
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
    for (position, (shot_id, start_s, end_s)) in
        [("reel-shot-a", 0.25, 1.25), ("reel-shot-b", 3.25, 4.25)]
            .into_iter()
            .enumerate()
    {
        store
            .plan_add_item(
                DEFAULT_OWNER_ID,
                &PlanItem {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    plan_id: "reel-project".to_owned(),
                    media_kind: MediaKind::Shot,
                    media_id: shot_id.to_owned(),
                    position: position as i64,
                    start_s: Some(start_s),
                    end_s: Some(end_s),
                    pacing: None,
                    crop_x: None,
                    grade_json: None,
                    reason: String::new(),
                    signals_json: "{}".to_owned(),
                    origin: PlanOrigin::General,
                    rank: None,
                    profile_version: None,
                    provenance_json: "{}".to_owned(),
                    added_at: now,
                },
            )
            .unwrap();
    }
    let revision = store
        .plan_save_revision(DEFAULT_OWNER_ID, "reel-project", "render golden")
        .unwrap();
    store
        .render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: "ordered-reel-mp4".to_owned(),
                version: 1,
                kind: RenderRecipeKind::Reel,
                name: "Ordered reel MP4".to_owned(),
                schema_json: serde_json::json!({
                    "schema_version": 1,
                    "kind": "reel",
                    "transition": {"kind": "cut"},
                    "audio": {"mode": "source"},
                    "output": {"preset": "mp4-h264-sdr-v1"}
                })
                .to_string(),
                created_at: now,
            },
        )
        .unwrap();
    store
        .render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: "render-ordered-reel".to_owned(),
                recipe_id: "ordered-reel-mp4".to_owned(),
                recipe_version: 1,
                plan_id: Some("reel-project".to_owned()),
                plan_revision: Some(revision.revision),
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": "render-test",
                    "selection_provenance": {"origin": "general"},
                    "sources": shots.iter().map(|shot| serde_json::json!({
                        "media_kind": "shot",
                        "media_id": shot.id,
                        "source_id": "video-reel",
                        "sha256": source_hash,
                        "path": source,
                    })).collect::<Vec<_>>()
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used"
                    }
                })
                .to_string(),
                destination_path: destination.to_string_lossy().into_owned(),
                created_at: now,
            },
        )
        .unwrap();
    drop(store);

    let pipeline = Pipeline::new(
        Config {
            data_dir: Some(directory.path().to_path_buf()),
            ..Config::default()
        },
        AppPaths {
            root: directory.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let output = pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "render-ordered-reel")
        .unwrap();

    assert_eq!(output.media_type, "video/mp4");
    assert!((output.duration_s.unwrap() - 2.0).abs() <= 0.12);
    assert_eq!(sha256_file(&source).unwrap(), source_hash);
    assert!(destination.is_file());
    let manifest: serde_json::Value = serde_json::from_str(&output.manifest_json).unwrap();
    assert_eq!(manifest["render"]["preset"], "mp4-h264-sdr-v1");
    assert_eq!(manifest["sources"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["verification"]["sources_unchanged"], true);
    assert_eq!(manifest["verification"]["item_count"], 2);

    // TASK-036 golden — the 021 review's ground truth from the burned-in frame counter.
    // Requested 0.25-1.25s + 3.25-4.25s at 30 fps must deliver source frames 8-37 then
    // 98-127: exactly 60 video frames, no lead dead zone, the cut exactly at 1.0s, and
    // audio that never outlasts the video.
    assert_eq!(manifest["verification"]["video_frame_count"], 60);
    assert_eq!(manifest["verification"]["video_duration_s"], 2.0);
    let items = manifest["verification"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["first_source_frame"], 8);
    assert_eq!(items[0]["last_source_frame"], 37);
    assert_eq!(items[0]["requested_frame_count"], 30);
    assert_eq!(items[0]["rendered_frame_count"], 30);
    assert_eq!(items[1]["first_source_frame"], 98);
    assert_eq!(items[1]["last_source_frame"], 127);
    assert_eq!(items[1]["rendered_frame_count"], 30);
    let audio_duration = manifest["verification"]["audio_duration_s"]
        .as_f64()
        .unwrap();
    assert!(audio_duration <= 2.0 + 0.002);

    let resolved = crush_stage_split::ffmpeg::resolve().unwrap();
    let pts = video_packet_pts(&resolved.ffprobe_path, &destination);
    assert_eq!(pts.len(), 60);
    assert!(pts[0] <= 0.002, "no lead dead zone before the first frame");
    assert!((pts[29] - 29.0 / 30.0).abs() <= 0.002);
    assert!(
        (pts[30] - 1.0).abs() <= 0.002,
        "the cut must land exactly at the previous item's video duration"
    );
    assert!((pts[59] - 59.0 / 30.0).abs() <= 0.002);
    assert_frame_identity(&resolved.path, &source, &destination, 0, 8);
    assert_frame_identity(&resolved.path, &source, &destination, 29, 37);
    assert_frame_identity(&resolved.path, &source, &destination, 30, 98);
    assert_frame_identity(&resolved.path, &source, &destination, 59, 127);

    preserve_review_output(&output, "reel-speech-two-cuts.mp4");
}

#[test]
fn render_jobs_and_sources_are_owner_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let (pipeline, source) = setup_photo_job(
        directory.path(),
        "jpeg-srgb-v1",
        &directory.path().join("exports/owner-a.jpg"),
        "owner-isolation",
    );

    // A different owner cannot see or execute the default owner's recipe or job.
    let owner_b = "owner-b";
    let other_store = Store::open(directory.path()).unwrap();
    assert!(other_store
        .render_job_by_id(owner_b, "owner-isolation")
        .unwrap()
        .is_none());
    assert!(other_store
        .render_recipe_get(owner_b, "owner-isolation-recipe", 1)
        .unwrap()
        .is_none());
    let blocked = pipeline.execute_render_job(owner_b, "owner-isolation");
    assert!(blocked.is_err(), "another owner must not start this render");
    assert!(
        !directory.path().join("exports/owner-a.jpg").exists(),
        "no output may be published for a foreign owner"
    );

    // The owner who created the job publishes a verified derivative and never touches the source.
    let source_hash = sha256_file(&source).unwrap();
    let output = pipeline
        .execute_render_job(DEFAULT_OWNER_ID, "owner-isolation")
        .unwrap();
    assert!(Path::new(&output.output_path).is_file());
    assert!(Path::new(&output.manifest_path).is_file());
    assert_eq!(sha256_file(&source).unwrap(), source_hash);
}
