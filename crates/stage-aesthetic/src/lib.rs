//! Deterministic, local-first strong-shot analysis.
//!
//! This stage deliberately separates observable technical, design, and moment/sequence evidence.
//! It never uses identity or owner feedback. Semantic moment signals are optional CLIP comparisons
//! supplied by the caller; the pixel baseline remains useful when no model or profile is present.

use image::{imageops::FilterType, DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const MODEL_VERSION: &str = "strong-shot-v1";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisContext {
    pub source_width: u32,
    pub source_height: u32,
    pub duration_s: Option<f64>,
    pub index: Option<usize>,
    pub sequence_len: Option<usize>,
}

impl AnalysisContext {
    pub fn photo(image: &DynamicImage) -> Self {
        let (source_width, source_height) = image.dimensions();
        Self {
            source_width,
            source_height,
            duration_s: None,
            index: None,
            sequence_len: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticSignals {
    /// Bipolar CLIP evidence in [0, 1], where 0.5 means unavailable or inconclusive.
    pub expression: f64,
    pub gesture: f64,
    pub action: f64,
    pub story: f64,
    /// Confidence in the four semantic comparisons in [0, 1].
    pub confidence: f64,
}

impl SemanticSignals {
    pub const fn neutral() -> Self {
        Self {
            expression: 0.5,
            gesture: 0.5,
            action: 0.5,
            story: 0.5,
            confidence: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrongShotScores {
    pub technical_quality: f64,
    pub sharpness: f64,
    pub blur_control: f64,
    pub exposure: f64,
    pub clipping_control: f64,
    pub noise_control: f64,
    pub compression_quality: f64,
    pub resolution_quality: f64,
    pub motion_stability: f64,
    pub duplicate_confidence: f64,
    pub composition_quality: f64,
    pub contrast: f64,
    pub color_harmony: f64,
    pub hierarchy: f64,
    pub balance: f64,
    pub subject_placement: f64,
    pub negative_space: f64,
    pub leading_lines: f64,
    pub symmetry: f64,
    pub crop_potential: f64,
    pub visual_clarity: f64,
    pub moment_story: f64,
    pub expression: f64,
    pub gesture: f64,
    pub action: f64,
    pub novelty: f64,
    pub pacing: f64,
    pub repetition_risk: f64,
    pub overall: f64,
    pub confidence: f64,
    pub explanation_json: String,
}

#[derive(Debug, Clone)]
struct Features {
    width: usize,
    height: usize,
    luma: Vec<f64>,
    saliency: Vec<f64>,
    mean: f64,
    std_dev: f64,
    p05: f64,
    p95: f64,
    clipped: f64,
    gradient_mean: f64,
    gradient_p90: f64,
    blockiness: f64,
    noise: f64,
    saturation_mean: f64,
    hue_histogram: [f64; 12],
}

/// Analyze one still or representative shot frame. `motion_frames` come from inside the same
/// video shot; `sequence_neighbors` are adjacent assets used for novelty and repetition.
pub fn analyze(
    image: &DynamicImage,
    context: AnalysisContext,
    semantic: SemanticSignals,
    motion_frames: &[DynamicImage],
    sequence_neighbors: &[DynamicImage],
) -> StrongShotScores {
    let features = extract_features(image);
    let sharpness = smoothstep(0.015, 0.11, features.gradient_p90);
    let blur_control = clamp01(0.25 + 0.75 * sharpness);
    let exposure_center = 1.0 - ((features.mean - 0.5).abs() / 0.48).powf(1.4);
    let clipping_control = clamp01(1.0 - features.clipped * 4.5);
    let exposure = clamp01(0.65 * exposure_center + 0.35 * clipping_control);
    let contrast = smoothstep(0.12, 0.62, features.p95 - features.p05);
    let noise_control = clamp01(1.0 - smoothstep(0.018, 0.11, features.noise));
    let compression_quality = clamp01(1.0 - smoothstep(0.012, 0.09, features.blockiness));
    let megapixels =
        f64::from(context.source_width) * f64::from(context.source_height) / 1_000_000.0;
    let resolution_quality = smoothstep(0.25, 3.0, megapixels);

    let (motion_stability, temporal_action, duplicate_confidence, novelty, repetition_risk) =
        temporal_scores(image, motion_frames, sequence_neighbors);
    let composition = composition_scores(&features);
    let color_harmony = color_harmony(&features);
    let technical_quality = weighted_mean(&[
        (sharpness, 0.20),
        (blur_control, 0.10),
        (exposure, 0.18),
        (clipping_control, 0.10),
        (noise_control, 0.10),
        (compression_quality, 0.08),
        (resolution_quality, 0.14),
        (
            motion_stability,
            if context.duration_s.is_some() {
                0.10
            } else {
                0.0
            },
        ),
    ]);
    let composition_quality = weighted_mean(&[
        (contrast, 0.12),
        (color_harmony, 0.12),
        (composition.hierarchy, 0.16),
        (composition.balance, 0.12),
        (composition.subject_placement, 0.12),
        (composition.negative_space, 0.09),
        (composition.leading_lines, 0.07),
        (composition.symmetry, 0.05),
        (composition.crop_potential, 0.07),
        (composition.visual_clarity, 0.08),
    ]);

    let semantic = sanitize_semantic(semantic);
    let action = if context.duration_s.is_some() {
        weighted_mean(&[
            (semantic.action, semantic.confidence),
            (temporal_action, 0.65),
        ])
    } else {
        semantic.action
    };
    let pacing = context.duration_s.map_or(0.5, pacing_score);
    let moment_story = weighted_mean(&[
        (semantic.expression, 0.18),
        (semantic.gesture, 0.17),
        (action, 0.20),
        (semantic.story, 0.20),
        (novelty, 0.15),
        (
            pacing,
            if context.duration_s.is_some() {
                0.10
            } else {
                0.0
            },
        ),
    ]);
    let duplicate_penalty = 0.12 * duplicate_confidence.max(repetition_risk);
    let overall = clamp01(
        technical_quality * 0.42 + composition_quality * 0.40 + moment_story * 0.18
            - duplicate_penalty,
    );
    let pixel_confidence = smoothstep(
        10_000.0,
        150_000.0,
        (features.width * features.height) as f64,
    );
    let sequence_confidence = if motion_frames.is_empty() && sequence_neighbors.is_empty() {
        0.0
    } else {
        1.0
    };
    let confidence = clamp01(
        0.62 * pixel_confidence
            + 0.23 * semantic.confidence
            + 0.15
                * if context.duration_s.is_some() {
                    sequence_confidence
                } else {
                    1.0
                },
    );

    let mut scores = StrongShotScores {
        technical_quality,
        sharpness,
        blur_control,
        exposure,
        clipping_control,
        noise_control,
        compression_quality,
        resolution_quality,
        motion_stability,
        duplicate_confidence,
        composition_quality,
        contrast,
        color_harmony,
        hierarchy: composition.hierarchy,
        balance: composition.balance,
        subject_placement: composition.subject_placement,
        negative_space: composition.negative_space,
        leading_lines: composition.leading_lines,
        symmetry: composition.symmetry,
        crop_potential: composition.crop_potential,
        visual_clarity: composition.visual_clarity,
        moment_story,
        expression: semantic.expression,
        gesture: semantic.gesture,
        action,
        novelty,
        pacing,
        repetition_risk,
        overall,
        confidence,
        explanation_json: String::new(),
    };
    scores.explanation_json = explanation(&scores, &features, context, semantic.confidence);
    scores
}

#[derive(Debug, Clone, Copy)]
struct Composition {
    hierarchy: f64,
    balance: f64,
    subject_placement: f64,
    negative_space: f64,
    leading_lines: f64,
    symmetry: f64,
    crop_potential: f64,
    visual_clarity: f64,
}

fn extract_features(image: &DynamicImage) -> Features {
    let rgb = image.thumbnail(384, 384).to_rgb8();
    let (width, height) = rgb.dimensions();
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;
    let mut luma = Vec::with_capacity(width * height);
    let mut saturation_sum = 0.0;
    let mut hue_histogram = [0.0; 12];
    for pixel in rgb.pixels() {
        let r = f64::from(pixel[0]) / 255.0;
        let g = f64::from(pixel[1]) / 255.0;
        let b = f64::from(pixel[2]) / 255.0;
        luma.push(0.2126 * r + 0.7152 * g + 0.0722 * b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let saturation = if max <= f64::EPSILON {
            0.0
        } else {
            delta / max
        };
        saturation_sum += saturation;
        if delta > 0.03 {
            let raw = if max == r {
                ((g - b) / delta).rem_euclid(6.0)
            } else if max == g {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };
            let bin = ((raw / 6.0 * 12.0).floor() as usize).min(11);
            hue_histogram[bin] += saturation;
        }
    }
    let mean = average(&luma);
    let std_dev = (luma.iter().map(|value| (value - mean).powi(2)).sum::<f64>()
        / luma.len().max(1) as f64)
        .sqrt();
    let mut sorted = luma.clone();
    sorted.sort_by(f64::total_cmp);
    let p05 = percentile(&sorted, 0.05);
    let p95 = percentile(&sorted, 0.95);
    let clipped =
        luma.iter().filter(|&&v| v < 0.015 || v > 0.985).count() as f64 / luma.len().max(1) as f64;
    let mut gradients = vec![0.0; luma.len()];
    let mut residuals = Vec::new();
    let mut block_boundary = 0.0;
    let mut ordinary_boundary = 0.0;
    let mut block_count = 0usize;
    let mut ordinary_count = 0usize;
    if width > 2 && height > 2 {
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let idx = y * width + x;
                let gx = luma[idx + 1] - luma[idx - 1];
                let gy = luma[idx + width] - luma[idx - width];
                gradients[idx] = (gx * gx + gy * gy).sqrt();
                let local =
                    (luma[idx - 1] + luma[idx + 1] + luma[idx - width] + luma[idx + width]) / 4.0;
                if gradients[idx] < 0.06 {
                    residuals.push((luma[idx] - local).abs());
                }
                if x % 8 == 0 {
                    block_boundary += (luma[idx] - luma[idx - 1]).abs();
                    block_count += 1;
                } else {
                    ordinary_boundary += (luma[idx] - luma[idx - 1]).abs();
                    ordinary_count += 1;
                }
            }
        }
    }
    let mut gradient_sorted = gradients.clone();
    gradient_sorted.sort_by(f64::total_cmp);
    let gradient_mean = average(&gradients);
    let gradient_p90 = percentile(&gradient_sorted, 0.90);
    let noise = if residuals.is_empty() {
        0.0
    } else {
        average(&residuals)
    };
    let block_mean = block_boundary / block_count.max(1) as f64;
    let ordinary_mean = ordinary_boundary / ordinary_count.max(1) as f64;
    let blockiness = (block_mean - ordinary_mean).max(0.0);
    let saliency = gradients
        .iter()
        .zip(&luma)
        .map(|(gradient, value)| gradient * 0.72 + (value - mean).abs() * 0.28)
        .collect();
    Features {
        width,
        height,
        luma,
        saliency,
        mean,
        std_dev,
        p05,
        p95,
        clipped,
        gradient_mean,
        gradient_p90,
        blockiness,
        noise,
        saturation_mean: saturation_sum / (width * height).max(1) as f64,
        hue_histogram,
    }
}

fn composition_scores(f: &Features) -> Composition {
    let total = f.saliency.iter().sum::<f64>().max(1e-9);
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut left = 0.0;
    let mut right = 0.0;
    let mut top = 0.0;
    let mut bottom = 0.0;
    let mut border = 0.0;
    let mut low = 0usize;
    let saliency_mean = total / f.saliency.len().max(1) as f64;
    for y in 0..f.height {
        for x in 0..f.width {
            let value = f.saliency[y * f.width + x];
            let nx = x as f64 / f.width.saturating_sub(1).max(1) as f64;
            let ny = y as f64 / f.height.saturating_sub(1).max(1) as f64;
            sx += value * nx;
            sy += value * ny;
            if nx < 0.5 {
                left += value
            } else {
                right += value
            }
            if ny < 0.5 {
                top += value
            } else {
                bottom += value
            }
            if !(0.08..=0.92).contains(&nx) || !(0.08..=0.92).contains(&ny) {
                border += value;
            }
            if value < saliency_mean * 0.55 {
                low += 1;
            }
        }
    }
    let cx = sx / total;
    let cy = sy / total;
    let balance = clamp01(1.0 - ((left - right).abs() + (top - bottom).abs()) / (2.0 * total));
    let thirds = [
        (1.0 / 3.0, 1.0 / 3.0),
        (2.0 / 3.0, 1.0 / 3.0),
        (1.0 / 3.0, 2.0 / 3.0),
        (2.0 / 3.0, 2.0 / 3.0),
        (0.5, 0.5),
    ];
    let distance = thirds
        .iter()
        .map(|(x, y)| ((cx - x).powi(2) + (cy - y).powi(2)).sqrt())
        .fold(f64::INFINITY, f64::min);
    let subject_placement = clamp01(1.0 - distance / 0.36);
    let negative_fraction = low as f64 / f.saliency.len().max(1) as f64;
    let negative_space = clamp01(1.0 - ((negative_fraction - 0.42).abs() / 0.42));
    let max_saliency = f.saliency.iter().copied().fold(0.0, f64::max);
    let hierarchy = clamp01(
        smoothstep(1.2, 5.5, max_saliency / saliency_mean.max(1e-9)) * 0.65
            + smoothstep(0.02, 0.16, f.std_dev) * 0.35,
    );
    let border_fraction = border / total;
    let crop_potential = clamp01(1.0 - smoothstep(0.20, 0.55, border_fraction));
    let symmetry = mirror_similarity(&f.luma, f.width, f.height);
    let leading_lines = orientation_coherence(&f.luma, f.width, f.height);
    let visual_clarity = clamp01(
        0.55 * hierarchy + 0.25 * crop_potential + 0.20 * smoothstep(0.01, 0.08, f.gradient_mean),
    );
    Composition {
        hierarchy,
        balance,
        subject_placement,
        negative_space,
        leading_lines,
        symmetry,
        crop_potential,
        visual_clarity,
    }
}

fn color_harmony(f: &Features) -> f64 {
    if f.saturation_mean < 0.05 {
        return 0.72; // deliberate monochrome/neutral palettes remain valid.
    }
    let total = f.hue_histogram.iter().sum::<f64>().max(1e-9);
    let dominant = f.hue_histogram.iter().copied().fold(0.0, f64::max) / total;
    let complementary = (0..6)
        .map(|i| (f.hue_histogram[i] + f.hue_histogram[i + 6]) / total)
        .fold(0.0, f64::max);
    clamp01(0.35 + 0.42 * dominant + 0.35 * complementary)
}

fn temporal_scores(
    image: &DynamicImage,
    motion_frames: &[DynamicImage],
    sequence_neighbors: &[DynamicImage],
) -> (f64, f64, f64, f64, f64) {
    let base = tiny_luma(image);
    let motion_differences = motion_frames
        .iter()
        .map(|other| mean_abs_diff(&base, &tiny_luma(other)))
        .collect::<Vec<_>>();
    let mean_motion = if motion_differences.is_empty() {
        0.08
    } else {
        average(&motion_differences)
    };
    let neighbor_differences = sequence_neighbors
        .iter()
        .map(|other| mean_abs_diff(&base, &tiny_luma(other)))
        .collect::<Vec<_>>();
    let nearest = neighbor_differences
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let duplicate_confidence = if nearest.is_finite() {
        clamp01(1.0 - smoothstep(0.015, 0.13, nearest))
    } else {
        0.0
    };
    let repetition_risk = duplicate_confidence;
    let novelty = if neighbor_differences.is_empty() {
        0.65
    } else {
        clamp01(smoothstep(0.025, 0.22, average(&neighbor_differences)))
    };
    let temporal_action = clamp01(smoothstep(0.018, 0.24, mean_motion));
    // Very large representative-frame changes are more likely unstable, while modest movement
    // is compatible with intentional camera or subject motion.
    let motion_stability = clamp01(1.0 - smoothstep(0.18, 0.55, mean_motion));
    (
        motion_stability,
        temporal_action,
        duplicate_confidence,
        novelty,
        repetition_risk,
    )
}

fn tiny_luma(image: &DynamicImage) -> Vec<f64> {
    image
        .resize_exact(48, 48, FilterType::Triangle)
        .to_luma8()
        .pixels()
        .map(|p| f64::from(p[0]) / 255.0)
        .collect()
}

fn mean_abs_diff(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(a, b)| (a - b).abs())
        .sum::<f64>()
        / left.len().min(right.len()).max(1) as f64
}

fn pacing_score(duration: f64) -> f64 {
    if !duration.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    if duration < 0.6 {
        smoothstep(0.15, 0.6, duration)
    } else if duration <= 8.0 {
        1.0
    } else {
        clamp01(1.0 - smoothstep(8.0, 24.0, duration) * 0.55)
    }
}

fn mirror_similarity(luma: &[f64], width: usize, height: usize) -> f64 {
    if width < 2 || height == 0 {
        return 0.5;
    }
    let mut error = 0.0;
    let mut count = 0usize;
    for y in 0..height {
        for x in 0..width / 2 {
            error += (luma[y * width + x] - luma[y * width + (width - 1 - x)]).abs();
            count += 1;
        }
    }
    clamp01(1.0 - error / count.max(1) as f64 * 2.0)
}

fn orientation_coherence(luma: &[f64], width: usize, height: usize) -> f64 {
    if width < 3 || height < 3 {
        return 0.5;
    }
    let mut bins = [0.0_f64; 8];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let idx = y * width + x;
            let gx = luma[idx + 1] - luma[idx - 1];
            let gy = luma[idx + width] - luma[idx - width];
            let magnitude = (gx * gx + gy * gy).sqrt();
            if magnitude > 0.025 {
                let angle = gy.atan2(gx).rem_euclid(std::f64::consts::PI);
                let bin = ((angle / std::f64::consts::PI * 8.0).floor() as usize).min(7);
                bins[bin] += magnitude;
            }
        }
    }
    let total = bins.iter().sum::<f64>().max(1e-9);
    let peak = bins.iter().copied().fold(0.0, f64::max) / total;
    clamp01(0.35 + smoothstep(0.18, 0.55, peak) * 0.65)
}

fn explanation(
    scores: &StrongShotScores,
    f: &Features,
    context: AnalysisContext,
    semantic_confidence: f64,
) -> String {
    let mut ranked = [
        (
            "focus",
            scores.sharpness,
            format!("edge detail measured {:.3}", f.gradient_p90),
        ),
        (
            "exposure",
            scores.exposure,
            format!(
                "mean luminance {:.2}; clipped pixels {:.1}%",
                f.mean,
                f.clipped * 100.0
            ),
        ),
        (
            "composition",
            scores.composition_quality,
            "hierarchy, balance, placement, and usable space were measured independently"
                .to_owned(),
        ),
        (
            "color harmony",
            scores.color_harmony,
            format!("mean saturation {:.2}", f.saturation_mean),
        ),
        (
            "moment/story",
            scores.moment_story,
            if semantic_confidence > 0.0 {
                "identity-free CLIP concept comparisons plus temporal evidence".to_owned()
            } else {
                "semantic model evidence unavailable; neutral values were retained".to_owned()
            },
        ),
    ];
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let strengths = ranked.iter().take(2).map(|(component, score, evidence)| json!({"component": component, "score": round3(*score), "evidence": format!("Strong {component}: {evidence}.")})).collect::<Vec<_>>();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
    let cautions = ranked.iter().filter(|(_, score, _)| *score < 0.62).take(2).map(|(component, score, evidence)| json!({"component": component, "score": round3(*score), "evidence": format!("{component} limited the result: {evidence}.")})).collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "summary": format!("General strong-shot score {:.0}/100 from separate technical, design, and moment evidence.", scores.overall * 100.0),
        "independent_of_profile": true,
        "identity_used": false,
        "strengths": strengths,
        "cautions": cautions,
        "groups": {
            "technical": {"score": round3(scores.technical_quality), "focus": round3(scores.sharpness), "blur_control": round3(scores.blur_control), "exposure": round3(scores.exposure), "clipping_control": round3(scores.clipping_control), "noise_control": round3(scores.noise_control), "compression_quality": round3(scores.compression_quality), "resolution_quality": round3(scores.resolution_quality), "motion_stability": round3(scores.motion_stability), "duplicate_confidence": round3(scores.duplicate_confidence)},
            "composition": {"score": round3(scores.composition_quality), "hierarchy": round3(scores.hierarchy), "balance": round3(scores.balance), "subject_placement": round3(scores.subject_placement), "negative_space": round3(scores.negative_space), "leading_lines": round3(scores.leading_lines), "symmetry": round3(scores.symmetry), "contrast": round3(scores.contrast), "color_harmony": round3(scores.color_harmony), "crop_potential": round3(scores.crop_potential), "visual_clarity": round3(scores.visual_clarity)},
            "moment_sequence": {"score": round3(scores.moment_story), "expression": round3(scores.expression), "gesture": round3(scores.gesture), "action": round3(scores.action), "novelty": round3(scores.novelty), "pacing": round3(scores.pacing), "repetition_risk": round3(scores.repetition_risk), "semantic_confidence": round3(semantic_confidence)}
        },
        "context": {"source_width": context.source_width, "source_height": context.source_height, "duration_s": context.duration_s, "index": context.index, "sequence_len": context.sequence_len},
        "model_version": MODEL_VERSION
    }).to_string()
}

pub fn bipolar_similarity(positive: f32, negative: f32) -> (f64, f64) {
    let delta = f64::from(positive - negative);
    let score = 1.0 / (1.0 + (-delta * 12.0).exp());
    let confidence = clamp01(delta.abs() * 8.0);
    (score, confidence)
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn sanitize_semantic(value: SemanticSignals) -> SemanticSignals {
    SemanticSignals {
        expression: clamp01(value.expression),
        gesture: clamp01(value.gesture),
        action: clamp01(value.action),
        story: clamp01(value.story),
        confidence: clamp01(value.confidence),
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let idx = ((sorted.len().saturating_sub(1)) as f64 * quantile).round() as usize;
    sorted.get(idx).copied().unwrap_or(0.0)
}

fn average(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

fn weighted_mean(values: &[(f64, f64)]) -> f64 {
    let total = values.iter().map(|(_, weight)| *weight).sum::<f64>();
    if total <= f64::EPSILON {
        0.5
    } else {
        clamp01(
            values
                .iter()
                .map(|(value, weight)| value * weight)
                .sum::<f64>()
                / total,
        )
    }
}

fn smoothstep(edge0: f64, edge1: f64, value: f64) -> f64 {
    if edge1 <= edge0 {
        return f64::from(value >= edge1);
    }
    let x = clamp01((value - edge0) / (edge1 - edge0));
    x * x * (3.0 - 2.0 * x)
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}
fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn designed(width: u32, height: u32) -> DynamicImage {
        let mut image = RgbImage::from_pixel(width, height, Rgb([28, 42, 68]));
        for y in height / 5..height * 4 / 5 {
            for x in width / 2..width * 5 / 6 {
                let edge = x == width / 2
                    || y == height / 5
                    || x + 1 == width * 5 / 6
                    || y + 1 == height * 4 / 5;
                image.put_pixel(
                    x,
                    y,
                    if edge {
                        Rgb([250, 205, 70])
                    } else {
                        Rgb([190, 70, 45])
                    },
                );
            }
        }
        DynamicImage::ImageRgb8(image)
    }

    #[test]
    fn no_profile_orders_clear_exposed_image_over_flat_blur() {
        let strong = designed(1800, 1200);
        let weak = DynamicImage::ImageRgb8(RgbImage::from_pixel(1800, 1200, Rgb([252, 252, 252])));
        let strong_score = analyze(
            &strong,
            AnalysisContext::photo(&strong),
            SemanticSignals::neutral(),
            &[],
            &[],
        );
        let weak_score = analyze(
            &weak,
            AnalysisContext::photo(&weak),
            SemanticSignals::neutral(),
            &[],
            &[],
        );
        assert!(
            strong_score.overall > weak_score.overall + 0.15,
            "strong={} weak={}",
            strong_score.overall,
            weak_score.overall
        );
        let explanation: serde_json::Value =
            serde_json::from_str(&strong_score.explanation_json).unwrap();
        assert_eq!(explanation["independent_of_profile"], true);
        assert_eq!(explanation["identity_used"], false);
        assert!(explanation["summary"]
            .as_str()
            .unwrap()
            .contains("General strong-shot score"));
    }

    #[test]
    fn repetition_is_detected_without_identity() {
        let image = designed(600, 400);
        let duplicate = image.clone();
        let scores = analyze(
            &image,
            AnalysisContext {
                source_width: 600,
                source_height: 400,
                duration_s: Some(3.0),
                index: Some(0),
                sequence_len: Some(2),
            },
            SemanticSignals::neutral(),
            &[],
            &[duplicate],
        );
        assert!(scores.duplicate_confidence > 0.95);
        assert!(scores.repetition_risk > 0.95);
    }

    #[test]
    fn bipolar_semantics_report_score_and_evidence_confidence() {
        let (positive, confidence) = bipolar_similarity(0.31, 0.18);
        let (uncertain, uncertain_confidence) = bipolar_similarity(0.20, 0.20);
        assert!(positive > 0.8);
        assert!(confidence > uncertain_confidence);
        assert!((uncertain - 0.5).abs() < 1e-9);
    }
}
