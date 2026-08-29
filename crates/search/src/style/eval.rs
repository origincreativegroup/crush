//! Held-out evaluation gate for personal style profiles.
//!
//! A profile may only be marked `learned` when held-out preference pairs strictly improve on
//! the non-personalized baseline (docs/dam-feedback-blueprint.md: "A style profile cannot be
//! called learned without held-out improvement over the general ranker"). The split is
//! deterministic: every third pair of the deterministically ordered pair list is held out and
//! the rest train, so the same evidence always produces the same metrics.

/// Pairs held out for evaluation; the gate refuses to declare learning below this floor.
pub const MIN_HELD_OUT_PAIRS: usize = 4;
/// Personal accuracy must clear this floor even when it beats the baseline.
pub const MIN_PERSONAL_ACCURACY: f64 = 0.6;
/// Every k-th pair (k = 3) of the deterministic pair order is held out.
const HOLDOUT_STRIDE: usize = 3;
pub const SPLIT_LABEL: &str = "loo-every-3rd";

/// One deterministic train/eval pair: the trained residual must rank `plus` above `minus`.
#[derive(Debug, Clone)]
pub struct RankedPair {
    /// `features(plus) - features(minus)`; the margin is `dot(weights, margin_features)`.
    pub margin_features: Vec<f64>,
    /// Evidence strength of the pair (strongest for prefer events and curated examples).
    pub weight: f64,
    /// Baseline vote for this pair: 1.0 when the general `overall` ordering already prefers
    /// `plus`, 0.0 when it prefers `minus`, and 0.5 when unavailable or tied.
    pub baseline_vote: f64,
}

/// Deterministic train/held-out split over an ordered pair list: every third pair held out.
pub fn split_pairs(pairs: &[RankedPair]) -> (Vec<&RankedPair>, Vec<&RankedPair>) {
    let mut train = Vec::new();
    let mut held_out = Vec::new();
    for (index, pair) in pairs.iter().enumerate() {
        if index % HOLDOUT_STRIDE == HOLDOUT_STRIDE - 1 {
            held_out.push(pair);
        } else {
            train.push(pair);
        }
    }
    (train, held_out)
}

/// Baseline vote from the general aesthetic `overall` scores, or 0.5 when unavailable/tied.
pub fn baseline_vote(plus_overall: Option<f64>, minus_overall: Option<f64>) -> f64 {
    match (plus_overall, minus_overall) {
        (Some(plus), Some(minus)) if plus > minus => 1.0,
        (Some(plus), Some(minus)) if plus < minus => 0.0,
        _ => 0.5,
    }
}

pub struct EvalOutcome {
    pub held_out_pairs: usize,
    pub personal_accuracy: f64,
    pub baseline_accuracy: f64,
    pub learned: bool,
}

/// Pairwise ranking accuracy of the trained residual against the non-personalized baseline.
/// Ties count as failures for the personal model: it must strictly beat the baseline, never
/// merely match it.
pub fn evaluate(held_out: &[&RankedPair], weights: &[f64]) -> EvalOutcome {
    let mut personal_votes = 0.0_f64;
    let mut baseline_votes = 0.0_f64;
    for pair in held_out {
        let margin = pair
            .margin_features
            .iter()
            .zip(weights)
            .map(|(feature, weight)| feature * weight)
            .sum::<f64>();
        if margin > 0.0 {
            personal_votes += 1.0;
        }
        baseline_votes += pair.baseline_vote;
    }
    let count = held_out.len() as f64;
    let personal_accuracy = if held_out.is_empty() {
        0.0
    } else {
        personal_votes / count
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
        baseline_accuracy,
        learned,
    }
}

/// Metrics recorded verbatim on the profile row for UI and auditability.
pub fn metrics_json(outcome: &EvalOutcome) -> String {
    serde_json::json!({
        "held_out_pairs": outcome.held_out_pairs,
        "personal_accuracy": outcome.personal_accuracy,
        "baseline_accuracy": outcome.baseline_accuracy,
        "learned": outcome.learned,
        "split": SPLIT_LABEL,
        "trainer": crate::style::trainer::TRAINER_VERSION,
    })
    .to_string()
}
