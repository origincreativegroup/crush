#[cfg(target_os = "macos")]
mod macos {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    };

    use anyhow::{ensure, Context};
    use crush_core::cancellation::CancellationToken;
    use crush_core::job::JobRecord;
    use crush_core::models::{self, ModelStatus};
    use crush_core::paths::AppPaths;
    use crush_core::{Config, DEFAULT_OWNER_ID};
    use crush_pipeline::{IngestSummary, Pipeline};
    use crush_search::{
        personal_style_score, retrain_style_profile, AssetSearchResult, SearchEngine,
    };
    use crush_stage_embed::embedder::{Embedder, ProviderPreference};
    use crush_stage_split::ffmpeg;
    use crush_store::{
        EmbeddingMeta, FeedbackEvent, FeedbackSignal, JobFilter, MediaKind, PhotoStatus,
        ReferenceItemRole, ReferenceSet, ReferenceSetItem, ReferenceSetScope, ReferenceSetStatus,
        Store, VideoStatus,
    };
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
        /// Set by `record_feedback`; the next `search` retrains the style profile instead of
        /// doing it inline on every pick click.
        retrain_dirty: Arc<AtomicBool>,
    }

    struct ActiveIngest {
        job_id: String,
        cancellation: CancellationToken,
    }

    struct SearchRuntime {
        engine: SearchEngine,
        embedder: Embedder,
        /// Store `data_version` the engine's vector index was built from; the index reloads
        /// only when the store changes.
        generation: Option<i64>,
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
        asset_type: String,
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
        aesthetic_score: Option<f64>,
        personal_style_score: Option<f32>,
        technical_score: Option<f64>,
        composition_score: Option<f64>,
        moment_score: Option<f64>,
        analysis_summary: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PhotoDetailView {
        id: String,
        photo_path: String,
        width: i64,
        height: i64,
        format: String,
        quality: Option<i64>,
        aesthetic_score: Option<f64>,
        personal_style_score: Option<f32>,
        technical_score: Option<f64>,
        composition_score: Option<f64>,
        moment_score: Option<f64>,
        analysis_summary: Option<String>,
        description: String,
        tags: String,
        notes: String,
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
                .map(|check| {
                    Ok(ModelFileStatus {
                        bytes: manifest
                            .files
                            .get(&check.name)
                            .with_context(|| {
                                format!("model manifest is missing entry for {}", check.name)
                            })?
                            .bytes,
                        name: check.name,
                        status: model_status_name(check.status).to_owned(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?)
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
        allow_asset_path(&app, &input)?;
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
        )
        .inspect_err(|_| release_ingest_slot(&state.active_ingest, &job_id))?;
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
            release_ingest_slot(&active_ingest, &spawned_job_id);
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
            let failures = store
                .failed_job_errors(DEFAULT_OWNER_ID)?
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>();
            let mut output = Vec::new();
            for video in store.videos(DEFAULT_OWNER_ID)? {
                let last_error = if video.status == VideoStatus::Failed {
                    failures.get(&video.id).cloned()
                } else {
                    None
                };
                output.push(VideoView {
                    asset_type: "video".to_owned(),
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
            for photo in store.photos(DEFAULT_OWNER_ID)? {
                output.push(VideoView {
                    asset_type: "photo".to_owned(),
                    id: photo.id,
                    path: photo.path,
                    duration_s: None,
                    fps: None,
                    width: Some(photo.width),
                    height: Some(photo.height),
                    has_audio: false,
                    status: photo_status_name(photo.status).to_owned(),
                    indexed_at: photo.indexed_at.map(|value| value.to_rfc3339()),
                    shots: 0,
                    last_error: None,
                });
            }
            output.sort_by(|left, right| left.path.cmp(&right.path));
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
            .map_err(anyhow::Error::msg)
            .inspect_err(|_| release_ingest_slot(&state.active_ingest, &job_id))?;
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
                release_ingest_slot(&active_ingest, &spawned_job_id);
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
    ) -> CommandResult<Vec<AssetSearchResult>> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        let cache = Arc::clone(&state.search);
        let retrain_dirty = Arc::clone(&state.retrain_dirty);
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                ensure!(!q.trim().is_empty(), "search query must not be empty");
                ensure!(top > 0, "top must be greater than zero");
                let mut store = Store::open(&paths.root)?;
                let mut runtime = lock_anyhow(&cache)?;
                if runtime.is_none() {
                    *runtime = Some(SearchRuntime {
                        engine: SearchEngine::load(
                            &store,
                            DEFAULT_OWNER_ID,
                            config.search.transcript_hit_boost,
                        )?,
                        // CPU starts cold in under the 500 ms search budget, while CoreML's
                        // one-time graph compilation is optimized for batch image ingestion.
                        // Task 8 goldens enforce that both providers share the same CLIP space.
                        embedder: Embedder::new(
                            paths.models(),
                            ProviderPreference::Cpu,
                            config.limits.threads,
                        )?,
                        generation: None,
                    });
                }
                let runtime = runtime.as_mut().context("search runtime was not created")?;
                // Feedback recorded since the last search defers its retrain to here so a
                // pick click never blocks the main thread on a full feedback-table pass.
                // On failure, re-arm the flag so the next search retries it.
                if retrain_dirty.swap(false, Ordering::AcqRel) {
                    if let Err(error) = retrain_style_profile(&mut store, DEFAULT_OWNER_ID) {
                        retrain_dirty.store(true, Ordering::Release);
                        eprintln!("deferred style retrain failed: {error:#}");
                    }
                }
                let generation = store.data_version()?;
                if runtime.generation != Some(generation) {
                    runtime.engine.reload(&store)?;
                    runtime.generation = Some(generation);
                }
                let SearchRuntime {
                    engine, embedder, ..
                } = runtime;
                engine.search_assets(&store, &mut |text: &str| embedder.embed_text(text), &q, top)
            })())
        })
        .await
        .map_err(|error| format!("search worker failed: {error}"))?
    }

    #[tauri::command]
    fn shot_detail(
        id: String,
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<ShotDetailView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let shot = store
                .shot_by_id(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("shot {id} was not found"))?;
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, &shot.video_id)?
                .with_context(|| format!("video {} was not found", shot.video_id))?;
            let playback_path = store
                .video_source_metadata(DEFAULT_OWNER_ID, &video.id)?
                .and_then(|metadata| metadata.proxy_rel)
                .map(|relative| store.proxy_path(&relative))
                .transpose()?
                .filter(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from(&video.path));
            allow_asset_path(&app, &playback_path).map_err(anyhow::Error::msg)?;
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
            let assessment = store.aesthetic_assessment(
                DEFAULT_OWNER_ID,
                crush_store::MediaKind::Shot,
                &shot.id,
            )?;
            let style = personal_style_score(
                &store,
                DEFAULT_OWNER_ID,
                crush_store::MediaKind::Shot,
                &shot.id,
            )?;
            Ok(ShotDetailView {
                id: shot.id,
                video_id: shot.video_id,
                video_path: playback_path.display().to_string(),
                idx: shot.idx,
                shot_count,
                start_s: shot.start_s,
                end_s: shot.end_s,
                rep_frame_s: shot.rep_frame_s,
                fps: video.fps,
                thumb_path,
                transcripts,
                aesthetic_score: assessment.as_ref().map(|value| value.overall),
                personal_style_score: style,
                technical_score: assessment.as_ref().map(|value| value.technical_quality),
                composition_score: assessment.as_ref().map(|value| value.composition_quality),
                moment_score: assessment.as_ref().map(|value| value.moment_story),
                analysis_summary: assessment.as_ref().and_then(assessment_summary),
            })
        })())
    }

    #[tauri::command]
    fn photo_detail(
        id: String,
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PhotoDetailView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let photo = store
                .photo_by_id(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("photo {id} was not found"))?;
            let display_path = store
                .photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)?
                .and_then(|metadata| metadata.proxy_rel)
                .map(|relative| store.proxy_path(&relative))
                .transpose()?
                .filter(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from(&photo.path));
            allow_asset_path(&app, &display_path).map_err(anyhow::Error::msg)?;
            let annotation = store.editorial_annotation(
                DEFAULT_OWNER_ID,
                crush_store::MediaKind::Photo,
                &photo.id,
            )?;
            let assessment = store.aesthetic_assessment(
                DEFAULT_OWNER_ID,
                crush_store::MediaKind::Photo,
                &photo.id,
            )?;
            let style = personal_style_score(
                &store,
                DEFAULT_OWNER_ID,
                crush_store::MediaKind::Photo,
                &photo.id,
            )?;
            Ok(PhotoDetailView {
                id: photo.id,
                photo_path: display_path.display().to_string(),
                width: photo.width,
                height: photo.height,
                format: photo.format,
                quality: annotation.as_ref().and_then(|value| value.quality),
                aesthetic_score: assessment.as_ref().map(|value| value.overall),
                personal_style_score: style,
                technical_score: assessment.as_ref().map(|value| value.technical_quality),
                composition_score: assessment.as_ref().map(|value| value.composition_quality),
                moment_score: assessment.as_ref().map(|value| value.moment_story),
                analysis_summary: assessment.as_ref().and_then(assessment_summary),
                description: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.description.clone()),
                tags: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.tags.clone()),
                notes: annotation.map_or_else(String::new, |value| value.notes),
            })
        })())
    }

    fn assessment_summary(assessment: &crush_store::AestheticAssessment) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&assessment.explanation_json)
            .ok()?
            .get("summary")?
            .as_str()
            .map(str::to_owned)
    }

    #[tauri::command]
    async fn record_feedback(
        asset_type: String,
        id: String,
        signal: String,
        value: Option<f64>,
        context: Option<String>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<String> {
        let paths = state.paths.clone();
        let retrain_dirty = Arc::clone(&state.retrain_dirty);
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                let media_kind = match asset_type.as_str() {
                    "photo" => MediaKind::Photo,
                    "video" => MediaKind::Shot,
                    _ => anyhow::bail!("unsupported asset type {asset_type:?}"),
                };
                let signal = match signal.as_str() {
                    "pick" => FeedbackSignal::Pick,
                    "reject" => FeedbackSignal::Reject,
                    "rating" => FeedbackSignal::Rating,
                    _ => anyhow::bail!("unsupported feedback signal {signal:?}"),
                };
                // Feedback events are append-only, so anything that slips through here is
                // permanent: validate the value and the asset before writing.
                let value = match signal {
                    FeedbackSignal::Rating => {
                        let value =
                            value.context("rating feedback requires a value from 1 to 5")?;
                        ensure!(
                            (1.0..=5.0).contains(&value),
                            "rating feedback must be between 1 and 5, got {value}"
                        );
                        Some(value)
                    }
                    FeedbackSignal::Pick => {
                        ensure!(value == Some(1.0), "pick feedback requires value 1");
                        Some(1.0)
                    }
                    FeedbackSignal::Reject => {
                        ensure!(value == Some(-1.0), "reject feedback requires value -1");
                        Some(-1.0)
                    }
                    _ => anyhow::bail!("unsupported feedback signal"),
                };
                let store = Store::open(&paths.root)?;
                let exists = match media_kind {
                    MediaKind::Photo => store.photo_by_id(DEFAULT_OWNER_ID, &id)?.is_some(),
                    MediaKind::Shot => store.shot_by_id(DEFAULT_OWNER_ID, &id)?.is_some(),
                };
                let kind = match media_kind {
                    MediaKind::Photo => "photo",
                    MediaKind::Shot => "shot",
                };
                ensure!(exists, "no {kind} exists with id {id}");
                let event_id = Uuid::new_v4().to_string();
                store.append_feedback(
                    DEFAULT_OWNER_ID,
                    &FeedbackEvent {
                        id: event_id.clone(),
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        media_kind,
                        media_id: id,
                        signal,
                        value,
                        compared_media_kind: None,
                        compared_media_id: None,
                        context_json: serde_json::json!({ "query": context.unwrap_or_default() })
                            .to_string(),
                        created_at: chrono::Utc::now(),
                    },
                )?;
                // Retraining scans the whole feedback table, so it is deferred to the next
                // search rather than run inline on every click.
                retrain_dirty.store(true, Ordering::Release);
                Ok(event_id)
            })())
        })
        .await
        .map_err(|error| format!("feedback worker failed: {error}"))?
    }

    // ---- Style + reference sets (Task 018b) ----

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ReferenceSetView {
        id: String,
        name: String,
        context_key: String,
        description: String,
        scope: String,
        status: String,
        item_count: usize,
        created_at: String,
        confirmed_at: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StyleProfileStatusView {
        /// True only when the active profile carries `learned = 1`, which the held-out
        /// evaluation gate (Task 018a) sets at train time; the UI never says "Learned"
        /// without it.
        learned: bool,
        has_active_profile: bool,
        profile_id: Option<String>,
        context_key: Option<String>,
        version: Option<i64>,
        algorithm_version: Option<String>,
        sample_count: Option<i64>,
        held_out_metric: Option<f64>,
        baseline_metric: Option<f64>,
        metrics: Option<serde_json::Value>,
        reference_sets_total: usize,
        reference_sets_confirmed: usize,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RetrainOutcome {
        /// False when the evidence is below the trainer's minimum-samples floor and the
        /// previous profile was left untouched (sparse evidence never invents certainty).
        trained: bool,
        status: StyleProfileStatusView,
    }

    /// Reference-set items use the store's `photo`/`shot` kinds; the UI talks about
    /// "photo"/"video" assets (mirroring `record_feedback`'s asset type vocabulary).
    fn parse_media_kind(value: &str) -> anyhow::Result<MediaKind> {
        match value.trim() {
            "photo" => Ok(MediaKind::Photo),
            "shot" | "video" => Ok(MediaKind::Shot),
            other => anyhow::bail!("unsupported media kind {other:?}"),
        }
    }

    fn reference_set_view(store: &Store, set: ReferenceSet) -> anyhow::Result<ReferenceSetView> {
        let item_count = store.reference_set_items(DEFAULT_OWNER_ID, &set.id)?.len();
        Ok(ReferenceSetView {
            id: set.id,
            name: set.name,
            context_key: set.context_key,
            description: set.description,
            scope: crush_store::reference_scope_to_str(set.scope).to_owned(),
            status: crush_store::reference_status_to_str(set.status).to_owned(),
            item_count,
            created_at: set.created_at.to_rfc3339(),
            confirmed_at: set.confirmed_at.map(|value| value.to_rfc3339()),
        })
    }

    /// Status surface for the "learned vs. baseline" badge. A profile that exists but never
    /// passed the eval gate reports `learned: false`, so the UI shows the general-model copy
    /// (the ranking path ignores unlearned profiles too).
    fn style_profile_status_view(store: &Store) -> anyhow::Result<StyleProfileStatusView> {
        let profile = store.active_style_profile(DEFAULT_OWNER_ID)?;
        let sets = store.reference_set_list(DEFAULT_OWNER_ID)?;
        let confirmed = sets
            .iter()
            .filter(|set| set.status == ReferenceSetStatus::Confirmed)
            .count();
        Ok(match profile {
            Some(profile) => StyleProfileStatusView {
                learned: profile.learned,
                has_active_profile: true,
                profile_id: Some(profile.id),
                context_key: Some(profile.context_key),
                version: Some(profile.version),
                algorithm_version: Some(profile.algorithm_version),
                sample_count: Some(profile.sample_count),
                held_out_metric: profile.held_out_metric,
                baseline_metric: profile.baseline_metric,
                metrics: serde_json::from_str(&profile.metrics_json).ok(),
                reference_sets_total: sets.len(),
                reference_sets_confirmed: confirmed,
            },
            None => StyleProfileStatusView {
                learned: false,
                has_active_profile: false,
                profile_id: None,
                context_key: None,
                version: None,
                algorithm_version: None,
                sample_count: None,
                held_out_metric: None,
                baseline_metric: None,
                metrics: None,
                reference_sets_total: sets.len(),
                reference_sets_confirmed: confirmed,
            },
        })
    }

    #[tauri::command]
    fn reference_set_create(
        name: String,
        context_key: String,
        description: String,
        scope: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let name = name.trim();
            ensure!(!name.is_empty(), "reference set name must not be empty");
            let context_key = context_key.trim();
            let context_key = if context_key.is_empty() {
                "default"
            } else {
                context_key
            };
            let scope = match scope.trim() {
                "whole_set" => ReferenceSetScope::WholeSet,
                "selected" => ReferenceSetScope::Selected,
                other => anyhow::bail!("unsupported reference set scope {other:?}"),
            };
            let store = Store::open(&state.paths.root)?;
            store.reference_set_create(
                DEFAULT_OWNER_ID,
                &ReferenceSet {
                    id: Uuid::new_v4().to_string(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    name: name.to_owned(),
                    context_key: context_key.to_owned(),
                    description: description.trim().to_owned(),
                    scope,
                    // Sets are inert until the user explicitly confirms them.
                    status: ReferenceSetStatus::Unconfirmed,
                    source_collection_id: None,
                    created_at: chrono::Utc::now(),
                    confirmed_at: None,
                },
            )?;
            Ok(())
        })())
    }

    #[tauri::command]
    fn reference_set_list(state: State<'_, RuntimeState>) -> CommandResult<Vec<ReferenceSetView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let sets = store.reference_set_list(DEFAULT_OWNER_ID)?;
            sets.into_iter()
                .map(|set| reference_set_view(&store, set))
                .collect()
        })())
    }

    #[tauri::command]
    fn reference_set_add_item(
        set_id: String,
        media_kind: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let media_kind = parse_media_kind(&media_kind)?;
            let store = Store::open(&state.paths.root)?;
            // Mirror record_feedback: validate the asset before writing anything.
            let exists = match media_kind {
                MediaKind::Photo => store.photo_by_id(DEFAULT_OWNER_ID, &media_id)?.is_some(),
                MediaKind::Shot => store.shot_by_id(DEFAULT_OWNER_ID, &media_id)?.is_some(),
            };
            let kind = match media_kind {
                MediaKind::Photo => "photo",
                MediaKind::Shot => "shot",
            };
            ensure!(exists, "no {kind} exists with id {media_id}");
            store.reference_set_add_item(
                DEFAULT_OWNER_ID,
                &ReferenceSetItem {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    set_id,
                    media_kind,
                    media_id,
                    role: ReferenceItemRole::Positive,
                    added_at: chrono::Utc::now(),
                },
            )?;
            Ok(())
        })())
    }

    #[tauri::command]
    fn reference_set_remove_item(
        set_id: String,
        media_kind: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let media_kind = parse_media_kind(&media_kind)?;
            let store = Store::open(&state.paths.root)?;
            store.reference_set_remove_item(DEFAULT_OWNER_ID, &set_id, media_kind, &media_id)
        })())
    }

    #[tauri::command]
    fn reference_set_confirm(
        set_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.reference_set_confirm(DEFAULT_OWNER_ID, &set_id)
        })())
    }

    #[tauri::command]
    fn reference_set_disable(
        set_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.reference_set_disable(DEFAULT_OWNER_ID, &set_id)
        })())
    }

    #[tauri::command]
    fn reference_set_delete(
        set_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.reference_set_delete(DEFAULT_OWNER_ID, &set_id)
        })())
    }

    #[tauri::command]
    fn style_profile_status(
        state: State<'_, RuntimeState>,
    ) -> CommandResult<StyleProfileStatusView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            style_profile_status_view(&store)
        })())
    }

    #[tauri::command]
    fn style_profile_reset(state: State<'_, RuntimeState>) -> CommandResult<usize> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.reset_style_profiles(DEFAULT_OWNER_ID)
        })())
    }

    #[tauri::command]
    async fn style_profile_retrain(
        state: State<'_, RuntimeState>,
    ) -> CommandResult<RetrainOutcome> {
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                let mut store = Store::open(&paths.root)?;
                let trained = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)?.is_some();
                let status = style_profile_status_view(&store)?;
                Ok(RetrainOutcome { trained, status })
            })())
        })
        .await
        .map_err(|error| format!("style retrain worker failed: {error}"))?
    }

    #[tauri::command]
    fn shot_at_index(
        video_id: String,
        idx: i64,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Option<String>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .shots_for_video(DEFAULT_OWNER_ID, &video_id)?
                .into_iter()
                .find(|shot| shot.idx == idx)
                .map(|shot| shot.id))
        })())
    }

    /// Lets the webview load a footage file or folder through Tauri's `asset:` protocol.
    /// The scope starts empty and only ever grows with paths the user added or is viewing.
    fn allow_asset_path(app: &AppHandle, path: &Path) -> CommandResult<()> {
        let scope = app.asset_protocol_scope();
        let result = if path.is_dir() {
            scope.allow_directory(path, true)
        } else {
            scope.allow_file(path)
        };
        result.map_err(|error| format!("could not expose {} to the app: {error}", path.display()))
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

    fn photo_status_name(status: PhotoStatus) -> &'static str {
        match status {
            PhotoStatus::Pending => "pending",
            PhotoStatus::Embedded => "embedded",
            PhotoStatus::Done => "done",
            PhotoStatus::Failed => "failed",
        }
    }

    fn ingest_summary(summary: &IngestSummary) -> String {
        format!(
            "discovered={} photos={} indexed={} indexed_photos={} skipped={} failed={} recovered={} vectors={}",
            summary.discovered,
            summary.discovered_photos,
            summary.indexed,
            summary.indexed_photos,
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

    /// Clears the ingest slot only if it still belongs to `job_id`, so a slot claimed by a
    /// newer ingest is never released by an older task's cleanup path.
    fn release_ingest_slot(active_ingest: &Mutex<Option<ActiveIngest>>, job_id: &str) {
        if let Ok(mut active) = active_ingest.lock() {
            if active
                .as_ref()
                .is_some_and(|current| current.job_id == job_id)
            {
                *active = None;
            }
        }
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
                let store = Store::open(&paths.root)?;
                store.fail_running_jobs_as_interrupted(DEFAULT_OWNER_ID)?;
                let scope = app.asset_protocol_scope();
                let thumbs_dir = paths.thumbs();
                std::fs::create_dir_all(&thumbs_dir)?;
                scope.allow_directory(&thumbs_dir, true)?;
                let proxies_dir = paths.root.join("proxies");
                std::fs::create_dir_all(&proxies_dir)?;
                scope.allow_directory(&proxies_dir, true)?;
                for video in store.videos(DEFAULT_OWNER_ID)? {
                    if let Err(error) = scope.allow_file(&video.path) {
                        eprintln!("could not expose {} to the webview: {error}", video.path);
                    }
                }
                for photo in store.photos(DEFAULT_OWNER_ID)? {
                    if let Err(error) = scope.allow_file(&photo.path) {
                        eprintln!("could not expose {} to the webview: {error}", photo.path);
                    }
                }
                app.manage(RuntimeState {
                    config,
                    paths,
                    background: Arc::new(Mutex::new(BTreeMap::new())),
                    active_ingest: Arc::new(Mutex::new(None)),
                    search: Arc::new(Mutex::new(None)),
                    retrain_dirty: Arc::new(AtomicBool::new(false)),
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
                photo_detail,
                record_feedback,
                shot_at_index,
                reference_set_create,
                reference_set_list,
                reference_set_add_item,
                reference_set_remove_item,
                reference_set_confirm,
                reference_set_disable,
                reference_set_delete,
                style_profile_status,
                style_profile_reset,
                style_profile_retrain,
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
            assert!(report.contains("schema=6"));
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(not(target_os = "macos"))]
pub fn run() {
    eprintln!("Crush desktop is currently supported on macOS only");
}
