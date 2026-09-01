//! End-to-end, resumable, one-video-at-a-time ingestion orchestration.

pub mod reel_recipe;
pub mod reel_studio_import;
pub mod render;
pub mod source;
pub mod video_source;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{ensure, Context};
use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::job::Stage;
use crush_core::models;
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_search::SearchEngine;
use crush_stage_aesthetic::{
    analyze, bipolar_similarity, cosine, AnalysisContext, SemanticSignals, StrongShotScores,
    MODEL_VERSION as AESTHETIC_MODEL_VERSION,
};
use crush_stage_asr::{
    choose_model, model_path, total_memory_bytes, transcribe_video_with_control, TranscribeOptions,
};
use crush_stage_embed::embed_missing_shots_with_control;
use crush_stage_embed::embedder::{Embedder, ProviderPreference};
use crush_stage_embed::preprocess::preprocess;
use crush_stage_split::{ffmpeg, scene};
use crush_store::{
    AestheticAssessment, EmbeddingMeta, MediaKind, NewJob, Photo, PhotoSourceMetadata, PhotoStatus,
    Store, Video, VideoSourceMetadata, VideoStatus,
};
use image::{DynamicImage, GenericImageView};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const HASH_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSummary {
    pub discovered: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub discovered_photos: usize,
    pub indexed_photos: usize,
    /// Files whose (owner, sha256) identity was already indexed at a different path and
    /// were re-pointed to the existing row during this ingest ("moved/renamed/duplicate →
    /// relinked"). Additive: `indexed`/`skipped` keep their exact meaning, and `moved` /
    /// `renamed` only ever count files whose old copy is really gone — a same-content
    /// file whose old path still exists is a duplicate copy, counted in `duplicated`.
    pub moved: usize,
    pub renamed: usize,
    /// Same content found at a new path while the old file still exists on disk: the
    /// catalog was re-pointed to the new copy, but nothing moved — the old copy is still
    /// where it was.
    pub duplicated: usize,
    pub recovered_jobs: usize,
    pub search_vectors: usize,
    pub errors: Vec<(PathBuf, String)>,
    /// Per-file detail for every counted moved/renamed outcome.
    pub relinked: Vec<RelinkedAsset>,
}

/// One media file that ingest found at a new path: the existing identity row was
/// re-pointed, never duplicated. `Moved` means the directory changed; `Renamed` means
/// only the file name changed; `DuplicateCopy` means the old file still exists on disk —
/// the same content lives in two places and the catalog now points at the new copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkedAsset {
    pub media_kind: &'static str,
    pub id: String,
    pub from_path: PathBuf,
    pub to_path: PathBuf,
    pub kind: RelinkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelinkKind {
    Moved,
    Renamed,
    DuplicateCopy,
}

impl RelinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RelinkKind::Moved => "moved",
            RelinkKind::Renamed => "renamed",
            RelinkKind::DuplicateCopy => "duplicate copy",
        }
    }
}

/// Classify a same-content path change found during ingest. If the old file still exists
/// on disk, nothing was moved or renamed — the same content exists at two locations and
/// the catalog now points at the new copy, so report it honestly as a duplicate copy
/// (the path update semantics are unchanged). Otherwise a new parent directory is a
/// move, and a new file name in the same directory is a rename.
fn relink_kind(old_path: &str, new_path: &Path) -> RelinkKind {
    if Path::new(old_path).exists() {
        return RelinkKind::DuplicateCopy;
    }
    let old = Path::new(old_path);
    match (old.parent(), new_path.parent()) {
        (Some(old_parent), Some(new_parent)) if old_parent == new_parent => RelinkKind::Renamed,
        _ => RelinkKind::Moved,
    }
}

/// The result of the explicit relink flow (`crushctl relink`, the app's "Locate moved
/// file…" action): the catalog row was re-pointed after SHA-256 verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelinkOutcome {
    pub media_kind: &'static str,
    pub id: String,
    pub old_path: String,
    pub new_path: String,
}

pub struct Pipeline {
    config: Config,
    paths: AppPaths,
    cancellation: CancellationToken,
    semantic_model: OnceLock<MomentSemanticModel>,
}

impl Pipeline {
    pub fn new(config: Config, paths: AppPaths, cancellation: CancellationToken) -> Self {
        Self {
            config,
            paths,
            cancellation,
            semantic_model: OnceLock::new(),
        }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn ingest(&self, input: &Path, debug: bool) -> anyhow::Result<IngestSummary> {
        lower_priority();
        let media = collect_media_files(input)?;
        ensure!(
            !media.videos.is_empty() || !media.photos.is_empty() || !media.unsupported.is_empty(),
            "no supported photo or video files found at {}",
            input.display()
        );
        let mut store = Store::open(&self.paths.root)?;
        let recovered_jobs = store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        ensure_embedding_metadata(&store)?;
        let mut summary = IngestSummary {
            discovered: media.videos.len() + media.photos.len(),
            discovered_photos: media.photos.len(),
            recovered_jobs,
            ..IngestSummary::default()
        };
        for (path, reason) in media.unsupported {
            tracing::warn!(path = %path.display(), reason = %reason, "known-unsupported media format found");
            summary.errors.push((path, reason));
        }

        for path in media.videos {
            if self.cancellation.is_cancelled() {
                anyhow::bail!("ingest cancelled");
            }
            match self.ingest_one(&mut store, &path, debug) {
                Ok(outcome) => {
                    match outcome.result {
                        IngestOne::Indexed => summary.indexed += 1,
                        IngestOne::Skipped => summary.skipped += 1,
                    }
                    record_relink(&mut summary, outcome.relink);
                }
                Err(error) if self.cancellation.is_cancelled() => {
                    return Err(error).context("ingest cancelled");
                }
                Err(error) => {
                    tracing::error!(path = %path.display(), error = %error, "video ingest failed");
                    summary.failed += 1;
                    summary.errors.push((path, format!("{error:#}")));
                }
            }
        }

        if !media.photos.is_empty() {
            let preference = ProviderPreference::parse(&self.config.embed.provider)?;
            let mut embedder =
                Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
            for path in media.photos {
                if self.cancellation.is_cancelled() {
                    anyhow::bail!("ingest cancelled");
                }
                match self.ingest_photo_one(&store, &path, &mut embedder) {
                    Ok(outcome) => {
                        match outcome.result {
                            IngestOne::Indexed => {
                                summary.indexed += 1;
                                summary.indexed_photos += 1;
                            }
                            IngestOne::Skipped => summary.skipped += 1,
                        }
                        record_relink(&mut summary, outcome.relink);
                    }
                    Err(error) if self.cancellation.is_cancelled() => {
                        return Err(error).context("ingest cancelled");
                    }
                    Err(error) => {
                        summary.failed += 1;
                        summary.errors.push((path, format!("{error:#}")));
                    }
                }
            }
            self.analyze_photos(&store)?;
        }

        let index = SearchEngine::load(
            &store,
            DEFAULT_OWNER_ID,
            self.config.search.transcript_hit_boost,
        )?;
        summary.search_vectors = index.len();
        Ok(summary)
    }

    fn ingest_photo_one(
        &self,
        store: &Store,
        path: &Path,
        embedder: &mut Embedder,
    ) -> anyhow::Result<IngestOutcome> {
        let sha256 = sha256_file(path)?;
        let existing = store.photo_by_sha(DEFAULT_OWNER_ID, &sha256)?;
        // Same (owner, sha256) identity at a different path: moved, renamed, or a
        // duplicate copy (old file still on disk). The path update below rides on the
        // same-row upsert, and the outcome is reported honestly instead of looking like
        // an ordinary skip.
        let mut relink = None;
        if let Some(existing_photo) = &existing {
            if existing_photo.path != path.to_string_lossy() {
                relink = Some(RelinkedAsset {
                    media_kind: "photo",
                    id: existing_photo.id.clone(),
                    from_path: PathBuf::from(&existing_photo.path),
                    to_path: path.to_path_buf(),
                    kind: relink_kind(&existing_photo.path, path),
                });
            }
            let fidelity_complete = store
                .photo_source_metadata(DEFAULT_OWNER_ID, &existing_photo.id)?
                .is_some_and(|metadata| photo_fidelity_complete(store, existing_photo, &metadata));
            if existing_photo.status == PhotoStatus::Done && fidelity_complete {
                if relink.is_some() {
                    let mut updated = existing_photo.clone();
                    updated.path = path.to_string_lossy().into_owned();
                    store.upsert_photo(DEFAULT_OWNER_ID, &updated)?;
                }
                tracing::info!(path = %path.display(), "skip: photo already indexed");
                return Ok(IngestOutcome {
                    result: IngestOne::Skipped,
                    relink,
                });
            }
        }
        let photo_id = existing
            .map(|photo| photo.id)
            .unwrap_or_else(|| format!("photo-{}", &sha256[..32]));
        // The jobs-table FK points at photos(id, owner_id), so the photo row must exist
        // before its job starts. Failures before that point leave nothing to resume: the
        // next ingest retries the deterministic photo id from scratch.
        self.index_photo(store, path, &photo_id, &sha256, embedder)?;
        Ok(IngestOutcome {
            result: IngestOne::Indexed,
            relink,
        })
    }

    fn index_photo(
        &self,
        store: &Store,
        path: &Path,
        photo_id: &str,
        sha256: &str,
        embedder: &mut Embedder,
    ) -> anyhow::Result<()> {
        ensure!(
            !self.cancellation.is_cancelled(),
            "photo embedding cancelled"
        );
        let decoded = source::decode_photo(path, &self.cancellation)?;
        let (width, height) = decoded.image.dimensions();
        let thumb_rel = format!("photos/{photo_id}.jpg");
        let proxy_rel = format!("photos/{photo_id}.jpg");
        store.upsert_photo(
            DEFAULT_OWNER_ID,
            &Photo {
                id: photo_id.to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: path.to_string_lossy().into_owned(),
                sha256: sha256.to_owned(),
                width: i64::from(width),
                height: i64::from(height),
                format: decoded.source_format.clone(),
                orientation: decoded.orientation,
                captured_at: decoded.captured_at,
                camera_make: decoded.camera_make.clone(),
                camera_model: decoded.camera_model.clone(),
                lens: decoded.lens.clone(),
                thumb_rel: Some(thumb_rel.clone()),
                status: PhotoStatus::Pending,
                indexed_at: None,
            },
        )?;
        let job = self.start_photo_job(store, photo_id, Stage::PhotoIngest)?;
        tracing::info!(
            job_id = %job.id,
            stage = %Stage::PhotoIngest,
            photo_id = %photo_id,
            path = %path.display(),
            "photo ingest started"
        );
        let result = (|| {
            let proxy = source::write_jpeg_derivative(
                &decoded.image,
                &store.proxy_path(&proxy_rel)?,
                PHOTO_PROXY_MAX_DIMENSION_PX,
                PHOTO_PROXY_QUALITY,
                decoded.icc_profile.as_deref(),
            )?;
            let thumbnail_path = store.thumbnail_path(&thumb_rel)?;
            let thumbnail = source::write_jpeg_derivative(
                &decoded.image,
                &thumbnail_path,
                PHOTO_THUMBNAIL_MAX_DIMENSION_PX,
                PHOTO_THUMBNAIL_QUALITY,
                decoded.icc_profile.as_deref(),
            )?;
            let original_size_bytes = i64::try_from(std::fs::metadata(path)?.len())
                .context("photo size exceeds SQLite integer range")?;
            let mut metadata: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&decoded.metadata_json)
                    .context("decoded photo metadata_json must be a JSON object")?;
            metadata.insert(
                "proxy_recipe".to_owned(),
                serde_json::json!({
                    "proxy": {
                        "format": "jpeg",
                        "max_dimension_px": PHOTO_PROXY_MAX_DIMENSION_PX,
                        "quality": PHOTO_PROXY_QUALITY,
                    },
                    "thumbnail": {
                        "format": "jpeg",
                        "max_dimension_px": PHOTO_THUMBNAIL_MAX_DIMENSION_PX,
                        "quality": PHOTO_THUMBNAIL_QUALITY,
                    },
                }),
            );
            metadata.insert(
                "thumbnail_sha256".to_owned(),
                serde_json::json!(thumbnail.sha256),
            );
            store.upsert_photo_source_metadata(
                DEFAULT_OWNER_ID,
                &PhotoSourceMetadata {
                    photo_id: photo_id.to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    source_format: decoded.source_format,
                    decoder: decoded.decoder,
                    proxy_rel: Some(proxy_rel),
                    proxy_width: Some(i64::from(proxy.width)),
                    proxy_height: Some(i64::from(proxy.height)),
                    proxy_sha256: Some(proxy.sha256),
                    proxy_provenance: decoded.proxy_provenance,
                    orientation_applied: decoded.orientation_applied,
                    bit_depth: decoded.bit_depth,
                    color_space: decoded.color_space,
                    icc_profile_name: decoded.icc_profile_name,
                    icc_profile_sha256: decoded.icc_profile_sha256,
                    exposure_json: decoded.exposure_json,
                    gps_present: decoded.gps_present,
                    metadata_json: serde_json::Value::Object(metadata).to_string(),
                    original_size_bytes,
                    extracted_at: Utc::now(),
                },
            )?;
            let vector = embedder.embed_image(&preprocess(&decoded.image))?;
            store.put_photo_vector(DEFAULT_OWNER_ID, photo_id, &vector)?;
            store.set_photo_status(DEFAULT_OWNER_ID, photo_id, PhotoStatus::Embedded)?;
            ensure!(
                sha256_file(path)? == sha256,
                "photo source changed while it was being indexed; no source writes were attempted"
            );
            store.set_photo_status(DEFAULT_OWNER_ID, photo_id, PhotoStatus::Done)?;
            Ok(())
        })();
        match result {
            Ok(()) => store.job_finish(DEFAULT_OWNER_ID, &job.id, Utc::now()),
            Err(error) => {
                tracing::error!(
                    job_id = %job.id,
                    stage = %Stage::PhotoIngest,
                    photo_id = %photo_id,
                    path = %path.display(),
                    error = %error,
                    "photo ingest failed"
                );
                if self.cancellation.is_cancelled() {
                    store.job_cancel(DEFAULT_OWNER_ID, &job.id, Utc::now())?;
                } else {
                    store.job_fail(DEFAULT_OWNER_ID, &job.id, Utc::now(), &format!("{error:#}"))?;
                }
                if store.photo_by_id(DEFAULT_OWNER_ID, photo_id)?.is_some() {
                    store.set_photo_status(DEFAULT_OWNER_ID, photo_id, PhotoStatus::Failed)?;
                }
                Err(error)
            }
        }
    }

    /// First-class relink for a moved or renamed file. Resolve the target by asset id or
    /// stale stored path across both media kinds, hash the bytes at the new path, and let
    /// the store update the existing identity row only when that hash matches the recorded
    /// sha256. A mismatch refuses without writing anything; a missing new path refuses;
    /// no duplicate row is ever created and the original file is never modified.
    pub fn relink(&self, target: &str, new_path: &Path) -> anyhow::Result<RelinkOutcome> {
        let mut store = Store::open(&self.paths.root)?;
        ensure!(
            new_path.is_file(),
            "the new path does not exist or is not a file: {}",
            new_path.display()
        );
        // Ingest stores canonicalized paths, so relink records the same form.
        let canonical = new_path
            .canonicalize()
            .unwrap_or_else(|_| new_path.to_path_buf());
        let new_path_string = canonical.to_string_lossy().into_owned();
        let resolved = if let Some(video) = store.video_by_id(DEFAULT_OWNER_ID, target)? {
            Some(("video", video.id, video.path))
        } else if let Some(photo) = store.photo_by_id(DEFAULT_OWNER_ID, target)? {
            Some(("photo", photo.id, photo.path))
        } else if let Some(video) = store.video_by_path(DEFAULT_OWNER_ID, target)? {
            Some(("video", video.id, video.path))
        } else if let Some(photo) = store.photo_by_path(DEFAULT_OWNER_ID, target)? {
            Some(("photo", photo.id, photo.path))
        } else {
            None
        }
        .with_context(|| format!("no indexed photo or video matches {target:?}"))?;
        let (media_kind, id, old_path) = resolved;
        let verified_sha256 = sha256_file(&canonical)?;
        match media_kind {
            "video" => {
                store.relink_video(DEFAULT_OWNER_ID, &id, &new_path_string, &verified_sha256)?;
            }
            _ => {
                store.relink_photo(DEFAULT_OWNER_ID, &id, &new_path_string, &verified_sha256)?;
            }
        }
        tracing::info!(
            media_kind,
            media_id = %id,
            from = %old_path,
            to = %new_path_string,
            "relinked moved media after sha256 verification"
        );
        Ok(RelinkOutcome {
            media_kind,
            id,
            old_path,
            new_path: new_path_string,
        })
    }

    /// Backfill aesthetic analysis for photos that are Done but have no current-model
    /// assessment. Thumbnails are decoded in bounded windows (with a one-photo seam overlap
    /// so adjacent-neighbor evidence survives window boundaries), while `AnalysisContext`
    /// keeps the global index/sequence_len semantics of the whole ordered library.
    fn analyze_photos(&self, store: &Store) -> anyhow::Result<usize> {
        let stale = store.photos_for_analysis(DEFAULT_OWNER_ID, AESTHETIC_MODEL_VERSION)?;
        if stale.is_empty() {
            return Ok(0);
        }
        let semantic_model = self.semantic_model()?;
        let photos = store.photos(DEFAULT_OWNER_ID)?;
        // Both lists share the (path, id) ordering, so a merge walk locates each stale
        // photo's global position without re-reading assessments per row.
        let mut targets = Vec::with_capacity(stale.len());
        let mut stale_cursor = 0;
        for (index, photo) in photos.iter().enumerate() {
            if stale_cursor < stale.len() && stale[stale_cursor].id == photo.id {
                targets.push(index);
                stale_cursor += 1;
            }
        }
        let mut assessed = 0;
        let mut target_cursor = 0;
        while target_cursor < targets.len() {
            let window_start = targets[target_cursor].saturating_sub(1);
            let window_end = (window_start + PHOTO_ANALYSIS_WINDOW).min(photos.len());
            let decode_start = window_start.saturating_sub(1);
            let decode_end = window_end.min(photos.len() - 1);
            let decoded = decode_photo_thumbnails(store, &photos[decode_start..=decode_end])?;
            while target_cursor < targets.len() && targets[target_cursor] < window_end {
                let index = targets[target_cursor];
                let photo = &photos[index];
                let local = index - decode_start;
                target_cursor += 1;
                let Some(image) = &decoded[local] else {
                    continue;
                };
                ensure!(
                    !self.cancellation.is_cancelled(),
                    "photo analysis cancelled"
                );
                let job = self.start_photo_job(store, &photo.id, Stage::Analyze)?;
                tracing::info!(
                    job_id = %job.id,
                    stage = %Stage::Analyze,
                    photo_id = %photo.id,
                    "photo analysis started"
                );
                let result = (|| {
                    let vector = store
                        .vector_for_photo(DEFAULT_OWNER_ID, &photo.id)?
                        .with_context(|| format!("photo {} has no visual vector", photo.id))?;
                    let semantic = semantic_model.signals(&vector);
                    let neighbors = adjacent_images(&decoded, local);
                    let scores = analyze(
                        image,
                        AnalysisContext {
                            source_width: u32::try_from(photo.width).unwrap_or(u32::MAX),
                            source_height: u32::try_from(photo.height).unwrap_or(u32::MAX),
                            duration_s: None,
                            index: Some(index),
                            sequence_len: Some(photos.len()),
                        },
                        semantic,
                        &[],
                        &neighbors,
                    );
                    store.upsert_aesthetic_assessment(
                        DEFAULT_OWNER_ID,
                        &persisted_assessment(MediaKind::Photo, &photo.id, scores),
                    )?;
                    Ok(())
                })();
                match result {
                    Ok(()) => store.job_finish(DEFAULT_OWNER_ID, &job.id, Utc::now())?,
                    Err(error) if self.cancellation.is_cancelled() => {
                        store.job_cancel(DEFAULT_OWNER_ID, &job.id, Utc::now())?;
                        return Err(error);
                    }
                    Err(error) => {
                        tracing::error!(
                            job_id = %job.id,
                            stage = %Stage::Analyze,
                            photo_id = %photo.id,
                            error = %error,
                            "photo analysis failed"
                        );
                        store.job_fail(
                            DEFAULT_OWNER_ID,
                            &job.id,
                            Utc::now(),
                            &format!("{error:#}"),
                        )?;
                        return Err(error);
                    }
                }
                assessed += 1;
            }
        }
        Ok(assessed)
    }

    pub fn resplit(&self, target: &str, debug: bool) -> anyhow::Result<()> {
        lower_priority();
        let mut store = Store::open(&self.paths.root)?;
        store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        ensure_embedding_metadata(&store)?;
        let video = resolve_video(&store, target)?;
        store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Pending)?;
        self.process_video(&mut store, &video.id, debug)
    }

    /// Re-run the full photo pipeline for one stored photo: decode, rebuild the working proxy
    /// and thumbnail, re-embed, verify the source stayed byte-identical, and backfill any stale
    /// analysis. The deterministic photo id is derived from the content hash, so a source that
    /// changed on disk will surface as a different photo rather than overwriting this one.
    pub fn reprocess_photo(&self, target: &str, debug: bool) -> anyhow::Result<()> {
        let _ = debug;
        lower_priority();
        let store = Store::open(&self.paths.root)?;
        store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        ensure_embedding_metadata(&store)?;
        let photo = store
            .photo_by_id(DEFAULT_OWNER_ID, target)?
            .with_context(|| format!("photo {target} was not found"))?;
        let path = Path::new(&photo.path);
        let sha256 = sha256_file(path)?;
        let preference = ProviderPreference::parse(&self.config.embed.provider)?;
        let mut embedder =
            Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
        store.delete_photo_vector(DEFAULT_OWNER_ID, &photo.id)?;
        self.index_photo(&store, path, &photo.id, &sha256, &mut embedder)?;
        self.analyze_photos(&store)?;
        Ok(())
    }

    pub fn reembed(&self, target: Option<&str>, all: bool, debug: bool) -> anyhow::Result<usize> {
        ensure!(all ^ target.is_some(), "choose either --all or one video");
        lower_priority();
        let mut store = Store::open(&self.paths.root)?;
        store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        let expected_metadata = expected_embedding_metadata()?;
        let current_metadata = store.embedding_meta_get(DEFAULT_OWNER_ID)?;
        if current_metadata
            .as_ref()
            .is_some_and(|metadata| metadata != &expected_metadata)
        {
            ensure!(all, "models changed; re-embedding one video would leave a mixed index, use `crushctl reembed --all`");
        }
        let videos = if all {
            store.videos(DEFAULT_OWNER_ID)?
        } else {
            vec![resolve_video(
                &store,
                target.context("one video is required without --all")?,
            )?]
        };
        let mut count = 0;
        for video in videos {
            if store
                .shots_for_video(DEFAULT_OWNER_ID, &video.id)?
                .is_empty()
            {
                continue;
            }
            let had_transcript = store.transcript_count_for_video(DEFAULT_OWNER_ID, &video.id)? > 0;
            store.delete_vectors_for_video(DEFAULT_OWNER_ID, &video.id)?;
            store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Split)?;
            self.run_embed(&store, &video, debug)?;
            self.run_analyze(&store, &video, debug)?;
            if had_transcript || !video.has_audio {
                store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Done)?;
            } else {
                self.run_transcribe(&mut store, &video, debug)?;
                store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Done)?;
            }
            count += 1;
        }
        store.embedding_meta_set(DEFAULT_OWNER_ID, &expected_metadata)?;
        Ok(count)
    }

    pub fn export_clip(
        &self,
        shot_id: &str,
        output: &Path,
    ) -> anyhow::Result<ffmpeg::ExportResult> {
        let store = Store::open(&self.paths.root)?;
        let shot = store
            .shot_by_id(DEFAULT_OWNER_ID, shot_id)?
            .with_context(|| format!("shot {shot_id} was not found"))?;
        let video = store
            .video_by_id(DEFAULT_OWNER_ID, &shot.video_id)?
            .with_context(|| format!("video {} was not found", shot.video_id))?;
        let runner = ffmpeg::Runner::new(ffmpeg::resolve()?, self.config.limits.threads, "clip");
        runner
            .export_clip_with_control(
                Path::new(&video.path),
                shot.start_s,
                shot.end_s,
                output,
                &self.cancellation,
                |_| {},
            )
            .map_err(Into::into)
    }

    fn ingest_one(
        &self,
        store: &mut Store,
        path: &Path,
        debug: bool,
    ) -> anyhow::Result<IngestOutcome> {
        let sha256 = sha256_file(path)?;
        if let Some(existing) = store.video_by_sha(DEFAULT_OWNER_ID, &sha256)? {
            let fidelity_complete = video_fidelity_complete(store, &existing.id)?;
            let mut existing = existing;
            // The (owner, sha256) identity already exists at a different path: same
            // content, new location. Keep the same-row path update and report it
            // honestly — moved or renamed when the old copy is gone, duplicate copy
            // when the old file still exists on disk — instead of a silent relink.
            let mut relink = None;
            if existing.path != path.to_string_lossy() {
                relink = Some(RelinkedAsset {
                    media_kind: "video",
                    id: existing.id.clone(),
                    from_path: PathBuf::from(&existing.path),
                    to_path: path.to_path_buf(),
                    kind: relink_kind(&existing.path, path),
                });
                existing.path = path.to_string_lossy().into_owned();
                store.upsert_video(DEFAULT_OWNER_ID, &existing)?;
            }
            if existing.status == VideoStatus::Done && fidelity_complete {
                if !video_assessments_current(store, &existing.id)? {
                    self.run_analyze(store, &existing, debug)?;
                    return Ok(IngestOutcome {
                        result: IngestOne::Indexed,
                        relink,
                    });
                }
                tracing::info!(path = %path.display(), "skip: already indexed");
                return Ok(IngestOutcome {
                    result: IngestOne::Skipped,
                    relink,
                });
            }
            if existing.status == VideoStatus::Done {
                store.set_video_status(DEFAULT_OWNER_ID, &existing.id, VideoStatus::Pending)?;
            }
            self.process_video(store, &existing.id, debug)?;
            return Ok(IngestOutcome {
                result: IngestOne::Indexed,
                relink,
            });
        }
        if let Some(old) = store.video_by_path(DEFAULT_OWNER_ID, &path.to_string_lossy())? {
            tracing::info!(
                path = %path.display(),
                old_sha256 = old.sha256,
                new_sha256 = sha256,
                "same path now contains new content; keeping the old video record"
            );
        }
        let video_id = format!("video-{}", &sha256[..32]);
        store.upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: video_id.clone(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: path.to_string_lossy().into_owned(),
                sha256,
                duration_s: None,
                fps: None,
                width: None,
                height: None,
                has_audio: true,
                status: VideoStatus::Pending,
                indexed_at: None,
            },
        )?;
        self.process_video(store, &video_id, debug)?;
        Ok(IngestOutcome {
            result: IngestOne::Indexed,
            relink: None,
        })
    }

    fn process_video(&self, store: &mut Store, video_id: &str, debug: bool) -> anyhow::Result<()> {
        let mut status = store.restore_failed_video_status(DEFAULT_OWNER_ID, video_id)?;
        if status == VideoStatus::Pending {
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, video_id)?
                .with_context(|| format!("video {video_id} disappeared before splitting"))?;
            self.run_split(store, &video, debug)?;
            status = VideoStatus::Split;
        }
        if status == VideoStatus::Split {
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, video_id)?
                .with_context(|| format!("video {video_id} disappeared before embedding"))?;
            self.run_embed(store, &video, debug)?;
            status = VideoStatus::Embedded;
        }
        if status == VideoStatus::Embedded {
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, video_id)?
                .with_context(|| format!("video {video_id} disappeared before transcription"))?;
            if !video_assessments_current(store, video_id)? {
                self.run_analyze(store, &video, debug)?;
            }
            self.run_transcribe(store, &video, debug)?;
            status = VideoStatus::Transcribed;
        }
        if status == VideoStatus::Transcribed {
            store.set_video_status(DEFAULT_OWNER_ID, video_id, VideoStatus::Done)?;
        }
        Ok(())
    }

    fn run_split(&self, store: &mut Store, video: &Video, debug: bool) -> anyhow::Result<()> {
        let job = self.start_job(store, video, Stage::Split, debug)?;
        let result = (|| {
            ensure!(!self.cancellation.is_cancelled(), "split cancelled");
            let source_path = Path::new(&video.path);
            video_source::validate_decoder_policy(source_path)?;
            let runner = self.runner(&job)?;
            let probe = runner.probe(source_path)?.value;
            let proxy_policy = video_source::proxy_policy(&probe)?;
            let mut probed = video.clone();
            probed.duration_s = Some(probe.duration_s);
            probed.fps = Some(probe.fps);
            probed.width = Some(i64::from(probe.width));
            probed.height = Some(i64::from(probe.height));
            probed.has_audio = probe.has_audio;
            probed.status = VideoStatus::Pending;
            store.upsert_video(DEFAULT_OWNER_ID, &probed)?;

            let (processing_path, proxy_rel, proxy_sha256, proxy_color) = if proxy_policy.required {
                let relative = format!("videos/{}.mp4", video.id);
                let proxy_path = store.proxy_path(&relative)?;
                let reusable = store
                    .video_source_metadata(DEFAULT_OWNER_ID, &video.id)?
                    .filter(|metadata| metadata.proxy_rel.as_deref() == Some(relative.as_str()))
                    .and_then(|metadata| {
                        proxy_path
                            .is_file()
                            .then(|| sha256_file(&proxy_path).ok())
                            .flatten()
                            .filter(|hash| metadata.proxy_sha256.as_deref() == Some(hash.as_str()))
                    });
                let hash = if let Some(hash) = reusable {
                    hash
                } else {
                    runner.generate_edit_proxy_with_control(
                        source_path,
                        &proxy_path,
                        &self.cancellation,
                        |_| {},
                    )?;
                    sha256_file(&proxy_path)?
                };
                // The proxy is never trusted blindly: re-probe it and record its color tags
                // so derivative color is auditable in metadata_json.
                let proxy_probe = runner.probe(&proxy_path)?.value;
                let proxy_color = serde_json::json!({
                    "color_space": proxy_probe.color_space,
                    "color_primaries": proxy_probe.color_primaries,
                    "color_transfer": proxy_probe.color_transfer,
                    "color_range": proxy_probe.color_range,
                });
                (proxy_path, Some(relative), Some(hash), Some(proxy_color))
            } else {
                (source_path.to_path_buf(), None, None, None)
            };
            let original_size_bytes = i64::try_from(std::fs::metadata(source_path)?.len())
                .context("video size exceeds SQLite integer range")?;
            store.upsert_video_source_metadata(
                DEFAULT_OWNER_ID,
                &VideoSourceMetadata {
                    video_id: video.id.clone(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    container: probe
                        .container
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    video_codec: probe
                        .video_codec
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    codec_profile: probe.codec_profile.clone(),
                    pixel_format: probe.pixel_format.clone(),
                    bit_depth: probe.bit_depth.map(i64::from),
                    color_space: probe.color_space.clone(),
                    color_primaries: probe.color_primaries.clone(),
                    color_transfer: probe.color_transfer.clone(),
                    color_range: probe.color_range.clone(),
                    rotation: probe.rotation.map(i64::from),
                    proxy_rel,
                    proxy_sha256,
                    proxy_required: proxy_policy.required,
                    proxy_reason: proxy_policy.reason,
                    original_size_bytes,
                    metadata_json: {
                        let mut metadata = serde_json::json!({
                            "decoder": "bundled_ffmpeg",
                            "codec_tag": probe.codec_tag,
                            "proxy_policy_version": 1,
                            "proxy_recipe": ffmpeg::edit_proxy_recipe(),
                        });
                        if let Some(proxy_color) = proxy_color {
                            metadata["proxy_color"] = proxy_color;
                        }
                        metadata.to_string()
                    },
                    probed_at: Utc::now(),
                },
            )?;

            let temporary;
            let frames = if let Some(directory) = &job.directory {
                directory.join("frames")
            } else {
                temporary = tempfile::tempdir_in(&self.paths.root)?;
                temporary.path().join("frames")
            };
            runner.sample_frames_with_control(
                &processing_path,
                f64::from(self.config.split.sample_fps),
                &frames,
                &self.cancellation,
                |_| {},
            )?;
            let mut frame_paths = std::fs::read_dir(&frames)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()?;
            frame_paths.retain(|path| path.extension().is_some_and(|value| value == "jpg"));
            frame_paths.sort();
            let detection = scene::detect_with_duration(
                &frame_paths,
                self.config.split.sample_fps,
                probe.duration_s,
                &self.config.split,
            )?;
            if let Some(directory) = &job.directory {
                scene::write_scores_csv(&directory.join("scores.csv"), &detection.scores)?;
            }
            scene::materialize_shots_with_control(
                &runner,
                store,
                DEFAULT_OWNER_ID,
                &video.id,
                &processing_path,
                &detection.shots,
                &self.paths.thumbs(),
                &self.cancellation,
            )?;
            ensure!(
                sha256_file(source_path)? == video.sha256,
                "video source changed while it was being indexed; no source writes were attempted"
            );
            store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Split)?;
            Ok(())
        })();
        self.finish_job(store, video, &job.id, result)
    }

    fn run_embed(&self, store: &Store, video: &Video, debug: bool) -> anyhow::Result<()> {
        let job = self.start_job(store, video, Stage::Embed, debug)?;
        let result = (|| {
            ensure!(!self.cancellation.is_cancelled(), "embedding cancelled");
            let preference = ProviderPreference::parse(&self.config.embed.provider)?;
            let mut embedder =
                Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
            embed_missing_shots_with_control(
                store,
                DEFAULT_OWNER_ID,
                &video.id,
                &mut embedder,
                &self.cancellation,
            )?;
            store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Embedded)?;
            if let Some(directory) = &job.directory {
                write_vectors_json(store, &video.id, &directory.join("vectors.json"))?;
            }
            Ok(())
        })();
        self.finish_job(store, video, &job.id, result)
    }

    fn run_analyze(&self, store: &Store, video: &Video, debug: bool) -> anyhow::Result<()> {
        let job = self.start_job(store, video, Stage::Analyze, debug)?;
        let result = (|| {
            ensure!(!self.cancellation.is_cancelled(), "analysis cancelled");
            let semantic_model = self.semantic_model()?;
            self.analyze_video_shots(store, video, semantic_model, &job)?;
            Ok(())
        })();
        self.finish_job(store, video, &job.id, result)
    }

    fn semantic_model(&self) -> anyhow::Result<&MomentSemanticModel> {
        if let Some(model) = self.semantic_model.get() {
            return Ok(model);
        }
        let preference = ProviderPreference::parse(&self.config.embed.provider)?;
        let mut embedder =
            Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
        let built = MomentSemanticModel::new(&mut embedder)?;
        let _ = self.semantic_model.set(built);
        Ok(self
            .semantic_model
            .get()
            .expect("semantic model was set by this call or another thread"))
    }

    fn analyze_video_shots(
        &self,
        store: &Store,
        video: &Video,
        semantic_model: &MomentSemanticModel,
        job: &ActiveJob,
    ) -> anyhow::Result<usize> {
        let shots = store.shots_for_video(DEFAULT_OWNER_ID, &video.id)?;
        let mut keyframes = Vec::with_capacity(shots.len());
        for shot in &shots {
            let path = shot
                .thumb_rel
                .as_deref()
                .with_context(|| format!("shot {} has no representative frame", shot.id))?;
            keyframes.push(Some(image::open(store.thumbnail_path(path)?)?));
        }
        let temporary;
        let motion_dir = if let Some(directory) = &job.directory {
            directory.join("aesthetic-frames")
        } else {
            temporary = tempfile::tempdir_in(&self.paths.root)?;
            temporary.path().join("aesthetic-frames")
        };
        std::fs::create_dir_all(&motion_dir)?;
        let processing_path = store
            .video_source_metadata(DEFAULT_OWNER_ID, &video.id)?
            .and_then(|metadata| metadata.proxy_rel)
            .map(|relative| store.proxy_path(&relative))
            .transpose()?
            .unwrap_or_else(|| PathBuf::from(&video.path));
        let runner = self.runner(job)?;
        let mut assessed = 0;
        for (index, shot) in shots.iter().enumerate() {
            ensure!(!self.cancellation.is_cancelled(), "analysis cancelled");
            let image = keyframes[index].as_ref().expect("loaded above");
            let duration = shot.end_s - shot.start_s;
            let mut motion_frames = Vec::new();
            for (sample_index, fraction) in [0.25, 0.75].into_iter().enumerate() {
                let path = motion_dir.join(format!("{}-{sample_index}.jpg", shot.id));
                let frame_margin = 2.0 / video.fps.unwrap_or(24.0).max(1.0);
                let safe_source_end =
                    (video.duration_s.unwrap_or(shot.end_s) - frame_margin).max(0.0);
                let sample_time = (shot.start_s + duration * fraction)
                    .min(safe_source_end)
                    .max(shot.start_s);
                runner.frame_at_with_control(
                    &processing_path,
                    sample_time,
                    &path,
                    &self.cancellation,
                )?;
                motion_frames.push(image::open(path)?);
            }
            let neighbors = adjacent_images(&keyframes, index);
            let vector = store
                .vector_for_shot(DEFAULT_OWNER_ID, &shot.id)?
                .with_context(|| format!("shot {} has no visual vector", shot.id))?;
            let scores = analyze(
                image,
                AnalysisContext {
                    source_width: video
                        .width
                        .and_then(|v| u32::try_from(v).ok())
                        .unwrap_or(image.width()),
                    source_height: video
                        .height
                        .and_then(|v| u32::try_from(v).ok())
                        .unwrap_or(image.height()),
                    duration_s: Some(duration),
                    index: Some(index),
                    sequence_len: Some(shots.len()),
                },
                semantic_model.signals(&vector),
                &motion_frames,
                &neighbors,
            );
            store.upsert_aesthetic_assessment(
                DEFAULT_OWNER_ID,
                &persisted_assessment(MediaKind::Shot, &shot.id, scores),
            )?;
            assessed += 1;
        }
        Ok(assessed)
    }

    fn run_transcribe(&self, store: &mut Store, video: &Video, debug: bool) -> anyhow::Result<()> {
        let job = self.start_job(store, video, Stage::Transcribe, debug)?;
        let result = (|| {
            ensure!(!self.cancellation.is_cancelled(), "transcription cancelled");
            let temporary;
            let wav = if let Some(directory) = &job.directory {
                directory.join("audio.wav")
            } else {
                temporary = tempfile::tempdir_in(&self.paths.root)?;
                temporary.path().join("audio.wav")
            };
            if video.has_audio {
                self.runner(&job)?.extract_audio_with_control(
                    Path::new(&video.path),
                    &wav,
                    &self.cancellation,
                    |_| {},
                )?;
            }
            let model = choose_model(&self.config.asr.model, total_memory_bytes())?;
            transcribe_video_with_control(
                store,
                DEFAULT_OWNER_ID,
                &video.id,
                &wav,
                model_path(self.paths.models(), model),
                model,
                TranscribeOptions {
                    threads: self.config.limits.threads,
                    language: self.config.asr.language.clone(),
                },
                &self.cancellation,
            )?;
            Ok(())
        })();
        self.finish_job(store, video, &job.id, result)
    }

    fn start_job(
        &self,
        store: &Store,
        video: &Video,
        stage: Stage,
        debug: bool,
    ) -> anyhow::Result<ActiveJob> {
        let id = Uuid::new_v4().to_string();
        let directory = debug.then(|| self.paths.debug().join(&id));
        if let Some(directory) = &directory {
            std::fs::create_dir_all(directory)?;
        }
        tracing::info!(
            job_id = %id,
            stage = %stage,
            video_id = %video.id,
            "video job started"
        );
        store.job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: id.clone(),
                video_id: Some(video.id.clone()),
                photo_id: None,
                stage,
                started_at: Utc::now(),
                debug_dir: directory.as_ref().map(|path| path.display().to_string()),
            },
        )?;
        Ok(ActiveJob { id, directory })
    }

    /// Photo jobs carry the photo id instead of a video id; the jobs-table CHECK enforces
    /// exactly-one-of and the composite FK keeps photo jobs owner-safe and cascading.
    fn start_photo_job(
        &self,
        store: &Store,
        photo_id: &str,
        stage: Stage,
    ) -> anyhow::Result<ActiveJob> {
        let id = Uuid::new_v4().to_string();
        tracing::info!(job_id = %id, stage = %stage, photo_id = %photo_id, "photo job started");
        store.job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: id.clone(),
                video_id: None,
                photo_id: Some(photo_id.to_owned()),
                stage,
                started_at: Utc::now(),
                debug_dir: None,
            },
        )?;
        Ok(ActiveJob {
            id,
            directory: None,
        })
    }

    fn finish_job(
        &self,
        store: &Store,
        video: &Video,
        job_id: &str,
        result: anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        match result {
            Ok(()) => store.job_finish(DEFAULT_OWNER_ID, job_id, Utc::now()),
            Err(error) if self.cancellation.is_cancelled() => {
                store.job_cancel(DEFAULT_OWNER_ID, job_id, Utc::now())?;
                Err(error)
            }
            Err(error) => {
                store.job_fail(DEFAULT_OWNER_ID, job_id, Utc::now(), &format!("{error:#}"))?;
                store.set_video_status(DEFAULT_OWNER_ID, &video.id, VideoStatus::Failed)?;
                Err(error)
            }
        }
    }

    fn runner(&self, job: &ActiveJob) -> anyhow::Result<ffmpeg::Runner> {
        let runner = ffmpeg::Runner::new(ffmpeg::resolve()?, self.config.limits.threads, &job.id);
        if let Some(directory) = &job.directory {
            Ok(runner.with_debug_dir(directory))
        } else {
            Ok(runner)
        }
    }
}

struct MomentSemanticModel {
    expression: ConceptPair,
    gesture: ConceptPair,
    action: ConceptPair,
    story: ConceptPair,
}

struct ConceptPair {
    positive: [f32; crush_stage_embed::embedder::EMBEDDING_DIM],
    negative: [f32; crush_stage_embed::embedder::EMBEDDING_DIM],
}

impl MomentSemanticModel {
    fn new(embedder: &mut Embedder) -> anyhow::Result<Self> {
        Ok(Self {
            expression: ConceptPair::new(
                embedder,
                "a photograph with a clear expressive emotional moment",
                "a photograph with no visible expression or emotional moment",
            )?,
            gesture: ConceptPair::new(
                embedder,
                "a photograph with a clear meaningful gesture or interaction",
                "a photograph with no visible gesture or interaction",
            )?,
            action: ConceptPair::new(
                embedder,
                "a photograph capturing a decisive action moment",
                "a static photograph with no action",
            )?,
            story: ConceptPair::new(
                embedder,
                "a compelling storytelling photograph with emotional clarity",
                "an ordinary photograph with no clear story",
            )?,
        })
    }

    fn signals(&self, image: &[f32]) -> SemanticSignals {
        let (expression, expression_confidence) = self.expression.score(image);
        let (gesture, gesture_confidence) = self.gesture.score(image);
        let (action, action_confidence) = self.action.score(image);
        let (story, story_confidence) = self.story.score(image);
        SemanticSignals {
            expression,
            gesture,
            action,
            story,
            confidence: (expression_confidence
                + gesture_confidence
                + action_confidence
                + story_confidence)
                / 4.0,
        }
    }
}

impl ConceptPair {
    fn new(embedder: &mut Embedder, positive: &str, negative: &str) -> anyhow::Result<Self> {
        Ok(Self {
            positive: embedder.embed_text(positive)?,
            negative: embedder.embed_text(negative)?,
        })
    }

    fn score(&self, image: &[f32]) -> (f64, f64) {
        bipolar_similarity(cosine(image, &self.positive), cosine(image, &self.negative))
    }
}

fn adjacent_images(images: &[Option<DynamicImage>], index: usize) -> Vec<DynamicImage> {
    let mut neighbors = Vec::with_capacity(2);
    if index > 0 {
        if let Some(image) = &images[index - 1] {
            neighbors.push(image.clone());
        }
    }
    if let Some(Some(image)) = images.get(index + 1) {
        neighbors.push(image.clone());
    }
    neighbors
}

fn persisted_assessment(
    media_kind: MediaKind,
    media_id: &str,
    scores: StrongShotScores,
) -> AestheticAssessment {
    AestheticAssessment {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        media_kind,
        media_id: media_id.to_owned(),
        sharpness: scores.sharpness,
        exposure: scores.exposure,
        contrast: scores.contrast,
        color_harmony: scores.color_harmony,
        balance: scores.balance,
        subject_placement: scores.subject_placement,
        negative_space: scores.negative_space,
        visual_clarity: scores.visual_clarity,
        technical_quality: scores.technical_quality,
        blur_control: scores.blur_control,
        clipping_control: scores.clipping_control,
        noise_control: scores.noise_control,
        compression_quality: scores.compression_quality,
        resolution_quality: scores.resolution_quality,
        motion_stability: scores.motion_stability,
        duplicate_confidence: scores.duplicate_confidence,
        composition_quality: scores.composition_quality,
        hierarchy: scores.hierarchy,
        leading_lines: scores.leading_lines,
        symmetry: scores.symmetry,
        crop_potential: scores.crop_potential,
        moment_story: scores.moment_story,
        expression: scores.expression,
        gesture: scores.gesture,
        action: scores.action,
        novelty: scores.novelty,
        pacing: scores.pacing,
        repetition_risk: scores.repetition_risk,
        overall: scores.overall,
        confidence: scores.confidence,
        explanation_json: scores.explanation_json,
        model_version: AESTHETIC_MODEL_VERSION.to_owned(),
        assessed_at: Utc::now(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IngestOne {
    Indexed,
    Skipped,
}

/// What one discovered file did during ingest. `relink` is set when the file's
/// (owner, sha256) identity was already indexed at a different path, so the outcome can
/// be reported as moved/renamed in addition to the ordinary indexed/skipped result.
struct IngestOutcome {
    result: IngestOne,
    relink: Option<RelinkedAsset>,
}

/// Count and log a moved/renamed/duplicate outcome. Additive by contract: `indexed`/
/// `skipped` keep their exact meaning (the UI progress copy keys on them), and `moved`/
/// `renamed` count only files whose old copy is really gone.
fn record_relink(summary: &mut IngestSummary, relink: Option<RelinkedAsset>) {
    let Some(relinked) = relink else {
        return;
    };
    match relinked.kind {
        RelinkKind::Moved => summary.moved += 1,
        RelinkKind::Renamed => summary.renamed += 1,
        RelinkKind::DuplicateCopy => summary.duplicated += 1,
    }
    tracing::info!(
        media_kind = relinked.media_kind,
        media_id = %relinked.id,
        from = %relinked.from_path.display(),
        to = %relinked.to_path.display(),
        outcome = relinked.kind.as_str(),
        "ingest re-pointed media to the existing identity row"
    );
    summary.relinked.push(relinked);
}

struct ActiveJob {
    id: String,
    directory: Option<PathBuf>,
}

fn ensure_embedding_metadata(store: &Store) -> anyhow::Result<()> {
    let expected = expected_embedding_metadata()?;
    match store.embedding_meta_get(DEFAULT_OWNER_ID)? {
        None => store.embedding_meta_set(DEFAULT_OWNER_ID, &expected),
        Some(found) => {
            ensure!(
                found == expected,
                "models changed, run `crushctl reembed --all`"
            );
            Ok(())
        }
    }
}

fn video_assessments_current(store: &Store, video_id: &str) -> anyhow::Result<bool> {
    let shots = store.shots_for_video(DEFAULT_OWNER_ID, video_id)?;
    if shots.is_empty() {
        return Ok(false);
    }
    for shot in shots {
        let current = store
            .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &shot.id)?
            .is_some_and(|assessment| assessment.model_version == AESTHETIC_MODEL_VERSION);
        if !current {
            return Ok(false);
        }
    }
    Ok(true)
}

const PHOTO_ANALYSIS_WINDOW: usize = 64;
const PHOTO_PROXY_MAX_DIMENSION_PX: u32 = 2560;
const PHOTO_PROXY_QUALITY: u8 = 92;
const PHOTO_THUMBNAIL_MAX_DIMENSION_PX: u32 = 960;
const PHOTO_THUMBNAIL_QUALITY: u8 = 85;

/// Decode thumbnails for a bounded slice of the ordered photo list. Photos that are not Done
/// or have no thumbnail on disk decode to `None`, matching the whole-library decode behavior
/// this replaces; only the memory footprint is bounded now.
fn decode_photo_thumbnails(
    store: &Store,
    photos: &[Photo],
) -> anyhow::Result<Vec<Option<DynamicImage>>> {
    photos
        .iter()
        .map(|photo| {
            if photo.status != PhotoStatus::Done {
                return Ok(None);
            }
            let path = photo
                .thumb_rel
                .as_deref()
                .map(|relative| store.thumbnail_path(relative))
                .transpose()?;
            path.filter(|path| path.is_file())
                .map(image::open)
                .transpose()
                .map_err(anyhow::Error::from)
        })
        .collect()
}

/// A Done photo may only be skipped when both derivatives exist and still match the hashes
/// recorded at index time. Rows without a recorded thumbnail hash are treated as incomplete
/// so the next ingest re-indexes them and backfills the hash.
fn photo_fidelity_complete(store: &Store, photo: &Photo, metadata: &PhotoSourceMetadata) -> bool {
    let (Some(proxy_rel), Some(proxy_sha256)) = (
        metadata.proxy_rel.as_deref(),
        metadata.proxy_sha256.as_deref(),
    ) else {
        return false;
    };
    let Ok(proxy_path) = store.proxy_path(proxy_rel) else {
        return false;
    };
    let proxy_intact =
        proxy_path.is_file() && sha256_file(&proxy_path).is_ok_and(|hash| hash == proxy_sha256);
    if !proxy_intact {
        return false;
    }
    let Some(thumb_rel) = photo.thumb_rel.as_deref() else {
        return false;
    };
    let Some(thumbnail_sha256) = recorded_thumbnail_sha256(metadata) else {
        return false;
    };
    let Ok(thumbnail_path) = store.thumbnail_path(thumb_rel) else {
        return false;
    };
    thumbnail_path.is_file()
        && sha256_file(&thumbnail_path).is_ok_and(|hash| hash == thumbnail_sha256)
}

fn recorded_thumbnail_sha256(metadata: &PhotoSourceMetadata) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&metadata.metadata_json)
        .ok()?
        .get("thumbnail_sha256")?
        .as_str()
        .map(str::to_string)
}

/// A Done video may only be skipped when its working proxy (if one is required) and every
/// shot thumbnail still exist with matching recorded content.
fn video_fidelity_complete(store: &Store, video_id: &str) -> anyhow::Result<bool> {
    let Some(metadata) = store.video_source_metadata(DEFAULT_OWNER_ID, video_id)? else {
        return Ok(false);
    };
    let proxy_intact = !metadata.proxy_required
        || match (
            metadata.proxy_rel.as_deref(),
            metadata.proxy_sha256.as_deref(),
        ) {
            (Some(relative), Some(expected)) => store.proxy_path(relative).is_ok_and(|proxy| {
                proxy.is_file() && sha256_file(&proxy).is_ok_and(|hash| hash == expected)
            }),
            _ => false,
        };
    if !proxy_intact {
        return Ok(false);
    }
    for shot in store.shots_for_video(DEFAULT_OWNER_ID, video_id)? {
        let Some(relative) = shot.thumb_rel.as_deref() else {
            return Ok(false);
        };
        if !store.thumbnail_path(relative)?.is_file() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn expected_embedding_metadata() -> anyhow::Result<EmbeddingMeta> {
    let manifest = models::bundled_manifest()?;
    Ok(EmbeddingMeta {
        owner_id: DEFAULT_OWNER_ID.to_owned(),
        model_name: manifest.model_name,
        model_sha256: manifest.embedding_sha256,
        dim: manifest.dim,
        preprocess_version: manifest.preprocess_version,
    })
}

fn resolve_video(store: &Store, target: &str) -> anyhow::Result<Video> {
    store
        .video_by_id(DEFAULT_OWNER_ID, target)?
        .or(store.video_by_path(DEFAULT_OWNER_ID, target)?)
        .with_context(|| format!("video {target:?} was not found by id or stored path"))
}

fn collect_media_files(input: &Path) -> anyhow::Result<DiscoveredMedia> {
    ensure!(input.exists(), "input does not exist: {}", input.display());
    let mut media = DiscoveredMedia::default();
    collect_media_files_inner(input, &mut media)?;
    media.videos.sort();
    media.videos.dedup();
    media.photos.sort();
    media.photos.dedup();
    media.unsupported.sort();
    media.unsupported.dedup();
    Ok(media)
}

#[derive(Debug, Default)]
struct DiscoveredMedia {
    videos: Vec<PathBuf>,
    photos: Vec<PathBuf>,
    /// Known-unsupported media files with their precise capability reasons. Arbitrary
    /// non-media files are never flagged.
    unsupported: Vec<(PathBuf, String)>,
}

fn collect_media_files_inner(input: &Path, media: &mut DiscoveredMedia) -> anyhow::Result<()> {
    if input.is_file() {
        let canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
        classify_media_file(&canonical, media);
        return Ok(());
    }
    for entry in std::fs::read_dir(input)
        .with_context(|| format!("failed to read directory {}", input.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_media_files_inner(&entry.path(), media)?;
        } else if file_type.is_file() {
            classify_media_file(&entry.path(), media);
        }
    }
    Ok(())
}

fn classify_media_file(path: &Path, media: &mut DiscoveredMedia) {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if is_video(&canonical) {
        media.videos.push(canonical);
    } else if is_photo(&canonical) {
        media.photos.push(canonical);
    } else if let Some(reason) = known_unsupported_reason(&canonical) {
        media.unsupported.push((canonical, reason.to_owned()));
    }
}

/// Curated registry of media extensions Crush recognizes but deliberately does not decode.
/// Discovery records the precise capability reason for each so files never vanish silently,
/// while arbitrary non-media files (".txt", ".DS_Store", ...) stay unflagged.
pub const KNOWN_UNSUPPORTED_EXTENSIONS: &[(&str, &str)] = &[
    (
        "avif",
        "AVIF decode is disabled: no approved full decoder is bundled; embedded-preview extraction is not full media support",
    ),
    (
        "jxl",
        "JPEG XL decode is disabled: no approved full decoder is bundled; embedded-preview extraction is not full media support",
    ),
    (
        "erf",
        "ERF (Phase One) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "iiq",
        "IIQ (Phase One) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "3fr",
        "3FR (Hasselblad) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "x3f",
        "X3F (Sigma Foveon) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "gpr",
        "GPR (GoPro RAW) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "mrw",
        "MRW (Minolta) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "pef",
        "PEF (Pentax) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
    (
        "srw",
        "SRW (Samsung) decode is disabled: no approved full decoder exists for this acquisition format; embedded-preview extraction is not full media support",
    ),
];

fn known_unsupported_reason(path: &Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    KNOWN_UNSUPPORTED_EXTENSIONS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, reason)| *reason)
}

fn is_video(path: &Path) -> bool {
    video_source::is_video_extension(path)
}

fn is_photo(path: &Path) -> bool {
    source::is_supported_photo_extension(path)
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(HASH_BUFFER_BYTES, file);
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn write_vectors_json(store: &Store, video_id: &str, output: &Path) -> anyhow::Result<()> {
    let vectors = store
        .shots_for_video(DEFAULT_OWNER_ID, video_id)?
        .into_iter()
        .map(|shot| {
            let vector = store.vector_for_shot(DEFAULT_OWNER_ID, &shot.id)?;
            Ok((shot.id, vector))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    std::fs::write(output, serde_json::to_vec_pretty(&vectors)?)?;
    Ok(())
}

fn lower_priority() {
    #[cfg(unix)]
    unsafe {
        if libc::setpriority(libc::PRIO_PROCESS, 0, 10) != 0 {
            tracing::warn!(error = %std::io::Error::last_os_error(), "could not lower ingest priority");
        }
    }
}
