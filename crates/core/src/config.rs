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
    pub ai: AiConfig,
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
    /// "auto" | "base" | "small". Auto picks base below 12 GiB, otherwise small.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    /// "none" | "ollama". "none" is honest: describe returns a capability error,
    /// never a silent fallback, and nothing else in Crush is affected.
    pub provider: String,
    /// Ollama host (ai-srv). Configured, never auto-discovered.
    pub ollama_host: String,
    pub ollama_model: String,
    /// nodeo's tuned finding: low temperature for consistent JSON output.
    pub temperature: f32,
    /// nodeo's tuned finding: 300 tokens covers the single-call structured response.
    pub max_tokens: u32,
    /// Bounded batch workers so a describe batch never saturates the one Ollama host.
    pub max_concurrent: usize,
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
            model: "auto".into(),
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
impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            ollama_host: "http://192.168.50.247:11434".into(),
            ollama_model: "llava".into(),
            temperature: 0.3,
            max_tokens: 300,
            max_concurrent: 2,
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
        if let Ok(p) = std::env::var("CRUSH_AI_PROVIDER") {
            cfg.ai.provider = p;
        }
        if let Ok(h) = std::env::var("CRUSH_AI_OLLAMA_HOST") {
            cfg.ai.ollama_host = h;
        }
        if let Ok(m) = std::env::var("CRUSH_AI_OLLAMA_MODEL") {
            cfg.ai.ollama_model = m;
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_defaults_are_stable_and_round_trip_through_toml() {
        let original = Config::default();
        let serialized = toml::to_string(&original).expect("config serializes to toml");
        let parsed: Config = toml::from_str(&serialized).expect("serialized config parses");
        assert_eq!(parsed.ai.provider, "none");
        assert_eq!(parsed.ai.ollama_host, "http://192.168.50.247:11434");
        assert_eq!(parsed.ai.ollama_model, "llava");
        assert!((parsed.ai.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(parsed.ai.max_tokens, 300);
        assert_eq!(parsed.ai.max_concurrent, 2);
    }

    #[test]
    fn config_without_ai_section_fills_ai_defaults() {
        let parsed: Config =
            toml::from_str("[limits]\nthreads = 2\n").expect("partial config parses");
        assert_eq!(parsed.limits.threads, 2);
        assert_eq!(parsed.ai.provider, "none");
        assert_eq!(parsed.ai.ollama_model, "llava");
    }

    #[test]
    fn ai_env_overrides_apply() {
        // Env vars are process-global; hold one lock so any future env-touching
        // test in this crate serializes against this one.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: the ENV_LOCK above makes this the only thread touching these vars.
        unsafe {
            std::env::set_var("CRUSH_AI_PROVIDER", "ollama");
            std::env::set_var("CRUSH_AI_OLLAMA_HOST", "http://127.0.0.1:11434");
            std::env::set_var("CRUSH_AI_OLLAMA_MODEL", "llava-test");
        }
        let config = Config::load(None).expect("default config loads");
        // SAFETY: restore before dropping the lock so other tests see defaults.
        unsafe {
            std::env::remove_var("CRUSH_AI_PROVIDER");
            std::env::remove_var("CRUSH_AI_OLLAMA_HOST");
            std::env::remove_var("CRUSH_AI_OLLAMA_MODEL");
        }
        drop(guard);
        assert_eq!(config.ai.provider, "ollama");
        assert_eq!(config.ai.ollama_host, "http://127.0.0.1:11434");
        assert_eq!(config.ai.ollama_model, "llava-test");
        // Non-overridden AI keys keep their defaults.
        assert!((config.ai.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.ai.max_tokens, 300);
    }
}
