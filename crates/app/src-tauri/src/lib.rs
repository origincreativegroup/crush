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
    use crush_pipeline::{
        render::RenderRecoverySummary, source::PhotoOutputPreset, IngestSummary, Pipeline,
    };
    use crush_search::{
        personal_style_score, retrain_style_profile, selects_candidates as rank_selects_candidates,
        AssetSearchResult, SearchEngine, SelectsCandidates,
    };
    use crush_stage_embed::embedder::{Embedder, ProviderPreference};
    use crush_stage_split::ffmpeg::{self, ClipOutputPreset};
    use crush_store::{
        reference_scope_from_str, reference_scope_to_str, reference_status_to_str, AssetFilter,
        Collection, CollectionItem, EditorialAnnotation, EmbeddingMeta, FeedbackEvent,
        FeedbackSignal, JobFilter, LibraryAsset, MediaKind, NewRenderJob, PhotoStatus, Plan,
        PlanItem, PlanItemPatch, PlanOrigin, ReferenceItemRole, ReferenceSet, ReferenceSetItem,
        ReferenceSetScope, ReferenceSetStatus, RenderRecipe, RenderRecipeKind, ReviewOp,
        SafetyFlags, SavedSearch, StackItem, StackItemRole, StackMediaKind, Store, VersionStack,
        VideoStatus,
    };
    use serde::{Deserialize, Serialize};
    use tauri::{AppHandle, Emitter, Manager, State};
    use uuid::Uuid;

    type CommandResult<T> = Result<T, String>;

    struct RuntimeState {
        config: Config,
        paths: AppPaths,
        background: Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
        active_ingest: Arc<Mutex<Option<ActiveIngest>>>,
        active_renders: Arc<Mutex<BTreeMap<String, CancellationToken>>>,
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
        /// Structured relink counts, set only for finished ingest tasks so the Library
        /// can report "moved/renamed/duplicate → relinked" without parsing the detail
        /// text. `moved`/`renamed` count only files whose old copy is really gone; a
        /// same-content file whose old path still exists is a duplicate copy.
        moved: Option<usize>,
        renamed: Option<usize>,
        duplicated: Option<usize>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TaskStarted {
        job_id: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RemoveOutcome {
        removed: bool,
        kind: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RelinkOutcomeView {
        media_kind: String,
        id: String,
        from_path: String,
        new_path: String,
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
        /// The recorded source file is not present on disk right now — the honest
        /// "missing source" state that enables the "Locate moved file…" action.
        source_missing: bool,
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

    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LibraryFilterArgs {
        kind: Option<String>,
        status: Option<String>,
        usable: Option<bool>,
        faces_visible: Option<bool>,
        blur_required: Option<bool>,
        quality_min: Option<i64>,
        feedback: Option<String>,
        collection_id: Option<String>,
        stack_id: Option<String>,
        context_key: Option<String>,
        search: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LibraryAssetView {
        media_kind: String,
        media_id: String,
        path: String,
        /// Absolute path resolved from the store's relative thumb location, ready for
        /// `convertFileSrc` in the webview.
        thumb_path: Option<String>,
        status: String,
        indexed_at: Option<String>,
        video_id: Option<String>,
        start_s: Option<f64>,
        end_s: Option<f64>,
        width: Option<i64>,
        height: Option<i64>,
        quality: Option<i64>,
        usable: bool,
        standout: bool,
        faces_visible: bool,
        nametags_visible: bool,
        blur_required: bool,
        tags: String,
        collection_ids: Vec<String>,
        stack_ids: Vec<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LibraryCountsView {
        photos: i64,
        shots: i64,
        picks: i64,
        rejects: i64,
        flagged: i64,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AnnotationView {
        description: String,
        subjects: String,
        action: String,
        tags: String,
        notes: String,
        standout: bool,
        usable: bool,
        faces_visible: bool,
        nametags_visible: bool,
        blur_required: bool,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CollectionView {
        id: String,
        name: String,
        description: String,
        created_at: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CollectionItemArgs {
        asset_type: String,
        media_id: String,
        context_key: Option<String>,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CollectionItemView {
        media_kind: String,
        media_id: String,
        context_key: Option<String>,
        marked: bool,
        added_at: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StackView {
        id: String,
        name: String,
        created_at: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SavedSearchView {
        id: String,
        name: String,
        query: String,
        context_key: String,
        filters_json: String,
        created_at: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReviewOpArgs {
        op: String,
        asset_type: Option<String>,
        media_id: Option<String>,
        rating: Option<i64>,
        faces_visible: Option<bool>,
        nametags_visible: Option<bool>,
        blur_required: Option<bool>,
        usable: Option<bool>,
        collection_id: Option<String>,
        context_key: Option<String>,
    }

    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AnnotationPatchArgs {
        description: Option<String>,
        subjects: Option<String>,
        action: Option<String>,
        tags: Option<String>,
        notes: Option<String>,
        quality: Option<i64>,
        crop_x: Option<f64>,
        grade_json: Option<String>,
        standout: Option<bool>,
    }

    #[tauri::command]
    fn doctor(state: State<'_, RuntimeState>) -> CommandResult<String> {
        command_result(doctor_report(&state.config, &state.paths))
    }

    /// One render-preset fact per preset, served from the preset enums so the UI never keeps
    /// its own drifting copies. Frozen contract values (`id`, muxer, media type) are exposed
    /// alongside the UI labels and save-dialog extensions.
    #[derive(Debug, Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RenderPresetView {
        id: &'static str,
        label: &'static str,
        extension: &'static str,
        extensions: &'static [&'static str],
        media_type: &'static str,
        /// FFmpeg muxer for video presets; photos are rendered by the image backend.
        muxer: Option<&'static str>,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RenderPresetCatalog {
        photo: Vec<RenderPresetView>,
        clip: Vec<RenderPresetView>,
    }

    fn render_preset_catalog() -> RenderPresetCatalog {
        let view = |preset: PhotoOutputPreset| RenderPresetView {
            id: preset.as_str(),
            label: preset.label(),
            extension: preset.extension(),
            extensions: preset.extensions(),
            media_type: preset.media_type(),
            muxer: None,
        };
        let clip_view = |preset: ClipOutputPreset| RenderPresetView {
            id: preset.as_str(),
            label: preset.label(),
            extension: preset.extension(),
            extensions: preset.extensions(),
            media_type: preset.media_type(),
            muxer: Some(preset.muxer()),
        };
        RenderPresetCatalog {
            photo: PhotoOutputPreset::ALL.iter().copied().map(view).collect(),
            clip: ClipOutputPreset::ALL
                .iter()
                .copied()
                .map(clip_view)
                .collect(),
        }
    }

    #[tauri::command]
    fn list_render_presets() -> RenderPresetCatalog {
        render_preset_catalog()
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
            found
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
                .collect::<anyhow::Result<Vec<_>>>()
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
                moved: None,
                renamed: None,
                duplicated: None,
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
                moved: None,
                renamed: None,
                duplicated: None,
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
            let completed = complete_ingest_background(&tasks, &spawned_job_id, result, cancelled);
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
                    source_missing: !Path::new(&video.path).exists(),
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
                    source_missing: !Path::new(&photo.path).exists(),
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
            let target = id.clone();
            start_reindex(app, state, video.path, move |pipeline| {
                pipeline.resplit(&target, false)
            })
        })())
    }

    #[tauri::command]
    fn reindex_asset(
        id: String,
        app: AppHandle,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<TaskStarted> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            if let Some(photo) = store.photo_by_id(DEFAULT_OWNER_ID, &id)? {
                drop(store);
                let path = photo.path.clone();
                let target = id.clone();
                return start_reindex(app, state, path, move |pipeline| {
                    pipeline.reprocess_photo(&target, false)
                });
            }
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("asset {id} was not found"))?;
            drop(store);
            let target = id.clone();
            start_reindex(app, state, video.path.clone(), move |pipeline| {
                pipeline.resplit(&target, false)
            })
        })())
    }

    #[tauri::command]
    fn remove_asset(id: String, state: State<'_, RuntimeState>) -> CommandResult<RemoveOutcome> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            if let Some(photo) = store.photo_by_id(DEFAULT_OWNER_ID, &id)? {
                let mut files = Vec::new();
                if let Some(metadata) = store.photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)? {
                    if let Some(rel) = metadata.proxy_rel {
                        files.push(store.proxy_path(&rel)?);
                    }
                }
                if let Some(rel) = &photo.thumb_rel {
                    files.push(store.thumbnail_path(rel)?);
                }
                if store.delete_photo_cascade(DEFAULT_OWNER_ID, &photo.id)? {
                    for path in files {
                        let _ = std::fs::remove_file(path);
                    }
                    return Ok(RemoveOutcome {
                        removed: true,
                        kind: "photo".to_owned(),
                    });
                }
            }
            if let Some(video) = store.video_by_id(DEFAULT_OWNER_ID, &id)? {
                let mut files = Vec::new();
                if let Some(metadata) = store.video_source_metadata(DEFAULT_OWNER_ID, &video.id)? {
                    if let Some(relative) = metadata.proxy_rel {
                        files.push(store.proxy_path(&relative)?);
                    }
                }
                for shot in store.shots_for_video(DEFAULT_OWNER_ID, &video.id)? {
                    if let Some(rel) = shot.thumb_rel {
                        if let Ok(path) = store.thumbnail_path(&rel) {
                            files.push(path);
                        }
                    }
                }
                if store.delete_video_cascade(DEFAULT_OWNER_ID, &video.id)? {
                    for path in files {
                        let _ = std::fs::remove_file(path);
                    }
                    return Ok(RemoveOutcome {
                        removed: true,
                        kind: "video".to_owned(),
                    });
                }
            }
            anyhow::bail!("asset {id} was not found in the library")
        })())
    }

    /// "Locate moved file…": the user picks the file at its new location, Crush verifies
    /// the bytes are identical (SHA-256) to the indexed media, and the existing catalog
    /// row is re-pointed at the new path. A different file is refused and nothing is
    /// changed; no duplicate row is created and the original file is never modified.
    #[tauri::command]
    fn relink_asset(
        id: String,
        new_path: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<RelinkOutcomeView> {
        command_result((|| {
            let outcome = Pipeline::new(
                state.config.clone(),
                state.paths.clone(),
                CancellationToken::default(),
            )
            .relink(&id, Path::new(&new_path))?;
            Ok(RelinkOutcomeView {
                media_kind: outcome.media_kind.to_owned(),
                id: outcome.id,
                from_path: outcome.old_path,
                new_path: outcome.new_path,
            })
        })())
    }

    fn start_reindex(
        app: AppHandle,
        state: State<'_, RuntimeState>,
        path: String,
        run: impl FnOnce(&Pipeline) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<TaskStarted> {
        {
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
                    detail: Some(format!("re-indexing {path}")),
                    error: None,
                    moved: None,
                    renamed: None,
                    duplicated: None,
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
                let pipeline = Pipeline::new(config, paths, spawned_cancellation.clone());
                let result = run(&pipeline).map(|()| "re-index complete".to_owned());
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
        }
    }

    #[derive(Debug, Clone, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ReelStudioImportRequest {
        catalogue: String,
        #[serde(default)]
        originals: Vec<String>,
        #[serde(default)]
        library: Option<String>,
        #[serde(default)]
        recipes: Vec<String>,
        #[serde(default)]
        context_key: Option<String>,
        #[serde(default)]
        match_by_hash: bool,
        #[serde(default)]
        keyframe_tolerance_s: Option<f64>,
        #[serde(default)]
        apply: bool,
    }

    /// Task 022: dry-run or apply a Reel Studio catalogue/recipe import. Long-running (hashes the
    /// catalogue and, with `matchByHash`, original footage), so it leaves the UI thread.
    #[tauri::command]
    async fn import_reel_studio(
        request: ReelStudioImportRequest,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<crush_pipeline::reel_studio_import::ImportReport> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                ensure!(
                    !request.catalogue.trim().is_empty(),
                    "choose the Reel Studio clips.db first"
                );
                let options = crush_pipeline::reel_studio_import::ImportOptions {
                    catalogue: PathBuf::from(&request.catalogue),
                    originals: request.originals.iter().map(PathBuf::from).collect(),
                    library: request.library.as_deref().map(PathBuf::from),
                    recipes: request.recipes.iter().map(PathBuf::from).collect(),
                    context_key: request
                        .context_key
                        .filter(|key| !key.trim().is_empty())
                        .unwrap_or_else(|| "default".to_owned()),
                    apply: request.apply,
                    match_by_hash: request.match_by_hash,
                    keyframe_tolerance_s: request.keyframe_tolerance_s.unwrap_or(
                        crush_pipeline::reel_studio_import::DEFAULT_KEYFRAME_TOLERANCE_S,
                    ),
                    threads: config.limits.threads,
                };
                let mut store = Store::open(&paths.root)?;
                crush_pipeline::reel_studio_import::import_reel_studio(&mut store, &options)
            })())
        })
        .await
        .map_err(|error| format!("import worker failed: {error}"))?
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
    #[allow(clippy::too_many_arguments)] // Backward-compatible Tauri command argument surface.
    async fn record_feedback(
        asset_type: String,
        id: String,
        signal: String,
        value: Option<f64>,
        context: Option<String>,
        context_key: Option<String>,
        compared_asset_type: Option<String>,
        compared_id: Option<String>,
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
                    "prefer" => FeedbackSignal::Prefer,
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
                    FeedbackSignal::Prefer => {
                        ensure!(value.is_none(), "prefer feedback does not take a value");
                        None
                    }
                    _ => anyhow::bail!("unsupported feedback signal"),
                };
                let compared_media_kind = match compared_asset_type.as_deref() {
                    Some("photo") => Some(MediaKind::Photo),
                    Some("video") => Some(MediaKind::Shot),
                    Some(other) => {
                        anyhow::bail!("unsupported compared asset type {other:?}")
                    }
                    None => None,
                };
                let store = Store::open(&paths.root)?;
                let exists = match media_kind {
                    MediaKind::Photo => store.photo_by_id(DEFAULT_OWNER_ID, &id)?.is_some(),
                    MediaKind::Shot => store.shot_by_id(DEFAULT_OWNER_ID, &id)?.is_some(),
                    MediaKind::Span => store.manual_span_by_id(DEFAULT_OWNER_ID, &id)?.is_some(),
                };
                let kind = match media_kind {
                    MediaKind::Photo => "photo",
                    MediaKind::Shot => "shot",
                    MediaKind::Span => "span",
                };
                ensure!(exists, "no {kind} exists with id {id}");
                if let Some(compared_id) = &compared_id {
                    let compared_exists = match compared_media_kind {
                        Some(MediaKind::Photo) => {
                            store.photo_by_id(DEFAULT_OWNER_ID, compared_id)?.is_some()
                        }
                        Some(MediaKind::Shot) => {
                            store.shot_by_id(DEFAULT_OWNER_ID, compared_id)?.is_some()
                        }
                        Some(MediaKind::Span) => store
                            .manual_span_by_id(DEFAULT_OWNER_ID, compared_id)?
                            .is_some(),
                        None => false,
                    };
                    ensure!(
                        compared_exists,
                        "no compared asset exists with id {compared_id}"
                    );
                }
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
                        compared_media_kind,
                        compared_media_id: compared_id,
                        context_json: feedback_context_json(
                            context.as_deref(),
                            context_key.as_deref(),
                        )?,
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

    fn feedback_context_json(
        query: Option<&str>,
        context_key: Option<&str>,
    ) -> anyhow::Result<String> {
        let key = context_key.unwrap_or("default").trim();
        ensure!(!key.is_empty(), "feedback context key must not be empty");
        Ok(serde_json::json!({ "query": query.unwrap_or_default(), "context": key }).to_string())
    }

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
        /// Automated eval result only. The held-out proof review was recorded 2026-08-31
        /// (docs/style-proof-review.md): the UI labels a gate-passed profile "Learned
        /// profile" with the required scope line; gate-failed profiles keep the
        /// experimental copy. This flag alone never unlocks stronger claims.
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
            "span" => Ok(MediaKind::Span),
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
            scope: reference_scope_to_str(set.scope).to_owned(),
            status: reference_status_to_str(set.status).to_owned(),
            item_count,
            created_at: set.created_at.to_rfc3339(),
            confirmed_at: set.confirmed_at.map(|value| value.to_rfc3339()),
        })
    }

    /// Status surface for the "learned vs. baseline" badge. A profile that exists but never
    /// passed the eval gate reports `learned: false`, so the UI keeps the experimental copy
    /// per the 2026-08-31 verdict conditions (docs/style-proof-review.md); the ranking path
    /// ignores unlearned profiles too.
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
                MediaKind::Span => store
                    .manual_span_by_id(DEFAULT_OWNER_ID, &media_id)?
                    .is_some(),
            };
            let kind = match media_kind {
                MediaKind::Photo => "photo",
                MediaKind::Shot => "shot",
                MediaKind::Span => "span",
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
            let mut store = Store::open(&state.paths.root)?;
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
    fn reference_set_delete(set_id: String, state: State<'_, RuntimeState>) -> CommandResult<bool> {
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

    // ---- Library organization (Task 019a) ----

    /// Mirrors `parse_media_kind` for the library commands; the UI talks about
    /// "photo"/"video" assets.
    fn parse_library_kind(value: &str) -> anyhow::Result<MediaKind> {
        match value {
            "photo" => Ok(MediaKind::Photo),
            "video" | "shot" => Ok(MediaKind::Shot),
            "span" => Ok(MediaKind::Span),
            other => anyhow::bail!("unsupported asset type {other:?}"),
        }
    }

    fn parse_stack_asset_type(value: &str) -> anyhow::Result<StackMediaKind> {
        match value {
            "photo" => Ok(StackMediaKind::Photo),
            "video" => Ok(StackMediaKind::Video),
            other => anyhow::bail!("unsupported stack asset type {other:?}"),
        }
    }

    fn parse_stack_role(value: &str) -> anyhow::Result<StackItemRole> {
        match value {
            "original" => Ok(StackItemRole::Original),
            "derived" => Ok(StackItemRole::Derived),
            other => anyhow::bail!("unsupported stack item role {other:?}"),
        }
    }

    fn library_asset_view(asset: LibraryAsset, thumb_path: Option<String>) -> LibraryAssetView {
        LibraryAssetView {
            media_kind: match asset.media_kind {
                MediaKind::Photo => "photo".to_owned(),
                MediaKind::Shot => "shot".to_owned(),
                MediaKind::Span => "span".to_owned(),
            },
            media_id: asset.media_id,
            path: asset.path,
            thumb_path,
            status: asset.status,
            indexed_at: asset.indexed_at.map(|value| value.to_rfc3339()),
            video_id: asset.video_id,
            start_s: asset.start_s,
            end_s: asset.end_s,
            width: asset.width,
            height: asset.height,
            quality: asset.quality,
            usable: asset.usable,
            standout: asset.standout,
            faces_visible: asset.faces_visible,
            nametags_visible: asset.nametags_visible,
            blur_required: asset.blur_required,
            tags: asset.tags,
            collection_ids: asset.collection_ids,
            stack_ids: asset.stack_ids,
        }
    }

    fn collection_view(collection: Collection) -> CollectionView {
        CollectionView {
            id: collection.id,
            name: collection.name,
            description: collection.description,
            created_at: collection.created_at.to_rfc3339(),
        }
    }

    fn stack_view(stack: VersionStack) -> StackView {
        StackView {
            id: stack.id,
            name: stack.name,
            created_at: stack.created_at.to_rfc3339(),
        }
    }

    #[tauri::command]
    fn library_browse(
        filter: Option<LibraryFilterArgs>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<LibraryAssetView>> {
        command_result((|| {
            let args = filter.unwrap_or_default();
            let kind = match args.kind.as_deref() {
                Some("photo") => Some(MediaKind::Photo),
                Some("shot" | "video") => Some(MediaKind::Shot),
                Some(other) => anyhow::bail!("unsupported asset kind {other:?}"),
                None => None,
            };
            let asset_filter = AssetFilter {
                kind,
                status: args.status,
                usable: args.usable,
                faces_visible: args.faces_visible,
                blur_required: args.blur_required,
                quality_min: args.quality_min,
                feedback: args.feedback,
                collection_id: args.collection_id,
                stack_id: args.stack_id,
                context_key: args.context_key,
                search: args.search,
            };
            let store = Store::open(&state.paths.root)?;
            let assets = store.browse_assets(DEFAULT_OWNER_ID, &asset_filter)?;
            let views = assets
                .into_iter()
                .map(|asset| {
                    // Resolve the stored relative thumb to an absolute path so the grid can
                    // load it through `convertFileSrc` without a second round trip.
                    let thumb_path = asset
                        .thumb_rel
                        .as_deref()
                        .map(|relative| store.thumbnail_path(relative))
                        .transpose()?
                        .map(|path| path.display().to_string());
                    Ok(library_asset_view(asset, thumb_path))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(views)
        })())
    }

    #[tauri::command]
    fn library_counts(state: State<'_, RuntimeState>) -> CommandResult<LibraryCountsView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let counts = store.library_counts(DEFAULT_OWNER_ID)?;
            Ok(LibraryCountsView {
                photos: counts.photos,
                shots: counts.shots,
                picks: counts.picks,
                rejects: counts.rejects,
                flagged: counts.flagged,
            })
        })())
    }

    #[tauri::command]
    fn collection_create(
        name: String,
        description: Option<String>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<CollectionView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let collection = Collection {
                id: Uuid::new_v4().to_string(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name,
                description: description.unwrap_or_default(),
                created_at: chrono::Utc::now(),
            };
            store.collection_create(DEFAULT_OWNER_ID, &collection)?;
            Ok(collection_view(collection))
        })())
    }

    #[tauri::command]
    fn collection_list(state: State<'_, RuntimeState>) -> CommandResult<Vec<CollectionView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .collection_list(DEFAULT_OWNER_ID)?
                .into_iter()
                .map(collection_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn collection_rename(
        id: String,
        name: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.collection_rename(DEFAULT_OWNER_ID, &id, &name)
        })())
    }

    #[tauri::command]
    fn collection_delete(id: String, state: State<'_, RuntimeState>) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.collection_delete(DEFAULT_OWNER_ID, &id)
        })())
    }

    #[tauri::command]
    fn collection_add_items(
        id: String,
        items: Vec<CollectionItemArgs>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<usize> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let added_at = chrono::Utc::now();
            let mut added = 0usize;
            for item in &items {
                store.collection_add_item(
                    DEFAULT_OWNER_ID,
                    &CollectionItem {
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        collection_id: id.clone(),
                        media_kind: parse_library_kind(&item.asset_type)?,
                        media_id: item.media_id.clone(),
                        context_key: item.context_key.clone(),
                        marked: false,
                        added_at,
                    },
                )?;
                added += 1;
            }
            Ok(added)
        })())
    }

    #[tauri::command]
    fn collection_remove_item(
        id: String,
        asset_type: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store.collection_remove_item(
                DEFAULT_OWNER_ID,
                &id,
                parse_library_kind(&asset_type)?,
                &media_id,
            )
        })())
    }

    #[tauri::command]
    fn collection_items(
        id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<CollectionItemView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .collection_items(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .map(|item| CollectionItemView {
                    media_kind: match item.media_kind {
                        MediaKind::Photo => "photo".to_owned(),
                        MediaKind::Shot => "shot".to_owned(),
                        MediaKind::Span => "span".to_owned(),
                    },
                    media_id: item.media_id,
                    context_key: item.context_key,
                    marked: item.marked,
                    added_at: item.added_at.to_rfc3339(),
                })
                .collect())
        })())
    }

    #[tauri::command]
    fn collection_set_item_marked(
        id: String,
        asset_type: String,
        media_id: String,
        marked: bool,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store.collection_set_item_marked(
                DEFAULT_OWNER_ID,
                &id,
                parse_library_kind(&asset_type)?,
                &media_id,
                marked,
            )
        })())
    }

    /// Creates a new `unconfirmed` reference set from a collection. The explicit confirm step
    /// afterwards is what makes the set contribute positive signal.
    #[tauri::command]
    fn collection_designate_reference_set(
        collection_id: String,
        name: String,
        context_key: String,
        scope: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<ReferenceSetView> {
        command_result((|| {
            let scope = reference_scope_from_str(&scope)?;
            let mut store = Store::open(&state.paths.root)?;
            let set = store.collection_designate_as_reference_set(
                DEFAULT_OWNER_ID,
                &collection_id,
                &name,
                &context_key,
                scope,
            )?;
            reference_set_view(&store, set)
        })())
    }

    #[tauri::command]
    fn stack_create(name: String, state: State<'_, RuntimeState>) -> CommandResult<StackView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let stack = VersionStack {
                id: Uuid::new_v4().to_string(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name,
                created_at: chrono::Utc::now(),
            };
            store.stack_create(DEFAULT_OWNER_ID, &stack)?;
            Ok(stack_view(stack))
        })())
    }

    #[tauri::command]
    fn stack_add_item(
        stack_id: String,
        asset_type: String,
        media_id: String,
        role: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store.stack_add_item(
                DEFAULT_OWNER_ID,
                &StackItem {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    stack_id,
                    media_kind: parse_stack_asset_type(&asset_type)?,
                    media_id,
                    role: parse_stack_role(&role)?,
                    added_at: chrono::Utc::now(),
                },
            )
        })())
    }

    #[tauri::command]
    fn stack_remove_item(
        stack_id: String,
        asset_type: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store.stack_remove_item(
                DEFAULT_OWNER_ID,
                &stack_id,
                parse_stack_asset_type(&asset_type)?,
                &media_id,
            )
        })())
    }

    #[tauri::command]
    fn stacks_for_asset(
        asset_type: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<StackView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .stacks_for_asset(
                    DEFAULT_OWNER_ID,
                    parse_stack_asset_type(&asset_type)?,
                    &media_id,
                )?
                .into_iter()
                .map(stack_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn stack_list(state: State<'_, RuntimeState>) -> CommandResult<Vec<StackView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .stack_list(DEFAULT_OWNER_ID)?
                .into_iter()
                .map(stack_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn saved_search_create(
        name: String,
        query: String,
        context_key: Option<String>,
        filters_json: Option<String>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<SavedSearchView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let saved = SavedSearch {
                id: Uuid::new_v4().to_string(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name,
                query,
                context_key: context_key.unwrap_or_else(|| "default".to_owned()),
                filters_json: filters_json.unwrap_or_else(|| "{}".to_owned()),
                created_at: chrono::Utc::now(),
            };
            store.saved_search_create(DEFAULT_OWNER_ID, &saved)?;
            Ok(SavedSearchView {
                id: saved.id,
                name: saved.name,
                query: saved.query,
                context_key: saved.context_key,
                filters_json: saved.filters_json,
                created_at: saved.created_at.to_rfc3339(),
            })
        })())
    }

    #[tauri::command]
    fn saved_search_list(state: State<'_, RuntimeState>) -> CommandResult<Vec<SavedSearchView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .saved_search_list(DEFAULT_OWNER_ID)?
                .into_iter()
                .map(|saved| SavedSearchView {
                    id: saved.id,
                    name: saved.name,
                    query: saved.query,
                    context_key: saved.context_key,
                    filters_json: saved.filters_json,
                    created_at: saved.created_at.to_rfc3339(),
                })
                .collect())
        })())
    }

    #[tauri::command]
    fn saved_search_delete(id: String, state: State<'_, RuntimeState>) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.saved_search_delete(DEFAULT_OWNER_ID, &id)
        })())
    }

    // ---- Editorial plans (Task 020a) ----
    //
    // Plans are documents, not feedback: none of these commands appends a feedback event.
    // Writes go through the plan state APIs only.

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanView {
        id: String,
        name: String,
        description: String,
        context_key: String,
        brief: String,
        created_at: String,
        updated_at: String,
        item_count: usize,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanItemView {
        media_kind: String,
        media_id: String,
        position: i64,
        start_s: Option<f64>,
        end_s: Option<f64>,
        pacing: Option<f64>,
        crop_x: Option<f64>,
        grade_json: Option<String>,
        reason: String,
        signals_json: String,
        origin: String,
        rank: Option<f64>,
        profile_version: Option<i64>,
        provenance_json: String,
        added_at: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanRevisionView {
        revision: i64,
        label: String,
        snapshot_json: String,
        created_at: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanItemArgs {
        asset_type: String,
        media_id: String,
        start_s: Option<f64>,
        end_s: Option<f64>,
        pacing: Option<f64>,
        crop_x: Option<f64>,
        grade_json: Option<String>,
        reason: Option<String>,
        signals_json: Option<String>,
        origin: Option<String>,
        rank: Option<f64>,
        profile_version: Option<i64>,
    }

    #[derive(Debug, Clone, Default, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanItemPatchArgs {
        start_s: Option<f64>,
        end_s: Option<f64>,
        pacing: Option<f64>,
        crop_x: Option<f64>,
        grade_json: Option<String>,
        reason: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PlanItemRefArgs {
        asset_type: String,
        media_id: String,
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PhotoRenderView {
        job_id: String,
        output_path: String,
        manifest_path: String,
        output_sha256: String,
        manifest_sha256: String,
        size_bytes: i64,
        media_type: String,
        width: Option<i64>,
        height: Option<i64>,
        duration_s: Option<f64>,
        completed_at: String,
    }

    fn render_output_view(output: crush_store::RenderOutput) -> PhotoRenderView {
        PhotoRenderView {
            job_id: output.job_id,
            output_path: output.output_path,
            manifest_path: output.manifest_path,
            output_sha256: output.output_sha256,
            manifest_sha256: output.manifest_sha256,
            size_bytes: output.size_bytes,
            media_type: output.media_type,
            width: output.width,
            height: output.height,
            duration_s: output.duration_s,
            completed_at: output.created_at.to_rfc3339(),
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct PhotoPresetSpec {
        id: &'static str,
        name: &'static str,
        preset: &'static str,
        extensions: &'static [&'static str],
    }

    #[derive(Debug, Clone, Copy)]
    struct ClipPresetSpec {
        name: &'static str,
        preset: &'static str,
        extensions: &'static [&'static str],
    }

    /// The saved photo recipe identity is an app-level fact; every other preset fact
    /// (name/label, extensions, preset id) comes from the enum.
    fn photo_recipe_id(preset: PhotoOutputPreset) -> &'static str {
        match preset {
            PhotoOutputPreset::JpegSrgbV1 => "crush-photo-jpeg-srgb",
            PhotoOutputPreset::PngSrgbV1 => "crush-photo-png-srgb",
            PhotoOutputPreset::TiffSrgbV1 => "crush-photo-tiff-srgb",
        }
    }

    fn clip_preset_spec(value: &str) -> anyhow::Result<ClipPresetSpec> {
        let preset = ClipOutputPreset::parse(value)
            .with_context(|| format!("unsupported video export preset {value:?}"))?;
        Ok(ClipPresetSpec {
            name: preset.label(),
            preset: preset.as_str(),
            extensions: preset.extensions(),
        })
    }

    fn clip_grade_from_plan(value: Option<&str>) -> anyhow::Result<serde_json::Value> {
        let Some(value) = value else {
            return Ok(serde_json::json!({ "mode": "none" }));
        };
        let parsed: serde_json::Value =
            serde_json::from_str(value).context("the saved color treatment is not valid JSON")?;
        let object = parsed
            .as_object()
            .context("the saved color treatment must be a JSON object")?;
        if object.is_empty() {
            return Ok(serde_json::json!({ "mode": "none" }));
        }
        match object.get("mode").and_then(serde_json::Value::as_str) {
            Some("none") if object.len() == 1 => Ok(parsed),
            Some("basic") => {
                let expected = [
                    "mode",
                    "exposure_ev",
                    "contrast",
                    "saturation",
                    "temperature",
                    "tint",
                ];
                ensure!(
                    object.len() == expected.len()
                        && expected.iter().all(|key| object.contains_key(*key)),
                    "the saved basic color treatment must include only exposure_ev, contrast, saturation, temperature, and tint"
                );
                Ok(parsed)
            }
            _ => anyhow::bail!(
                "this clip's saved color treatment cannot be rendered exactly yet; remove it or use a supported basic treatment"
            ),
        }
    }

    fn clip_recipe_schema(
        start_s: f64,
        end_s: f64,
        grade: serde_json::Value,
        audio: &str,
        preset: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "kind": "video_clip",
            // Plan and shot boundaries are absolute positions on the owning source video's
            // timeline. The renderer seeks that source directly, so no shot-start offset applies.
            "in_s": start_s,
            "out_s": end_s,
            "crop": null,
            "grade": grade,
            "transition": { "kind": "cut" },
            "audio": { "mode": audio },
            "output": { "preset": preset },
        })
    }

    fn photo_preset_spec(value: &str) -> anyhow::Result<PhotoPresetSpec> {
        let preset = PhotoOutputPreset::parse(value)
            .with_context(|| format!("unsupported photo export preset {value:?}"))?;
        Ok(PhotoPresetSpec {
            id: photo_recipe_id(preset),
            name: preset.label(),
            preset: preset.as_str(),
            extensions: preset.extensions(),
        })
    }

    fn ensure_photo_render_recipe(store: &Store, spec: PhotoPresetSpec) -> anyhow::Result<()> {
        let schema_json = serde_json::json!({
            "schema_version": 1,
            "kind": "photo",
            "crop": null,
            "rotation_degrees": 0,
            "grade": { "mode": "none" },
            "output": { "preset": spec.preset },
        })
        .to_string();
        if let Some(existing) = store.render_recipe_get(DEFAULT_OWNER_ID, spec.id, 1)? {
            ensure!(
                existing.kind == RenderRecipeKind::Photo
                    && existing.name == spec.name
                    && serde_json::from_str::<serde_json::Value>(&existing.schema_json)?
                        == serde_json::from_str::<serde_json::Value>(&schema_json)?,
                "saved photo export preset {} version 1 does not match this app version",
                spec.id
            );
            return Ok(());
        }
        store.render_recipe_create(
            DEFAULT_OWNER_ID,
            &RenderRecipe {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                id: spec.id.to_owned(),
                version: 1,
                kind: RenderRecipeKind::Photo,
                name: spec.name.to_owned(),
                schema_json,
                created_at: chrono::Utc::now(),
            },
        )
    }

    fn render_project_photo_job(
        config: &Config,
        paths: &AppPaths,
        project_id: &str,
        photo_id: &str,
        preset: &str,
        destination: &str,
    ) -> anyhow::Result<PhotoRenderView> {
        let spec = photo_preset_spec(preset)?;
        let destination_path = PathBuf::from(destination);
        ensure!(
            destination_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| spec
                    .extensions
                    .iter()
                    .any(|allowed| value.eq_ignore_ascii_case(allowed))),
            "the selected file type requires a .{} destination",
            spec.extensions[0]
        );

        let mut store = Store::open(&paths.root)?;
        let project = store
            .plan_get(DEFAULT_OWNER_ID, project_id)?
            .with_context(|| format!("project {project_id} was not found"))?;
        let selected = store
            .plan_items(DEFAULT_OWNER_ID, project_id)?
            .into_iter()
            .find(|item| item.media_kind == MediaKind::Photo && item.media_id == photo_id)
            .with_context(|| format!("photo {photo_id} is not selected in this project"))?;
        ensure!(
            selected.crop_x.is_none(),
            "this photo has a framing edit that the current photo renderer cannot reproduce yet"
        );
        if let Some(grade_json) = selected.grade_json.as_deref() {
            let grade: serde_json::Value = serde_json::from_str(grade_json)
                .context("the selected photo color edit is invalid JSON")?;
            let untreated = grade.as_object().is_some_and(|object| {
                object.is_empty()
                    || (object.len() == 1
                        && object.get("mode").and_then(serde_json::Value::as_str) == Some("none"))
            });
            ensure!(
                untreated,
                "this photo has a color edit that the current photo renderer cannot reproduce yet"
            );
        }
        let photo = store
            .photo_by_id(DEFAULT_OWNER_ID, photo_id)?
            .with_context(|| format!("photo {photo_id} was not found"))?;
        let annotation =
            store.editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, photo_id)?;
        ensure!(
            !annotation.is_some_and(|value| !value.usable || value.blur_required),
            "this photo is flagged unusable or blur-required and cannot be rendered"
        );

        ensure_photo_render_recipe(&store, spec)?;
        let origin = match selected.origin {
            PlanOrigin::General => "general",
            PlanOrigin::Personal => "personal",
            PlanOrigin::Historical => "historical",
            PlanOrigin::Imported => "imported",
        };
        let job_id = format!("render-job-{}", Uuid::new_v4());
        store.render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: job_id.clone(),
                recipe_id: spec.id.to_owned(),
                recipe_version: 1,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": project.context_key,
                    "selection_provenance": {
                        "project_id": project.id,
                        "position": selected.position,
                        "origin": origin,
                        "rank": selected.rank,
                        "profile_version": selected.profile_version,
                    },
                    "sources": [{
                        "media_kind": "photo",
                        "media_id": photo.id,
                        "source_id": photo.id,
                        "sha256": photo.sha256,
                        "path": photo.path,
                    }],
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used",
                    },
                })
                .to_string(),
                destination_path: destination_path.to_string_lossy().into_owned(),
                created_at: chrono::Utc::now(),
            },
        )?;
        drop(store);

        let output = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default())
            .execute_render_job(DEFAULT_OWNER_ID, &job_id)?;
        ensure!(
            Path::new(&output.output_path).is_file() && Path::new(&output.manifest_path).is_file(),
            "render completed without both verified files"
        );
        Ok(render_output_view(output))
    }

    /// Standalone photo derivative export from the Search/Review detail drawer, without a
    /// project. Keeps the same verified render contract as the project photo export and records
    /// `detail` provenance so the manifest never implies a project choice.
    fn render_photo_job(
        config: &Config,
        paths: &AppPaths,
        photo_id: &str,
        preset: &str,
        destination: &str,
    ) -> anyhow::Result<PhotoRenderView> {
        let spec = photo_preset_spec(preset)?;
        let destination_path = PathBuf::from(destination);
        ensure!(
            destination_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| spec
                    .extensions
                    .iter()
                    .any(|allowed| value.eq_ignore_ascii_case(allowed))),
            "the selected file type requires a .{} destination",
            spec.extensions[0]
        );

        let mut store = Store::open(&paths.root)?;
        let photo = store
            .photo_by_id(DEFAULT_OWNER_ID, photo_id)?
            .with_context(|| format!("photo {photo_id} was not found"))?;
        let annotation =
            store.editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Photo, photo_id)?;
        ensure!(
            !annotation.is_some_and(|value| !value.usable || value.blur_required),
            "this photo is flagged unusable or blur-required and cannot be rendered"
        );

        ensure_photo_render_recipe(&store, spec)?;
        let job_id = format!("render-job-{}", Uuid::new_v4());
        store.render_job_create(
            DEFAULT_OWNER_ID,
            &NewRenderJob {
                id: job_id.clone(),
                recipe_id: spec.id.to_owned(),
                recipe_version: 1,
                plan_id: None,
                plan_revision: None,
                source_snapshot_json: serde_json::json!({
                    "schema_version": 1,
                    "context_key": "default",
                    "selection_provenance": {
                        "origin": "detail_export",
                        "profile_version": null,
                    },
                    "sources": [{
                        "media_kind": "photo",
                        "media_id": photo.id,
                        "source_id": photo.id,
                        "sha256": photo.sha256,
                        "path": photo.path,
                    }],
                })
                .to_string(),
                model_versions_json: serde_json::json!({
                    "schema_version": 1,
                    "models": {
                        "clip": "not_used",
                        "aesthetic": "not_used",
                        "personal_style": "not_used",
                    },
                })
                .to_string(),
                destination_path: destination_path.to_string_lossy().into_owned(),
                created_at: chrono::Utc::now(),
            },
        )?;
        drop(store);

        let output = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default())
            .execute_render_job(DEFAULT_OWNER_ID, &job_id)?;
        ensure!(
            Path::new(&output.output_path).is_file() && Path::new(&output.manifest_path).is_file(),
            "render completed without both verified files"
        );
        Ok(render_output_view(output))
    }

    fn render_project_clip_job(
        config: &Config,
        paths: &AppPaths,
        project_id: &str,
        shot_id: &str,
        preset: &str,
        audio: &str,
        destination: &str,
    ) -> anyhow::Result<PhotoRenderView> {
        let spec = clip_preset_spec(preset)?;
        ensure!(
            matches!(audio, "source" | "mute"),
            "sound must be source or mute"
        );
        let destination_path = PathBuf::from(destination);
        ensure!(
            destination_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| spec
                    .extensions
                    .iter()
                    .any(|allowed| value.eq_ignore_ascii_case(allowed))),
            "the selected file type requires a .{} destination",
            spec.extensions[0]
        );

        let mut store = Store::open(&paths.root)?;
        let project = store
            .plan_get(DEFAULT_OWNER_ID, project_id)?
            .with_context(|| format!("project {project_id} was not found"))?;
        let selected = store
            .plan_items(DEFAULT_OWNER_ID, project_id)?
            .into_iter()
            .find(|item| item.media_kind == MediaKind::Shot && item.media_id == shot_id)
            .with_context(|| format!("clip {shot_id} is not selected in this project"))?;
        ensure!(
            selected.pacing.is_none(),
            "saved pacing is not supported by single-clip export yet; remove the pacing value before rendering"
        );
        ensure!(
            selected.crop_x.is_none(),
            "the saved horizontal crop cannot map exactly to this export; remove it before rendering"
        );
        let start_s = selected
            .start_s
            .context("the selected clip needs a saved In point before rendering")?;
        let end_s = selected
            .end_s
            .context("the selected clip needs a saved Out point before rendering")?;
        ensure!(
            end_s > start_s,
            "the selected clip's Out point must be after its In point"
        );
        let grade = clip_grade_from_plan(selected.grade_json.as_deref())?;
        let shot = store
            .shot_by_id(DEFAULT_OWNER_ID, shot_id)?
            .with_context(|| format!("clip {shot_id} was not found"))?;
        ensure!(
            start_s >= shot.start_s && end_s <= shot.end_s,
            "the selected clip's saved In and Out points must stay inside its source shot"
        );
        let video = store
            .video_by_id(DEFAULT_OWNER_ID, &shot.video_id)?
            .with_context(|| format!("source video {} was not found", shot.video_id))?;
        let annotation = store.editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Shot, shot_id)?;
        ensure!(
            !annotation.is_some_and(|value| !value.usable || value.blur_required),
            "this clip is flagged unusable or blur-required and cannot be rendered"
        );

        let recipe_id = format!("crush-video-clip-{}", Uuid::new_v4());
        let recipe = RenderRecipe {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            id: recipe_id.clone(),
            version: 1,
            kind: RenderRecipeKind::VideoClip,
            name: spec.name.to_owned(),
            schema_json: clip_recipe_schema(start_s, end_s, grade, audio, spec.preset).to_string(),
            created_at: chrono::Utc::now(),
        };
        let origin = match selected.origin {
            PlanOrigin::General => "general",
            PlanOrigin::Personal => "personal",
            PlanOrigin::Historical => "historical",
            PlanOrigin::Imported => "imported",
        };
        let job_id = format!("render-job-{}", Uuid::new_v4());
        let job = NewRenderJob {
            id: job_id.clone(),
            recipe_id,
            recipe_version: 1,
            plan_id: None,
            plan_revision: None,
            source_snapshot_json: serde_json::json!({
                "schema_version": 1,
                "context_key": project.context_key,
                "selection_provenance": {
                    "project_id": project.id,
                    "position": selected.position,
                    "origin": origin,
                    "rank": selected.rank,
                    "profile_version": selected.profile_version,
                },
                "sources": [{
                    "media_kind": "shot",
                    "media_id": shot.id,
                    "source_id": video.id,
                    "sha256": video.sha256,
                    "path": video.path,
                }],
            })
            .to_string(),
            model_versions_json: serde_json::json!({
                "schema_version": 1,
                "models": {
                    "clip": "not_used",
                    "aesthetic": "not_used",
                    "personal_style": "not_used",
                },
            })
            .to_string(),
            destination_path: destination_path.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now(),
        };
        store.render_recipe_and_job_create(DEFAULT_OWNER_ID, &recipe, &job)?;
        drop(store);

        let output = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default())
            .execute_render_job(DEFAULT_OWNER_ID, &job_id)?;
        ensure!(
            Path::new(&output.output_path).is_file() && Path::new(&output.manifest_path).is_file(),
            "render completed without both verified files"
        );
        Ok(render_output_view(output))
    }

    fn render_project_reel_job(
        config: &Config,
        paths: &AppPaths,
        project_id: &str,
        preset: &str,
        audio: &str,
        destination: &str,
        cancellation: CancellationToken,
    ) -> anyhow::Result<PhotoRenderView> {
        let spec = clip_preset_spec(preset)?;
        ensure!(
            matches!(audio, "source" | "mute"),
            "sound must be source or mute"
        );
        let destination_path = PathBuf::from(destination);
        ensure!(
            destination_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| spec
                    .extensions
                    .iter()
                    .any(|allowed| value.eq_ignore_ascii_case(allowed))),
            "the selected file type requires a .{} destination",
            spec.extensions[0]
        );

        let mut store = Store::open(&paths.root)?;
        let project = store
            .plan_get(DEFAULT_OWNER_ID, project_id)?
            .with_context(|| format!("project {project_id} was not found"))?;
        let items = store.plan_items(DEFAULT_OWNER_ID, project_id)?;
        ensure!(
            !items.is_empty(),
            "add at least one clip before rendering a reel"
        );
        let mut sources = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            ensure!(
                item.media_kind == MediaKind::Shot,
                "item {} is a photo; whole-reel photo holds need a saved duration and framing contract, so export it individually for now",
                index + 1
            );
            ensure!(
                item.pacing.is_none(),
                "item {} has pacing that the current reel renderer cannot reproduce",
                index + 1
            );
            ensure!(
                item.crop_x.is_none(),
                "item {} has framing that the current reel renderer cannot reproduce",
                index + 1
            );
            clip_grade_from_plan(item.grade_json.as_deref()).with_context(|| {
                format!(
                    "item {} has a color treatment the current reel renderer cannot reproduce",
                    index + 1
                )
            })?;
            let start_s = item
                .start_s
                .with_context(|| format!("item {} needs a saved In point", index + 1))?;
            let end_s = item
                .end_s
                .with_context(|| format!("item {} needs a saved Out point", index + 1))?;
            ensure!(
                end_s > start_s,
                "item {} needs an Out point after its In point",
                index + 1
            );
            let shot = store
                .shot_by_id(DEFAULT_OWNER_ID, &item.media_id)?
                .with_context(|| format!("clip {} was not found", item.media_id))?;
            ensure!(
                start_s >= shot.start_s && end_s <= shot.end_s,
                "item {} boundaries must stay inside its source shot",
                index + 1
            );
            let annotation =
                store.editorial_annotation(DEFAULT_OWNER_ID, MediaKind::Shot, &shot.id)?;
            ensure!(
                !annotation.is_some_and(|value| !value.usable || value.blur_required),
                "item {} is flagged unusable or blur-required and cannot be rendered",
                index + 1
            );
            let video = store
                .video_by_id(DEFAULT_OWNER_ID, &shot.video_id)?
                .with_context(|| format!("source video {} was not found", shot.video_id))?;
            sources.push(serde_json::json!({
                "media_kind": "shot",
                "media_id": shot.id,
                "source_id": video.id,
                "sha256": video.sha256,
                "path": video.path,
            }));
        }

        let revision = store.plan_save_revision(
            DEFAULT_OWNER_ID,
            project_id,
            &format!(
                "Export · {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
            ),
        )?;
        let recipe_id = format!("crush-video-reel-{}", Uuid::new_v4());
        let recipe = RenderRecipe {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            id: recipe_id.clone(),
            version: 1,
            kind: RenderRecipeKind::Reel,
            name: spec.name.to_owned(),
            schema_json: serde_json::json!({
                "schema_version": 1,
                "kind": "reel",
                "transition": {"kind": "cut"},
                "audio": {"mode": audio},
                "output": {"preset": spec.preset},
            })
            .to_string(),
            created_at: chrono::Utc::now(),
        };
        let job_id = format!("render-job-{}", Uuid::new_v4());
        let job = NewRenderJob {
            id: job_id.clone(),
            recipe_id,
            recipe_version: 1,
            plan_id: Some(project.id.clone()),
            plan_revision: Some(revision.revision),
            source_snapshot_json: serde_json::json!({
                "schema_version": 1,
                "context_key": project.context_key,
                "selection_provenance": {
                    "project_id": project.id,
                    "revision": revision.revision,
                    "origin": "project_sequence",
                },
                "sources": sources,
            })
            .to_string(),
            model_versions_json: serde_json::json!({
                "schema_version": 1,
                "models": {
                    "clip": "not_used",
                    "aesthetic": "not_used",
                    "personal_style": "not_used",
                },
            })
            .to_string(),
            destination_path: destination_path.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now(),
        };
        store.render_recipe_and_job_create(DEFAULT_OWNER_ID, &recipe, &job)?;
        drop(store);

        let output = Pipeline::new(config.clone(), paths.clone(), cancellation)
            .execute_render_job(DEFAULT_OWNER_ID, &job_id)?;
        ensure!(
            Path::new(&output.output_path).is_file() && Path::new(&output.manifest_path).is_file(),
            "render completed without both verified files"
        );
        Ok(render_output_view(output))
    }

    fn plan_view(store: &Store, plan: Plan) -> anyhow::Result<PlanView> {
        let item_count = store.plan_items(DEFAULT_OWNER_ID, &plan.id)?.len();
        Ok(PlanView {
            id: plan.id,
            name: plan.name,
            description: plan.description,
            context_key: plan.context_key,
            brief: plan.brief,
            created_at: plan.created_at.to_rfc3339(),
            updated_at: plan.updated_at.to_rfc3339(),
            item_count,
        })
    }

    fn plan_item_view(item: PlanItem) -> PlanItemView {
        PlanItemView {
            media_kind: match item.media_kind {
                MediaKind::Photo => "photo".to_owned(),
                MediaKind::Shot => "shot".to_owned(),
                MediaKind::Span => "span".to_owned(),
            },
            media_id: item.media_id,
            position: item.position,
            start_s: item.start_s,
            end_s: item.end_s,
            pacing: item.pacing,
            crop_x: item.crop_x,
            grade_json: item.grade_json,
            reason: item.reason,
            signals_json: item.signals_json,
            origin: match item.origin {
                PlanOrigin::General => "general".to_owned(),
                PlanOrigin::Personal => "personal".to_owned(),
                PlanOrigin::Historical => "historical".to_owned(),
                PlanOrigin::Imported => "imported".to_owned(),
            },
            rank: item.rank,
            profile_version: item.profile_version,
            provenance_json: item.provenance_json,
            added_at: item.added_at.to_rfc3339(),
        }
    }

    fn parse_plan_origin(origin: Option<&str>) -> anyhow::Result<PlanOrigin> {
        match origin.unwrap_or("general") {
            "general" => Ok(PlanOrigin::General),
            "personal" => Ok(PlanOrigin::Personal),
            other => anyhow::bail!("unsupported plan item origin {other:?}"),
        }
    }

    #[tauri::command]
    fn plan_create(
        name: String,
        description: Option<String>,
        brief: Option<String>,
        context_key: Option<String>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PlanView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let now = chrono::Utc::now();
            let plan = Plan {
                id: Uuid::new_v4().to_string(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                name,
                description: description.unwrap_or_default(),
                context_key: context_key.unwrap_or_else(|| "default".to_owned()),
                brief: brief.unwrap_or_default(),
                created_at: now,
                updated_at: now,
            };
            store.plan_create(DEFAULT_OWNER_ID, &plan)?;
            plan_view(&store, plan)
        })())
    }

    #[tauri::command]
    fn plan_list(state: State<'_, RuntimeState>) -> CommandResult<Vec<PlanView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store
                .plan_list(DEFAULT_OWNER_ID)?
                .into_iter()
                .map(|plan| plan_view(&store, plan))
                .collect()
        })())
    }

    #[tauri::command]
    fn plan_get(id: String, state: State<'_, RuntimeState>) -> CommandResult<PlanView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let plan = store
                .plan_get(DEFAULT_OWNER_ID, &id)?
                .with_context(|| format!("plan {id} was not found"))?;
            plan_view(&store, plan)
        })())
    }

    #[tauri::command]
    fn plan_update(
        id: String,
        name: String,
        description: String,
        brief: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.plan_update(DEFAULT_OWNER_ID, &id, &name, &description, &brief)
        })())
    }

    #[tauri::command]
    fn plan_delete(id: String, state: State<'_, RuntimeState>) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.plan_delete(DEFAULT_OWNER_ID, &id)
        })())
    }

    #[tauri::command]
    fn plan_items(id: String, state: State<'_, RuntimeState>) -> CommandResult<Vec<PlanItemView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .plan_items(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .map(plan_item_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn plan_add_item(
        id: String,
        item: PlanItemArgs,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PlanItemView> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let plan_item = PlanItem {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                plan_id: id,
                media_kind: parse_library_kind(&item.asset_type)?,
                media_id: item.media_id,
                // plan_add_item assigns the next dense position.
                position: 0,
                start_s: item.start_s,
                end_s: item.end_s,
                pacing: item.pacing,
                crop_x: item.crop_x,
                grade_json: item.grade_json,
                reason: item.reason.unwrap_or_default(),
                // The UI freezes the score breakdown it showed at selection time.
                signals_json: item.signals_json.unwrap_or_else(|| "{}".to_owned()),
                origin: parse_plan_origin(item.origin.as_deref())?,
                rank: item.rank,
                profile_version: item.profile_version,
                provenance_json: "{}".to_owned(),
                added_at: chrono::Utc::now(),
            };
            let plan_id = plan_item.plan_id.clone();
            let media_id = plan_item.media_id.clone();
            let media_kind = plan_item.media_kind;
            let mut store = store;
            store.plan_add_item(DEFAULT_OWNER_ID, &plan_item)?;
            let stored = store
                .plan_items(DEFAULT_OWNER_ID, &plan_id)?
                .into_iter()
                .find(|stored| stored.media_id == media_id && stored.media_kind == media_kind)
                .context("stored plan item was not found after insert")?;
            Ok(plan_item_view(stored))
        })())
    }

    #[tauri::command]
    fn plan_update_item(
        id: String,
        asset_type: String,
        media_id: String,
        patch: PlanItemPatchArgs,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PlanItemView> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            let media_kind = parse_library_kind(&asset_type)?;
            store.plan_update_item(
                DEFAULT_OWNER_ID,
                &id,
                media_kind,
                &media_id,
                &PlanItemPatch {
                    start_s: patch.start_s,
                    end_s: patch.end_s,
                    pacing: patch.pacing,
                    crop_x: patch.crop_x,
                    grade_json: patch.grade_json,
                    reason: patch.reason,
                },
            )?;
            let stored = store
                .plan_items(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .find(|stored| stored.media_id == media_id && stored.media_kind == media_kind)
                .context("stored plan item was not found after update")?;
            Ok(plan_item_view(stored))
        })())
    }

    #[tauri::command]
    fn plan_remove_item(
        id: String,
        asset_type: String,
        media_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.plan_remove_item(
                DEFAULT_OWNER_ID,
                &id,
                parse_library_kind(&asset_type)?,
                &media_id,
            )
        })())
    }

    #[tauri::command]
    fn plan_reorder_items(
        id: String,
        items: Vec<PlanItemRefArgs>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<PlanItemView>> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            let ordered = items
                .iter()
                .map(|reference| {
                    Ok((
                        parse_library_kind(&reference.asset_type)?,
                        reference.media_id.clone(),
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            store.plan_reorder_items(DEFAULT_OWNER_ID, &id, &ordered)?;
            Ok(store
                .plan_items(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .map(plan_item_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn plan_save_revision(
        id: String,
        label: Option<String>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PlanRevisionView> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            let revision =
                store.plan_save_revision(DEFAULT_OWNER_ID, &id, &label.unwrap_or_default())?;
            Ok(PlanRevisionView {
                revision: revision.revision,
                label: revision.label,
                snapshot_json: revision.snapshot_json,
                created_at: revision.created_at.to_rfc3339(),
            })
        })())
    }

    /// Plain-language sequence observations over the ordered plan (Task 033): per-item and
    /// per-transition repetition/similarity notes, pacing, and source coverage. Read-only.
    #[tauri::command]
    fn plan_sequence_signals(
        id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<crush_search::sequence::SequenceReport> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let items = store.plan_items(DEFAULT_OWNER_ID, &id)?;
            crush_search::sequence::sequence_report(&store, &items)
        })())
    }

    /// One-click reorder suggestions for near-duplicate adjacencies. Read-only: applying one
    /// is the UI calling the existing `plan_reorder_items` with `suggested_order`, and undo
    /// is the existing revision restore.
    #[tauri::command]
    fn plan_sequence_suggestions(
        id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<crush_search::sequence::SequenceSuggestion>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            let items = store.plan_items(DEFAULT_OWNER_ID, &id)?;
            crush_search::sequence::sequence_suggestions(&store, &items)
        })())
    }

    #[tauri::command]
    fn plan_revisions(
        id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<PlanRevisionView>> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            Ok(store
                .plan_revisions(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .map(|revision| PlanRevisionView {
                    revision: revision.revision,
                    label: revision.label,
                    snapshot_json: revision.snapshot_json,
                    created_at: revision.created_at.to_rfc3339(),
                })
                .collect())
        })())
    }

    #[tauri::command]
    fn plan_restore_revision(
        id: String,
        revision: i64,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<Vec<PlanItemView>> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            store.plan_restore_revision(DEFAULT_OWNER_ID, &id, revision)?;
            Ok(store
                .plan_items(DEFAULT_OWNER_ID, &id)?
                .into_iter()
                .map(plan_item_view)
                .collect())
        })())
    }

    #[tauri::command]
    fn plan_duplicate(
        id: String,
        name: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PlanView> {
        command_result((|| {
            let mut store = Store::open(&state.paths.root)?;
            let copy = store.plan_duplicate(DEFAULT_OWNER_ID, &id, &name)?;
            plan_view(&store, copy)
        })())
    }

    /// Both selects orderings in one response: the general cold-start strong-shot list and,
    /// when a brief is supplied, the separately explainable personalized ordering.
    #[tauri::command]
    async fn selects_candidates(
        brief: Option<String>,
        context: Option<String>,
        top: usize,
        duplicate_cap: Option<usize>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<SelectsCandidates> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        let cache = Arc::clone(&state.search);
        let retrain_dirty = Arc::clone(&state.retrain_dirty);
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
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
                        embedder: Embedder::new(
                            paths.models(),
                            ProviderPreference::Cpu,
                            config.limits.threads,
                        )?,
                        generation: None,
                    });
                }
                let runtime = runtime.as_mut().context("search runtime was not created")?;
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
                rank_selects_candidates(
                    &store,
                    DEFAULT_OWNER_ID,
                    engine,
                    &mut |text: &str| embedder.embed_text(text),
                    brief.as_deref(),
                    top,
                    context.as_deref(),
                    duplicate_cap,
                )
            })())
        })
        .await
        .map_err(|error| format!("selects worker failed: {error}"))?
    }

    /// Read-only projection of the current editorial annotation for the review drawer.
    /// Defaults mirror the 0002 columns and `browse_assets`' COALESCE defaults, so assets
    /// without an annotation row present `usable = true`, every privacy flag cleared.
    #[tauri::command]
    fn editorial_annotation_get(
        asset_type: String,
        id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<AnnotationView> {
        command_result((|| {
            let media_kind = parse_library_kind(&asset_type)?;
            let store = Store::open(&state.paths.root)?;
            let annotation = store.editorial_annotation(DEFAULT_OWNER_ID, media_kind, &id)?;
            Ok(AnnotationView {
                description: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.description.clone()),
                subjects: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.subjects.clone()),
                action: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.action.clone()),
                tags: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.tags.clone()),
                notes: annotation
                    .as_ref()
                    .map_or_else(String::new, |value| value.notes.clone()),
                standout: annotation.as_ref().is_some_and(|value| value.standout),
                usable: annotation.as_ref().is_none_or(|value| value.usable),
                faces_visible: annotation.as_ref().is_some_and(|value| value.faces_visible),
                nametags_visible: annotation
                    .as_ref()
                    .is_some_and(|value| value.nametags_visible),
                blur_required: annotation.as_ref().is_some_and(|value| value.blur_required),
            })
        })())
    }

    /// The only UI path to the safety columns. Clearing a flag is an explicit user action; the
    /// confirm dialog lives in the review UI (019b), and no machine path shares this writer.
    #[tauri::command]
    fn set_safety_flags(
        asset_type: String,
        id: String,
        faces_visible: bool,
        nametags_visible: bool,
        blur_required: bool,
        usable: bool,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let store = Store::open(&state.paths.root)?;
            store.set_safety_flags(
                DEFAULT_OWNER_ID,
                parse_library_kind(&asset_type)?,
                &id,
                SafetyFlags {
                    usable,
                    faces_visible,
                    nametags_visible,
                    blur_required,
                },
            )?;
            Ok(())
        })())
    }

    /// Editable current-state metadata; each edited category also appends its append-only
    /// feedback signal (tags, edits, crops, grades, ratings).
    #[tauri::command]
    fn set_annotation(
        asset_type: String,
        id: String,
        fields: Option<AnnotationPatchArgs>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<()> {
        command_result((|| {
            let patch = fields.unwrap_or_default();
            let media_kind = parse_library_kind(&asset_type)?;
            let store = Store::open(&state.paths.root)?;
            let now = chrono::Utc::now();
            let mut annotation =
                match store.editorial_annotation(DEFAULT_OWNER_ID, media_kind, &id)? {
                    Some(value) => value,
                    None => {
                        // Defaults mirror the 0002 columns; the upsert's target-existence
                        // triggers refuse unknown media.
                        EditorialAnnotation {
                            owner_id: DEFAULT_OWNER_ID.to_owned(),
                            media_kind,
                            media_id: id.clone(),
                            description: String::new(),
                            subjects: String::new(),
                            action: String::new(),
                            tags: String::new(),
                            quality: None,
                            standout: false,
                            usable: true,
                            faces_visible: false,
                            nametags_visible: false,
                            blur_required: false,
                            crop_x: None,
                            grade_json: None,
                            notes: String::new(),
                            updated_at: now,
                        }
                    }
                };
            // Capture the edit flags before the patch fields are moved into the annotation.
            let tags_edited = patch.tags.is_some();
            let copy_edited = patch.description.is_some()
                || patch.subjects.is_some()
                || patch.action.is_some()
                || patch.notes.is_some();
            let crop_edited = patch.crop_x.is_some();
            let grade_edited = patch.grade_json.is_some();
            if let Some(value) = patch.description {
                annotation.description = value;
            }
            if let Some(value) = patch.subjects {
                annotation.subjects = value;
            }
            if let Some(value) = patch.action {
                annotation.action = value;
            }
            if let Some(value) = patch.tags {
                annotation.tags = value;
            }
            if let Some(value) = patch.notes {
                annotation.notes = value;
            }
            if let Some(value) = patch.quality {
                annotation.quality = Some(value);
            }
            if let Some(value) = patch.crop_x {
                annotation.crop_x = Some(value);
            }
            if let Some(value) = patch.grade_json {
                annotation.grade_json = Some(value);
            }
            if let Some(value) = patch.standout {
                annotation.standout = value;
            }
            annotation.updated_at = now;
            store.upsert_editorial_annotation(DEFAULT_OWNER_ID, &annotation)?;

            let mut signals: Vec<(FeedbackSignal, Option<f64>)> = Vec::new();
            if tags_edited {
                signals.push((FeedbackSignal::Tag, None));
            }
            if copy_edited {
                signals.push((FeedbackSignal::Edit, None));
            }
            if crop_edited {
                signals.push((FeedbackSignal::Crop, None));
            }
            if grade_edited {
                signals.push((FeedbackSignal::Grade, None));
            }
            if let Some(rating) = patch.quality {
                signals.push((FeedbackSignal::Rating, Some(rating as f64)));
            }
            for (signal, value) in signals {
                store.append_feedback(
                    DEFAULT_OWNER_ID,
                    &FeedbackEvent {
                        id: Uuid::new_v4().to_string(),
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        media_kind,
                        media_id: id.clone(),
                        signal,
                        value,
                        compared_media_kind: None,
                        compared_media_id: None,
                        context_json: "{}".to_owned(),
                        created_at: now,
                    },
                )?;
            }
            Ok(())
        })())
    }

    /// Bulk pick/reject/rate/flag/add-to-collection. One bad op aborts the whole batch.
    #[tauri::command]
    fn review_batch(
        ops: Vec<ReviewOpArgs>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<usize> {
        command_result((|| {
            let mut review_ops = Vec::with_capacity(ops.len());
            for op in &ops {
                let media_kind = match &op.asset_type {
                    Some(value) => parse_library_kind(value)?,
                    None => anyhow::bail!("review op {:?} requires assetType", op.op),
                };
                let media_id = op
                    .media_id
                    .clone()
                    .with_context(|| format!("review op {:?} requires mediaId", op.op))?;
                review_ops.push(match op.op.as_str() {
                    "pick" => ReviewOp::Pick {
                        media_kind,
                        media_id,
                    },
                    "reject" => ReviewOp::Reject {
                        media_kind,
                        media_id,
                    },
                    "rate" => ReviewOp::Rate {
                        media_kind,
                        media_id,
                        rating: op.rating.context("rate op requires rating")?,
                    },
                    "flag" => ReviewOp::SetFlags {
                        media_kind,
                        media_id,
                        flags: SafetyFlags {
                            usable: op.usable.context("flag op requires usable")?,
                            faces_visible: op
                                .faces_visible
                                .context("flag op requires facesVisible")?,
                            nametags_visible: op
                                .nametags_visible
                                .context("flag op requires nametagsVisible")?,
                            blur_required: op
                                .blur_required
                                .context("flag op requires blurRequired")?,
                        },
                    },
                    "add_to_collection" => ReviewOp::AddToCollection {
                        collection_id: op
                            .collection_id
                            .clone()
                            .context("add_to_collection op requires collectionId")?,
                        media_kind,
                        media_id,
                        context_key: op.context_key.clone(),
                    },
                    other => anyhow::bail!("unsupported review op {other:?}"),
                });
            }
            let mut store = Store::open(&state.paths.root)?;
            store.bulk_review(DEFAULT_OWNER_ID, &review_ops)
        })())
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
    async fn render_project_photo(
        project_id: String,
        photo_id: String,
        preset: String,
        destination: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PhotoRenderView> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result(render_project_photo_job(
                &config,
                &paths,
                &project_id,
                &photo_id,
                &preset,
                &destination,
            ))
        })
        .await
        .map_err(|error| format!("photo render worker failed: {error}"))?
    }

    #[tauri::command]
    async fn render_photo(
        photo_id: String,
        preset: String,
        destination: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PhotoRenderView> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result(render_photo_job(
                &config,
                &paths,
                &photo_id,
                &preset,
                &destination,
            ))
        })
        .await
        .map_err(|error| format!("photo render worker failed: {error}"))?
    }

    #[tauri::command]
    async fn render_project_clip(
        project_id: String,
        shot_id: String,
        preset: String,
        audio: String,
        destination: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PhotoRenderView> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result(render_project_clip_job(
                &config,
                &paths,
                &project_id,
                &shot_id,
                &preset,
                &audio,
                &destination,
            ))
        })
        .await
        .map_err(|error| format!("clip render worker failed: {error}"))?
    }

    #[tauri::command]
    async fn render_project_reel(
        project_id: String,
        preset: String,
        audio: String,
        destination: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<PhotoRenderView> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        let cancellation = CancellationToken::default();
        {
            let mut active = lock(&state.active_renders)?;
            if active.contains_key(&project_id) {
                return Err("this project is already rendering".to_owned());
            }
            active.insert(project_id.clone(), cancellation.clone());
        }
        let active_renders = Arc::clone(&state.active_renders);
        let active_key = project_id.clone();
        let worker = tauri::async_runtime::spawn_blocking(move || {
            command_result(render_project_reel_job(
                &config,
                &paths,
                &project_id,
                &preset,
                &audio,
                &destination,
                cancellation,
            ))
        });
        let result = worker
            .await
            .map_err(|error| format!("reel render worker failed: {error}"));
        if let Ok(mut active) = active_renders.lock() {
            active.remove(&active_key);
        }
        result?
    }

    #[tauri::command]
    fn cancel_project_render(
        project_id: String,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<bool> {
        let active = lock(&state.active_renders)?;
        let Some(cancellation) = active.get(&project_id) else {
            return Ok(false);
        };
        cancellation.cancel();
        Ok(true)
    }

    #[tauri::command]
    async fn export_clip(
        id: String,
        out: String,
        allow_unsafe_export: Option<bool>,
        state: State<'_, RuntimeState>,
    ) -> CommandResult<ExportedClip> {
        let config = state.config.clone();
        let paths = state.paths.clone();
        tauri::async_runtime::spawn_blocking(move || {
            command_result((|| {
                let output = PathBuf::from(out);
                // Earliest privacy enforcement point (TASK-021 adds the full render/export
                // gate): refuse to export a shot the owner flagged unusable or blur-required
                // unless the request explicitly acknowledges it. Machine scores can never
                // clear these flags, so this refusal can only be lifted by the user.
                {
                    let store = Store::open(&paths.root)?;
                    let annotation = store.editorial_annotation(
                        DEFAULT_OWNER_ID,
                        crush_store::MediaKind::Shot,
                        &id,
                    )?;
                    let flagged =
                        annotation.is_some_and(|value| !value.usable || value.blur_required);
                    ensure!(
                        !flagged || allow_unsafe_export == Some(true),
                        "shot {id} is flagged unusable or blur-required; export was refused \
                         because allow_unsafe_export was not set"
                    );
                }
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
            "discovered={} photos={} indexed={} indexed_photos={} skipped={} failed={} moved={} renamed={} duplicated={} recovered={} vectors={}",
            summary.discovered,
            summary.discovered_photos,
            summary.indexed,
            summary.indexed_photos,
            summary.skipped,
            summary.failed,
            summary.moved,
            summary.renamed,
            summary.duplicated,
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

    /// Ingest tasks carry the moved/renamed/duplicate counts as structured fields
    /// alongside the summary text, so the Library can report relinked files without
    /// string parsing. The status/detail/error transitions are exactly
    /// [`complete_background`]'s: the summary is flattened to its text first, the count
    /// fields are written onto the stored task while it is still running, and
    /// [`complete_background`] then flips the task to its terminal state and returns a
    /// clone that already carries the counts.
    fn complete_ingest_background(
        tasks: &Arc<Mutex<BTreeMap<String, BackgroundTask>>>,
        job_id: &str,
        result: anyhow::Result<IngestSummary>,
        cancelled: bool,
    ) -> CommandResult<BackgroundTask> {
        let (detail, counts) = match result {
            Ok(summary) => (
                Ok(ingest_summary(&summary)),
                (
                    Some(summary.moved),
                    Some(summary.renamed),
                    Some(summary.duplicated),
                ),
            ),
            Err(error) => (Err(error), (None, None, None)),
        };
        {
            let mut stored = lock(tasks)?;
            let stored = stored
                .get_mut(job_id)
                .ok_or_else(|| format!("background task {job_id} disappeared"))?;
            let (moved, renamed, duplicated) = counts;
            stored.moved = moved;
            stored.renamed = renamed;
            stored.duplicated = duplicated;
        }
        complete_background(tasks, job_id, detail, cancelled)
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

    fn recover_interrupted_renders(
        config: &Config,
        paths: &AppPaths,
    ) -> anyhow::Result<RenderRecoverySummary> {
        Pipeline::new(config.clone(), paths.clone(), CancellationToken::default())
            .recover_interrupted_render_jobs(DEFAULT_OWNER_ID)
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
                // Recovery touches user-chosen export volumes; a read-only or unmounted volume
                // must not brick every subsequent launch, so failures are logged, not fatal.
                // It also full-hashes published outputs, so it leaves this setup thread: launch
                // proceeds while the blocking work runs, and the result is logged when done.
                let recovery_config = config.clone();
                let recovery_paths = paths.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    match recover_interrupted_renders(&recovery_config, &recovery_paths) {
                        Ok(render_recovery) => eprintln!(
                            "startup render recovery: finalized={} failed={} staging_removed={}",
                            render_recovery.finalized,
                            render_recovery.failed,
                            render_recovery.staging_removed
                        ),
                        Err(error) => eprintln!(
                            "startup render recovery could not complete; interrupted renders stay recoverable: {error:#}"
                        ),
                    }
                });
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
                    active_renders: Arc::new(Mutex::new(BTreeMap::new())),
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
                reindex_asset,
                relink_asset,
                remove_asset,
                import_reel_studio,
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
                library_browse,
                library_counts,
                collection_create,
                collection_list,
                collection_rename,
                collection_delete,
                collection_add_items,
                collection_remove_item,
                collection_items,
                collection_set_item_marked,
                collection_designate_reference_set,
                stack_create,
                stack_add_item,
                stack_remove_item,
                stacks_for_asset,
                stack_list,
                saved_search_create,
                saved_search_list,
                saved_search_delete,
                editorial_annotation_get,
                set_safety_flags,
                set_annotation,
                review_batch,
                plan_create,
                plan_list,
                plan_get,
                plan_update,
                plan_delete,
                plan_items,
                plan_add_item,
                plan_update_item,
                plan_remove_item,
                plan_reorder_items,
                plan_save_revision,
                plan_sequence_signals,
                plan_sequence_suggestions,
                plan_revisions,
                plan_restore_revision,
                plan_duplicate,
                selects_candidates,
                list_render_presets,
                render_project_photo,
                render_photo,
                render_project_clip,
                render_project_reel,
                cancel_project_render,
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
        fn plan_feedback_context_does_not_become_universal_taste() {
            let scoped: serde_json::Value = serde_json::from_str(
                &feedback_context_json(Some("warm portraits"), Some(" campaign ")).unwrap(),
            )
            .unwrap();
            assert_eq!(scoped["context"], "campaign");
            assert_eq!(scoped["query"], "warm portraits");
            let legacy: serde_json::Value =
                serde_json::from_str(&feedback_context_json(Some("legacy search"), None).unwrap())
                    .unwrap();
            assert_eq!(legacy["context"], "default");
            assert!(feedback_context_json(None, Some("  ")).is_err());
        }

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
            assert!(report.contains("schema=11"));
        }

        #[test]
        fn startup_render_recovery_accepts_an_empty_library() {
            let temporary = tempfile::tempdir().unwrap();
            let paths = AppPaths {
                root: temporary.path().join("data"),
            };
            std::fs::create_dir_all(&paths.root).unwrap();

            let summary = recover_interrupted_renders(&Config::default(), &paths).unwrap();

            assert_eq!(summary, RenderRecoverySummary::default());
        }

        #[test]
        fn photo_export_presets_are_versioned_and_reused() {
            let temporary = tempfile::tempdir().unwrap();
            let store = Store::open(temporary.path()).unwrap();

            for preset in ["jpeg-srgb-v1", "png-srgb-v1", "tiff-srgb-v1"] {
                let spec = photo_preset_spec(preset).unwrap();
                ensure_photo_render_recipe(&store, spec).unwrap();
                ensure_photo_render_recipe(&store, spec).unwrap();
            }

            let recipes = store
                .render_recipes(DEFAULT_OWNER_ID, Some(RenderRecipeKind::Photo))
                .unwrap();
            assert_eq!(recipes.len(), 3);
            assert!(recipes.iter().all(|recipe| recipe.version == 1));
            assert!(photo_preset_spec("webp").is_err());
        }

        /// TASK-035 item 5: the command serves every preset fact from the enums, so the UI
        /// has no second table to drift. This pins the canonical values the reviewer approved
        /// (labels, extensions, media types, ids) in one place.
        #[test]
        fn list_render_presets_matches_the_preset_enums() {
            let catalog = render_preset_catalog();

            assert_eq!(
                catalog
                    .photo
                    .iter()
                    .map(|preset| (
                        preset.id,
                        preset.label,
                        preset.extension,
                        preset.extensions,
                        preset.media_type,
                        preset.muxer,
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        "jpeg-srgb-v1",
                        "JPEG — smaller, easy to share",
                        "jpg",
                        &["jpg", "jpeg"][..],
                        "image/jpeg",
                        None,
                    ),
                    (
                        "png-srgb-v1",
                        "PNG — lossless",
                        "png",
                        &["png"][..],
                        "image/png",
                        None,
                    ),
                    (
                        "tiff-srgb-v1",
                        "TIFF — lossless 8-bit copy",
                        "tif",
                        &["tif", "tiff"][..],
                        "image/tiff",
                        None,
                    ),
                ]
            );
            assert_eq!(
                catalog
                    .clip
                    .iter()
                    .map(|preset| (
                        preset.id,
                        preset.label,
                        preset.extension,
                        preset.extensions,
                        preset.media_type,
                        preset.muxer,
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        "mp4-h264-sdr-v1",
                        "MP4 — compatible H.264",
                        "mp4",
                        &["mp4"][..],
                        "video/mp4",
                        Some("mp4"),
                    ),
                    (
                        "mov-h264-sdr-v1",
                        "MOV — editing-friendly H.264",
                        "mov",
                        &["mov"][..],
                        "video/quicktime",
                        Some("mov"),
                    ),
                ]
            );

            // Recipe identities stay app-level facts keyed by the enum presets.
            for (preset_id, recipe_id) in [
                ("jpeg-srgb-v1", "crush-photo-jpeg-srgb"),
                ("png-srgb-v1", "crush-photo-png-srgb"),
                ("tiff-srgb-v1", "crush-photo-tiff-srgb"),
            ] {
                assert_eq!(photo_preset_spec(preset_id).unwrap().id, recipe_id);
            }
            assert!(clip_preset_spec("libx264").is_err());
        }

        #[test]
        fn clip_export_maps_only_exact_supported_treatments() {
            assert_eq!(
                clip_grade_from_plan(None).unwrap(),
                serde_json::json!({ "mode": "none" })
            );
            assert_eq!(
                clip_grade_from_plan(Some("{}")).unwrap(),
                serde_json::json!({ "mode": "none" })
            );
            let basic = r#"{"mode":"basic","exposure_ev":0.1,"contrast":0.2,"saturation":1.1,"temperature":0.0,"tint":-0.1}"#;
            assert_eq!(clip_grade_from_plan(Some(basic)).unwrap()["mode"], "basic");
            assert!(clip_grade_from_plan(Some(r#"{"exposure":0.1}"#)).is_err());
            assert_eq!(
                clip_preset_spec("mp4-h264-sdr-v1").unwrap().extensions,
                &["mp4"]
            );
            assert_eq!(
                clip_preset_spec("mov-h264-sdr-v1").unwrap().extensions,
                &["mov"]
            );
            let schema = clip_recipe_schema(
                3.4,
                5.2,
                serde_json::json!({ "mode": "none" }),
                "mute",
                "mov-h264-sdr-v1",
            );
            assert_eq!(schema["in_s"], 3.4);
            assert_eq!(schema["out_s"], 5.2);
            assert_eq!(schema["audio"]["mode"], "mute");
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(not(target_os = "macos"))]
pub fn run() {
    eprintln!("Crush desktop is currently supported on macOS only");
}
