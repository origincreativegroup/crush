use std::path::{Path, PathBuf};
use std::time::Instant;

use crush_core::DEFAULT_OWNER_ID;
use crush_stage_asr::{
    align_video, model_path, transcribe_video, ModelChoice, TranscribeOptions, Transcriber,
};
use crush_stage_split::ffmpeg;
use crush_store::{Shot, Store, Video, VideoStatus};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GoldenTranscript {
    segments: Vec<GoldenSegment>,
}

#[derive(Debug, Deserialize)]
struct GoldenSegment {
    text: String,
}

#[derive(Debug, Deserialize)]
struct GoldenScenes {
    shots: Vec<GoldenShot>,
}

#[derive(Debug, Deserialize)]
struct GoldenShot {
    start_s: f64,
    end_s: f64,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn test_models() -> PathBuf {
    std::env::var_os("CRUSH_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("models"))
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn word_error_rate(expected: &str, actual: &str) -> f64 {
    let expected = words(expected);
    let actual = words(actual);
    if expected.is_empty() {
        return if actual.is_empty() { 0.0 } else { 1.0 };
    }
    let mut previous = (0..=actual.len()).collect::<Vec<_>>();
    for (row, expected_word) in expected.iter().enumerate() {
        let mut current = vec![row + 1];
        for (column, actual_word) in actual.iter().enumerate() {
            let substitution = previous[column] + usize::from(expected_word != actual_word);
            let deletion = previous[column + 1] + 1;
            let insertion = current[column] + 1;
            current.push(substitution.min(deletion).min(insertion));
        }
        previous = current;
    }
    previous[actual.len()] as f64 / expected.len() as f64
}

#[test]
fn speech_fixtures_stay_below_fifteen_percent_wer() {
    let model = model_path(test_models(), ModelChoice::Small);
    if !model.is_file() {
        eprintln!("skipping ASR golden: {} is not installed", model.display());
        return;
    }
    let runner = ffmpeg::Runner::new(ffmpeg::resolve().unwrap(), 0, "asr-golden");
    let transcriber = Transcriber::new(
        &model,
        ModelChoice::Small,
        TranscribeOptions {
            threads: 0,
            language: Some("en".to_owned()),
        },
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();

    for fixture in ["goodnight-earth-vertical", "synthetic-speech"] {
        let clip = repo_root().join(format!("fixtures/clips/{fixture}.mp4"));
        let wav = temp.path().join(format!("{fixture}.wav"));
        runner.extract_audio(&clip, &wav).unwrap();
        let (segments, audio_s, inference_ms) = transcriber.transcribe_wav(&wav).unwrap();
        for segment in &segments {
            eprintln!(
                "{fixture}: segment {:.2}-{:.2} confidence={:?} text={:?}",
                segment.start_s, segment.end_s, segment.confidence, segment.text
            );
        }
        let actual = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let golden: GoldenTranscript = serde_json::from_slice(
            &std::fs::read(repo_root().join(format!("fixtures/golden/{fixture}.transcript.json")))
                .unwrap(),
        )
        .unwrap();
        let expected = golden
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let wer = word_error_rate(&expected, &actual);
        let normalized_ms = inference_ms * 10.0 / audio_s;
        eprintln!(
            "{fixture}: backend={} audio_s={audio_s:.2} inference_ms={inference_ms:.2} ms_per_10s={normalized_ms:.2} wer={wer:.3} text={actual:?}",
            transcriber.backend()
        );
        assert!(wer < 0.15, "{fixture} WER {wer:.3}: {actual:?}");
    }
}

#[test]
fn silent_fixture_finishes_without_opening_wav_or_model() {
    let runner = ffmpeg::Runner::new(ffmpeg::resolve().unwrap(), 0, "asr-silent");
    let clip = repo_root().join("fixtures/clips/earth-timelapse-silent.mp4");
    let probe = runner.probe(&clip).unwrap().value;
    assert!(!probe.has_audio);

    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: "silent-fixture".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: clip.display().to_string(),
                sha256: "silent-fixture-sha".to_owned(),
                duration_s: Some(probe.duration_s),
                fps: Some(probe.fps),
                width: Some(i64::from(probe.width)),
                height: Some(i64::from(probe.height)),
                has_audio: probe.has_audio,
                status: VideoStatus::Pending,
                indexed_at: None,
            },
        )
        .unwrap();
    let started = Instant::now();
    let report = transcribe_video(
        &mut store,
        DEFAULT_OWNER_ID,
        "silent-fixture",
        temp.path().join("missing.wav"),
        temp.path().join("missing.bin"),
        ModelChoice::Small,
        TranscribeOptions::default(),
    )
    .unwrap();
    eprintln!(
        "silent ASR fast path: {:.3} ms",
        started.elapsed().as_secs_f64() * 1_000.0
    );
    assert!(report.skipped_no_audio);
    assert_eq!(report.segment_count, 0);
    assert!(started.elapsed().as_millis() < 250);
}

#[test]
fn query_time_alignment_matches_overlapping_fixture_shots() {
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::open(temp.path()).unwrap();
    let video_id = "synthetic-speech";
    store
        .upsert_video(
            DEFAULT_OWNER_ID,
            &Video {
                id: video_id.to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                path: "fixtures/clips/synthetic-speech.mp4".to_owned(),
                sha256: "synthetic-speech-sha".to_owned(),
                duration_s: Some(8.0),
                fps: Some(30.0),
                width: Some(1280),
                height: Some(720),
                has_audio: true,
                status: VideoStatus::Transcribed,
                indexed_at: None,
            },
        )
        .unwrap();
    let scenes: GoldenScenes = serde_json::from_slice(
        &std::fs::read(repo_root().join("fixtures/golden/synthetic-speech.scenes.json")).unwrap(),
    )
    .unwrap();
    let shots = scenes
        .shots
        .iter()
        .enumerate()
        .map(|(index, shot)| Shot {
            id: format!("shot-{index}"),
            video_id: video_id.to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            idx: index as i64,
            start_s: shot.start_s,
            end_s: shot.end_s,
            rep_frame_s: shot.start_s + (shot.end_s - shot.start_s) * 0.4,
            thumb_rel: None,
            scene_score: None,
        })
        .collect::<Vec<_>>();
    store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
    store
        .insert_transcript_segments(
            DEFAULT_OWNER_ID,
            &[
                crush_store::TranscriptSegment {
                    id: "segment-0".to_owned(),
                    video_id: video_id.to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    start_s: 0.0,
                    end_s: 4.08,
                    text: "A camera follows a small red boat across the quiet blue water."
                        .to_owned(),
                    confidence: Some(0.9),
                },
                crush_store::TranscriptSegment {
                    id: "segment-1".to_owned(),
                    video_id: video_id.to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    start_s: 4.08,
                    end_s: 6.72,
                    text: "The lighthouse is visible beyond the rocky shore.".to_owned(),
                    confidence: Some(0.9),
                },
            ],
        )
        .unwrap();
    let aligned = align_video(&store, DEFAULT_OWNER_ID, video_id).unwrap();
    for row in &aligned {
        eprintln!(
            "shot {} | {:.2}-{:.2} | {} | {}",
            row.shot.idx,
            row.shot.start_s,
            row.shot.end_s,
            row.segments.len(),
            row.segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    assert_eq!(aligned.len(), shots.len());
    assert!(aligned.iter().any(|row| !row.segments.is_empty()));
}

#[test]
fn word_error_rate_counts_insertions_deletions_and_substitutions() {
    assert_eq!(word_error_rate("one two three", "one two three"), 0.0);
    assert_eq!(
        word_error_rate("one two three", "one four three"),
        1.0 / 3.0
    );
    assert_eq!(word_error_rate("one two", "one bright two"), 0.5);
}
