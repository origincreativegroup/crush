//! Content-based hard-cut detection matching the Python answer-key contract.

use crate::ffmpeg::Runner;
use anyhow::{ensure, Context};
use crush_core::config::SplitConfig;
use crush_store::{Shot, Store, VideoStatus};
use image::{ImageReader, RgbImage};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

pub type FramePath = PathBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameScore {
    pub time_s: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotSpan {
    pub start_s: f64,
    pub end_s: f64,
    pub rep_frame_s: f64,
    pub scene_score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub shots: Vec<ShotSpan>,
    pub scores: Vec<FrameScore>,
}

/// Blueprint-compatible convenience entrypoint.
///
/// Product code should call [`detect_checked`] so unreadable frames are returned as errors.
pub fn detect(frames: &[FramePath], fps: f32, config: &SplitConfig) -> Vec<ShotSpan> {
    detect_checked(frames, fps, config)
        .expect("sampled scene frames must be readable and consistently sized")
        .shots
}

pub fn detect_checked(
    frames: &[FramePath],
    fps: f32,
    config: &SplitConfig,
) -> anyhow::Result<Detection> {
    let duration_s = frames.len() as f64 / f64::from(fps);
    detect_with_duration(frames, fps, duration_s, config)
}

pub fn detect_with_duration(
    frames: &[FramePath],
    fps: f32,
    duration_s: f64,
    config: &SplitConfig,
) -> anyhow::Result<Detection> {
    validate_inputs(frames, fps, duration_s, config)?;
    if frames.is_empty() {
        return Ok(Detection {
            shots: Vec::new(),
            scores: Vec::new(),
        });
    }

    let mut previous: Option<Vec<Hsv>> = None;
    let mut dimensions = None;
    let mut scores = Vec::with_capacity(frames.len());
    let mut cuts = Vec::new();
    let mut last_cut_s = 0.0;
    let mut threshold_run = None;
    let threshold = f64::from(config.threshold);
    let transition_floor = threshold * 0.9;
    let min_scene_len_s = f64::from(config.min_scene_len_s);

    for (index, path) in frames.iter().enumerate() {
        let image = load_rgb(path)?;
        let current_dimensions = image.dimensions();
        if let Some(expected) = dimensions {
            ensure!(
                current_dimensions == expected,
                "sampled frame {} is {}x{}, expected {}x{}",
                path.display(),
                current_dimensions.0,
                current_dimensions.1,
                expected.0,
                expected.1
            );
        } else {
            dimensions = Some(current_dimensions);
        }

        let hsv = convert_hsv(&image);
        let score = previous
            .as_ref()
            .map_or(0.0, |previous| mean_content_delta(previous, &hsv));
        let time_s = index as f64 / f64::from(fps);
        scores.push(FrameScore { time_s, score });
        if score >= threshold {
            // A multi-frame threshold run is one visual transition, not a new hard cut on
            // every sampled frame. If the run begins inside the minimum shot length, keep
            // considering it until it can legally open a shot.
            let run = threshold_run.get_or_insert_with(|| ThresholdRun::new(score));
            run.observe(score);
            if !run.cut_emitted
                && time_s - last_cut_s + f64::EPSILON >= min_scene_len_s
                && time_s < duration_s
            {
                cuts.push((time_s, score));
                last_cut_s = time_s;
                run.cut_emitted = true;
            }
        } else if score >= transition_floor && threshold_run.is_some() {
            threshold_run
                .as_mut()
                .expect("checked above")
                .observe(score);
        } else {
            // At low sample rates a monotonic fade can contain two full-rate cuts while only
            // its leading delta crosses the threshold. Recover the fade tail at the first
            // settled frame, but only for a sustained, strictly falling score run. This keeps
            // the HSV content score and hard-cut threshold unchanged.
            if threshold_run.as_ref().is_some_and(|run| {
                run.cut_emitted
                    && run.samples >= 3
                    && run.descending
                    && time_s - last_cut_s + f64::EPSILON >= min_scene_len_s
                    && time_s < duration_s
            }) {
                cuts.push((time_s, score));
                last_cut_s = time_s;
            }
            threshold_run = None;
        }
        previous = Some(hsv);
    }

    Ok(Detection {
        shots: build_shots(&cuts, duration_s, f64::from(config.rep_frame_pos)),
        scores,
    })
}

pub fn scores_csv(scores: &[FrameScore]) -> String {
    let mut csv = String::from("t_s,score\n");
    for frame in scores {
        writeln!(&mut csv, "{:.6},{:.6}", frame.time_s, frame.score)
            .expect("writing to a String cannot fail");
    }
    csv
}

pub fn write_scores_csv(path: &Path, scores: &[FrameScore]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, scores_csv(scores))
        .with_context(|| format!("failed to write scene scores to {}", path.display()))
}

/// Extract representative thumbnails and persist the corresponding shot rows.
pub fn materialize_shots(
    runner: &Runner,
    store: &mut Store,
    owner_id: &str,
    video_id: &str,
    input: &Path,
    spans: &[ShotSpan],
    thumbs_dir: &Path,
) -> anyhow::Result<Vec<Shot>> {
    ensure!(!spans.is_empty(), "cannot materialize an empty shot list");
    ensure!(
        video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "video id must be safe for thumbnail filenames"
    );
    fs::create_dir_all(thumbs_dir)?;

    let mut shots = Vec::with_capacity(spans.len());
    for (index, span) in spans.iter().enumerate() {
        let shot_id = format!("{video_id}-shot-{index:06}");
        let thumb_rel = format!("{shot_id}.jpg");
        runner
            .frame_at(input, span.rep_frame_s, &thumbs_dir.join(&thumb_rel))
            .with_context(|| format!("failed to extract thumbnail for shot {index}"))?;
        shots.push(Shot {
            id: shot_id,
            video_id: video_id.to_owned(),
            owner_id: owner_id.to_owned(),
            idx: i64::try_from(index).context("shot index exceeded i64")?,
            start_s: span.start_s,
            end_s: span.end_s,
            rep_frame_s: span.rep_frame_s,
            thumb_rel: Some(thumb_rel),
            scene_score: Some(span.scene_score),
        });
    }
    store.insert_shots(owner_id, &shots)?;
    store.set_video_status(owner_id, video_id, VideoStatus::Split)?;
    Ok(shots)
}

fn validate_inputs(
    frames: &[FramePath],
    fps: f32,
    duration_s: f64,
    config: &SplitConfig,
) -> anyhow::Result<()> {
    ensure!(
        fps.is_finite() && fps > 0.0,
        "scene fps must be finite and positive"
    );
    if !frames.is_empty() {
        ensure!(
            duration_s.is_finite() && duration_s > 0.0,
            "scene duration must be finite and positive"
        );
    }
    ensure!(
        config.threshold.is_finite() && config.threshold >= 0.0,
        "scene threshold must be finite and non-negative"
    );
    ensure!(
        config.min_scene_len_s.is_finite() && config.min_scene_len_s >= 0.0,
        "minimum scene length must be finite and non-negative"
    );
    ensure!(
        config.rep_frame_pos.is_finite() && (0.0..=1.0).contains(&config.rep_frame_pos),
        "representative-frame position must be between zero and one"
    );
    Ok(())
}

fn load_rgb(path: &Path) -> anyhow::Result<RgbImage> {
    ImageReader::open(path)
        .with_context(|| format!("failed to open sampled frame {}", path.display()))?
        .decode()
        .with_context(|| format!("failed to decode sampled frame {}", path.display()))
        .map(|image| image.into_rgb8())
}

#[derive(Debug, Clone, Copy)]
struct Hsv {
    hue: f32,
    saturation: f32,
    value: f32,
}

#[derive(Debug)]
struct ThresholdRun {
    previous_score: f64,
    samples: usize,
    descending: bool,
    cut_emitted: bool,
}

impl ThresholdRun {
    fn new(score: f64) -> Self {
        Self {
            previous_score: score,
            samples: 0,
            descending: true,
            cut_emitted: false,
        }
    }

    fn observe(&mut self, score: f64) {
        if self.samples > 0 && score > self.previous_score + f64::EPSILON {
            self.descending = false;
        }
        self.previous_score = score;
        self.samples += 1;
    }
}

fn convert_hsv(image: &RgbImage) -> Vec<Hsv> {
    image
        .pixels()
        .map(|pixel| rgb_to_hsv(pixel.0[0], pixel.0[1], pixel.0[2]))
        .collect()
}

fn rgb_to_hsv(red: u8, green: u8, blue: u8) -> Hsv {
    let red = f32::from(red);
    let green = f32::from(green);
    let blue = f32::from(blue);
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let delta = maximum - minimum;
    let hue_degrees = if delta == 0.0 {
        0.0
    } else if maximum == red {
        60.0 * ((green - blue) / delta).rem_euclid(6.0)
    } else if maximum == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    };
    Hsv {
        hue: hue_degrees / 360.0 * 255.0,
        saturation: if maximum == 0.0 {
            0.0
        } else {
            delta / maximum * 255.0
        },
        value: maximum,
    }
}

fn mean_content_delta(previous: &[Hsv], current: &[Hsv]) -> f64 {
    debug_assert_eq!(previous.len(), current.len());
    let (hue, saturation, value) = previous.iter().zip(current).fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(hue, saturation, value), (previous, current)| {
            (
                hue + f64::from((current.hue - previous.hue).abs()),
                saturation + f64::from((current.saturation - previous.saturation).abs()),
                value + f64::from((current.value - previous.value).abs()),
            )
        },
    );
    let pixels = previous.len() as f64;
    (hue / pixels + saturation / pixels + value / pixels) / 3.0
}

fn build_shots(cuts: &[(f64, f64)], duration_s: f64, rep_frame_pos: f64) -> Vec<ShotSpan> {
    let mut shots = Vec::with_capacity(cuts.len() + 1);
    let mut start_s = 0.0;
    let mut opening_score = 0.0;
    for &(cut_s, score) in cuts {
        shots.push(shot_span(start_s, cut_s, opening_score, rep_frame_pos));
        start_s = cut_s;
        opening_score = score;
    }
    shots.push(shot_span(start_s, duration_s, opening_score, rep_frame_pos));
    shots
}

fn shot_span(start_s: f64, end_s: f64, scene_score: f64, rep_frame_pos: f64) -> ShotSpan {
    ShotSpan {
        start_s,
        end_s,
        rep_frame_s: start_s + (end_s - start_s) * rep_frame_pos,
        scene_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid_frame(directory: &Path, index: usize, rgb: [u8; 3]) -> PathBuf {
        let path = directory.join(format!("frame-{index}.ppm"));
        RgbImage::from_pixel(4, 4, Rgb(rgb)).save(&path).unwrap();
        path
    }

    #[test]
    fn primary_and_secondary_colors_map_to_expected_hsv_scale() {
        let red = rgb_to_hsv(255, 0, 0);
        assert_eq!(red.value, 255.0);
        assert_eq!(red.saturation, 255.0);
        assert_eq!(red.hue, 0.0);
        let green = rgb_to_hsv(0, 255, 0);
        assert!((green.hue - 85.0).abs() < 1e-5);
        let blue = rgb_to_hsv(0, 0, 255);
        assert!((blue.hue - 170.0).abs() < 1e-5);
        let cyan = rgb_to_hsv(0, 255, 255);
        assert!((cyan.hue - 127.5).abs() < 1e-5);
    }

    #[test]
    fn shot_builder_preserves_opening_score_and_rep_position() {
        let shots = build_shots(&[(2.0, 42.0), (5.0, 30.0)], 8.0, 0.4);
        assert_eq!(shots.len(), 3);
        assert_eq!(shots[0].scene_score, 0.0);
        assert_eq!(shots[1].scene_score, 42.0);
        assert_eq!(shots[2].scene_score, 30.0);
        assert!((shots[1].rep_frame_s - 3.2).abs() < 1e-12);
    }

    #[test]
    fn detector_applies_the_hsv_formula_and_minimum_scene_length() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = [
            solid_frame(temporary.path(), 0, [0, 0, 0]),
            solid_frame(temporary.path(), 1, [0, 0, 0]),
            solid_frame(temporary.path(), 2, [0, 0, 0]),
            solid_frame(temporary.path(), 3, [255, 255, 255]),
        ];
        let detection = detect_with_duration(&frames, 4.0, 1.0, &SplitConfig::default()).unwrap();
        assert_eq!(detection.shots.len(), 2);
        assert_eq!(detection.shots[0].end_s, 0.75);
        assert_eq!(detection.shots[1].scene_score, 85.0);
        assert_eq!(detection.scores[3].score, 85.0);
    }

    #[test]
    fn sustained_sampled_fade_recovers_its_settled_tail() {
        let temporary = tempfile::tempdir().unwrap();
        let frames = [
            solid_frame(temporary.path(), 0, [0, 0, 0]),
            solid_frame(temporary.path(), 1, [0, 0, 0]),
            solid_frame(temporary.path(), 2, [0, 0, 0]),
            solid_frame(temporary.path(), 3, [255, 255, 255]),
            solid_frame(temporary.path(), 4, [128, 128, 128]),
            solid_frame(temporary.path(), 5, [40, 40, 40]),
            solid_frame(temporary.path(), 6, [20, 20, 20]),
            solid_frame(temporary.path(), 7, [20, 20, 20]),
        ];
        let detection = detect_with_duration(&frames, 4.0, 2.0, &SplitConfig::default()).unwrap();
        assert_eq!(
            detection
                .shots
                .iter()
                .take(detection.shots.len() - 1)
                .map(|shot| shot.end_s)
                .collect::<Vec<_>>(),
            vec![0.75, 1.5]
        );
    }
}
