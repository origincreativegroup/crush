#![cfg(target_os = "macos")]

use crush_core::{config::SplitConfig, DEFAULT_OWNER_ID};
use crush_stage_split::{
    ffmpeg::{self, Runner},
    scene::{detect_with_duration, materialize_shots, scores_csv, Detection, FramePath},
};
use crush_store::{Store, Video, VideoStatus};
use serde::Deserialize;
#[cfg(not(debug_assertions))]
use std::time::Instant;
use std::{fs, path::PathBuf};

const FIXTURES: &[&str] = &[
    "earth-timelapse-silent",
    "goodnight-earth-vertical",
    "rocket-launch",
    "synthetic-speech",
];

#[derive(Deserialize)]
struct Golden {
    shots: Vec<GoldenShot>,
}

#[derive(Deserialize)]
struct GoldenShot {
    end_s: f64,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(stem: &str) -> PathBuf {
    root().join("fixtures/clips").join(format!("{stem}.mp4"))
}

fn golden(stem: &str) -> Golden {
    let path = root()
        .join("fixtures/golden")
        .join(format!("{stem}.scenes.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn runner(debug_dir: &std::path::Path) -> Runner {
    Runner::new(ffmpeg::resolve().unwrap(), 2, "task-005-test").with_debug_dir(debug_dir)
}

fn sampled_frames(directory: &std::path::Path) -> Vec<FramePath> {
    let mut frames = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "jpg"))
        .collect::<Vec<_>>();
    frames.sort();
    frames
}

fn detect_fixture(stem: &str, temporary: &tempfile::TempDir) -> (Runner, Detection) {
    let config = SplitConfig::default();
    let runner = runner(&temporary.path().join("debug"));
    let input = fixture(stem);
    let duration_s = runner.probe(&input).unwrap().value.duration_s;
    let frame_dir = temporary.path().join("frames");
    runner
        .sample_frames(&input, f64::from(config.sample_fps), &frame_dir)
        .unwrap();
    let frames = sampled_frames(&frame_dir);
    let detection = detect_with_duration(&frames, config.sample_fps, duration_s, &config).unwrap();
    (runner, detection)
}

#[test]
fn all_reference_cuts_match_at_four_fps() {
    let config = SplitConfig::default();
    let tolerance_s = f64::from(2.0 / config.sample_fps);

    for stem in FIXTURES {
        let temporary = tempfile::tempdir().unwrap();
        let (_, detection) = detect_fixture(stem, &temporary);
        let expected = golden(stem);
        let reference_cuts = expected
            .shots
            .iter()
            .take(expected.shots.len().saturating_sub(1))
            .map(|shot| shot.end_s)
            .collect::<Vec<_>>();
        let detected_cuts = detection
            .shots
            .iter()
            .take(detection.shots.len().saturating_sub(1))
            .map(|shot| shot.end_s)
            .collect::<Vec<_>>();
        for reference in &reference_cuts {
            assert!(
                detected_cuts
                    .iter()
                    .any(|detected| (detected - reference).abs() <= tolerance_s),
                "{stem}: no detected cut within {tolerance_s}s of {reference}; detected={detected_cuts:?}"
            );
        }
        let unmatched = detected_cuts
            .iter()
            .filter(|detected| {
                !reference_cuts
                    .iter()
                    .any(|reference| (*detected - reference).abs() <= tolerance_s)
            })
            .count();
        let duration_s = detection.shots.last().unwrap().end_s;
        let allowed_extras = (duration_s / 60.0).ceil() as usize;
        assert!(
            unmatched <= allowed_extras,
            "{stem}: {unmatched} unmatched cuts exceeds {allowed_extras}; detected={detected_cuts:?}"
        );
    }
}

#[test]
fn no_cut_fixture_yields_one_shot() {
    let temporary = tempfile::tempdir().unwrap();
    let (_, detection) = detect_fixture("synthetic-speech", &temporary);
    assert_eq!(detection.shots.len(), 1);
    assert_eq!(detection.shots[0].scene_score, 0.0);
}

#[test]
fn csv_thumbnails_and_store_rows_describe_the_same_detection() {
    let temporary = tempfile::tempdir().unwrap();
    let (runner, detection) = detect_fixture("goodnight-earth-vertical", &temporary);
    let input = fixture("goodnight-earth-vertical");
    let mut store = Store::open(temporary.path().join("data")).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: "golden-video".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: input.display().to_string(),
                sha256: "task-005-golden".to_owned(),
                duration_s: detection.shots.last().map(|shot| shot.end_s),
                fps: None,
                width: None,
                height: None,
                has_audio: false,
                status: VideoStatus::Pending,
                indexed_at: None,
            },
        )
        .unwrap();
    let thumbs = temporary.path().join("data/thumbs");
    let written = materialize_shots(
        &runner,
        &mut store,
        DEFAULT_OWNER_ID,
        "golden-video",
        &input,
        &detection.shots,
        &thumbs,
    )
    .unwrap();
    let stored = store
        .shots_for_video(DEFAULT_OWNER_ID, "golden-video")
        .unwrap();
    assert_eq!(stored, written);
    assert_eq!(stored.len(), detection.shots.len());
    for (row, span) in stored.iter().zip(&detection.shots) {
        assert_eq!(row.start_s, span.start_s);
        assert_eq!(row.end_s, span.end_s);
        assert_eq!(row.rep_frame_s, span.rep_frame_s);
        assert_eq!(row.scene_score, Some(span.scene_score));
        assert!(thumbs.join(row.thumb_rel.as_ref().unwrap()).is_file());
        assert!(scores_csv(&detection.scores)
            .lines()
            .any(|line| { line == format!("{:.6},{:.6}", row.start_s, span.scene_score) }));
    }
    assert_eq!(
        store
            .video_by_sha(DEFAULT_OWNER_ID, "task-005-golden")
            .unwrap()
            .unwrap()
            .status,
        VideoStatus::Split
    );
}

#[cfg(not(debug_assertions))]
#[test]
#[ignore = "explicit release-mode acceptance benchmark"]
fn detects_2400_480p_frames_in_under_five_seconds() {
    let temporary = tempfile::tempdir().unwrap();
    let config = SplitConfig::default();
    let runner = runner(&temporary.path().join("debug"));
    let frame_dir = temporary.path().join("frames");
    runner
        .sample_frames(&fixture("synthetic-speech"), 1.0, &frame_dir)
        .unwrap();
    let frame = sampled_frames(&frame_dir).remove(0);
    let frames = vec![frame; 2_400];
    let started = Instant::now();
    let detection = detect_with_duration(&frames, config.sample_fps, 600.0, &config).unwrap();
    std::hint::black_box(detection);
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs_f64() < 5.0, "detection took {elapsed:?}");
}
