#[cfg(target_os = "macos")]
mod macos {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex, MutexGuard};

    use anyhow::{ensure, Context};
    use crush_core::cancellation::CancellationToken;
    use crush_core::job::{JobRecord, JobStatus};
    use crush_core::models::{self, ModelStatus};
    use crush_core::paths::AppPaths;
    use crush_core::{Config, DEFAULT_OWNER_ID};
    use crush_pipeline::{IngestSummary, Pipeline};
    use crush_search::{SearchEngine, SearchResult};
    use crush_stage_embed::embedder::{Embedder, ProviderPreference};
    use crush_stage_split::ffmpeg;
    use crush_store::{EmbeddingMeta, JobFilter, Store, VideoStatus};
    use serde::Serialize;
    use tauri::{AppHandle, Emitter, Manager, State};
    use uuid::Uuid;

    type CommandResult<T> = Result<T, String>;

    struct RuntimeState {
        config: Config,
        paths: AppPaths,
        background: Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
        active_ingest: Arc<Mutex<Option<ActiveIngest>>>,
        search: Arc<Mutex<Option<SearchRuntime>>>,
    }

    struct ActiveIngest {
        job_id: String,
        cancellation: CancellationToken,
    }

    struct SearchRuntime {
        engine: SearchEngine,
        embedder: Embedder,
    }

    #[derive(Debug, Clone, Copy, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum BackgroundKind {
        ModelsDownload,
        Ingest,
    }

    #[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum BackgroundStatus {
        Running,
        Done,
        Failed,
        Cancelled,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct BackgroundTask {
        job_id: String,
        kind: BackgroundKind,
        status: BackgroundStatus,
        detail: Option<String>,
        error: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TaskStarted {
        job_id: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelFileStatus {
        name: String,
        bytes: u64,
        status: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelDownloadProgress {
        job_id: String,
        name: Option<String>,
        downloaded: Option<u64>,
        total: Option<u64>,
        status: BackgroundStatus,
        detail: Option<String>,
        error: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct VideoView {
        id: String,
        path: String,
        duration_s: Option<f64>,
        fps: Option<f64>,
        width: Option<i64>,
        height: Option<i64>,
        has_audio: bool,
        status: String,
        indexed_at: Option<String>,
        shots: usize,
        last_error: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct JobSnapshot {
        background: Vec<BackgroundTask>,
        pipeline: Vec<JobRecord>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TranscriptView {
        id: String,
        start_s: f64,
        end_s: f64,
        text: String,
        confidence: Option<f64>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ShotDetailView {
        id: String,
        video_id: String,
        video_path: String,
        idx: i64,
        shot_count: usize,
        start_s: f64,
        end_s: f64,
        rep_frame_s: f64,
        fps: Option<f64>,
        thumb_path: Option<String>,
        transcripts: Vec<TranscriptView>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ExportedClip {
        path: String,
        mode: String,
    }

    #[tauri::command]
    fn doctor(state: State<'_, RuntimeState>) -> CommandResult<String> {
        command_result(doctor_report(&state.config, &state.paths))
    }

    fn doctor_report(config: &Config, paths: &AppPaths) -> anyhow::Result<String> {
        let resolved = ffmpeg::resolve()?;
        let runner = ffmpeg::Runner::new(resolved, config.limits.threads, "app-doctor");
        let version = runner.version()?.value;
        let manifest = models::bundled_manifest()?;
        let checks = models::inspect(&paths.models(), &manifest)?;
        let present = checks
            .iter()
            .filter(|check| check.status == ModelStatus::Present)
            .count();
        let store = Store::open(&paths.root)?;
        Ok(format!(
            "Crush doctor\ndata_dir={}\ndatabase={} schema={}\nffmpeg source={:?} path={}\n{}\nmodels={}/{} present",
            paths.root.display(),
            store.db_path().display(),
            store.schema_version()?,
            runner.resolved().source,
            runner.resolved().path.display(),
            version,
            present,
            checks.len()
        ))
    }

    #[tauri::command]
    fn models_status(state: State<'_, RuntimeState>) -> CommandResult<Vec<ModelFileStatus>> {
        command_result((|| {
            let manifest = models::bundled_manifest()?;
            let found = models::inspect(&state.paths.models(), &manifest)?;
            Ok(found
                .into_iter()
                .map(|check| ModelFileStatus {
                    bytes: manifest.files[&check.name].bytes,
                    name: check.name,
                    status: model_status_name(check.status).to_owned(),
                })
                .collect())
        })())
    }

    #[tauri::command]
    fn models_download(
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<TaskStarted> {
        let job_id = Uuid::new_v4().to_string();
        insert_background(
            &state.background,
            BackgroundTask {
                job_id: job_id.clone(),
                kind: BackgroundKind::ModelsDownload,
                status: BackgroundStatus::Running,
                detail: Some("checking model assets".to_owned()),
                error: None,
            },
        )?;
        let models_dir = state.paths.models();
        let data_dir = state.paths.root.clone();
        let tasks = Arc::clone(&state.background);
        let spawned_job_id = job_id.clone();
        drop(tauri::async_runtime::spawn_blocking(move || {
            let progress_app = app.clone();
            let result = models::ensure(&models_dir, models::DEFAULT_MANIFEST_URL, |progress| {
                let _ = progress_app.emit(
                    "download-progress",
                    ModelDownloadProgress {
                        job_id: spawned_job_id.clone(),
                        name: Some(progress.name),
                        downloaded: Some(progress.downloaded),
                        total: Some(progress.total),
                        status: BackgroundStatus::Running,
                        detail: None,
                        error: None,
                    },
                );
            })
            .and_then(|manifest| record_embedding_metadata(&data_dir, &manifest))
            .map(|()| "models ready".to_owned());
            let completed = complete_background(&tasks, &spawned_job_id, result, false);
            if let Ok(task) = completed {
                let _ = app.emit(
                    "download-progress",
                    ModelDownloadProgress {
                        job_id: task.job_id,
                        name: None,
                        downloaded: None,
                        total: None,
                        status: task.status,
                        detail: task.detail,
                        error: task.error,
                    },
                );
            }
        }));
        Ok(TaskStarted { job_id })
    }

    #[tauri::command]
    fn add_folder(
        path: String,
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<TaskStarted> {
        let input = PathBuf::from(&path);
        if !input.exists() {
            return Err(format!("input does not exist: {}", input.display()));
        }
        let job_id = Uuid::new_v4().to_string();
        let cancellation = CancellationToken::default();
        {
            let mut active = lock(&state.active_ingest)?;
            if let Some(current) = active.as_ref() {
                return Err(format!("ingest {} is already running", current.job_id));
            }
            *active = Some(ActiveIngest {
                job_id: job_id.clone(),
                cancellation: cancellation.clone(),
            });
        }
        insert_background(
            &state.background,
            BackgroundTask {
                job_id: job_id.clone(),
                kind: BackgroundKind::Ingest,
                status: BackgroundStatus::Running,
                detail: Some(format!("indexing {}", input.display())),
                error: None,
            },
        )?;
        let initial = command_result(job_snapshot(&state.paths.root, &state.background))?;
        let _ = app.emit("ingest-progress", initial);

        let config = state.config.clone();
        let paths = state.paths.clone();
        let data_dir = paths.root.clone();
        let tasks = Arc::clone(&state.background);
        let active_ingest = Arc::clone(&state.active_ingest);
        let spawned_job_id = job_id.clone();
        let spawned_cancellation = cancellation.clone();
        drop(tauri::async_runtime::spawn_blocking(move || {
            let result =
                Pipeline::new(config, paths, spawned_cancellation.clone()).ingest(&input, false);
            let cancelled = spawned_cancellation.is_cancelled();
            let result = result.map(|summary| ingest_summary(&summary));
            let completed = complete_background(&tasks, &spawned_job_id, result, cancelled);
            if let Ok(mut active) = active_ingest.lock() {
                if active
                    .as_ref()
                    .is_some_and(|current| current.job_id == spawned_job_id)
                {
                    *active = None;
                }
            }
            if completed.is_ok() {
                if let Ok(snapshot) = job_snapshot(&data_dir, &tasks) {
                    let _ = app.emit("ingest-progress", snapshot);
                }
            }
        }));
        Ok(TaskStarted { job_id })
    }

    #[tauri::command]
    fn list_videos(state: State<'_, RuntimeState>) -> CommandResult<Vec<VideoView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let mut output = Vec::new();
            for video in store.videos(DEFAULT_OWNER_ID)? {
                let jobs = store.jobs(
                    DEFAULT_OWNER_ID,
                    &JobFilter {
                        video_id: Some(video.id.clone()),
                        ..JobFilter::default()
                    },
                )?;
                let last_error = if video.status == VideoStatus::Failed {
                    jobs.iter()
                        .find(|job| job.status == JobStatus::Failed)
                        .and_then(|job| job.error.clone())
                } else {
                    None
                };
                output.push(VideoView {
                    shots: store.shots_for_video(DEFAULT_OWNER_ID, &video.id)?.len(),
                    id: video.id,
                    path: video.path,
                    duration_s: video.duration_s,
                    fps: video.fps,
                    width: video.width,
                    height: video.height,
                    has_audio: video.has_audio,
                    status: video_status_name(video.status).to_owned(),
                    indexed_at: video.indexed_at.map(|value| value.to_rfc3339()),
                    last_error,
                });
            }
            Ok(output)
        })())
    }

    #[tauri::command]
    fn job_status(app: AppHandle, state: State<'_, RuntimeState>) -> CommandResult<JobSnapshot> {
        command_result((|| {
            let snapshot = job_snapshot(&state.paths.root, &state.background)?;
            app.emit("ingest-progress", snapshot.clone())?;
            Ok(snapshot)
        })())
    }

    #[tauri::command]
    fn cancel_ingest(state: State<'_, RuntimeState>) -> CommandResult<bool> {
        let active = lock(&state.active_ingest)?;
        if let Some(active) = active.as_ref() {
            active.cancellation.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[tauri::command]
    fn reindex_video(
        id: String,
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<TaskStarted> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("video {id} was not found"))?;
            drop(store);

            let job_id = Uuid::new_v4().to_string();
            let cancellation = CancellationToken::default();
            {
                let mut active = lock_anyhow(&state.active_ingest)?;
                if let Some(current) = active.as_ref() {
                    anyhow::bail!("ingest {} is already running", current.job_id);
                }
                *active = Some(ActiveIngest {
                    job_id: job_id.clone(),
                    cancellation: cancellation.clone(),
                });
            }
            insert_background(
                &state.background,
                BackgroundTask {
                    job_id: job_id.clone(),
                    kind: BackgroundKind::Ingest,
                    status: BackgroundStatus::Running,
                    detail: Some(format!("re-indexing {}", video.path)),
                    error: None,
                },
            )
            .map_err(anyhow::Error::msg)?;
            let initial = job_snapshot(&state.paths.root, &state.background)?;
            let _ = app.emit("ingest-progress", initial);

            let config = state.config.clone();
            let paths = state.paths.clone();
            let data_dir = paths.root.clone();
            let tasks = Arc::clone(&state.background);
            let active_ingest = Arc::clone(&state.active_ingest);
            let spawned_job_id = job_id.clone();
            let spawned_cancellation = cancellation.clone();
            drop(tauri::async_runtime::spawn_blocking(move || {
                let result = Pipeline::new(config, paths, spawned_cancellation.clone())
                    .resplit(&id, false)
                    .map(|()| "re-index complete".to_owned());
                let cancelled = spawned_cancellation.is_cancelled();
                let completed = complete_background(&tasks, &spawned_job_id, result, cancelled);
                if let Ok(mut active) = active_ingest.lock() {
                    if active
                        .as_ref()
                        .is_some_and(|current| current.job_id == spawned_job_id)
                    {
                        *active = None;
                    }
                }
                if completed.is_ok() {
                    if let Ok(snapshot) = job_snapshot(&data_dir, &tasks) {
                        let _ = app.emit("ingest-progress", snapshot);
                    }
                }
            }));
            Ok(TaskStarted { job_id })
        })())
    }

    #[tauri::command]
    async fn search(
        q: String,
        top: usize,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<SearchResult>> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        let cache = Arc::clone(&state.search);
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                ensure!(!q.trim().is_empty(), "search query must not be empty");
                ensure!(top > 0, "top must be greater than zero");
                let store = Store::open(&paths.root)?;
                let mut runtime = lock_anyhow(&cache)?;
                if runtime.is_none() {
                    let preference = ProviderPreference::parse(&config.embed.provider)?;
                    *runtime = Some(SearchRuntime {
                        engine: SearchEngine::load(
                            &store,
                            DEFAULT_OWNER_ID,
                            config.search.transcript_hit_boost,
                        )?,
                        embedder: Embedder::new(paths.models(), preference, config.limits.threads)?,
                    });
                }
                let runtime = runtime.as_mut().context("search runtime was not created")?;
                runtime.engine.reload(&store)?;
                let SearchRuntime { engine, embedder } = runtime;
                engine.search(&store, &mut |text: &str| embedder.embed_text(text), &q, top)
            })())
        })
        .await
        .map_err(|error| format!("search worker failed: {error}"))?
    }

    #[tauri::command]
    fn shot_detail(id: String, state: State<'_, RuntimeState>) -> CommandResult<ShotDetailView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let shot = store
                .shot_by_id(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("shot {id} was not found"))?;
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, &shot.video_id)?
                .with_context(|| format!("video {} was not found", shot.video_id))?;
            let shot_count = store
                .shots_for_video(DEFAULT_OWNER_ID, &shot.video_id)?
                .len();
            let transcripts = store
                .segments_overlapping(DEFAULT_OWNER_ID, &shot.video_id, shot.start_s, shot.end_s)?
                .into_iter()
                .map(|segment| TranscriptView {
                    id: segment.id,
                    start_s: segment.start_s,
                    end_s: segment.end_s,
                    text: segment.text,
                    confidence: segment.confidence,
                })
                .collect();
            let thumb_path = shot
                .thumb_rel
                .as_deref()
                .map(|relative| store.thumbnail_path(relative))
                .transpose()?
                .map(|path| path.display().to_string());
            Ok(ShotDetailView {
                id: shot.id,
                video_id: shot.video_id,
                video_path: video.path,
                idx: shot.idx,
                shot_count,
                start_s: shot.start_s,
                end_s: shot.end_s,
                rep_frame_s: shot.rep_frame_s,
                fps: video.fps,
                thumb_path,
                transcripts,
            })
        })())
    }

    #[tauri::command]
    async fn export_clip(
        id: String,
        out: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<ExportedClip> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                let output = PathBuf::from(out);
                let result = Pipeline::new(config, paths, CancellationToken::default())
                    .export_clip(&id, &output)?;
                Ok(ExportedClip {
                    path: output.display().to_string(),
                    mode: format!("{:?}", result.mode),
                })
            })())
        })
        .await
        .map_err(|error| format!("clip export worker failed: {error}"))?
    }

    #[tauri::command]
    fn open_in_finder(path: String) -> CommandResult<()> {
        command_result((|| {
            let path = PathBuf::from(path);
            ensure!(path.exists(), "path does not exist: {}", path.display());
            let mut command = Command::new("/usr/bin/open");
            if path.is_file() {
                command.arg("-R");
            }
            let status = command.arg(&path).status()?;
            ensure!(status.success(), "Finder returned {status}");
            Ok(())
        })())
    }

    fn model_status_name(status: ModelStatus) -> &'static str {
        match status {
            ModelStatus::Present => "present",
            ModelStatus::Missing => "missing",
            ModelStatus::ShaMismatch => "sha_mismatch",
        }
    }

    fn video_status_name(status: VideoStatus) -> &'static str {
        match status {
            VideoStatus::Pending => "pending",
            VideoStatus::Split => "split",
            VideoStatus::Embedded => "embedded",
            VideoStatus::Transcribed => "transcribed",
            VideoStatus::Done => "done",
            VideoStatus::Failed => "failed",
        }
    }

    fn ingest_summary(summary: &IngestSummary) -> String {
        format!(
            "discovered={} indexed={} skipped={} failed={} recovered={} vectors={}",
            summary.discovered,
            summary.indexed,
            summary.skipped,
            summary.failed,
            summary.recovered_jobs,
            summary.search_vectors
        )
    }

    fn record_embedding_metadata(
        data_dir: &Path,
        manifest: &models::Manifest,
    ) -> anyhow::Result<()> {
        let store = Store::open(data_dir)?;
        if store.embedding_meta_get(DEFAULT_OWNER_ID)?.is_none() {
            store.embedding_meta_set(
                DEFAULT_OWNER_ID,
                &EmbeddingMeta {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    model_name: manifest.model_name.clone(),
                    model_sha256: manifest.embedding_sha256.clone(),
                    dim: manifest.dim,
                    preprocess_version: manifest.preprocess_version,
                },
            )?;
        }
        Ok(())
    }

    fn insert_background(
        tasks: &Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
        task: BackgroundTask,
    ) -> CommandResult<()> {
        lock(tasks)?.insert(task.job_id.clone(), task);
        Ok(())
    }

    fn complete_background<T: ToString>(
        tasks: &Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
        job_id: &str,
        result: anyhow::Result<T>,
        cancelled: bool,
    ) -> CommandResult<BackgroundTask> {
        let mut tasks = lock(tasks)?;
        let task = tasks
            .get_mut(job_id)
            .ok_or_else(|| format!("background task {job_id} disappeared"))?;
        match result {
            Ok(detail) if cancelled => {
                task.status = BackgroundStatus::Cancelled;
                task.detail = Some(detail.to_string());
            }
            Ok(detail) => {
                task.status = BackgroundStatus::Done;
                task.detail = Some(detail.to_string());
            }
            Err(error) if cancelled => {
                task.status = BackgroundStatus::Cancelled;
                task.error = Some(format!("{error:#}"));
            }
            Err(error) => {
                task.status = BackgroundStatus::Failed;
                task.error = Some(format!("{error:#}"));
            }
        }
        Ok(task.clone())
    }

    fn background_snapshot(
        tasks: &Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
    ) -> CommandResult<Vec<BackgroundTask>> {
        Ok(lock(tasks)?.values().cloned().collect())
    }

    fn job_snapshot(
        data_dir: &Path,
        tasks: &Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
    ) -> anyhow::Result<JobSnapshot> {
        let store = Store::open(data_dir)?;
        Ok(JobSnapshot {
            background: background_snapshot(tasks).map_err(anyhow::Error::msg)?,
            pipeline: store.jobs(DEFAULT_OWNER_ID, &JobFilter::default())?,
        })
    }

    fn lock<T>(mutex: &Mutex<T>) -> CommandResult<MutexGuard<'_, T>> {
        mutex
            .lock()
            .map_err(|_| "application state lock was poisoned".to_owned())
    }

    fn lock_anyhow<T>(mutex: &Mutex<T>) -> anyhow::Result<MutexGuard<'_, T>> {
        mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("application state lock was poisoned"))
    }

    fn command_result<T>(result: anyhow::Result<T>) -> CommandResult<T> {
        result.map_err(|error| format!("{error:#}"))
    }

    pub fn run() {
        tauri::Builder::default()
            .plugin(tauri_plugin_clipboard_manager::init())
            .plugin(tauri_plugin_dialog::init())
            .setup(|app| {
                let resource_dir = app.path().resource_dir()?;
                ffmpeg::register_bundle_resource_dir(resource_dir)?;
                let data_dir = app.path().app_data_dir()?;
                std::fs::create_dir_all(&data_dir)?;
                let mut config = Config::load(None)?;
                config.data_dir = Some(data_dir.clone());
                let paths = AppPaths::resolve(config.data_dir.as_ref())?;
                Store::open(&paths.root)?.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
                app.manage(RuntimeState {
                    config,
                    paths,
                    background: Arc::new(Mutex::new(BTreeMap::new())),
                    active_ingest: Arc::new(Mutex::new(None)),
                    search: Arc::new(Mutex::new(None)),
                });
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                doctor,
                models_status,
                models_download,
                add_folder,
                list_videos,
                job_status,
                cancel_ingest,
                reindex_video,
                search,
                shot_detail,
                export_clip,
                open_in_finder
            ])
            .run(tauri::generate_context!())
            .expect("error while running Crush");
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn doctor_reports_tauri_bundle_sidecars_as_bundled() {
            let temporary = tempfile::tempdir().unwrap();
            let contents = temporary.path().join("Crush.app/Contents");
            let resources = contents.join("Resources");
            let macos = contents.join("MacOS");
            std::fs::create_dir_all(&resources).unwrap();
            std::fs::create_dir_all(&macos).unwrap();
            for binary in ["ffmpeg", "ffprobe"] {
                let path = macos.join(binary);
                std::fs::write(&path, "#!/bin/sh\nprintf 'ffmpeg version crush-test\\n'\n")
                    .unwrap();
                let mut permissions = path.metadata().unwrap().permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(path, permissions).unwrap();
            }
            ffmpeg::register_bundle_resource_dir(resources).unwrap();
            let paths = AppPaths {
                root: temporary.path().join("data"),
            };
            std::fs::create_dir_all(&paths.root).unwrap();

            let report = doctor_report(&Config::default(), &paths).unwrap();

            assert!(report.contains("ffmpeg source=Bundled"));
            assert!(report.contains("ffmpeg version crush-test"));
            assert!(report.contains("schema=1"));
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(not(target_os = "macos"))]
pub fn run() {
    eprintln!("Crush desktop is currently supported on macOS only");
}
