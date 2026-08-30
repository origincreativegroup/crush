use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_pipeline::{sha256_file, Pipeline};
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
