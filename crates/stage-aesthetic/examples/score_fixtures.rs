use std::path::PathBuf;

use crush_stage_aesthetic::{analyze, AnalysisContext, SemanticSignals};

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden");
    let names = [
        "earth-timelapse-silent",
        "goodnight-earth-vertical",
        "rocket-launch",
        "synthetic-speech",
    ];
    let images = names
        .iter()
        .map(|name| image::open(root.join(format!("{name}.frame.ppm"))))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (name, image)) in names.iter().zip(&images).enumerate() {
        let neighbors = images
            .iter()
            .enumerate()
            .filter(|(other, _)| other.abs_diff(index) == 1)
            .map(|(_, image)| image.clone())
            .collect::<Vec<_>>();
        let scores = analyze(
            image,
            AnalysisContext {
                source_width: image.width(),
                source_height: image.height(),
                duration_s: Some(4.0),
                index: Some(index),
                sequence_len: Some(images.len()),
            },
            SemanticSignals::neutral(),
            &[],
            &neighbors,
        );
        println!(
            "{name}: overall={:.3} technical={:.3} composition={:.3} moment={:.3} focus={:.3} exposure={:.3} clipping={:.3} hierarchy={:.3} balance={:.3} negative_space={:.3} duplicate={:.3}",
            scores.overall,
            scores.technical_quality,
            scores.composition_quality,
            scores.moment_story,
            scores.sharpness,
            scores.exposure,
            scores.clipping_control,
            scores.hierarchy,
            scores.balance,
            scores.negative_space,
            scores.duplicate_confidence,
        );
    }
    Ok(())
}
