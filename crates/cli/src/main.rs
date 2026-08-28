use clap::{Parser, Subcommand};
use crush_core::{paths::AppPaths, telemetry, Config};
use crush_stage_split::{ffmpeg, scene};
use std::path::{Path, PathBuf};

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
enum DebugCommand {
    /// Sample a video, write scores.csv, and print the per-frame scene scores.
    Scenes { video: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    let paths = AppPaths::resolve(cfg.data_dir.as_ref())?;
    telemetry::init(&paths.logs())?;

    match cli.cmd {
        Cmd::Doctor => doctor(&cfg, &paths),
        Cmd::Ingest { .. } => anyhow::bail!("not implemented yet — Task 11"),
        Cmd::Search { .. } => anyhow::bail!("not implemented yet — Task 9"),
        Cmd::Jobs { .. } => anyhow::bail!("not implemented yet — Task 11"),
        Cmd::Debug {
            command: DebugCommand::Scenes { video },
        } => debug_scenes(&cfg, &paths, &video),
    }
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

/// Task 1 stub. Each line becomes a real check as its task lands.
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
    println!("  models        unchecked — Task 6 (sha256 verified)");
    println!(
        "  embed provider requested={} active=unchecked — Task 8",
        cfg.embed.provider
    );
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
}
