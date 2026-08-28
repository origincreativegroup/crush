//! Local speech transcription and query-time shot alignment.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{ensure, Context};
use crush_store::{Shot, Store, TranscriptSegment, VideoStatus};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const SAMPLE_RATE: u32 = 16_000;
pub const LOW_MEMORY_THRESHOLD_BYTES: u64 = 12 * 1024 * 1024 * 1024;
/// Token-probability floor that rejects confident-looking music hallucinations in short clips.
pub const MIN_SEGMENT_CONFIDENCE: f64 = 0.78;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelChoice {
    Base,
    Small,
}

impl ModelChoice {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Base => "ggml-base.bin",
            Self::Small => "ggml-small.bin",
        }
    }
}

impl fmt::Display for ModelChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Base => "base",
            Self::Small => "small",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Metal,
    Cpu,
}

impl fmt::Display for Backend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Metal => "metal",
            Self::Cpu => "cpu",
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscribeOptions {
    pub threads: usize,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecognizedSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionReport {
    pub skipped_no_audio: bool,
    pub segment_count: usize,
    pub audio_s: f64,
    pub inference_ms: f64,
    pub model: ModelChoice,
    pub backend: Backend,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShotAlignment {
    pub shot: Shot,
    pub segments: Vec<TranscriptSegment>,
}

pub struct Transcriber {
    context: WhisperContext,
    options: TranscribeOptions,
    model: ModelChoice,
    backend: Backend,
}

impl Transcriber {
    pub fn new(
        model_path: impl AsRef<Path>,
        model: ModelChoice,
        options: TranscribeOptions,
    ) -> anyhow::Result<Self> {
        let model_path = model_path.as_ref();
        ensure!(
            model_path.is_file(),
            "required Whisper model is missing: {}",
            model_path.display()
        );
        let mut parameters = WhisperContextParameters::default();
        let backend = production_backend();
        parameters
            .use_gpu(backend == Backend::Metal)
            .flash_attn(backend == Backend::Metal);
        let context = WhisperContext::new_with_params(
            model_path
                .to_str()
                .context("Whisper model path is not valid UTF-8")?,
            parameters,
        )
        .context("failed to initialize Whisper")?;
        Ok(Self {
            context,
            options,
            model,
            backend,
        })
    }

    pub fn transcribe_wav(
        &self,
        wav_path: impl AsRef<Path>,
    ) -> anyhow::Result<(Vec<RecognizedSegment>, f64, f64)> {
        let (samples, audio_s) = read_wav(wav_path.as_ref())?;
        let mut state = self
            .context
            .create_state()
            .context("failed to create Whisper state")?;
        let mut parameters = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        parameters.set_n_threads(effective_threads(self.options.threads) as i32);
        parameters.set_language(self.options.language.as_deref());
        parameters.set_token_timestamps(true);
        parameters.set_print_special(false);
        parameters.set_print_progress(false);
        parameters.set_print_realtime(false);
        parameters.set_print_timestamps(false);

        let started = Instant::now();
        state
            .full(parameters, &samples)
            .context("Whisper inference failed")?;
        let inference_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let mut recognized = Vec::new();
        for segment in state.as_iter() {
            let text = segment.to_str_lossy()?.trim().to_owned();
            let start_s = segment.start_timestamp() as f64 / 100.0;
            let end_s = segment.end_timestamp() as f64 / 100.0;
            if text.is_empty() || end_s <= start_s {
                continue;
            }
            let probabilities = (0..segment.n_tokens())
                .filter_map(|index| segment.get_token(index))
                .map(|token| f64::from(token.token_probability()))
                .filter(|probability| probability.is_finite())
                .collect::<Vec<_>>();
            let confidence = (!probabilities.is_empty())
                .then(|| probabilities.iter().sum::<f64>() / probabilities.len() as f64);
            if !confidence.is_some_and(|confidence| confidence >= MIN_SEGMENT_CONFIDENCE) {
                tracing::debug!(
                    start_s,
                    end_s,
                    confidence,
                    text,
                    "discarding low-confidence Whisper segment"
                );
                continue;
            }
            recognized.push(RecognizedSegment {
                start_s,
                end_s,
                text,
                confidence,
            });
        }
        Ok((recognized, audio_s, inference_ms))
    }

    pub fn model(&self) -> ModelChoice {
        self.model
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }
}

/// Transcribe one stored video. Silent videos return before touching either input path.
pub fn transcribe_video(
    store: &mut Store,
    owner_id: &str,
    video_id: &str,
    wav_path: impl AsRef<Path>,
    model_path: impl AsRef<Path>,
    model: ModelChoice,
    options: TranscribeOptions,
) -> anyhow::Result<TranscriptionReport> {
    let video = store
        .video_by_id(owner_id, video_id)?
        .with_context(|| format!("video {video_id} was not found"))?;
    if !video.has_audio {
        store.replace_transcript_segments(owner_id, video_id, &[])?;
        store.set_video_status(owner_id, video_id, VideoStatus::Transcribed)?;
        return Ok(TranscriptionReport {
            skipped_no_audio: true,
            segment_count: 0,
            audio_s: 0.0,
            inference_ms: 0.0,
            model,
            backend: production_backend(),
        });
    }

    let transcriber = Transcriber::new(model_path, model, options)?;
    let (recognized, audio_s, inference_ms) = transcriber.transcribe_wav(wav_path)?;
    let segments = recognized
        .into_iter()
        .enumerate()
        .map(|(index, segment)| TranscriptSegment {
            id: format!("{video_id}-segment-{index:06}"),
            video_id: video_id.to_owned(),
            owner_id: owner_id.to_owned(),
            start_s: segment.start_s,
            end_s: segment.end_s,
            text: segment.text,
            confidence: segment.confidence,
        })
        .collect::<Vec<_>>();
    store.replace_transcript_segments(owner_id, video_id, &segments)?;
    store.set_video_status(owner_id, video_id, VideoStatus::Transcribed)?;
    Ok(TranscriptionReport {
        skipped_no_audio: false,
        segment_count: segments.len(),
        audio_s,
        inference_ms,
        model: transcriber.model(),
        backend: transcriber.backend(),
    })
}

pub fn align_video(
    store: &Store,
    owner_id: &str,
    video_id: &str,
) -> anyhow::Result<Vec<ShotAlignment>> {
    store
        .shots_for_video(owner_id, video_id)?
        .into_iter()
        .map(|shot| {
            let segments =
                store.segments_overlapping(owner_id, video_id, shot.start_s, shot.end_s)?;
            Ok(ShotAlignment { shot, segments })
        })
        .collect()
}

pub fn choose_model(
    configured: &str,
    total_memory_bytes: Option<u64>,
) -> anyhow::Result<ModelChoice> {
    match configured.trim().to_ascii_lowercase().as_str() {
        "auto" | "" => Ok(
            if total_memory_bytes.is_some_and(|bytes| bytes < LOW_MEMORY_THRESHOLD_BYTES) {
                ModelChoice::Base
            } else {
                ModelChoice::Small
            },
        ),
        "base" => Ok(ModelChoice::Base),
        "small" => Ok(ModelChoice::Small),
        other => anyhow::bail!("unsupported ASR model {other:?}; expected auto, base, or small"),
    }
}

pub fn model_path(models_dir: impl AsRef<Path>, model: ModelChoice) -> PathBuf {
    models_dir.as_ref().join(model.filename())
}

pub fn total_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).trim().parse().ok();
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kib = meminfo
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        kib.checked_mul(1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

pub fn production_backend() -> Backend {
    if cfg!(target_os = "macos") {
        Backend::Metal
    } else {
        Backend::Cpu
    }
}

fn effective_threads(configured: usize) -> usize {
    if configured > 0 {
        configured
    } else {
        std::thread::available_parallelism()
            .map(|count| count.get().saturating_sub(2).max(1))
            .unwrap_or(1)
    }
}

fn read_wav(path: &Path) -> anyhow::Result<(Vec<f32>, f64)> {
    ensure!(path.is_file(), "WAV does not exist: {}", path.display());
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open WAV {}", path.display()))?;
    let spec = reader.spec();
    ensure!(spec.channels == 1, "WAV must be mono");
    ensure!(spec.sample_rate == SAMPLE_RATE, "WAV must be 16 kHz");
    ensure!(
        spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16,
        "WAV must contain signed 16-bit PCM"
    );
    let integer = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?;
    let mut samples = vec![0.0_f32; integer.len()];
    whisper_rs::convert_integer_to_float_audio(&integer, &mut samples)?;
    let duration_s = samples.len() as f64 / f64::from(SAMPLE_RATE);
    Ok((samples, duration_s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crush_core::DEFAULT_OWNER_ID;
    use crush_store::Video;

    fn video(id: &str, has_audio: bool) -> Video {
        Video {
            id: id.to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            path: format!("/fixtures/{id}.mp4"),
            sha256: format!("sha-{id}"),
            duration_s: Some(10.0),
            fps: Some(30.0),
            width: Some(1920),
            height: Some(1080),
            has_audio,
            status: VideoStatus::Pending,
            indexed_at: None,
        }
    }

    #[test]
    fn auto_model_uses_twelve_gib_threshold_and_allows_overrides() {
        assert_eq!(
            choose_model("auto", Some(LOW_MEMORY_THRESHOLD_BYTES - 1)).unwrap(),
            ModelChoice::Base
        );
        assert_eq!(
            choose_model("auto", Some(LOW_MEMORY_THRESHOLD_BYTES)).unwrap(),
            ModelChoice::Small
        );
        assert_eq!(choose_model("base", None).unwrap(), ModelChoice::Base);
        assert_eq!(choose_model("small", Some(1)).unwrap(), ModelChoice::Small);
        assert!(choose_model("large", None).is_err());
    }

    #[test]
    fn silent_video_skips_wav_and_model_and_records_zero_segments() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(temp.path()).unwrap();
        store
            .upsert_video(DEFAULT_OWNER_ID, &video("silent", false))
            .unwrap();
        let report = transcribe_video(
            &mut store,
            DEFAULT_OWNER_ID,
            "silent",
            temp.path().join("does-not-exist.wav"),
            temp.path().join("does-not-exist.bin"),
            ModelChoice::Small,
            TranscribeOptions::default(),
        )
        .unwrap();
        assert!(report.skipped_no_audio);
        assert_eq!(report.segment_count, 0);
        assert_eq!(
            store
                .transcript_count_for_video(DEFAULT_OWNER_ID, "silent")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .video_by_id(DEFAULT_OWNER_ID, "silent")
                .unwrap()
                .unwrap()
                .status,
            VideoStatus::Transcribed
        );
    }
}
