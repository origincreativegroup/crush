//! Backend-neutral resolved reel recipes and the first ordered FFmpeg renderer.
//!
//! Reel Studio's recipe vocabulary is represented here even where the first backend cannot yet
//! execute it. Validation rejects every unsupported non-default value before rendering so creative
//! intent is never silently omitted.
//!
//! TASK-036 boundary contract: every item delivers `round((out_s - in_s) * fps)` frames starting
//! at the first source frame at or after `in_s`, its video timeline starts at zero, and the
//! assembly places each cut exactly at the previous item's video duration. Within an item the
//! audio is TRIMMED to the item's video duration (not the reverse): the requested video frames
//! are the content contract, and padding video to cover audio would invent frames nobody asked
//! for. The assembly stream-copies the video track and re-encodes one audio track from the
//! items, because stream-copying item audio would carry each item's AAC encoder-priming packet
//! (a negative raw timestamp the MP4 edit list normally hides), and the concat demuxer's
//! per-file timestamp normalization turns that priming into a reel-wide head dead zone and a
//! cut that drifts late — the exact defects the 021 render review rejected.

use crate::ffmpeg::{
    BasicVideoGrade, CancellationToken, ClipAudio, ClipOutputPreset, Error, NormalizedVideoCrop,
    Probe, Progress, ReelItemRenderSpec, Result, Runner, VideoGrade,
};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const EPSILON: f64 = 0.000_001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelMediaKind {
    Video,
    Photo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelFormat {
    Source,
    Portrait9x16,
    Portrait4x5,
    Square1x1,
    Landscape16x9,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedReelCropKeyframe {
    pub time_s: f64,
    pub crop: NormalizedVideoCrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelCaptionPosition {
    Low,
    Mid,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReelCaption {
    pub text: String,
    pub position: ReelCaptionPosition,
}

/// Transition into an item. Names preserve Reel Studio's mapping to FFmpeg `xfade` effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelTransitionKind {
    Cut,
    Mix,
    Fade,
    White,
    SlideLeft,
    SlideRight,
    SlideUp,
    WipeLeft,
    Circle,
    BlurMix,
    Whip,
    Zoom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedReelTransition {
    pub kind: ReelTransitionKind,
    pub duration_s: f64,
}

impl Default for ResolvedReelTransition {
    fn default() -> Self {
        Self {
            kind: ReelTransitionKind::Cut,
            duration_s: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelMotion {
    None,
    In,
    Out,
    Left,
    Right,
}

/// Normalized resolved grade. The extended fields retain Reel Studio's hue, vibrance, shadow,
/// and highlight intent even though the first backend only supports the basic grade subset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedReelGrade {
    pub exposure_ev: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    pub hue_degrees: f64,
    pub vibrance: f64,
    pub shadows: f64,
    pub highlights: f64,
}

impl Default for ResolvedReelGrade {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            contrast: 0.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            hue_degrees: 0.0,
            vibrance: 0.0,
            shadows: 0.0,
            highlights: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelItem {
    pub source_path: PathBuf,
    pub media_kind: ReelMediaKind,
    /// Source-timeline in point for video; must be zero for a future photo hold.
    pub in_s: f64,
    /// Source-timeline out point for video; the difference is the future photo hold duration.
    pub out_s: f64,
    pub crop: Option<NormalizedVideoCrop>,
    pub crop_keyframes: Vec<ResolvedReelCropKeyframe>,
    pub caption: Option<ResolvedReelCaption>,
    pub transition: ResolvedReelTransition,
    pub speed: f64,
    pub motion: ReelMotion,
    /// Natural source-audio gain, normalized to 0..=1.
    pub volume: f64,
    pub grade: ResolvedReelGrade,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelMusic {
    pub source_path: PathBuf,
    pub volume: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelWatermarkPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelWatermark {
    pub source_path: PathBuf,
    pub position: ReelWatermarkPosition,
    pub opacity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelCover {
    pub source_path: PathBuf,
    pub time_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelRequest {
    pub items: Vec<ResolvedReelItem>,
    pub format: ReelFormat,
    pub music: Option<ResolvedReelMusic>,
    /// Final program gain, normalized to 0..=1.
    pub master_volume: f64,
    pub watermark: Option<ResolvedReelWatermark>,
    pub cover: Option<ResolvedReelCover>,
    pub output: ClipOutputPreset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelRenderBackend {
    VideoToolboxConcatDemuxer,
}

impl ReelRenderBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VideoToolboxConcatDemuxer => "videotoolbox+concat-demuxer",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReelRenderResult {
    /// The final ordered-assembly command.
    pub command: String,
    /// The frame-sensitive per-item encode commands, in recipe order.
    pub item_commands: Vec<String>,
    /// The video-only stream-copy commands that feed the concat demuxer, in recipe order.
    pub video_remux_commands: Vec<String>,
    pub backend: ReelRenderBackend,
    pub encoder: &'static str,
    pub preset: &'static str,
    pub requested_duration_s: f64,
    pub output_probe: Probe,
    pub probe_command: String,
    /// Per-item frame-exactness evidence: requested and delivered frame counts, the exact
    /// video duration, and the first source frame the item starts from (TASK-036).
    pub item_verifications: Vec<ReelItemVerification>,
    /// Concat video stream frame count (not the container duration audio padding can inflate).
    pub video_frame_count: i64,
    /// Concat video stream duration; the cut after item k lands exactly at its partial sum.
    pub video_duration_s: f64,
}

/// Frame-exactness evidence for one rendered reel item, recorded in the render manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct ReelItemVerification {
    pub index: usize,
    pub source_path: PathBuf,
    pub in_s: f64,
    pub out_s: f64,
    pub fps: f64,
    /// First source frame at or after `in_s` — the frame the item must start with.
    pub first_source_frame: i64,
    /// Last source frame the item delivers: `first_source_frame + frame_count - 1`.
    pub last_source_frame: i64,
    pub requested_frame_count: i64,
    pub rendered_frame_count: i64,
    pub video_duration_s: f64,
    pub audio_duration_s: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportedAudio {
    Source,
    Mute,
}

#[derive(Debug)]
struct ValidatedReel {
    duration_s: f64,
    audio: SupportedAudio,
    grades: Vec<VideoGrade>,
}

impl Runner {
    /// Render a resolved reel to a caller-owned private staging path.
    pub fn render_reel(
        &self,
        request: &ResolvedReelRequest,
        staging_output: &Path,
    ) -> Result<ReelRenderResult> {
        self.render_reel_with_control(
            request,
            staging_output,
            &CancellationToken::default(),
            |_| {},
        )
    }

    pub fn render_reel_with_control<F>(
        &self,
        request: &ResolvedReelRequest,
        staging_output: &Path,
        cancellation: &CancellationToken,
        mut progress: F,
    ) -> Result<ReelRenderResult>
    where
        F: FnMut(Progress),
    {
        reject_existing_destination(staging_output)?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled {
                command: "reel render before validation".to_owned(),
            });
        }
        let validated = validate_supported_request(request)?;

        let parent = staging_output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let private_render = tempfile::Builder::new()
            .prefix(".crush-reel-render-")
            .tempdir_in(parent)?;
        let extension = output_extension(request.output);
        let clip_audio = match validated.audio {
            SupportedAudio::Source => ClipAudio::Source,
            SupportedAudio::Mute => ClipAudio::Mute,
        };

        // Source-audio assembly decodes every item's audio and encodes one reel track, so the
        // items must share one audio topology. Silence insertion and a true audio mix graph
        // are separate capabilities and must not be inferred here.
        let mut audio_sample_rate = None;
        if validated.audio == SupportedAudio::Source {
            for item in &request.items {
                let probe = self.probe(&item.source_path)?.value;
                if !probe.has_audio {
                    return Err(unsupported(
                        "reel source-audio topology",
                        format!(
                            "{} has no audio stream; schema v1 does not insert synthetic silence",
                            item.source_path.display()
                        ),
                    ));
                }
                let rate = probe.audio_sample_rate.ok_or_else(|| unsupported(
                    "reel source-audio topology",
                    format!(
                        "{} reports no audio sample rate; the reel audio assembly cannot be verified",
                        item.source_path.display()
                    ),
                ))?;
                if let Some(expected) = audio_sample_rate {
                    if expected != rate {
                        return Err(unsupported(
                            "reel source-audio topology",
                            format!(
                                "reel items mix audio sample rates ({expected} Hz and {rate} Hz); schema v1 has no resampling policy"
                            ),
                        ));
                    }
                }
                audio_sample_rate = Some(rate);
            }
        }

        let item_count = request.items.len();
        let mut item_commands = Vec::with_capacity(item_count);
        let mut video_remux_commands = Vec::with_capacity(item_count);
        let mut item_verifications = Vec::with_capacity(item_count);
        let mut item_paths = Vec::with_capacity(item_count);
        let mut concat_paths = Vec::with_capacity(item_count);
        let mut completed_duration = 0.0;
        let mut completed_frames = 0_i64;
        let mut expected_dimensions = None;
        for (index, (item, grade)) in request
            .items
            .iter()
            .zip(validated.grades.iter().copied())
            .enumerate()
        {
            let source_probe = self.probe(&item.source_path)?.value;
            let plan = plan_item_frames(item.in_s, item.out_s, source_probe.fps)?;
            let item_output = private_render
                .path()
                .join(format!("item-{index:06}.{extension}"));
            let completed_before = completed_duration;
            let item_progress = |value: Progress| {
                let encoded = (completed_before + value.out_time_s).min(validated.duration_s);
                progress(Progress {
                    out_time_s: encoded,
                    percent: (encoded / (validated.duration_s * 2.0) * 100.0).clamp(0.0, 50.0),
                });
            };
            let rendered = self.render_reel_item_with_control(
                &item.source_path,
                &ReelItemRenderSpec {
                    in_s: item.in_s,
                    out_s: item.out_s,
                    seek_s: plan.seek_s,
                    read_s: plan.read_s,
                    frame_count: plan.frame_count,
                    video_duration_s: plan.video_duration_s,
                    crop: item.crop,
                    grade,
                    audio: clip_audio,
                    output: request.output,
                },
                &item_output,
                cancellation,
                item_progress,
            )?;
            verify_reel_item(
                &rendered.output_probe,
                index,
                &plan,
                clip_audio == ClipAudio::Source,
            )?;
            let dimensions = (rendered.output_probe.width, rendered.output_probe.height);
            if let Some(expected) = expected_dimensions {
                if dimensions != expected {
                    return Err(unsupported(
                        "reel source-format normalization",
                        format!(
                            "item {index} rendered at {}x{}, but the first item is {}x{}; choose matching crops or a future fixed-format backend",
                            dimensions.0, dimensions.1, expected.0, expected.1
                        ),
                    ));
                }
            } else {
                expected_dimensions = Some(dimensions);
            }
            item_commands.push(rendered.command);

            // Source-audio items keep their audio for the assembly decode; the concat
            // demuxer consumes a video-only copy so item offsets stay on frame boundaries.
            let concat_input = if validated.audio == SupportedAudio::Source {
                let video_only = private_render
                    .path()
                    .join(format!("item-{index:06}-video.{extension}"));
                let remux_progress = |value: Progress| {
                    progress(Progress {
                        out_time_s: completed_before,
                        percent: (50.0 * (index as f64 + value.percent / 100.0)
                            / item_count as f64)
                            .clamp(0.0, 50.0),
                    });
                };
                let command = self.remux_video_only_with_control(
                    &item_output,
                    &video_only,
                    cancellation,
                    remux_progress,
                )?;
                let remux_probe = self.probe(&video_only)?;
                verify_reel_video_copy(
                    &remux_probe.value,
                    index,
                    &plan,
                    (dimensions.0, dimensions.1),
                )?;
                video_remux_commands.push(command);
                video_only
            } else {
                item_output.clone()
            };
            concat_paths.push(concat_input);
            item_paths.push(item_output);
            item_verifications.push(ReelItemVerification {
                index,
                source_path: item.source_path.clone(),
                in_s: item.in_s,
                out_s: item.out_s,
                fps: source_probe.fps,
                first_source_frame: plan.first_source_frame,
                last_source_frame: plan.first_source_frame + plan.frame_count - 1,
                requested_frame_count: plan.frame_count,
                rendered_frame_count: rendered.output_probe.video_frame_count.unwrap_or_default(),
                video_duration_s: plan.video_duration_s,
                audio_duration_s: rendered.output_probe.audio_duration_s,
            });
            completed_duration += plan.video_duration_s;
            completed_frames += plan.frame_count;
        }

        let concat_list = private_render.path().join("items.ffconcat");
        let mut concat_document = String::from("ffconcat version 1.0\n");
        for item_path in &concat_paths {
            let name = item_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Error::InvalidArgument("reel item path is not UTF-8".to_owned()))?;
            concat_document.push_str(&format!("file '{name}'\n"));
        }
        fs::write(&concat_list, concat_document)?;

        // Assembly: the video track is stream-copied through the concat demuxer over the
        // video-only items, so every cut lands exactly at the previous item's video duration.
        // Source audio is decoded from the full items, trimmed of the encoder-priming frames
        // that live at negative raw timestamps, joined with the concat filter, and encoded
        // once; the MP4 edit list then presents it from zero without shifting the video.
        // Re-encoding the reel audio is the documented TASK-036 decision: stream-copying it
        // would carry each item's AAC priming packet, and the concat demuxer's per-file
        // timestamp normalization turns that priming into the reel-wide head dead zone and
        // cut drift the 021 review rejected.
        let rendered = private_render.path().join(format!("rendered.{extension}"));
        let mut arguments = vec![
            OsString::from("-n"),
            OsString::from("-threads"),
            OsString::from(self.threads().to_string()),
            OsString::from("-f"),
            OsString::from("concat"),
            OsString::from("-safe"),
            OsString::from("1"),
            OsString::from("-i"),
            concat_list.as_os_str().to_owned(),
        ];
        if validated.audio == SupportedAudio::Source {
            for item_path in &item_paths {
                arguments.push(OsString::from("-i"));
                arguments.push(item_path.as_os_str().to_owned());
            }
        }
        arguments.push(OsString::from("-t"));
        // The frame-exact total (sum of per-item frame durations), not the raw requested
        // sum: for intervals that are not frame boundaries the delivered frame count is
        // the contract, and the output cap must never shave a delivered frame.
        arguments.push(OsString::from(format!("{:.9}", completed_duration)));
        arguments.push(OsString::from("-map"));
        arguments.push(OsString::from("0:v:0"));
        if validated.audio == SupportedAudio::Source {
            let mut filter = String::new();
            for index in 0..item_count {
                filter.push_str(&format!("[{}:a]atrim=start=0[a{index}];", index + 1));
            }
            if item_count == 1 {
                // A single item needs no concat filter, only the priming trim.
                filter.push_str("[a0]anull[aout]");
            } else {
                let joined = (0..item_count)
                    .map(|index| format!("[a{index}]"))
                    .collect::<String>();
                filter.push_str(&format!("{joined}concat=n={item_count}:v=0:a=1[aout]"));
            }
            arguments.push(OsString::from("-filter_complex"));
            arguments.push(OsString::from(filter));
            arguments.push(OsString::from("-map"));
            arguments.push(OsString::from("[aout]"));
            arguments.push(OsString::from("-c:v"));
            arguments.push(OsString::from("copy"));
            arguments.push(OsString::from("-c:a"));
            arguments.push(OsString::from("aac"));
            arguments.push(OsString::from("-b:a"));
            arguments.push(OsString::from("192k"));
        } else {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from("copy"));
            arguments.push(OsString::from("-an"));
        }
        arguments.extend([
            OsString::from("-map_metadata"),
            OsString::from("-1"),
            OsString::from("-map_chapters"),
            OsString::from("-1"),
            OsString::from("-movflags"),
            OsString::from("+faststart"),
            OsString::from("-f"),
            OsString::from(output_muxer(request.output)),
            OsString::from("-progress"),
            OsString::from("pipe:1"),
            OsString::from("-nostats"),
            rendered.as_os_str().to_owned(),
        ]);
        let mut assembly_progress = |value: Progress| {
            progress(Progress {
                out_time_s: value.out_time_s.min(completed_duration),
                percent: (50.0 + value.percent * 0.5).clamp(50.0, 100.0),
            });
        };
        let command = self.run_ffmpeg_progress_args(
            arguments,
            completed_duration,
            cancellation,
            &mut assembly_progress,
        )?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled { command });
        }
        let measured = self.probe(&rendered)?;
        let output_probe = measured.value;
        let video_frame_count = output_probe.video_frame_count.unwrap_or_default();
        let video_duration_s = output_probe
            .video_duration_s
            .unwrap_or(output_probe.duration_s);
        verify_reel_render(
            &output_probe,
            completed_duration,
            completed_frames,
            expected_dimensions.expect("validated reels contain at least one item"),
            validated.audio == SupportedAudio::Source,
            request.items.len(),
        )?;
        fs::File::open(&rendered)?.sync_all()?;
        fs::hard_link(&rendered, staging_output)?;

        Ok(ReelRenderResult {
            command,
            item_commands,
            video_remux_commands,
            backend: ReelRenderBackend::VideoToolboxConcatDemuxer,
            encoder: "h264_videotoolbox",
            preset: request.output.as_str(),
            requested_duration_s: completed_duration,
            output_probe,
            probe_command: measured.command,
            item_verifications,
            video_frame_count,
            video_duration_s,
        })
    }
}

fn validate_supported_request(request: &ResolvedReelRequest) -> Result<ValidatedReel> {
    if request.items.is_empty() {
        return Err(Error::InvalidArgument(
            "reel requires at least one item".to_owned(),
        ));
    }
    validate_unit_gain(request.master_volume, "reel master_volume")?;
    if !approximately(request.master_volume, 1.0) {
        return Err(unsupported(
            "reel master-volume mix",
            "schema v1 supports master_volume=1 only",
        ));
    }
    if request.format != ReelFormat::Source {
        return Err(unsupported(
            "fixed reel format",
            "schema v1 supports Source format only; it will not infer a crop/pad policy",
        ));
    }
    if request.music.is_some() {
        return Err(unsupported(
            "reel music mix",
            "music was resolved but schema v1 has no approved mix, ducking, or ending policy",
        ));
    }
    if request.watermark.is_some() {
        return Err(unsupported(
            "reel watermark",
            "watermark was resolved but schema v1 has no approved scale/margin policy",
        ));
    }
    if request.cover.is_some() {
        return Err(unsupported(
            "reel cover extraction",
            "cover selection was resolved but schema v1 does not yet publish a companion cover asset",
        ));
    }

    let mut duration_s = 0.0;
    let mut audio = None;
    let mut grades = Vec::with_capacity(request.items.len());
    for (index, item) in request.items.iter().enumerate() {
        if !item.source_path.is_absolute() {
            return Err(Error::InvalidArgument(format!(
                "reel item {index} source_path must be absolute"
            )));
        }
        if item.media_kind != ReelMediaKind::Video {
            return Err(unsupported(
                "photo holds",
                format!("reel item {index} is a photo; schema v1 renders video items only"),
            ));
        }
        if !item.in_s.is_finite()
            || !item.out_s.is_finite()
            || item.in_s < 0.0
            || item.out_s <= item.in_s
        {
            return Err(Error::InvalidArgument(format!(
                "reel item {index} range must be finite with 0 <= in_s < out_s"
            )));
        }
        validate_crop(item.crop, &format!("reel item {index} crop"))?;
        for (keyframe_index, keyframe) in item.crop_keyframes.iter().enumerate() {
            if !keyframe.time_s.is_finite()
                || keyframe.time_s < item.in_s
                || keyframe.time_s > item.out_s
            {
                return Err(Error::InvalidArgument(format!(
                    "reel item {index} crop keyframe {keyframe_index} must have a finite source time inside the item range"
                )));
            }
            validate_crop(
                Some(keyframe.crop),
                &format!("reel item {index} crop keyframe {keyframe_index}"),
            )?;
        }
        if !item.crop_keyframes.is_empty() {
            return Err(unsupported(
                "animated reel crop keyframes",
                format!("reel item {index} declares crop keyframes"),
            ));
        }
        if let Some(caption) = &item.caption {
            if caption.text.trim().is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "reel item {index} caption text cannot be empty"
                )));
            }
            return Err(unsupported(
                "reel captions",
                format!("reel item {index} declares an on-screen caption"),
            ));
        }
        if !item.transition.duration_s.is_finite() || item.transition.duration_s < 0.0 {
            return Err(Error::InvalidArgument(format!(
                "reel item {index} transition duration must be finite and non-negative"
            )));
        }
        if item.transition.kind != ReelTransitionKind::Cut
            || !approximately(item.transition.duration_s, 0.0)
        {
            return Err(unsupported(
                "reel transition filter",
                format!(
                    "reel item {index} requests {:?} over {:.3}s; schema v1 supports zero-duration cuts only",
                    item.transition.kind, item.transition.duration_s
                ),
            ));
        }
        if !item.speed.is_finite() || !(0.5..=2.0).contains(&item.speed) {
            return Err(Error::InvalidArgument(format!(
                "reel item {index} speed must be finite and between 0.5 and 2"
            )));
        }
        if !approximately(item.speed, 1.0) {
            return Err(unsupported(
                "reel speed filter",
                format!("reel item {index} requests speed {}", item.speed),
            ));
        }
        if item.motion != ReelMotion::None {
            return Err(unsupported(
                "reel motion filter",
                format!("reel item {index} requests {:?} motion", item.motion),
            ));
        }
        validate_unit_gain(item.volume, &format!("reel item {index} volume"))?;
        let item_audio = if approximately(item.volume, 0.0) {
            SupportedAudio::Mute
        } else if approximately(item.volume, 1.0) {
            SupportedAudio::Source
        } else {
            return Err(unsupported(
                "reel clip-volume mix",
                format!(
                    "reel item {index} requests volume {}; schema v1 supports 0 or 1 only",
                    item.volume
                ),
            ));
        };
        if audio.is_some_and(|audio| audio != item_audio) {
            return Err(unsupported(
                "mixed reel audio topology",
                "schema v1 requires every item to use source audio or every item to be muted",
            ));
        }
        audio = Some(item_audio);
        grades.push(validate_grade(item.grade, index)?);
        duration_s += item.out_s - item.in_s;
    }

    Ok(ValidatedReel {
        duration_s,
        audio: audio.expect("validated reels contain at least one item"),
        grades,
    })
}

fn validate_grade(grade: ResolvedReelGrade, item_index: usize) -> Result<VideoGrade> {
    let values = [
        grade.exposure_ev,
        grade.contrast,
        grade.saturation,
        grade.temperature,
        grade.tint,
        grade.hue_degrees,
        grade.vibrance,
        grade.shadows,
        grade.highlights,
    ];
    if !values.iter().all(|value| value.is_finite()) {
        return Err(Error::InvalidArgument(format!(
            "reel item {item_index} grade values must be finite"
        )));
    }
    if !(-5.0..=5.0).contains(&grade.exposure_ev)
        || !(-1.0..=1.0).contains(&grade.contrast)
        || !(0.0..=2.0).contains(&grade.saturation)
        || !(-1.0..=1.0).contains(&grade.temperature)
        || !(-1.0..=1.0).contains(&grade.tint)
        || !(-180.0..=180.0).contains(&grade.hue_degrees)
        || !(-1.0..=1.0).contains(&grade.vibrance)
        || !(-1.0..=1.0).contains(&grade.shadows)
        || !(-1.0..=1.0).contains(&grade.highlights)
    {
        return Err(Error::InvalidArgument(format!(
            "reel item {item_index} grade is outside the declared normalized ranges"
        )));
    }
    if !approximately(grade.hue_degrees, 0.0)
        || !approximately(grade.vibrance, 0.0)
        || !approximately(grade.shadows, 0.0)
        || !approximately(grade.highlights, 0.0)
    {
        return Err(unsupported(
            "extended reel grade filters",
            format!("reel item {item_index} declares hue, vibrance, shadows, or highlights"),
        ));
    }
    if approximately(grade.exposure_ev, 0.0)
        && approximately(grade.contrast, 0.0)
        && approximately(grade.saturation, 1.0)
        && approximately(grade.temperature, 0.0)
        && approximately(grade.tint, 0.0)
    {
        Ok(VideoGrade::None)
    } else {
        Ok(VideoGrade::Basic(BasicVideoGrade {
            exposure_ev: grade.exposure_ev,
            contrast: grade.contrast,
            saturation: grade.saturation,
            temperature: grade.temperature,
            tint: grade.tint,
        }))
    }
}

fn validate_crop(crop: Option<NormalizedVideoCrop>, label: &str) -> Result<()> {
    let Some(crop) = crop else {
        return Ok(());
    };
    let values = [crop.x, crop.y, crop.width, crop.height];
    if !values.iter().all(|value| value.is_finite()) {
        return Err(Error::InvalidArgument(format!(
            "{label} values must be finite"
        )));
    }
    if crop.x < 0.0
        || crop.y < 0.0
        || crop.width <= 0.0
        || crop.height <= 0.0
        || crop.x + crop.width > 1.0
        || crop.y + crop.height > 1.0
    {
        return Err(Error::InvalidArgument(format!(
            "{label} must be a positive normalized rectangle inside source bounds"
        )));
    }
    Ok(())
}

fn validate_unit_gain(value: f64, label: &str) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(Error::InvalidArgument(format!(
            "{label} must be finite and between 0 and 1"
        )));
    }
    Ok(())
}

/// Two microseconds: far below any real edit granularity, but above the microsecond
/// rounding FFmpeg applies when parsing `-ss`, so an in point that sits exactly on a frame
/// boundary never rounds past the frame it requested.
const SEEK_BOUNDARY_EPSILON_S: f64 = 0.000_002;

/// Timestamp tolerance for exact stream facts. MP4 durations are exact to the track
/// timebase; two milliseconds absorbs probe rounding while staying far below one frame.
const STREAM_DURATION_TOLERANCE_S: f64 = 0.002;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ItemFramePlan {
    in_s: f64,
    out_s: f64,
    fps: f64,
    /// First source frame at or after `in_s`: the frame the item must start with.
    first_source_frame: i64,
    /// Exact output frame count: `round((out_s - in_s) * fps)`.
    frame_count: i64,
    /// The item's exact video duration: `frame_count / fps`.
    video_duration_s: f64,
    /// Input-side seek target: `in_s` minus the boundary epsilon.
    seek_s: f64,
    /// Input-side read window: video duration plus decode slack for the final frames.
    read_s: f64,
}

/// The TASK-036 frame-math rule. For an item requesting source interval `[in_s, out_s)`:
/// the item delivers `round((out_s - in_s) * fps)` frames, starting at the first source
/// frame at or after `in_s`. In and out points that sit on frame boundaries therefore
/// yield exactly the requested frames — no dropped tail frame, no lead dead zone.
fn plan_item_frames(in_s: f64, out_s: f64, fps: f64) -> Result<ItemFramePlan> {
    if !fps.is_finite() || fps <= 0.0 {
        return Err(Error::InvalidProbe(
            "reel item source reports no usable frame rate".to_owned(),
        ));
    }
    let frame_count = ((out_s - in_s) * fps).round() as i64;
    if frame_count < 1 {
        return Err(Error::InvalidArgument(format!(
            "reel item {in_s:.6}-{out_s:.6}s at {fps:.6} fps yields no frames"
        )));
    }
    let first_source_frame = ((in_s - SEEK_BOUNDARY_EPSILON_S) * fps).ceil() as i64;
    let video_duration_s = frame_count as f64 / fps;
    Ok(ItemFramePlan {
        in_s,
        out_s,
        fps,
        first_source_frame,
        frame_count,
        video_duration_s,
        seek_s: (in_s - SEEK_BOUNDARY_EPSILON_S).max(0.0),
        read_s: video_duration_s + 3.0 / fps,
    })
}

/// Verify one rendered reel item against its frame plan: exact video frame count, exact
/// video stream duration, and audio that never outlasts the video.
fn verify_reel_item(
    output: &Probe,
    index: usize,
    plan: &ItemFramePlan,
    expected_audio: bool,
) -> Result<()> {
    let rendered_frames = output.video_frame_count.ok_or_else(|| {
        Error::InvalidProbe(format!(
            "reel item {index} reports no video frame count; boundary exactness cannot be verified"
        ))
    })?;
    if rendered_frames != plan.frame_count {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} rendered {rendered_frames} video frames, expected exactly {} for \
             source {:.6}-{:.6}s at {:.6} fps",
            plan.frame_count, plan.in_s, plan.out_s, plan.fps
        )));
    }
    let video_duration = output.video_duration_s.ok_or_else(|| {
        Error::InvalidProbe(format!(
            "reel item {index} reports no video stream duration"
        ))
    })?;
    if (video_duration - plan.video_duration_s).abs() > STREAM_DURATION_TOLERANCE_S {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} video stream duration is {video_duration:.6}s, expected exactly {:.6}s",
            plan.video_duration_s
        )));
    }
    if output.has_audio != expected_audio {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} audio presence is {}, expected {}",
            output.has_audio, expected_audio
        )));
    }
    if let Some(audio_duration) = output.audio_duration_s {
        if audio_duration > plan.video_duration_s + STREAM_DURATION_TOLERANCE_S {
            return Err(Error::InvalidProbe(format!(
                "reel item {index} audio stream lasts {audio_duration:.6}s, longer than its {:.6}s video; \
                 reel items trim audio to the video duration",
                plan.video_duration_s
            )));
        }
    }
    Ok(())
}

/// Verify the video-only concat copy of an item: same frames, same duration, no audio.
fn verify_reel_video_copy(
    output: &Probe,
    index: usize,
    plan: &ItemFramePlan,
    expected_dimensions: (u32, u32),
) -> Result<()> {
    if output.has_audio {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} video-only copy still carries an audio stream"
        )));
    }
    if (output.width, output.height) != expected_dimensions {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} video-only copy is {}x{}, expected {}x{}",
            output.width, output.height, expected_dimensions.0, expected_dimensions.1
        )));
    }
    let frames = output.video_frame_count.unwrap_or_default();
    if frames != plan.frame_count {
        return Err(Error::InvalidProbe(format!(
            "reel item {index} video-only copy has {frames} frames, expected {}",
            plan.frame_count
        )));
    }
    if let Some(duration) = output.video_duration_s {
        if (duration - plan.video_duration_s).abs() > STREAM_DURATION_TOLERANCE_S {
            return Err(Error::InvalidProbe(format!(
                "reel item {index} video-only copy duration is {duration:.6}s, expected {:.6}s",
                plan.video_duration_s
            )));
        }
    }
    Ok(())
}

fn verify_reel_render(
    output: &Probe,
    expected_duration_s: f64,
    expected_frame_count: i64,
    expected_dimensions: (u32, u32),
    expected_audio: bool,
    item_count: usize,
) -> Result<()> {
    if output.video_codec.as_deref() != Some("h264") {
        return Err(Error::InvalidProbe(format!(
            "reel output codec is {:?}, expected H.264",
            output.video_codec
        )));
    }
    if (output.width, output.height) != expected_dimensions {
        return Err(Error::InvalidProbe(format!(
            "reel output dimensions are {}x{}, expected {}x{}",
            output.width, output.height, expected_dimensions.0, expected_dimensions.1
        )));
    }
    if output.pixel_format.as_deref() != Some("yuv420p") || output.bit_depth != Some(8) {
        return Err(Error::InvalidProbe(format!(
            "reel output pixel format/depth is {:?}/{:?}, expected yuv420p/8-bit",
            output.pixel_format, output.bit_depth
        )));
    }
    // TASK-036: count the VIDEO stream, not the container. Audio padding previously hid
    // missing video frames behind a passing container duration (fps 28.77 symptom).
    let rendered_frames = output.video_frame_count.ok_or_else(|| {
        Error::InvalidProbe(
            "reel output reports no video frame count; boundary exactness cannot be verified"
                .to_owned(),
        )
    })?;
    if rendered_frames != expected_frame_count {
        return Err(Error::InvalidProbe(format!(
            "reel output has {rendered_frames} video frames, expected exactly {expected_frame_count} \
             across {item_count} items"
        )));
    }
    let video_duration = output.video_duration_s.ok_or_else(|| {
        Error::InvalidProbe("reel output reports no video stream duration".to_owned())
    })?;
    if (video_duration - expected_duration_s).abs() > STREAM_DURATION_TOLERANCE_S {
        return Err(Error::InvalidProbe(format!(
            "reel output video stream duration is {video_duration:.6}s, expected exactly \
             {expected_duration_s:.6}s so every cut lands on a frame boundary"
        )));
    }
    if output.has_audio != expected_audio {
        return Err(Error::InvalidProbe(format!(
            "reel output audio presence is {}, expected {}",
            output.has_audio, expected_audio
        )));
    }
    if let Some(audio_duration) = output.audio_duration_s {
        if audio_duration > video_duration + STREAM_DURATION_TOLERANCE_S {
            return Err(Error::InvalidProbe(format!(
                "reel audio stream lasts {audio_duration:.6}s, longer than the {video_duration:.6}s \
                 video; tail audio must never play over a frozen last frame"
            )));
        }
    }
    for (label, actual) in [
        ("primaries", output.color_primaries.as_deref()),
        ("transfer", output.color_transfer.as_deref()),
        ("matrix", output.color_space.as_deref()),
    ] {
        if actual != Some("bt709") {
            return Err(Error::InvalidProbe(format!(
                "reel output {label} is {actual:?}, expected bt709"
            )));
        }
    }
    Ok(())
}

fn reject_existing_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(Error::InvalidArgument(format!(
            "reel staging destination already exists; choose a new private staging path: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn unsupported(capability: impl Into<String>, reason: impl Into<String>) -> Error {
    Error::CapabilityUnavailable {
        capability: capability.into(),
        reason: reason.into(),
    }
}

fn approximately(left: f64, right: f64) -> bool {
    (left - right).abs() <= EPSILON
}

const fn output_extension(output: ClipOutputPreset) -> &'static str {
    match output {
        ClipOutputPreset::Mp4H264SdrV1 => "mp4",
        ClipOutputPreset::MovH264SdrV1 => "mov",
    }
}

const fn output_muxer(output: ClipOutputPreset) -> &'static str {
    match output {
        ClipOutputPreset::Mp4H264SdrV1 => "mp4",
        ClipOutputPreset::MovH264SdrV1 => "mov",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_math_delivers_the_requested_frames_from_the_requested_first_frame() {
        // The 021 review's golden interval: 0.25-1.25s at 30 fps is frames 8-37.
        let plan = plan_item_frames(0.25, 1.25, 30.0).unwrap();
        assert_eq!(plan.first_source_frame, 8);
        assert_eq!(plan.frame_count, 30);
        assert!((plan.video_duration_s - 1.0).abs() <= 1e-9);
        assert!((plan.seek_s - 0.249998).abs() <= 1e-9);

        // An in point exactly on a frame boundary must keep that frame, not round past it.
        let boundary = plan_item_frames(8.0 / 30.0, 38.0 / 30.0, 30.0).unwrap();
        assert_eq!(boundary.first_source_frame, 8);
        assert_eq!(boundary.frame_count, 30);
        assert_eq!(boundary.first_source_frame + boundary.frame_count - 1, 37);

        // In at zero starts at frame zero.
        let head = plan_item_frames(0.0, 1.0, 30.0).unwrap();
        assert_eq!(head.first_source_frame, 0);
        assert_eq!(head.frame_count, 30);

        // Fractional frame rates keep the same rule.
        let ntsc = plan_item_frames(0.25, 1.25, 30_000.0 / 1001.0).unwrap();
        assert_eq!(ntsc.frame_count, 30);
        assert_eq!(ntsc.first_source_frame, 8);

        // An interval shorter than one frame is a contract violation, not a rounding guess.
        assert!(plan_item_frames(1.0, 1.01, 30.0).is_err());
        assert!(plan_item_frames(1.0, 2.0, 0.0).is_err());
    }

    fn item() -> ResolvedReelItem {
        ResolvedReelItem {
            source_path: PathBuf::from("/library/clip.mp4"),
            media_kind: ReelMediaKind::Video,
            in_s: 1.0,
            out_s: 3.0,
            crop: None,
            crop_keyframes: Vec::new(),
            caption: None,
            transition: ResolvedReelTransition::default(),
            speed: 1.0,
            motion: ReelMotion::None,
            volume: 0.0,
            grade: ResolvedReelGrade::default(),
        }
    }

    fn base_request() -> ResolvedReelRequest {
        ResolvedReelRequest {
            items: vec![item()],
            format: ReelFormat::Source,
            music: None,
            master_volume: 1.0,
            watermark: None,
            cover: None,
            output: ClipOutputPreset::Mp4H264SdrV1,
        }
    }

    #[test]
    fn validates_the_initial_ordered_video_subset() {
        let validated = validate_supported_request(&base_request()).unwrap();
        assert_eq!(validated.duration_s, 2.0);
        assert_eq!(validated.audio, SupportedAudio::Mute);
        assert_eq!(validated.grades, vec![VideoGrade::None]);
    }

    #[test]
    fn rejects_populated_recipe_intent_instead_of_dropping_it() {
        let mut request = base_request();
        request.items[0].caption = Some(ResolvedReelCaption {
            text: "Keep this".to_owned(),
            position: ReelCaptionPosition::Low,
        });
        let error = validate_supported_request(&request).unwrap_err();
        assert!(matches!(error, Error::CapabilityUnavailable { .. }));
        assert!(error.to_string().contains("captions"));

        let mut request = base_request();
        request.items[0].transition = ResolvedReelTransition {
            kind: ReelTransitionKind::Mix,
            duration_s: 0.4,
        };
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("transition filter"));

        let mut request = base_request();
        request.items[0].grade.vibrance = 0.2;
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("extended reel grade"));
    }

    #[test]
    fn rejects_mixed_or_fractional_natural_audio() {
        let mut request = base_request();
        request.items.push(item());
        request.items[1].volume = 1.0;
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("mixed reel audio topology"));

        let mut request = base_request();
        request.items[0].volume = 0.5;
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("clip-volume mix"));
    }

    #[test]
    fn rejects_photo_and_fixed_format_without_inference() {
        let mut request = base_request();
        request.items[0].media_kind = ReelMediaKind::Photo;
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("photo holds"));

        let mut request = base_request();
        request.format = ReelFormat::Portrait9x16;
        let error = validate_supported_request(&request).unwrap_err();
        assert!(error.to_string().contains("fixed reel format"));
    }
}
