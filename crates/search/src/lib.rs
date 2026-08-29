//! In-process cosine search with a deliberately small transcript keyword boost.
//!
//! This crate does not depend on ONNX Runtime. Callers supply text embeddings through
//! [`TextEmbedder`], keeping the index and ranking logic independently testable.

use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashMap},
};

use anyhow::{ensure, Context};
use crush_store::{FeedbackSignal, MediaKind, Store, StyleProfile};
use serde::Serialize;

pub const EMBEDDING_DIM: usize = 512;

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
}

/// Rebuild the active owner-specific visual preference vector from retained feedback.
/// This is intentionally an auditable linear baseline; later trainers can replace it behind the
/// versioned StyleProfile record without changing feedback or search contracts.
pub fn retrain_style_profile(
    store: &mut Store,
    owner_id: &str,
) -> anyhow::Result<Option<StyleProfile>> {
    let events = store.feedback_events(owner_id)?;
    let mut weights = vec![0.0_f32; EMBEDDING_DIM];
    let mut sample_count = 0_i64;
    for event in events {
        let coefficient = match event.signal {
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
        if coefficient != 0.0 {
            if let Some(vector) = media_vector(store, owner_id, event.media_kind, &event.media_id)?
            {
                add_scaled(&mut weights, &vector, coefficient)?;
                sample_count += 1;
            }
        }
        if event.signal == FeedbackSignal::Prefer {
            if let (Some(kind), Some(id)) = (event.compared_media_kind, event.compared_media_id) {
                if let Some(vector) = media_vector(store, owner_id, kind, &id)? {
                    add_scaled(&mut weights, &vector, -1.0)?;
                    sample_count += 1;
                }
            }
        }
    }
    let norm = weights
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if sample_count == 0 || norm <= f32::EPSILON {
        return Ok(None);
    }
    for value in &mut weights {
        *value /= norm;
    }
    let previous_version = store
        .active_style_profile(owner_id)?
        .map_or(0, |profile| profile.version);
    let profile = StyleProfile {
        id: uuid::Uuid::new_v4().to_string(),
        owner_id: owner_id.to_owned(),
        name: "default".to_owned(),
        version: previous_version + 1,
        algorithm_version: "feedback-centroid-v1".to_owned(),
        embedding_weights: weights,
        feature_weights_json: "{}".to_owned(),
        sample_count,
        held_out_metric: None,
        active: true,
        trained_at: chrono::Utc::now(),
    };
    store.put_style_profile(owner_id, &profile)?;
    Ok(Some(profile))
}

fn media_vector(
    store: &Store,
    owner_id: &str,
    kind: MediaKind,
    id: &str,
) -> anyhow::Result<Option<Vec<f32>>> {
    match kind {
        MediaKind::Photo => store.vector_for_photo(owner_id, id),
        MediaKind::Shot => store.vector_for_shot(owner_id, id),
    }
}

fn add_scaled(target: &mut [f32], values: &[f32], scale: f32) -> anyhow::Result<()> {
    ensure!(
        target.len() == values.len(),
        "feedback vector dimension {} does not match style dimension {}",
        values.len(),
        target.len()
    );
    for (target, value) in target.iter_mut().zip(values) {
        *target += value * scale;
    }
    Ok(())
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
        let style_profile = store.active_style_profile(&self.owner_id)?;
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
            let personal_style_score = personal_style_score(
                store,
                &self.owner_id,
                MediaKind::Shot,
                &found.shot_id,
                style_profile.as_ref(),
            )?;
            let score = found.score
                + editorial_adjustment(annotation.as_ref())
                + general_aesthetic_adjustment(aesthetic.as_ref())
                + personal_style_score.unwrap_or(0.0) * 0.15;
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
                aesthetic_score: aesthetic.map(|value| value.overall),
                personal_style_score,
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
            let personal_style_score = personal_style_score(
                store,
                &self.owner_id,
                MediaKind::Photo,
                &found.shot_id,
                style_profile.as_ref(),
            )?;
            let score = found.score
                + editorial_adjustment(annotation.as_ref())
                + general_aesthetic_adjustment(aesthetic.as_ref())
                + personal_style_score.unwrap_or(0.0) * 0.15;
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
                aesthetic_score: aesthetic.map(|value| value.overall),
                personal_style_score,
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

fn editorial_adjustment(annotation: Option<&crush_store::EditorialAnnotation>) -> f32 {
    let Some(annotation) = annotation else {
        return 0.0;
    };
    if !annotation.usable {
        return -1.0;
    }
    let quality = annotation
        .quality
        .map_or(0.0, |value| (value as f32 - 3.0) * 0.025);
    quality + if annotation.standout { 0.05 } else { 0.0 }
}

fn general_aesthetic_adjustment(assessment: Option<&crush_store::AestheticAssessment>) -> f32 {
    assessment.map_or(0.0, |value| ((value.overall - 0.5) * 0.16) as f32)
}

fn personal_style_score(
    store: &Store,
    owner_id: &str,
    media_kind: MediaKind,
    media_id: &str,
    profile: Option<&StyleProfile>,
) -> anyhow::Result<Option<f32>> {
    let Some(profile) = profile else {
        return Ok(None);
    };
    if profile.embedding_weights.len() != EMBEDDING_DIM {
        return Ok(None);
    }
    Ok(media_vector(store, owner_id, media_kind, media_id)?
        .filter(|vector| vector.len() == EMBEDDING_DIM)
        .map(|vector| dot_512(&vector, &profile.embedding_weights)))
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
        EmbeddingMeta, FeedbackEvent, FeedbackSignal, MediaKind, Shot, TranscriptSegment, Video,
        VideoStatus,
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
        let (_directory, mut store) = populated_store();
        let mut preferred = [0.0_f32; EMBEDDING_DIM];
        preferred[1] = 1.0;
        store
            .put_vector(DEFAULT_OWNER_ID, "shot-b", &preferred)
            .unwrap();
        for (id, media_id, signal, value) in [
            ("feedback-pick", "shot-b", FeedbackSignal::Pick, Some(1.0)),
            (
                "feedback-reject",
                "shot-a",
                FeedbackSignal::Reject,
                Some(-1.0),
            ),
        ] {
            store
                .append_feedback(
                    DEFAULT_OWNER_ID,
                    &FeedbackEvent {
                        id: id.to_owned(),
                        owner_id: DEFAULT_OWNER_ID.to_owned(),
                        media_kind: MediaKind::Shot,
                        media_id: media_id.to_owned(),
                        signal,
                        value,
                        compared_media_kind: None,
                        compared_media_id: None,
                        context_json: "{}".to_owned(),
                        created_at: chrono::Utc::now(),
                    },
                )
                .unwrap();
        }
        let profile = retrain_style_profile(&mut store, DEFAULT_OWNER_ID)
            .unwrap()
            .unwrap();
        assert_eq!(profile.sample_count, 2);
        assert_eq!(profile.algorithm_version, "feedback-centroid-v1");

        let engine = SearchEngine::load(&store, DEFAULT_OWNER_ID, 0.0).unwrap();
        let mut embedder = |_text: &str| Ok([0.0_f32; EMBEDDING_DIM]);
        let results = engine
            .search_assets(&store, &mut embedder, "same semantics", 2)
            .unwrap();
        assert_eq!(results[0].asset_id, "shot-b");
        assert!(results[0].personal_style_score.unwrap() > 0.0);
        assert!(results[1].personal_style_score.unwrap() < 0.0);
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
}
