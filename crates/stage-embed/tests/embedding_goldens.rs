use std::path::{Path, PathBuf};

use crush_stage_embed::{
    embed_missing_shots,
    embedder::{ActiveProvider, Embedder, ProviderPreference},
    preprocess::preprocess,
    tokenizer::ClipTokenizer,
};
use crush_store::{Shot, Store, Video, VideoStatus};
use serde::Deserialize;

#[derive(Deserialize)]
struct ImageGolden {
    input: String,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct TextGolden {
    input: String,
    token_ids: Vec<i64>,
    embedding: Vec<f32>,
}

#[test]
fn clip_token_ids_match_all_text_goldens() {
    let Some(models) = models_dir() else {
        eprintln!("skipping tokenizer goldens: set CRUSH_TEST_MODELS or install models/");
        return;
    };
    let mut tokenizer =
        ClipTokenizer::from_gzip(models.join("bpe_simple_vocab_16e6.txt.gz")).unwrap();
    for path in text_goldens() {
        let golden: TextGolden = read_json(&path);
        let found = tokenizer.encode(&golden.input).unwrap();
        assert_eq!(found.as_slice(), golden.token_ids, "{}", path.display());
    }
}

#[test]
fn cpu_image_and_text_embeddings_match_goldens() {
    let Some(models) = models_dir() else {
        eprintln!("skipping CPU embedding goldens: set CRUSH_TEST_MODELS or install models/");
        return;
    };
    let root = repository_root();
    let mut embedder = Embedder::new(&models, ProviderPreference::Cpu, 1).unwrap();
    for path in image_goldens() {
        let golden: ImageGolden = read_json(&path);
        let image = image::open(root.join(&golden.input)).unwrap();
        let found = embedder.embed_image(&preprocess(&image)).unwrap();
        let cosine = cosine(&found, &golden.embedding);
        eprintln!("{} cpu image cosine={cosine:.9}", path.display());
        assert!(cosine > 0.999, "{} cosine={cosine}", path.display());
    }
    check_text_goldens(&mut embedder, 0.999);
    assert_eq!(embedder.active_provider(), ActiveProvider::Cpu);

    let temporary = tempfile::tempdir().unwrap();
    let mut store = Store::open(temporary.path()).unwrap();
    store
        .upsert_video(
            "local",
            &Video {
                id: "video-stage".to_owned(),
                owner_id: "local".to_owned(),
                path: "/tmp/stage.mp4".to_owned(),
                sha256: "stage-sha".to_owned(),
                duration_s: Some(1.0),
                fps: Some(30.0),
                width: Some(640),
                height: Some(480),
                has_audio: false,
                status: VideoStatus::Split,
                indexed_at: None,
            },
        )
        .unwrap();
    store
        .insert_shots(
            "local",
            &[Shot {
                id: "shot-stage".to_owned(),
                video_id: "video-stage".to_owned(),
                owner_id: "local".to_owned(),
                idx: 0,
                start_s: 0.0,
                end_s: 1.0,
                rep_frame_s: 0.4,
                thumb_rel: Some("stage.ppm".to_owned()),
                scene_score: None,
            }],
        )
        .unwrap();
    std::fs::copy(
        root.join("fixtures/golden/rocket-launch.frame.ppm"),
        store.thumbnail_path("stage.ppm").unwrap(),
    )
    .unwrap();
    assert_eq!(
        embed_missing_shots(&store, "local", "video-stage", &mut embedder).unwrap(),
        1
    );
    assert_eq!(
        embed_missing_shots(&store, "local", "video-stage", &mut embedder).unwrap(),
        0,
        "completed vectors must be resumable without duplicate work"
    );
    let stored = store
        .vector_for_shot("local", "shot-stage")
        .unwrap()
        .unwrap();
    assert_eq!(stored.len(), 512);
    assert!((cosine(&stored, &stored) - 1.0).abs() < 1e-9);
}

#[test]
#[ignore = "CoreML compiles the models on first use; run explicitly for Task 8 acceptance"]
fn coreml_image_and_text_embeddings_match_goldens() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipping CoreML goldens: CoreML is available only on macOS");
        return;
    }
    let Some(models) = models_dir() else {
        eprintln!("skipping CoreML goldens: set CRUSH_TEST_MODELS or install models/");
        return;
    };
    let root = repository_root();
    let mut embedder = Embedder::new(&models, ProviderPreference::CoreMl, 0).unwrap();
    for path in image_goldens() {
        let golden: ImageGolden = read_json(&path);
        let image = image::open(root.join(&golden.input)).unwrap();
        let found = embedder.embed_image(&preprocess(&image)).unwrap();
        let cosine = cosine(&found, &golden.embedding);
        eprintln!("{} coreml image cosine={cosine:.9}", path.display());
        assert!(cosine > 0.99, "{} cosine={cosine}", path.display());
    }
    check_text_goldens(&mut embedder, 0.99);
    assert_eq!(embedder.active_provider(), ActiveProvider::CoreMl);
}

fn check_text_goldens(embedder: &mut Embedder, threshold: f64) {
    for path in text_goldens() {
        let golden: TextGolden = read_json(&path);
        let ids = embedder.tokenize(&golden.input).unwrap();
        assert_eq!(ids.as_slice(), golden.token_ids, "{}", path.display());
        let found = embedder.embed_text(&golden.input).unwrap();
        let cosine = cosine(&found, &golden.embedding);
        eprintln!("{} text cosine={cosine:.9}", path.display());
        assert!(cosine > threshold, "{} cosine={cosine}", path.display());
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

fn image_goldens() -> Vec<PathBuf> {
    let directory = repository_root().join("fixtures/golden");
    [
        "earth-timelapse-silent.image.json",
        "goodnight-earth-vertical.image.json",
        "rocket-launch.image.json",
        "synthetic-speech.image.json",
    ]
    .into_iter()
    .map(|name| directory.join(name))
    .collect()
}

fn text_goldens() -> Vec<PathBuf> {
    let directory = repository_root().join("fixtures/golden");
    (1..=5)
        .map(|index| directory.join(format!("text{index}.json")))
        .collect()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    assert_eq!(left.len(), right.len());
    let dot = left
        .iter()
        .zip(right)
        .map(|(&left, &right)| f64::from(left) * f64::from(right))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    dot / (left_norm * right_norm)
}
