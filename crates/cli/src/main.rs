use clap::{Parser, Subcommand};
use crush_core::{paths::AppPaths, telemetry, Config};

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
    }
}

/// Task 1 stub. Each line becomes a real check as its task lands.
fn doctor(cfg: &Config, paths: &AppPaths) -> anyhow::Result<()> {
    tracing::info!(job_id = "doctor", stage = "doctor", "doctor started");
    println!("Crush doctor");
    println!("  data dir      {}", paths.root.display());
    println!(
        "  database      {} (unchecked — Task 2)",
        paths.db().display()
    );
    println!("  ffmpeg        unchecked — Task 4 (must resolve from bundle, not PATH)");
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
