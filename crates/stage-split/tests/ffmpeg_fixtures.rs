#![cfg(target_os = "macos")]

use crush_stage_split::ffmpeg::{
    self, BasicVideoGrade, CancellationToken, ClipAudio, ClipOutputPreset, ClipRenderBackend,
    ClipRenderRequest, ClipTransition, Error, ExportMode, NormalizedVideoCrop, Runner, Source,
    VideoGrade,
};
use serde_json::Value;
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
    let resolved =
        ffmpeg::resolve().expect("Task 4 fixture tests require sidecars/get-sidecars.sh");
    assert_eq!(resolved.source, Source::DevSidecarDir);
    Runner::new(resolved, 2, "task-004-test").with_debug_dir(debug_dir)
}

fn raw_ffprobe(ffprobe: &Path, input: &Path) -> Value {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(input)
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn mean_pixel(ffmpeg: &Path, input: &Path, time_s: f64) -> f64 {
    let mut command = Command::new(ffmpeg);
    command.args(["-v", "error"]);
    if time_s > 0.0 {
        command.args(["-ss", &format!("{time_s:.6}")]);
    }
    let output = command
        .arg("-i")
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

#[test]
fn probe_sampling_audio_and_frame_match_fixture_contracts() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let speech = fixture("synthetic-speech.mp4");

    let probe = runner.probe(&speech).unwrap().value;
    assert!((probe.duration_s - 12.0).abs() < 0.01);
    assert!((probe.fps - 30.0).abs() < 0.001);
    assert_eq!((probe.width, probe.height), (640, 360));
    assert!(probe.has_audio);
    assert_eq!(probe.video_codec.as_deref(), Some("mpeg4"));
    assert_eq!(probe.bit_depth, Some(8));
    assert!(probe
        .container
        .as_deref()
        .is_some_and(|value| value.contains("mp4")));

    let frames = temporary.path().join("frames");
    let sampled = runner.sample_frames(&speech, 0.5, &frames).unwrap();
    assert!(sampled.value.abs_diff(6) <= 1);
    std::fs::write(frames.join("f999999.jpg"), b"stale").unwrap();
    let resampled = runner.sample_frames(&speech, 0.5, &frames).unwrap();
    assert!(resampled.value.abs_diff(6) <= 1);
    assert!(!frames.join("f999999.jpg").exists());
    let first_frame = frames.join("f000001.jpg");
    let sampled_probe = runner.probe(&first_frame).unwrap().value;
    assert_eq!((sampled_probe.width, sampled_probe.height), (854, 480));

    let wav = temporary.path().join("speech.wav");
    runner.extract_audio(&speech, &wav).unwrap();
    let wav_json = raw_ffprobe(&runner.resolved().ffprobe_path, &wav);
    let audio = wav_json["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["codec_type"] == "audio")
        .unwrap();
    assert_eq!(audio["sample_rate"], "16000");
    assert_eq!(audio["channels"], 1);

    let frame = temporary.path().join("frame.jpg");
    runner.frame_at(&speech, 4.8, &frame).unwrap();
    let frame_probe = runner.probe(&frame).unwrap().value;
    assert_eq!((frame_probe.width, frame_probe.height), (640, 360));
    let expected = root().join("fixtures/golden/synthetic-speech.frame.ppm");
    let actual_mean = mean_pixel(&runner.resolved().path, &frame, 0.0);
    let expected_mean = mean_pixel(&runner.resolved().path, &expected, 0.0);
    assert!((actual_mean - expected_mean).abs() / expected_mean < 0.02);

    let commands = std::fs::read_to_string(temporary.path().join("debug/commands.txt")).unwrap();
    assert!(commands.contains("-progress pipe:1 -nostats"));
    assert!(commands.contains("fps=0.5,scale=-2:480"));
    assert!(commands
        .lines()
        .all(|line| line.starts_with("/usr/bin/nice ") || line.contains("ffprobe")));
}

#[test]
fn edit_proxy_is_playable_bounded_and_uses_lgpl_videotoolbox_path() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("working-proxy.mp4");
    let operation = runner
        .generate_edit_proxy_with_control(&input, &output, &CancellationToken::default(), |_| {})
        .unwrap();
    assert!(operation.command.contains("h264_videotoolbox"));
    assert!(operation.command.contains("scale=w="));
    let probe = runner.probe(&output).unwrap().value;
    assert_eq!(probe.video_codec.as_deref(), Some("h264"));
    assert!(probe.width <= 1920);
    assert!(probe.height <= 1080);
    assert!((probe.duration_s - 12.0).abs() < 0.1);
    assert!(!temporary
        .path()
        .join(".working-proxy.mp4.partial.mp4")
        .exists());
}

#[test]
fn export_reports_progress_and_starts_within_one_frame() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("rocket-launch.mp4");
    let output = temporary.path().join("clip.mp4");
    let progress_values = Arc::new(Mutex::new(Vec::new()));
    let callback_values = Arc::clone(&progress_values);
    let cancellation = CancellationToken::default();
    let result = runner
        .export_clip_with_control(&input, 3.2, 6.2, &output, &cancellation, move |value| {
            callback_values.lock().unwrap().push(value);
        })
        .unwrap();
    assert!(!result.command.is_empty());
    assert!(!progress_values.lock().unwrap().is_empty());
    let probe = runner.probe(&output).unwrap().value;
    assert!((probe.duration_s - 3.0).abs() <= 1.0 / probe.fps + 0.05);
    let source_mean = mean_pixel(&runner.resolved().path, &input, 3.2);
    let output_mean = mean_pixel(&runner.resolved().path, &output, 0.0);
    assert!((source_mean - output_mean).abs() / source_mean < 0.02);
}

#[test]
fn recipe_clip_render_encodes_and_returns_manifest_probe_facts() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("synthetic-speech.mp4");
    let input_bytes = std::fs::read(&input).unwrap();
    let output = temporary.path().join("private-staging.mp4");
    let request = ClipRenderRequest {
        in_s: 2.0,
        out_s: 4.0,
        crop: None,
        grade: VideoGrade::None,
        transition: ClipTransition::Cut,
        audio: ClipAudio::Source,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };
    let result = runner.render_clip(&input, &request, &output).unwrap();
    assert_eq!(result.backend, ClipRenderBackend::VideoToolbox);
    assert_eq!(result.backend.as_str(), "videotoolbox");
    assert_eq!(result.encoder, "h264_videotoolbox");
    assert_eq!(result.preset, "mp4-h264-sdr-v1");
    assert_eq!(result.source_color_handling, "untagged-assumed-sdr-bt709");
    assert!(result.command.contains("-n"));
    assert!(result.command.contains("h264_videotoolbox"));
    assert!(!result.command.contains("-c copy"));
    assert!(result.probe_command.contains("ffprobe"));
    assert_eq!(result.output_probe.video_codec.as_deref(), Some("h264"));
    assert_eq!(result.output_probe.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(result.output_probe.bit_depth, Some(8));
    assert_eq!(
        (
            result.output_probe.width,
            result.output_probe.height,
            result.output_probe.has_audio,
        ),
        (640, 360, true)
    );
    assert!((result.output_probe.duration_s - 2.0).abs() <= 1.0 / 30.0 + 0.05);
    assert_eq!(std::fs::read(&input).unwrap(), input_bytes);
    assert!(!std::fs::read_dir(temporary.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".crush-clip-render-")));
}

#[test]
fn recipe_clip_render_applies_crop_grade_and_mute_without_stream_copy() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("private-staging.mov");
    let request = ClipRenderRequest {
        in_s: 1.0,
        out_s: 3.0,
        crop: Some(NormalizedVideoCrop {
            x: 0.25,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        }),
        grade: VideoGrade::Basic(BasicVideoGrade {
            exposure_ev: 0.5,
            contrast: 0.2,
            saturation: 0.8,
            temperature: 0.1,
            tint: -0.1,
        }),
        transition: ClipTransition::Cut,
        audio: ClipAudio::Mute,
        output: ClipOutputPreset::MovH264SdrV1,
    };
    let result = runner.render_clip(&input, &request, &output).unwrap();
    assert_eq!(result.preset, "mov-h264-sdr-v1");
    assert_eq!(
        (result.output_probe.width, result.output_probe.height),
        (320, 360)
    );
    assert!(!result.output_probe.has_audio);
    assert!(result.command.contains("crop=320:360:160:0"));
    assert!(result.command.contains("exposure=exposure=0.5"));
    assert!(result.command.contains("colorlevels="));
    assert!(result.command.contains("hue=s=0.8"));
    assert!(result.command.contains("colorbalance="));
    assert!(result.command.contains("-an"));
    assert!(result.command.contains("-f mov"));
    assert!(!result.command.contains("-c copy"));
}

#[test]
fn recipe_clip_render_rejects_existing_staging_and_pre_cancelled_work() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("synthetic-speech.mp4");
    let request = ClipRenderRequest {
        in_s: 0.0,
        out_s: 1.0,
        crop: None,
        grade: VideoGrade::None,
        transition: ClipTransition::Cut,
        audio: ClipAudio::Mute,
        output: ClipOutputPreset::Mp4H264SdrV1,
    };
    let existing = temporary.path().join("existing.mp4");
    std::fs::write(&existing, b"another job owns this").unwrap();
    let error = runner.render_clip(&input, &request, &existing).unwrap_err();
    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read(&existing).unwrap(), b"another job owns this");

    let cancelled_output = temporary.path().join("cancelled.mp4");
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let error = runner
        .render_clip_with_control(&input, &request, &cancelled_output, &cancellation, |_| {})
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled { .. }));
    assert!(!cancelled_output.exists());
}

#[test]
fn cancelled_export_is_playable_or_removed() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("cancelled.mp4");
    let cancellation = CancellationToken::default();
    let triggered = cancellation.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1));
        triggered.cancel();
    });
    let result = runner.export_clip_with_control(&input, 0.0, 10.0, &output, &cancellation, |_| {});
    trigger.join().unwrap();
    assert!(matches!(result, Err(Error::Cancelled { .. })));
    if output.exists() {
        assert!(runner.probe(&output).unwrap().value.duration_s > 0.0);
    }
}

#[test]
fn incompatible_stream_copy_uses_lgpl_videotoolbox_fallback() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let incompatible = temporary.path().join("raw.avi");
    let generated = Command::new(&runner.resolved().path)
        .args(["-y", "-v", "error", "-i"])
        .arg(&source)
        .args(["-t", "2", "-an", "-c:v", "rawvideo", "-pix_fmt", "yuv420p"])
        .arg(&incompatible)
        .status()
        .unwrap();
    assert!(generated.success());

    let output = temporary.path().join("fallback.mp4");
    let result = runner
        .export_clip(&incompatible, 0.2, 1.2, &output)
        .unwrap();
    assert_eq!(result.mode, ExportMode::VideoToolboxReencode);
    assert_eq!(result.attempted_commands.len(), 2);
    assert!(result.attempted_commands[0].contains("-c copy"));
    assert!(result.attempted_commands[1].contains("h264_videotoolbox"));
    assert!(runner.probe(&output).unwrap().value.duration_s > 0.9);
}

#[test]
fn clip_export_never_overwrites_existing_files_or_source_aliases() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = temporary.path().join("source.mp4");
    std::fs::copy(fixture("synthetic-speech.mp4"), &source).unwrap();
    let source_bytes = std::fs::read(&source).unwrap();
    let hardlink = temporary.path().join("hardlink.mp4");
    std::fs::hard_link(&source, &hardlink).unwrap();
    let symlink_path = temporary.path().join("symlink.mp4");
    symlink(&source, &symlink_path).unwrap();
    let existing = temporary.path().join("existing.mp4");
    std::fs::write(&existing, b"keep this existing export").unwrap();
    let dangling = temporary.path().join("dangling.mp4");
    symlink(temporary.path().join("missing.mp4"), &dangling).unwrap();
    for target in [&source, &hardlink, &symlink_path, &existing, &dangling] {
        let error = runner.export_clip(&source, 1.0, 2.0, target).unwrap_err();
        assert!(error.to_string().contains("already exists"));
    }
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read(&existing).unwrap(),
        b"keep this existing export"
    );
    assert!(std::fs::symlink_metadata(&dangling)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn clip_export_collision_during_render_keeps_the_other_file_and_cleans_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("raced.mp4");
    let mut raced = false;
    let result = runner.export_clip_with_control(
        &source,
        1.0,
        2.0,
        &output,
        &CancellationToken::default(),
        |_| {
            if !raced {
                std::fs::write(&output, b"another writer owns this path").unwrap();
                raced = true;
            }
        },
    );
    assert!(raced, "fixture must exercise the publication race");
    assert!(
        matches!(result, Err(Error::Io(ref error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(
        std::fs::read(&output).unwrap(),
        b"another writer owns this path"
    );
    assert!(!std::fs::read_dir(temporary.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".crush-export-")));
}

#[test]
fn clip_export_cancellation_and_failure_publish_no_partial_output() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let source = fixture("synthetic-speech.mp4");
    let output = temporary.path().join("cancelled.mp4");
    let cancellation = CancellationToken::default();
    let result = runner.export_clip_with_control(&source, 1.0, 2.0, &output, &cancellation, |_| {
        cancellation.cancel()
    });
    assert!(matches!(result, Err(Error::Cancelled { .. })));
    assert!(!output.exists());
    let missing = temporary.path().join("missing.mp4");
    assert!(runner.export_clip(&missing, 1.0, 2.0, &output).is_err());
    assert!(!output.exists());
    assert!(!std::fs::read_dir(temporary.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".crush-export-")));
}

#[test]
fn progress_callback_fires_for_thirty_second_export() {
    let temporary = tempfile::tempdir().unwrap();
    let runner = runner(&temporary.path().join("debug"));
    let thirty_seconds = temporary.path().join("thirty-seconds.mp4");
    let generated = Command::new(&runner.resolved().path)
        .args(["-y", "-v", "error", "-stream_loop", "2", "-i"])
        .arg(fixture("synthetic-speech.mp4"))
        .args(["-t", "30", "-c", "copy"])
        .arg(&thirty_seconds)
        .status()
        .unwrap();
    assert!(generated.success());

    let progress_values = Arc::new(Mutex::new(Vec::new()));
    let callback_values = Arc::clone(&progress_values);
    let output = temporary.path().join("thirty-second-export.mp4");
    runner
        .export_clip_with_control(
            &thirty_seconds,
            0.0,
            30.0,
            &output,
            &CancellationToken::default(),
            move |value| callback_values.lock().unwrap().push(value),
        )
        .unwrap();
    let progress_values = progress_values.lock().unwrap();
    assert!(!progress_values.is_empty());
    assert!(progress_values
        .iter()
        .all(|value| (0.0..=100.0).contains(&value.percent)));
    assert!(progress_values.iter().any(|value| value.out_time_s > 0.0));
}
