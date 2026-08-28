use std::path::{Path, PathBuf};

use crush_core::{models, Config, DEFAULT_OWNER_ID};
use crush_search::SearchEngine;
use crush_stage_embed::{
    embed_missing_shots,
    embedder::{Embedder, ProviderPreference},
};
use crush_stage_split::{
    ffmpeg::{self, Runner},
    scene::{self, ShotSpan},
};
use crush_store::{EmbeddingMeta, Store, TranscriptSegment, Video, VideoStatus};

#[test]
fn five_fixture_queries_report_expected_shot_in_top_three() {
    let Some(models_dir) = models_dir() else {
        eprintln!("skipping fixture search: set CRUSH_TEST_MODELS or install models/");
        return;
    };
    let root = repository_root();
    let fixture_manifest: serde_json::Value =
        read_json(&root.join("fixtures/golden/fixtures-manifest.json"));
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    let runner = Runner::new(ffmpeg::resolve().unwrap(), 1, "fixture-search");
    let mut embedder = Embedder::new(&models_dir, ProviderPreference::Cpu, 1).unwrap();

    for (filename, fixture) in fixture_manifest["clips"].as_object().unwrap() {
        let stem = filename.strip_suffix(".mp4").unwrap();
        let video_id = stem.to_owned();
        let clip = root.join("fixtures/clips").join(filename);
        store
            .upsert_video(
                DEFAULT_OWNER_ID,
                &Video {
                    id: video_id.clone(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: clip.display().to_string(),
                    sha256: fixture["sha256"].as_str().unwrap().to_owned(),
                    duration_s: fixture["duration_s"].as_f64(),
                    fps: None,
                    width: None,
                    height: None,
                    has_audio: fixture["has_audio"].as_bool().unwrap(),
                    status: VideoStatus::Pending,
                    indexed_at: None,
                },
            )
            .unwrap();
        let scene_golden: serde_json::Value = read_json(
            &root
                .join("fixtures/golden")
                .join(format!("{stem}.scenes.json")),
        );
        let spans = scene_golden["shots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|shot| {
                let start_s = shot["start_s"].as_f64().unwrap();
                let end_s = shot["end_s"].as_f64().unwrap();
                ShotSpan {
                    start_s,
                    end_s,
                    rep_frame_s: start_s + (end_s - start_s) * 0.4,
                    scene_score: 0.0,
                }
            })
            .collect::<Vec<_>>();
        scene::materialize_shots(
            &runner,
            &mut store,
            DEFAULT_OWNER_ID,
            &video_id,
            &clip,
            &spans,
            &directory.path().join("thumbs"),
        )
        .unwrap();
        embed_missing_shots(&store, DEFAULT_OWNER_ID, &video_id, &mut embedder).unwrap();

        let transcript: serde_json::Value = read_json(
            &root
                .join("fixtures/golden")
                .join(format!("{stem}.transcript.json")),
        );
        let segments = transcript["segments"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, segment)| TranscriptSegment {
                id: format!("{video_id}-segment-{index:06}"),
                video_id: video_id.clone(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                start_s: segment["start_s"].as_f64().unwrap(),
                end_s: segment["end_s"].as_f64().unwrap(),
                text: segment["text"].as_str().unwrap().to_owned(),
                confidence: None,
            })
            .collect::<Vec<_>>();
        store
            .insert_transcript_segments(DEFAULT_OWNER_ID, &segments)
            .unwrap();
    }

    let manifest = models::bundled_manifest().unwrap();
    store
        .embedding_meta_set(
            DEFAULT_OWNER_ID,
            &EmbeddingMeta {
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                model_name: manifest.model_name,
                model_sha256: manifest.embedding_sha256,
                dim: manifest.dim,
                preprocess_version: manifest.preprocess_version,
            },
        )
        .unwrap();
    let config = Config::default();
    let engine =
        SearchEngine::load(&store, DEFAULT_OWNER_ID, config.search.transcript_hit_boost).unwrap();
    let expectations_path = root.join("fixtures/golden/expected_search.json");
    let expectations = expectations_path
        .is_file()
        .then(|| read_json::<serde_json::Value>(&expectations_path));
    let queries = fixture_manifest["queries"].as_array().unwrap();
    for query in queries {
        let query = query.as_str().unwrap();
        let mut text_embedder = |text: &str| embedder.embed_text(text);
        let results = engine.search(&store, &mut text_embedder, query, 3).unwrap();
        assert_eq!(results.len(), 3);
        eprintln!("query={query:?}");
        for result in &results {
            eprintln!(
                "  shot={} score={:.9} cosine={:.9}",
                result.shot_id, result.score, result.cosine
            );
        }
        if let Some(expectations) = &expectations {
            let expected = expectations["queries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|value| value["query"].as_str() == Some(query))
                .and_then(|value| value["expected_shot_id"].as_str())
                .unwrap_or_else(|| panic!("expected_search.json has no entry for {query:?}"));
            assert!(
                results.iter().any(|result| result.shot_id == expected),
                "expected {expected} in the top three for {query:?}"
            );
        }
    }
    if expectations.is_none() {
        eprintln!(
            "candidate rankings only: fixtures/golden/expected_search.json awaits human review"
        );
    }
}

fn models_dir() -> Option<PathBuf> {
    let path = std::env::var_os("CRUSH_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("models"));
    path.join("clip-image.onnx").is_file().then_some(path)
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
