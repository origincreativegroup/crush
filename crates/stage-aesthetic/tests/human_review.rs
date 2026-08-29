use std::path::{Path, PathBuf};

use crush_stage_aesthetic::{analyze, AnalysisContext, SemanticSignals, MODEL_VERSION};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ReviewSet {
    model_version: String,
    review_policy: String,
    fixtures: Vec<ReviewFixture>,
}

#[derive(Debug, Deserialize)]
struct ReviewFixture {
    name: String,
    media_kind: String,
    review: String,
    overall: [f64; 2],
    technical: [f64; 2],
    composition: [f64; 2],
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn reviewed_still_and_video_components_stay_calibrated() {
    let root = repo_root();
    let review: ReviewSet = serde_json::from_slice(
        &std::fs::read(root.join("fixtures/aesthetic/human-reviewed-v1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(review.model_version, MODEL_VERSION);
    assert!(review.review_policy.contains("Component calibration"));
    assert!(review
        .fixtures
        .iter()
        .any(|item| item.media_kind == "still"));
    assert!(review
        .fixtures
        .iter()
        .any(|item| item.media_kind.starts_with("video_shot")));

    let images = review
        .fixtures
        .iter()
        .map(|fixture| {
            image::open(
                root.join("fixtures/golden")
                    .join(format!("{}.frame.ppm", fixture.name)),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();

    for (index, fixture) in review.fixtures.iter().enumerate() {
        assert!(
            !fixture.review.is_empty(),
            "{} needs human rationale",
            fixture.name
        );
        let neighbors = images
            .iter()
            .enumerate()
            .filter(|(other, _)| other.abs_diff(index) == 1)
            .map(|(_, image)| image.clone())
            .collect::<Vec<_>>();
        let scores = analyze(
            &images[index],
            AnalysisContext {
                source_width: images[index].width(),
                source_height: images[index].height(),
                duration_s: Some(4.0),
                index: Some(index),
                sequence_len: Some(images.len()),
            },
            SemanticSignals::neutral(),
            &[],
            &neighbors,
        );
        assert_window(&fixture.name, "overall", scores.overall, fixture.overall);
        assert_window(
            &fixture.name,
            "technical",
            scores.technical_quality,
            fixture.technical,
        );
        assert_window(
            &fixture.name,
            "composition",
            scores.composition_quality,
            fixture.composition,
        );
        let evidence: serde_json::Value = serde_json::from_str(&scores.explanation_json).unwrap();
        assert_eq!(evidence["identity_used"], false);
        assert!(evidence["strengths"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
    }
}

fn assert_window(name: &str, component: &str, value: f64, window: [f64; 2]) {
    assert!(
        (window[0]..=window[1]).contains(&value),
        "{name} {component}={value:.3} drifted outside {:.3}..={:.3}",
        window[0],
        window[1]
    );
}
