#![cfg(target_os = "macos")]

use crush_stage_split::ffmpeg::{self, ClipOutputPreset, NormalizedVideoCrop, Runner, Source};
use crush_stage_split::reel::{
    ReelFormat, ReelMediaKind, ReelMotion, ReelRenderBackend, ResolvedReelGrade, ResolvedReelItem,
    ResolvedReelRequest, ResolvedReelTransition,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    root().join("fixtures/clips").join(name)
}

fn runner(debug_dir: &Path) -> Runner {
    let resolved = ffmpeg::resolve().expect("reel fixture tests require sidecars/get-sidecars.sh");
    assert_eq!(resolved.source, Source::DevSidecarDir);
    Runner::new(resolved, 2, "task-021-reel-test").with_debug_dir(debug_dir)
}

fn item(source_path: &Path, in_s: f64, out_s: f64) -> ResolvedReelItem {
    ResolvedReelItem {
        source_path: source_path.to_owned(),
        media_kind: ReelMediaKind::Video,
        in_s,
        out_s,
        crop: Some(NormalizedVideoCrop {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
        crop_keyframes: Vec::new(),
        caption: None,
        transition: ResolvedReelTransition::default(),
        speed: 1.0,
        motion: ReelMotion::None,
        volume: 1.0,
        grade: ResolvedReelGrade::default(),
    }
}

fn mean_pixel(ffmpeg: &Path, input: &Path, time_s: f64) -> f64 {
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-ss", &format!("{time_s:.6}"), "-i"])
        .arg(input)
        .args([
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout.iter().map(|value| *value as u64).sum::<u64>() as f64 / output.stdout.len() as f64
}

/// Decode one frame by index to raw YUV420P planes so boundary frames can be identified by
/// content. Raw planes avoid any color-matrix assumption: the untagged MPEG-4 fixture and
/// the BT.709-tagged H.264 reel would otherwise round-trip through different RGB matrices.
fn frame_yuv420p(ffmpeg: &Path, input: &Path, frame_index: i64) -> Vec<u8> {
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(input)
        .args([
            "-vf",
            &format!("select=eq(n\\,{frame_index})"),
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout
}

fn mean_abs_diff(left: &[u8], right: &[u8]) -> f64 {
    assert_eq!(left.len(), right.len(), "frames must share dimensions");
    left.iter()
        .zip(right)
        .map(|(a, b)| (*a as i64 - *b as i64).abs() as f64)
        .sum::<f64>()
        / left.len() as f64
}

/// Presentation timestamps of every video packet, in order.
fn video_packet_pts(ffprobe: &Path, input: &Path) -> Vec<f64> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_packets",
            "-select_streams",
            "v",
        ])
        .arg(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    document["packets"]
        .as_array()
        .expect("video packets")
        .iter()
        .filter_map(|packet| packet["pts_time"].as_str())
        .filter_map(|value| value.parse::<f64>().ok())
        .collect()
}

/// The burned-in frame counter in the synthetic-speech fixture gives ground truth: the
/// reel frame must be nearest (by mean absolute pixel difference) to the expected source
/// frame and clearly separated from its neighbours.
fn assert_frame_identity(
    ffmpeg: &Path,
    source: &Path,
    reel: &Path,
    reel_frame: i64,
    expected_source_frame: i64,
) {
    let rendered = frame_yuv420p(ffmpeg, reel, reel_frame);
    let mut scores = Vec::new();
    for candidate in [
        expected_source_frame - 1,
        expected_source_frame,
        expected_source_frame + 1,
    ] {
        if candidate < 0 {
            continue;
        }
        let reference = frame_yuv420p(ffmpeg, source, candidate);
        scores.push((candidate, mean_abs_diff(&rendered, &reference)));
    }
    let best = scores
        .iter()
        .copied()
        .min_by(|a, b| a.1.partial_cmp(&b.1).expect("finite frame distances"));
    let (best_frame, best_score) = best.expect("at least the expected frame is compared");
    assert_eq!(
        best_frame, expected_source_frame,
        "reel frame {reel_frame} must be source frame {expected_source_frame}; scores: {scores:?}"
    );
    let neighbour = scores
        .iter()
        .find(|(frame, _)| *frame != expected_source_frame)
        .map(|(_, score)| *score)
        .unwrap_or_default();
    assert!(
        best_score * 4.0 < neighbour,
        "reel frame {reel_frame} match to source frame {expected_source_frame} must be unambiguous; \
         best {best_score:.3} vs neighbour {neighbour:.3}"
    );
}

#[test]
fn ordered_reel_encodes_items_then_concats_with_measured_manifest_facts() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("private-reel-staging.mp4");
    let request = ResolvedReelRequest {
        items: vec![item(&source, 1.0, 2.0), item(&source, 3.0, 4.0)],
        format: ReelFormat::Source,
        music: None,
        master_volume: 1.0,
        watermark: None,
        cover: None,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };
    let values = Arc::new(Mutex::new(Vec::new()));
    let callback_values = Arc::clone(&values);
    let result = runner
        .render_reel_with_control(&request, &output, &Default::default(), move |progress| {
            callback_values.lock().unwrap().push(progress.percent)
        })
        .unwrap();

    assert_eq!(result.backend, ReelRenderBackend::VideoToolboxConcatDemuxer);
    assert_eq!(result.backend.as_str(), "videotoolbox+concat-demuxer");
    assert_eq!(result.encoder, "h264_videotoolbox");
    assert_eq!(result.preset, "mp4-h264-sdr-v1");
    assert_eq!(result.item_commands.len(), 2);
    assert!(result
        .item_commands
        .iter()
        .all(|command| command.contains("h264_videotoolbox") && !command.contains("-c copy")));
    // Input-side seeking lands on the requested first frame; the exact frame count is
    // pinned by -frames:v, and the item timeline starts at zero.
    assert!(result.item_commands[0].contains("-ss 0.999998 -t 1.1 -i"));
    assert!(result.item_commands[0].contains("setpts=PTS-STARTPTS"));
    assert!(result.item_commands[0].contains("-frames:v 30"));
    assert!(result.item_commands[1].contains("-ss 2.999998 -t 1.1 -i"));
    assert!(result.item_commands[1].contains("-frames:v 30"));
    assert_eq!(result.video_remux_commands.len(), 2);
    assert!(result
        .video_remux_commands
        .iter()
        .all(|command| command.contains("-map 0:v:0 -c copy -an")));
    assert!(result.command.contains("-f concat"));
    assert!(result.command.contains("-c:v copy"));
    assert!(result.command.contains("concat=n=2:v=0:a=1"));
    assert!(result.probe_command.contains("ffprobe"));
    assert_eq!(result.output_probe.video_codec.as_deref(), Some("h264"));
    assert_eq!(result.output_probe.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(result.output_probe.bit_depth, Some(8));
    assert_eq!(
        (result.output_probe.width, result.output_probe.height),
        (640, 360)
    );
    assert!(result.output_probe.has_audio);

    // TASK-036 frame-exactness: the video stream carries every requested frame and its
    // duration is the frame-count duration, not an audio-padded container duration.
    assert_eq!(result.video_frame_count, 60);
    assert!((result.video_duration_s - 2.0).abs() <= 0.002);
    assert_eq!(result.output_probe.video_frame_count, Some(60));
    assert!((result.output_probe.duration_s - 2.0).abs() <= 0.12);
    let item_a = &result.item_verifications[0];
    let item_b = &result.item_verifications[1];
    assert_eq!(item_a.requested_frame_count, 30);
    assert_eq!(item_a.rendered_frame_count, 30);
    assert_eq!(item_a.first_source_frame, 30);
    assert_eq!(item_a.last_source_frame, 59);
    assert!((item_a.video_duration_s - 1.0).abs() <= 0.002);
    assert!(item_a.audio_duration_s.unwrap() <= item_a.video_duration_s + 0.002);
    assert_eq!(item_b.requested_frame_count, 30);
    assert_eq!(item_b.rendered_frame_count, 30);
    assert_eq!(item_b.first_source_frame, 90);
    assert_eq!(item_b.last_source_frame, 119);

    // The cut lands exactly at the previous item's video duration: no PTS gap, no hold.
    let ffmpeg = runner.resolved().path.clone();
    let ffprobe = runner.resolved().ffprobe_path.clone();
    let pts = video_packet_pts(&ffprobe, &output);
    assert_eq!(pts.len(), 60);
    assert!((pts[0] - 0.0).abs() <= 0.002, "no lead dead zone");
    assert!((pts[29] - 29.0 / 30.0).abs() <= 0.002);
    assert!((pts[30] - 1.0).abs() <= 0.002, "cut lands at exactly 1.0s");
    assert!((pts[59] - 59.0 / 30.0).abs() <= 0.002);

    // The burned-in frame counter is the ground truth: first and last source frame of
    // each segment must be exactly the requested ones.
    assert_frame_identity(&ffmpeg, &source, &output, 0, 30);
    assert_frame_identity(&ffmpeg, &source, &output, 29, 59);
    assert_frame_identity(&ffmpeg, &source, &output, 30, 90);
    assert_frame_identity(&ffmpeg, &source, &output, 59, 119);

    let progress = values.lock().unwrap();
    assert!(!progress.is_empty());
    assert!(progress.iter().all(|value| (0.0..=100.0).contains(value)));
    assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));

    let first_source_mean = mean_pixel(&runner.resolved().path, &source, 1.2);
    let first_reel_mean = mean_pixel(&runner.resolved().path, &output, 0.2);
    assert!((first_source_mean - first_reel_mean).abs() / first_source_mean < 0.03);
    let second_source_mean = mean_pixel(&runner.resolved().path, &source, 3.2);
    let second_reel_mean = mean_pixel(&runner.resolved().path, &output, 1.2);
    assert!((second_source_mean - second_reel_mean).abs() / second_source_mean < 0.03);
}

#[test]
fn single_item_source_audio_reel_renders_without_the_concat_filter() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("single-reel.mp4");
    let request = ResolvedReelRequest {
        items: vec![item(&source, 0.5, 1.5)],
        format: ReelFormat::Source,
        music: None,
        master_volume: 1.0,
        watermark: None,
        cover: None,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };

    let result = runner.render_reel(&request, &output).unwrap();
    assert_eq!(result.video_frame_count, 30);
    assert!((result.video_duration_s - 1.0).abs() <= 0.002);
    assert!(result.command.contains("atrim=start=0[a0];[a0]anull[aout]"));
    assert!(!result.command.contains("concat=n="));
    assert_eq!(result.item_verifications.len(), 1);
    assert_eq!(result.item_verifications[0].first_source_frame, 15);
    assert_eq!(result.item_verifications[0].last_source_frame, 44);
    assert!(result.output_probe.has_audio);
    let audio_duration = result.output_probe.audio_duration_s.unwrap();
    assert!(audio_duration <= result.video_duration_s + 0.002);
}

/// Generate a deterministic source clip in-test with the bundled FFmpeg — no tracked fixture,
/// so no license/determinism review (review MEDIUM-1 preferred in-test generation). The
/// `testsrc` video is 320x180 @ 30 fps for exactly 1.0 s; the 1 kHz @ 44.1 kHz sine tone runs
/// for `audio_duration_s`, so `audio_duration_s < 1.0` produces a source whose audio track
/// ends early inside the requested item interval — the ordinary real-world clip shape the
/// review's MEDIUM-1 covers. Pinned lavfi/sine parameters keep the generation reproducible.
fn generate_tone_source(ffmpeg: &Path, output: &Path, audio_duration_s: f64) {
    let status = Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x180:rate=30:duration=1",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=1000:sample_rate=44100:duration={audio_duration_s}"),
        ])
        .args(["-map", "0:v", "-map", "1:a"])
        .args(["-c:v", "h264_videotoolbox", "-pix_fmt", "yuv420p"])
        .args([
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
        ])
        .args(["-c:a", "aac", "-b:a", "192k"])
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success());
}

/// Root mean square of the mono f32 PCM in `[t0, t1)` — proves where the tone actually plays
/// in the rendered reel, which is the A/V alignment ground truth for this test.
fn audio_window_rms(ffmpeg: &Path, input: &Path, t0: f64, t1: f64) -> f64 {
    const SAMPLE_RATE: usize = 44_100;
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "44100", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let samples: Vec<f32> = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    let start = (t0 * SAMPLE_RATE as f64) as usize;
    let end = (t1 * SAMPLE_RATE as f64) as usize;
    let window = &samples[start.min(samples.len())..end.min(samples.len())];
    assert!(
        !window.is_empty(),
        "decoded reel audio is shorter than {t1}s"
    );
    f64::from(
        (window.iter().map(|sample| sample * sample).sum::<f32>() / window.len() as f32).sqrt(),
    )
}

/// Review MEDIUM-1: a source whose audio track ends early inside the requested interval is
/// silence-padded to the item's exact video duration, and a multi-item reel stays A/V
/// aligned. The concat filter joins item audio end-to-end, so without the pad item B's tone
/// would start at 0.5 s (where item A's short audio ended) while item B's video starts at the
/// 1.0 s frame-exact cut — progressive desync published silently.
#[test]
fn short_item_audio_is_silence_padded_so_the_reel_stays_aligned() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let ffmpeg = runner.resolved().path.clone();
    let short_source = temporary.path().join("tone-short-audio.mp4");
    let full_source = temporary.path().join("tone-full-audio.mp4");
    generate_tone_source(&ffmpeg, &short_source, 0.5);
    generate_tone_source(&ffmpeg, &full_source, 1.0);
    let output = temporary.path().join("short-audio-reel.mp4");
    let request = ResolvedReelRequest {
        items: vec![item(&short_source, 0.0, 1.0), item(&full_source, 0.0, 1.0)],
        format: ReelFormat::Source,
        music: None,
        master_volume: 1.0,
        watermark: None,
        cover: None,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };

    let result = runner.render_reel(&request, &output).unwrap();

    // The pad is in the item command, and verification is fail-closed on the result: each
    // item's audio duration EQUALS its video duration, never merely "not longer".
    assert!(result.item_commands[0].contains("apad=whole_dur=1"));
    assert!(result.item_commands[1].contains("apad=whole_dur=1"));
    assert_eq!(result.video_frame_count, 60);
    assert!((result.video_duration_s - 2.0).abs() <= 0.002);
    let item_a = &result.item_verifications[0];
    let item_b = &result.item_verifications[1];
    assert!((item_a.audio_duration_s.unwrap() - 1.0).abs() <= 0.002);
    assert!((item_b.audio_duration_s.unwrap() - 1.0).abs() <= 0.002);
    let audio_duration = result.output_probe.audio_duration_s.unwrap();
    assert!((audio_duration - result.video_duration_s).abs() <= 0.002);

    // Alignment ground truth from the tone: item A's 1 kHz tone plays [0, 0.5) and its
    // padded silence covers [0.5, 1.0); item B's tone must start at the 1.0 s cut. Without
    // the pad, [0.55, 0.95) would carry item B's tone and [1.55, 1.95) would be silent.
    assert!(audio_window_rms(&ffmpeg, &output, 0.05, 0.45) > 0.04);
    assert!(audio_window_rms(&ffmpeg, &output, 0.55, 0.95) < 0.01);
    assert!(audio_window_rms(&ffmpeg, &output, 1.05, 1.45) > 0.04);
    assert!(audio_window_rms(&ffmpeg, &output, 1.55, 1.95) > 0.04);
}

#[test]
fn ordered_reel_never_overwrites_a_staging_destination() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("owned.mp4");
    std::fs::write(&output, b"another job").unwrap();
    let request = ResolvedReelRequest {
        items: vec![item(&source, 1.0, 2.0)],
        format: ReelFormat::Source,
        music: None,
        master_volume: 1.0,
        watermark: None,
        cover: None,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };

    let error = runner.render_reel(&request, &output).unwrap_err();
    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read(&output).unwrap(), b"another job");
}
