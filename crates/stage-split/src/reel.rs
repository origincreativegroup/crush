//! Backend-neutral resolved reel recipes and the first ordered FFmpeg renderer.
//!
//! Reel Studio's recipe vocabulary is represented here even where the first backend cannot yet
//! execute it. Validation rejects every unsupported non-default value before rendering so creative
//! intent is never silently omitted.

use crate::ffmpeg::{
    BasicVideoGrade, CancellationToken, ClipAudio, ClipOutputPreset, ClipRenderRequest,
    ClipTransition, Error, NormalizedVideoCrop, Probe, Progress, Result, Runner, VideoGrade,
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
    pub backend: ReelRenderBackend,
    pub encoder: &'static str,
    pub preset: &'static str,
    pub requested_duration_s: f64,
    pub output_probe: Probe,
    pub probe_command: String,
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

        // Source-audio concat requires a uniform stream topology. Silence insertion and a true
        // audio mix graph are separate capabilities and must not be inferred here.
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
            }
        }

        let mut item_commands = Vec::with_capacity(request.items.len());
        let mut item_paths = Vec::with_capacity(request.items.len());
        let mut completed_duration = 0.0;
        let mut expected_dimensions = None;
        for (index, (item, grade)) in request
            .items
            .iter()
            .zip(validated.grades.iter().copied())
            .enumerate()
        {
            let item_output = private_render
                .path()
                .join(format!("item-{index:06}.{extension}"));
            let item_duration = item.out_s - item.in_s;
            let completed_before = completed_duration;
            let mut item_progress = |value: Progress| {
                let encoded = (completed_before + value.out_time_s).min(validated.duration_s);
                progress(Progress {
                    out_time_s: encoded,
                    percent: (encoded / (validated.duration_s * 2.0) * 100.0).clamp(0.0, 50.0),
                });
            };
            let rendered = self.render_clip_with_control(
                &item.source_path,
                &ClipRenderRequest {
                    in_s: item.in_s,
                    out_s: item.out_s,
                    crop: item.crop,
                    grade,
                    transition: ClipTransition::Cut,
                    audio: clip_audio,
                    output: request.output,
                },
                &item_output,
                cancellation,
                &mut item_progress,
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
            item_paths.push(item_output);
            completed_duration += item_duration;
        }

        let concat_list = private_render.path().join("items.ffconcat");
        let mut concat_document = String::from("ffconcat version 1.0\n");
        for item_path in &item_paths {
            let name = item_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Error::InvalidArgument("reel item path is not UTF-8".to_owned()))?;
            concat_document.push_str(&format!("file '{name}'\n"));
        }
        fs::write(&concat_list, concat_document)?;

        let rendered = private_render.path().join(format!("rendered.{extension}"));
        let arguments = vec![
            OsString::from("-n"),
            OsString::from("-threads"),
            OsString::from(self.threads().to_string()),
            OsString::from("-f"),
            OsString::from("concat"),
            OsString::from("-safe"),
            OsString::from("1"),
            OsString::from("-i"),
            concat_list.as_os_str().to_owned(),
            OsString::from("-map"),
            OsString::from("0:v:0"),
            OsString::from("-map"),
            OsString::from(if validated.audio == SupportedAudio::Source {
                "0:a:0"
            } else {
                "0:a?"
            }),
            OsString::from("-c"),
            OsString::from("copy"),
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
        ];
        let mut assembly_progress = |value: Progress| {
            progress(Progress {
                out_time_s: value.out_time_s.min(validated.duration_s),
                percent: (50.0 + value.percent * 0.5).clamp(50.0, 100.0),
            });
        };
        let command = self.run_ffmpeg_progress_args(
            arguments,
            validated.duration_s,
            cancellation,
            &mut assembly_progress,
        )?;
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled { command });
        }
        let measured = self.probe(&rendered)?;
        verify_reel_render(
            &measured.value,
            validated.duration_s,
            expected_dimensions.expect("validated reels contain at least one item"),
            validated.audio == SupportedAudio::Source,
            request.items.len(),
        )?;
        fs::File::open(&rendered)?.sync_all()?;
        fs::hard_link(&rendered, staging_output)?;

        Ok(ReelRenderResult {
            command,
            item_commands,
            backend: ReelRenderBackend::VideoToolboxConcatDemuxer,
            encoder: "h264_videotoolbox",
            preset: request.output.as_str(),
            requested_duration_s: validated.duration_s,
            output_probe: measured.value,
            probe_command: measured.command,
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

fn verify_reel_render(
    output: &Probe,
    expected_duration_s: f64,
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
    let frame_tolerance = if output.fps > 0.0 {
        1.0 / output.fps
    } else {
        1.0 / 30.0
    };
    let duration_tolerance = 0.05 + frame_tolerance * item_count as f64;
    if (output.duration_s - expected_duration_s).abs() > duration_tolerance {
        return Err(Error::InvalidProbe(format!(
            "reel output duration {:.6}s differs from requested {:.6}s",
            output.duration_s, expected_duration_s
        )));
    }
    if output.has_audio != expected_audio {
        return Err(Error::InvalidProbe(format!(
            "reel output audio presence is {}, expected {}",
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
