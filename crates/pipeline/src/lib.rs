//! End-to-end, resumable, one-video-at-a-time ingestion orchestration.

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
use crush_stage_asr::{
    choose_model, model_path, total_memory_bytes, transcribe_video_with_control, TranscribeOptions,
};
use crush_stage_embed::embed_missing_shots_with_control;
use crush_stage_embed::embedder::{Embedder, ProviderPreference};
use crush_stage_split::{ffmpeg, scene};
use crush_store::{EmbeddingMeta, NewJob, Store, Video, VideoStatus};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "avi", "mts"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSummary {
    pub discovered: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub failed: usize,
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
        let files = collect_video_files(input)?;
        ensure!(
            !files.is_empty(),
            "no supported video files found at {}",
            input.display()
        );
        let mut store = Store::open(&self.paths.root)?;
        let recovered_jobs = store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
        ensure_embedding_metadata(&store)?;
        let mut summary = IngestSummary {
            discovered: files.len(),
            recovered_jobs,
            ..IngestSummary::default()
        };

        for path in files {
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

        let index = SearchEngine::load(
            &store,
            DEFAULT_OWNER_ID,
            self.config.search.transcript_hit_boost,
        )?;
        summary.search_vectors = index.len();
        Ok(summary)
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
            if existing.status == VideoStatus::Done {
                tracing::info!(path = %path.display(), "skip: already indexed");
                return Ok(IngestOne::Skipped);
            }
            if existing.path != path.to_string_lossy() {
                let mut updated = existing.clone();
                updated.path = path.to_string_lossy().into_owned();
                store.upsert_video(DEFAULT_OWNER_ID, &updated)?;
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
            let runner = self.runner(&job)?;
            let probe = runner.probe(Path::new(&video.path))?.value;
            let mut probed = video.clone();
            probed.duration_s = Some(probe.duration_s);
            probed.fps = Some(probe.fps);
            probed.width = Some(i64::from(probe.width));
            probed.height = Some(i64::from(probe.height));
            probed.has_audio = probe.has_audio;
            probed.status = VideoStatus::Pending;
            store.upsert_video(DEFAULT_OWNER_ID, &probed)?;

            let temporary;
            let frames = if let Some(directory) = &job.directory {
                directory.join("frames")
            } else {
                temporary = tempfile::tempdir_in(&self.paths.root)?;
                temporary.path().join("frames")
            };
            runner.sample_frames_with_control(
                Path::new(&video.path),
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
                Path::new(&video.path),
                &detection.shots,
                &self.paths.thumbs(),
                &self.cancellation,
            )?;
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
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
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
