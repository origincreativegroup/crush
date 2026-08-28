use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Loaded from crush.toml, then overridden by CRUSH_* env vars.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where library.db, thumbs/, models/, debug/ live. Defaults to the platform app-data dir.
    pub data_dir: Option<PathBuf>,
    pub split: SplitConfig,
    pub embed: EmbedConfig,
    pub search: SearchConfig,
    pub asr: AsrConfig,
    pub limits: LimitsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitConfig {
    /// Frames per second sampled for scene detection (footage is downscaled to 480p first).
    pub sample_fps: f32,
    /// HSV histogram delta threshold. Tune with `crushctl debug scenes`.
    pub threshold: f32,
    /// Minimum shot length in seconds.
    pub min_scene_len_s: f32,
    /// Where in the shot the representative frame is taken (0.0–1.0). 0.4 avoids fade-ins and the cut frame.
    pub rep_frame_pos: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedConfig {
    pub model: String,
    /// "coreml" | "cpu". doctor reports which one is actually active.
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Added once when any non-stopword query term occurs in an overlapping transcript segment.
    pub transcript_hit_boost: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// "base" | "small". doctor picks base on 8 GB machines.
    pub model: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    /// Threads for ort/whisper. 0 = physical cores minus two.
    pub threads: usize,
    /// One video at a time in Phase 1.
    pub concurrent_videos: usize,
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            sample_fps: 4.0,
            threshold: 27.0,
            min_scene_len_s: 0.6,
            rep_frame_pos: 0.4,
        }
    }
}
impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            model: "clip-vit-b-32".into(),
            provider: "coreml".into(),
        }
    }
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            transcript_hit_boost: 0.15,
        }
    }
}
impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model: "small".into(),
            language: None,
        }
    }
}
impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            threads: 0,
            concurrent_videos: 1,
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let mut cfg = match path {
            Some(p) if p.exists() => toml::from_str(&std::fs::read_to_string(p)?)?,
            _ => Config::default(),
        };
        if let Ok(d) = std::env::var("CRUSH_DATA_DIR") {
            cfg.data_dir = Some(PathBuf::from(d));
        }
        Ok(cfg)
    }
}
