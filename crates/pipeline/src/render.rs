//! Durable, non-destructive execution of frozen render jobs.
//!
//! Recipe intent remains platform neutral. This module records the actual CPU/image backend in
//! the manifest and publishes only fully verified files through exclusive same-filesystem links.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context};
use chrono::Utc;
use crush_stage_split::ffmpeg::{
    self, BasicVideoGrade, ClipAudio, ClipOutputPreset, ClipRenderRequest, ClipTransition,
    NormalizedVideoCrop, VideoGrade,
};
use crush_stage_split::reel::{
    ReelFormat, ReelMediaKind, ReelMotion, ResolvedReelGrade, ResolvedReelItem,
    ResolvedReelRequest, ResolvedReelTransition,
};
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
struct VideoSourceSnapshot {
    media_kind: String,
    media_id: String,
    source_id: String,
    sha256: String,
    frozen_path: PathBuf,
}

#[derive(Debug)]
struct ResolvedVideoSource {
    path: PathBuf,
    has_audio: bool,
}

#[derive(Debug)]
struct ReelV1Recipe {
    audio: ClipAudio,
    output: ClipOutputPreset,
}

#[derive(Debug)]
struct FrozenReelPlanItem {
    /// `shot` or `span` (an imported/manual span, Task 022).
    media_kind: String,
    media_id: String,
    start_s: f64,
    end_s: f64,
    grade: ResolvedReelGrade,
}

#[derive(Debug)]
struct ResolvedReelSources {
    request: ResolvedReelRequest,
    snapshots: Vec<VideoSourceSnapshot>,
    resolved_paths: BTreeMap<String, PathBuf>,
}

#[derive(Debug)]
struct ManagedStaging {
    directory: TempDir,
    output: PathBuf,
    manifest: PathBuf,
    marker: PathBuf,
}

/// Throttled, monotonic job-progress writer fed by the ffmpeg `-progress pipe:1` callbacks
/// (TASK-035 item 4; the TASK-040 B10 item). The renderer's overall `Progress.percent` —
/// measured out_time against the requested duration, mapped by the reel renderer across items,
/// remuxes, and assembly — lands in the job's 0.1..0.75 window; 1.0 stays reserved for
/// verification and the executor's final 0.75 write is unchanged. Writes are throttled
/// (one SQLite immediate transaction per update), monotonic, and honestly limited to what the
/// executor can measure: if the store refuses an update, further attempts stop and the render
/// continues — progress is advisory, the durable guards remain the contract.
struct JobProgressWriter<'a> {
    store: &'a mut Store,
    owner_id: &'a str,
    job_id: &'a str,
    last_progress: f64,
    last_write: Option<Instant>,
    stopped: bool,
}

impl JobProgressWriter<'_> {
    const BASE: f64 = 0.1;
    const SPAN: f64 = 0.65;
    const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(250);

    fn new<'a>(store: &'a mut Store, owner_id: &'a str, job_id: &'a str) -> JobProgressWriter<'a> {
        JobProgressWriter {
            store,
            owner_id,
            job_id,
            last_progress: Self::BASE,
            last_write: None,
            stopped: false,
        }
    }

    fn record(&mut self, progress: &ffmpeg::Progress) {
        if self.stopped {
            return;
        }
        let mapped = Self::BASE + Self::SPAN * (progress.percent / 100.0).clamp(0.0, 1.0);
        if mapped <= self.last_progress {
            return;
        }
        let now = Instant::now();
        if self
            .last_write
            .is_some_and(|last| now.duration_since(last) < Self::MIN_WRITE_INTERVAL)
        {
            return;
        }
        match self
            .store
            .render_job_set_progress(self.owner_id, self.job_id, mapped)
        {
            Ok(()) => {
                self.last_progress = mapped;
                self.last_write = Some(now);
            }
            Err(error) => {
                self.stopped = true;
                tracing::warn!(
                    job_id = %self.job_id,
                    stage = "render",
                    error = %error,
                    "render progress updates stopped after a refused write"
                );
            }
        }
    }
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

fn settle_render_execution_error(
    store: &mut Store,
    owner_id: &str,
    job_id: &str,
    destination: &Path,
    cancelled: bool,
    execution_error: anyhow::Error,
) -> anyhow::Error {
    let current = match store.render_job_by_id(owner_id, job_id) {
        Ok(current) => current,
        Err(state_error) => {
            return anyhow::anyhow!(
                "render execution failed: {execution_error:#}; durable job state could not be read: {state_error:#}"
            )
        }
    };
    let Some(current) = current else {
        return execution_error;
    };
    if !matches!(
        current.status,
        RenderJobStatus::Running | RenderJobStatus::Verifying
    ) {
        return execution_error;
    }

    // Once both public files exist, cancellation must not relabel a checksummed publication as
    // cancelled. Keep it verifying so recovery can validate the pair and finish atomically.
    if current.status == RenderJobStatus::Verifying
        && destination.is_file()
        && manifest_path(destination).is_file()
    {
        return execution_error;
    }

    let transition = if cancelled {
        store.render_job_cancel(owner_id, job_id, Utc::now())
    } else {
        store.render_job_fail(
            owner_id,
            job_id,
            &format!("{execution_error:#}"),
            Utc::now(),
        )
    };
    match transition {
        Ok(()) => execution_error,
        Err(state_error) => anyhow::anyhow!(
            "render execution failed: {execution_error:#}; durable failure state could not be persisted: {state_error:#}"
        ),
    }
}

impl Pipeline {
    /// Execute one frozen photo, clip, or initial ordered-reel job.
    pub fn execute_render_job(&self, owner_id: &str, job_id: &str) -> anyhow::Result<RenderOutput> {
        let mut store = Store::open(&self.paths.root)?;
        let job = store
            .render_job_by_id(owner_id, job_id)?
            .with_context(|| format!("render job {job_id} was not found for this owner"))?;
        ensure!(
            matches!(
                job.status,
                RenderJobStatus::Queued | RenderJobStatus::Failed | RenderJobStatus::Cancelled
            ),
            "render job {job_id} is not ready to start"
        );
        ensure!(
            !self.cancellation.is_cancelled(),
            "render was cancelled before it started"
        );
        match job.recipe_kind {
            RenderRecipeKind::VideoClip => {
                return self.execute_video_clip_job(&mut store, owner_id, &job)
            }
            RenderRecipeKind::Reel => return self.execute_reel_job(&mut store, owner_id, &job),
            RenderRecipeKind::Photo => {}
        }

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
            Err(error) => Err(settle_render_execution_error(
                &mut store,
                owner_id,
                job_id,
                &destination,
                self.cancellation.is_cancelled(),
                error,
            )),
        }
    }

    fn execute_video_clip_job(
        &self,
        store: &mut Store,
        owner_id: &str,
        job: &RenderJob,
    ) -> anyhow::Result<RenderOutput> {
        let recipe = parse_video_clip_recipe(&job.frozen_recipe_json)?;
        let source_snapshot = parse_video_source_snapshot(&job.source_snapshot_json)?;
        let resolved = resolve_video_source(store, owner_id, &source_snapshot, &recipe)?;
        let source_hash_before = sha256_file(&resolved.path)
            .with_context(|| format!("failed to hash render source {}", resolved.path.display()))?;
        ensure!(
            source_hash_before.eq_ignore_ascii_case(&source_snapshot.sha256),
            "video source bytes changed after this render was queued"
        );
        let destination = PathBuf::from(&job.destination_path);
        validate_video_destination(&destination, recipe.output)?;
        let staging = ManagedStaging::create(&destination)?;
        let attempt = store.render_job_start(
            owner_id,
            &job.id,
            &staging.output.to_string_lossy(),
            Utc::now(),
        )?;
        let execution = (|| {
            staging.write_marker(owner_id, &job.id, attempt.attempt, &destination)?;
            store.render_attempt_set_commands(
                owner_id,
                &job.id,
                attempt.attempt,
                &json!([{
                    "executor": "crush-video-clip-v1",
                    "phase": "started",
                    "staging_output": staging.output,
                    "destination": destination,
                }])
                .to_string(),
            )?;
            self.execute_video_clip_attempt(
                store,
                owner_id,
                job,
                &attempt,
                &source_snapshot,
                &resolved,
                &destination,
                &staging,
                &recipe,
            )
        })();
        match execution {
            Ok(output) => Ok(output),
            Err(error) => Err(settle_render_execution_error(
                store,
                owner_id,
                &job.id,
                &destination,
                self.cancellation.is_cancelled(),
                error,
            )),
        }
    }

    fn execute_reel_job(
        &self,
        store: &mut Store,
        owner_id: &str,
        job: &RenderJob,
    ) -> anyhow::Result<RenderOutput> {
        let recipe = parse_reel_v1_recipe(&job.frozen_recipe_json)?;
        let plan = job
            .frozen_plan_json
            .as_deref()
            .context("reel job has no frozen project revision")?;
        let sources = resolve_reel_v1_sources(store, owner_id, job, plan, &recipe)?;
        let destination = PathBuf::from(&job.destination_path);
        validate_video_destination(&destination, recipe.output)?;
        let staging = ManagedStaging::create(&destination)?;
        let attempt = store.render_job_start(
            owner_id,
            &job.id,
            &staging.output.to_string_lossy(),
            Utc::now(),
        )?;
        let execution = (|| {
            staging.write_marker(owner_id, &job.id, attempt.attempt, &destination)?;
            store.render_attempt_set_commands(
                owner_id,
                &job.id,
                attempt.attempt,
                &json!([{
                    "executor": "crush-video-reel-v1",
                    "phase": "started",
                    "staging_output": staging.output,
                    "destination": destination,
                }])
                .to_string(),
            )?;
            self.execute_reel_attempt(
                store,
                owner_id,
                job,
                &attempt,
                &sources,
                &destination,
                &staging,
            )
        })();
        match execution {
            Ok(output) => Ok(output),
            Err(error) => Err(settle_render_execution_error(
                store,
                owner_id,
                &job.id,
                &destination,
                self.cancellation.is_cancelled(),
                error,
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_reel_attempt(
        &self,
        store: &mut Store,
        owner_id: &str,
        job: &RenderJob,
        attempt: &RenderAttempt,
        sources: &ResolvedReelSources,
        destination: &Path,
        staging: &ManagedStaging,
    ) -> anyhow::Result<RenderOutput> {
        store.render_job_set_progress(owner_id, &job.id, 0.1)?;
        let runner = ffmpeg::Runner::new(
            ffmpeg::resolve()?,
            self.config.limits.threads,
            job.id.clone(),
        );
        let rendered = {
            let mut progress_writer = JobProgressWriter::new(store, owner_id, &job.id);
            runner.render_reel_with_control(
                &sources.request,
                &staging.output,
                &self.cancellation,
                |progress| progress_writer.record(&progress),
            )?
        };
        store.render_job_set_progress(owner_id, &job.id, 0.75)?;
        ensure!(!self.cancellation.is_cancelled(), "reel render cancelled");

        let mut source_evidence = Vec::with_capacity(sources.snapshots.len());
        // After-pass source-hash memo: each distinct source path is re-read and hashed once
        // per attempt (not once per item). This is deliberately a FRESH read — the point of
        // hash_after is to measure that the bytes did not change while the render ran, so it
        // never reuses the before-pass memoized values.
        let mut hash_after_cache: BTreeMap<PathBuf, String> = BTreeMap::new();
        for snapshot in &sources.snapshots {
            let path = sources
                .resolved_paths
                .get(&snapshot.media_id)
                .context("resolved reel source path is missing")?;
            let hash_after = match hash_after_cache.get(path) {
                Some(hash) => hash.clone(),
                None => {
                    let hash = sha256_file(path).with_context(|| {
                        format!("failed to recheck reel source {}", path.display())
                    })?;
                    hash_after_cache.insert(path.clone(), hash.clone());
                    hash
                }
            };
            ensure!(
                hash_after.eq_ignore_ascii_case(&snapshot.sha256),
                "reel source changed while it was rendering"
            );
            source_evidence.push(json!({
                "media_kind": snapshot.media_kind,
                "media_id": snapshot.media_id,
                "source_id": snapshot.source_id,
                "sha256": snapshot.sha256,
                "frozen_path": snapshot.frozen_path,
                "resolved_path": path,
                "hash_after": hash_after,
            }));
        }

        let output_size = i64::try_from(fs::metadata(&staging.output)?.len())
            .context("render output size overflowed i64")?;
        let output_sha256 = sha256_file(&staging.output)?;
        // Reel duration rule: N independent frame-boundary cuts, so the tolerance is the sum
        // of per-item frame slacks plus the shared container slack
        // (`DURATION_TOLERANCE_SLACK_S + items / fps`), per the documented rule in
        // `crush_stage_split::ffmpeg::duration_tolerance_s`.
        let duration_tolerance_s = if rendered.output_probe.fps > 0.0 {
            ffmpeg::DURATION_TOLERANCE_SLACK_S
                + sources.request.items.len() as f64 / rendered.output_probe.fps
        } else {
            0.1
        };
        let duration_delta_s =
            (rendered.output_probe.duration_s - rendered.requested_duration_s).abs();
        ensure!(
            duration_delta_s <= duration_tolerance_s,
            "rendered reel duration differs from requested duration beyond frame tolerance"
        );
        // TASK-036: the container duration can be padded by audio; the video stream is the
        // content contract. The renderer already fails closed on these facts — re-assert
        // them here so a durable job can never publish without frame-exact evidence.
        let requested_frames: i64 = rendered
            .item_verifications
            .iter()
            .map(|item| item.requested_frame_count)
            .sum();
        ensure!(
            rendered.video_frame_count == requested_frames,
            "rendered reel has {} video frames, expected exactly {requested_frames}",
            rendered.video_frame_count
        );
        ensure!(
            (rendered.video_duration_s - rendered.requested_duration_s).abs() <= 0.002,
            "rendered reel video stream duration {:.6}s differs from requested {:.6}s",
            rendered.video_duration_s,
            rendered.requested_duration_s
        );
        let verification = json!({
            "sources_unchanged": true,
            "source_count": sources.snapshots.len(),
            "item_count": sources.request.items.len(),
            "requested_duration_s": rendered.requested_duration_s,
            "measured_duration_s": rendered.output_probe.duration_s,
            "duration_delta_s": duration_delta_s,
            "duration_tolerance_s": duration_tolerance_s,
            "video_frame_count": rendered.video_frame_count,
            "video_duration_s": rendered.video_duration_s,
            "video_stream_duration_delta_s":
                (rendered.video_duration_s - rendered.requested_duration_s).abs(),
            "frame_rule": "each item delivers round((out_s - in_s) * fps) frames starting at \
                           the first source frame at or after in_s; cuts land exactly at the \
                           previous item's video duration; audio is trimmed and \
                           silence-padded to exactly the item video duration so it never \
                           outlasts video and never shifts later items' audio early",
            "items": rendered.item_verifications.iter().map(|item| json!({
                "index": item.index,
                "source_path": item.source_path,
                "in_s": item.in_s,
                "out_s": item.out_s,
                "fps": item.fps,
                "first_source_frame": item.first_source_frame,
                "last_source_frame": item.last_source_frame,
                "requested_frame_count": item.requested_frame_count,
                "rendered_frame_count": item.rendered_frame_count,
                "video_duration_s": item.video_duration_s,
                "audio_duration_s": item.audio_duration_s,
            })).collect::<Vec<_>>(),
            "dimensions": {
                "width": rendered.output_probe.width,
                "height": rendered.output_probe.height,
            },
            "fps": rendered.output_probe.fps,
            "has_audio": rendered.output_probe.has_audio,
            "audio_duration_s": rendered.output_probe.audio_duration_s,
            "video_codec": rendered.output_probe.video_codec,
            "pixel_format": rendered.output_probe.pixel_format,
            "color_space": rendered.output_probe.color_space,
            "color_primaries": rendered.output_probe.color_primaries,
            "color_transfer": rendered.output_probe.color_transfer,
        });
        let created_at = Utc::now();
        let media_type = sources.request.output.media_type().to_owned();
        let mut commands = rendered.item_commands.clone();
        commands.extend(rendered.video_remux_commands.iter().cloned());
        commands.push(rendered.command.clone());
        commands.push(rendered.probe_command.clone());
        let manifest_destination = manifest_path(destination);
        let manifest = json!({
            "schema_version": MANIFEST_SCHEMA_VERSION,
            "job": {
                "id": job.id,
                "attempt": attempt.attempt,
                "owner_id": owner_id,
                "created_at": created_at,
            },
            "sources": source_evidence,
            "frozen_recipe": serde_json::from_str::<Value>(&job.frozen_recipe_json)?,
            "frozen_plan": serde_json::from_str::<Value>(
                job.frozen_plan_json.as_deref().context("reel frozen plan is missing")?
            )?,
            "model_versions": serde_json::from_str::<Value>(&job.model_versions_json)?,
            "tool_versions": {
                "crush_pipeline": env!("CARGO_PKG_VERSION"),
                "executor": "crush-video-reel-v1",
                "backend": rendered.backend.as_str(),
                "encoder": rendered.encoder,
            },
            "commands": commands,
            "render": {
                "preset": rendered.preset,
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
            width: Some(i64::from(rendered.output_probe.width)),
            height: Some(i64::from(rendered.output_probe.height)),
            duration_s: Some(rendered.output_probe.duration_s),
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
        fs::hard_link(&staging.manifest, &manifest_destination)
            .context("filesystem does not support exclusive render-manifest publication")?;
        if let Err(error) = fs::hard_link(&staging.output, destination) {
            let _ = fs::remove_file(&manifest_destination);
            return Err(error).context("filesystem does not support exclusive render publication");
        }
        sync_parent(destination)?;
        store.render_job_finish(owner_id, &output)?;
        Ok(output)
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_video_clip_attempt(
        &self,
        store: &mut Store,
        owner_id: &str,
        job: &RenderJob,
        attempt: &RenderAttempt,
        source_snapshot: &VideoSourceSnapshot,
        source: &ResolvedVideoSource,
        destination: &Path,
        staging: &ManagedStaging,
        recipe: &ClipRenderRequest,
    ) -> anyhow::Result<RenderOutput> {
        store.render_job_set_progress(owner_id, &job.id, 0.1)?;
        let runner = ffmpeg::Runner::new(
            ffmpeg::resolve()?,
            self.config.limits.threads,
            job.id.clone(),
        );
        let rendered = {
            let mut progress_writer = JobProgressWriter::new(store, owner_id, &job.id);
            runner.render_clip_with_control(
                &source.path,
                recipe,
                &staging.output,
                &self.cancellation,
                |progress| progress_writer.record(&progress),
            )?
        };
        store.render_job_set_progress(owner_id, &job.id, 0.75)?;
        ensure!(!self.cancellation.is_cancelled(), "video render cancelled");
        let source_hash_after = sha256_file(&source.path)?;
        ensure!(
            source_hash_after.eq_ignore_ascii_case(&source_snapshot.sha256),
            "video source changed while it was rendering"
        );
        let output_size = i64::try_from(fs::metadata(&staging.output)?.len())
            .context("render output size overflowed i64")?;
        let output_sha256 = sha256_file(&staging.output)?;
        // Same documented rule as the encoder-side check (crush_stage_split::ffmpeg):
        // duration_tolerance = frame_tolerance + slack, so a container the encoder accepted
        // can never fail here — no pass-then-fail window (e.g. 60 fps AAC priming).
        let duration_tolerance_s = ffmpeg::duration_tolerance_s(rendered.output_probe.fps);
        let duration_delta_s =
            (rendered.output_probe.duration_s - rendered.requested_duration_s).abs();
        ensure!(
            duration_delta_s <= duration_tolerance_s,
            "rendered clip duration differs from requested duration beyond frame tolerance"
        );
        match recipe.audio {
            ClipAudio::Mute => ensure!(
                !rendered.output_probe.has_audio,
                "muted clip unexpectedly contains audio"
            ),
            ClipAudio::Source if source.has_audio => ensure!(
                rendered.output_probe.has_audio,
                "source-audio clip lost its audio stream"
            ),
            ClipAudio::Source => {}
        }
        let verification = json!({
            "source_hash_before": source_snapshot.sha256,
            "source_hash_after": source_hash_after,
            "source_unchanged": true,
            "requested_duration_s": rendered.requested_duration_s,
            "measured_duration_s": rendered.output_probe.duration_s,
            "duration_delta_s": duration_delta_s,
            "duration_tolerance_s": duration_tolerance_s,
            "dimensions": {
                "width": rendered.output_probe.width,
                "height": rendered.output_probe.height,
            },
            "fps": rendered.output_probe.fps,
            "has_audio": rendered.output_probe.has_audio,
            "video_codec": rendered.output_probe.video_codec,
            "pixel_format": rendered.output_probe.pixel_format,
            "color_space": rendered.output_probe.color_space,
            "color_primaries": rendered.output_probe.color_primaries,
            "color_transfer": rendered.output_probe.color_transfer,
        });
        let created_at = Utc::now();
        let media_type = recipe.output.media_type().to_owned();
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
                "media_kind": source_snapshot.media_kind,
                "media_id": source_snapshot.media_id,
                "source_id": source_snapshot.source_id,
                "sha256": source_snapshot.sha256,
                "frozen_path": source_snapshot.frozen_path,
                "resolved_path": source.path,
            },
            "frozen_recipe": serde_json::from_str::<Value>(&job.frozen_recipe_json)?,
            "frozen_plan": job.frozen_plan_json.as_deref().map(serde_json::from_str::<Value>).transpose()?,
            "model_versions": serde_json::from_str::<Value>(&job.model_versions_json)?,
            "tool_versions": {
                "crush_pipeline": env!("CARGO_PKG_VERSION"),
                "executor": "crush-video-clip-v1",
                "backend": rendered.backend.as_str(),
                "encoder": rendered.encoder,
            },
            "commands": [rendered.command, rendered.probe_command],
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
            width: Some(i64::from(rendered.output_probe.width)),
            height: Some(i64::from(rendered.output_probe.height)),
            duration_s: Some(rendered.output_probe.duration_s),
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
        fs::hard_link(&staging.manifest, &manifest_destination)
            .context("filesystem does not support exclusive render-manifest publication")?;
        if let Err(error) = fs::hard_link(&staging.output, destination) {
            let _ = fs::remove_file(&manifest_destination);
            return Err(error).context("filesystem does not support exclusive render publication");
        }
        sync_parent(destination)?;
        store.render_job_finish(owner_id, &output)?;
        Ok(output)
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
        let media_type = recipe.output.media_type().to_owned();
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
    let output = PhotoOutputPreset::parse(preset)
        .with_context(|| format!("unsupported photo output preset {preset:?}"))?;
    Ok(PhotoRenderRecipe {
        crop,
        rotation_degrees,
        grade,
        output,
    })
}

fn parse_video_clip_recipe(frozen: &str) -> anyhow::Result<ClipRenderRequest> {
    let frozen: Value = serde_json::from_str(frozen).context("frozen recipe is invalid JSON")?;
    let schema = frozen
        .get("schema")
        .and_then(Value::as_object)
        .context("frozen recipe schema is missing")?;
    ensure!(
        schema.get("schema_version").and_then(Value::as_u64) == Some(1),
        "video clip executor supports only recipe schema version 1"
    );
    ensure!(
        schema.get("kind").and_then(Value::as_str) == Some("video_clip"),
        "frozen recipe is not a video clip"
    );
    let crop = match schema.get("crop") {
        Some(Value::Null) => None,
        Some(value) => {
            let crop = value
                .as_object()
                .context("video clip crop must be an object")?;
            Some(NormalizedVideoCrop {
                x: required_f64(crop, "x", "video clip crop")?,
                y: required_f64(crop, "y", "video clip crop")?,
                width: required_f64(crop, "width", "video clip crop")?,
                height: required_f64(crop, "height", "video clip crop")?,
            })
        }
        None => bail!("video clip crop is missing"),
    };
    let grade_object = schema
        .get("grade")
        .and_then(Value::as_object)
        .context("video clip grade is missing")?;
    let grade = match grade_object.get("mode").and_then(Value::as_str) {
        Some("none") => VideoGrade::None,
        Some("basic") => VideoGrade::Basic(BasicVideoGrade {
            exposure_ev: required_f64(grade_object, "exposure_ev", "video clip grade")?,
            contrast: required_f64(grade_object, "contrast", "video clip grade")?,
            saturation: required_f64(grade_object, "saturation", "video clip grade")?,
            temperature: required_f64(grade_object, "temperature", "video clip grade")?,
            tint: required_f64(grade_object, "tint", "video clip grade")?,
        }),
        Some(other) => bail!("unsupported video clip grade mode {other:?}"),
        None => bail!("video clip grade mode is missing"),
    };
    ensure!(
        schema
            .get("transition")
            .and_then(Value::as_object)
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            == Some("cut"),
        "video clip transition must be cut"
    );
    let audio = match schema
        .get("audio")
        .and_then(Value::as_object)
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
    {
        Some("source") => ClipAudio::Source,
        Some("mute") => ClipAudio::Mute,
        Some(other) => bail!("unsupported video clip audio mode {other:?}"),
        None => bail!("video clip audio mode is missing"),
    };
    let output_preset = schema
        .get("output")
        .and_then(Value::as_object)
        .and_then(|value| value.get("preset"))
        .and_then(Value::as_str);
    let output = match output_preset {
        Some(preset) => ClipOutputPreset::parse(preset)
            .with_context(|| format!("unsupported video clip output preset {preset:?}"))?,
        None => bail!("video clip output preset is missing"),
    };
    let recipe = ClipRenderRequest {
        in_s: required_f64(schema, "in_s", "video clip")?,
        out_s: required_f64(schema, "out_s", "video clip")?,
        crop,
        grade,
        transition: ClipTransition::Cut,
        audio,
        output,
    };
    recipe.validate()?;
    Ok(recipe)
}

fn parse_reel_v1_recipe(frozen: &str) -> anyhow::Result<ReelV1Recipe> {
    let frozen: Value = serde_json::from_str(frozen).context("frozen recipe is invalid JSON")?;
    let schema = frozen
        .get("schema")
        .and_then(Value::as_object)
        .context("frozen recipe schema is missing")?;
    ensure!(
        schema.get("schema_version").and_then(Value::as_u64) == Some(1),
        "ordered reel executor supports only recipe schema version 1"
    );
    ensure!(
        schema.get("kind").and_then(Value::as_str) == Some("reel"),
        "frozen recipe is not a reel"
    );
    ensure!(
        schema
            .get("transition")
            .and_then(Value::as_object)
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            == Some("cut"),
        "ordered reel v1 supports cut transitions only"
    );
    let audio = match schema
        .get("audio")
        .and_then(Value::as_object)
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
    {
        Some("source") => ClipAudio::Source,
        Some("mute") => ClipAudio::Mute,
        Some(other) => bail!("unsupported ordered reel audio mode {other:?}"),
        None => bail!("ordered reel audio mode is missing"),
    };
    let output_preset = schema
        .get("output")
        .and_then(Value::as_object)
        .and_then(|value| value.get("preset"))
        .and_then(Value::as_str);
    let output = match output_preset {
        Some(preset) => ClipOutputPreset::parse(preset)
            .with_context(|| format!("unsupported ordered reel output preset {preset:?}"))?,
        None => bail!("ordered reel output preset is missing"),
    };
    Ok(ReelV1Recipe { audio, output })
}

fn parse_frozen_reel_plan(value: &str) -> anyhow::Result<Vec<FrozenReelPlanItem>> {
    let value: Value = serde_json::from_str(value).context("frozen project is invalid JSON")?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .context("frozen project has no items array")?;
    ensure!(!items.is_empty(), "frozen reel project is empty");
    let mut parsed = Vec::with_capacity(items.len());
    for (index, value) in items.iter().enumerate() {
        let item = value
            .as_object()
            .with_context(|| format!("frozen project item {index} is not an object"))?;
        let media_kind = item.get("media_kind").and_then(Value::as_str);
        ensure!(
            matches!(media_kind, Some("shot" | "span")),
            "ordered reel v1 cannot render project item {index}: photo holds need a versioned duration and framing contract"
        );
        ensure!(
            item.get("pacing").is_none_or(Value::is_null),
            "ordered reel v1 cannot reproduce saved pacing on project item {index}"
        );
        ensure!(
            item.get("crop_x").is_none_or(Value::is_null),
            "ordered reel v1 cannot reproduce scalar crop intent on project item {index}"
        );
        let grade = parse_reel_plan_grade(item.get("grade_json"), index)?;
        let start_s = item
            .get("start_s")
            .and_then(Value::as_f64)
            .with_context(|| format!("frozen project item {index} has no start_s"))?;
        let end_s = item
            .get("end_s")
            .and_then(Value::as_f64)
            .with_context(|| format!("frozen project item {index} has no end_s"))?;
        ensure!(
            start_s.is_finite() && end_s.is_finite() && start_s >= 0.0 && end_s > start_s,
            "frozen project item {index} has invalid clip boundaries"
        );
        parsed.push(FrozenReelPlanItem {
            media_kind: media_kind.expect("checked above").to_owned(),
            media_id: required_string(item, "media_id", "frozen project item")?.to_owned(),
            start_s,
            end_s,
            grade,
        });
    }
    Ok(parsed)
}

fn parse_reel_plan_grade(value: Option<&Value>, index: usize) -> anyhow::Result<ResolvedReelGrade> {
    let Some(Value::String(value)) = value else {
        ensure!(
            value.is_none_or(Value::is_null),
            "frozen project item {index} grade_json must be a JSON string or null"
        );
        return Ok(ResolvedReelGrade::default());
    };
    let parsed: Value = serde_json::from_str(value)
        .with_context(|| format!("frozen project item {index} grade_json is invalid"))?;
    let object = parsed
        .as_object()
        .with_context(|| format!("frozen project item {index} grade must be an object"))?;
    if object.is_empty()
        || (object.len() == 1 && object.get("mode").and_then(Value::as_str) == Some("none"))
    {
        return Ok(ResolvedReelGrade::default());
    }
    let expected = [
        "mode",
        "exposure_ev",
        "contrast",
        "saturation",
        "temperature",
        "tint",
    ];
    ensure!(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "ordered reel v1 cannot reproduce the saved color treatment on project item {index}"
    );
    ensure!(
        object.get("mode").and_then(Value::as_str) == Some("basic"),
        "ordered reel v1 supports only none or basic color treatment on project item {index}"
    );
    Ok(ResolvedReelGrade {
        exposure_ev: required_f64(object, "exposure_ev", "reel grade")?,
        contrast: required_f64(object, "contrast", "reel grade")?,
        saturation: required_f64(object, "saturation", "reel grade")?,
        temperature: required_f64(object, "temperature", "reel grade")?,
        tint: required_f64(object, "tint", "reel grade")?,
        ..ResolvedReelGrade::default()
    })
}

fn parse_reel_source_snapshots(value: &str) -> anyhow::Result<Vec<VideoSourceSnapshot>> {
    let value: Value = serde_json::from_str(value).context("source snapshot is invalid JSON")?;
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .context("source snapshot sources are missing")?;
    ensure!(!sources.is_empty(), "a reel requires frozen sources");
    sources
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let source = value
                .as_object()
                .with_context(|| format!("reel source {index} must be an object"))?;
            let media_kind = source.get("media_kind").and_then(Value::as_str);
            ensure!(
                matches!(media_kind, Some("shot" | "span")),
                "ordered reel v1 supports shot and imported span sources only"
            );
            Ok(VideoSourceSnapshot {
                media_kind: media_kind.expect("checked above").to_owned(),
                media_id: required_string(source, "media_id", "reel source")?.to_owned(),
                source_id: required_string(source, "source_id", "reel source")?.to_owned(),
                sha256: required_string(source, "sha256", "reel source")?.to_owned(),
                frozen_path: PathBuf::from(required_string(source, "path", "reel source")?),
            })
        })
        .collect()
}

fn resolve_reel_v1_sources(
    store: &Store,
    owner_id: &str,
    job: &RenderJob,
    frozen_plan: &str,
    recipe: &ReelV1Recipe,
) -> anyhow::Result<ResolvedReelSources> {
    let plan_items = parse_frozen_reel_plan(frozen_plan)?;
    let snapshots = parse_reel_source_snapshots(&job.source_snapshot_json)?;
    ensure!(
        snapshots.len() == plan_items.len(),
        "frozen reel sources must exactly match the frozen project items"
    );
    let mut by_media_id = BTreeMap::new();
    for snapshot in &snapshots {
        ensure!(
            by_media_id
                .insert(snapshot.media_id.clone(), snapshot)
                .is_none(),
            "frozen reel sources contain a duplicate shot"
        );
    }

    let volume = match recipe.audio {
        ClipAudio::Source => 1.0,
        ClipAudio::Mute => 0.0,
    };
    let mut resolved_paths = BTreeMap::new();
    let mut items = Vec::with_capacity(plan_items.len());
    // Before-pass source-hash memo: a reel that cuts several items from one source video
    // hashes that file once per distinct path, not once per item. (The after-pass keeps its
    // own memo in execute_reel_attempt — sharing this map with it would fabricate the
    // "sources unchanged while rendering" evidence instead of measuring it.)
    let mut hash_before_cache: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (index, item) in plan_items.iter().enumerate() {
        let snapshot = by_media_id.remove(&item.media_id).with_context(|| {
            format!(
                "frozen project item {} has no matching source",
                item.media_id
            )
        })?;
        ensure!(
            snapshot.media_kind == item.media_kind,
            "frozen reel source kind does not match its project item"
        );
        // Shots come from Crush's own scene detection; spans are human-decided boundaries
        // (imported from Reel Studio or set manually) that survive resplit. Both bind to one
        // owner-scoped video and must contain the frozen in/out.
        let (video_id, bound_start, bound_end) = if item.media_kind == "span" {
            let span = store
                .manual_span_by_id(owner_id, &item.media_id)?
                .with_context(|| {
                    format!("reel span {} is not owned by this owner", item.media_id)
                })?;
            (span.video_id, span.start_s, span.end_s)
        } else {
            let shot = store
                .shot_by_id(owner_id, &item.media_id)?
                .with_context(|| {
                    format!("reel shot {} is not owned by this owner", item.media_id)
                })?;
            (shot.video_id, shot.start_s, shot.end_s)
        };
        ensure!(
            video_id == snapshot.source_id,
            "reel {} source_id does not match its owner-scoped video",
            item.media_kind
        );
        ensure!(
            item.start_s >= bound_start && item.end_s <= bound_end,
            "frozen reel boundaries must stay inside {} {}",
            item.media_kind,
            item.media_id
        );
        let video = store.video_by_id(owner_id, &video_id)?.with_context(|| {
            format!(
                "reel {} {} has no owned source video",
                item.media_kind, item.media_id
            )
        })?;
        ensure!(
            video.sha256.eq_ignore_ascii_case(&snapshot.sha256),
            "library video hash changed after this reel was queued"
        );
        if let Some(duration) = video.duration_s {
            ensure!(
                item.end_s <= duration,
                "frozen reel boundary exceeds the source duration"
            );
        }
        if recipe.audio == ClipAudio::Source {
            ensure!(
                video.has_audio,
                "source-audio reel item {index} has no audio; silence insertion is not approved in v1"
            );
        }
        let path = PathBuf::from(video.path);
        ensure!(path.is_absolute(), "library video path is not absolute");
        let hash_before = match hash_before_cache.get(&path) {
            Some(hash) => hash.clone(),
            None => {
                let hash = sha256_file(&path)
                    .with_context(|| format!("failed to hash reel source {}", path.display()))?;
                hash_before_cache.insert(path.clone(), hash.clone());
                hash
            }
        };
        ensure!(
            hash_before.eq_ignore_ascii_case(&snapshot.sha256),
            "reel source bytes changed after this render was queued"
        );
        resolved_paths.insert(item.media_id.clone(), path.clone());
        items.push(ResolvedReelItem {
            source_path: path,
            media_kind: ReelMediaKind::Video,
            in_s: item.start_s,
            out_s: item.end_s,
            crop: None,
            crop_keyframes: Vec::new(),
            caption: None,
            transition: ResolvedReelTransition::default(),
            speed: 1.0,
            motion: ReelMotion::None,
            volume,
            grade: item.grade,
        });
    }
    ensure!(
        by_media_id.is_empty(),
        "frozen reel sources contain shots absent from the frozen project"
    );
    Ok(ResolvedReelSources {
        request: ResolvedReelRequest {
            items,
            format: ReelFormat::Source,
            music: None,
            master_volume: 1.0,
            watermark: None,
            cover: None,
            output: recipe.output,
        },
        snapshots,
        resolved_paths,
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

fn parse_video_source_snapshot(value: &str) -> anyhow::Result<VideoSourceSnapshot> {
    let value: Value = serde_json::from_str(value).context("source snapshot is invalid JSON")?;
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .context("source snapshot sources are missing")?;
    ensure!(
        sources.len() == 1,
        "a video clip recipe requires exactly one frozen source"
    );
    let source = sources[0]
        .as_object()
        .context("video source snapshot must be an object")?;
    let media_kind = required_string(source, "media_kind", "video source")?;
    ensure!(
        matches!(media_kind, "video" | "shot"),
        "video clip source must have media_kind video or shot"
    );
    Ok(VideoSourceSnapshot {
        media_kind: media_kind.to_owned(),
        media_id: required_string(source, "media_id", "video source")?.to_owned(),
        source_id: required_string(source, "source_id", "video source")?.to_owned(),
        sha256: required_string(source, "sha256", "video source")?.to_owned(),
        frozen_path: PathBuf::from(required_string(source, "path", "video source")?),
    })
}

fn resolve_video_source(
    store: &Store,
    owner_id: &str,
    snapshot: &VideoSourceSnapshot,
    recipe: &ClipRenderRequest,
) -> anyhow::Result<ResolvedVideoSource> {
    let video = match snapshot.media_kind.as_str() {
        "video" => {
            ensure!(
                snapshot.source_id == snapshot.media_id,
                "video source_id must match media_id"
            );
            store
                .video_by_id(owner_id, &snapshot.media_id)?
                .with_context(|| {
                    format!("video {} is not owned by this owner", snapshot.media_id)
                })?
        }
        "shot" => {
            let shot = store
                .shot_by_id(owner_id, &snapshot.media_id)?
                .with_context(|| {
                    format!("shot {} is not owned by this owner", snapshot.media_id)
                })?;
            ensure!(
                shot.video_id == snapshot.source_id,
                "shot source_id does not match its owner-scoped video"
            );
            ensure!(
                recipe.in_s >= shot.start_s && recipe.out_s <= shot.end_s,
                "video clip boundaries must stay inside the selected shot"
            );
            store
                .video_by_id(owner_id, &shot.video_id)?
                .with_context(|| format!("shot {} has no owned source video", snapshot.media_id))?
        }
        _ => unreachable!("snapshot parser accepts video or shot"),
    };
    ensure!(
        video.sha256.eq_ignore_ascii_case(&snapshot.sha256),
        "library video hash changed after this render was queued"
    );
    if let Some(duration) = video.duration_s {
        ensure!(
            recipe.out_s <= duration,
            "video clip boundary exceeds the source duration"
        );
    }
    let path = PathBuf::from(video.path);
    ensure!(path.is_absolute(), "library video path is not absolute");
    Ok(ResolvedVideoSource {
        path,
        has_audio: video.has_audio,
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
    ensure!(
        preset
            .extensions()
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed)),
        "destination extension does not match preset {}",
        preset.as_str()
    );
    Ok(())
}

fn validate_video_destination(path: &Path, preset: ClipOutputPreset) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .context("video render destination needs a file extension")?;
    ensure!(
        preset
            .extensions()
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed)),
        "destination extension does not match preset {}",
        preset.as_str()
    );
    Ok(())
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
    // Cheap short-circuit before any SHA-256: recovery must never pay a full hash for a file
    // whose size already disagrees with the checksummed evidence. Size equality is verified
    // first and the hashes remain the contract; mtime is deliberately never consulted.
    let expected_size = u64::try_from(output.size_bytes).unwrap_or(u64::MAX);
    match fs::metadata(output_path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == expected_size => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect published output {}",
                    output_path.display()
                )
            })
        }
    }
    if !manifest_path.is_file() {
        return Ok(false);
    }
    Ok(
        sha256_file(output_path)?.eq_ignore_ascii_case(&output.output_sha256)
            && sha256_file(manifest_path)?.eq_ignore_ascii_case(&output.manifest_sha256),
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
    use crush_core::DEFAULT_OWNER_ID;
    use crush_store::{NewRenderJob, Photo, PhotoStatus, RenderRecipe};
    use rusqlite::Connection;

    fn running_photo_job(store: &mut Store, root: &Path, job_id: &str) -> PathBuf {
        let source_path = root.join("source.jpg");
        let source_hash = "a".repeat(64);
        store
            .upsert_photo(
                DEFAULT_OWNER_ID,
                &Photo {
                    id: "photo-1".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: source_path.to_string_lossy().into_owned(),
                    sha256: source_hash.clone(),
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
                },
            )
            .unwrap();
        store
            .render_recipe_create(
                DEFAULT_OWNER_ID,
                &RenderRecipe {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    id: "photo-test".to_owned(),
                    version: 1,
                    kind: RenderRecipeKind::Photo,
                    name: "Photo test".to_owned(),
                    schema_json: json!({
                        "schema_version": 1,
                        "kind": "photo",
                        "crop": null,
                        "rotation_degrees": 0,
                        "grade": {"mode": "none"},
                        "output": {"preset": "jpeg-srgb-v1"}
                    })
                    .to_string(),
                    created_at: Utc::now(),
                },
            )
            .unwrap();
        let destination = root.join("output.jpg");
        store
            .render_job_create(
                DEFAULT_OWNER_ID,
                &NewRenderJob {
                    id: job_id.to_owned(),
                    recipe_id: "photo-test".to_owned(),
                    recipe_version: 1,
                    plan_id: None,
                    plan_revision: None,
                    source_snapshot_json: json!({
                        "schema_version": 1,
                        "context_key": "test",
                        "selection_provenance": {"origin": "general"},
                        "sources": [{
                            "media_kind": "photo",
                            "media_id": "photo-1",
                            "source_id": "photo-1",
                            "sha256": source_hash,
                            "path": source_path,
                        }]
                    })
                    .to_string(),
                    model_versions_json: json!({
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
        store
            .render_job_start(
                DEFAULT_OWNER_ID,
                job_id,
                &root.join("staging/output.jpg").to_string_lossy(),
                Utc::now(),
            )
            .unwrap();
        destination
    }

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

    /// TASK-035 item 4: ffmpeg's measured progress maps monotonically into the job's
    /// 0.1..0.75 window through the real guarded store UPDATE — never reaching the
    /// verification-reserved values — and writes are throttled.
    #[test]
    fn ffmpeg_progress_maps_monotonically_into_the_job_window() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        let _destination = running_photo_job(&mut store, directory.path(), "progress-job");
        let mut writer = JobProgressWriter::new(&mut store, DEFAULT_OWNER_ID, "progress-job");
        let job_progress = |root: &Path| {
            Store::open(root)
                .unwrap()
                .render_job_by_id(DEFAULT_OWNER_ID, "progress-job")
                .unwrap()
                .unwrap()
                .progress
        };

        // Half of the measured render maps to 0.1 + 0.65*0.5 — beyond the 0.1 staging mark.
        writer.record(&ffmpeg::Progress {
            out_time_s: 1.0,
            percent: 50.0,
        });
        assert_eq!(job_progress(directory.path()), 0.1 + 0.65 * 0.5);

        // Equal or lower ffmpeg progress never writes.
        writer.record(&ffmpeg::Progress {
            out_time_s: 1.0,
            percent: 50.0,
        });
        writer.record(&ffmpeg::Progress {
            out_time_s: 0.2,
            percent: 10.0,
        });
        assert_eq!(job_progress(directory.path()), 0.1 + 0.65 * 0.5);

        // A higher value inside the throttle window is skipped, then written after the
        // interval passes.
        writer.record(&ffmpeg::Progress {
            out_time_s: 1.2,
            percent: 60.0,
        });
        assert_eq!(job_progress(directory.path()), 0.1 + 0.65 * 0.5);
        std::thread::sleep(JobProgressWriter::MIN_WRITE_INTERVAL);
        writer.record(&ffmpeg::Progress {
            out_time_s: 1.2,
            percent: 60.0,
        });
        assert_eq!(job_progress(directory.path()), 0.1 + 0.65 * 0.6);

        // Even a completed encode stays at 0.75: 1.0 is reserved for verification, and the
        // executor's explicit final 0.75 write remains the terminal pre-verify value.
        std::thread::sleep(JobProgressWriter::MIN_WRITE_INTERVAL);
        writer.record(&ffmpeg::Progress {
            out_time_s: 2.0,
            percent: 100.0,
        });
        assert_eq!(job_progress(directory.path()), 0.75);
    }

    /// Startup recovery verifies a publication by size first: a truncated or missing output
    /// fails without ever paying a full SHA-256, and only a size-matching file is hashed.
    #[test]
    fn verified_publication_checks_size_before_any_hash() {
        let directory = tempfile::tempdir().unwrap();
        let output_path = directory.path().join("published.png");
        let manifest_path = directory.path().join("published.png.crush-manifest.json");
        let content = b"published output bytes".to_vec();
        fs::write(&output_path, &content).unwrap();
        fs::write(&manifest_path, b"published manifest").unwrap();
        let output = RenderOutput {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            id: "out".to_owned(),
            job_id: "job".to_owned(),
            attempt: 1,
            output_path: output_path.to_string_lossy().into_owned(),
            output_sha256: sha256_file(&output_path).unwrap(),
            size_bytes: content.len() as i64,
            media_type: "image/png".to_owned(),
            width: None,
            height: None,
            duration_s: None,
            verification_json: "{}".to_owned(),
            manifest_path: manifest_path.to_string_lossy().into_owned(),
            manifest_json: "published manifest".to_owned(),
            manifest_sha256: sha256_file(&manifest_path).unwrap(),
            created_at: Utc::now(),
        };
        assert!(verified_publication_matches(&output).unwrap());

        // Truncated output: the size mismatch short-circuits. The claimed hash is deliberately
        // unreachable ("not the content hash"), so returning false here cannot have hashed the
        // file — the cheap stat decided it.
        let truncated = RenderOutput {
            size_bytes: 999_999,
            output_sha256: "f".repeat(64),
            ..output.clone()
        };
        assert!(!verified_publication_matches(&truncated).unwrap());

        let missing = RenderOutput {
            output_path: directory
                .path()
                .join("gone.png")
                .to_string_lossy()
                .into_owned(),
            ..output.clone()
        };
        assert!(!verified_publication_matches(&missing).unwrap());

        // Intact size but tampered bytes still verifies false on the hash; and a missing
        // manifest never finalizes.
        fs::write(&output_path, b"tampered").unwrap();
        assert!(!verified_publication_matches(&output).unwrap());
        fs::write(&output_path, &content).unwrap();
        fs::remove_file(&manifest_path).unwrap();
        assert!(!verified_publication_matches(&output).unwrap());
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

    #[test]
    fn ordered_reel_v1_parser_preserves_supported_intent_and_rejects_v2() {
        let frozen = json!({
            "id": "reel-cut",
            "version": 1,
            "kind": "reel",
            "name": "Ordered cut reel",
            "schema": {
                "schema_version": 1,
                "kind": "reel",
                "transition": {"kind": "cut"},
                "audio": {"mode": "mute"},
                "output": {"preset": "mov-h264-sdr-v1"}
            }
        });
        let recipe = parse_reel_v1_recipe(&frozen.to_string()).unwrap();
        assert_eq!(recipe.audio, ClipAudio::Mute);
        assert_eq!(recipe.output, ClipOutputPreset::MovH264SdrV1);

        let mut v2 = frozen;
        v2["schema"]["schema_version"] = json!(2);
        assert!(parse_reel_v1_recipe(&v2.to_string()).is_err());
    }

    #[test]
    fn ordered_reel_plan_rejects_treatment_it_cannot_reproduce() {
        let plan = |crop_x: Value, grade_json: Value| {
            json!({
                "items": [{
                    "media_kind": "shot",
                    "media_id": "shot-1",
                    "start_s": 1.0,
                    "end_s": 2.0,
                    "pacing": null,
                    "crop_x": crop_x,
                    "grade_json": grade_json
                }]
            })
        };
        let supported = parse_frozen_reel_plan(
            &plan(
                Value::Null,
                json!(r#"{"mode":"basic","exposure_ev":0.1,"contrast":0.0,"saturation":1.0,"temperature":0.0,"tint":0.0}"#),
            )
            .to_string(),
        )
        .unwrap();
        assert_eq!(supported[0].start_s, 1.0);
        assert_eq!(supported[0].grade.exposure_ev, 0.1);
        assert!(parse_frozen_reel_plan(&plan(json!(0.5), Value::Null).to_string()).is_err());
        assert!(
            parse_frozen_reel_plan(&plan(Value::Null, json!(r#"{"warmth":12}"#)).to_string())
                .is_err()
        );
    }

    #[test]
    fn published_verifying_pair_wins_over_late_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        let destination = running_photo_job(&mut store, directory.path(), "late-cancel");
        store
            .render_job_mark_verifying(DEFAULT_OWNER_ID, "late-cancel")
            .unwrap();
        fs::write(&destination, b"published output").unwrap();
        fs::write(manifest_path(&destination), b"published manifest").unwrap();

        let returned = settle_render_execution_error(
            &mut store,
            DEFAULT_OWNER_ID,
            "late-cancel",
            &destination,
            true,
            anyhow::anyhow!("simulated finalization failure"),
        );

        assert!(returned
            .to_string()
            .contains("simulated finalization failure"));
        assert_eq!(
            store
                .render_job_by_id(DEFAULT_OWNER_ID, "late-cancel")
                .unwrap()
                .unwrap()
                .status,
            RenderJobStatus::Verifying
        );
    }

    #[test]
    fn durable_failure_transition_errors_are_not_swallowed() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        let destination = running_photo_job(&mut store, directory.path(), "persist-failure");
        let audit = Connection::open(store.db_path()).unwrap();
        audit
            .execute_batch(
                "CREATE TRIGGER reject_failed_render
                 BEFORE UPDATE OF status ON render_jobs
                 WHEN NEW.status = 'failed'
                 BEGIN
                   SELECT RAISE(ABORT, 'simulated durable transition failure');
                 END;",
            )
            .unwrap();

        let returned = settle_render_execution_error(
            &mut store,
            DEFAULT_OWNER_ID,
            "persist-failure",
            &destination,
            false,
            anyhow::anyhow!("simulated render failure"),
        );
        let message = format!("{returned:#}");
        assert!(message.contains("simulated render failure"));
        assert!(message.contains("durable failure state could not be persisted"));
        assert!(message.contains("simulated durable transition failure"));
        assert_eq!(
            store
                .render_job_by_id(DEFAULT_OWNER_ID, "persist-failure")
                .unwrap()
                .unwrap()
                .status,
            RenderJobStatus::Running
        );
    }
}
