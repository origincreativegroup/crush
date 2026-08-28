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
static BUNDLE_RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub out_time_s: f64,
    pub percent: f64,
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

        let frame_tolerance = if source_probe.fps > 0.0 {
            1.0 / source_probe.fps
        } else {
            1.0 / 30.0
        };
        if self.copy_is_accurate(input, start_s, output, expected_duration, frame_tolerance) {
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
        if (output_probe.duration_s - expected_duration).abs() > frame_tolerance + 0.05 {
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
        tolerance: f64,
    ) -> bool {
        let Ok(probe) = self.probe(output) else {
            return false;
        };
        if (probe.value.duration_s - expected_duration).abs() > tolerance + 0.05 {
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

fn format_number(value: f64) -> String {
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
        Ok(Probe {
            duration_s,
            fps,
            width,
            height,
            has_audio,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
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
            r#"{"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"30000/1001"},{"codec_type":"audio"}],"format":{"duration":"12.500000"}}"#,
        )
        .unwrap();
        let probe = document.into_probe().unwrap();
        assert_eq!(probe.width, 640);
        assert_eq!(probe.height, 360);
        assert!((probe.fps - 29.970_029_97).abs() < 1e-8);
        assert_eq!(probe.duration_s, 12.5);
        assert!(probe.has_audio);
    }

    #[test]
    fn progress_uses_microseconds_and_is_bounded() {
        assert_eq!(parse_out_time_us("out_time_us=2500000"), Some(2_500_000));
        assert_eq!(parse_out_time_us("frame=12"), None);
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
