use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::job::{JobStatus, Stage};
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_pipeline::Pipeline;
use crush_stage_split::ffmpeg;
use crush_store::{JobFilter, NewJob, Store, VideoStatus};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn source_models() -> PathBuf {
    std::env::var_os("CRUSH_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("models"))
}

fn install_model_links(data_dir: &Path) -> bool {
    let source = source_models();
    let required = [
        "clip-image.onnx",
        "clip-text.onnx",
        "bpe_simple_vocab_16e6.txt.gz",
        "ggml-small.bin",
    ];
    if !required.iter().all(|name| source.join(name).is_file()) {
        eprintln!("skipping pipeline fixture test: complete models-v1 is not installed");
        return false;
    }
    let destination = data_dir.join("models");
    std::fs::create_dir_all(&destination).unwrap();
    for name in required {
        let source = source.join(name);
        let target = destination.join(name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(source, target).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(source, target).unwrap();
    }
    true
}

fn fixture_config(data_dir: &Path) -> Config {
    let mut config = Config {
        data_dir: Some(data_dir.to_path_buf()),
        ..Config::default()
    };
    config.embed.provider = "cpu".to_owned();
    config.asr.model = "small".to_owned();
    config.asr.language = Some("en".to_owned());
    config
}

#[test]
fn fixture_ingest_is_idempotent_resumable_and_exports_a_clip() {
    if ffmpeg::resolve().is_err() {
        eprintln!("skipping pipeline fixture test: FFmpeg sidecars are not installed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let config = fixture_config(temp.path());
    let pipeline = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default());
    let clips = repo_root().join("fixtures/clips");

    let first = pipeline.ingest(&clips, true).unwrap();
    assert_eq!(first.discovered, 4);
    assert_eq!(first.indexed, 4);
    assert_eq!(first.failed, 0);
    assert!(first.search_vectors > 0);

    let store = Store::open(temp.path()).unwrap();
    let videos = store.videos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(videos.len(), 4);
    assert!(videos.iter().all(|video| video.status == VideoStatus::Done));
    let jobs = store.jobs(DEFAULT_OWNER_ID, &JobFilter::default()).unwrap();
    assert_eq!(jobs.len(), 12);
    assert!(jobs.iter().all(|job| job.status == JobStatus::Done));
    for job in &jobs {
        let directory = Path::new(job.debug_dir.as_deref().expect("debug directory recorded"));
        assert!(directory.is_dir());
        match job.stage {
            Stage::Split => {
                assert!(directory.join("frames").is_dir());
                assert!(directory.join("scores.csv").is_file());
                assert!(directory.join("commands.txt").is_file());
            }
            Stage::Embed => assert!(directory.join("vectors.json").is_file()),
            Stage::Transcribe => {
                let video = store
                    .video_by_id(DEFAULT_OWNER_ID, &job.video_id)
                    .unwrap()
                    .unwrap();
                if video.has_audio {
                    assert!(directory.join("audio.wav").is_file());
                    assert!(directory.join("commands.txt").is_file());
                }
            }
        }
    }
    drop(store);

    let second = pipeline.ingest(&clips, false).unwrap();
    assert_eq!(second.skipped, 4);
    assert_eq!(second.indexed, 0);

    let store = Store::open(temp.path()).unwrap();
    let target = store
        .videos(DEFAULT_OWNER_ID)
        .unwrap()
        .into_iter()
        .find(|video| video.path.ends_with("synthetic-speech.mp4"))
        .unwrap();
    let before_ids = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .map(|shot| shot.id)
        .collect::<Vec<_>>();
    drop(store);
    pipeline.resplit(&target.id, false).unwrap();
    let store = Store::open(temp.path()).unwrap();
    let after_ids = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .map(|shot| shot.id)
        .collect::<Vec<_>>();
    assert_eq!(
        before_ids, after_ids,
        "resplit must preserve stable shot ids"
    );

    let shot = store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    drop(store);
    let exported = temp.path().join("exported-shot.mp4");
    pipeline.export_clip(&shot.id, &exported).unwrap();
    let probe = ffmpeg::Runner::new(ffmpeg::resolve().unwrap(), 0, "verify-clip")
        .probe(&exported)
        .unwrap()
        .value;
    assert!(probe.duration_s > 0.0);
    assert!((probe.duration_s - (shot.end_s - shot.start_s)).abs() < 0.25);

    let store = Store::open(temp.path()).unwrap();
    store
        .set_video_status(DEFAULT_OWNER_ID, &target.id, VideoStatus::Split)
        .unwrap();
    let shots = store.shots_for_video(DEFAULT_OWNER_ID, &target.id).unwrap();
    let preserved = store
        .vector_for_shot(DEFAULT_OWNER_ID, &shots[0].id)
        .unwrap()
        .unwrap();
    store
        .delete_vectors_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap();
    store
        .put_vector(DEFAULT_OWNER_ID, &shots[0].id, &preserved)
        .unwrap();
    store
        .job_start(
            DEFAULT_OWNER_ID,
            &NewJob {
                id: "killed-mid-embed".to_owned(),
                video_id: target.id.clone(),
                stage: Stage::Embed,
                started_at: Utc::now(),
                debug_dir: None,
            },
        )
        .unwrap();
    drop(store);

    let resumed = pipeline.ingest(Path::new(&target.path), false).unwrap();
    assert_eq!(resumed.recovered_jobs, 1);
    let store = Store::open(temp.path()).unwrap();
    assert_eq!(
        store
            .jobs(
                DEFAULT_OWNER_ID,
                &JobFilter {
                    status: Some(JobStatus::Failed),
                    ..JobFilter::default()
                },
            )
            .unwrap()
            .into_iter()
            .find(|job| job.id == "killed-mid-embed")
            .unwrap()
            .error
            .as_deref(),
        Some("interrupted")
    );
    assert!(store
        .shots_for_video(DEFAULT_OWNER_ID, &target.id)
        .unwrap()
        .iter()
        .all(|shot| store
            .vector_for_shot(DEFAULT_OWNER_ID, &shot.id)
            .unwrap()
            .is_some()));
}

#[test]
fn bad_video_records_a_failed_job_and_does_not_block_other_files() {
    if ffmpeg::resolve().is_err() {
        eprintln!("skipping pipeline fixture test: FFmpeg sidecars are not installed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("00-bad.mp4"), b"not a video").unwrap();
    std::fs::copy(
        repo_root().join("fixtures/clips/earth-timelapse-silent.mp4"),
        input.join("01-valid.mp4"),
    )
    .unwrap();
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        paths,
        CancellationToken::default(),
    );
    let summary = pipeline.ingest(&input, false).unwrap();
    assert_eq!(summary.discovered, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.indexed, 1);
    let store = Store::open(temp.path()).unwrap();
    let failed = store
        .jobs(
            DEFAULT_OWNER_ID,
            &JobFilter {
                status: Some(JobStatus::Failed),
                ..JobFilter::default()
            },
        )
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]
        .error
        .as_deref()
        .is_some_and(|error| !error.is_empty()));
    assert_eq!(
        store
            .videos(DEFAULT_OWNER_ID)
            .unwrap()
            .into_iter()
            .find(|video| video.path.ends_with("01-valid.mp4"))
            .unwrap()
            .status,
        VideoStatus::Done
    );
}

#[test]
fn pre_cancelled_pipeline_stops_without_creating_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        cancellation,
    );
    assert!(pipeline
        .ingest(&repo_root().join("fixtures/clips"), false)
        .is_err());
    let store = Store::open(temp.path()).unwrap();
    assert!(store
        .jobs(DEFAULT_OWNER_ID, &JobFilter::default())
        .unwrap()
        .is_empty());
}

#[test]
#[ignore = "10-minute local responsiveness smoke; run explicitly before release"]
fn ten_minute_silent_ingest_smoke() {
    let Ok(resolved) = ffmpeg::resolve() else {
        eprintln!("skipping 10-minute smoke: FFmpeg sidecars are not installed");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }

    let input = temp.path().join("ten-minute-silent.mp4");
    let status = Command::new(&resolved.path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-stream_loop",
            "59",
            "-i",
        ])
        .arg(repo_root().join("fixtures/clips/earth-timelapse-silent.mp4"))
        .args(["-t", "600", "-an", "-c:v", "copy"])
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());

    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let started = Instant::now();
    let summary = pipeline.ingest(&input, false).unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "10-minute ingest: {:.2}s wall time, {} vectors",
        elapsed.as_secs_f64(),
        summary.search_vectors
    );
    assert_eq!(summary.indexed, 1);
    assert_eq!(summary.failed, 0);

    let store = Store::open(temp.path()).unwrap();
    let video = store.videos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
    assert_eq!(video.status, VideoStatus::Done);
    assert!(video.duration_s.is_some_and(|duration| duration >= 599.0));
}
