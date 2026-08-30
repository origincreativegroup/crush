//! Durable, non-destructive execution of frozen render jobs.
//!
//! Recipe intent remains platform neutral. This module records the actual CPU/image backend in
//! the manifest and publishes only fully verified files through exclusive same-filesystem links.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context};
use chrono::Utc;
use crush_store::{
    RenderAttempt, RenderJob, RenderJobStatus, RenderOutput, RenderRecipeKind, Store,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;
use uuid::Uuid;

use crate::source::{
    self, BasicPhotoGrade, NormalizedCrop, PhotoGrade, PhotoOutputPreset, PhotoRenderRecipe,
};
use crate::{sha256_file, Pipeline};

const MANIFEST_SCHEMA_VERSION: u64 = 1;
const EXECUTOR_ID: &str = "crush-photo-cpu-v1";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderRecoverySummary {
    pub finalized: usize,
    pub failed: usize,
    pub staging_removed: usize,
}

#[derive(Debug)]
struct PhotoSourceSnapshot {
    media_id: String,
    source_id: String,
    sha256: String,
    frozen_path: PathBuf,
}

#[derive(Debug)]
struct ManagedStaging {
    directory: TempDir,
    output: PathBuf,
    manifest: PathBuf,
    marker: PathBuf,
}

impl ManagedStaging {
    fn create(destination: &Path) -> anyhow::Result<Self> {
        reject_existing(destination, "render destination")?;
        reject_existing(&manifest_path(destination), "render manifest destination")?;
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .context("render destination must have a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create export directory {}", parent.display()))?;
        let directory = tempfile::Builder::new()
            .prefix(".crush-render-")
            .tempdir_in(parent)
            .context("failed to create managed render staging")?;
        let filename = destination
            .file_name()
            .context("render destination must name a file")?;
        let output = directory.path().join(filename);
        let manifest = directory.path().join(
            manifest_path(destination)
                .file_name()
                .context("render manifest must name a file")?,
        );
        let marker = directory.path().join("marker.json");
        Ok(Self {
            directory,
            output,
            manifest,
            marker,
        })
    }

    fn write_marker(
        &self,
        owner_id: &str,
        job_id: &str,
        attempt: i64,
        destination: &Path,
    ) -> anyhow::Result<()> {
        write_new_synced(
            &self.marker,
            serde_json::to_string_pretty(&json!({
                "schema_version": 1,
                "owner_id": owner_id,
                "job_id": job_id,
                "attempt": attempt,
                "destination": destination,
            }))?
            .as_bytes(),
        )
    }
}

impl Pipeline {
    /// Execute one frozen photo job. Video and reel jobs deliberately fail as unsupported until
    /// their backend-neutral renderers and golden verifiers land later in Task 021.
    pub fn execute_render_job(&self, owner_id: &str, job_id: &str) -> anyhow::Result<RenderOutput> {
        let mut store = Store::open(&self.paths.root)?;
        let job = store
            .render_job_by_id(owner_id, job_id)?
            .with_context(|| format!("render job {job_id} was not found for this owner"))?;
        ensure!(
            job.recipe_kind == RenderRecipeKind::Photo,
            "{} render execution is not implemented yet",
            recipe_kind_name(job.recipe_kind)
        );
        ensure!(
            matches!(
                job.status,
                RenderJobStatus::Queued | RenderJobStatus::Failed | RenderJobStatus::Cancelled
            ),
            "render job {job_id} is not ready to start"
        );
        ensure!(
            !self.cancellation.is_cancelled(),
            "photo render was cancelled before it started"
        );

        let recipe = parse_photo_recipe(&job.frozen_recipe_json)?;
        let source_snapshot = parse_photo_source_snapshot(&job.source_snapshot_json)?;
        let photo = store
            .photo_by_id(owner_id, &source_snapshot.media_id)?
            .with_context(|| {
                format!(
                    "frozen photo {} no longer exists for this owner",
                    source_snapshot.media_id
                )
            })?;
        ensure!(
            source_snapshot.source_id == photo.id,
            "frozen photo source identity does not match the owner-scoped library record"
        );
        ensure!(
            photo.sha256.eq_ignore_ascii_case(&source_snapshot.sha256),
            "library photo hash changed after this render was queued"
        );
        let source_path = PathBuf::from(&photo.path);
        ensure!(
            source_path.is_absolute(),
            "library photo path is not absolute"
        );
        let source_hash_before = sha256_file(&source_path)
            .with_context(|| format!("failed to hash render source {}", source_path.display()))?;
        ensure!(
            source_hash_before.eq_ignore_ascii_case(&source_snapshot.sha256),
            "photo source bytes changed after this render was queued"
        );

        let destination = PathBuf::from(&job.destination_path);
        validate_photo_destination(&destination, recipe.output)?;
        let staging = ManagedStaging::create(&destination)?;
        let attempt = store.render_job_start(
            owner_id,
            job_id,
            &staging.output.to_string_lossy(),
            Utc::now(),
        )?;
        let execution = (|| {
            staging.write_marker(owner_id, job_id, attempt.attempt, &destination)?;
            store.render_attempt_set_commands(
                owner_id,
                job_id,
                attempt.attempt,
                &json!([{
                    "executor": EXECUTOR_ID,
                    "phase": "started",
                    "backend": "cpu",
                    "staging_output": staging.output,
                    "destination": destination,
                }])
                .to_string(),
            )?;
            self.execute_photo_attempt(
                &mut store,
                owner_id,
                &job,
                &attempt,
                &source_snapshot,
                &source_path,
                &destination,
                &staging,
                &recipe,
            )
        })();
        match execution {
            Ok(output) => Ok(output),
            Err(error) => {
                let current = store.render_job_by_id(owner_id, job_id)?;
                if let Some(current) = current {
                    if matches!(
                        current.status,
                        RenderJobStatus::Running | RenderJobStatus::Verifying
                    ) {
                        if self.cancellation.is_cancelled() {
                            let _ = store.render_job_cancel(owner_id, job_id, Utc::now());
                        } else if destination.is_file()
                            && manifest_path(&destination).is_file()
                            && current.status == RenderJobStatus::Verifying
                        {
                            // Publication completed but SQLite finalization did not. Leave the
                            // job verifying so startup recovery can validate and finish it.
                        } else {
                            let _ = store.render_job_fail(
                                owner_id,
                                job_id,
                                &format!("{error:#}"),
                                Utc::now(),
                            );
                        }
                    }
                }
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_photo_attempt(
        &self,
        store: &mut Store,
        owner_id: &str,
        job: &RenderJob,
        attempt: &RenderAttempt,
        source_snapshot: &PhotoSourceSnapshot,
        source_path: &Path,
        destination: &Path,
        staging: &ManagedStaging,
        recipe: &PhotoRenderRecipe,
    ) -> anyhow::Result<RenderOutput> {
        ensure!(!self.cancellation.is_cancelled(), "photo render cancelled");
        store.render_job_set_progress(owner_id, &job.id, 0.1)?;
        let decoded = source::decode_photo(source_path, &self.cancellation)?;
        ensure!(!self.cancellation.is_cancelled(), "photo render cancelled");
        let rendered = source::render_photo_derivative(&decoded, recipe, &staging.output)?;
        store.render_job_set_progress(owner_id, &job.id, 0.75)?;

        let source_hash_after = sha256_file(source_path)?;
        ensure!(
            source_hash_after.eq_ignore_ascii_case(&source_snapshot.sha256),
            "photo source changed while it was rendering"
        );
        ensure!(!self.cancellation.is_cancelled(), "photo render cancelled");
        let output_size = i64::try_from(fs::metadata(&staging.output)?.len())
            .context("render output size overflowed i64")?;
        let output_sha256 = sha256_file(&staging.output)?;
        ensure!(
            output_sha256 == rendered.derivative.sha256,
            "photo renderer checksum did not match verification checksum"
        );
        let media_type = preset_media_type(recipe.output).to_owned();
        let verification = json!({
            "source_hash_before": source_hash_before(source_snapshot),
            "source_hash_after": source_hash_after,
            "source_unchanged": true,
            "dimensions": {
                "width": rendered.derivative.width,
                "height": rendered.derivative.height,
            },
            "orientation_applied_once": decoded.orientation_applied,
            "output_color_space": rendered.output_color_space,
            "output_bit_depth": rendered.output_bit_depth,
            "metadata_policy": rendered.metadata_policy,
        });
        let created_at = Utc::now();
        let manifest_destination = manifest_path(destination);
        let manifest = json!({
            "schema_version": MANIFEST_SCHEMA_VERSION,
            "job": {
                "id": job.id,
                "attempt": attempt.attempt,
                "owner_id": owner_id,
                "created_at": created_at,
            },
            "source": {
                "media_kind": "photo",
                "media_id": source_snapshot.media_id,
                "source_id": source_snapshot.source_id,
                "sha256": source_snapshot.sha256,
                "frozen_path": source_snapshot.frozen_path,
                "resolved_path": source_path,
            },
            "frozen_recipe": serde_json::from_str::<Value>(&job.frozen_recipe_json)?,
            "frozen_plan": job.frozen_plan_json.as_deref().map(serde_json::from_str::<Value>).transpose()?,
            "model_versions": serde_json::from_str::<Value>(&job.model_versions_json)?,
            "tool_versions": {
                "crush_pipeline": env!("CARGO_PKG_VERSION"),
                "executor": EXECUTOR_ID,
                "decoder": decoded.decoder,
                "backend": "cpu",
            },
            "render": {
                "preset": rendered.preset,
                "source_color_handling": rendered.source_color_handling,
                "output_path": destination,
                "media_type": media_type,
                "checksum_sha256": output_sha256,
                "size_bytes": output_size,
            },
            "verification": verification,
        });
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        write_new_synced(&staging.manifest, manifest_json.as_bytes())?;
        let manifest_sha256 = sha256_file(&staging.manifest)?;
        File::open(&staging.output)?.sync_all()?;

        let output = RenderOutput {
            owner_id: owner_id.to_owned(),
            id: format!("render-output-{}", Uuid::new_v4()),
            job_id: job.id.clone(),
            attempt: attempt.attempt,
            output_path: destination.to_string_lossy().into_owned(),
            output_sha256,
            size_bytes: output_size,
            media_type,
            width: Some(i64::from(rendered.derivative.width)),
            height: Some(i64::from(rendered.derivative.height)),
            duration_s: None,
            verification_json: verification.to_string(),
            manifest_path: manifest_destination.to_string_lossy().into_owned(),
            manifest_json,
            manifest_sha256,
            created_at,
        };
        store.render_attempt_set_commands(
            owner_id,
            &job.id,
            attempt.attempt,
            &recovery_command_json(&output, &staging.manifest),
        )?;
        store.render_job_mark_verifying(owner_id, &job.id)?;

        reject_existing(&manifest_destination, "render manifest destination")?;
        reject_existing(destination, "render destination")?;
        // Publishing the manifest first keeps an incomplete media output invisible. If the media
        // link loses a race, roll back only the manifest link this invocation just created.
        fs::hard_link(&staging.manifest, &manifest_destination)
            .context("filesystem does not support exclusive render-manifest publication")?;
        if let Err(error) = fs::hard_link(&staging.output, destination) {
            let _ = fs::remove_file(&manifest_destination);
            return Err(error).context("filesystem does not support exclusive render publication");
        }
        sync_parent(destination)?;
        store.render_job_finish(owner_id, &output)?;
        tracing::info!(
            job_id = %job.id,
            output = %destination.display(),
            manifest = %manifest_destination.display(),
            "verified photo render published without overwrite"
        );
        // Make the use of the retained guard explicit: it cleans the private staging directory
        // on normal completion/error, while process death leaves the tracked path for recovery.
        let _ = staging.directory.path();
        Ok(output)
    }

    /// Reconcile attempts left active by process death. Verified publications are finalized from
    /// their checksummed recovery evidence; unverified managed staging is removed and the job is
    /// made explicitly failed/retryable.
    pub fn recover_interrupted_render_jobs(
        &self,
        owner_id: &str,
    ) -> anyhow::Result<RenderRecoverySummary> {
        let mut store = Store::open(&self.paths.root)?;
        let mut summary = RenderRecoverySummary::default();
        let mut active = store.render_jobs(owner_id, Some(RenderJobStatus::Running))?;
        active.extend(store.render_jobs(owner_id, Some(RenderJobStatus::Verifying))?);
        for job in active {
            let attempt = store
                .render_attempt(owner_id, &job.id, job.current_attempt)?
                .with_context(|| format!("active render {} has no attempt", job.id))?;
            if job.status == RenderJobStatus::Verifying {
                if let Ok(output) = parse_recovery_output(&attempt.command_json) {
                    if verified_publication_matches(&output)? {
                        store.render_job_finish(owner_id, &output)?;
                        cleanup_managed_staging(owner_id, &job, &attempt, &mut summary)?;
                        summary.finalized += 1;
                        continue;
                    }
                    remove_owned_manifest_only(&output)?;
                }
            }
            let staging_removed = cleanup_managed_staging(owner_id, &job, &attempt, &mut summary)?;
            store.render_job_fail(
                owner_id,
                &job.id,
                if staging_removed {
                    "render was interrupted before a complete verified publication"
                } else {
                    "render was interrupted; unrecognized staging was preserved for safety"
                },
                Utc::now(),
            )?;
            summary.failed += 1;
        }
        Ok(summary)
    }
}

fn parse_photo_recipe(frozen: &str) -> anyhow::Result<PhotoRenderRecipe> {
    let frozen: Value = serde_json::from_str(frozen).context("frozen recipe is invalid JSON")?;
    let object = frozen
        .as_object()
        .context("frozen recipe must be an object")?;
    let schema = object
        .get("schema")
        .and_then(Value::as_object)
        .context("frozen recipe schema is missing")?;
    ensure!(
        schema.get("schema_version").and_then(Value::as_u64) == Some(1),
        "photo executor supports only recipe schema version 1"
    );
    ensure!(schema.get("kind").and_then(Value::as_str) == Some("photo"));
    let crop = match schema.get("crop") {
        Some(Value::Null) => None,
        Some(value) => {
            let crop = value.as_object().context("photo crop must be an object")?;
            Some(NormalizedCrop {
                x: required_f64(crop, "x", "photo crop")?,
                y: required_f64(crop, "y", "photo crop")?,
                width: required_f64(crop, "width", "photo crop")?,
                height: required_f64(crop, "height", "photo crop")?,
            })
        }
        None => bail!("photo crop is missing"),
    };
    let rotation_degrees = schema
        .get("rotation_degrees")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .context("photo rotation_degrees is invalid")?;
    let grade_object = schema
        .get("grade")
        .and_then(Value::as_object)
        .context("photo grade is missing")?;
    let grade = match grade_object.get("mode").and_then(Value::as_str) {
        Some("none") => PhotoGrade::None,
        Some("basic") => PhotoGrade::Basic(BasicPhotoGrade {
            exposure_ev: required_f64(grade_object, "exposure_ev", "photo grade")?,
            contrast: required_f64(grade_object, "contrast", "photo grade")?,
            saturation: required_f64(grade_object, "saturation", "photo grade")?,
            temperature: required_f64(grade_object, "temperature", "photo grade")?,
            tint: required_f64(grade_object, "tint", "photo grade")?,
        }),
        Some(other) => bail!("unsupported photo grade mode {other:?}"),
        None => bail!("photo grade mode is missing"),
    };
    let preset = schema
        .get("output")
        .and_then(Value::as_object)
        .and_then(|output| output.get("preset"))
        .and_then(Value::as_str)
        .context("photo output preset is missing")?;
    let output = match preset {
        "jpeg-srgb-v1" => PhotoOutputPreset::JpegSrgbV1,
        "png-srgb-v1" => PhotoOutputPreset::PngSrgbV1,
        "tiff-srgb-v1" => PhotoOutputPreset::TiffSrgbV1,
        other => bail!("unsupported photo output preset {other:?}"),
    };
    Ok(PhotoRenderRecipe {
        crop,
        rotation_degrees,
        grade,
        output,
    })
}

fn parse_photo_source_snapshot(value: &str) -> anyhow::Result<PhotoSourceSnapshot> {
    let value: Value = serde_json::from_str(value).context("source snapshot is invalid JSON")?;
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .context("source snapshot sources are missing")?;
    ensure!(
        sources.len() == 1,
        "a photo recipe requires exactly one frozen source"
    );
    let source = sources[0]
        .as_object()
        .context("photo source snapshot must be an object")?;
    ensure!(
        source.get("media_kind").and_then(Value::as_str) == Some("photo"),
        "photo recipe source must have media_kind photo"
    );
    Ok(PhotoSourceSnapshot {
        media_id: required_string(source, "media_id", "photo source")?.to_owned(),
        source_id: required_string(source, "source_id", "photo source")?.to_owned(),
        sha256: required_string(source, "sha256", "photo source")?.to_owned(),
        frozen_path: PathBuf::from(required_string(source, "path", "photo source")?),
    })
}

fn required_f64(object: &Map<String, Value>, key: &str, name: &str) -> anyhow::Result<f64> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("{name} {key} must be a number"))?;
    ensure!(value.is_finite(), "{name} {key} must be finite");
    Ok(value)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
) -> anyhow::Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("{name} {key} is required"))
}

fn validate_photo_destination(path: &Path, preset: PhotoOutputPreset) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .context("photo render destination needs a file extension")?;
    let valid = match preset {
        PhotoOutputPreset::JpegSrgbV1 => {
            extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
        }
        PhotoOutputPreset::PngSrgbV1 => extension.eq_ignore_ascii_case("png"),
        PhotoOutputPreset::TiffSrgbV1 => {
            extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff")
        }
    };
    ensure!(
        valid,
        "destination extension does not match preset {}",
        preset.as_str()
    );
    Ok(())
}

fn preset_media_type(preset: PhotoOutputPreset) -> &'static str {
    match preset {
        PhotoOutputPreset::JpegSrgbV1 => "image/jpeg",
        PhotoOutputPreset::PngSrgbV1 => "image/png",
        PhotoOutputPreset::TiffSrgbV1 => "image/tiff",
    }
}

fn manifest_path(destination: &Path) -> PathBuf {
    let filename = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    destination.with_file_name(format!("{filename}.crush-manifest.json"))
}

fn reject_existing(path: &Path, name: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("{name} already exists: {}", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sync_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn source_hash_before(source: &PhotoSourceSnapshot) -> &str {
    &source.sha256
}

fn recipe_kind_name(kind: RenderRecipeKind) -> &'static str {
    match kind {
        RenderRecipeKind::Photo => "photo",
        RenderRecipeKind::VideoClip => "video clip",
        RenderRecipeKind::Reel => "reel",
    }
}

fn recovery_command_json(output: &RenderOutput, staging_manifest: &Path) -> String {
    json!([{
        "executor": EXECUTOR_ID,
        "phase": "verified_staging",
        "staging_manifest": staging_manifest,
        "output": {
            "owner_id": output.owner_id,
            "id": output.id,
            "job_id": output.job_id,
            "attempt": output.attempt,
            "output_path": output.output_path,
            "output_sha256": output.output_sha256,
            "size_bytes": output.size_bytes,
            "media_type": output.media_type,
            "width": output.width,
            "height": output.height,
            "duration_s": output.duration_s,
            "verification_json": output.verification_json,
            "manifest_path": output.manifest_path,
            "manifest_json": output.manifest_json,
            "manifest_sha256": output.manifest_sha256,
            "created_at": output.created_at,
        }
    }])
    .to_string()
}

fn parse_recovery_output(command_json: &str) -> anyhow::Result<RenderOutput> {
    let commands: Value = serde_json::from_str(command_json)?;
    let command = commands
        .as_array()
        .and_then(|commands| commands.first())
        .and_then(Value::as_object)
        .context("render recovery command is missing")?;
    ensure!(
        command.get("phase").and_then(Value::as_str) == Some("verified_staging"),
        "render attempt did not reach verified staging"
    );
    let output = command
        .get("output")
        .and_then(Value::as_object)
        .context("render recovery output evidence is missing")?;
    Ok(RenderOutput {
        owner_id: required_string(output, "owner_id", "recovery output")?.to_owned(),
        id: required_string(output, "id", "recovery output")?.to_owned(),
        job_id: required_string(output, "job_id", "recovery output")?.to_owned(),
        attempt: output
            .get("attempt")
            .and_then(Value::as_i64)
            .context("recovery output attempt is missing")?,
        output_path: required_string(output, "output_path", "recovery output")?.to_owned(),
        output_sha256: required_string(output, "output_sha256", "recovery output")?.to_owned(),
        size_bytes: output
            .get("size_bytes")
            .and_then(Value::as_i64)
            .context("recovery output size_bytes is missing")?,
        media_type: required_string(output, "media_type", "recovery output")?.to_owned(),
        width: optional_i64(output, "width")?,
        height: optional_i64(output, "height")?,
        duration_s: optional_f64(output, "duration_s")?,
        verification_json: required_string(output, "verification_json", "recovery output")?
            .to_owned(),
        manifest_path: required_string(output, "manifest_path", "recovery output")?.to_owned(),
        manifest_json: required_string(output, "manifest_json", "recovery output")?.to_owned(),
        manifest_sha256: required_string(output, "manifest_sha256", "recovery output")?.to_owned(),
        created_at: required_string(output, "created_at", "recovery output")?
            .parse()
            .context("recovery output created_at is invalid")?,
    })
}

fn optional_i64(object: &Map<String, Value>, key: &str) -> anyhow::Result<Option<i64>> {
    match object.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .with_context(|| format!("recovery output {key} is invalid")),
    }
}

fn optional_f64(object: &Map<String, Value>, key: &str) -> anyhow::Result<Option<f64>> {
    match object.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .with_context(|| format!("recovery output {key} is invalid")),
    }
}

fn verified_publication_matches(output: &RenderOutput) -> anyhow::Result<bool> {
    let output_path = Path::new(&output.output_path);
    let manifest_path = Path::new(&output.manifest_path);
    if !output_path.is_file() || !manifest_path.is_file() {
        return Ok(false);
    }
    Ok(
        sha256_file(output_path)?.eq_ignore_ascii_case(&output.output_sha256)
            && sha256_file(manifest_path)?.eq_ignore_ascii_case(&output.manifest_sha256)
            && fs::metadata(output_path)?.len()
                == u64::try_from(output.size_bytes).unwrap_or(u64::MAX),
    )
}

fn remove_owned_manifest_only(output: &RenderOutput) -> anyhow::Result<()> {
    let output_path = Path::new(&output.output_path);
    let manifest_path = Path::new(&output.manifest_path);
    if output_path.exists() || !manifest_path.is_file() {
        return Ok(());
    }
    if !sha256_file(manifest_path)?.eq_ignore_ascii_case(&output.manifest_sha256) {
        return Ok(());
    }
    let manifest: Value = serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
    let belongs_to_attempt = manifest.pointer("/job/id").and_then(Value::as_str)
        == Some(output.job_id.as_str())
        && manifest.pointer("/job/attempt").and_then(Value::as_i64) == Some(output.attempt);
    if belongs_to_attempt {
        fs::remove_file(manifest_path)
            .context("failed to remove interrupted manifest-only publication")?;
        sync_parent(manifest_path)?;
    }
    Ok(())
}

fn cleanup_managed_staging(
    owner_id: &str,
    job: &RenderJob,
    attempt: &RenderAttempt,
    summary: &mut RenderRecoverySummary,
) -> anyhow::Result<bool> {
    let staging_file = Path::new(&attempt.staging_path);
    let staging_dir = staging_file
        .parent()
        .context("tracked render staging path has no parent")?;
    let destination_parent = Path::new(&job.destination_path)
        .parent()
        .context("render destination has no parent")?;
    let managed_name = staging_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".crush-render-"));
    if !managed_name || staging_dir.parent() != Some(destination_parent) {
        return Ok(false);
    }
    let marker_path = staging_dir.join("marker.json");
    let marker: Value = match fs::read_to_string(&marker_path)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
    {
        Some(marker) => marker,
        None => return Ok(false),
    };
    let marker_matches = marker.get("schema_version").and_then(Value::as_u64) == Some(1)
        && marker.get("owner_id").and_then(Value::as_str) == Some(owner_id)
        && marker.get("job_id").and_then(Value::as_str) == Some(job.id.as_str())
        && marker.get("attempt").and_then(Value::as_i64) == Some(attempt.attempt)
        && marker.get("destination").and_then(Value::as_str) == Some(job.destination_path.as_str());
    if !marker_matches {
        return Ok(false);
    }
    match fs::remove_dir_all(staging_dir) {
        Ok(()) => summary.staging_removed += 1,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to remove interrupted render staging"),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_a_sibling_and_destination_extensions_are_strict() {
        assert_eq!(
            manifest_path(Path::new("/tmp/hero.jpg")),
            PathBuf::from("/tmp/hero.jpg.crush-manifest.json")
        );
        validate_photo_destination(Path::new("/tmp/hero.JPEG"), PhotoOutputPreset::JpegSrgbV1)
            .unwrap();
        assert!(validate_photo_destination(
            Path::new("/tmp/hero.png"),
            PhotoOutputPreset::JpegSrgbV1
        )
        .is_err());
    }

    #[test]
    fn frozen_photo_recipe_parser_preserves_every_edit() {
        let recipe = parse_photo_recipe(
            &json!({
                "id": "photo-web",
                "version": 1,
                "kind": "photo",
                "name": "Photo web",
                "schema": {
                    "schema_version": 1,
                    "kind": "photo",
                    "crop": {"x": 0.1, "y": 0.2, "width": 0.7, "height": 0.6},
                    "rotation_degrees": 90,
                    "grade": {
                        "mode": "basic",
                        "exposure_ev": 0.5,
                        "contrast": 0.1,
                        "saturation": 1.1,
                        "temperature": -0.2,
                        "tint": 0.3
                    },
                    "output": {"preset": "png-srgb-v1"}
                }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(recipe.rotation_degrees, 90);
        assert_eq!(recipe.output, PhotoOutputPreset::PngSrgbV1);
        assert!(matches!(recipe.grade, PhotoGrade::Basic(_)));
        assert_eq!(recipe.crop.unwrap().width, 0.7);
    }
}
