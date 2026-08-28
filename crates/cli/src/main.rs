use anyhow::Context;
use clap::{Parser, Subcommand};
use crush_core::{models, paths::AppPaths, telemetry, Config, DEFAULT_OWNER_ID};
use crush_stage_embed::{
    embedder::{Embedder, ProviderPreference},
    preprocess::{preprocess, Tensor, IMAGE_SIZE, TENSOR_LEN},
};
use crush_stage_split::{ffmpeg, scene};
use crush_store::{EmbeddingMeta, Store};
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
    },
    /// List pipeline jobs (Task 11)
    Jobs {
        #[arg(long)]
        failed: bool,
    },
    /// Inspect raw intermediate values from one pipeline stage.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let paths = AppPaths::resolve(cfg.data_dir.as_ref())?;
    telemetry::init(&paths.logs())?;

    match cli.cmd {
        Cmd::Doctor => doctor(&cfg, &paths),
        Cmd::Models {
            command: ModelsCommand::Ensure { manifest_url },
        } => ensure_models(&paths, &manifest_url),
        Cmd::Ingest { .. } => anyhow::bail!("not implemented yet — Task 11"),
        Cmd::Search { .. } => anyhow::bail!("not implemented yet — Task 9"),
        Cmd::Jobs { .. } => anyhow::bail!("not implemented yet — Task 11"),
        Cmd::Debug {
            command: DebugCommand::Scenes { video },
        } => debug_scenes(&cfg, &paths, &video),
        Cmd::Debug {
            command: DebugCommand::Frame { image, golden },
        } => debug_frame(&image, golden.as_deref()),
        Cmd::Debug {
            command: DebugCommand::Vector { shot_id },
        } => debug_vector(&cfg, &paths, &shot_id),
    }
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
    println!(
        "  whisper       model={} metal=unchecked — Task 10",
        cfg.asr.model
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
}
