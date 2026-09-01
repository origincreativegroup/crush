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
use crush_store::{AssetFilter, EmbeddingMeta, JobFilter, LibraryCounts, Store};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Full version line with build provenance, e.g. `0.0.1 (build 823c867)`.
/// clap's derive needs a `&'static str`, so the formatted line lives in a
/// `LazyLock` instead of a `const` (the commit comes from crush-core's
/// compile-time stamp; see docs/release.md).
static VERSION_LINE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{} (build {})",
        env!("CARGO_PKG_VERSION"),
        crush_core::BUILD_COMMIT
    )
});

#[derive(Parser)]
#[command(
    name = "crushctl",
    version = VERSION_LINE.as_str(),
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
    Doctor {
        /// Also run the deep library-integrity scan (missing vectors, transcripts, thumbnails,
        /// foreign-key orphans) and report every problem found.
        #[arg(long)]
        deep: bool,
    },
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
    /// Relink a moved or renamed file after verifying its bytes are identical (SHA-256).
    /// The catalog row is re-pointed in place; a different file is refused and the
    /// original file is never modified.
    Relink {
        /// Asset id (video-… or photo-…) or the stale stored path of the moved file.
        target: String,
        /// Where the file lives now. Crush verifies it is the same media before relinking.
        new_path: PathBuf,
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
    /// Browse the mixed photo/shot library with organization filters (Task 19a).
    Library {
        /// Filter to photos or shots (videos map to their shots).
        #[arg(long)]
        kind: Option<String>,
        /// Filter to one collection id.
        #[arg(long)]
        collection: Option<String>,
        /// Filter to one version-stack id.
        #[arg(long)]
        stack: Option<String>,
        /// Filter to a per-item context key (e.g. homepage-hero).
        #[arg(long)]
        context: Option<String>,
        /// Case-insensitive file-name substring over the stored path.
        #[arg(long)]
        search: Option<String>,
        /// Match assets with at least one recorded editorial action: pick, reject, or rating.
        #[arg(long, value_name = "pick|reject|rating")]
        feedback: Option<String>,
        /// Only assets rated at least this many stars (1–5).
        #[arg(long, value_name = "N")]
        min_rating: Option<i64>,
        /// Print JSON rows instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Strong-shot selects with an optional brief-driven personalized ordering (Task 20a).
    Selects {
        /// Creative brief for the personalized ordering; omit for the general list only.
        #[arg(long)]
        brief: Option<String>,
        /// Context key the personalized ranking is scoped to.
        #[arg(long)]
        context: Option<String>,
        #[arg(long, default_value_t = 12)]
        top: usize,
        /// Diversify the general list: at most this many candidates per source
        /// (near-duplicate photos count as one source). Omit for no cap.
        #[arg(long)]
        duplicate_cap: Option<usize>,
        /// Print JSON rows instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List editorial plans and their items (Task 20a).
    Plans {
        /// Show the items of one plan id.
        #[arg(long)]
        items: Option<String>,
        /// Print JSON rows instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Import historical evidence from another tool (Task 022).
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Import a Reel Studio catalogue (`clips.db`) and exported reel recipes. Dry-run by default.
    ReelStudio {
        /// Path to the Reel Studio `clips.db`.
        #[arg(long)]
        catalogue: PathBuf,
        /// Directory containing the original source files (repeatable).
        #[arg(long = "originals")]
        originals: Vec<PathBuf>,
        /// Reel Studio library folder holding `clips/<segment_id>.mp4` (improves boundary basis).
        #[arg(long)]
        library: Option<PathBuf>,
        /// Exported reel recipe JSON (repeatable).
        #[arg(long = "recipe")]
        recipes: Vec<PathBuf>,
        /// Context key for the imported projects.
        #[arg(long, default_value = "default")]
        context: String,
        /// Also match originals by SHA-256 when the stored path differs (slow on 4K footage).
        #[arg(long)]
        match_by_hash: bool,
        /// Tolerance recorded for keyframe-aligned library copies, in seconds.
        #[arg(long, default_value_t = crush_pipeline::reel_studio_import::DEFAULT_KEYFRAME_TOLERANCE_S)]
        keyframe_tolerance: f64,
        /// Write the planned changes. Without this flag only the report is produced.
        #[arg(long)]
        apply: bool,
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
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
        Cmd::Doctor { deep } => doctor(&cfg, &paths, deep),
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
        Cmd::Relink { target, new_path } => {
            let outcome = Pipeline::new(cfg, paths, cancellation).relink(&target, &new_path)?;
            println!(
                "Relinked {} {} (SHA-256 verified; the original file was not modified)",
                outcome.media_kind, outcome.id
            );
            println!("  was {}", outcome.old_path);
            println!("  now {}", outcome.new_path);
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
        Cmd::Library {
            kind,
            collection,
            stack,
            context,
            search,
            feedback,
            min_rating,
            json,
        } => library(
            &paths, kind, collection, stack, context, search, feedback, min_rating, json,
        ),
        Cmd::Selects {
            brief,
            context,
            top,
            duplicate_cap,
            json,
        } => selects(&cfg, &paths, brief, context, top, duplicate_cap, json),
        Cmd::Plans { items, json } => plans(&paths, items, json),
        Cmd::Import {
            command:
                ImportCommand::ReelStudio {
                    catalogue,
                    originals,
                    library,
                    recipes,
                    context,
                    match_by_hash,
                    keyframe_tolerance,
                    apply,
                    json,
                },
        } => import_reel_studio(
            &cfg,
            &paths,
            crush_pipeline::reel_studio_import::ImportOptions {
                catalogue,
                originals,
                library,
                recipes,
                context_key: context,
                apply,
                match_by_hash,
                keyframe_tolerance_s: keyframe_tolerance,
                threads: cfg.limits.threads,
            },
            json,
        ),
    }
}

fn import_reel_studio(
    _cfg: &Config,
    paths: &AppPaths,
    options: crush_pipeline::reel_studio_import::ImportOptions,
    json: bool,
) -> anyhow::Result<()> {
    let mut store = Store::open(&paths.root)?;
    let report = crush_pipeline::reel_studio_import::import_reel_studio(&mut store, &options)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("{}", report.summary_line());
    println!(
        "import {} · catalogue sha256 {}",
        report.import_id, report.catalogue_sha256
    );
    println!("\nsources");
    for source in &report.sources {
        println!(
            "  {:<12} {:<12} {}",
            source.clip_id,
            source.matched_by,
            source.video_id.as_deref().unwrap_or("-")
        );
    }
    println!("\nsegments");
    for segment in &report.segments {
        println!(
            "  {:<16} {:<9} {:>8.3}..{:<8.3} {:<13} ±{:.3}s {}",
            segment.segment_id,
            segment.outcome,
            segment.start_s,
            segment.end_s,
            segment.boundary_basis,
            segment.boundary_tolerance_s,
            segment.reason.as_deref().unwrap_or("")
        );
    }
    if !report.recipes.is_empty() {
        println!("\nrecipes");
        for recipe in &report.recipes {
            println!(
                "  {:<40} {:<9} items={} finished={} {}",
                recipe.file,
                recipe.outcome,
                recipe.items,
                recipe.finished_project,
                recipe.reason.as_deref().unwrap_or("")
            );
        }
    }
    if !report.issues.is_empty() {
        println!("\nissues");
        for issue in &report.issues {
            println!(
                "  {:<15} {:<20} {}",
                issue.kind, issue.subject, issue.detail
            );
        }
    }
    let writes = &report.planned_writes;
    println!(
        "\nplanned writes: spans +{} ~{} · recipes +{} · projects +{} (items {}) · feedback +{} · reference sets +{}",
        writes.manual_spans_insert,
        writes.manual_spans_update,
        writes.render_recipes_insert,
        writes.plans_insert,
        writes.plan_items_insert,
        writes.feedback_events_insert,
        writes.reference_sets_insert
    );
    if !report.reference_set_candidates.is_empty() {
        println!(
            "finished projects eligible as previous-work reference sets (confirm explicitly in Preferences): {}",
            report.reference_set_candidates.join(", ")
        );
    }
    if !options.apply {
        println!("\ndry run only — re-run with --apply to write these changes");
    }
    Ok(())
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
    let mut store = Store::open(&paths.root)?;
    let count = store.reset_style_profiles(DEFAULT_OWNER_ID)?;
    println!("Deactivated {count} style profile(s); ranking uses the general model.");
    Ok(())
}

#[allow(clippy::too_many_arguments)] // flat positional CLI handler mirrors the subcommand fields
fn library(
    paths: &AppPaths,
    kind: Option<String>,
    collection: Option<String>,
    stack: Option<String>,
    context: Option<String>,
    search: Option<String>,
    feedback: Option<String>,
    min_rating: Option<i64>,
    json: bool,
) -> anyhow::Result<()> {
    let parsed_kind = match kind.as_deref() {
        Some("photo") => Some(crush_store::MediaKind::Photo),
        Some("shot" | "video") => Some(crush_store::MediaKind::Shot),
        Some(other) => anyhow::bail!("unsupported kind {other:?} (use photo or shot)"),
        None => None,
    };
    let filter = AssetFilter {
        kind: parsed_kind,
        collection_id: collection,
        stack_id: stack,
        context_key: context,
        search,
        feedback,
        quality_min: min_rating,
        ..AssetFilter::default()
    };
    let store = Store::open(&paths.root)?;
    let counts: LibraryCounts = store.library_counts(DEFAULT_OWNER_ID)?;
    let assets = store.browse_assets(DEFAULT_OWNER_ID, &filter)?;
    if json {
        let rows: Vec<serde_json::Value> = assets
            .iter()
            .map(|asset| {
                serde_json::json!({
                    "media_kind": match asset.media_kind {
                        crush_store::MediaKind::Photo => "photo",
                        crush_store::MediaKind::Shot => "shot",
                        crush_store::MediaKind::Span => "span",
                    },
                    "media_id": asset.media_id,
                    "path": asset.path,
                    "status": asset.status,
                    "quality": asset.quality,
                    "usable": asset.usable,
                    "blur_required": asset.blur_required,
                    "video_id": asset.video_id,
                    "start_s": asset.start_s,
                    "end_s": asset.end_s,
                    "collections": asset.collection_ids,
                    "stacks": asset.stack_ids,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "counts": {
                    "photos": counts.photos,
                    "shots": counts.shots,
                    "picks": counts.picks,
                    "rejects": counts.rejects,
                    "flagged": counts.flagged,
                },
                "assets": rows,
            }))?
        );
        return Ok(());
    }
    println!(
        "photos={} shots={} picks={} rejects={} flagged={}",
        counts.photos, counts.shots, counts.picks, counts.rejects, counts.flagged
    );
    println!(
        "{:<6} {:<24} {:<10} {:>7}  {:<8} {:<8}  ASSET",
        "KIND", "ID", "STATUS", "QUALITY", "COLS", "STACKS"
    );
    for asset in &assets {
        let kind = match asset.media_kind {
            crush_store::MediaKind::Photo => "photo",
            crush_store::MediaKind::Shot => "shot",
            crush_store::MediaKind::Span => "span",
        };
        println!(
            "{:<6} {:<24} {:<10} {:>7}  {:<8} {:<8}  {}",
            kind,
            asset.media_id,
            asset.status,
            asset
                .quality
                .map_or_else(|| "—".to_owned(), |value| value.to_string()),
            asset.collection_ids.len(),
            asset.stack_ids.len(),
            asset.path,
        );
    }
    Ok(())
}

fn metric_text(metric: Option<f64>) -> String {
    metric.map_or_else(|| "—".to_owned(), |value| format!("{value:.3}"))
}

fn selects(
    cfg: &Config,
    paths: &AppPaths,
    brief: Option<String>,
    context: Option<String>,
    top: usize,
    duplicate_cap: Option<usize>,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(top > 0, "--top must be greater than zero");
    let store = Store::open(&paths.root)?;
    let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, cfg.search.transcript_hit_boost)?;
    let preference = ProviderPreference::parse(&cfg.embed.provider)?;
    eprintln!(
        "selects: loading text encoder and {} indexed vectors...",
        engine.len()
    );
    let mut embedder = Embedder::new(paths.models(), preference, cfg.limits.threads)?;
    let mut text_embedder = |text: &str| embedder.embed_text(text);
    let selection = crush_search::selects_candidates(
        &store,
        DEFAULT_OWNER_ID,
        &engine,
        &mut text_embedder,
        brief.as_deref(),
        top,
        context.as_deref(),
        duplicate_cap,
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&selection)?);
        return Ok(());
    }
    if !selection.brief.is_empty() {
        println!(
            "brief: {} (context: {})",
            selection.brief,
            selection.context_key.as_deref().unwrap_or("none")
        );
    }
    // Echo the cap and the skipped count in table mode too, so the human-readable output
    // is as honest about diversification as the JSON mode.
    match selection.duplicate_cap {
        Some(cap) => println!(
            "\ngeneral strong shots ({} of {}, similar-shot cap {}, {} skipped):",
            selection.general.len(),
            selection.general.len() + selection.skipped_duplicates,
            cap,
            selection.skipped_duplicates
        ),
        None => println!("\ngeneral strong shots ({}):", selection.general.len()),
    }
    for (rank, result) in selection.general.iter().enumerate() {
        println!(
            "{:<4} {:<6} {:>8}  {}",
            rank + 1,
            result.asset_type,
            format!("{:.3}", result.score),
            result.path
        );
    }
    if selection.personalized.is_empty() {
        println!("\npersonalized: (no brief supplied)");
        return Ok(());
    }
    println!(
        "\npersonalized for the brief ({}):",
        selection.personalized.len()
    );
    for (rank, result) in selection.personalized.iter().enumerate() {
        let breakdown = result.score_breakdown;
        println!(
            "{:<4} {:<6} {:>8}  style={:<8}  {}",
            rank + 1,
            result.asset_type,
            format!("{:.3}", result.score),
            result
                .personal_style_score
                .map_or_else(|| "-".to_owned(), |value| format!("{value:.3}")),
            result.path
        );
        if let Some(breakdown) = breakdown {
            println!(
                "     semantic{:+.3} general{:+.3} style{:+.3} context{:+.3} penalty{:+.3} total{:+.3}",
                breakdown.semantic,
                breakdown.general_aesthetic,
                breakdown.personal_affinity,
                breakdown.context_fit,
                breakdown.penalties,
                breakdown.total,
            );
        }
    }
    Ok(())
}

fn plans(paths: &AppPaths, items: Option<String>, json: bool) -> anyhow::Result<()> {
    let store = Store::open(&paths.root)?;
    if let Some(plan_id) = items {
        let plan = store
            .plan_get(DEFAULT_OWNER_ID, &plan_id)?
            .with_context(|| format!("plan {plan_id:?} was not found"))?;
        let rows = store.plan_items(DEFAULT_OWNER_ID, &plan_id)?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "plan": {
                        "id": plan.id,
                        "name": plan.name,
                        "context_key": plan.context_key,
                        "brief": plan.brief,
                        "updated_at": plan.updated_at.to_rfc3339(),
                    },
                    "items": rows,
                }))?
            );
            return Ok(());
        }
        println!(
            "plan {} ({}) context={} items={}",
            plan.name,
            plan.id,
            plan.context_key,
            rows.len()
        );
        println!(
            "{:<4} {:<6} {:<24} {:>8} {:>8}  {:<10} MEDIA",
            "#", "KIND", "ID", "START", "END", "ORIGIN"
        );
        for item in &rows {
            println!(
                "{:<4} {:<6} {:<24} {:>8} {:>8}  {:<10} {}",
                item.position,
                match item.media_kind {
                    crush_store::MediaKind::Photo => "photo",
                    crush_store::MediaKind::Shot => "shot",
                    crush_store::MediaKind::Span => "span",
                },
                item.media_id,
                item.start_s
                    .map_or_else(|| "—".to_owned(), |v| format!("{v:.2}")),
                item.end_s
                    .map_or_else(|| "—".to_owned(), |v| format!("{v:.2}")),
                crush_store::plan_origin_to_str(item.origin),
                item.reason,
            );
        }
        return Ok(());
    }
    let plans = store.plan_list(DEFAULT_OWNER_ID)?;
    if plans.is_empty() {
        println!("No editorial plans.");
        return Ok(());
    }
    if json {
        let rows = plans
            .iter()
            .map(|plan| {
                Ok(serde_json::json!({
                    "id": plan.id,
                    "name": plan.name,
                    "context_key": plan.context_key,
                    "brief": plan.brief,
                    "created_at": plan.created_at.to_rfc3339(),
                    "updated_at": plan.updated_at.to_rfc3339(),
                    "items": store.plan_items(DEFAULT_OWNER_ID, &plan.id)?.len(),
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!("{:<36} {:<24} {:<16} ITEMS", "PLAN", "NAME", "CONTEXT");
    for plan in &plans {
        let count = store.plan_items(DEFAULT_OWNER_ID, &plan.id)?.len();
        println!(
            "{:<36} {:<24} {:<16} {}",
            plan.id, plan.name, plan.context_key, count
        );
    }
    Ok(())
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

fn doctor(cfg: &Config, paths: &AppPaths, deep: bool) -> anyhow::Result<()> {
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
    if deep {
        let store = Store::open(&paths.root)?;
        let problems = store.integrity()?;
        if problems.is_empty() {
            println!("  integrity     clean");
        } else {
            for problem in &problems {
                println!(
                    "  integrity     {:?} {}: {}",
                    problem.kind, problem.entity_id, problem.detail
                );
            }
        }
    }
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
    fn selects_cli_shape_is_stable() {
        let with_brief = Cli::try_parse_from([
            "crushctl",
            "selects",
            "--brief",
            "a quiet travel film",
            "--context",
            "homepage-hero",
            "--top",
            "5",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            with_brief.cmd,
            Cmd::Selects {
                brief: Some(brief),
                context: Some(context),
                top: 5,
                duplicate_cap: None,
                json: true,
            } if brief == "a quiet travel film" && context == "homepage-hero"
        ));
        let general_only = Cli::try_parse_from(["crushctl", "selects", "--top", "3"]).unwrap();
        assert!(matches!(
            general_only.cmd,
            Cmd::Selects {
                brief: None,
                context: None,
                top: 3,
                duplicate_cap: None,
                json: false,
            }
        ));
    }

    #[test]
    fn plans_cli_shape_is_stable() {
        let list = Cli::try_parse_from(["crushctl", "plans"]).unwrap();
        assert!(matches!(
            list.cmd,
            Cmd::Plans {
                items: None,
                json: false
            }
        ));
        let items =
            Cli::try_parse_from(["crushctl", "plans", "--items", "plan-1", "--json"]).unwrap();
        assert!(matches!(
            items.cmd,
            Cmd::Plans {
                items: Some(items),
                json: true,
            } if items == "plan-1"
        ));
    }

    #[test]
    fn library_cli_shape_is_stable() {
        let cli = Cli::try_parse_from([
            "crushctl",
            "library",
            "--kind",
            "shot",
            "--collection",
            "col-1",
            "--stack",
            "stack-1",
            "--context",
            "homepage-hero",
            "--search",
            "rocket",
            "--feedback",
            "pick",
            "--min-rating",
            "4",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Library {
                kind: Some(kind),
                collection: Some(collection),
                stack: Some(stack),
                context: Some(context),
                search: Some(search),
                feedback: Some(feedback),
                min_rating: Some(min_rating),
                json: true,
            } if kind == "shot"
                && collection == "col-1"
                && stack == "stack-1"
                && context == "homepage-hero"
                && search == "rocket"
                && feedback == "pick"
                && min_rating == 4
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

    #[test]
    fn relink_cli_shape_is_stable() {
        let by_id = Cli::try_parse_from([
            "crushctl",
            "relink",
            "video-abc123",
            "/Volumes/Footage/renamed.mov",
        ])
        .unwrap();
        assert!(matches!(
            by_id.cmd,
            Cmd::Relink { target, new_path }
                if target == "video-abc123" && new_path == Path::new("/Volumes/Footage/renamed.mov")
        ));
        let by_stale_path =
            Cli::try_parse_from(["crushctl", "relink", "/old/drive/clip.mov", "/new/clip.mov"])
                .unwrap();
        assert!(matches!(
            by_stale_path.cmd,
            Cmd::Relink { target, new_path }
                if target == "/old/drive/clip.mov" && new_path == Path::new("/new/clip.mov")
        ));
    }
}
