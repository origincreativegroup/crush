//! Held-out evaluation gate for personal style profiles.
//!
//! A profile may only be marked `learned` when the composed production ranker strictly improves
//! on the non-personalized baseline on unseen work (docs/dam-feedback-blueprint.md: "A style
//! profile cannot be called learned without held-out improvement over the general ranker").
//!
//! Two remediation rules keep the evaluation honest (docs/review-2026-08-29.md finding 2):
//!
//! 1. **The split is media-disjoint, not pair-disjoint.** Every media asset is assigned to
//!    train or evaluation, so no asset (and no pair touching it) appears on both sides: the
//!    held-out pairs are always ranked with a residual that never saw their media. Pairs that
//!    straddle the partition are counted and dropped from both sides.
//! 2. **The scored margin is the composed production margin** — the production general
//!    aesthetic adjustment for the pair plus the personal affinity scale times the residual
//!    margin — not the residual alone. The residual-only accuracy is reported next to it for
//!    transparency.

use std::collections::{BTreeSet, HashSet};

/// Pairs held out for evaluation; the gate refuses to declare learning below this floor.
pub const MIN_HELD_OUT_PAIRS: usize = 4;
/// Personal accuracy must clear this floor even when it beats the baseline.
pub const MIN_PERSONAL_ACCURACY: f64 = 0.6;
/// Every k-th media asset (k = 3) of the deterministic media order is held out.
const HOLDOUT_STRIDE: usize = 3;
pub const SPLIT_LABEL: &str = "media-disjoint-every-3rd";
/// The production ranker adds `PERSONAL_AFFINITY_SCALE * dot(vector, residual)` to the general
/// score ([`crate::PERSONAL_WEIGHT`]); evaluation composes the same scaled residual.
pub const PERSONAL_AFFINITY_SCALE: f64 = crate::PERSONAL_WEIGHT as f64;

/// One deterministic train/eval pair: the composed ranker must rank `plus` above `minus`.
#[derive(Debug, Clone)]
pub struct RankedPair {
    /// `features(plus) - features(minus)`; the residual margin is `dot(weights, margin_features)`.
    pub margin_features: Vec<f64>,
    /// Evidence strength of the pair (strongest for prefer events and curated examples);
    /// conflicting reverse pairs have already been netted out by the trainer.
    pub weight: f64,
    /// Pool key (`<kind>:<media_id>`) of the preferred side; the media-disjoint split uses
    /// these to keep every media asset wholly on one side of the partition.
    pub plus_media: String,
    /// Pool key of the rejected side.
    pub minus_media: String,
    /// The production general-aesthetic margin for this pair without any personal term — the
    /// difference of the general adjustment `GENERAL_AESTHETIC_WEIGHT * (overall - 0.5)`
    /// ([`crate::GENERAL_AESTHETIC_WEIGHT`]) that production composes beside the personal
    /// term. A missing side is neutral (adjustment 0, i.e. `overall` 0.5), matching
    /// production's missing-assessment behavior. The composed margin is
    /// `general_margin + PERSONAL_AFFINITY_SCALE * residual`.
    pub general_margin: f64,
}

/// The deterministic train/held-out partition over an ordered pair list.
///
/// Media assets are sorted and every third asset held out; a pair belongs to `held_out` only
/// when both of its media are held out, to `train` only when neither is, and otherwise counts
/// as `straddling_pairs` (dropped from both sides, reported in the metrics).
#[derive(Debug, Default)]
pub struct Split<'a> {
    pub train: Vec<&'a RankedPair>,
    pub held_out: Vec<&'a RankedPair>,
    pub straddling_pairs: usize,
}

pub fn split_pairs(pairs: &[RankedPair]) -> Split<'_> {
    let mut media: BTreeSet<&str> = BTreeSet::new();
    for pair in pairs {
        media.insert(pair.plus_media.as_str());
        media.insert(pair.minus_media.as_str());
    }
    let held_out_media: HashSet<&str> = media
        .iter()
        .enumerate()
        .filter(|(index, _)| index % HOLDOUT_STRIDE == HOLDOUT_STRIDE - 1)
        .map(|(_, key)| *key)
        .collect();
    let mut split = Split::default();
    for pair in pairs {
        let plus_held = held_out_media.contains(pair.plus_media.as_str());
        let minus_held = held_out_media.contains(pair.minus_media.as_str());
        match (plus_held, minus_held) {
            (true, true) => split.held_out.push(pair),
            (false, false) => split.train.push(pair),
            _ => split.straddling_pairs += 1,
        }
    }
    split
}

pub struct EvalOutcome {
    pub held_out_pairs: usize,
    /// Accuracy of the composed production ranker (general margin + scaled residual).
    pub personal_accuracy: f64,
    /// Accuracy of the residual margin alone, reported for transparency.
    pub residual_only_accuracy: f64,
    pub baseline_accuracy: f64,
    /// Pairs dropped for straddling the media partition.
    pub straddling_pairs: usize,
    pub learned: bool,
}

/// Pairwise ranking accuracy of the trained residual against the non-personalized baseline.
///
/// A pair counts for the personal model when the composed margin
/// `general_margin + PERSONAL_AFFINITY_SCALE * residual_margin` is positive, and for the
/// baseline when the general margin alone is positive. Zero margins are ties and count as
/// failures for the personal model: it must strictly beat the baseline, never merely match it.
pub fn evaluate(held_out: &[&RankedPair], weights: &[f64]) -> EvalOutcome {
    let mut personal_votes = 0.0_f64;
    let mut residual_votes = 0.0_f64;
    let mut baseline_votes = 0.0_f64;
    for pair in held_out {
        let residual = pair
            .margin_features
            .iter()
            .zip(weights)
            .map(|(feature, weight)| feature * weight)
            .sum::<f64>();
        if pair.general_margin + PERSONAL_AFFINITY_SCALE * residual > 0.0 {
            personal_votes += 1.0;
        }
        if residual > 0.0 {
            residual_votes += 1.0;
        }
        if pair.general_margin > 0.0 {
            baseline_votes += 1.0;
        }
    }
    let count = held_out.len() as f64;
    let personal_accuracy = if held_out.is_empty() {
        0.0
    } else {
        personal_votes / count
    };
    let residual_only_accuracy = if held_out.is_empty() {
        0.0
    } else {
        residual_votes / count
    };
    let baseline_accuracy = if held_out.is_empty() {
        0.0
    } else {
        baseline_votes / count
    };
    let learned = held_out.len() >= MIN_HELD_OUT_PAIRS
        && personal_accuracy > baseline_accuracy
        && personal_accuracy >= MIN_PERSONAL_ACCURACY;
    EvalOutcome {
        held_out_pairs: held_out.len(),
        personal_accuracy,
        residual_only_accuracy,
        baseline_accuracy,
        straddling_pairs: 0,
        learned,
    }
}

/// Metrics recorded verbatim on the profile row for UI and auditability.
pub fn metrics_json(outcome: &EvalOutcome) -> String {
    serde_json::json!({
        "held_out_pairs": outcome.held_out_pairs,
        "personal_accuracy": outcome.personal_accuracy,
        "residual_only_accuracy": outcome.residual_only_accuracy,
        "baseline_accuracy": outcome.baseline_accuracy,
        "straddling_pairs": outcome.straddling_pairs,
        "learned": outcome.learned,
        "split": SPLIT_LABEL,
        "trainer": crate::style::trainer::TRAINER_VERSION,
        "personal_scale": PERSONAL_AFFINITY_SCALE,
    })
    .to_string()
}
