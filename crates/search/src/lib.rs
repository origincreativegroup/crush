//! In-process cosine search with a deliberately small transcript keyword boost.
//!
//! This crate does not depend on ONNX Runtime. Callers supply text embeddings through
//! [`TextEmbedder`], keeping the index and ranking logic independently testable.

use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashMap},
};

use anyhow::{ensure, Context};
use crush_store::{MediaKind, Store, StrongAsset, StyleProfile};
use serde::Serialize;

pub mod sequence;
pub mod style;

pub use style::trainer::{retrain_style_profile, retrain_style_profile_for_context};

pub const EMBEDDING_DIM: usize = 512;

/// Weight of the personal residual term in the composed score (the previously magic 0.15).
pub const PERSONAL_WEIGHT: f32 = 0.15;

/// Weight of the general aesthetic `overall` adjustment in the composed score (the previously
/// magic 0.16). The style evaluation composes the same weight beside [`PERSONAL_WEIGHT`] so
/// the eval margin and the production margin cannot drift.
pub const GENERAL_AESTHETIC_WEIGHT: f64 = 0.16;

/// Named persisted aesthetic features the personal residual may weight, in fixed order.
/// `aesthetic_feature_vector` fills them in exactly this order from an assessment row.
pub(crate) const AESTHETIC_FEATURES: [&str; 28] = [
    "sharpness",
    "exposure",
    "contrast",
    "color_harmony",
    "balance",
    "subject_placement",
    "negative_space",
    "visual_clarity",
    "technical_quality",
    "blur_control",
    "clipping_control",
    "noise_control",
    "compression_quality",
    "resolution_quality",
    "motion_stability",
    "duplicate_confidence",
    "composition_quality",
    "hierarchy",
    "leading_lines",
    "symmetry",
    "crop_potential",
    "moment_story",
    "expression",
    "gesture",
    "action",
    "novelty",
    "pacing",
    "repetition_risk",
];

const STOPWORDS: &[&str] = &[
    "and", "are", "but", "for", "from", "has", "have", "into", "not", "that", "the", "this", "was",
    "were", "with", "you", "your",
];

pub trait TextEmbedder {
    fn embed_text(&mut self, text: &str) -> anyhow::Result<[f32; EMBEDDING_DIM]>;
}

impl<F> TextEmbedder for F
where
    F: FnMut(&str) -> anyhow::Result<[f32; EMBEDDING_DIM]>,
{
    fn embed_text(&mut self, text: &str) -> anyhow::Result<[f32; EMBEDDING_DIM]> {
        self(text)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub shot_id: String,
    pub score: f32,
    pub cosine: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub shot_id: String,
    pub video_path: String,
    pub start_s: f64,
    pub end_s: f64,
    pub thumb_path: Option<String>,
    pub score: f32,
    pub cosine: f32,
    pub transcript_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssetSearchResult {
    pub asset_type: String,
    pub asset_id: String,
    pub path: String,
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub thumb_path: Option<String>,
    pub score: f32,
    pub cosine: f32,
    pub transcript_snippet: Option<String>,
    pub editorial_quality: Option<i64>,
    pub aesthetic_score: Option<f64>,
    pub personal_style_score: Option<f32>,
    pub score_breakdown: Option<ScoreBreakdown>,
}

/// Plain-language decomposition of one asset's composed score
/// (docs/dam-feedback-blueprint.md: "The UI should expose that breakdown in plain language").
/// The components sum to `total` (== `AssetSearchResult::score`) up to float tolerance; every
/// field is always exported, using `0.0` (never `null`) when a term is absent so consumers
/// never have to invent certainty.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScoreBreakdown {
    /// Cosine similarity between the query and the asset embedding.
    pub semantic: f32,
    /// Additive transcript keyword boost, `0.0` when the query has no transcript hits.
    pub transcript_boost: f32,
    /// Editorial annotation quality adjustment (the usable-false penalty is exported
    /// separately as `penalties` so general quality and safety stay individually readable).
    pub editorial: f32,
    /// General strong-shot aesthetic adjustment, ±0.08 around `overall` 0.5.
    pub general_aesthetic: f32,
    /// Safety/usability penalty (`-1.0` when the annotation marks the asset unusable).
    pub penalties: f32,
    /// Personal residual affinity for the default-context profile, already scaled by
    /// [`PERSONAL_WEIGHT`]; `0.0` when no profile passes the held-out evaluation gate.
    pub personal_affinity: f32,
    /// Affinity of the request's context profile beyond the default-context profile, scaled
    /// by [`PERSONAL_WEIGHT`]; `0.0` when no context was requested.
    pub context_fit: f32,
    /// The composed total: the sum of every component above.
    pub total: f32,
}

/// Resolve one stored media vector (photos and shots share the shape). The trainer and the
/// scoring path both read through this so personal affinity and training features agree.
pub(crate) fn media_vector(
    store: &Store,
    owner_id: &str,
    kind: MediaKind,
    id: &str,
) -> anyhow::Result<Option<Vec<f32>>> {
    match kind {
        MediaKind::Photo => store.vector_for_photo(owner_id, id),
        MediaKind::Shot => store.vector_for_shot(owner_id, id),
        // Imported spans carry catalogue evidence only; they are not embedded (Task 022).
        MediaKind::Span => Ok(None),
    }
}

#[derive(Debug, Clone)]
pub struct VectorIndex {
    owner_id: String,
    shot_ids: Vec<String>,
    matrix: Vec<f32>,
}

impl VectorIndex {
    pub fn load(store: &Store, owner_id: &str) -> anyhow::Result<Self> {
        validate_embedding_metadata(store, owner_id)?;
        let (shot_ids, matrix) = store.load_all_vectors(owner_id)?;
        Self::from_parts(owner_id, shot_ids, matrix)
    }

    fn load_photos(store: &Store, owner_id: &str) -> anyhow::Result<Self> {
        validate_embedding_metadata(store, owner_id)?;
        let (photo_ids, matrix) = store.load_all_photo_vectors(owner_id)?;
        Self::from_parts(owner_id, photo_ids, matrix)
    }

    fn from_parts(owner_id: &str, shot_ids: Vec<String>, matrix: Vec<f32>) -> anyhow::Result<Self> {
        ensure!(
            matrix.len() == shot_ids.len() * EMBEDDING_DIM,
            "vector matrix contains {} values for {} shots; expected dimension {EMBEDDING_DIM}",
            matrix.len(),
            shot_ids.len()
        );
        ensure!(
            matrix.iter().all(|value| value.is_finite()),
            "vector matrix contains non-finite values"
        );
        Ok(Self {
            owner_id: owner_id.to_owned(),
            shot_ids,
            matrix,
        })
    }

    pub fn reload(&mut self, store: &Store, owner_id: &str) -> anyhow::Result<()> {
        *self = Self::load(store, owner_id)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.shot_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shot_ids.is_empty()
    }

    pub fn search(
        &self,
        query_vector: &[f32],
        top_k: usize,
        owner_id: &str,
    ) -> anyhow::Result<Vec<VectorMatch>> {
        self.search_with_boosts(query_vector, top_k, owner_id, &HashMap::new(), 0.0)
    }

    fn search_with_boosts(
        &self,
        query_vector: &[f32],
        top_k: usize,
        owner_id: &str,
        transcript_hits: &HashMap<String, String>,
        transcript_hit_boost: f32,
    ) -> anyhow::Result<Vec<VectorMatch>> {
        ensure!(
            self.owner_id == owner_id,
            "vector index belongs to owner {:?}, not {:?}",
            self.owner_id,
            owner_id
        );
        ensure!(
            query_vector.len() == EMBEDDING_DIM,
            "query vector has dimension {}; expected {EMBEDDING_DIM}",
            query_vector.len()
        );
        ensure!(
            query_vector.iter().all(|value| value.is_finite()),
            "query vector contains non-finite values"
        );
        ensure!(
            transcript_hit_boost.is_finite() && transcript_hit_boost >= 0.0,
            "transcript hit boost must be finite and non-negative"
        );
        if top_k == 0 || self.shot_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut heap = BinaryHeap::with_capacity(top_k.min(self.shot_ids.len()));
        for (row, values) in self
            .matrix
            .as_chunks::<EMBEDDING_DIM>()
            .0
            .iter()
            .enumerate()
        {
            let cosine = dot_512(values, query_vector);
            let score = cosine
                + if transcript_hits.contains_key(&self.shot_ids[row]) {
                    transcript_hit_boost
                } else {
                    0.0
                };
            let candidate = RankedRow { row, score, cosine };
            if heap.len() < top_k {
                heap.push(Reverse(candidate));
            } else if heap.peek().is_some_and(|worst| candidate > worst.0) {
                let _ = heap.pop();
                heap.push(Reverse(candidate));
            }
        }

        let mut ranked = heap.into_iter().map(|entry| entry.0).collect::<Vec<_>>();
        ranked.sort_unstable_by(|left, right| right.cmp(left));
        Ok(ranked
            .into_iter()
            .map(|entry| VectorMatch {
                shot_id: self.shot_ids[entry.row].clone(),
                score: entry.score,
                cosine: entry.cosine,
            })
            .collect())
    }
}

pub struct SearchEngine {
    index: VectorIndex,
    photo_index: VectorIndex,
    owner_id: String,
    transcript_hit_boost: f32,
}

impl SearchEngine {
    pub fn load(store: &Store, owner_id: &str, transcript_hit_boost: f32) -> anyhow::Result<Self> {
        ensure!(
            transcript_hit_boost.is_finite() && transcript_hit_boost >= 0.0,
            "transcript hit boost must be finite and non-negative"
        );
        Ok(Self {
            index: VectorIndex::load(store, owner_id)?,
            photo_index: VectorIndex::load_photos(store, owner_id)?,
            owner_id: owner_id.to_owned(),
            transcript_hit_boost,
        })
    }

    pub fn reload(&mut self, store: &Store) -> anyhow::Result<()> {
        self.index.reload(store, &self.owner_id)?;
        self.photo_index = VectorIndex::load_photos(store, &self.owner_id)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.index.len() + self.photo_index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty() && self.photo_index.is_empty()
    }

    pub fn search<E: TextEmbedder>(
        &self,
        store: &Store,
        embedder: &mut E,
        query: &str,
        top_k: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        ensure!(!query.trim().is_empty(), "search query must not be empty");
        ensure!(top_k > 0, "top must be greater than zero");

        let fts_query = transcript_fts_query(query);
        let mut transcript_hits = HashMap::new();
        if let Some(fts_query) = fts_query {
            for hit in store.transcript_shot_hits(&self.owner_id, &fts_query)? {
                transcript_hits.entry(hit.shot_id).or_insert(hit.text);
            }
        }

        let query_vector = embedder.embed_text(query)?;
        let matches = self.index.search_with_boosts(
            &query_vector,
            top_k,
            &self.owner_id,
            &transcript_hits,
            self.transcript_hit_boost,
        )?;
        let mut results = Vec::with_capacity(matches.len());
        for found in matches {
            let context = store
                .search_shot_context(&self.owner_id, &found.shot_id)?
                .with_context(|| format!("indexed shot {} no longer exists", found.shot_id))?;
            let transcript_snippet = if let Some(text) = transcript_hits.get(&found.shot_id) {
                Some(snippet(text))
            } else {
                store
                    .segments_overlapping(
                        &self.owner_id,
                        &context.video_id,
                        context.start_s,
                        context.end_s,
                    )?
                    .first()
                    .map(|segment| snippet(&segment.text))
            };
            let thumb_path = context
                .thumb_rel
                .as_deref()
                .map(|relative| store.thumbnail_path(relative))
                .transpose()?
                .map(|path| path.display().to_string());
            results.push(SearchResult {
                shot_id: found.shot_id,
                video_path: context.video_path,
                start_s: context.start_s,
                end_s: context.end_s,
                thumb_path,
                score: found.score,
                cosine: found.cosine,
                transcript_snippet,
            });
        }
        tracing::info!(
            job_id = "search",
            stage = "search",
            owner_id = self.owner_id,
            query,
            indexed_shots = self.index.len(),
            results = results.len(),
            "search complete"
        );
        Ok(results)
    }

    pub fn search_assets<E: TextEmbedder>(
        &self,
        store: &Store,
        embedder: &mut E,
        query: &str,
        top_k: usize,
    ) -> anyhow::Result<Vec<AssetSearchResult>> {
        self.search_assets_in_context(store, embedder, query, top_k, None)
    }

    /// Mixed-media search with an optional context key: a per-context profile adjusts ranking
    /// on top of the default-context profile (see [`ScoreBreakdown::context_fit`]). Without a
    /// context key the ranking is identical to [`SearchEngine::search_assets`].
    pub fn search_assets_in_context<E: TextEmbedder>(
        &self,
        store: &Store,
        embedder: &mut E,
        query: &str,
        top_k: usize,
        context_key: Option<&str>,
    ) -> anyhow::Result<Vec<AssetSearchResult>> {
        ensure!(!query.trim().is_empty(), "search query must not be empty");
        ensure!(top_k > 0, "top must be greater than zero");

        let mut transcript_hits = HashMap::new();
        if let Some(fts_query) = transcript_fts_query(query) {
            for hit in store.transcript_shot_hits(&self.owner_id, &fts_query)? {
                transcript_hits.entry(hit.shot_id).or_insert(hit.text);
            }
        }
        let query_vector = embedder.embed_text(query)?;
        let shot_matches = self.index.search_with_boosts(
            &query_vector,
            top_k,
            &self.owner_id,
            &transcript_hits,
            self.transcript_hit_boost,
        )?;
        let photo_matches = self
            .photo_index
            .search(&query_vector, top_k, &self.owner_id)?;
        let personal = PersonalScorer::load(store, &self.owner_id, context_key)?;
        let mut results = Vec::with_capacity(shot_matches.len() + photo_matches.len());

        for found in shot_matches {
            let context = store
                .search_shot_context(&self.owner_id, &found.shot_id)?
                .with_context(|| format!("indexed shot {} no longer exists", found.shot_id))?;
            let transcript_snippet = if let Some(text) = transcript_hits.get(&found.shot_id) {
                Some(snippet(text))
            } else {
                store
                    .segments_overlapping(
                        &self.owner_id,
                        &context.video_id,
                        context.start_s,
                        context.end_s,
                    )?
                    .first()
                    .map(|segment| snippet(&segment.text))
            };
            let annotation =
                store.editorial_annotation(&self.owner_id, MediaKind::Shot, &found.shot_id)?;
            let aesthetic =
                store.aesthetic_assessment(&self.owner_id, MediaKind::Shot, &found.shot_id)?;
            let vector = media_vector(store, &self.owner_id, MediaKind::Shot, &found.shot_id)?;
            let aesthetic_features = aesthetic_feature_vector(aesthetic.as_ref());
            let (score, breakdown, personal_style_score) = compose_score(
                found.score,
                found.cosine,
                annotation.as_ref(),
                aesthetic.as_ref(),
                vector.as_ref(),
                &aesthetic_features,
                &personal,
            );
            results.push(AssetSearchResult {
                asset_type: "video".to_owned(),
                asset_id: found.shot_id,
                path: context.video_path,
                start_s: Some(context.start_s),
                end_s: Some(context.end_s),
                thumb_path: context
                    .thumb_rel
                    .as_deref()
                    .map(|relative| store.thumbnail_path(relative))
                    .transpose()?
                    .map(|path| path.display().to_string()),
                score,
                cosine: found.cosine,
                transcript_snippet,
                editorial_quality: annotation.as_ref().and_then(|value| value.quality),
                aesthetic_score: aesthetic.as_ref().map(|value| value.overall),
                personal_style_score,
                score_breakdown: Some(breakdown),
            });
        }

        for found in photo_matches {
            let photo = store
                .photo_by_id(&self.owner_id, &found.shot_id)?
                .with_context(|| format!("indexed photo {} no longer exists", found.shot_id))?;
            let annotation =
                store.editorial_annotation(&self.owner_id, MediaKind::Photo, &found.shot_id)?;
            let aesthetic =
                store.aesthetic_assessment(&self.owner_id, MediaKind::Photo, &found.shot_id)?;
            let vector = media_vector(store, &self.owner_id, MediaKind::Photo, &found.shot_id)?;
            let aesthetic_features = aesthetic_feature_vector(aesthetic.as_ref());
            let (score, breakdown, personal_style_score) = compose_score(
                found.score,
                found.cosine,
                annotation.as_ref(),
                aesthetic.as_ref(),
                vector.as_ref(),
                &aesthetic_features,
                &personal,
            );
            results.push(AssetSearchResult {
                asset_type: "photo".to_owned(),
                asset_id: found.shot_id,
                path: photo.path,
                start_s: None,
                end_s: None,
                thumb_path: photo
                    .thumb_rel
                    .as_deref()
                    .map(|relative| store.thumbnail_path(relative))
                    .transpose()?
                    .map(|path| path.display().to_string()),
                score,
                cosine: found.cosine,
                transcript_snippet: None,
                editorial_quality: annotation.as_ref().and_then(|value| value.quality),
                aesthetic_score: aesthetic.as_ref().map(|value| value.overall),
                personal_style_score,
                score_breakdown: Some(breakdown),
            });
        }

        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.asset_id.cmp(&right.asset_id))
        });
        results.truncate(top_k);
        tracing::info!(
            job_id = "search",
            stage = "search",
            owner_id = self.owner_id,
            query,
            indexed_assets = self.len(),
            results = results.len(),
            "mixed-media search complete"
        );
        Ok(results)
    }
}

fn general_aesthetic_adjustment(assessment: Option<&crush_store::AestheticAssessment>) -> f32 {
    assessment.map_or(0.0, |value| {
        ((value.overall - 0.5) * GENERAL_AESTHETIC_WEIGHT) as f32
    })
}

/// The general editorial term with the usability penalty moved out, so the breakdown can show
/// general quality and penalties separately. `editorial_quality_adjustment +
/// editorial_penalty` equals the original single term in every case: a not-usable annotation
/// contributes the `-1.0` penalty and no quality term, a usable one contributes no penalty.
fn editorial_quality_adjustment(annotation: Option<&crush_store::EditorialAnnotation>) -> f32 {
    let Some(annotation) = annotation else {
        return 0.0;
    };
    if !annotation.usable {
        return 0.0;
    }
    let quality = annotation
        .quality
        .map_or(0.0, |value| (value as f32 - 3.0) * 0.025);
    quality + if annotation.standout { 0.05 } else { 0.0 }
}

fn editorial_penalty(annotation: Option<&crush_store::EditorialAnnotation>) -> f32 {
    match annotation {
        Some(annotation) if !annotation.usable => -1.0,
        _ => 0.0,
    }
}

/// Aesthetic features in [`AESTHETIC_FEATURES`] order, centered/scaled to [-0.5, 0.5] for the
/// personal residual; the raw `overall` stays untouched for the general adjustment. With no
/// assessment every feature is 0.0, so the aesthetic residual contributes nothing.
fn aesthetic_feature_vector(
    assessment: Option<&crush_store::AestheticAssessment>,
) -> [f32; AESTHETIC_FEATURES.len()] {
    let mut features = [0.0; AESTHETIC_FEATURES.len()];
    let Some(assessment) = assessment else {
        return features;
    };
    for (feature, name) in features.iter_mut().zip(AESTHETIC_FEATURES) {
        let value = aesthetic_component(assessment, name);
        *feature = (value - 0.5) as f32;
    }
    features
}

fn aesthetic_component(assessment: &crush_store::AestheticAssessment, name: &str) -> f64 {
    match name {
        "sharpness" => assessment.sharpness,
        "exposure" => assessment.exposure,
        "contrast" => assessment.contrast,
        "color_harmony" => assessment.color_harmony,
        "balance" => assessment.balance,
        "subject_placement" => assessment.subject_placement,
        "negative_space" => assessment.negative_space,
        "visual_clarity" => assessment.visual_clarity,
        "technical_quality" => assessment.technical_quality,
        "blur_control" => assessment.blur_control,
        "clipping_control" => assessment.clipping_control,
        "noise_control" => assessment.noise_control,
        "compression_quality" => assessment.compression_quality,
        "resolution_quality" => assessment.resolution_quality,
        "motion_stability" => assessment.motion_stability,
        "duplicate_confidence" => assessment.duplicate_confidence,
        "composition_quality" => assessment.composition_quality,
        "hierarchy" => assessment.hierarchy,
        "leading_lines" => assessment.leading_lines,
        "symmetry" => assessment.symmetry,
        "crop_potential" => assessment.crop_potential,
        "moment_story" => assessment.moment_story,
        "expression" => assessment.expression,
        "gesture" => assessment.gesture,
        "action" => assessment.action,
        "novelty" => assessment.novelty,
        "pacing" => assessment.pacing,
        "repetition_risk" => assessment.repetition_risk,
        _ => 0.5,
    }
}

/// Aesthetic-feature residual parsed from a profile's `feature_weights_json`; zeros when
/// absent or malformed, so a broken payload degrades to the CLIP-only residual gracefully.
fn aesthetic_weights(profile: &StyleProfile) -> [f32; AESTHETIC_FEATURES.len()] {
    let mut weights = [0.0; AESTHETIC_FEATURES.len()];
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&profile.feature_weights_json) else {
        return weights;
    };
    let Some(feature_weights) = value.get("feature_weights") else {
        return weights;
    };
    for (slot, name) in weights.iter_mut().zip(AESTHETIC_FEATURES) {
        *slot = feature_weights
            .get(name)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32;
    }
    weights
}

/// Active, held-out-gated personal profiles for one search request. The default-context
/// profile carries the personal affinity; a requested context adds the context-fit difference
/// on top. Profiles that failed the held-out evaluation gate are ignored here even if somehow
/// active (defense in depth: the gate is set at train time and re-checked at ranking time).
struct PersonalScorer {
    default_profile: Option<(StyleProfile, [f32; AESTHETIC_FEATURES.len()])>,
    context_profile: Option<(StyleProfile, [f32; AESTHETIC_FEATURES.len()])>,
    context_requested: bool,
}

impl PersonalScorer {
    fn load(store: &Store, owner_id: &str, context_key: Option<&str>) -> anyhow::Result<Self> {
        let default_profile = gated_profile(store.active_style_profile(owner_id)?).map(|profile| {
            let weights = aesthetic_weights(&profile);
            (profile, weights)
        });
        let context_profile = match context_key {
            Some(key) if key != style::trainer::DEFAULT_CONTEXT_KEY => gated_profile(
                store.active_style_profile_for_context(owner_id, key)?,
            )
            .map(|profile| {
                let weights = aesthetic_weights(&profile);
                (profile, weights)
            }),
            _ => None,
        };
        Ok(Self {
            default_profile,
            context_profile,
            context_requested: context_key.is_some(),
        })
    }

    /// Raw (unscaled) personal affinity: the CLIP residual plus the aesthetic-feature
    /// residual, matching the residual the trainer and the held-out evaluation measure.
    fn raw_affinity(
        profile: &(StyleProfile, [f32; AESTHETIC_FEATURES.len()]),
        vector: &[f32],
        aesthetic_features: &[f32; AESTHETIC_FEATURES.len()],
    ) -> Option<f32> {
        if profile.0.embedding_weights.len() != EMBEDDING_DIM || vector.len() != EMBEDDING_DIM {
            return None;
        }
        let clip = dot_512(vector, &profile.0.embedding_weights);
        let aesthetic_term = profile
            .1
            .iter()
            .zip(aesthetic_features)
            .map(|(weight, feature)| weight * feature)
            .sum::<f32>();
        Some(clip + aesthetic_term)
    }

    /// Scaled personal affinity (default-context profile) and context fit (the context
    /// profile's affinity beyond the default). Both are `0.0` when no gated profile applies —
    /// the score never invents certainty from an absent or unlearned profile.
    fn terms(
        &self,
        vector: Option<&Vec<f32>>,
        aesthetic_features: &[f32; AESTHETIC_FEATURES.len()],
    ) -> (f32, f32) {
        let Some(vector) = vector else {
            return (0.0, 0.0);
        };
        let default_raw = self
            .default_profile
            .as_ref()
            .and_then(|profile| Self::raw_affinity(profile, vector, aesthetic_features))
            .unwrap_or(0.0);
        let context_raw = if self.context_requested {
            self.context_profile
                .as_ref()
                .and_then(|profile| Self::raw_affinity(profile, vector, aesthetic_features))
        } else {
            None
        };
        let personal_affinity = default_raw * PERSONAL_WEIGHT;
        let context_fit = match context_raw {
            Some(context_raw) => (context_raw - default_raw) * PERSONAL_WEIGHT,
            None => 0.0,
        };
        (personal_affinity, context_fit)
    }

    /// The exported raw personal affinity: the effective affinity for this request
    /// (context profile when one was requested and applies, else the default profile).
    fn exported_affinity(
        &self,
        vector: Option<&Vec<f32>>,
        aesthetic_features: &[f32; AESTHETIC_FEATURES.len()],
    ) -> Option<f32> {
        let vector = vector?;
        if self.context_requested {
            if let Some(profile) = self.context_profile.as_ref() {
                return Self::raw_affinity(profile, vector, aesthetic_features);
            }
        }
        self.default_profile
            .as_ref()
            .and_then(|profile| Self::raw_affinity(profile, vector, aesthetic_features))
    }
}

/// Ranking-time re-check of the train-time gate: an unlearned profile never contributes.
fn gated_profile(profile: Option<StyleProfile>) -> Option<StyleProfile> {
    profile.filter(|profile| {
        profile.learned
            && profile
                .held_out_metric
                .is_some_and(|held_out| held_out > profile.baseline_metric.unwrap_or(0.0))
    })
}

/// Compose the score exactly as the general ranker did while exporting every term. With no
/// gated profile the personal and context-fit terms are `0.0` and the left-to-right addition
/// order matches the pre-breakdown composition, keeping the no-profile path bit-identical:
/// `hybrid index score + editorial + general aesthetic + penalties (+ personal + context)`.
fn compose_score(
    semantic: f32,
    cosine: f32,
    annotation: Option<&crush_store::EditorialAnnotation>,
    assessment: Option<&crush_store::AestheticAssessment>,
    vector: Option<&Vec<f32>>,
    aesthetic_features: &[f32; AESTHETIC_FEATURES.len()],
    personal: &PersonalScorer,
) -> (f32, ScoreBreakdown, Option<f32>) {
    let transcript_boost = semantic - cosine;
    let editorial = editorial_quality_adjustment(annotation);
    let penalties = editorial_penalty(annotation);
    let general_aesthetic = general_aesthetic_adjustment(assessment);
    let (personal_affinity, context_fit) = personal.terms(vector, aesthetic_features);
    let total =
        semantic + editorial + general_aesthetic + penalties + personal_affinity + context_fit;
    let breakdown = ScoreBreakdown {
        semantic,
        transcript_boost,
        editorial,
        general_aesthetic,
        penalties,
        personal_affinity,
        context_fit,
        total,
    };
    let personal_style_score = personal.exported_affinity(vector, aesthetic_features);
    (total, breakdown, personal_style_score)
}

/// Personal-style affinity for one asset under the owner's gated default-context profile.
/// `None` when the asset has no stored vector or no gated profile applies — detail views
/// show no personal score rather than inventing one.
pub fn personal_style_score(
    store: &Store,
    owner_id: &str,
    kind: MediaKind,
    id: &str,
) -> anyhow::Result<Option<f32>> {
    let personal = PersonalScorer::load(store, owner_id, None)?;
    let Some(vector) = media_vector(store, owner_id, kind, id)? else {
        return Ok(None);
    };
    let aesthetic = store.aesthetic_assessment(owner_id, kind, id)?;
    let aesthetic_features = aesthetic_feature_vector(aesthetic.as_ref());
    Ok(personal.exported_affinity(Some(&vector), &aesthetic_features))
}

/// The Task 020a selects surface: one response carrying BOTH orderings.
///
/// `general` is the cold-start strong-shot list straight from the
/// `aesthetic_assessments_strongest` index — no brief, no profile, privacy flags respected by
/// the store query. In that list `AssetSearchResult::score` is the general `overall` judgment
/// and `cosine` is `0.0` because no semantic query is involved. `personalized` is the
/// brief-driven ordering from [`SearchEngine::search_assets_in_context`], which composes the
/// general ranker with the gated personal adaptor and exports every term in its breakdown.
/// The general assessment is never hidden by the personalization: both lists are returned
/// together, per docs/dam-feedback-blueprint.md §5.
#[derive(Debug, Clone, Serialize)]
pub struct SelectsCandidates {
    /// The brief that drove the personalized ordering (empty when only the general list was
    /// requested).
    pub brief: String,
    /// Context key the personalized ranking was scoped to, when one was requested.
    pub context_key: Option<String>,
    /// Strong-shot ordering from the general model.
    pub general: Vec<AssetSearchResult>,
    /// Brief-driven personalized ordering; empty when no brief was supplied.
    pub personalized: Vec<AssetSearchResult>,
    /// Effective profile used for this request, not a later UI status lookup. Absent means
    /// the right-hand list is brief-driven general ranking, not personalized evidence.
    pub profile: Option<SelectsProfile>,
    /// The near-duplicate-per-source cap applied to the general list, echoed for visibility.
    /// `None` means no diversification was requested.
    pub duplicate_cap: Option<usize>,
    /// How many general-list candidates were skipped by the cap, for transparency.
    pub skipped_duplicates: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectsProfile {
    pub id: String,
    pub version: i64,
    pub context_key: String,
    pub algorithm_version: String,
}

/// Produce both selects orderings in one response. With a `brief`, the personalized list is
/// ranked through the same composed score as mixed-media search (so its breakdown is
/// directly explainable); without one, only the general strong-shot list is produced.
///
/// `duplicate_cap` optionally diversifies the general list: after ranking, at most that many
/// candidates per source may appear (shots count per source video; near-duplicate photos
/// cosine ≥ 0.95 count as one source). The cap is echoed on the response with the number of
/// skipped candidates so the UI can show it; the personalized list is never altered.
#[allow(clippy::too_many_arguments)] // the flat candidate-request shape is the public contract
pub fn selects_candidates<E: TextEmbedder>(
    store: &Store,
    owner_id: &str,
    engine: &SearchEngine,
    embedder: &mut E,
    brief: Option<&str>,
    top_k: usize,
    context_key: Option<&str>,
    duplicate_cap: Option<usize>,
) -> anyhow::Result<SelectsCandidates> {
    ensure!(top_k > 0, "top must be greater than zero");
    if let Some(cap) = duplicate_cap {
        ensure!(cap > 0, "duplicate cap must be greater than zero");
    }
    let scorer = PersonalScorer::load(store, owner_id, context_key)?;
    let profile = brief.and_then(|_| {
        scorer
            .context_profile
            .as_ref()
            .or(scorer.default_profile.as_ref())
            .map(|(profile, _)| SelectsProfile {
                id: profile.id.clone(),
                version: profile.version,
                context_key: profile.context_key.clone(),
                algorithm_version: profile.algorithm_version.clone(),
            })
    });
    let personalized = match brief {
        Some(brief) => {
            ensure!(!brief.trim().is_empty(), "brief must not be empty");
            engine.search_assets_in_context(store, embedder, brief, top_k, context_key)?
        }
        None => Vec::new(),
    };
    let mut ranked_general = Vec::new();
    for strong in store.strongest_assets(owner_id, top_k)? {
        ranked_general.push(hydrate_strong_asset(store, owner_id, &strong)?);
    }
    let (general, skipped_duplicates) = match duplicate_cap {
        None => (ranked_general, 0),
        Some(cap) => diversify_general(store, owner_id, ranked_general, cap)?,
    };
    Ok(SelectsCandidates {
        brief: brief.unwrap_or_default().to_owned(),
        context_key: context_key.map(str::to_owned),
        general,
        personalized,
        profile,
        duplicate_cap,
        skipped_duplicates,
    })
}

/// Enforce the near-duplicate-per-source cap on the ranked general list. Shots group by
/// their source video; near-duplicate photos (cosine ≥ 0.95) share the first kept photo's
/// source key, so a burst counts as one exhibit against the cap. Deterministic: the walk
/// follows the ranked order and keeps the first candidates per source.
fn diversify_general(
    store: &Store,
    owner_id: &str,
    ranked: Vec<AssetSearchResult>,
    cap: usize,
) -> anyhow::Result<(Vec<AssetSearchResult>, usize)> {
    let mut kept = Vec::with_capacity(ranked.len());
    let mut source_counts: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut kept_photo_vectors: Vec<(String, Vec<f32>)> = Vec::new();
    let mut skipped = 0usize;
    for result in ranked {
        let key = if result.asset_type == "video" {
            let video_id = store
                .search_shot_context(owner_id, &result.asset_id)
                .ok()
                .flatten()
                .map(|context| context.video_id)
                .unwrap_or_else(|| result.asset_id.clone());
            format!("video:{video_id}")
        } else {
            // Near-duplicate photos share the first kept photo's bucket, so a burst counts
            // as one exhibit against the cap.
            let mut anchor: Option<String> = None;
            if let Some(vector) = store.vector_for_photo(owner_id, &result.asset_id)? {
                for (kept_id, kept_vector) in &kept_photo_vectors {
                    if sequence::cosine(vector.as_slice(), kept_vector)
                        >= sequence::NEAR_DUPLICATE_COSINE
                    {
                        anchor = Some(kept_id.clone());
                        break;
                    }
                }
                if anchor.is_none() {
                    kept_photo_vectors.push((result.asset_id.clone(), vector));
                }
            }
            format!(
                "photo:{}",
                anchor.unwrap_or_else(|| result.asset_id.clone())
            )
        };
        let count = source_counts.entry(key).or_insert(0);
        if *count >= cap {
            skipped += 1;
            continue;
        }
        *count += 1;
        kept.push(result);
    }
    Ok((kept, skipped))
}

/// Hydrate one general strong-shot row into the same result shape search returns, so the UI
/// renders both lists with one component. The score is the general aesthetic `overall`; the
/// personal affinity (when a gated profile exists) is exported separately, never mixed in.
fn hydrate_strong_asset(
    store: &Store,
    owner_id: &str,
    strong: &StrongAsset,
) -> anyhow::Result<AssetSearchResult> {
    let editorial_quality = store
        .editorial_annotation(owner_id, strong.media_kind, &strong.media_id)?
        .and_then(|annotation| annotation.quality);
    let style = personal_style_score(store, owner_id, strong.media_kind, &strong.media_id)?;
    match strong.media_kind {
        MediaKind::Span => anyhow::bail!(
            "imported span {} is catalogue evidence and is not aesthetically assessed",
            strong.media_id
        ),
        MediaKind::Shot => {
            let context = store
                .search_shot_context(owner_id, &strong.media_id)?
                .with_context(|| format!("assessed shot {} no longer exists", strong.media_id))?;
            let transcript_snippet = store
                .segments_overlapping(owner_id, &context.video_id, context.start_s, context.end_s)?
                .first()
                .map(|segment| snippet(&segment.text));
            Ok(AssetSearchResult {
                asset_type: "video".to_owned(),
                asset_id: strong.media_id.clone(),
                path: context.video_path,
                start_s: Some(context.start_s),
                end_s: Some(context.end_s),
                thumb_path: context
                    .thumb_rel
                    .as_deref()
                    .map(|relative| store.thumbnail_path(relative))
                    .transpose()?
                    .map(|path| path.display().to_string()),
                score: strong.overall as f32,
                cosine: 0.0,
                transcript_snippet,
                editorial_quality,
                aesthetic_score: Some(strong.overall),
                personal_style_score: style,
                score_breakdown: None,
            })
        }
        MediaKind::Photo => {
            let photo = store
                .photo_by_id(owner_id, &strong.media_id)?
                .with_context(|| format!("assessed photo {} no longer exists", strong.media_id))?;
            Ok(AssetSearchResult {
                asset_type: "photo".to_owned(),
                asset_id: strong.media_id.clone(),
                path: photo.path,
                start_s: None,
                end_s: None,
                thumb_path: photo
                    .thumb_rel
                    .as_deref()
                    .map(|relative| store.thumbnail_path(relative))
                    .transpose()?
                    .map(|path| path.display().to_string()),
                score: strong.overall as f32,
                cosine: 0.0,
                transcript_snippet: None,
                editorial_quality,
                aesthetic_score: Some(strong.overall),
                personal_style_score: style,
                score_breakdown: None,
            })
        }
    }
}

fn validate_embedding_metadata(store: &Store, owner_id: &str) -> anyhow::Result<()> {
    let manifest = crush_core::models::bundled_manifest()?;
    let metadata = store.embedding_meta_get(owner_id)?.context(
        "embedding metadata is missing; run `crushctl models ensure` and re-embed the library",
    )?;
    ensure!(
        metadata.model_sha256 == manifest.embedding_sha256
            && metadata.dim == manifest.dim
            && metadata.preprocess_version == manifest.preprocess_version,
        "models changed, run `crushctl reembed --all`"
    );
    Ok(())
}

fn transcript_fts_query(query: &str) -> Option<String> {
    let mut words = query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.chars().count() >= 3)
        .filter(|word| STOPWORDS.binary_search(&word.as_str()).is_err())
        .collect::<Vec<_>>();
    words.sort_unstable();
    words.dedup();
    (!words.is_empty()).then(|| {
        words
            .into_iter()
            .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

fn snippet(text: &str) -> String {
    text.chars().take(200).collect()
}

#[inline]
fn dot_512(left: &[f32], right: &[f32]) -> f32 {
    debug_assert_eq!(left.len(), EMBEDDING_DIM);
    debug_assert_eq!(right.len(), EMBEDDING_DIM);
    let mut sum = 0.0_f32;
    let mut index = 0;
    while index < EMBEDDING_DIM {
        sum += left[index] * right[index]
            + left[index + 1] * right[index + 1]
            + left[index + 2] * right[index + 2]
            + left[index + 3] * right[index + 3]
            + left[index + 4] * right[index + 4]
            + left[index + 5] * right[index + 5]
            + left[index + 6] * right[index + 6]
            + left[index + 7] * right[index + 7];
        index += 8;
    }
    sum
}

#[derive(Debug, Clone, Copy)]
struct RankedRow {
    row: usize,
    score: f32,
    cosine: f32,
}

impl PartialEq for RankedRow {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RankedRow {}

impl PartialOrd for RankedRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedRow {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.cosine.total_cmp(&other.cosine))
            // Rows are loaded in shot-id order, so the lower row wins a complete score tie.
            .then_with(|| other.row.cmp(&self.row))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crush_core::DEFAULT_OWNER_ID;
    use crush_store::{
        AestheticAssessment, EditorialAnnotation, EmbeddingMeta, FeedbackEvent, FeedbackSignal,
        MediaKind, Photo, PhotoStatus, Shot, TranscriptSegment, Video, VideoStatus,
    };
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn ten_thousand_vectors_match_brute_force_under_budget() {
        const ROWS: usize = 10_000;
        let mut state = 0x9E37_79B9_u32;
        let mut matrix = Vec::with_capacity(ROWS * EMBEDDING_DIM);
        for _ in 0..ROWS {
            let start = matrix.len();
            for _ in 0..EMBEDDING_DIM {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                matrix.push((state as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            normalize(&mut matrix[start..]);
        }
        let query = matrix[4_321 * EMBEDDING_DIM..4_322 * EMBEDDING_DIM].to_vec();
        let ids = (0..ROWS)
            .map(|row| format!("shot-{row:05}"))
            .collect::<Vec<_>>();
        let index = VectorIndex {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            shot_ids: ids,
            matrix,
        };
        let expected = index
            .matrix
            .as_chunks::<EMBEDDING_DIM>()
            .0
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                dot_512(*left, &query).total_cmp(&dot_512(*right, &query))
            })
            .unwrap()
            .0;

        let started = Instant::now();
        let found = index.search(&query, 1, DEFAULT_OWNER_ID).unwrap();
        let elapsed = started.elapsed();
        eprintln!("searched {ROWS}x{EMBEDDING_DIM} vectors in {elapsed:?}");
        assert_eq!(found[0].shot_id, format!("shot-{expected:05}"));
        assert!(
            elapsed < Duration::from_millis(30),
            "10k vector search took {elapsed:?}"
        );
    }

    #[test]
    fn query_words_are_filtered_and_escaped_for_fts() {
        assert_eq!(
            transcript_fts_query("The RED, red boat and lighthouse!"),
            Some("\"boat\" OR \"lighthouse\" OR \"red\"".to_owned())
        );
        assert_eq!(transcript_fts_query("a to the"), None);
    }

    #[test]
    fn transcript_only_word_lifts_an_otherwise_equal_shot() {
        let (_directory, mut store) = populated_store();
        store
            .insert_transcript_segments(
                DEFAULT_OWNER_ID,
                &[TranscriptSegment {
                    id: "segment-b".to_owned(),
                    video_id: "video-1".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    start_s: 1.0,
                    end_s: 2.0,
                    text: "A zeppelin crosses the horizon".to_owned(),
                    confidence: Some(0.9),
                }],
            )
            .unwrap();
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.15).unwrap();
        let mut embedder = |_text: &str| {
            let mut vector = [0.0_f32; EMBEDDING_DIM];
            vector[0] = 1.0;
            Ok(vector)
        };
        let results = engine.search(&store, &mut embedder, "zeppelin", 2).unwrap();

        assert_eq!(results[0].shot_id, "shot-b");
        assert!((results[0].score - 1.15).abs() < 1e-6);
        assert_eq!(results[0].cosine, 1.0);
        assert_eq!(
            results[0].transcript_snippet.as_deref(),
            Some("A zeppelin crosses the horizon")
        );
        assert_eq!(results[1].shot_id, "shot-a");
        assert_eq!(results[1].score, 1.0);
    }

    #[test]
    fn changed_model_metadata_refuses_to_load_the_index() {
        let (_directory, store) = populated_store();
        let mut metadata = store.embedding_meta_get(DEFAULT_OWNER_ID).unwrap().unwrap();
        metadata.model_sha256 = "changed".to_owned();
        store
            .embedding_meta_set(DEFAULT_OWNER_ID, &metadata)
            .unwrap();
        let error = VectorIndex::load(&store, DEFAULT_OWNER_ID).unwrap_err();
        assert!(error
            .to_string()
            .contains("models changed, run `crushctl reembed --all`"));
    }

    #[test]
    fn explicit_feedback_trains_a_personal_ranker_that_changes_order() {
        let (_directory, mut store) = style_store();
        append_default_picks_and_rejects(&mut store);
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert_eq!(profile.algorithm_version, "personal-residual-v1");
        assert_eq!(profile.context_key, "default");
        assert_eq!(profile.sample_count, 12);

        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0_f32; EMBEDDING_DIM]);
        let results = engine
            .search_assets(&store, &mut embedder, "same semantics", 12)
            .unwrap();
        assert!(results[0].asset_id.starts_with("shot-good"));
        assert!(results[11].asset_id.starts_with("shot-bad"));
        for result in &results {
            let breakdown = result.score_breakdown.expect("breakdown exported");
            let sum = breakdown.semantic
                + breakdown.transcript_boost
                + breakdown.editorial
                + breakdown.general_aesthetic
                + breakdown.penalties
                + breakdown.personal_affinity
                + breakdown.context_fit;
            assert!((sum - breakdown.total).abs() < 1e-5);
            assert_eq!(breakdown.total, result.score);
            assert_eq!(breakdown.penalties, 0.0);
        }
        assert!(results[0].personal_style_score.unwrap() > 0.0);
        assert!(results[11].personal_style_score.unwrap() < 0.0);
        assert!(results[0].score_breakdown.unwrap().personal_affinity > 0.0);
        assert!(results[11].score_breakdown.unwrap().personal_affinity < 0.0);
    }

    #[test]
    fn planted_style_marks_learned_and_beats_the_baseline() {
        let (_directory, mut store) = style_store();
        append_default_picks_and_rejects(&mut store);
        // Differing general assessments so the composed margin is genuinely exercised: the
        // owner picks shots the general model scores LOW (overall 0.3) over shots it scores
        // HIGH (overall 0.7). Every pair then carries a nonzero production-scale general
        // margin that OPPOSES the personal residual — the composed ranker must override the
        // general term, while the baseline (general term alone) gets every held-out pair
        // wrong.
        for index in 0..6 {
            store
                .upsert_aesthetic_assessment(
                    DEFAULT_OWNER_ID,
                    &aesthetic_assessment(&format!("shot-good-{index}"), 0.3),
                )
                .unwrap();
            store
                .upsert_aesthetic_assessment(
                    DEFAULT_OWNER_ID,
                    &aesthetic_assessment(&format!("shot-bad-{index}"), 0.7),
                )
                .unwrap();
        }
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert!(profile.learned, "planted style must pass the eval gate");
        eprintln!(
            "synthetic planted-style probe (not human approval): {}",
            profile.metrics_json
        );
        let held_out = profile.held_out_metric.expect("held-out metric recorded");
        let baseline = profile.baseline_metric.expect("baseline recorded");
        assert!(
            held_out > baseline,
            "held-out {held_out} vs baseline {baseline}"
        );
        assert!(held_out >= 0.6);
        let metrics: serde_json::Value = serde_json::from_str(&profile.metrics_json).unwrap();
        assert_eq!(metrics["learned"], true);
        assert_eq!(metrics["split"], "media-disjoint-every-3rd");
        assert_eq!(metrics["trainer"], "personal-residual-v1");
        assert!(metrics["held_out_pairs"].as_u64().unwrap() >= 4);
        // The general term is nonzero on every pair (it opposes the picks), so the baseline
        // must fail every held-out pair while the composed ranker recovers them.
        assert_eq!(metrics["baseline_accuracy"], 0.0);
    }

    #[test]
    fn style_profile_training_is_deterministic() {
        let (_directory, mut store) = style_store();
        append_default_picks_and_rejects(&mut store);
        let first = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        let second = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert_eq!(second.version, first.version + 1);
        assert_eq!(first.embedding_weights, second.embedding_weights);
        assert_eq!(first.feature_weights_json, second.feature_weights_json);
        assert_eq!(first.metrics_json, second.metrics_json);
        assert_eq!(first.held_out_metric, second.held_out_metric);
        assert_eq!(first.baseline_metric, second.baseline_metric);
    }

    #[test]
    fn sparse_feedback_returns_none_and_keeps_the_previous_profile() {
        let (_directory, mut store) = style_store();
        let retained = StyleProfile {
            id: "style-keep".to_owned(),
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            name: "default".to_owned(),
            version: 1,
            algorithm_version: "personal-residual-v1".to_owned(),
            embedding_weights: vec![0.0; EMBEDDING_DIM],
            feature_weights_json: "{}".to_owned(),
            sample_count: 12,
            held_out_metric: Some(0.75),
            baseline_metric: Some(0.5),
            context_key: "default".to_owned(),
            metrics_json: "{}".to_owned(),
            learned: true,
            active: true,
            trained_at: chrono::Utc::now(),
        };
        store
            .put_style_profile(DEFAULT_OWNER_ID, &retained)
            .unwrap();
        append_event(
            &mut store,
            "sparse-pick",
            "shot-good-0",
            FeedbackSignal::Pick,
        );
        append_event(
            &mut store,
            "sparse-reject",
            "shot-bad-0",
            FeedbackSignal::Reject,
        );
        assert!(retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        let active = store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert_eq!(active.id, "style-keep");
        assert!(active.active);
    }

    #[test]
    fn context_partitioning_keeps_named_context_out_of_default() {
        let (_directory, mut store) = style_store();
        for index in 0..6 {
            append_context_event(
                &mut store,
                &format!("context-pick-{index}"),
                &format!("shot-good-{index}"),
                FeedbackSignal::Pick,
                "homepage-hero",
            );
            append_context_event(
                &mut store,
                &format!("context-reject-{index}"),
                &format!("shot-bad-{index}"),
                FeedbackSignal::Reject,
                "homepage-hero",
            );
        }
        // A preference in one context must never become a universal rule: the default context
        // sees no evidence at all and the previous (absent) profile is left untouched.
        assert!(retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        let profile =
            retrain_style_profile_for_context(&mut store, DEFAULT_OWNER_ID, "homepage-hero")
                .unwrap()
                .unwrap();
        assert_eq!(profile.name, "homepage-hero");
        assert_eq!(profile.context_key, "homepage-hero");
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0; EMBEDDING_DIM]);
        let named = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            Some("portraits"),
            12,
            Some("homepage-hero"),
            None,
        )
        .unwrap();
        let provenance = named.profile.expect("effective named profile exported");
        assert_eq!(provenance.id, profile.id);
        assert_eq!(provenance.version, profile.version);
        assert_eq!(provenance.context_key, "homepage-hero");
        let unrelated = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            Some("portraits"),
            12,
            Some("another-project"),
            None,
        )
        .unwrap();
        assert!(
            unrelated.profile.is_none(),
            "another context must not borrow this profile"
        );
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        assert!(store
            .active_style_profile_for_context(DEFAULT_OWNER_ID, "homepage-hero")
            .unwrap()
            .is_some());
    }

    #[test]
    fn noise_feedback_refuses_to_mark_learned_and_the_gate_ignores_it() {
        let (_directory, mut store) = style_store();
        // Identical vectors for every asset: feedback carries no learnable direction, so the
        // held-out pairs are all ties and the gate must refuse.
        let shared = [0.7_f32, 0.7];
        let mut vector = [0.0_f32; EMBEDDING_DIM];
        vector[0] = shared[0];
        vector[1] = shared[1];
        for index in 0..6 {
            store
                .put_vector(DEFAULT_OWNER_ID, &format!("shot-good-{index}"), &vector)
                .unwrap();
            store
                .put_vector(DEFAULT_OWNER_ID, &format!("shot-bad-{index}"), &vector)
                .unwrap();
        }
        append_default_picks_and_rejects(&mut store);
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert!(!profile.learned, "noise must never be marked learned");
        eprintln!(
            "identical-vector noise probe (not human approval): {}",
            profile.metrics_json
        );
        assert_eq!(profile.held_out_metric, Some(0.0));
        // The media-disjoint composed evaluation: identical vectors carry no learnable
        // direction, and general-margin ties give the baseline no credit either — the probe
        // ends at 0.0 vs 0.0, far from any learned claim.
        assert_eq!(profile.baseline_metric, Some(0.0));

        // The profile is active but unlearned: ranking must ignore it entirely.
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0_f32; EMBEDDING_DIM]);
        let results = engine
            .search_assets(&store, &mut embedder, "same semantics", 12)
            .unwrap();
        for result in &results {
            let breakdown = result.score_breakdown.expect("breakdown exported");
            assert_eq!(breakdown.personal_affinity, 0.0);
            assert_eq!(breakdown.context_fit, 0.0);
            assert!(result.personal_style_score.is_none());
        }
    }

    #[test]
    fn confirmed_reference_sets_train_but_unconfirmed_sets_stay_inert() {
        let (_directory, mut store) = style_store();
        seed_confirmed_previous_work_set(&mut store);
        // Before confirmation nothing trains: the set is inert until the explicit confirm.
        // (The helper confirms at the end; unconfirmed-only behavior is covered below by
        // re-creating an inert set.)
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert!(profile.learned);
        assert_eq!(profile.sample_count, 12);
        // Disabling mutes the evidence without deleting it AND invalidates the profile it
        // trained: the active profile falls back to the general model immediately, and the
        // next retrain has no positives to restore learning from.
        store
            .reference_set_disable(DEFAULT_OWNER_ID, "set-previous-work")
            .unwrap();
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        assert!(retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
    }

    fn seed_confirmed_previous_work_set(store: &mut Store) {
        let now = chrono::Utc::now();
        store
            .reference_set_create(
                DEFAULT_OWNER_ID,
                &crush_store::ReferenceSet {
                    id: "set-previous-work".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    name: "previous work".to_owned(),
                    context_key: "default".to_owned(),
                    description: "finished selects".to_owned(),
                    scope: crush_store::ReferenceSetScope::WholeSet,
                    status: crush_store::ReferenceSetStatus::Unconfirmed,
                    source_collection_id: None,
                    created_at: now,
                    confirmed_at: None,
                },
            )
            .unwrap();
        for index in 0..6 {
            store
                .reference_set_add_item(
                    DEFAULT_OWNER_ID,
                    &crush_store::ReferenceSetItem {
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        set_id: "set-previous-work".to_owned(),
                        media_kind: MediaKind::Shot,
                        media_id: format!("shot-good-{index}"),
                        role: crush_store::ReferenceItemRole::Positive,
                        added_at: now,
                    },
                )
                .unwrap();
        }
        // Negatives exist, but an unconfirmed set is inert: no curated evidence, no training.
        for index in 0..6 {
            append_event(
                store,
                &format!("reject-{index}"),
                &format!("shot-bad-{index}"),
                FeedbackSignal::Reject,
            );
        }
        assert!(retrain_style_profile(store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        assert!(
            store
                .reference_set_confirm(DEFAULT_OWNER_ID, "set-previous-work")
                .unwrap(),
            "confirming an existing set must succeed"
        );
    }

    #[test]
    fn deleting_a_confirmed_set_also_invalidates_its_profile() {
        let (_directory, mut store) = style_store();
        seed_confirmed_previous_work_set(&mut store);
        assert!(
            retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
                .unwrap()
                .unwrap()
                .learned
        );
        store
            .reference_set_delete(DEFAULT_OWNER_ID, "set-previous-work")
            .unwrap();
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
        assert!(retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
    }

    #[test]
    fn disabling_an_unconfirmed_set_leaves_profiles_alone() {
        let (_directory, mut store) = style_store();
        seed_confirmed_previous_work_set(&mut store);
        assert!(
            retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
                .unwrap()
                .unwrap()
                .learned
        );
        // A second, never-confirmed set has contributed no evidence; disabling it must not
        // invalidate a profile that was never trained on it.
        store
            .reference_set_create(
                DEFAULT_OWNER_ID,
                &crush_store::ReferenceSet {
                    id: "set-other".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    name: "Other work".to_owned(),
                    context_key: "default".to_owned(),
                    description: String::new(),
                    scope: crush_store::ReferenceSetScope::WholeSet,
                    status: crush_store::ReferenceSetStatus::Unconfirmed,
                    source_collection_id: None,
                    created_at: chrono::Utc::now(),
                    confirmed_at: None,
                },
            )
            .unwrap();
        store
            .reference_set_disable(DEFAULT_OWNER_ID, "set-other")
            .unwrap();
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_some());
    }

    #[test]
    fn reset_falls_back_to_the_identical_no_profile_ranking() {
        let (_directory, mut store) = style_store();
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0_f32; EMBEDDING_DIM]);
        let general = engine
            .search_assets(&store, &mut embedder, "same semantics", 12)
            .unwrap();

        append_default_picks_and_rejects(&mut store);
        assert!(retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .is_some());
        let personalized = engine
            .search_assets(&store, &mut embedder, "same semantics", 12)
            .unwrap();
        assert_ne!(general, personalized, "a learned profile must move ranking");

        assert_eq!(store.reset_style_profiles(DEFAULT_OWNER_ID).unwrap(), 1);
        let after_reset = engine
            .search_assets(&store, &mut embedder, "same semantics", 12)
            .unwrap();
        assert_eq!(
            general, after_reset,
            "reset must fall back to the general ranking bit-for-bit"
        );
        assert!(store
            .active_style_profile(DEFAULT_OWNER_ID)
            .unwrap()
            .is_none());
    }

    #[test]
    fn eval_gate_counts_ties_as_failures_and_needs_enough_held_out_pairs() {
        let pair = style::eval::RankedPair {
            margin_features: vec![1.0],
            weight: 1.0,
            plus_media: "shot:a".to_owned(),
            minus_media: "shot:b".to_owned(),
            general_margin: 0.0,
        };
        let four = vec![&pair, &pair, &pair, &pair];
        let tied = style::eval::evaluate(&four, &[0.0]);
        assert_eq!(tied.personal_accuracy, 0.0);
        assert_eq!(tied.residual_only_accuracy, 0.0);
        assert_eq!(tied.baseline_accuracy, 0.0);
        assert!(!tied.learned, "ties must count as failures");
        let winning = style::eval::evaluate(&four, &[1.0]);
        assert!(winning.learned);
        let too_few = vec![&pair, &pair, &pair];
        assert!(!style::eval::evaluate(&too_few, &[1.0]).learned);
    }

    #[test]
    fn composed_scoring_uses_the_production_general_margin_scale() {
        // The composed margin must weight the general term exactly as production does —
        // GENERAL_AESTHETIC_WEIGHT * (overall - 0.5) per side, so a pair margin is at most
        // ±2 * 0.08 — never the raw ±1.0 `overall` difference (which would over-credit the
        // general term ~6.25x and could certify a profile production makes worse).
        //
        // Pair 1: the production-maximum general margin (overall 1.0 vs 0.0 → 0.16) beside a
        // -1.0 residual: the composed vote stays positive (0.16 - 0.15 > 0) while the
        // residual alone is negative, so the composed accuracy must diverge from the
        // residual-only accuracy.
        let production_max = style::eval::RankedPair {
            margin_features: vec![-1.0],
            weight: 1.0,
            plus_media: "shot:a".to_owned(),
            minus_media: "shot:b".to_owned(),
            general_margin: GENERAL_AESTHETIC_WEIGHT,
        };
        let four = vec![
            &production_max,
            &production_max,
            &production_max,
            &production_max,
        ];
        let outcome = style::eval::evaluate(&four, &[1.0]);
        assert_eq!(outcome.personal_accuracy, 1.0);
        assert_eq!(outcome.residual_only_accuracy, 0.0);
        assert_ne!(
            outcome.personal_accuracy, outcome.residual_only_accuracy,
            "the composed vote must diverge from the residual-only vote"
        );
        // Pair 2: a mild general margin (overall 0.6 vs 0.4 → production margin 0.032)
        // cannot outweigh the scaled residual (-0.15). Under the old raw-scale margin
        // (0.2) this pair wrongly earned composed credit; at production scale it must not.
        let mild = style::eval::RankedPair {
            margin_features: vec![-1.0],
            weight: 1.0,
            plus_media: "shot:c".to_owned(),
            minus_media: "shot:d".to_owned(),
            general_margin: GENERAL_AESTHETIC_WEIGHT * 0.2,
        };
        let four = vec![&mild, &mild, &mild, &mild];
        let outcome = style::eval::evaluate(&four, &[1.0]);
        assert_eq!(outcome.personal_accuracy, 0.0);
        assert_eq!(outcome.residual_only_accuracy, 0.0);
    }

    #[test]
    fn media_disjoint_split_never_shares_media_between_train_and_eval() {
        // Six media, held-out set {c, f} (every third sorted asset): the pair between the two
        // held-out media is the only held-out pair, the pair between two trained media is the
        // only train pair, and the straddling pairs are dropped from both sides and counted.
        let pair = |plus: &str, minus: &str| style::eval::RankedPair {
            margin_features: vec![1.0],
            weight: 1.0,
            plus_media: plus.to_owned(),
            minus_media: minus.to_owned(),
            general_margin: 0.0,
        };
        let pairs = vec![
            pair("shot:a", "shot:b"),
            pair("shot:b", "shot:e"),
            pair("shot:c", "shot:f"),
            pair("shot:a", "shot:c"),
            pair("shot:d", "shot:f"),
        ];
        let split = style::eval::split_pairs(&pairs);
        let train_media: std::collections::BTreeSet<&str> = split
            .train
            .iter()
            .flat_map(|pair| [pair.plus_media.as_str(), pair.minus_media.as_str()])
            .collect();
        let held_media: std::collections::BTreeSet<&str> = split
            .held_out
            .iter()
            .flat_map(|pair| [pair.plus_media.as_str(), pair.minus_media.as_str()])
            .collect();
        assert!(
            train_media.is_disjoint(&held_media),
            "no media may appear on both sides of the split"
        );
        assert_eq!(
            train_media,
            ["shot:a", "shot:b", "shot:e"].into_iter().collect()
        );
        assert_eq!(held_media, ["shot:c", "shot:f"].into_iter().collect());
        assert_eq!(split.train.len(), 2);
        assert_eq!(split.held_out.len(), 1);
        assert_eq!(split.straddling_pairs, 2);
    }

    #[test]
    fn repeated_and_conflicting_prefer_evidence_is_netted_not_duplicated() {
        // Store-level netting probe (documented beside the planted/noise probes for John's
        // eval review): the planted 12-media setup plus two prefer votes for good-0 over
        // bad-0 and one reversed prefer vote. The repeats must accumulate (+1 each) and the
        // reversal subtract (-1), netting to exactly one extra unit of pair weight — never
        // duplicated rows, never invented certainty from the conflict.
        let (_directory, mut store) = style_store();
        append_default_picks_and_rejects(&mut store);
        append_prefer_event(&mut store, "prefer-good-0-a", "shot-good-0", "shot-bad-0");
        append_prefer_event(&mut store, "prefer-good-0-b", "shot-good-0", "shot-bad-0");
        append_prefer_event(&mut store, "prefer-bad-0", "shot-bad-0", "shot-good-0");
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .expect("12 media clear the sample floor; the netting probe must actually train");
        eprintln!(
            "repeated+conflicting prefer netting probe (not human approval): {}",
            profile.metrics_json
        );
        assert_eq!(profile.sample_count, 12);
        assert!(profile.learned, "netted evidence must not break the gate");

        // Two repeats minus one reversal net to a single prefer vote's worth of evidence:
        // the profile trained on one lone prefer pair is identical, proving the conflicting
        // rows were netted inside pair building instead of duplicating training rows.
        let (_directory, mut single) = style_store();
        append_default_picks_and_rejects(&mut single);
        append_prefer_event(&mut single, "prefer-good-0", "shot-good-0", "shot-bad-0");
        let single = retrain_style_profile(&mut single, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert_eq!(profile.embedding_weights, single.embedding_weights);
        assert_eq!(profile.feature_weights_json, single.feature_weights_json);
        assert_eq!(profile.metrics_json, single.metrics_json);
    }

    fn append_default_picks_and_rejects(store: &mut Store) {
        for index in 0..6 {
            append_event(
                store,
                &format!("pick-{index}"),
                &format!("shot-good-{index}"),
                FeedbackSignal::Pick,
            );
            append_event(
                store,
                &format!("reject-{index}"),
                &format!("shot-bad-{index}"),
                FeedbackSignal::Reject,
            );
        }
    }

    fn append_event(store: &mut Store, id: &str, media_id: &str, signal: FeedbackSignal) {
        append_context_event(store, id, media_id, signal, "default");
    }

    /// A pairwise `prefer` event: `media_id` preferred over `compared_media_id`. The plain
    /// `append_event` helper leaves `compared_media_*` unset, which the trainer reads as a
    /// non-pair event.
    fn append_prefer_event(store: &mut Store, id: &str, media_id: &str, compared_media_id: &str) {
        store
            .append_feedback(
                DEFAULT_OWNER_ID,
                &FeedbackEvent {
                    id: id.to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    media_kind: MediaKind::Shot,
                    media_id: media_id.to_owned(),
                    signal: FeedbackSignal::Prefer,
                    value: None,
                    compared_media_kind: Some(MediaKind::Shot),
                    compared_media_id: Some(compared_media_id.to_owned()),
                    context_json: "{}".to_owned(),
                    created_at: chrono::Utc::now(),
                },
            )
            .unwrap();
    }

    fn append_context_event(
        store: &mut Store,
        id: &str,
        media_id: &str,
        signal: FeedbackSignal,
        context_key: &str,
    ) {
        let context_json = if context_key == "default" {
            "{}".to_owned()
        } else {
            format!(r#"{{"context":"{context_key}"}}"#)
        };
        store
            .append_feedback(
                DEFAULT_OWNER_ID,
                &FeedbackEvent {
                    id: id.to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    media_kind: MediaKind::Shot,
                    media_id: media_id.to_owned(),
                    signal,
                    value: None,
                    compared_media_kind: None,
                    compared_media_id: None,
                    context_json,
                    created_at: chrono::Utc::now(),
                },
            )
            .unwrap();
    }

    fn style_store() -> (TempDir, Store) {
        let directory = TempDir::new().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store
            .upsert_video(
                DEFAULT_OWNER_ID,
                &Video {
                    id: "video-1".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: "/footage/video.mov".to_owned(),
                    sha256: "video-sha".to_owned(),
                    duration_s: Some(2.0),
                    fps: Some(24.0),
                    width: Some(1920),
                    height: Some(1080),
                    has_audio: true,
                    status: VideoStatus::Embedded,
                    indexed_at: None,
                },
            )
            .unwrap();
        let mut shots = Vec::new();
        for index in 0..6 {
            shots.push(Shot {
                id: format!("shot-good-{index}"),
                video_id: "video-1".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: index,
                start_s: index as f64,
                end_s: index as f64 + 1.0,
                rep_frame_s: index as f64 + 0.4,
                thumb_rel: None,
                scene_score: None,
            });
            shots.push(Shot {
                id: format!("shot-bad-{index}"),
                video_id: "video-1".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: index + 6,
                start_s: index as f64 + 6.0,
                end_s: index as f64 + 7.0,
                rep_frame_s: index as f64 + 6.4,
                thumb_rel: None,
                scene_score: None,
            });
        }
        store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
        let mut good = [0.0_f32; EMBEDDING_DIM];
        good[0] = 1.0;
        let mut bad = [0.0_f32; EMBEDDING_DIM];
        bad[1] = 1.0;
        for shot in &shots {
            let vector = if shot.id.starts_with("shot-good") {
                good
            } else {
                bad
            };
            store
                .put_vector(DEFAULT_OWNER_ID, &shot.id, &vector)
                .unwrap();
        }
        let manifest = crush_core::models::bundled_manifest().unwrap();
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
        (directory, store)
    }

    #[test]
    fn general_aesthetic_adjustment_is_bounded_and_centered() {
        let assessment_for = |overall: f64| aesthetic_assessment("shot-x", overall);
        assert_eq!(
            general_aesthetic_adjustment(Some(&assessment_for(1.0))),
            0.08
        );
        assert_eq!(
            general_aesthetic_adjustment(Some(&assessment_for(0.0))),
            -0.08
        );
        assert_eq!(
            general_aesthetic_adjustment(Some(&assessment_for(0.5))),
            0.0
        );
        assert_eq!(general_aesthetic_adjustment(None), 0.0);
        for overall in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let adjustment = general_aesthetic_adjustment(Some(&assessment_for(overall)));
            assert!(
                adjustment.abs() <= 0.08,
                "overall {overall} produced adjustment {adjustment}"
            );
        }
    }

    #[test]
    fn equal_cosine_assets_rank_by_general_aesthetic() {
        let (_directory, store) = populated_store();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-a", 1.0))
            .unwrap();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-b", 0.0))
            .unwrap();
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| {
            let mut vector = [0.0_f32; EMBEDDING_DIM];
            vector[0] = 1.0;
            Ok(vector)
        };

        let results = engine
            .search_assets(&store, &mut embedder, "neutral query", 2)
            .unwrap();
        assert_eq!(results[0].asset_id, "shot-a");
        assert_eq!(results[1].asset_id, "shot-b");
        assert_eq!(results[0].cosine, results[1].cosine);
        let breakdown_a = results[0].score_breakdown.expect("breakdown exported");
        let breakdown_b = results[1].score_breakdown.expect("breakdown exported");
        assert!((breakdown_a.general_aesthetic - 0.08).abs() < 1e-6);
        assert!((breakdown_b.general_aesthetic + 0.08).abs() < 1e-6);
        for result in &results {
            let breakdown = result.score_breakdown.expect("breakdown exported");
            let sum = breakdown.semantic
                + breakdown.transcript_boost
                + breakdown.editorial
                + breakdown.general_aesthetic
                + breakdown.penalties
                + breakdown.personal_affinity
                + breakdown.context_fit;
            assert!(
                (sum - breakdown.total).abs() < 1e-4,
                "breakdown {breakdown:?} does not sum to its total"
            );
            assert!(
                (result.score - breakdown.total).abs() < 1e-4,
                "breakdown total {} does not match score {}",
                breakdown.total,
                result.score
            );
        }

        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-a", 0.0))
            .unwrap();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-b", 1.0))
            .unwrap();
        let results = engine
            .search_assets(&store, &mut embedder, "neutral query", 2)
            .unwrap();
        assert_eq!(results[0].asset_id, "shot-b");
        assert_eq!(results[1].asset_id, "shot-a");
    }

    #[test]
    fn selects_candidates_return_both_orderings_in_one_response() {
        let (_directory, store) = populated_store();
        store
            .upsert_photo(
                DEFAULT_OWNER_ID,
                &Photo {
                    id: "photo-a".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: "/photos/photo-a.jpg".to_owned(),
                    sha256: "photo-a-sha".to_owned(),
                    width: 6000,
                    height: 4000,
                    format: "jpeg".to_owned(),
                    orientation: None,
                    captured_at: None,
                    camera_make: None,
                    camera_model: None,
                    lens: None,
                    thumb_rel: None,
                    status: PhotoStatus::Done,
                    indexed_at: None,
                },
            )
            .unwrap();
        let mut vector = [0.0_f32; EMBEDDING_DIM];
        vector[0] = 1.0;
        store
            .put_photo_vector(DEFAULT_OWNER_ID, "photo-a", &vector)
            .unwrap();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-a", 0.9))
            .unwrap();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-b", 0.7))
            .unwrap();
        store
            .upsert_aesthetic_assessment(
                DEFAULT_OWNER_ID,
                &AestheticAssessment {
                    media_kind: MediaKind::Photo,
                    media_id: "photo-a".to_owned(),
                    ..aesthetic_assessment("photo-a", 0.8)
                },
            )
            .unwrap();
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| {
            let mut query = [0.0_f32; EMBEDDING_DIM];
            query[0] = 1.0;
            Ok(query)
        };

        let selection = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            Some("a quiet travel film"),
            3,
            None,
            None,
        )
        .unwrap();

        // The general list is the cold-start strong-shot ordering, brief-independent.
        let general_ids = selection
            .general
            .iter()
            .map(|result| result.asset_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(general_ids, vec!["shot-a", "photo-a", "shot-b"]);
        assert_eq!(selection.general[0].asset_type, "video");
        assert_eq!(selection.general[0].score_breakdown, None);
        assert!((selection.general[0].score - 0.9).abs() < 1e-6);
        // Photo hydration carries no clip boundaries.
        assert_eq!(selection.general[1].asset_type, "photo");
        assert_eq!(selection.general[1].start_s, None);
        // The personalized ordering rides in the same response with full breakdowns.
        assert_eq!(selection.personalized.len(), 3);
        assert!(
            selection.profile.is_none(),
            "brief alone is not personal provenance"
        );
        for result in &selection.personalized {
            assert!(result.score_breakdown.is_some());
        }

        // Without a brief only the general list is produced.
        let general_only = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            None,
            3,
            None,
            None,
        )
        .unwrap();
        assert_eq!(general_only.general.len(), 3);
        assert!(general_only.personalized.is_empty());
        assert!(general_only.profile.is_none());

        // Privacy: a not-usable annotation removes the asset from the general list.
        store
            .upsert_editorial_annotation(
                DEFAULT_OWNER_ID,
                &EditorialAnnotation {
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    media_kind: MediaKind::Shot,
                    media_id: "shot-a".to_owned(),
                    description: String::new(),
                    subjects: String::new(),
                    action: String::new(),
                    tags: String::new(),
                    quality: None,
                    standout: false,
                    usable: false,
                    faces_visible: false,
                    nametags_visible: false,
                    blur_required: false,
                    crop_x: None,
                    grade_json: None,
                    notes: String::new(),
                    updated_at: chrono::Utc::now(),
                },
            )
            .unwrap();
        let filtered = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            None,
            3,
            None,
            None,
        )
        .unwrap();
        assert!(filtered
            .general
            .iter()
            .all(|result| result.asset_id != "shot-a"));
    }

    fn aesthetic_assessment(media_id: &str, overall: f64) -> AestheticAssessment {
        AestheticAssessment {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            media_kind: MediaKind::Shot,
            media_id: media_id.to_owned(),
            sharpness: 0.5,
            exposure: 0.5,
            contrast: 0.5,
            color_harmony: 0.5,
            balance: 0.5,
            subject_placement: 0.5,
            negative_space: 0.5,
            visual_clarity: 0.5,
            technical_quality: 0.5,
            blur_control: 0.5,
            clipping_control: 0.5,
            noise_control: 0.5,
            compression_quality: 0.5,
            resolution_quality: 0.5,
            motion_stability: 0.5,
            duplicate_confidence: 0.0,
            composition_quality: 0.5,
            hierarchy: 0.5,
            leading_lines: 0.5,
            symmetry: 0.5,
            crop_potential: 0.5,
            moment_story: 0.5,
            expression: 0.5,
            gesture: 0.5,
            action: 0.5,
            novelty: 0.5,
            pacing: 0.5,
            repetition_risk: 0.0,
            overall,
            confidence: 1.0,
            explanation_json: "{}".to_owned(),
            model_version: "test-v1".to_owned(),
            assessed_at: chrono::Utc::now(),
        }
    }

    fn populated_store() -> (TempDir, Store) {
        let directory = TempDir::new().unwrap();
        let mut store = Store::open(directory.path()).unwrap();
        store
            .upsert_video(
                DEFAULT_OWNER_ID,
                &Video {
                    id: "video-1".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: "/footage/video.mov".to_owned(),
                    sha256: "video-sha".to_owned(),
                    duration_s: Some(2.0),
                    fps: Some(24.0),
                    width: Some(1920),
                    height: Some(1080),
                    has_audio: true,
                    status: VideoStatus::Embedded,
                    indexed_at: None,
                },
            )
            .unwrap();
        let shots = [
            Shot {
                id: "shot-a".to_owned(),
                video_id: "video-1".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: 0,
                start_s: 0.0,
                end_s: 1.0,
                rep_frame_s: 0.4,
                thumb_rel: Some("shot-a.jpg".to_owned()),
                scene_score: None,
            },
            Shot {
                id: "shot-b".to_owned(),
                video_id: "video-1".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: 1,
                start_s: 1.0,
                end_s: 2.0,
                rep_frame_s: 1.4,
                thumb_rel: Some("shot-b.jpg".to_owned()),
                scene_score: None,
            },
        ];
        store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
        let mut vector = [0.0_f32; EMBEDDING_DIM];
        vector[0] = 1.0;
        for shot in &shots {
            store
                .put_vector(DEFAULT_OWNER_ID, &shot.id, &vector)
                .unwrap();
        }
        let manifest = crush_core::models::bundled_manifest().unwrap();
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
        (directory, store)
    }

    fn normalize(values: &mut [f32]) {
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        for value in values {
            *value /= norm;
        }
    }

    // ---- Task 033: sequence and repetition judgment ----

    fn sequence_plan(store: &mut Store) -> String {
        store
            .plan_create(
                DEFAULT_OWNER_ID,
                &crush_store::Plan {
                    id: "plan-seq".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    name: "Sequence fixture".to_owned(),
                    description: String::new(),
                    context_key: "default".to_owned(),
                    brief: String::new(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                },
            )
            .unwrap();
        "plan-seq".to_owned()
    }

    fn plan_item(
        plan_id: &str,
        media_kind: MediaKind,
        media_id: &str,
        start_s: Option<f64>,
        end_s: Option<f64>,
    ) -> crush_store::PlanItem {
        crush_store::PlanItem {
            owner_id: DEFAULT_OWNER_ID.to_owned(),
            plan_id: plan_id.to_owned(),
            media_kind,
            media_id: media_id.to_owned(),
            position: 0,
            start_s,
            end_s,
            pacing: None,
            crop_x: None,
            grade_json: None,
            reason: String::new(),
            signals_json: "{}".to_owned(),
            origin: crush_store::PlanOrigin::General,
            rank: None,
            profile_version: None,
            provenance_json: "{}".to_owned(),
            added_at: chrono::Utc::now(),
        }
    }

    fn sequence_fixture() -> (TempDir, Store, String) {
        let (directory, mut store) = populated_store();
        // A second video with a visually distinct shot, so coverage has two sources.
        store
            .upsert_video(
                DEFAULT_OWNER_ID,
                &Video {
                    id: "video-2".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    path: "/footage/video-b.mov".to_owned(),
                    sha256: "video-sha-2".to_owned(),
                    duration_s: Some(2.0),
                    fps: Some(24.0),
                    width: Some(1920),
                    height: Some(1080),
                    has_audio: false,
                    status: VideoStatus::Embedded,
                    indexed_at: None,
                },
            )
            .unwrap();
        let shots = [
            Shot {
                id: "shot-c".to_owned(),
                video_id: "video-2".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: 0,
                start_s: 0.0,
                end_s: 1.5,
                rep_frame_s: 0.5,
                thumb_rel: None,
                scene_score: None,
            },
            Shot {
                id: "shot-d".to_owned(),
                video_id: "video-2".to_owned(),
                owner_id: DEFAULT_OWNER_ID.to_owned(),
                idx: 1,
                start_s: 1.5,
                end_s: 2.0,
                rep_frame_s: 1.6,
                thumb_rel: None,
                scene_score: None,
            },
        ];
        store.insert_shots(DEFAULT_OWNER_ID, &shots).unwrap();
        // shot-a and shot-b share one vector (near-duplicates); shot-c is orthogonal.
        let mut other = [0.0_f32; EMBEDDING_DIM];
        other[1] = 1.0;
        store
            .put_vector(DEFAULT_OWNER_ID, "shot-c", &other)
            .unwrap();
        store
            .put_vector(DEFAULT_OWNER_ID, "shot-d", &other)
            .unwrap();
        let plan_id = sequence_plan(&mut store);
        store
            .plan_add_item(
                DEFAULT_OWNER_ID,
                &plan_item(&plan_id, MediaKind::Shot, "shot-a", Some(0.0), Some(1.0)),
            )
            .unwrap();
        store
            .plan_add_item(
                DEFAULT_OWNER_ID,
                &plan_item(&plan_id, MediaKind::Shot, "shot-b", Some(1.0), Some(2.0)),
            )
            .unwrap();
        store
            .plan_add_item(
                DEFAULT_OWNER_ID,
                &plan_item(&plan_id, MediaKind::Shot, "shot-c", Some(0.0), Some(1.5)),
            )
            .unwrap();
        (directory, store, plan_id)
    }

    #[test]
    fn sequence_report_flags_near_duplicate_adjacency_and_coverage() {
        let (_directory, store, plan_id) = sequence_fixture();
        let items = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let report = sequence_report_helper(&store, &items);

        assert_eq!(report.summary.item_count, 3);
        assert_eq!(report.summary.distinct_sources, 2);
        assert_eq!(report.summary.near_duplicate_adjacencies, 1);
        assert!(report
            .summary
            .coverage_note
            .contains("3 items from 2 distinct sources"));
        assert!(report.summary.pacing_note.contains("median"));

        let first_cut = &report.transitions[0];
        assert!(
            first_cut.near_duplicate,
            "identical vectors are near-duplicates"
        );
        assert!(first_cut.same_source, "shot-a and shot-b share video-1");
        assert!(first_cut.similarity.is_some_and(|value| value > 0.99));
        assert!(first_cut.note.contains("near-identical"));
        let second_cut = &report.transitions[1];
        assert!(!second_cut.near_duplicate);
        assert!(!second_cut.same_source, "shot-c lives on video-2");
        assert!(second_cut.note.is_empty(), "quiet transitions say nothing");

        assert!(report.items[1]
            .notes
            .iter()
            .any(|note| note.contains("near-identical to the previous item")));
        assert!(report.items[0].neighbor_similarity.unwrap() > 0.99);
    }

    #[test]
    fn sequence_suggestion_apply_clears_the_adjacency() {
        let (_directory, mut store, plan_id) = sequence_fixture();
        let items = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let suggestions = crate::sequence::sequence_suggestions(&store, &items).unwrap();
        assert_eq!(suggestions.len(), 1, "one adjacency, one suggestion");
        let suggestion = &suggestions[0];
        assert_eq!(suggestion.position, 1, "the later twin moves");
        assert_eq!(suggestion.neighbor_position, 0);
        assert!(suggestion.note.contains("Move item 2 to the end"));

        let ordered = crate::sequence::reorder_pairs(suggestion);
        store
            .plan_reorder_items(DEFAULT_OWNER_ID, &plan_id, &ordered)
            .unwrap();
        let reordered = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let after = crate::sequence::sequence_report(&store, &reordered).expect("report succeeds");
        assert_eq!(
            after.summary.near_duplicate_adjacencies, 0,
            "applying the suggestion must clear the flagged adjacency"
        );
        // b was moved to the end; a and c now sit adjacent and they are dissimilar.
        assert_eq!(reordered[2].media_id, "shot-b");
    }

    #[test]
    fn sequence_report_groups_imported_spans_by_external_source() {
        let (_directory, mut store, plan_id) = sequence_fixture();
        // Two spans imported from one Reel Studio project (import-1) plus one from another
        // import. Spans are not embedded (Task 022), so their repetition evidence is
        // provenance, not cosine.
        for (id, external, import_id, video, start, end) in [
            ("span-a", "seg-1", "import-1", "video-1", 0.0, 1.0),
            ("span-b", "seg-2", "import-1", "video-1", 1.0, 2.0),
            ("span-c", "seg-3", "import-2", "video-1", 0.0, 1.0),
        ] {
            store
                .manual_span_upsert(
                    DEFAULT_OWNER_ID,
                    &crush_store::ManualSpan {
                        id: id.to_owned(),
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        video_id: video.to_owned(),
                        source: "reel_studio".to_owned(),
                        external_id: external.to_owned(),
                        start_s: start,
                        end_s: end,
                        boundary_basis: crush_store::SpanBoundaryBasis::CatalogueTc,
                        boundary_tolerance_s: 0.05,
                        library_relative_offset_s: 0.0,
                        description: String::new(),
                        shot_type: String::new(),
                        camera_move: String::new(),
                        subjects: String::new(),
                        action: String::new(),
                        tags: String::new(),
                        quality: None,
                        standout: false,
                        usable: true,
                        faces_visible: true,
                        nametags_visible: false,
                        blur_required: false,
                        used_in: String::new(),
                        crop_x: None,
                        notes: String::new(),
                        import_id: None,
                        imported_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    },
                )
                .unwrap();
            let mut item = plan_item(&plan_id, MediaKind::Span, id, Some(start), Some(end));
            item.provenance_json = format!(
                r#"{{"source":"reel_studio","external_id":"{external}","import_id":"{import_id}"}}"#
            );
            store.plan_add_item(DEFAULT_OWNER_ID, &item).unwrap();
        }
        let items = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let report = sequence_report_helper(&store, &items);
        let span_transition = &report.transitions[3]; // between span-a and span-b
        assert!(
            span_transition.same_source,
            "same external_id is one source"
        );
        assert!(!span_transition.near_duplicate, "spans carry no embeddings");
        assert!(span_transition.similarity.is_none());
        assert!(span_transition.note.contains("same source"));
        // Coverage: 3 shots across 2 videos + 2 span sources = 4 distinct sources now.
        assert_eq!(report.summary.distinct_sources, 4);
    }

    #[test]
    fn selects_duplicate_cap_diversifies_per_source() {
        let (_directory, store) = populated_store();
        // photo-b is a near-duplicate of photo-a (a burst), photo-c is distinct.
        for (id, sha) in [
            ("photo-a", "photo-a-sha"),
            ("photo-b", "photo-b-sha"),
            ("photo-c", "photo-c-sha"),
        ] {
            store
                .upsert_photo(
                    DEFAULT_OWNER_ID,
                    &Photo {
                        id: id.to_owned(),
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        path: format!("/photos/{id}.jpg"),
                        sha256: sha.to_owned(),
                        width: 6000,
                        height: 4000,
                        format: "jpeg".to_owned(),
                        orientation: None,
                        captured_at: None,
                        camera_make: None,
                        camera_model: None,
                        lens: None,
                        thumb_rel: None,
                        status: PhotoStatus::Done,
                        indexed_at: None,
                    },
                )
                .unwrap();
        }
        let mut burst = [0.0_f32; EMBEDDING_DIM];
        burst[0] = 1.0;
        let mut distinct = [0.0_f32; EMBEDDING_DIM];
        distinct[1] = 1.0;
        for (id, vector) in [
            ("photo-a", burst),
            ("photo-b", burst),
            ("photo-c", distinct),
        ] {
            store
                .put_photo_vector(DEFAULT_OWNER_ID, id, &vector)
                .unwrap();
            store
                .upsert_aesthetic_assessment(
                    DEFAULT_OWNER_ID,
                    &AestheticAssessment {
                        media_kind: MediaKind::Photo,
                        media_id: id.to_owned(),
                        ..aesthetic_assessment(id, 0.8)
                    },
                )
                .unwrap();
        }
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-a", 0.9))
            .unwrap();
        store
            .upsert_aesthetic_assessment(DEFAULT_OWNER_ID, &aesthetic_assessment("shot-b", 0.7))
            .unwrap();
        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0_f32; EMBEDDING_DIM]);

        let uncapped = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            None,
            5,
            None,
            None,
        )
        .unwrap();
        assert_eq!(uncapped.duplicate_cap, None);
        assert_eq!(uncapped.skipped_duplicates, 0);
        assert_eq!(uncapped.general.len(), 5);

        let capped = selects_candidates(
            &store,
            DEFAULT_OWNER_ID,
            &engine,
            &mut embedder,
            None,
            5,
            None,
            Some(1),
        )
        .unwrap();
        assert_eq!(capped.duplicate_cap, Some(1));
        // Ranked order shot-a (video-1), photo-b (0.8 tie order by id then photo-a, photo-c,
        // shot-b (video-1 again): the cap keeps one per source, so video-1's second shot and
        // the burst twin are skipped; photo-c and photo-a survive as their own exhibits.
        let kept_ids = capped
            .general
            .iter()
            .map(|result| result.asset_id.as_str())
            .collect::<Vec<_>>();
        assert!(
            kept_ids
                .iter()
                .filter(|id| **id == "shot-a" || **id == "shot-b")
                .count()
                == 1,
            "only one shot per source video survives the cap: {kept_ids:?}"
        );
        assert!(
            !(kept_ids.contains(&"photo-a") && kept_ids.contains(&"photo-b")),
            "near-duplicate photo bursts count as one source: {kept_ids:?}"
        );
        assert_eq!(capped.general.len() + capped.skipped_duplicates, 5);
        assert_eq!(capped.skipped_duplicates, 5 - capped.general.len());
    }

    #[test]
    fn sequence_suggestions_refuse_a_move_that_creates_new_adjacencies() {
        let (_directory, mut store, plan_id) = sequence_fixture();
        // A mutual near-duplicate clique: shot-e repeats shot-a's vector, so shot-a,
        // shot-b and shot-e are pairwise near-identical. Moving the flagged twin to the
        // end separates that pair but seats it next to the third twin, so the chip's
        // promise would be false after applying it.
        store
            .insert_shots(
                DEFAULT_OWNER_ID,
                &[Shot {
                    id: "shot-e".to_owned(),
                    video_id: "video-2".to_owned(),
                    owner_id: DEFAULT_OWNER_ID.to_owned(),
                    idx: 2,
                    start_s: 2.0,
                    end_s: 2.5,
                    rep_frame_s: 2.1,
                    thumb_rel: None,
                    scene_score: None,
                }],
            )
            .unwrap();
        let mut twin = [0.0_f32; EMBEDDING_DIM];
        twin[0] = 1.0;
        store.put_vector(DEFAULT_OWNER_ID, "shot-e", &twin).unwrap();
        store
            .plan_remove_item(DEFAULT_OWNER_ID, &plan_id, MediaKind::Shot, "shot-c")
            .unwrap();
        store
            .plan_add_item(
                DEFAULT_OWNER_ID,
                &plan_item(&plan_id, MediaKind::Shot, "shot-e", Some(2.0), Some(2.5)),
            )
            .unwrap();
        let items = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let report = crate::sequence::sequence_report(&store, &items).unwrap();
        assert_eq!(
            report.summary.near_duplicate_adjacencies, 2,
            "the clique flags both adjacencies"
        );
        let suggestions = crate::sequence::sequence_suggestions(&store, &items).unwrap();
        assert!(
            suggestions.is_empty(),
            "a move that trades the flagged pair for new adjacencies must not be suggested"
        );
    }

    #[test]
    fn sequence_suggestions_refuse_a_move_that_cannot_separate() {
        let (_directory, mut store, plan_id) = sequence_fixture();
        // A two-item plan with a near-duplicate pair: moving either twin to the end is a
        // no-op, so no suggestion may pretend to fix it. The note alone informs the editor.
        store
            .plan_remove_item(DEFAULT_OWNER_ID, &plan_id, MediaKind::Shot, "shot-c")
            .unwrap();
        let items = store.plan_items(DEFAULT_OWNER_ID, &plan_id).unwrap();
        let suggestions = crate::sequence::sequence_suggestions(&store, &items).unwrap();
        assert!(
            suggestions.is_empty(),
            "a no-op reorder must not be suggested"
        );
        let report = crate::sequence::sequence_report(&store, &items).unwrap();
        assert_eq!(report.summary.near_duplicate_adjacencies, 1);
        assert!(report.transitions[0].note.contains("near-identical"));
    }

    fn sequence_report_helper(
        store: &Store,
        items: &[crush_store::PlanItem],
    ) -> crate::sequence::SequenceReport {
        crate::sequence::sequence_report(store, items).expect("sequence report succeeds")
    }
}
