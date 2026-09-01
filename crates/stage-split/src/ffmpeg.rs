//! Bundled FFmpeg/FFprobe resolution and the five supported video operations.

use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

const CANCEL_GRACE: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const EDIT_PROXY_FILTER: &str = "scale=w='min(1920,iw)':h='min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,format=yuv420p";
static BUNDLE_RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The exact edit-proxy encoding recipe, recorded in video source metadata_json so
/// derivatives are auditable and reproducible without reading stage code.
pub fn edit_proxy_recipe() -> serde_json::Value {
    serde_json::json!({
        "filter": EDIT_PROXY_FILTER,
        "max_dimensions_px": { "width": 1920, "height": 1080 },
        "video_encoder": "h264_videotoolbox",
        "video_bitrate": "12M",
        "video_maxrate": "16M",
        "video_bufsize": "24M",
        "audio_encoder": "aac",
        "audio_bitrate": "192k",
        "movflags": "+faststart",
        "color_policy": "pass_through_probed_source_tags_otherwise_explicit_bt709_no_tonemap",
    })
}

/// Explicit output color tags for the edit proxy. Probed source tags pass through unchanged;
/// when the source reports nothing, the proxy is explicitly tagged as SDR BT.709 instead of
/// inheriting encoder defaults silently.
fn edit_proxy_color_args(probe: &Probe) -> Vec<String> {
    let mut arguments = Vec::new();
    for (flag, value) in [
        ("-color_primaries", probe.color_primaries.as_deref()),
        ("-color_trc", probe.color_transfer.as_deref()),
        ("-colorspace", probe.color_space.as_deref()),
    ] {
        let passed_through = value
            .filter(|value| !value.trim().is_empty() && !value.eq_ignore_ascii_case("unknown"));
        arguments.push(flag.to_owned());
        arguments.push(
            passed_through
                .map(str::to_owned)
                .unwrap_or_else(|| "bt709".to_owned()),
        );
    }
    if let Some(range) = probe
        .color_range
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        arguments.push("-color_range".to_owned());
        arguments.push(range.to_owned());
    }
    arguments
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Bundled,
    DevSidecarDir,
    Path,
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Bundled => "bundled",
            Self::DevSidecarDir => "dev-sidecar-dir",
            Self::Path => "PATH",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub ffprobe_path: PathBuf,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Probe {
    pub duration_s: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub codec_profile: Option<String>,
    pub codec_tag: Option<String>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<u8>,
    pub color_space: Option<String>,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub color_range: Option<String>,
    pub rotation: Option<i32>,
    /// Frames reported by the video stream sample table. MP4/MOV outputs carry an exact
    /// count; a `None` on exotic containers means "count it another way", never "zero".
    pub video_frame_count: Option<i64>,
    /// Video stream duration, which container duration can hide when audio padding is longer.
    pub video_duration_s: Option<f64>,
    /// Audio stream duration, used to prove audio never outlasts video inside an item.
    pub audio_duration_s: Option<f64>,
    pub audio_sample_rate: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub out_time_s: f64,
    pub percent: f64,
}

/// Container/muxer slack shared by every rendered-container duration check. The native AAC
/// encoder's priming packet is presented through the MP4 edit list and can pad the container
/// duration slightly past the video stream (the TASK-021/036 AAC-priming finding), so the
/// duration rule below adds this slack on top of the frame-boundary term.
pub const DURATION_TOLERANCE_SLACK_S: f64 = 0.05;

/// One source-frame period of boundary slack, the frame term of the duration-tolerance rule.
/// A container whose video starts on the first source frame at or after the requested start
/// and ends on the last delivered frame can differ from the requested duration by up to one
/// source-frame period. Probes that report no usable frame rate fall back to 30 fps.
pub fn frame_tolerance_s(fps: f64) -> f64 {
    if fps > 0.0 {
        1.0 / fps
    } else {
        1.0 / 30.0
    }
}

/// The documented duration-tolerance rule shared by every rendered-container duration check —
/// stage-split verification (`verify_clip_render`, the export path) and the durable executor's
/// re-checks in `crates/pipeline/src/render.rs`:
///
/// `duration_tolerance_s = frame_tolerance + 0.05`, where `frame_tolerance = 1.0 / fps`
/// (fallback 1/30).
///
/// `frame_tolerance` covers frame-boundary rounding; the extra 0.05 s
/// ([`DURATION_TOLERANCE_SLACK_S`]) covers container padding from the AAC priming packet.
/// Without one shared rule, a 60 fps render could pass the encoder-side check
/// (1/60 + 0.05 ≈ 0.067 s) and then fail a stricter executor re-check (0.05 s) — the
/// pass-then-fail window this function removes. A reel of N items renders N independent
/// frame-boundary cuts, so the reel executor sums per-item frame slacks plus the shared
/// container slack: `DURATION_TOLERANCE_SLACK_S + N / fps`. Every duration check in the render
/// path must derive from this rule; do not add a second tolerance formula.
pub fn duration_tolerance_s(fps: f64) -> f64 {
    frame_tolerance_s(fps) + DURATION_TOLERANCE_SLACK_S
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation<T> {
    pub value: T,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    StreamCopy,
    VideoToolboxReencode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportResult {
    pub command: String,
    pub attempted_commands: Vec<String>,
    pub mode: ExportMode,
}

/// Normalized crop in the displayed, auto-rotated source frame. Bounds are quantized outward to
/// even chroma-aligned pixels because the v1 H.264 SDR presets use yuv420p.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedVideoCrop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasicVideoGrade {
    pub exposure_ev: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoGrade {
    None,
    Basic(BasicVideoGrade),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipTransition {
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipAudio {
    Source,
    Mute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOutputPreset {
    Mp4H264SdrV1,
    MovH264SdrV1,
}

impl ClipOutputPreset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mp4H264SdrV1 => "mp4-h264-sdr-v1",
            Self::MovH264SdrV1 => "mov-h264-sdr-v1",
        }
    }

    const fn muxer(self) -> &'static str {
        match self {
            Self::Mp4H264SdrV1 => "mp4",
            Self::MovH264SdrV1 => "mov",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRenderRequest {
    pub in_s: f64,
    pub out_s: f64,
    pub crop: Option<NormalizedVideoCrop>,
    pub grade: VideoGrade,
    pub transition: ClipTransition,
    pub audio: ClipAudio,
    pub output: ClipOutputPreset,
}

impl ClipRenderRequest {
    pub fn validate(&self) -> Result<()> {
        if !self.in_s.is_finite()
            || !self.out_s.is_finite()
            || self.in_s < 0.0
            || self.out_s <= self.in_s
        {
            return Err(Error::InvalidArgument(
                "clip range must be finite with 0 <= in_s < out_s".to_owned(),
            ));
        }
        if let Some(crop) = self.crop {
            let values = [crop.x, crop.y, crop.width, crop.height];
            if !values.iter().all(|value| value.is_finite()) {
                return Err(Error::InvalidArgument(
                    "normalized clip crop values must be finite".to_owned(),
                ));
            }
            if crop.x < 0.0 || crop.y < 0.0 || crop.width <= 0.0 || crop.height <= 0.0 {
                return Err(Error::InvalidArgument(
                    "normalized clip crop needs a non-negative origin and positive dimensions"
                        .to_owned(),
                ));
            }
            if crop.x + crop.width > 1.0 || crop.y + crop.height > 1.0 {
                return Err(Error::InvalidArgument(
                    "normalized clip crop must stay inside source bounds".to_owned(),
                ));
            }
        }
        if let VideoGrade::Basic(grade) = self.grade {
            if ![
                grade.exposure_ev,
                grade.contrast,
                grade.saturation,
                grade.temperature,
                grade.tint,
            ]
            .iter()
            .all(|value| value.is_finite())
            {
                return Err(Error::InvalidArgument(
                    "basic clip grade values must be finite".to_owned(),
                ));
            }
            if !(-5.0..=5.0).contains(&grade.exposure_ev) {
                return Err(Error::InvalidArgument(
                    "clip grade exposure_ev must be between -5 and 5".to_owned(),
                ));
            }
            if !(-1.0..=1.0).contains(&grade.contrast) {
                return Err(Error::InvalidArgument(
                    "clip grade contrast must be between -1 and 1".to_owned(),
                ));
            }
            if !(0.0..=2.0).contains(&grade.saturation) {
                return Err(Error::InvalidArgument(
                    "clip grade saturation must be between 0 and 2".to_owned(),
                ));
            }
            if !(-1.0..=1.0).contains(&grade.temperature) || !(-1.0..=1.0).contains(&grade.tint) {
                return Err(Error::InvalidArgument(
                    "clip grade temperature and tint must be between -1 and 1".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipRenderBackend {
    VideoToolbox,
}

impl ClipRenderBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VideoToolbox => "videotoolbox",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipRenderResult {
    pub command: String,
    pub backend: ClipRenderBackend,
    pub encoder: &'static str,
    pub preset: &'static str,
    pub requested_duration_s: f64,
    pub source_color_handling: String,
    pub output_probe: Probe,
    pub probe_command: String,
}

/// Frame-exact render plan for one ordered-reel item (TASK-036). The frame math is owned by
/// the reel renderer; this carries the numbers the FFmpeg command needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReelItemRenderSpec {
    /// Requested source in point.
    pub in_s: f64,
    /// Requested source out point.
    pub out_s: f64,
    /// Input-side seek target: `in_s` minus a microsecond-scale boundary epsilon.
    pub seek_s: f64,
    /// Input-side read window: the item's video duration plus decode slack, so the exact
    /// frame count is available without reading the source to EOF.
    pub read_s: f64,
    /// Exact output frame count: `round((out_s - in_s) * fps)`.
    pub frame_count: i64,
    /// The item's exact video duration: `frame_count / fps`. Also the audio trim end.
    pub video_duration_s: f64,
    pub crop: Option<NormalizedVideoCrop>,
    pub grade: VideoGrade,
    pub audio: ClipAudio,
    pub output: ClipOutputPreset,
}

impl ReelItemRenderSpec {
    /// The clip request is only used to share the crop/grade filter-chain builder; the
    /// boundary handling is the reel item's own frame-exact contract above.
    fn as_clip_request(&self) -> ClipRenderRequest {
        ClipRenderRequest {
            in_s: self.in_s,
            out_s: self.out_s,
            crop: self.crop,
            grade: self.grade,
            transition: ClipTransition::Cut,
            audio: self.audio,
            output: self.output,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReelItemRenderResult {
    pub command: String,
    pub expected_width: u32,
    pub expected_height: u32,
    pub source_color_handling: String,
    pub output_probe: Probe,
    pub probe_command: String,
}

pub use crush_core::cancellation::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ffmpeg and ffprobe were not found in the app bundle or development sidecars")]
    NotFound,
    #[error("invalid ffmpeg argument: {0}")]
    InvalidArgument(String),
    #[error("command failed ({status}): {command}\n{stderr}")]
    CommandFailed {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("command was cancelled: {command}")]
    Cancelled { command: String },
    #[error("ffprobe output did not describe a playable media file: {0}")]
    InvalidProbe(String),
    #[error("required media capability is unavailable: {capability}: {reason}")]
    CapabilityUnavailable { capability: String, reason: String },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Register Tauri's bundle resource directory before resolving production sidecars.
pub fn register_bundle_resource_dir(directory: PathBuf) -> Result<()> {
    if let Some(current) = BUNDLE_RESOURCE_DIR.get() {
        if current == &directory {
            return Ok(());
        }
        return Err(Error::InvalidArgument(format!(
            "bundle resource directory is already registered as {}",
            current.display()
        )));
    }
    let _ = BUNDLE_RESOURCE_DIR.set(directory);
    Ok(())
}

/// Resolve an FFmpeg/FFprobe pair in production-safe order.
pub fn resolve() -> Result<Resolved> {
    if let Some(resource_dir) = BUNDLE_RESOURCE_DIR.get() {
        let macos_dir = resource_dir.parent().map(|contents| contents.join("MacOS"));
        for directory in [Some(resource_dir.as_path()), macos_dir.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(resolved) = resolve_pair(directory, Source::Bundled) {
                return Ok(resolved);
            }
        }
    }
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let development_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("sidecars");
    resolve_with(
        executable_dir.as_deref(),
        Some(&development_dir),
        std::env::var_os("PATH").as_deref(),
        cfg!(debug_assertions),
    )
}

fn resolve_with(
    executable_dir: Option<&Path>,
    development_dir: Option<&Path>,
    path: Option<&OsStr>,
    allow_path: bool,
) -> Result<Resolved> {
    if let Some(directory) = executable_dir {
        if let Some(resolved) = resolve_pair(directory, Source::Bundled) {
            return Ok(resolved);
        }
    }
    if let Some(directory) = development_dir {
        if let Some(resolved) = resolve_pair(directory, Source::DevSidecarDir) {
            return Ok(resolved);
        }
    }
    if allow_path {
        if let Some(path) = path {
            for directory in std::env::split_paths(path) {
                if let Some(resolved) = resolve_pair(&directory, Source::Path) {
                    tracing::warn!(
                        job_id = "resolver",
                        stage = "ffmpeg",
                        ffmpeg = %resolved.path.display(),
                        "using FFmpeg from PATH; production builds never allow this fallback"
                    );
                    return Ok(resolved);
                }
            }
        }
    }
    Err(Error::NotFound)
}

fn resolve_pair(directory: &Path, source: Source) -> Option<Resolved> {
    let suffixes: &[&str] = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        &["", "-aarch64-apple-darwin"]
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        &["", "-x86_64-apple-darwin"]
    } else {
        &[""]
    };
    suffixes.iter().find_map(|suffix| {
        let ffmpeg = directory.join(format!("ffmpeg{suffix}"));
        let ffprobe = directory.join(format!("ffprobe{suffix}"));
        if is_executable(&ffmpeg) && is_executable(&ffprobe) {
            Some(Resolved {
                path: absolute_path(ffmpeg),
                ffprobe_path: absolute_path(ffprobe),
                source,
            })
        } else {
            None
        }
    })
}

fn absolute_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Debug, Clone)]
pub struct Runner {
    resolved: Resolved,
    threads: usize,
    job_id: String,
    debug_dir: Option<PathBuf>,
}

impl Runner {
    pub fn new(resolved: Resolved, configured_threads: usize, job_id: impl Into<String>) -> Self {
        Self {
            resolved,
            threads: effective_threads(configured_threads),
            job_id: job_id.into(),
            debug_dir: None,
        }
    }

    pub fn with_debug_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.debug_dir = Some(directory.into());
        self
    }

    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    pub fn version(&self) -> Result<Operation<String>> {
        let spec = CommandSpec::new(&self.resolved.path).arg("-version");
        let output = self.run_capture(&spec, false)?;
        let version = output
            .stdout
            .lines()
            .next()
            .ok_or_else(|| Error::InvalidProbe("ffmpeg -version returned no text".into()))?
            .to_owned();
        Ok(Operation {
            value: version,
            command: output.command,
        })
    }

    /// Probe media metadata through FFprobe JSON, never stderr parsing.
    pub fn probe(&self, input: &Path) -> Result<Operation<Probe>> {
        let spec = CommandSpec::new(&self.resolved.ffprobe_path)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_streams",
                "-show_format",
            ])
            .arg(input);
        let output = self.run_capture(&spec, false)?;
        let document: ProbeDocument = serde_json::from_str(&output.stdout)?;
        Ok(Operation {
            value: document.into_probe()?,
            command: output.command,
        })
    }

    /// Sample downscaled JPEGs at the requested rate.
    pub fn sample_frames(
        &self,
        input: &Path,
        fps: f64,
        output_dir: &Path,
    ) -> Result<Operation<usize>> {
        let cancellation = CancellationToken::default();
        self.sample_frames_with_control(input, fps, output_dir, &cancellation, |_| {})
    }

    pub fn sample_frames_with_control<F>(
        &self,
        input: &Path,
        fps: f64,
        output_dir: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<Operation<usize>>
    where
        F: FnMut(Progress),
    {
        if !fps.is_finite() || fps <= 0.0 {
            return Err(Error::InvalidArgument(
                "fps must be finite and positive".into(),
            ));
        }
        fs::create_dir_all(output_dir)?;
        remove_sampled_frames(output_dir)?;
        let duration = self.probe(input)?.value.duration_s;
        let pattern = output_dir.join("f%06d.jpg");
        let filter = format!("fps={},scale=-2:480", format_number(fps));
        let spec = CommandSpec::new(&self.resolved.path)
            .args(["-y", "-threads"])
            .arg(self.threads.to_string())
            .arg("-i")
            .arg(input)
            .args(["-vf", &filter, "-q:v", "3", "-threads"])
            .arg(self.threads.to_string())
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(&pattern);
        let command = self.run_progress(&spec, duration, cancellation, &mut progress)?;
        let count = fs::read_dir(output_dir)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| is_sampled_frame_name(&entry.file_name()))
            .count();
        Ok(Operation {
            value: count,
            command,
        })
    }

    /// Generate a lightweight edit proxy for acquisition codecs that are expensive to seek.
    /// The LGPL sidecar intentionally uses Apple's native H.264 encoder instead of libx264.
    /// Output color tags are explicit: probed source tags pass through, otherwise the proxy
    /// is tagged SDR BT.709. Tonemapping is deliberately not applied; the recorded recipe
    /// (`edit_proxy_recipe`) states this decision so derivatives stay auditable.
    pub fn generate_edit_proxy_with_control<F>(
        &self,
        input: &Path,
        output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<Operation<()>>
    where
        F: FnMut(Progress),
    {
        ensure_parent(output)?;
        let probed = self.probe(input)?.value;
        let duration = probed.duration_s;
        let color_args = edit_proxy_color_args(&probed);
        let file_name = output
            .file_name()
            .ok_or_else(|| Error::InvalidArgument("proxy output needs a file name".into()))?
            .to_string_lossy();
        let temporary = output.with_file_name(format!(".{file_name}.partial.mp4"));
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        let spec = CommandSpec::new(&self.resolved.path)
            .args(["-y", "-threads"])
            .arg(self.threads.to_string())
            .arg("-i")
            .arg(input)
            .args(["-map", "0:v:0", "-map", "0:a?", "-vf", EDIT_PROXY_FILTER])
            .args(&color_args)
            .args([
                "-c:v",
                "h264_videotoolbox",
                "-b:v",
                "12M",
                "-maxrate",
                "16M",
                "-bufsize",
                "24M",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
            ])
            .arg(&temporary);
        let command = match self.run_progress(&spec, duration, cancellation, &mut progress) {
            Ok(command) => command,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        fs::rename(&temporary, output)?;
        Ok(Operation { value: (), command })
    }

    /// Extract 16 kHz mono signed-16-bit PCM for Whisper.
    pub fn extract_audio(&self, input: &Path, output_wav: &Path) -> Result<Operation<()>> {
        let cancellation = CancellationToken::default();
        self.extract_audio_with_control(input, output_wav, &cancellation, |_| {})
    }

    pub fn extract_audio_with_control<F>(
        &self,
        input: &Path,
        output_wav: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<Operation<()>>
    where
        F: FnMut(Progress),
    {
        ensure_parent(output_wav)?;
        let duration = self.probe(input)?.value.duration_s;
        let spec = CommandSpec::new(&self.resolved.path)
            .args(["-y", "-threads"])
            .arg(self.threads.to_string())
            .arg("-i")
            .arg(input)
            .args([
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
                "-threads",
            ])
            .arg(self.threads.to_string())
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(output_wav);
        let command = self.run_progress(&spec, duration, cancellation, &mut progress)?;
        Ok(Operation { value: (), command })
    }

    /// Extract one JPEG with input-side seeking for speed.
    pub fn frame_at(&self, input: &Path, time_s: f64, output_jpg: &Path) -> Result<Operation<()>> {
        self.frame_at_with_control(input, time_s, output_jpg, &CancellationToken::default())
    }

    pub fn frame_at_with_control(
        &self,
        input: &Path,
        time_s: f64,
        output_jpg: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Operation<()>> {
        if !time_s.is_finite() || time_s < 0.0 {
            return Err(Error::InvalidArgument(
                "frame time must be finite and non-negative".into(),
            ));
        }
        ensure_parent(output_jpg)?;
        let spec = CommandSpec::new(&self.resolved.path)
            .args(["-y", "-threads"])
            .arg(self.threads.to_string())
            .args(["-ss", &format_number(time_s), "-i"])
            .arg(input)
            .args(["-frames:v", "1", "-update", "1", "-q:v", "2", "-threads"])
            .arg(self.threads.to_string())
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(output_jpg);
        let mut ignore_progress = |_| {};
        let command = self.run_progress(&spec, 1.0, cancellation, &mut ignore_progress)?;
        Ok(Operation { value: (), command })
    }

    /// Render one boundary-sensitive clip recipe to a caller-owned private staging path. Unlike
    /// `export_clip`, this contract always encodes: crop, grade, audio, and exact boundaries are
    /// recipe intent and can never be silently reduced to stream copy.
    pub fn render_clip(
        &self,
        input: &Path,
        request: &ClipRenderRequest,
        staging_output: &Path,
    ) -> Result<ClipRenderResult> {
        self.render_clip_with_control(
            input,
            request,
            staging_output,
            &CancellationToken::default(),
            |_| {},
        )
    }

    pub fn render_clip_with_control<F>(
        &self,
        input: &Path,
        request: &ClipRenderRequest,
        staging_output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<ClipRenderResult>
    where
        F: FnMut(Progress),
    {
        request.validate()?;
        reject_existing_destination(staging_output, "clip staging destination")?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled {
                command: "clip render before capability checks".to_owned(),
            });
        }
        self.require_ffmpeg_component("-encoders", "h264_videotoolbox", "video encoder")?;

        let source_operation = self.probe(input)?;
        let source_probe = source_operation.value;
        if source_probe.width == 0 || source_probe.height == 0 {
            return Err(Error::InvalidProbe(
                "clip render source has no video stream".to_owned(),
            ));
        }
        let frame_tolerance = frame_tolerance_s(source_probe.fps);
        if request.out_s > source_probe.duration_s + frame_tolerance {
            return Err(Error::InvalidArgument(format!(
                "clip out_s {:.6} exceeds source duration {:.6}",
                request.out_s, source_probe.duration_s
            )));
        }
        let source_color_handling = validate_h264_sdr_source(&source_probe)?;
        if request.audio == ClipAudio::Source && source_probe.has_audio {
            self.require_ffmpeg_component("-encoders", "aac", "audio encoder")?;
        }
        for filter in required_clip_filters(request) {
            self.require_ffmpeg_component("-filters", filter, "video filter")?;
        }
        let (filter_chain, expected_width, expected_height) =
            clip_filter_chain(&source_probe, request)?;

        let parent = staging_output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let private_render = tempfile::Builder::new()
            .prefix(".crush-clip-render-")
            .tempdir_in(parent)?;
        let rendered = private_render
            .path()
            .join(format!("rendered.{}", request.output.muxer()));
        let expected_duration = request.out_s - request.in_s;
        let mut spec = CommandSpec::new(&self.resolved.path)
            .args(["-n", "-threads"])
            .arg(self.threads.to_string())
            .arg("-i")
            .arg(input)
            .args([
                "-ss",
                &format_number(request.in_s),
                "-t",
                &format_number(expected_duration),
                "-map",
                "0:v:0",
            ]);
        if request.audio == ClipAudio::Source {
            spec = spec.args(["-map", "0:a?"]);
        }
        spec = spec
            .args(["-vf", &filter_chain, "-c:v", "h264_videotoolbox"])
            .args([
                "-allow_sw",
                "1",
                "-b:v",
                "8M",
                "-pix_fmt",
                "yuv420p",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
                "-color_range",
                "tv",
                "-tag:v",
                "avc1",
            ]);
        spec = match request.audio {
            ClipAudio::Source => spec.args(["-c:a", "aac", "-b:a", "192k"]),
            ClipAudio::Mute => spec.arg("-an"),
        };
        spec = spec
            .args([
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
                "-movflags",
                "+faststart",
                "-avoid_negative_ts",
                "make_zero",
                "-f",
                request.output.muxer(),
                "-progress",
                "pipe:1",
                "-nostats",
            ])
            .arg(&rendered);

        let command = self
            .run_progress(&spec, expected_duration, cancellation, &mut progress)
            .map_err(|error| classify_clip_render_error(error, request.audio))?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled { command });
        }
        let measured = self.probe(&rendered)?;
        verify_clip_render(
            &measured.value,
            request,
            &source_probe,
            expected_width,
            expected_height,
            expected_duration,
            duration_tolerance_s(source_probe.fps),
        )?;
        fs::File::open(&rendered)?.sync_all()?;
        // The caller path is published exclusively. A racing writer wins without being replaced;
        // the private render disappears when its temporary directory drops.
        fs::hard_link(&rendered, staging_output)?;
        Ok(ClipRenderResult {
            command,
            backend: ClipRenderBackend::VideoToolbox,
            encoder: "h264_videotoolbox",
            preset: request.output.as_str(),
            requested_duration_s: expected_duration,
            source_color_handling,
            output_probe: measured.value,
            probe_command: measured.command,
        })
    }

    /// Render one ordered-reel item with frame-exact boundaries (TASK-036).
    ///
    /// This is deliberately NOT the clip path: the approved single-clip renderer keeps its
    /// own command shape, while reel items must pin an exact output frame count and a
    /// zero-based video timeline so the concat assembly lands cuts on frame boundaries.
    ///
    /// Frame contract, computed by the caller from the source probe:
    /// - input-side `-ss` lands on the first source frame at or after `in_s` (the caller
    ///   subtracts a microsecond-scale epsilon so an `in_s` exactly on a frame boundary is
    ///   never rounded past that frame by FFmpeg's microsecond seek parsing);
    /// - `-frames:v` delivers exactly `frame_count` frames from that first frame, which is
    ///   `round((out_s - in_s) * fps)` frames — the requested content, no more, no fewer;
    /// - `setpts=PTS-STARTPTS` starts the item's video at zero with no lead dead zone;
    /// - audio is trimmed to `video_duration_s` (the item's exact video length) and then
    ///   silence-padded to exactly that duration (`atrim` + `apad`), so item audio is never
    ///   longer AND never shorter than its video: a source whose audio track ends early
    ///   inside the item interval is ordinary real-world media, and the silence fill is the
    ///   editorially expected behavior — but leaving the shortfall would shift every later
    ///   item's audio early at the concat join (progressive A/V desync). The native AAC
    ///   encoder's priming packet stays
    ///   at a negative raw timestamp and is presented from zero through the MP4 edit list;
    ///   `-avoid_negative_ts make_zero` is intentionally NOT used because shifting the whole
    ///   item by the audio priming is what created the reel's head dead zone and cut drift.
    pub(crate) fn render_reel_item_with_control<F>(
        &self,
        input: &Path,
        spec: &ReelItemRenderSpec,
        staging_output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<ReelItemRenderResult>
    where
        F: FnMut(Progress),
    {
        reject_existing_destination(staging_output, "reel item staging destination")?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled {
                command: "reel item render before capability checks".to_owned(),
            });
        }
        self.require_ffmpeg_component("-encoders", "h264_videotoolbox", "video encoder")?;

        let source_operation = self.probe(input)?;
        let source_probe = source_operation.value;
        if source_probe.width == 0 || source_probe.height == 0 {
            return Err(Error::InvalidProbe(
                "reel item source has no video stream".to_owned(),
            ));
        }
        if source_probe.fps <= 0.0 {
            return Err(Error::InvalidProbe(
                "reel item source reports no frame rate; boundary frame math is impossible"
                    .to_owned(),
            ));
        }
        let frame_tolerance = frame_tolerance_s(source_probe.fps);
        if spec.out_s > source_probe.duration_s + frame_tolerance {
            return Err(Error::InvalidArgument(format!(
                "reel item out_s {:.6} exceeds source duration {:.6}",
                spec.out_s, source_probe.duration_s
            )));
        }
        let source_color_handling = validate_h264_sdr_source(&source_probe)?;
        if spec.audio == ClipAudio::Source && source_probe.has_audio {
            self.require_ffmpeg_component("-encoders", "aac", "audio encoder")?;
        }
        for filter in required_clip_filters(&spec.as_clip_request()) {
            self.require_ffmpeg_component("-filters", filter, "video filter")?;
        }
        let (filter_chain, expected_width, expected_height) =
            clip_filter_chain(&source_probe, &spec.as_clip_request())?;

        let parent = staging_output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let private_render = tempfile::Builder::new()
            .prefix(".crush-reel-item-")
            .tempdir_in(parent)?;
        let rendered = private_render
            .path()
            .join(format!("rendered.{}", spec.output.muxer()));

        let mut spec_command = CommandSpec::new(&self.resolved.path)
            .args(["-n", "-threads"])
            .arg(self.threads.to_string())
            .args([
                "-ss",
                &format_number(spec.seek_s),
                "-t",
                &format_number(spec.read_s),
            ])
            .arg("-i")
            .arg(input)
            .args(["-map", "0:v:0"]);
        if spec.audio == ClipAudio::Source {
            spec_command = spec_command.args(["-map", "0:a?"]);
        }
        spec_command = spec_command.args([
            "-vf",
            &format!("{filter_chain},setpts=PTS-STARTPTS"),
            "-frames:v",
            &spec.frame_count.to_string(),
        ]);
        if spec.audio == ClipAudio::Source {
            // Trim to the exact video duration, then silence-pad to that same duration:
            // `apad` is a no-op for full-length audio but fills a source track that ends
            // early inside the item interval, so the item's audio equals its video exactly.
            spec_command = spec_command.args([
                "-af",
                &format!(
                    "asetpts=PTS-STARTPTS,atrim=end={},apad=whole_dur={}",
                    format_number(spec.video_duration_s),
                    format_number(spec.video_duration_s)
                ),
            ]);
        }
        spec_command = spec_command.args(["-c:v", "h264_videotoolbox"]).args([
            "-allow_sw",
            "1",
            "-b:v",
            "8M",
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-tag:v",
            "avc1",
        ]);
        spec_command = match spec.audio {
            ClipAudio::Source => spec_command.args(["-c:a", "aac", "-b:a", "192k"]),
            ClipAudio::Mute => spec_command.arg("-an"),
        };
        spec_command = spec_command
            .args([
                "-map_metadata",
                "-1",
                "-map_chapters",
                "-1",
                "-movflags",
                "+faststart",
                "-f",
                spec.output.muxer(),
                "-progress",
                "pipe:1",
                "-nostats",
            ])
            .arg(&rendered);

        let command = self
            .run_progress(
                &spec_command,
                spec.video_duration_s,
                cancellation,
                &mut progress,
            )
            .map_err(|error| classify_clip_render_error(error, spec.audio))?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled { command });
        }
        let measured = self.probe(&rendered)?;
        fs::File::open(&rendered)?.sync_all()?;
        // The caller path is published exclusively, exactly like the clip renderer: a racing
        // writer wins without being replaced, and the private render disappears with its
        // temporary directory.
        fs::hard_link(&rendered, staging_output)?;
        Ok(ReelItemRenderResult {
            command,
            expected_width,
            expected_height,
            source_color_handling,
            output_probe: measured.value,
            probe_command: measured.command,
        })
    }

    /// Stream-copy the video track of a rendered reel item into a video-only intermediate.
    ///
    /// The concat demuxer offsets each file by its container duration and normalizes packet
    /// timestamps by the file's most negative start; an item that still carries its AAC
    /// priming packet at a negative raw timestamp would push the whole reel's video late by
    /// that priming. A video-only copy has a zero-based timeline and a container duration
    /// exactly equal to its video stream, so concat offsets land on frame boundaries.
    pub(crate) fn remux_video_only_with_control<F>(
        &self,
        input: &Path,
        output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<String>
    where
        F: FnMut(Progress),
    {
        reject_existing_destination(output, "reel video-only staging destination")?;
        let probed = self.probe(input)?;
        let duration = probed
            .value
            .video_duration_s
            .unwrap_or(probed.value.duration_s);
        let mut spec = CommandSpec::new(&self.resolved.path)
            .args(["-n", "-threads"])
            .arg(self.threads.to_string())
            .arg("-i")
            .arg(input)
            .args(["-map", "0:v:0", "-c", "copy", "-an"]);
        spec = spec
            .args(["-map_metadata", "-1", "-map_chapters", "-1"])
            .args(["-movflags", "+faststart", "-f", "mp4"])
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(output);
        self.run_progress(&spec, duration, cancellation, &mut progress)
    }

    fn require_ffmpeg_component(
        &self,
        listing_flag: &str,
        component: &str,
        capability_kind: &str,
    ) -> Result<()> {
        let spec = CommandSpec::new(&self.resolved.path).args(["-hide_banner", listing_flag]);
        let output =
            self.run_capture(&spec, false)
                .map_err(|error| Error::CapabilityUnavailable {
                    capability: format!("{capability_kind} {component}"),
                    reason: format!("could not query bundled FFmpeg: {error}"),
                })?;
        if component_listing_contains(&output.stdout, component) {
            Ok(())
        } else {
            Err(Error::CapabilityUnavailable {
                capability: format!("{capability_kind} {component}"),
                reason: "the bundled FFmpeg build does not advertise it".to_owned(),
            })
        }
    }

    /// Export a clip, preferring stream copy and falling back to the LGPL VideoToolbox encoder.
    pub fn export_clip(
        &self,
        input: &Path,
        start_s: f64,
        end_s: f64,
        output: &Path,
    ) -> Result<ExportResult> {
        let cancellation = CancellationToken::default();
        self.export_clip_with_control(input, start_s, end_s, output, &cancellation, |_| {})
    }

    pub fn export_clip_with_control<F>(
        &self,
        input: &Path,
        start_s: f64,
        end_s: f64,
        output: &Path,
        cancellation: &CancellationToken,
        progress: F,
    ) -> Result<ExportResult>
    where
        F: FnMut(Progress),
    {
        // Never hand a caller-selected destination to FFmpeg's overwrite flag. Existing
        // files (including source aliases and dangling symlinks) are rejected up front;
        // exclusive publication below also closes the destination-creation race.
        match fs::symlink_metadata(output) {
            Ok(_) => {
                return Err(Error::InvalidArgument(format!(
                    "export destination already exists; choose a new filename: {}",
                    output.display()
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled {
                command: "clip export before staging".to_owned(),
            });
        }
        let filename = output.file_name().ok_or_else(|| {
            Error::InvalidArgument("export destination needs a filename".to_owned())
        })?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".crush-export-")
            .tempdir_in(parent)?;
        let staged_output = staging.path().join(filename);
        let result = self.export_clip_staged(
            input,
            start_s,
            end_s,
            &staged_output,
            cancellation,
            progress,
        )?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled {
                command: result.command,
            });
        }
        fs::File::open(&staged_output)?.sync_all()?;
        // Staging is on the same filesystem. hard_link creates the destination atomically
        // and fails if anything already occupies it; never fall back to an overwriting copy.
        fs::hard_link(&staged_output, output)?;
        tracing::info!(job_id = %self.job_id, stage = "render", output = %output.display(), "verified clip published without overwrite");
        Ok(result)
    }

    fn export_clip_staged<F>(
        &self,
        input: &Path,
        start_s: f64,
        end_s: f64,
        output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<ExportResult>
    where
        F: FnMut(Progress),
    {
        if !start_s.is_finite() || !end_s.is_finite() || start_s < 0.0 || end_s <= start_s {
            return Err(Error::InvalidArgument(
                "clip range must be finite with 0 <= start < end".into(),
            ));
        }
        ensure_parent(output)?;
        let expected_duration = end_s - start_s;
        let source_probe = self.probe(input)?.value;
        let copy = CommandSpec::new(&self.resolved.path)
            .args([
                "-y",
                "-ss",
                &format_number(start_s),
                "-to",
                &format_number(end_s),
                "-i",
            ])
            .arg(input)
            .args(["-map", "0", "-c", "copy", "-progress", "pipe:1", "-nostats"])
            .arg(output);
        let copy_command =
            match self.run_progress(&copy, expected_duration, cancellation, &mut progress) {
                Ok(command) => command,
                Err(error @ Error::Cancelled { .. }) => {
                    self.remove_if_corrupt(output);
                    return Err(error);
                }
                Err(error) => {
                    tracing::warn!(
                        job_id = %self.job_id,
                        stage = "ffmpeg",
                        error = %error,
                        "stream-copy export failed; retrying with VideoToolbox"
                    );
                    copy.render(true)
                }
            };

        let duration_tolerance = duration_tolerance_s(source_probe.fps);
        if self.copy_is_accurate(
            input,
            start_s,
            output,
            expected_duration,
            duration_tolerance,
        ) {
            return Ok(ExportResult {
                command: copy_command.clone(),
                attempted_commands: vec![copy_command],
                mode: ExportMode::StreamCopy,
            });
        }

        tracing::warn!(
            job_id = %self.job_id,
            stage = "ffmpeg",
            "stream-copy output did not start within one frame; retrying with VideoToolbox"
        );
        let _ = fs::remove_file(output);
        let reencode = CommandSpec::new(&self.resolved.path)
            .args(["-y", "-threads"])
            .arg(self.threads.to_string())
            .args([
                "-ss",
                &format_number(start_s),
                "-to",
                &format_number(end_s),
                "-i",
            ])
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                "-c:v",
                "h264_videotoolbox",
                "-allow_sw",
                "1",
                "-b:v",
                "8M",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-threads",
            ])
            .arg(self.threads.to_string())
            .args(["-progress", "pipe:1", "-nostats"])
            .arg(output);
        let reencode_command =
            match self.run_progress(&reencode, expected_duration, cancellation, &mut progress) {
                Ok(command) => command,
                Err(error @ Error::Cancelled { .. }) => {
                    self.remove_if_corrupt(output);
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
        let output_probe = self.probe(output)?.value;
        if (output_probe.duration_s - expected_duration).abs() > duration_tolerance {
            return Err(Error::InvalidProbe(format!(
                "export duration {:.6}s differs from requested {:.6}s",
                output_probe.duration_s, expected_duration
            )));
        }
        Ok(ExportResult {
            command: reencode_command.clone(),
            attempted_commands: vec![copy_command, reencode_command],
            mode: ExportMode::VideoToolboxReencode,
        })
    }

    fn copy_is_accurate(
        &self,
        input: &Path,
        start_s: f64,
        output: &Path,
        expected_duration: f64,
        duration_tolerance: f64,
    ) -> bool {
        let Ok(probe) = self.probe(output) else {
            return false;
        };
        if (probe.value.duration_s - expected_duration).abs() > duration_tolerance {
            return false;
        }
        match (self.frame_md5(input, start_s), self.frame_md5(output, 0.0)) {
            (Ok(source), Ok(exported)) => source == exported,
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(
                    job_id = %self.job_id,
                    stage = "ffmpeg",
                    error = %error,
                    "could not verify stream-copy first frame"
                );
                false
            }
        }
    }

    fn frame_md5(&self, input: &Path, time_s: f64) -> Result<String> {
        let spec = CommandSpec::new(&self.resolved.path)
            .arg("-threads")
            .arg(self.threads.to_string())
            .args(["-ss", &format_number(time_s), "-i"])
            .arg(input)
            .args([
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-f",
                "framemd5",
                "pipe:1",
            ]);
        let output = self.run_capture(&spec, true)?;
        output
            .stdout
            .lines()
            .rev()
            .find(|line| !line.starts_with('#') && !line.trim().is_empty())
            .and_then(|line| line.rsplit(',').next())
            .map(str::trim)
            .map(str::to_owned)
            .ok_or_else(|| Error::InvalidProbe("framemd5 produced no video frame".into()))
    }

    fn remove_if_corrupt(&self, output: &Path) {
        if !output.exists() {
            return;
        }
        let playable = match self.probe(output) {
            Ok(probe) if probe.value.duration_s > 0.0 && probe.value.width == 0 => {
                probe.value.has_audio
            }
            Ok(probe) if probe.value.duration_s > 0.0 => self.frame_md5(output, 0.0).is_ok(),
            _ => false,
        };
        if !playable {
            let _ = fs::remove_file(output);
        }
    }

    fn run_capture(&self, spec: &CommandSpec, low_priority: bool) -> Result<Captured> {
        let command_line = self.record_command(spec, low_priority)?;
        let output = spec.command(low_priority).output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            return Err(Error::CommandFailed {
                command: command_line,
                status: output.status,
                stderr,
            });
        }
        Ok(Captured {
            command: command_line,
            stdout,
        })
    }

    fn run_progress<F>(
        &self,
        spec: &CommandSpec,
        expected_duration_s: f64,
        cancellation: &CancellationToken,
        progress: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Progress),
    {
        let command_line = self.record_command(spec, true)?;
        let mut command = spec.command(true);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stdout pipe missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg stderr pipe missing"))?;
        let (sender, receiver) = mpsc::channel();
        let stdout_thread = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(stderr).read_to_end(&mut bytes);
            bytes
        });

        let mut report_progress = |line: io::Result<String>| {
            if let Ok(line) = line {
                if let Some(value) = parse_out_time_us(&line) {
                    let out_time_s = value as f64 / 1_000_000.0;
                    let percent = if expected_duration_s > 0.0 {
                        (out_time_s / expected_duration_s * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    };
                    progress(Progress {
                        out_time_s,
                        percent,
                    });
                }
            }
        };
        let mut cancel_started = None;
        let mut killed = false;
        let status = loop {
            while let Ok(line) = receiver.try_recv() {
                report_progress(line);
            }
            if cancellation.is_cancelled() && cancel_started.is_none() {
                signal_group(&child, libc::SIGINT)?;
                cancel_started = Some(Instant::now());
            }
            if let Some(started) = cancel_started {
                if !killed && started.elapsed() >= CANCEL_GRACE {
                    signal_group(&child, libc::SIGKILL)?;
                    killed = true;
                }
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            thread::sleep(POLL_INTERVAL);
        };

        let _ = stdout_thread.join();
        for line in receiver.try_iter() {
            report_progress(line);
        }
        let stderr_bytes = stderr_thread.join().unwrap_or_default();
        let stderr_text = String::from_utf8_lossy(&stderr_bytes).into_owned();
        if cancel_started.is_some() {
            return Err(Error::Cancelled {
                command: command_line,
            });
        }
        if !status.success() {
            return Err(Error::CommandFailed {
                command: command_line,
                status,
                stderr: stderr_text,
            });
        }
        Ok(command_line)
    }

    /// Run a crate-internal FFmpeg operation through the same command recording,
    /// cancellation, process-group, and progress supervision as the built-in stages.
    /// Keeping raw arguments crate-private lets sibling render modules compose bundled
    /// FFmpeg without exposing an unsupervised command surface to application callers.
    pub(crate) fn run_ffmpeg_progress_args<F>(
        &self,
        arguments: Vec<OsString>,
        expected_duration_s: f64,
        cancellation: &CancellationToken,
        progress: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Progress),
    {
        let spec = CommandSpec::new(&self.resolved.path).args(arguments);
        self.run_progress(&spec, expected_duration_s, cancellation, progress)
    }

    fn record_command(&self, spec: &CommandSpec, low_priority: bool) -> Result<String> {
        let rendered = spec.render(low_priority);
        tracing::info!(
            job_id = %self.job_id,
            stage = "ffmpeg",
            command = %rendered,
            "running media command"
        );
        if let Some(directory) = &self.debug_dir {
            fs::create_dir_all(directory)?;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join("commands.txt"))?;
            writeln!(file, "{rendered}")?;
        }
        Ok(rendered)
    }
}

fn reject_existing_destination(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(Error::InvalidArgument(format!(
            "{label} already exists; choose a new private staging path: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn classify_clip_render_error(error: Error, audio: ClipAudio) -> Error {
    let Error::CommandFailed {
        command,
        status,
        stderr,
    } = error
    else {
        return error;
    };
    let normalized = stderr.to_ascii_lowercase();
    if normalized.contains("videotoolbox")
        || normalized.contains("error while opening encoder")
        || normalized.contains("encoder not found")
    {
        let audio_note = if audio == ClipAudio::Source {
            " and AAC"
        } else {
            ""
        };
        Error::CapabilityUnavailable {
            capability: format!("VideoToolbox H.264{audio_note} recipe encoding"),
            reason: format!(
                "the bundled encoder was advertised but could not initialize at runtime (status {status}); command: {command}; stderr: {}",
                stderr.trim()
            ),
        }
    } else {
        Error::CommandFailed {
            command,
            status,
            stderr,
        }
    }
}

fn component_listing_contains(listing: &str, component: &str) -> bool {
    listing.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some(flags) = fields.next() else {
            return false;
        };
        let Some(name) = fields.next() else {
            return false;
        };
        flags
            .chars()
            .all(|value| value == '.' || value.is_ascii_uppercase())
            && name == component
    })
}

pub(crate) fn required_clip_filters(request: &ClipRenderRequest) -> Vec<&'static str> {
    let mut filters = vec!["format", "setparams"];
    if request.crop.is_some() {
        filters.push("crop");
    }
    if let VideoGrade::Basic(grade) = request.grade {
        if grade.exposure_ev != 0.0 {
            filters.push("exposure");
        }
        if grade.contrast != 0.0 {
            filters.push("colorlevels");
        }
        if grade.saturation != 1.0 {
            filters.push("hue");
        }
        if grade.temperature != 0.0 || grade.tint != 0.0 {
            filters.push("colorbalance");
        }
    }
    filters
}

pub(crate) fn clip_filter_chain(
    probe: &Probe,
    request: &ClipRenderRequest,
) -> Result<(String, u32, u32)> {
    let (display_width, display_height) = displayed_dimensions(probe);
    let (width, height, crop_filter) = match request.crop {
        Some(crop) => {
            let (x, y, width, height) = quantized_video_crop(display_width, display_height, crop)?;
            (
                width,
                height,
                Some(format!("crop={width}:{height}:{x}:{y}")),
            )
        }
        None => {
            if display_width % 2 != 0 || display_height % 2 != 0 {
                return Err(Error::CapabilityUnavailable {
                    capability: "yuv420p H.264 dimensions".to_owned(),
                    reason: format!(
                        "displayed source dimensions {display_width}x{display_height} are odd and schema v1 declares no crop or padding policy"
                    ),
                });
            }
            (display_width, display_height, None)
        }
    };
    let mut filters = Vec::new();
    if let Some(crop_filter) = crop_filter {
        filters.push(crop_filter);
    }
    if let VideoGrade::Basic(grade) = request.grade {
        append_exposure_filters(&mut filters, grade.exposure_ev);
        if grade.contrast > 0.0 {
            let input_black = grade.contrast * 0.25;
            let input_white = 1.0 - grade.contrast * 0.25;
            filters.push(format!(
                "colorlevels=rimin={}:gimin={}:bimin={}:rimax={}:gimax={}:bimax={}",
                format_number(input_black),
                format_number(input_black),
                format_number(input_black),
                format_number(input_white),
                format_number(input_white),
                format_number(input_white)
            ));
        } else if grade.contrast < 0.0 {
            let output_black = -grade.contrast * 0.25;
            let output_white = 1.0 + grade.contrast * 0.25;
            filters.push(format!(
                "colorlevels=romin={}:gomin={}:bomin={}:romax={}:gomax={}:bomax={}",
                format_number(output_black),
                format_number(output_black),
                format_number(output_black),
                format_number(output_white),
                format_number(output_white),
                format_number(output_white)
            ));
        }
        if grade.saturation != 1.0 {
            filters.push(format!("hue=s={}", format_number(grade.saturation)));
        }
        if grade.temperature != 0.0 || grade.tint != 0.0 {
            let red = 0.25 * grade.temperature + 0.12 * grade.tint;
            let green = -0.25 * grade.tint;
            let blue = -0.25 * grade.temperature + 0.12 * grade.tint;
            filters.push(format!(
                "colorbalance=rs={r}:rm={r}:rh={r}:gs={g}:gm={g}:gh={g}:bs={b}:bm={b}:bh={b}:pl=1",
                r = format_number(red),
                g = format_number(green),
                b = format_number(blue)
            ));
        }
    }
    filters.push("format=yuv420p".to_owned());
    filters.push(
        "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709".to_owned(),
    );
    Ok((filters.join(","), width, height))
}

fn append_exposure_filters(filters: &mut Vec<String>, exposure_ev: f64) {
    let mut remaining = exposure_ev;
    while remaining.abs() > f64::EPSILON {
        let step = remaining.clamp(-3.0, 3.0);
        filters.push(format!("exposure=exposure={}", format_number(step)));
        remaining -= step;
        if remaining.abs() < 0.000_000_5 {
            break;
        }
    }
}

fn displayed_dimensions(probe: &Probe) -> (u32, u32) {
    if probe.rotation.unwrap_or(0).rem_euclid(180) == 90 {
        (probe.height, probe.width)
    } else {
        (probe.width, probe.height)
    }
}

fn quantized_video_crop(
    width: u32,
    height: u32,
    crop: NormalizedVideoCrop,
) -> Result<(u32, u32, u32, u32)> {
    if width < 2 || height < 2 {
        return Err(Error::InvalidArgument(
            "video is too small for a yuv420p crop".to_owned(),
        ));
    }
    let mut left = (crop.x * f64::from(width)).floor() as u32;
    let mut top = (crop.y * f64::from(height)).floor() as u32;
    let mut right = ((crop.x + crop.width) * f64::from(width)).ceil() as u32;
    let mut bottom = ((crop.y + crop.height) * f64::from(height)).ceil() as u32;
    left = left.min(width - 1) & !1;
    top = top.min(height - 1) & !1;
    right = right.min(width);
    bottom = bottom.min(height);
    right = (right + 1) & !1;
    bottom = (bottom + 1) & !1;
    if right > width {
        right = width & !1;
    }
    if bottom > height {
        bottom = height & !1;
    }
    if right <= left || bottom <= top {
        return Err(Error::InvalidArgument(
            "normalized clip crop collapsed after yuv420p pixel quantization".to_owned(),
        ));
    }
    Ok((left, top, right - left, bottom - top))
}

pub(crate) fn validate_h264_sdr_source(probe: &Probe) -> Result<String> {
    let bit_depth = probe
        .bit_depth
        .ok_or_else(|| Error::CapabilityUnavailable {
            capability: "8-bit H.264 SDR conversion".to_owned(),
            reason: "source bit depth is unknown".to_owned(),
        })?;
    if bit_depth > 8 {
        return Err(Error::CapabilityUnavailable {
            capability: "8-bit H.264 SDR conversion".to_owned(),
            reason: format!(
                "source is {bit_depth}-bit and schema v1 has no approved high-depth tone-map/dither policy"
            ),
        });
    }
    let primaries = meaningful_probe_tag(probe.color_primaries.as_deref());
    let transfer = meaningful_probe_tag(probe.color_transfer.as_deref());
    let space = meaningful_probe_tag(probe.color_space.as_deref());
    let range = meaningful_probe_tag(probe.color_range.as_deref());
    let unsupported = [
        ("primaries", primaries, &["bt709"][..]),
        ("transfer", transfer, &["bt709"][..]),
        ("matrix", space, &["bt709"][..]),
        ("range", range, &["tv", "mpeg"][..]),
    ]
    .into_iter()
    .find(|(_, value, allowed)| value.is_some_and(|value| !allowed.contains(&value)));
    if let Some((field, value, _)) = unsupported {
        return Err(Error::CapabilityUnavailable {
            capability: "H.264 SDR BT.709 color conversion".to_owned(),
            reason: format!(
                "source {field} tag {:?} requires a conversion/tone-map policy not declared by schema v1",
                value.unwrap_or("unknown")
            ),
        });
    }
    if primaries.is_none() && transfer.is_none() && space.is_none() && range.is_none() {
        Ok("untagged-assumed-sdr-bt709".to_owned())
    } else {
        Ok("verified-sdr-bt709".to_owned())
    }
}

fn meaningful_probe_tag(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        !value.trim().is_empty()
            && !value.eq_ignore_ascii_case("unknown")
            && !value.eq_ignore_ascii_case("unspecified")
    })
}

#[allow(clippy::too_many_arguments)]
fn verify_clip_render(
    output: &Probe,
    request: &ClipRenderRequest,
    source: &Probe,
    expected_width: u32,
    expected_height: u32,
    expected_duration: f64,
    duration_tolerance: f64,
) -> Result<()> {
    if output.video_codec.as_deref() != Some("h264") {
        return Err(Error::InvalidProbe(format!(
            "clip output codec is {:?}, expected H.264",
            output.video_codec
        )));
    }
    if (output.width, output.height) != (expected_width, expected_height) {
        return Err(Error::InvalidProbe(format!(
            "clip output dimensions are {}x{}, expected {}x{}",
            output.width, output.height, expected_width, expected_height
        )));
    }
    if output.pixel_format.as_deref() != Some("yuv420p") || output.bit_depth != Some(8) {
        return Err(Error::InvalidProbe(format!(
            "clip output pixel format/depth is {:?}/{:?}, expected yuv420p/8-bit",
            output.pixel_format, output.bit_depth
        )));
    }
    if (output.duration_s - expected_duration).abs() > duration_tolerance {
        return Err(Error::InvalidProbe(format!(
            "clip output duration {:.6}s differs from requested {:.6}s",
            output.duration_s, expected_duration
        )));
    }
    let expected_audio = request.audio == ClipAudio::Source && source.has_audio;
    if output.has_audio != expected_audio {
        return Err(Error::InvalidProbe(format!(
            "clip output audio presence is {}, expected {}",
            output.has_audio, expected_audio
        )));
    }
    for (label, actual) in [
        ("primaries", output.color_primaries.as_deref()),
        ("transfer", output.color_transfer.as_deref()),
        ("matrix", output.color_space.as_deref()),
    ] {
        if actual != Some("bt709") {
            return Err(Error::InvalidProbe(format!(
                "clip output {label} is {actual:?}, expected bt709"
            )));
        }
    }
    if output.rotation.is_some_and(|rotation| rotation != 0) {
        return Err(Error::InvalidProbe(format!(
            "clip output retained unexpected rotation metadata {:?}",
            output.rotation
        )));
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn remove_sampled_frames(output_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_sampled_frame_name(&entry.file_name()) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn is_sampled_frame_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.strip_prefix('f')
        .and_then(|value| value.strip_suffix(".jpg"))
        .is_some_and(|digits| digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn signal_group(child: &Child, signal: libc::c_int) -> Result<()> {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as libc::pid_t);
        // SAFETY: `kill` is called with the child process group created immediately before spawn.
        let result = unsafe { libc::kill(process_group, signal) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(Error::Io(error));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        child.kill().map_err(Error::Io)
    }
}

fn effective_threads(configured: usize) -> usize {
    if configured > 0 {
        return configured.max(1);
    }
    physical_cores().saturating_sub(2).max(1)
}

fn physical_cores() -> usize {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.physicalcpu"])
            .output()
        {
            if output.status.success() {
                if let Ok(value) = String::from_utf8_lossy(&output.stdout).trim().parse() {
                    return value;
                }
            }
        }
    }
    thread::available_parallelism().map_or(1, usize::from)
}

fn parse_out_time_us(line: &str) -> Option<u64> {
    line.strip_prefix("out_time_us=")?.trim().parse().ok()
}

fn parse_rate(rate: &str) -> f64 {
    let Some((numerator, denominator)) = rate.split_once('/') else {
        return rate.parse().unwrap_or(0.0);
    };
    let numerator = numerator.parse::<f64>().unwrap_or(0.0);
    let denominator = denominator.parse::<f64>().unwrap_or(0.0);
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

pub(crate) fn format_number(value: f64) -> String {
    let formatted = format!("{value:.6}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[derive(Debug)]
struct Captured {
    command: String,
    stdout: String,
}

#[derive(Debug, Clone)]
struct CommandSpec {
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl CommandSpec {
    fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
        }
    }

    fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    fn command(&self, low_priority: bool) -> Command {
        if low_priority && cfg!(target_os = "macos") {
            let mut command = Command::new("/usr/bin/nice");
            command
                .args(["-n", "10"])
                .arg(&self.program)
                .args(&self.arguments);
            command
        } else {
            let mut command = Command::new(&self.program);
            command.args(&self.arguments);
            command
        }
    }

    fn render(&self, low_priority: bool) -> String {
        let mut parts = Vec::new();
        if low_priority && cfg!(target_os = "macos") {
            parts.extend([quote(OsStr::new("/usr/bin/nice")), "-n".into(), "10".into()]);
        }
        parts.push(quote(self.program.as_os_str()));
        parts.extend(self.arguments.iter().map(|argument| quote(argument)));
        parts.join(" ")
    }
}

fn quote(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:=,%@".contains(&byte))
    {
        value.into_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: ProbeFormat,
}

impl ProbeDocument {
    fn into_probe(self) -> Result<Probe> {
        let video = self
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("video"));
        let has_audio = self
            .streams
            .iter()
            .any(|stream| stream.codec_type.as_deref() == Some("audio"));
        if video.is_none() && !has_audio {
            return Err(Error::InvalidProbe("no audio or video streams".into()));
        }
        let duration_s = self
            .format
            .duration
            .as_deref()
            .and_then(|value| value.parse().ok())
            .or_else(|| video.and_then(|stream| stream.duration.as_deref()?.parse().ok()))
            .filter(|duration: &f64| duration.is_finite() && *duration >= 0.0)
            .ok_or_else(|| Error::InvalidProbe("duration is missing or invalid".into()))?;
        let (fps, width, height) = video.map_or((0.0, 0, 0), |stream| {
            let average = stream
                .avg_frame_rate
                .as_deref()
                .map(parse_rate)
                .unwrap_or(0.0);
            let rate = if average > 0.0 {
                average
            } else {
                stream
                    .r_frame_rate
                    .as_deref()
                    .map(parse_rate)
                    .unwrap_or(0.0)
            };
            (rate, stream.width.unwrap_or(0), stream.height.unwrap_or(0))
        });
        let audio = self
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some("audio"));
        let parse_stream_duration = |stream: Option<&ProbeStream>| {
            stream?
                .duration
                .as_deref()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
        };
        Ok(Probe {
            duration_s,
            fps,
            width,
            height,
            has_audio,
            container: self.format.format_name,
            video_codec: video.and_then(|stream| stream.codec_name.clone()),
            codec_profile: video.and_then(|stream| stream.profile.clone()),
            codec_tag: video.and_then(|stream| stream.codec_tag_string.clone()),
            pixel_format: video.and_then(|stream| stream.pix_fmt.clone()),
            bit_depth: video.and_then(infer_bit_depth),
            color_space: video.and_then(|stream| stream.color_space.clone()),
            color_primaries: video.and_then(|stream| stream.color_primaries.clone()),
            color_transfer: video.and_then(|stream| stream.color_transfer.clone()),
            color_range: video.and_then(|stream| stream.color_range.clone()),
            rotation: video.and_then(stream_rotation),
            video_frame_count: video
                .and_then(|stream| stream.nb_frames.as_deref())
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value >= 0),
            video_duration_s: parse_stream_duration(video),
            audio_duration_s: parse_stream_duration(audio),
            audio_sample_rate: audio
                .and_then(|stream| stream.sample_rate.as_deref())
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| *value > 0),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    format_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    codec_tag_string: Option<String>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    color_range: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    nb_frames: Option<String>,
    sample_rate: Option<String>,
    #[serde(default)]
    tags: ProbeTags,
    #[serde(default)]
    side_data_list: Vec<ProbeSideData>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeTags {
    rotate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeSideData {
    rotation: Option<i32>,
}

/// Pixel formats whose 8-bit depth is known. Anything else without an explicit depth marker
/// stays unknown so the direct-edit gate can require a proxy instead of guessing.
const KNOWN_8BIT_PIXEL_FORMATS: &[&str] = &[
    "yuv420p", "yuvj420p", "yuv422p", "yuvj422p", "yuv444p", "yuvj444p", "yuv410p", "yuv411p",
    "nv12", "nv21", "nv16", "yuyv422", "uyvy422", "gray", "gbrp", "rgb24", "bgr24", "rgba", "bgra",
    "argb", "abgr", "rgb0", "bgr0", "pal8",
];

fn infer_bit_depth(stream: &ProbeStream) -> Option<u8> {
    stream
        .bits_per_raw_sample
        .as_deref()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .or_else(|| {
            let pixel_format = stream.pix_fmt.as_deref()?;
            for depth in [16, 14, 12, 10, 9] {
                let marker = depth.to_string();
                if pixel_format.contains(&format!("{marker}le"))
                    || pixel_format.contains(&format!("{marker}be"))
                {
                    return Some(depth);
                }
            }
            KNOWN_8BIT_PIXEL_FORMATS
                .contains(&pixel_format)
                .then_some(8)
        })
}

fn stream_rotation(stream: &ProbeStream) -> Option<i32> {
    stream
        .side_data_list
        .iter()
        .find_map(|data| data.rotation)
        .or_else(|| stream.tags.rotate.as_deref()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn executable(path: &Path) {
        File::create(path).unwrap();
        #[cfg(unix)]
        {
            let mut permissions = path.metadata().unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    /// Pins the one documented duration-tolerance rule shared by stage-split and the durable
    /// executor: frame_tolerance = 1/fps (fallback 1/30), plus the shared container slack.
    /// The 60 fps value is the AAC-priming case: the encoder check and the executor re-check
    /// must agree exactly (before the shared rule the executor rejected beyond 0.05 s what
    /// the encoder accepted up to ≈0.067 s).
    #[test]
    fn duration_tolerance_rule_is_the_single_shared_formula() {
        assert!(
            (duration_tolerance_s(60.0) - (1.0 / 60.0 + 0.05)).abs() < 1e-12,
            "60 fps must allow the AAC-priming container slack"
        );
        assert!((duration_tolerance_s(30.0) - (1.0 / 30.0 + 0.05)).abs() < 1e-12);
        assert!((duration_tolerance_s(15.0) - (1.0 / 15.0 + 0.05)).abs() < 1e-12);
        assert!((duration_tolerance_s(0.0) - (1.0 / 30.0 + 0.05)).abs() < 1e-12);
        assert!((duration_tolerance_s(-12.0) - duration_tolerance_s(0.0)).abs() < 1e-12);
        assert!((frame_tolerance_s(24.0) - 1.0 / 24.0).abs() < 1e-12);
        assert!((DURATION_TOLERANCE_SLACK_S - 0.05).abs() < 1e-12);
    }

    #[test]
    fn resolver_prefers_bundle_then_dev_then_path() {
        let temporary = tempfile::tempdir().unwrap();
        let bundle = temporary.path().join("bundle");
        let development = temporary.path().join("development");
        let path = temporary.path().join("path");
        for directory in [&bundle, &development, &path] {
            fs::create_dir(directory).unwrap();
            executable(&directory.join("ffmpeg"));
            executable(&directory.join("ffprobe"));
        }
        let path_value = std::env::join_paths([&path]).unwrap();
        let resolved =
            resolve_with(Some(&bundle), Some(&development), Some(&path_value), true).unwrap();
        assert_eq!(resolved.source, Source::Bundled);
        fs::remove_file(bundle.join("ffmpeg")).unwrap();
        let resolved =
            resolve_with(Some(&bundle), Some(&development), Some(&path_value), true).unwrap();
        assert_eq!(resolved.source, Source::DevSidecarDir);
        fs::remove_file(development.join("ffmpeg")).unwrap();
        let resolved =
            resolve_with(Some(&bundle), Some(&development), Some(&path_value), true).unwrap();
        assert_eq!(resolved.source, Source::Path);
        assert!(resolve_with(None, None, Some(&path_value), false).is_err());
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn resolver_accepts_tauri_target_triple_sidecar_names() {
        let temporary = tempfile::tempdir().unwrap();
        executable(&temporary.path().join("ffmpeg-aarch64-apple-darwin"));
        executable(&temporary.path().join("ffprobe-aarch64-apple-darwin"));

        let resolved = resolve_pair(temporary.path(), Source::Bundled).unwrap();
        assert_eq!(resolved.source, Source::Bundled);
        assert!(resolved.path.ends_with("ffmpeg-aarch64-apple-darwin"));
        assert!(resolved
            .ffprobe_path
            .ends_with("ffprobe-aarch64-apple-darwin"));
    }

    #[test]
    fn parses_video_and_audio_probe_json() {
        let document: ProbeDocument = serde_json::from_str(
            r#"{"streams":[{"codec_type":"video","codec_name":"hevc","profile":"Main 10","codec_tag_string":"hvc1","pix_fmt":"yuv420p10le","bits_per_raw_sample":"10","color_space":"bt2020nc","color_primaries":"bt2020","color_transfer":"smpte2084","color_range":"tv","width":640,"height":360,"avg_frame_rate":"30000/1001","side_data_list":[{"rotation":-90}]},{"codec_type":"audio"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"12.500000"}}"#,
        )
        .unwrap();
        let probe = document.into_probe().unwrap();
        assert_eq!(probe.width, 640);
        assert_eq!(probe.height, 360);
        assert!((probe.fps - 29.970_029_97).abs() < 1e-8);
        assert_eq!(probe.duration_s, 12.5);
        assert!(probe.has_audio);
        assert_eq!(probe.container.as_deref(), Some("mov,mp4,m4a,3gp,3g2,mj2"));
        assert_eq!(probe.video_codec.as_deref(), Some("hevc"));
        assert_eq!(probe.codec_profile.as_deref(), Some("Main 10"));
        assert_eq!(probe.codec_tag.as_deref(), Some("hvc1"));
        assert_eq!(probe.bit_depth, Some(10));
        assert_eq!(probe.color_primaries.as_deref(), Some("bt2020"));
        assert_eq!(probe.rotation, Some(-90));
    }

    #[test]
    fn progress_uses_microseconds_and_is_bounded() {
        assert_eq!(parse_out_time_us("out_time_us=2500000"), Some(2_500_000));
        assert_eq!(parse_out_time_us("frame=12"), None);
    }

    fn color_probe(
        primaries: Option<&str>,
        transfer: Option<&str>,
        space: Option<&str>,
        range: Option<&str>,
    ) -> Probe {
        Probe {
            duration_s: 1.0,
            fps: 24.0,
            width: 640,
            height: 360,
            has_audio: false,
            container: None,
            video_codec: Some("hevc".to_owned()),
            codec_profile: None,
            codec_tag: None,
            pixel_format: None,
            bit_depth: None,
            color_space: space.map(str::to_owned),
            color_primaries: primaries.map(str::to_owned),
            color_transfer: transfer.map(str::to_owned),
            color_range: range.map(str::to_owned),
            rotation: None,
            video_frame_count: None,
            video_duration_s: None,
            audio_duration_s: None,
            audio_sample_rate: None,
        }
    }

    #[test]
    fn edit_proxy_color_args_pass_source_tags_through_or_default_to_bt709() {
        let hdr = color_probe(
            Some("bt2020"),
            Some("smpte2084"),
            Some("bt2020nc"),
            Some("tv"),
        );
        assert_eq!(
            edit_proxy_color_args(&hdr),
            vec![
                "-color_primaries",
                "bt2020",
                "-color_trc",
                "smpte2084",
                "-colorspace",
                "bt2020nc",
                "-color_range",
                "tv",
            ]
        );
        let untagged = color_probe(None, None, None, None);
        assert_eq!(
            edit_proxy_color_args(&untagged),
            vec![
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
            ]
        );
        let unknown_tagged = color_probe(Some("unknown"), Some("unknown"), None, None);
        assert_eq!(
            edit_proxy_color_args(&unknown_tagged),
            vec![
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
                "-colorspace",
                "bt709",
            ]
        );
    }

    #[test]
    fn edit_proxy_recipe_records_the_encode_settings() {
        let recipe = edit_proxy_recipe();
        assert_eq!(recipe["video_encoder"], "h264_videotoolbox");
        assert_eq!(recipe["video_bitrate"], "12M");
        assert_eq!(recipe["audio_bitrate"], "192k");
        assert_eq!(recipe["movflags"], "+faststart");
        assert!(recipe["filter"]
            .as_str()
            .unwrap()
            .contains("force_divisible_by=2"));
    }

    fn clip_request() -> ClipRenderRequest {
        ClipRenderRequest {
            in_s: 1.0,
            out_s: 2.0,
            crop: None,
            grade: VideoGrade::None,
            transition: ClipTransition::Cut,
            audio: ClipAudio::Source,
            output: ClipOutputPreset::Mp4H264SdrV1,
        }
    }

    #[test]
    fn clip_recipe_validation_rejects_out_of_contract_values() {
        assert!(clip_request().validate().is_ok());
        let invalid_range = ClipRenderRequest {
            out_s: 1.0,
            ..clip_request()
        };
        assert!(invalid_range.validate().is_err());
        let invalid_crop = ClipRenderRequest {
            crop: Some(NormalizedVideoCrop {
                x: 0.9,
                y: 0.0,
                width: 0.2,
                height: 1.0,
            }),
            ..clip_request()
        };
        assert!(invalid_crop.validate().is_err());
        let invalid_grade = ClipRenderRequest {
            grade: VideoGrade::Basic(BasicVideoGrade {
                exposure_ev: 0.0,
                contrast: 0.0,
                saturation: 2.1,
                temperature: 0.0,
                tint: 0.0,
            }),
            ..clip_request()
        };
        assert!(invalid_grade.validate().is_err());
    }

    #[test]
    fn clip_filter_chain_quantizes_crop_and_splits_large_exposure() {
        let mut probe = color_probe(None, None, None, None);
        probe.width = 641;
        probe.height = 359;
        probe.rotation = Some(-90);
        let request = ClipRenderRequest {
            crop: Some(NormalizedVideoCrop {
                x: 0.1,
                y: 0.2,
                width: 0.5,
                height: 0.5,
            }),
            grade: VideoGrade::Basic(BasicVideoGrade {
                exposure_ev: 5.0,
                contrast: -0.5,
                saturation: 0.0,
                temperature: 1.0,
                tint: -1.0,
            }),
            ..clip_request()
        };
        let (filter, width, height) = clip_filter_chain(&probe, &request).unwrap();
        assert_eq!((width % 2, height % 2), (0, 0));
        assert!(filter.starts_with("crop="));
        assert!(filter.contains("exposure=exposure=3,exposure=exposure=2"));
        assert!(filter.contains("colorlevels=romin="));
        assert!(filter.contains("hue=s=0"));
        assert!(filter.contains("colorbalance="));
        assert!(filter.ends_with(
            "format=yuv420p,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
        ));
    }

    #[test]
    fn h264_sdr_gate_rejects_unapproved_hdr_or_high_depth_conversion() {
        let mut hdr = color_probe(
            Some("bt2020"),
            Some("smpte2084"),
            Some("bt2020nc"),
            Some("tv"),
        );
        hdr.bit_depth = Some(10);
        let error = validate_h264_sdr_source(&hdr).unwrap_err();
        assert!(matches!(error, Error::CapabilityUnavailable { .. }));
        assert!(error.to_string().contains("10-bit"));

        let mut p3 = color_probe(Some("smpte432"), Some("bt709"), Some("bt709"), Some("tv"));
        p3.bit_depth = Some(8);
        let error = validate_h264_sdr_source(&p3).unwrap_err();
        assert!(matches!(error, Error::CapabilityUnavailable { .. }));
        assert!(error.to_string().contains("smpte432"));

        let mut untagged = color_probe(None, None, None, None);
        untagged.bit_depth = Some(8);
        assert_eq!(
            validate_h264_sdr_source(&untagged).unwrap(),
            "untagged-assumed-sdr-bt709"
        );
    }

    #[test]
    fn ffmpeg_component_listing_parser_does_not_match_help_text() {
        let listing = "Encoders:\n V....D h264_videotoolbox VideoToolbox H.264 Encoder\n A....D aac AAC\n TS exposure V->V\n -- h264_videotoolbox mentioned in help";
        assert!(component_listing_contains(listing, "h264_videotoolbox"));
        assert!(component_listing_contains(listing, "aac"));
        assert!(component_listing_contains(listing, "exposure"));
        assert!(!component_listing_contains(listing, "libx264"));
    }

    #[test]
    fn bit_depth_inference_treats_unrecognized_pixel_formats_as_unknown() {
        let stream = |pix_fmt: Option<&str>| ProbeStream {
            pix_fmt: pix_fmt.map(str::to_owned),
            ..ProbeStream::default()
        };
        assert_eq!(infer_bit_depth(&stream(Some("yuv420p"))), Some(8));
        assert_eq!(infer_bit_depth(&stream(Some("yuv420p10le"))), Some(10));
        assert_eq!(infer_bit_depth(&stream(Some("yuv420p16be"))), Some(16));
        assert_eq!(infer_bit_depth(&stream(Some("p016le"))), Some(16));
        assert_eq!(infer_bit_depth(&stream(Some("yuv410p"))), Some(8));
        assert_eq!(infer_bit_depth(&stream(Some("made_up_fmt"))), None);
        assert_eq!(infer_bit_depth(&stream(None)), None);
    }

    #[test]
    fn command_rendering_is_shell_pasteable() {
        let spec = CommandSpec::new("/tmp/ffmpeg").args([
            "-i",
            "a file's clip.mp4",
            "-vf",
            "fps=4,scale=-2:480",
        ]);
        let rendered = spec.render(false);
        assert_eq!(
            rendered,
            "/tmp/ffmpeg -i 'a file'\\''s clip.mp4' -vf fps=4,scale=-2:480"
        );
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_escalates_to_process_group_kill_after_grace_period() {
        let resolved = Resolved {
            path: "python3".into(),
            ffprobe_path: "python3".into(),
            source: Source::Bundled,
        };
        let runner = Runner::new(resolved, 1, "cancel-test");
        let cancellation = CancellationToken::default();
        let triggered = cancellation.clone();
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            triggered.cancel();
        });
        let started = Instant::now();
        let spec = CommandSpec::new("python3").args([
            "-c",
            "import signal,time; signal.signal(signal.SIGINT, signal.SIG_IGN); signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(10)",
        ]);
        let result = runner.run_progress(&spec, 1.0, &cancellation, &mut |_| {});
        trigger.join().unwrap();
        assert!(matches!(result, Err(Error::Cancelled { .. })));
        assert!(started.elapsed() >= CANCEL_GRACE);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
