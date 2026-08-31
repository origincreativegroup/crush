//! The `personal-residual-v1` trainer.
//!
//! The personal head is a residual over the untouched general ranker: it starts at zero, so
//! with no evidence the ranker is exactly the general ranker. Evidence comes from pairwise
//! `prefer` events, pick/reject and rating labels (with the blueprint's signal strengths),
//! weak workflow positives (export 0.5, crop/grade/tag/edit 0.25), and confirmed reference-set
//! examples at curated strength 1.0. The objective is a weighted logistic pairwise loss with
//! L2 regularization that strengthens as evidence grows sparse, a hard weight-norm cap, and a
//! minimum-samples floor below which the trainer returns `Ok(None)` and leaves the previous
//! profile untouched. Sparse feedback therefore regularizes toward the general model and never
//! invents certainty.
//!
//! Everything here is hand-written and deterministic: fixed iteration count, fixed learning
//! rate, deterministically ordered pairs, and a deterministic every-third-pair held-out split.

use std::collections::{BTreeMap, HashSet};

use anyhow::ensure;
use crush_store::{FeedbackEvent, FeedbackSignal, MediaKind, Store, StyleProfile};
use serde_json::json;

use crate::{
    media_vector,
    style::eval::{self, RankedPair},
    AESTHETIC_FEATURES, EMBEDDING_DIM,
};

pub const TRAINER_VERSION: &str = "personal-residual-v1";
pub const DEFAULT_CONTEXT_KEY: &str = "default";
/// Evidence floor for the default context; below it the previous profile is left untouched.
pub const DEFAULT_MIN_SAMPLES: usize = 6;
/// Named contexts describe a narrower slice of work, so they may learn with less evidence.
pub const NAMED_CONTEXT_MIN_SAMPLES: usize = 4;

const BASE_LAMBDA: f64 = 0.1;
const LEARNING_RATE: f64 = 0.25;
const GRADIENT_ITERATIONS: usize = 240;
const MAX_WEIGHT_NORM: f64 = 4.0;
const MAX_POOL_PER_SIDE: usize = 64;

/// Rebuild the default-context style profile. Kept as the name callers use.
pub fn retrain_style_profile(
    store: &mut Store,
    owner_id: &str,
) -> anyhow::Result<Option<StyleProfile>> {
    retrain_style_profile_for_context(store, owner_id, DEFAULT_CONTEXT_KEY)
}

/// Rebuild the active style profile for one context key. Returns `Ok(None)` — leaving the
/// previous profile untouched — when the evidence is below the minimum-samples floor.
pub fn retrain_style_profile_for_context(
    store: &mut Store,
    owner_id: &str,
    context_key: &str,
) -> anyhow::Result<Option<StyleProfile>> {
    ensure!(
        !context_key.trim().is_empty(),
        "context key must not be empty"
    );
    let context_key = context_key.trim();
    let evidence = gather_evidence(store, owner_id, context_key)?;
    let minimum_samples = if context_key == DEFAULT_CONTEXT_KEY {
        DEFAULT_MIN_SAMPLES
    } else {
        NAMED_CONTEXT_MIN_SAMPLES
    };
    if evidence.sample_count < minimum_samples {
        return Ok(None);
    }
    let pairs = build_pairs(&evidence);
    if pairs.is_empty() {
        return Ok(None);
    }
    let split = eval::split_pairs(&pairs);
    let lambda = BASE_LAMBDA / (1.0 + split.train.len() as f64);
    let weights = train_weights(&split.train, lambda);
    let mut outcome = eval::evaluate(&split.held_out, &weights);
    outcome.straddling_pairs = split.straddling_pairs;

    let clip_weights = weights[..EMBEDDING_DIM]
        .iter()
        .map(|value| *value as f32)
        .collect::<Vec<_>>();
    let clip_residual_norm = clip_weights
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    let mut feature_weights_map = serde_json::Map::new();
    for (name, value) in AESTHETIC_FEATURES.iter().zip(&weights[EMBEDDING_DIM..]) {
        feature_weights_map.insert((*name).to_owned(), json!(value));
    }
    let feature_weights_json = json!({
        "trainer": TRAINER_VERSION,
        "lambda": lambda,
        "clip_residual_norm": clip_residual_norm,
        "feature_weights": feature_weights_map,
    })
    .to_string();

    let previous_version = store
        .style_profiles_for_context(owner_id, context_key)?
        .iter()
        .map(|profile| profile.version)
        .max()
        .unwrap_or(0);
    let profile = StyleProfile {
        id: uuid::Uuid::new_v4().to_string(),
        owner_id: owner_id.to_owned(),
        name: context_key.to_owned(),
        version: previous_version + 1,
        algorithm_version: TRAINER_VERSION.to_owned(),
        embedding_weights: clip_weights,
        feature_weights_json,
        sample_count: evidence.sample_count as i64,
        held_out_metric: Some(outcome.personal_accuracy),
        baseline_metric: Some(outcome.baseline_accuracy),
        context_key: context_key.to_owned(),
        metrics_json: eval::metrics_json(&outcome),
        learned: outcome.learned,
        active: true,
        trained_at: chrono::Utc::now(),
    };
    store.put_style_profile(owner_id, &profile)?;
    Ok(Some(profile))
}

#[derive(Debug, Clone)]
struct Sample {
    media_kind: MediaKind,
    media_id: String,
    /// Signed evidence strength in [-1, 1]; the sign separates the pair pools.
    label: f32,
    vector: Vec<f32>,
    aesthetic: [f32; AESTHETIC_FEATURES.len()],
    overall: Option<f64>,
}

impl Sample {
    fn pool_key(&self) -> String {
        format!("{}:{}", media_kind_tag(self.media_kind), self.media_id)
    }
}

#[derive(Debug, Default)]
struct Evidence {
    prefer_pairs: Vec<(Sample, Sample)>,
    positives: Vec<Sample>,
    negatives: Vec<Sample>,
    /// Distinct media contributing to at least one pair; the trainer's sample count.
    sample_count: usize,
}

/// Collect the owner's evidence for one context. Feedback is partitioned by the context key
/// inside `context_json` (`{"context": "<key>"}`; undefined or `"default"` collapses to the
/// default), so a preference in one context never becomes a universal rule. Curated positives
/// come only from confirmed reference sets; unconfirmed or disabled sets are inert.
fn gather_evidence(store: &Store, owner_id: &str, context_key: &str) -> anyhow::Result<Evidence> {
    let mut evidence = Evidence::default();
    // Deduplicate pool samples by media, keeping the strongest label seen for each asset.
    // A BTreeMap keeps the sample order deterministic, which fixes the pair order and with it
    // the held-out split: the same evidence always produces the same profile.
    let mut pools: BTreeMap<String, Sample> = BTreeMap::new();
    let mut media_keys = HashSet::new();
    for event in store.feedback_events(owner_id)? {
        if event_context_key(&event) != context_key {
            continue;
        }
        if event.signal == FeedbackSignal::Prefer {
            if let (Some(kind), Some(id)) = (&event.compared_media_kind, &event.compared_media_id) {
                if let Some((plus, minus)) =
                    prefer_pair(store, owner_id, &event, *kind, id.as_str())?
                {
                    media_keys.insert(plus.pool_key());
                    media_keys.insert(minus.pool_key());
                    evidence.prefer_pairs.push((plus, minus));
                }
            }
            continue;
        }
        let label = event_label(&event);
        if label == 0.0 {
            continue;
        }
        if let Some(sample) =
            load_sample(store, owner_id, event.media_kind, &event.media_id, label)?
        {
            media_keys.insert(sample.pool_key());
            pools
                .entry(sample.pool_key())
                .and_modify(|existing| {
                    if label.abs() > existing.label.abs() {
                        *existing = sample.clone();
                    }
                })
                .or_insert(sample);
        }
    }
    for (media_kind, media_id) in store.reference_set_confirmed_items(owner_id, context_key)? {
        if let Some(sample) = load_sample(store, owner_id, media_kind, &media_id, 1.0)? {
            media_keys.insert(sample.pool_key());
            evidence.positives.push(sample);
        }
    }
    for sample in pools.into_values() {
        if sample.label > 0.0 {
            evidence.positives.push(sample);
        } else {
            evidence.negatives.push(sample);
        }
    }
    evidence.sample_count = media_keys.len();
    Ok(evidence)
}

fn media_kind_tag(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Shot => "shot",
    }
}

/// The context key inside `context_json`; undefined or `"default"` collapses to `"default"`.
fn event_context_key(event: &FeedbackEvent) -> String {
    serde_json::from_str::<serde_json::Value>(&event.context_json)
        .ok()
        .and_then(|value| {
            value
                .get("context")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| DEFAULT_CONTEXT_KEY.to_owned())
}

/// Blueprint signal strengths: pick/publish 1.0, reject -1.0, rating (v-3)/2, prefer +1,
/// export 0.5, crop/grade/tag/edit 0.25. A neutral rating collapses to 0 and is skipped.
fn event_label(event: &FeedbackEvent) -> f32 {
    let label = match event.signal {
        FeedbackSignal::Pick | FeedbackSignal::Publish => 1.0,
        FeedbackSignal::Reject => -1.0,
        FeedbackSignal::Rating => event.value.map_or(0.0, |value| (value - 3.0) / 2.0) as f32,
        FeedbackSignal::Prefer => 1.0,
        FeedbackSignal::Export => 0.5,
        FeedbackSignal::Crop
        | FeedbackSignal::Grade
        | FeedbackSignal::Tag
        | FeedbackSignal::Edit => 0.25,
    };
    label.clamp(-1.0, 1.0)
}

/// Confirmed reference-set examples enter as positives at curated strength 1.0 (the same tier
/// as pick/publish); feedback_events stays purely user-action evidence.
fn load_sample(
    store: &Store,
    owner_id: &str,
    media_kind: MediaKind,
    media_id: &str,
    label: f32,
) -> anyhow::Result<Option<Sample>> {
    let Some(vector) = media_vector(store, owner_id, media_kind, media_id)? else {
        return Ok(None);
    };
    if vector.len() != EMBEDDING_DIM {
        return Ok(None);
    }
    let assessment = store.aesthetic_assessment(owner_id, media_kind, media_id)?;
    let aesthetic = crate::aesthetic_feature_vector(assessment.as_ref());
    let overall = assessment.as_ref().map(|value| value.overall);
    Ok(Some(Sample {
        media_kind,
        media_id: media_id.to_owned(),
        label,
        vector,
        aesthetic,
        overall,
    }))
}

fn prefer_pair(
    store: &Store,
    owner_id: &str,
    event: &FeedbackEvent,
    compared_kind: MediaKind,
    compared_id: &str,
) -> anyhow::Result<Option<(Sample, Sample)>> {
    let Some(plus) = load_sample(store, owner_id, event.media_kind, &event.media_id, 1.0)? else {
        return Ok(None);
    };
    let Some(minus) = load_sample(store, owner_id, compared_kind, compared_id, -1.0)? else {
        return Ok(None);
    };
    Ok(Some((plus, minus)))
}

fn build_pairs(evidence: &Evidence) -> Vec<RankedPair> {
    // Merge every evidence source into one map keyed by the ordered media pair, netting
    // conflicting directions explicitly: a reverse pair subtracts from the forward one, and a
    // fully cancelled pair is dropped rather than allowed to invent certainty. Repeated
    // evidence accumulates weight; it never duplicates rows in the training set.
    let mut merged: BTreeMap<(String, String), (f64, RankedPair)> = BTreeMap::new();
    let mut insert = |pair: RankedPair| {
        let forward = pair.plus_media <= pair.minus_media;
        let (key, signed) = if forward {
            (
                (pair.plus_media.clone(), pair.minus_media.clone()),
                pair.weight,
            )
        } else {
            (
                (pair.minus_media.clone(), pair.plus_media.clone()),
                -pair.weight,
            )
        };
        let base = if forward { pair } else { flip_pair(&pair) };
        let entry = merged.entry(key).or_insert_with(|| (0.0, base));
        entry.0 += signed;
    };
    for (plus, minus) in &evidence.prefer_pairs {
        insert(ranked_pair(plus, minus, 1.0));
    }
    for plus in cap_pool(&evidence.positives) {
        for minus in cap_pool(&evidence.negatives) {
            let weight = f64::from(plus.label.abs().min(minus.label.abs()));
            insert(ranked_pair(plus, minus, weight));
        }
    }
    merged
        .into_values()
        .filter(|(net, _)| net.abs() > f64::EPSILON)
        .map(|(net, mut pair)| {
            if net < 0.0 {
                pair = flip_pair(&pair);
            }
            pair.weight = net.abs();
            pair
        })
        .collect()
}

/// The same pair with its sides exchanged; margins and media keys negate together.
fn flip_pair(pair: &RankedPair) -> RankedPair {
    RankedPair {
        margin_features: pair.margin_features.iter().map(|value| -value).collect(),
        weight: pair.weight,
        plus_media: pair.minus_media.clone(),
        minus_media: pair.plus_media.clone(),
        general_margin: -pair.general_margin,
    }
}

fn ranked_pair(plus: &Sample, minus: &Sample, weight: f64) -> RankedPair {
    let mut margin_features = vec![0.0_f64; EMBEDDING_DIM + AESTHETIC_FEATURES.len()];
    for (index, value) in plus.vector.iter().enumerate() {
        margin_features[index] += f64::from(*value);
    }
    for (index, value) in minus.vector.iter().enumerate() {
        margin_features[index] -= f64::from(*value);
    }
    for (index, value) in plus.aesthetic.iter().enumerate() {
        margin_features[EMBEDDING_DIM + index] += f64::from(*value);
    }
    for (index, value) in minus.aesthetic.iter().enumerate() {
        margin_features[EMBEDDING_DIM + index] -= f64::from(*value);
    }
    // The general ranker's pair margin without any personal term: the difference of the
    // general aesthetic `overall` signal when both sides have assessments, else 0.0. Its sign
    // is also the non-personalized baseline vote, so evaluation has one source of truth.
    let general_margin = match (plus.overall, minus.overall) {
        (Some(plus_overall), Some(minus_overall)) => plus_overall - minus_overall,
        _ => 0.0,
    };
    RankedPair {
        margin_features,
        weight,
        plus_media: plus.pool_key(),
        minus_media: minus.pool_key(),
        general_margin,
    }
}

/// Bound cross-product pair growth deterministically: stride-sample each pool to at most
/// `MAX_POOL_PER_SIDE` samples, always keeping the first sample of every stride.
fn cap_pool(pool: &[Sample]) -> Vec<&Sample> {
    if pool.len() <= MAX_POOL_PER_SIDE {
        return pool.iter().collect();
    }
    let stride = pool.len() as f64 / MAX_POOL_PER_SIDE as f64;
    (0..MAX_POOL_PER_SIDE)
        .map(|index| &pool[(index as f64 * stride) as usize])
        .collect()
}

/// Batch gradient descent on the weighted logistic pairwise loss with L2 regularization.
fn train_weights(train: &[&RankedPair], lambda: f64) -> Vec<f64> {
    let mut weights = vec![0.0_f64; EMBEDDING_DIM + AESTHETIC_FEATURES.len()];
    let mut gradient = vec![0.0_f64; weights.len()];
    for _ in 0..GRADIENT_ITERATIONS {
        for value in gradient.iter_mut() {
            *value = 0.0;
        }
        for pair in train {
            let margin = pair
                .margin_features
                .iter()
                .zip(weights.iter())
                .map(|(feature, weight)| feature * weight)
                .sum::<f64>();
            let error = sigmoid(-margin);
            for (value, feature) in gradient.iter_mut().zip(&pair.margin_features) {
                *value -= pair.weight * error * feature;
            }
        }
        for (value, weight) in gradient.iter_mut().zip(weights.iter()) {
            *value += 2.0 * lambda * weight;
        }
        for (weight, value) in weights.iter_mut().zip(gradient.iter()) {
            *weight -= LEARNING_RATE * value;
        }
        cap_norm(&mut weights, MAX_WEIGHT_NORM);
    }
    weights
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponential = value.exp();
        exponential / (1.0 + exponential)
    }
}

fn cap_norm(weights: &mut [f64], max_norm: f64) {
    let norm = weights
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm > max_norm {
        let scale = max_norm / norm;
        for value in weights.iter_mut() {
            *value *= scale;
        }
    }
}
