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
    assert!(result.item_commands[0].contains("-ss 1 -t 1"));
    assert!(result.item_commands[1].contains("-ss 3 -t 1"));
    assert!(result.command.contains("-f concat"));
    assert!(result.command.contains("-c copy"));
    assert!(result.probe_command.contains("ffprobe"));
    assert_eq!(result.output_probe.video_codec.as_deref(), Some("h264"));
    assert_eq!(result.output_probe.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(result.output_probe.bit_depth, Some(8));
    assert_eq!(
        (result.output_probe.width, result.output_probe.height),
        (640, 360)
    );
    assert!(result.output_probe.has_audio);
    assert!((result.output_probe.duration_s - 2.0).abs() <= 0.12);

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
