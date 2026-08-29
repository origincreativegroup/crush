//! End-to-end, resumable, one-video-at-a-time ingestion orchestration.

pub mod source;
pub mod video_source;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

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
    pub recovered_jobs: usize,
    pub search_vectors: usize,
    pub errors: Vec<(PathBuf, String)>,
}

pub struct Pipeline {
    config: Config,
    paths: AppPaths,
    cancellation: CancellationToken,
}

impl Pipeline {
    pub fn new(config: Config, paths: AppPaths, cancellation: CancellationToken) -> Self {
        Self {
            config,
            paths,
            cancellation,
        }
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn ingest(&self, input: &Path, debug: bool) -> anyhow::Result<IngestSummary> {
        lower_priority();
        let videos = collect_video_files(input)?;
        let photos = collect_photo_files(input)?;
        ensure!(
            !videos.is_empty() || !photos.is_empty(),
            "no supported photo or video files found at {}",
            input.display()
        );
        let mut store = Store::open(&self.paths.root)?;
        let recovered_jobs = store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        ensure_embedding_metadata(&store)?;
        let mut summary = IngestSummary {
            discovered: videos.len() + photos.len(),
            discovered_photos: photos.len(),
            recovered_jobs,
            ..IngestSummary::default()
        };

        for path in videos {
            if self.cancellation.is_cancelled() {
                anyhow::bail!("ingest cancelled");
            }
            match self.ingest_one(&mut store, &path, debug) {
                Ok(IngestOne::Indexed) => summary.indexed += 1,
                Ok(IngestOne::Skipped) => summary.skipped += 1,
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

        if !photos.is_empty() {
            let preference = ProviderPreference::parse(&self.config.embed.provider)?;
            let mut embedder =
                Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
            for path in photos {
                if self.cancellation.is_cancelled() {
                    anyhow::bail!("ingest cancelled");
                }
                match self.ingest_photo_one(&store, &path, &mut embedder) {
                    Ok(IngestOne::Indexed) => {
                        summary.indexed += 1;
                        summary.indexed_photos += 1;
                    }
                    Ok(IngestOne::Skipped) => summary.skipped += 1,
                    Err(error) if self.cancellation.is_cancelled() => {
                        return Err(error).context("ingest cancelled");
                    }
                    Err(error) => {
                        tracing::error!(path = %path.display(), error = %error, "photo ingest failed");
                        summary.failed += 1;
                        summary.errors.push((path, format!("{error:#}")));
                    }
                }
            }
            self.analyze_photos(&store, &mut embedder)?;
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
    ) -> anyhow::Result<IngestOne> {
        let sha256 = sha256_file(path)?;
        let existing = store.photo_by_sha(DEFAULT_OWNER_ID, &sha256)?;
        if let Some(existing) = &existing {
            let fidelity_complete = store
                .photo_source_metadata(DEFAULT_OWNER_ID, &existing.id)?
                .and_then(|metadata| {
                    let relative = metadata.proxy_rel?;
                    let expected_hash = metadata.proxy_sha256?;
                    let proxy = store.proxy_path(&relative).ok()?;
                    proxy
                        .is_file()
                        .then(|| sha256_file(&proxy).ok())
                        .flatten()
                        .filter(|hash| hash == &expected_hash)
                })
                .is_some();
            if existing.status == PhotoStatus::Done && fidelity_complete {
                if existing.path != path.to_string_lossy() {
                    let mut updated = existing.clone();
                    updated.path = path.to_string_lossy().into_owned();
                    store.upsert_photo(DEFAULT_OWNER_ID, &updated)?;
                }
                tracing::info!(path = %path.display(), "skip: photo already indexed");
                return Ok(IngestOne::Skipped);
            }
        }
        let photo_id = existing
            .map(|photo| photo.id)
            .unwrap_or_else(|| format!("photo-{}", &sha256[..32]));
        let result = (|| {
            ensure!(
                !self.cancellation.is_cancelled(),
                "photo embedding cancelled"
            );
            let decoded = source::decode_photo(path)?;
            let (width, height) = decoded.image.dimensions();
            let thumb_rel = format!("photos/{photo_id}.jpg");
            let proxy_rel = format!("photos/{photo_id}.jpg");
            store.upsert_photo(
                DEFAULT_OWNER_ID,
                &Photo {
                    id: photo_id.clone(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: path.to_string_lossy().into_owned(),
                    sha256: sha256.clone(),
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
            let proxy = source::write_jpeg_derivative(
                &decoded.image,
                &store.proxy_path(&proxy_rel)?,
                2560,
                92,
                decoded.icc_profile.as_deref(),
            )?;
            let thumbnail_path = store.thumbnail_path(&thumb_rel)?;
            source::write_jpeg_derivative(
                &decoded.image,
                &thumbnail_path,
                960,
                85,
                decoded.icc_profile.as_deref(),
            )?;
            let original_size_bytes = i64::try_from(std::fs::metadata(path)?.len())
                .context("photo size exceeds SQLite integer range")?;
            store.upsert_photo_source_metadata(
                DEFAULT_OWNER_ID,
                &PhotoSourceMetadata {
                    photo_id: photo_id.clone(),
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
                    metadata_json: decoded.metadata_json,
                    original_size_bytes,
                    extracted_at: Utc::now(),
                },
            )?;
            let vector = embedder.embed_image(&preprocess(&decoded.image))?;
            store.put_photo_vector(DEFAULT_OWNER_ID, &photo_id, &vector)?;
            store.set_photo_status(DEFAULT_OWNER_ID, &photo_id, PhotoStatus::Embedded)?;
            ensure!(
                sha256_file(path)? == sha256,
                "photo source changed while it was being indexed; no source writes were attempted"
            );
            store.set_photo_status(DEFAULT_OWNER_ID, &photo_id, PhotoStatus::Done)?;
            Ok(IngestOne::Indexed)
        })();
        if result.is_err() && store.photo_by_id(DEFAULT_OWNER_ID, &photo_id)?.is_some() {
            store.set_photo_status(DEFAULT_OWNER_ID, &photo_id, PhotoStatus::Failed)?;
        }
        result
    }

    fn analyze_photos(&self, store: &Store, embedder: &mut Embedder) -> anyhow::Result<usize> {
        let semantic_model = MomentSemanticModel::new(embedder)?;
        let photos = store.photos(DEFAULT_OWNER_ID)?;
        let mut decoded = Vec::with_capacity(photos.len());
        for photo in &photos {
            if photo.status != PhotoStatus::Done {
                decoded.push(None);
                continue;
            }
            let path = photo
                .thumb_rel
                .as_deref()
                .map(|relative| store.thumbnail_path(relative))
                .transpose()?;
            decoded.push(
                path.filter(|path| path.is_file())
                    .map(image::open)
                    .transpose()?,
            );
        }
        let mut assessed = 0;
        for (index, photo) in photos.iter().enumerate() {
            let Some(image) = &decoded[index] else {
                continue;
            };
            let vector = store
                .vector_for_photo(DEFAULT_OWNER_ID, &photo.id)?
                .with_context(|| format!("photo {} has no visual vector", photo.id))?;
            let semantic = semantic_model.signals(&vector);
            let neighbors = adjacent_images(&decoded, index);
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
            assessed += 1;
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

    fn ingest_one(&self, store: &mut Store, path: &Path, debug: bool) -> anyhow::Result<IngestOne> {
        let sha256 = sha256_file(path)?;
        if let Some(existing) = store.video_by_sha(DEFAULT_OWNER_ID, &sha256)? {
            let fidelity_complete = store
                .video_source_metadata(DEFAULT_OWNER_ID, &existing.id)?
                .map(|metadata| {
                    if !metadata.proxy_required {
                        return true;
                    }
                    let Some(relative) = metadata.proxy_rel else {
                        return false;
                    };
                    let Some(expected_hash) = metadata.proxy_sha256 else {
                        return false;
                    };
                    store.proxy_path(&relative).is_ok_and(|proxy| {
                        proxy.is_file()
                            && sha256_file(&proxy).is_ok_and(|hash| hash == expected_hash)
                    })
                })
                .unwrap_or(false);
            let mut existing = existing;
            if existing.path != path.to_string_lossy() {
                existing.path = path.to_string_lossy().into_owned();
                store.upsert_video(DEFAULT_OWNER_ID, &existing)?;
            }
            if existing.status == VideoStatus::Done && fidelity_complete {
                if !video_assessments_current(store, &existing.id)? {
                    self.run_analyze(store, &existing, debug)?;
                    return Ok(IngestOne::Indexed);
                }
                tracing::info!(path = %path.display(), "skip: already indexed");
                return Ok(IngestOne::Skipped);
            }
            if existing.status == VideoStatus::Done {
                store.set_video_status(DEFAULT_OWNER_ID, &existing.id, VideoStatus::Pending)?;
            }
            self.process_video(store, &existing.id, debug)?;
            return Ok(IngestOne::Indexed);
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
        Ok(IngestOne::Indexed)
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

            let (processing_path, proxy_rel, proxy_sha256) = if proxy_policy.required {
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
                (proxy_path, Some(relative), Some(hash))
            } else {
                (source_path.to_path_buf(), None, None)
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
                    metadata_json: serde_json::json!({
                        "decoder": "bundled_ffmpeg",
                        "codec_tag": probe.codec_tag,
                        "proxy_policy_version": 1,
                    })
                    .to_string(),
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
            let preference = ProviderPreference::parse(&self.config.embed.provider)?;
            let mut embedder =
                Embedder::new(self.paths.models(), preference, self.config.limits.threads)?;
            let semantic_model = MomentSemanticModel::new(&mut embedder)?;
            self.analyze_video_shots(store, video, &semantic_model, &job)?;
            Ok(())
        })();
        self.finish_job(store, video, &job.id, result)
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
        store.job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: id.clone(),
                video_id: video.id.clone(),
                stage,
                started_at: Utc::now(),
                debug_dir: directory.as_ref().map(|path| path.display().to_string()),
            },
        )?;
        Ok(ActiveJob { id, directory })
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

fn collect_video_files(input: &Path) -> anyhow::Result<Vec<PathBuf>> {
    ensure!(input.exists(), "input does not exist: {}", input.display());
    let mut files = Vec::new();
    collect_video_files_inner(input, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_photo_files(input: &Path) -> anyhow::Result<Vec<PathBuf>> {
    collect_files(input, is_photo)
}

fn collect_files(input: &Path, predicate: fn(&Path) -> bool) -> anyhow::Result<Vec<PathBuf>> {
    ensure!(input.exists(), "input does not exist: {}", input.display());
    let mut files = Vec::new();
    collect_files_inner(input, predicate, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_files_inner(
    input: &Path,
    predicate: fn(&Path) -> bool,
    files: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if input.is_file() {
        if predicate(input) {
            files.push(input.canonicalize().unwrap_or_else(|_| input.to_path_buf()));
        }
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
            collect_files_inner(&entry.path(), predicate, files)?;
        } else if file_type.is_file() && predicate(&entry.path()) {
            files.push(entry.path().canonicalize().unwrap_or_else(|_| entry.path()));
        }
    }
    Ok(())
}

fn collect_video_files_inner(input: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if input.is_file() {
        if is_video(input) {
            files.push(input.canonicalize().unwrap_or_else(|_| input.to_path_buf()));
        }
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
            collect_video_files_inner(&entry.path(), files)?;
        } else if file_type.is_file() && is_video(&entry.path()) {
            files.push(entry.path().canonicalize().unwrap_or_else(|_| entry.path()));
        }
    }
    Ok(())
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
