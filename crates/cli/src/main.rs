use anyhow::Context;
use clap::{Parser, Subcommand};
use crush_core::{cancellation::CancellationToken, job::JobStatus};
use crush_core::{models, paths::AppPaths, telemetry, Config, DEFAULT_OWNER_ID};
use crush_pipeline::Pipeline;
use crush_search::SearchEngine;
use crush_stage_asr::{
    align_video, choose_model, model_path, production_backend, total_memory_bytes,
};
use crush_stage_embed::{
    embedder::{Embedder, ProviderPreference},
    preprocess::{preprocess, Tensor, IMAGE_SIZE, TENSOR_LEN},
};
use crush_stage_split::{ffmpeg, scene};
use crush_store::{EmbeddingMeta, JobFilter, Store};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "crushctl",
    version,
    about = "Search your footage in plain English. Runs entirely on your machine."
)]
struct Cli {
    /// Path to crush.toml (optional)
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check ffmpeg, models, acceleration, database. Run this first when anything is wrong.
    Doctor,
    /// Download and verify the pinned model release.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// Index a file or folder (Task 11)
    Ingest {
        path: std::path::PathBuf,
        #[arg(long)]
        debug: bool,
    },
    /// Search indexed shots (Task 9)
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        top: usize,
        #[arg(long)]
        json: bool,
    },
    /// List pipeline jobs (Task 11)
    Jobs {
        #[arg(long)]
        failed: bool,
        #[arg(long)]
        video: Option<String>,
    },
    /// Re-run scene splitting and all downstream stages for one stored video.
    Resplit {
        video: String,
        #[arg(long)]
        debug: bool,
    },
    /// Recompute shot embeddings for all videos or one stored video.
    Reembed {
        #[arg(long, conflicts_with = "video")]
        all: bool,
        video: Option<String>,
        #[arg(long)]
        debug: bool,
    },
    /// Export one indexed shot as a standalone playable clip.
    Clip {
        shot_id: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Inspect raw intermediate values from one pipeline stage.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
    /// Manage the personal style model (Task 18a).
    Style {
        #[command(subcommand)]
        command: StyleCommand,
    },
}

#[derive(Subcommand)]
enum ModelsCommand {
    /// Resume missing downloads, verify every sha256, and install atomically.
    Ensure {
        #[arg(long, default_value = models::DEFAULT_MANIFEST_URL)]
        manifest_url: String,
    },
}

#[derive(Subcommand)]
enum DebugCommand {
    /// Sample a video, write scores.csv, and print the per-frame scene scores.
    Scenes { video: PathBuf },
    /// Print the normalized CLIP tensor summary for an image and optional golden JSON.
    Frame {
        image: PathBuf,
        #[arg(long)]
        golden: Option<PathBuf>,
    },
    /// Print a stored shot vector summary and the verified embedding provider.
    Vector { shot_id: String },
    /// Print transcript segments overlapping every shot in a stored video.
    Align { video: String },
}

#[derive(Subcommand)]
enum StyleCommand {
    /// Rebuild the active style profile from retained feedback and confirmed reference sets.
    Retrain {
        /// Context key to train (defaults to the default context).
        #[arg(long)]
        context: Option<String>,
    },
    /// Show the active style profile, its metrics, and the reference sets feeding it.
    Status,
    /// Deactivate every style profile; ranking falls back to the general model.
    Reset,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let paths = AppPaths::resolve(cfg.data_dir.as_ref())?;
    telemetry::init(&paths.logs())?;
    let cancellation = CancellationToken::default();
    let signal_cancellation = cancellation.clone();
    ctrlc::set_handler(move || signal_cancellation.cancel())
        .context("failed to install Ctrl-C handler")?;

    match cli.cmd {
        Cmd::Doctor => doctor(&cfg, &paths),
        Cmd::Models {
            command: ModelsCommand::Ensure { manifest_url },
        } => ensure_models(&paths, &manifest_url),
        Cmd::Ingest { path, debug } => ingest(&cfg, &paths, &cancellation, &path, debug),
        Cmd::Search { query, top, json } => search(&cfg, &paths, &query, top, json),
        Cmd::Jobs { failed, video } => jobs(&paths, failed, video.as_deref()),
        Cmd::Resplit { video, debug } => {
            Pipeline::new(cfg, paths, cancellation).resplit(&video, debug)
        }
        Cmd::Reembed { all, video, debug } => {
            let pipeline = Pipeline::new(cfg, paths, cancellation);
            let count = pipeline.reembed(video.as_deref(), all, debug)?;
            println!("Re-embedded {count} video(s)");
            Ok(())
        }
        Cmd::Clip { shot_id, out } => {
            let result = Pipeline::new(cfg, paths, cancellation).export_clip(&shot_id, &out)?;
            println!(
                "Exported {} with {:?} ({})",
                out.display(),
                result.mode,
                result.command
            );
            Ok(())
        }
        Cmd::Debug {
            command: DebugCommand::Scenes { video },
        } => debug_scenes(&cfg, &paths, &video),
        Cmd::Debug {
            command: DebugCommand::Frame { image, golden },
        } => debug_frame(&image, golden.as_deref()),
        Cmd::Debug {
            command: DebugCommand::Vector { shot_id },
        } => debug_vector(&cfg, &paths, &shot_id),
        Cmd::Debug {
            command: DebugCommand::Align { video },
        } => debug_align(&paths, &video),
        Cmd::Style {
            command: StyleCommand::Retrain { context },
        } => style_retrain(&paths, context.as_deref()),
        Cmd::Style {
            command: StyleCommand::Status,
        } => style_status(&paths),
        Cmd::Style {
            command: StyleCommand::Reset,
        } => style_reset(&paths),
    }
}

fn ingest(
    cfg: &Config,
    paths: &AppPaths,
    cancellation: &CancellationToken,
    input: &Path,
    debug: bool,
) -> anyhow::Result<()> {
    let summary =
        Pipeline::new(cfg.clone(), paths.clone(), cancellation.clone()).ingest(input, debug)?;
    println!(
        "Ingest complete: discovered={} photos={} indexed={} indexed_photos={} skipped={} failed={} recovered_jobs={} search_vectors={}",
        summary.discovered,
        summary.discovered_photos,
        summary.indexed,
        summary.indexed_photos,
        summary.skipped,
        summary.failed,
        summary.recovered_jobs,
        summary.search_vectors
    );
    for (path, error) in &summary.errors {
        eprintln!("failed {}: {error}", path.display());
    }
    anyhow::ensure!(
        summary.failed == 0,
        "{} media file(s) failed",
        summary.failed
    );
    Ok(())
}

fn jobs(paths: &AppPaths, failed: bool, video: Option<&str>) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    let video_id = video
        .map(|target| {
            store
                .video_by_id(DEFAULT_OWNER_ID, target)?
                .or(store.video_by_path(DEFAULT_OWNER_ID, target)?)
                .map(|video| video.id)
                .with_context(|| format!("video {target:?} was not found by id or stored path"))
        })
        .transpose()?;
    let rows = store.jobs(
        DEFAULT_OWNER_ID,
        &JobFilter {
            video_id,
            status: failed.then_some(JobStatus::Failed),
            ..JobFilter::default()
        },
    )?;
    println!(
        "{:<36} {:<10} {:<10} {:>10}  TARGET / ERROR",
        "JOB", "STAGE", "STATUS", "MS"
    );
    for job in rows {
        println!(
            "{:<36} {:<10} {:<10} {:>10}  {}{}",
            job.id,
            format!("{:?}", job.stage).to_ascii_lowercase(),
            format!("{:?}", job.status).to_ascii_lowercase(),
            job.duration_ms
                .map_or_else(|| "—".to_owned(), |value| value.to_string()),
            job.video_id
                .as_deref()
                .or(job.photo_id.as_deref())
                .unwrap_or("—"),
            job.error
                .as_deref()
                .map_or_else(String::new, |error| format!(" — {error}"))
        );
    }
    Ok(())
}

fn search(
    cfg: &Config,
    paths: &AppPaths,
    query: &str,
    top: usize,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(top > 0, "--top must be greater than zero");
    let store = Store::open(&paths.root)?;
    let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, cfg.search.transcript_hit_boost)?;
    let preference = ProviderPreference::parse(&cfg.embed.provider)?;
    eprintln!(
        "search: loading text encoder and {} indexed shot vectors...",
        engine.len()
    );
    let mut embedder = Embedder::new(paths.models(), preference, cfg.limits.threads)?;
    let mut text_embedder = |text: &str| embedder.embed_text(text);
    let results = engine.search_assets(&store, &mut text_embedder, query, top)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    println!(
        "{:<4} {:<6} {:>8} {:>8} {:>10} {:>10}  ASSET",
        "RANK", "TYPE", "SCORE", "COSINE", "START", "END"
    );
    for (rank, result) in results.iter().enumerate() {
        println!(
            "{:<4} {:<6} {:>8.4} {:>8.4} {:>10} {:>10}  {}",
            rank + 1,
            result.asset_type,
            result.score,
            result.cosine,
            result
                .start_s
                .map_or_else(|| "—".to_owned(), |value| format!("{value:.3}")),
            result
                .end_s
                .map_or_else(|| "—".to_owned(), |value| format!("{value:.3}")),
            result.path
        );
        if let Some(breakdown) = &result.score_breakdown {
            println!(
                "     asset={} thumb={} editorial_quality={} aesthetic={} breakdown=semantic{:+.3} transcript{:+.3} general{:+.3} style{:+.3} context{:+.3} penalty{:+.3} editorial{:+.3} total{:+.3}",
                result.asset_id,
                result.thumb_path.as_deref().unwrap_or("-"),
                result
                    .editorial_quality
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                result
                    .aesthetic_score
                    .map_or_else(|| "-".to_owned(), |value| format!("{value:.3}")),
                breakdown.semantic,
                breakdown.transcript_boost,
                breakdown.general_aesthetic,
                breakdown.personal_affinity,
                breakdown.context_fit,
                breakdown.penalties,
                breakdown.editorial,
                breakdown.total,
            );
        }
        if let Some(snippet) = &result.transcript_snippet {
            println!("     transcript: {snippet}");
        }
    }
    Ok(())
}

fn debug_frame(image_path: &Path, golden_path: Option<&Path>) -> anyhow::Result<()> {
    anyhow::ensure!(
        image_path.is_file(),
        "image does not exist: {}",
        image_path.display()
    );
    let image = image::open(image_path)?;
    let tensor = preprocess(&image);
    print_tensor_summary("rust", tensor.values())?;
    if let Some(golden_path) = golden_path {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(golden_path)?)?;
        let values = value
            .get("tensor")
            .and_then(serde_json::Value::as_array)
            .context("golden JSON has no tensor array")?
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .map(|value| value as f32)
                    .context("golden tensor contains a non-number")
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        print_tensor_summary("golden", &values)?;
        anyhow::ensure!(
            values.len() == tensor.values().len(),
            "tensor lengths differ"
        );
        let (maximum, mismatches) = tensor.values().iter().zip(&values).fold(
            (0.0_f32, 0_usize),
            |(maximum, mismatches), (&found, &expected)| {
                let difference = (found - expected).abs();
                (
                    maximum.max(difference),
                    mismatches + usize::from(difference >= 1e-3),
                )
            },
        );
        println!("diff max_abs={maximum:.9} values_at_or_above_1e-3={mismatches}");
    }
    Ok(())
}

fn print_tensor_summary(label: &str, values: &[f32]) -> anyhow::Result<()> {
    anyhow::ensure!(
        values.len() == TENSOR_LEN,
        "{label} tensor has wrong length"
    );
    println!("{label} shape [1, 3, {IMAGE_SIZE}, {IMAGE_SIZE}]");
    let channel_len = IMAGE_SIZE * IMAGE_SIZE;
    for channel in 0..3 {
        let values = &values[channel * channel_len..(channel + 1) * channel_len];
        let minimum = values.iter().copied().fold(f32::INFINITY, f32::min);
        let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mean = values.iter().map(|&value| f64::from(value)).sum::<f64>() / values.len() as f64;
        println!("{label} channel {channel}: min={minimum:.7} max={maximum:.7} mean={mean:.7}");
    }
    println!("{label} first 8: {:?}", &values[..8]);
    Ok(())
}

fn debug_vector(cfg: &Config, paths: &AppPaths, shot_id: &str) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    let vector = store
        .vector_for_shot(DEFAULT_OWNER_ID, shot_id)?
        .with_context(|| format!("shot {shot_id} has no stored vector"))?;
    anyhow::ensure!(
        vector.len() == 512,
        "shot {shot_id} vector has dim {}",
        vector.len()
    );
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let preference = ProviderPreference::parse(&cfg.embed.provider)?;
    eprintln!(
        "verifying embedding provider; first CoreML initialization can take several minutes..."
    );
    let mut embedder = Embedder::new(paths.models(), preference, cfg.limits.threads)?;
    let _ = embedder.embed_image(&Tensor::zeros())?;
    println!("shot_id={shot_id}");
    println!("norm={norm:.9}");
    println!("first_8={:?}", &vector[..8]);
    println!("active={}", embedder.active_provider().as_str());
    Ok(())
}

fn debug_align(paths: &AppPaths, video: &str) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    let video = store
        .video_by_id(DEFAULT_OWNER_ID, video)?
        .or(store.video_by_path(DEFAULT_OWNER_ID, video)?)
        .with_context(|| format!("video {video:?} was not found by id or stored path"))?;
    println!(
        "{:<6} {:>10} {:>10} {:>8}  TEXT",
        "SHOT", "START", "END", "SEGMENTS"
    );
    for alignment in align_video(&store, DEFAULT_OWNER_ID, &video.id)? {
        let text = alignment
            .segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{:<6} {:>10.3} {:>10.3} {:>8}  {}",
            alignment.shot.idx,
            alignment.shot.start_s,
            alignment.shot.end_s,
            alignment.segments.len(),
            text
        );
    }
    Ok(())
}

fn style_retrain(paths: &AppPaths, context: Option<&str>) -> anyhow::Result<()> {
    let mut store = Store::open(&paths.root)?;
    let trained = match context {
        Some(key) => {
            crush_search::retrain_style_profile_for_context(&mut store, DEFAULT_OWNER_ID, key)?
        }
        None => crush_search::retrain_style_profile(&mut store, DEFAULT_OWNER_ID)?,
    };
    match trained {
        Some(profile) => println!(
            "Trained {} v{} for context {:?}: samples={} learned={} held-out={} baseline={}",
            profile.algorithm_version,
            profile.version,
            profile.name,
            profile.sample_count,
            profile.learned,
            metric_text(profile.held_out_metric),
            metric_text(profile.baseline_metric),
        ),
        None => println!(
            "Not enough evidence to train context {:?}; the previous profile is unchanged and \
             ranking keeps the general model.",
            context.unwrap_or("default")
        ),
    }
    Ok(())
}

fn style_status(paths: &AppPaths) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    match store.active_style_profile(DEFAULT_OWNER_ID)? {
        Some(profile) => {
            println!(
                "Active default profile {} v{} ({}): samples={} learned={}",
                profile.id,
                profile.version,
                profile.algorithm_version,
                profile.sample_count,
                profile.learned
            );
            println!(
                "held-out={} baseline={}",
                metric_text(profile.held_out_metric),
                metric_text(profile.baseline_metric)
            );
            println!("metrics {}", profile.metrics_json);
        }
        None => println!("No active default profile; ranking uses the general model."),
    }
    let profiles = store.style_profiles(DEFAULT_OWNER_ID)?;
    println!("{} retained profile version(s)", profiles.len());
    for profile in profiles {
        println!(
            "  {:<20} v{:<3} context={:<16} active={} learned={} samples={} held-out={}",
            profile.id,
            profile.version,
            profile.context_key,
            profile.active,
            profile.learned,
            profile.sample_count,
            metric_text(profile.held_out_metric),
        );
    }
    let sets = store.reference_set_list(DEFAULT_OWNER_ID)?;
    if sets.is_empty() {
        println!("No reference sets.");
        return Ok(());
    }
    for set in sets {
        println!(
            "  {:<24} context={:<16} scope={:<10} status={:<12} items={}",
            set.name,
            set.context_key,
            crush_store::reference_scope_to_str(set.scope),
            crush_store::reference_status_to_str(set.status),
            store.reference_set_items(DEFAULT_OWNER_ID, &set.id)?.len(),
        );
    }
    Ok(())
}

fn style_reset(paths: &AppPaths) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    let count = store.reset_style_profiles(DEFAULT_OWNER_ID)?;
    println!("Deactivated {count} style profile(s); ranking uses the general model.");
    Ok(())
}

fn metric_text(metric: Option<f64>) -> String {
    metric.map_or_else(|| "—".to_owned(), |value| format!("{value:.3}"))
}

fn ensure_models(paths: &AppPaths, manifest_url: &str) -> anyhow::Result<()> {
    let manifest = models::ensure(&paths.models(), manifest_url, |progress| {
        let percent = if progress.total == 0 {
            0.0
        } else {
            progress.downloaded as f64 * 100.0 / progress.total as f64
        };
        eprintln!(
            "model {:<31} {:>6.2}% ({}/{})",
            progress.name, percent, progress.downloaded, progress.total
        );
    })?;
    record_embedding_meta(paths, &manifest)?;
    println!("Models verified in {}", paths.models().display());
    Ok(())
}

fn record_embedding_meta(paths: &AppPaths, manifest: &models::Manifest) -> anyhow::Result<bool> {
    let store = Store::open(&paths.root)?;
    if store.embedding_meta_get(DEFAULT_OWNER_ID)?.is_some() {
        return Ok(false);
    }
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
    Ok(true)
}

fn debug_scenes(cfg: &Config, paths: &AppPaths, video: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(video.is_file(), "video does not exist: {}", video.display());
    let stem = video
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("video")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let debug_dir = paths.debug().join("scenes").join(stem);
    let frame_dir = debug_dir.join("frames");
    let runner = ffmpeg::Runner::new(ffmpeg::resolve()?, cfg.limits.threads, "debug-scenes")
        .with_debug_dir(&debug_dir);
    let duration_s = runner.probe(video)?.value.duration_s;
    runner.sample_frames(video, f64::from(cfg.split.sample_fps), &frame_dir)?;
    let mut frames = std::fs::read_dir(&frame_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    frames.retain(|path| path.extension().is_some_and(|extension| extension == "jpg"));
    frames.sort();
    let detection =
        scene::detect_with_duration(&frames, cfg.split.sample_fps, duration_s, &cfg.split)?;
    let csv = scene::scores_csv(&detection.scores);
    scene::write_scores_csv(&debug_dir.join("scores.csv"), &detection.scores)?;
    print!("{csv}");
    Ok(())
}

fn doctor(cfg: &Config, paths: &AppPaths) -> anyhow::Result<()> {
    tracing::info!(job_id = "doctor", stage = "doctor", "doctor started");
    println!("Crush doctor");
    println!("  data dir      {}", paths.root.display());
    println!(
        "  database      {} ({})",
        paths.db().display(),
        if paths.db().exists() {
            "present"
        } else {
            "not created yet"
        }
    );
    let resolved = ffmpeg::resolve()?;
    let runner = ffmpeg::Runner::new(resolved, cfg.limits.threads, "doctor");
    let version = runner.version()?.value;
    println!(
        "  ffmpeg        {} source={} path={}",
        version,
        runner.resolved().source,
        runner.resolved().path.display()
    );
    println!(
        "  ffprobe       path={}",
        runner.resolved().ffprobe_path.display()
    );
    let manifest = models::bundled_manifest()?;
    let model_checks = models::inspect(&paths.models(), &manifest)?;
    let mut models_green = true;
    for check in &model_checks {
        let status = match check.status {
            models::ModelStatus::Present => "present",
            models::ModelStatus::Missing => {
                models_green = false;
                "missing"
            }
            models::ModelStatus::ShaMismatch => {
                models_green = false;
                "sha-mismatch"
            }
        };
        println!("  model         {} {}", check.name, status);
    }
    println!(
        "  models        {}",
        if models_green {
            "green"
        } else {
            "attention required"
        }
    );
    if models_green {
        if record_embedding_meta(paths, &manifest)? {
            println!("  embed metadata initialized");
        }
        let preference = ProviderPreference::parse(&cfg.embed.provider)?;
        eprintln!(
            "doctor: initializing embedding sessions; first CoreML use can take several minutes..."
        );
        let initialized = Instant::now();
        let mut embedder = Embedder::new(paths.models(), preference, cfg.limits.threads)?;
        let init_ms = initialized.elapsed().as_secs_f64() * 1_000.0;
        let input = Tensor::zeros();
        let _ = embedder.embed_image(&input)?;
        let measured = Instant::now();
        for _ in 0..20 {
            let _ = embedder.embed_image(&input)?;
        }
        let mean_ms = measured.elapsed().as_secs_f64() * 1_000.0 / 20.0;
        println!(
            "  embed provider requested={} active={} providers={} init_ms={:.2} ms/frame={mean_ms:.2}",
            embedder.requested_provider().as_str(),
            embedder.active_provider().as_str(),
            embedder
                .active_providers()
                .iter()
                .map(|provider| provider.as_str())
                .collect::<Vec<_>>()
                .join(","),
            init_ms,
        );
        for warning in embedder.warnings() {
            println!("  embed warning  {warning}");
        }
    } else {
        println!(
            "  embed provider requested={} active=unavailable — install models first",
            cfg.embed.provider
        );
    }
    let memory = total_memory_bytes();
    let selected_asr_model = choose_model(&cfg.asr.model, memory)?;
    println!(
        "  memory        {}",
        memory.map_or_else(
            || "unknown".to_owned(),
            |bytes| format!(
                "{} bytes ({:.1} GiB)",
                bytes,
                bytes as f64 / 1024_f64.powi(3)
            )
        )
    );
    let asr_model_path = model_path(paths.models(), selected_asr_model);
    println!(
        "  whisper       configured={} selected={} backend={} model={}",
        cfg.asr.model,
        selected_asr_model,
        production_backend(),
        if asr_model_path.is_file() {
            "present"
        } else {
            "missing"
        }
    );
    println!("  threads       {} (0 = cores-2)", cfg.limits.threads);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_scenes_cli_shape_is_stable() {
        let cli = Cli::try_parse_from(["crushctl", "debug", "scenes", "clip.mp4"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Debug {
                command: DebugCommand::Scenes { video }
            } if video == Path::new("clip.mp4")
        ));
    }

    #[test]
    fn models_ensure_cli_shape_is_stable() {
        let cli = Cli::try_parse_from([
            "crushctl",
            "models",
            "ensure",
            "--manifest-url",
            "http://127.0.0.1/manifest.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Models {
                command: ModelsCommand::Ensure { manifest_url }
            } if manifest_url == "http://127.0.0.1/manifest.json"
        ));
    }

    #[test]
    fn debug_frame_cli_shape_is_stable() {
        let cli = Cli::try_parse_from([
            "crushctl",
            "debug",
            "frame",
            "frame.png",
            "--golden",
            "frame.image.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Debug {
                command: DebugCommand::Frame { image, golden }
            } if image == Path::new("frame.png")
                && golden.as_deref() == Some(Path::new("frame.image.json"))
        ));
    }

    #[test]
    fn debug_vector_cli_shape_is_stable() {
        let cli = Cli::try_parse_from(["crushctl", "debug", "vector", "shot-123"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Debug {
                command: DebugCommand::Vector { shot_id }
            } if shot_id == "shot-123"
        ));
    }

    #[test]
    fn debug_align_cli_shape_is_stable() {
        let cli = Cli::try_parse_from(["crushctl", "debug", "align", "video-123"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Debug {
                command: DebugCommand::Align { video }
            } if video == "video-123"
        ));
    }

    #[test]
    fn style_cli_shapes_are_stable() {
        let retrain = Cli::try_parse_from(["crushctl", "style", "retrain"]).unwrap();
        assert!(matches!(
            retrain.cmd,
            Cmd::Style {
                command: StyleCommand::Retrain { context: None }
            }
        ));
        let retrain_context =
            Cli::try_parse_from(["crushctl", "style", "retrain", "--context", "homepage-hero"])
                .unwrap();
        assert!(matches!(
            retrain_context.cmd,
            Cmd::Style {
                command: StyleCommand::Retrain { context: Some(context) }
            } if context == "homepage-hero"
        ));
        let status = Cli::try_parse_from(["crushctl", "style", "status"]).unwrap();
        assert!(matches!(
            status.cmd,
            Cmd::Style {
                command: StyleCommand::Status
            }
        ));
        let reset = Cli::try_parse_from(["crushctl", "style", "reset"]).unwrap();
        assert!(matches!(
            reset.cmd,
            Cmd::Style {
                command: StyleCommand::Reset
            }
        ));
    }

    #[test]
    fn search_cli_accepts_top_and_json() {
        let cli = Cli::try_parse_from([
            "crushctl",
            "search",
            "a rocket launching",
            "--top",
            "3",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Search { query, top: 3, json: true } if query == "a rocket launching"
        ));
    }

    #[test]
    fn ingest_jobs_reprocess_and_clip_cli_shapes_are_stable() {
        let ingest = Cli::try_parse_from(["crushctl", "ingest", "clips", "--debug"]).unwrap();
        assert!(matches!(
            ingest.cmd,
            Cmd::Ingest { path, debug: true } if path == Path::new("clips")
        ));
        let jobs = Cli::try_parse_from(["crushctl", "jobs", "--failed", "--video", "/clips/a.mov"])
            .unwrap();
        assert!(matches!(
            jobs.cmd,
            Cmd::Jobs { failed: true, video: Some(video) } if video == "/clips/a.mov"
        ));
        let resplit = Cli::try_parse_from(["crushctl", "resplit", "video-1"]).unwrap();
        assert!(matches!(
            resplit.cmd,
            Cmd::Resplit { video, debug: false } if video == "video-1"
        ));
        let reembed = Cli::try_parse_from(["crushctl", "reembed", "--all"]).unwrap();
        assert!(matches!(
            reembed.cmd,
            Cmd::Reembed {
                all: true,
                video: None,
                debug: false
            }
        ));
        let clip =
            Cli::try_parse_from(["crushctl", "clip", "shot-1", "--out", "export.mp4"]).unwrap();
        assert!(matches!(
            clip.cmd,
            Cmd::Clip { shot_id, out }
                if shot_id == "shot-1" && out == Path::new("export.mp4")
        ));
    }
}
