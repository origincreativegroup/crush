use serde::{Deserialize, Serialize};

/// The pipeline stages, in order. Each is its own crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Split,
    Embed,
    Analyze,
    Transcribe,
    /// Whole-file ingest of one photo: decode, derivatives, and embedding.
    PhotoIngest,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Split => "split",
            Self::Embed => "embed",
            Self::Analyze => "analyze",
            Self::Transcribe => "transcribe",
            Self::PhotoIngest => "photo_ingest",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// One row in the `jobs` table — the debugging spine. Written by the store crate (Task 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub owner_id: String,
    /// Video a job processes; always set except for photo jobs.
    pub video_id: Option<String>,
    /// Photo a job processes; always set except for video jobs.
    pub photo_id: Option<String>,
    pub stage: Stage,
    pub status: JobStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub debug_dir: Option<String>,
}
