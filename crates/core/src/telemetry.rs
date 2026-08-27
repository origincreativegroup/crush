use std::path::Path;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Pretty to terminal, JSON to logs/crush.log. Every stage span must carry job_id and stage.
pub fn init(log_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(log_dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("crush.log"))?;
    let filter = EnvFilter::try_from_env("CRUSH_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(fmt::layer().json().with_writer(file))
        .init();
    Ok(())
}
