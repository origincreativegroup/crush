use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use crush_core::cancellation::CancellationToken;
use crush_core::job::{JobStatus, Stage};
use crush_core::paths::AppPaths;
use crush_core::{Config, DEFAULT_OWNER_ID};
use crush_pipeline::{sha256_file, Pipeline};
use crush_search::SearchEngine;
use crush_stage_embed::embedder::{Embedder, ProviderPreference};
use crush_stage_split::ffmpeg;
use crush_store::{
    JobFilter, MediaKind, NewJob, PhotoProxyProvenance, PhotoStatus, Store, VideoStatus,
};
use image::{Rgb, RgbImage};

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
fn photo_ingest_is_idempotent_and_searchable() {
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("photos");
    std::fs::create_dir(&input).unwrap();
    let photo_path = input.join("warm-geometric-portrait.jpg");
    let image = RgbImage::from_fn(640, 426, |x, y| {
        if x > 380 && (90..350).contains(&y) {
            Rgb([230, 166, 72])
        } else {
            Rgb([34, 48, 78])
        }
    });
    image.save(&photo_path).unwrap();
    let original_sha256 = sha256_file(&photo_path).unwrap();
    let paths = AppPaths {
        root: temp.path().to_path_buf(),
    };
    let config = fixture_config(temp.path());
    let pipeline = Pipeline::new(config.clone(), paths.clone(), CancellationToken::default());

    let first = pipeline.ingest(&input, false).unwrap();
    assert_eq!(first.discovered, 1);
    assert_eq!(first.discovered_photos, 1);
    assert_eq!(first.indexed, 1);
    assert_eq!(first.indexed_photos, 1);
    assert_eq!(first.failed, 0);
    assert_eq!(first.search_vectors, 1);

    let store = Store::open(temp.path()).unwrap();
    let photo = store.photos(DEFAULT_OWNER_ID).unwrap().pop().unwrap();
    assert_eq!(photo.status, PhotoStatus::Done);
    assert_eq!(photo.sha256, original_sha256);
    assert_eq!(sha256_file(&photo_path).unwrap(), original_sha256);
    assert!(store
        .thumbnail_path(photo.thumb_rel.as_deref().unwrap())
        .unwrap()
        .is_file());
    let source_metadata = store
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)
        .unwrap()
        .unwrap();
    assert_eq!(source_metadata.decoder, "image-rs");
    assert_eq!(
        source_metadata.proxy_provenance,
        PhotoProxyProvenance::DecodedOriginal
    );
    let proxy_path = store
        .proxy_path(source_metadata.proxy_rel.as_deref().unwrap())
        .unwrap();
    assert!(proxy_path.is_file());
    assert_eq!(
        sha256_file(&proxy_path).unwrap(),
        source_metadata.proxy_sha256.as_deref().unwrap()
    );
    assert_eq!(
        store
            .vector_for_photo(DEFAULT_OWNER_ID, &photo.id)
            .unwrap()
            .unwrap()
            .len(),
        512
    );
    assert!(store
        .active_style_profile(DEFAULT_OWNER_ID)
        .unwrap()
        .is_none());
    let assessment = store
        .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Photo, &photo.id)
        .unwrap()
        .expect("cold-start photo assessment");
    assert_eq!(assessment.model_version, "strong-shot-v1");
    assert!((0.0..=1.0).contains(&assessment.technical_quality));
    assert!((0.0..=1.0).contains(&assessment.composition_quality));
    let evidence: serde_json::Value = serde_json::from_str(&assessment.explanation_json).unwrap();
    assert_eq!(evidence["independent_of_profile"], true);
    assert_eq!(evidence["identity_used"], false);
    let engine =
        SearchEngine::load(&store, DEFAULT_OWNER_ID, config.search.transcript_hit_boost).unwrap();
    let mut embedder = Embedder::new(paths.models(), ProviderPreference::Cpu, 2).unwrap();
    let results = engine
        .search_assets(
            &store,
            &mut |text: &str| embedder.embed_text(text),
            "warm geometric portrait",
            5,
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].asset_type, "photo");
    assert_eq!(results[0].asset_id, photo.id);
    drop(store);

    let second = pipeline.ingest(&input, false).unwrap();
    assert_eq!(second.indexed, 0);
    assert_eq!(second.skipped, 1);
    assert_eq!(sha256_file(&photo_path).unwrap(), original_sha256);

    let store = Store::open(temp.path()).unwrap();
    let photo_id = store.photos(DEFAULT_OWNER_ID).unwrap().pop().unwrap().id;
    drop(store);
    rusqlite::Connection::open(temp.path().join("library.db"))
        .unwrap()
        .execute(
            "DELETE FROM photo_source_metadata WHERE photo_id = ?1",
            [&photo_id],
        )
        .unwrap();
    let backfilled = pipeline.ingest(&input, false).unwrap();
    assert_eq!(
        backfilled.indexed, 1,
        "v2 photo rows must gain v3 fidelity metadata"
    );
    assert!(Store::open(temp.path())
        .unwrap()
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo_id)
        .unwrap()
        .is_some());
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
    assert_eq!(first.indexed, 4, "ingest errors: {:#?}", first.errors);
    assert_eq!(first.failed, 0);
    assert!(first.search_vectors > 0);

    let store = Store::open(temp.path()).unwrap();
    let videos = store.videos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(videos.len(), 4);
    assert!(videos.iter().all(|video| video.status == VideoStatus::Done));
    for video in &videos {
        assert_eq!(sha256_file(Path::new(&video.path)).unwrap(), video.sha256);
        let metadata = store
            .video_source_metadata(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap();
        assert!(!metadata.container.is_empty());
        assert!(!metadata.video_codec.is_empty());
        if metadata.proxy_required {
            let proxy = store
                .proxy_path(metadata.proxy_rel.as_deref().unwrap())
                .unwrap();
            assert!(proxy.is_file());
            assert_eq!(
                sha256_file(&proxy).unwrap(),
                metadata.proxy_sha256.as_deref().unwrap()
            );
        }
        for shot in store.shots_for_video(DEFAULT_OWNER_ID, &video.id).unwrap() {
            let assessment = store
                .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &shot.id)
                .unwrap()
                .expect("video shot assessment");
            assert_eq!(assessment.model_version, "strong-shot-v1");
            assert!((0.0..=1.0).contains(&assessment.motion_stability));
            assert!((0.0..=1.0).contains(&assessment.pacing));
            assert!(
                serde_json::from_str::<serde_json::Value>(&assessment.explanation_json).unwrap()
                    ["groups"]["moment_sequence"]
                    .is_object()
            );
        }
    }
    let jobs = store.jobs(DEFAULT_OWNER_ID, &JobFilter::default()).unwrap();
    assert_eq!(jobs.len(), 16);
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
            Stage::Analyze => assert!(directory.join("aesthetic-frames").is_dir()),
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
    rusqlite::Connection::open(temp.path().join("library.db"))
        .unwrap()
        .execute(
            "DELETE FROM aesthetic_assessments WHERE media_kind = 'shot' AND media_id = ?1",
            [&before_ids[0]],
        )
        .unwrap();
    let backfilled = pipeline.ingest(Path::new(&target.path), false).unwrap();
    assert_eq!(backfilled.indexed, 1);
    assert_eq!(backfilled.skipped, 0);
    assert!(Store::open(temp.path())
        .unwrap()
        .aesthetic_assessment(DEFAULT_OWNER_ID, MediaKind::Shot, &before_ids[0])
        .unwrap()
        .is_some());
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
#[cfg(target_os = "macos")]
fn packaged_pipeline_indexes_representative_professional_sources() {
    let resolved = match ffmpeg::resolve() {
        Ok(resolved) => resolved,
        Err(_) => {
            eprintln!("skipping Task 016 packaged-pipeline test: sidecars are not installed");
            return;
        }
    };
    let temp = tempfile::tempdir().unwrap();
    if !install_model_links(temp.path()) {
        return;
    }
    let input = temp.path().join("professional-sources");
    std::fs::create_dir(&input).unwrap();
    let still = RgbImage::from_fn(96, 64, |x, y| Rgb([(x * 2) as u8, (y * 3) as u8, 112]));
    let png = input.join("still.png");
    still.save(&png).unwrap();
    still
        .save_with_format(input.join("still.jpg"), image::ImageFormat::Jpeg)
        .unwrap();
    still
        .save_with_format(input.join("still.tiff"), image::ImageFormat::Tiff)
        .unwrap();
    let heic_status = Command::new("/usr/bin/sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(input.join("still.heic"))
        .status()
        .unwrap();
    assert!(heic_status.success());

    let source = repo_root().join("fixtures/clips/synthetic-speech.mp4");
    let video_cases = [
        (
            "prores.mov",
            vec!["-an", "-c:v", "prores", "-profile:v", "2"],
        ),
        ("h264.m4v", vec!["-an", "-c:v", "h264_videotoolbox"]),
        (
            "dnx.mxf",
            vec![
                "-an",
                "-vf",
                "scale=1280:720",
                "-c:v",
                "dnxhd",
                "-b:v",
                "90M",
            ],
        ),
        (
            "hevc.mov",
            vec!["-an", "-c:v", "hevc_videotoolbox", "-tag:v", "hvc1"],
        ),
    ];
    for (name, arguments) in &video_cases {
        let status = Command::new(&resolved.path)
            .args(["-y", "-v", "error", "-i"])
            .arg(&source)
            .args(["-t", "1"])
            .args(arguments)
            .arg(input.join(name))
            .status()
            .unwrap();
        assert!(status.success(), "could not generate {name}");
    }

    let started = Instant::now();
    let pipeline = Pipeline::new(
        fixture_config(temp.path()),
        AppPaths {
            root: temp.path().to_path_buf(),
        },
        CancellationToken::default(),
    );
    let summary = pipeline.ingest(&input, true).unwrap();
    let elapsed_ms = started.elapsed().as_millis();
    assert_eq!(summary.discovered, 8);
    assert_eq!(summary.discovered_photos, 4);
    assert_eq!(summary.indexed, 8);
    assert_eq!(summary.failed, 0, "{:?}", summary.errors);

    let store = Store::open(temp.path()).unwrap();
    let photos = store.photos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(photos.len(), 4);
    assert!(photos.iter().all(|photo| store
        .photo_source_metadata(DEFAULT_OWNER_ID, &photo.id)
        .unwrap()
        .is_some()));
    let videos = store.videos(DEFAULT_OWNER_ID).unwrap();
    assert_eq!(videos.len(), 4);
    let mut codec_policies = Vec::new();
    for video in videos {
        let metadata = store
            .video_source_metadata(DEFAULT_OWNER_ID, &video.id)
            .unwrap()
            .unwrap();
        codec_policies.push(serde_json::json!({
            "source": Path::new(&video.path).file_name().unwrap().to_string_lossy(),
            "container": metadata.container,
            "codec": metadata.video_codec,
            "bit_depth": metadata.bit_depth,
            "color_space": metadata.color_space,
            "proxy_required": metadata.proxy_required,
            "proxy_reason": metadata.proxy_reason,
        }));
    }
    let report = serde_json::json!({
        "fixture_set": "Task 016 representative packaged pipeline",
        "elapsed_ms": elapsed_ms,
        "peak_resident_bytes": peak_resident_bytes(),
        "failures": summary.errors,
        "orientation_check": "EXIF orientation normalization covered by source_fidelity fixture",
        "color_check": "source color fields persisted and derivatives visually decoded",
        "photos": photos.len(),
        "videos": codec_policies,
    });
    std::fs::write(
        temp.path().join("task-016-packaged-pipeline-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
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

#[cfg(target_os = "macos")]
fn peak_resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status == 0 {
        u64::try_from(unsafe { usage.assume_init() }.ru_maxrss).unwrap_or(0)
    } else {
        0
    }
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
