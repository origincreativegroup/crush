//! Automatic sequence and repetition judgment for ordered plans (Task 033).
//!
//! This closes the open Task 020 acceptance: candidate ordering ranks individual assets, but
//! nothing judged the sequence *as a sequence*. The signals here are explainable per-item and
//! per-transition observations over an ordered plan — near-duplicate neighbors (embedding
//! cosine), same-source adjacency (video/span provenance), pacing distribution, and coverage
//! across source clips/exhibits — expressed in plain editor language.
//!
//! Nothing here reorders a plan by itself and nothing claims "optimization": suggestions are
//! reads the UI offers as one-click actions that go through the normal plan state APIs (and
//! the existing revision history provides the undo). Docs rule (docs/review-2026-08-29.md
//! finding 7): describe what was measured, never label it automatic optimization.

use std::collections::BTreeMap;

use anyhow::Context as _;
use serde::Serialize;

use crush_store::{MediaKind, PlanItem, Store};

/// Cosine above which two neighbors are called near-duplicates (plain-language badge).
pub const NEAR_DUPLICATE_COSINE: f32 = 0.95;
/// Cosine above which two same-source neighbors are also called visually similar.
pub const REPETITION_COSINE: f32 = 0.85;

/// Plain-language sequence observations for one ordered plan.
#[derive(Debug, Clone, Serialize)]
pub struct SequenceReport {
    /// Per-position observations, in plan order.
    pub items: Vec<ItemSequence>,
    /// Per-adjacent-pair observations; `transitions[i]` sits between positions i and i+1.
    pub transitions: Vec<Transition>,
    pub summary: SequenceSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemSequence {
    pub position: usize,
    pub media_kind: String,
    pub media_id: String,
    /// Strongest similarity to an adjacent item, when both sides have embeddings. Imported
    /// spans are not embedded (Task 022), so their repetition evidence is source/temporal
    /// and stays `None` here.
    pub neighbor_similarity: Option<f32>,
    /// Plain-language observations for this position; empty means nothing notable.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transition {
    /// Between this position and the next one.
    pub position: usize,
    pub similarity: Option<f32>,
    pub near_duplicate: bool,
    pub same_source: bool,
    /// One plain-language sentence; empty means quiet.
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SequenceSummary {
    pub item_count: usize,
    /// Distinct source clips/exhibits behind the items (photos count individually; imported
    /// spans count by their external source).
    pub distinct_sources: usize,
    /// `source key -> item count`, sorted for deterministic rendering.
    pub sources: BTreeMap<String, usize>,
    /// Plain-language coverage sentence, e.g. "8 items from 3 distinct sources".
    pub coverage_note: String,
    /// Plain-language pacing sentence over video item durations; empty when no video items.
    pub pacing_note: String,
    /// Count of adjacent near-duplicate transitions.
    pub near_duplicate_adjacencies: usize,
}

/// One one-click reorder suggestion. Applying it writes normal plan state through the
/// existing reorder API; undo is the existing plan revision restore.
#[derive(Debug, Clone, Serialize)]
pub struct SequenceSuggestion {
    /// The item being moved (by plan position and media identity).
    pub position: usize,
    pub media_kind: String,
    pub media_id: String,
    /// The transition that motivated the move.
    pub neighbor_position: usize,
    /// Plain-language reason shown on the chip.
    pub note: String,
    /// The full suggested order as display pairs, ready for [`Store::plan_reorder_items`]
    /// through [`reorder_pairs`].
    pub suggested_order: Vec<OrderedItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderedItem {
    pub media_kind: String,
    pub media_id: String,
}

/// Score one ordered plan. The slice is sorted by position first, so the report is
/// deterministic regardless of the caller's ordering; every lookup is owner-checked by the
/// store (each plan item carries its `owner_id`).
pub fn sequence_report(store: &Store, items: &[PlanItem]) -> anyhow::Result<SequenceReport> {
    let mut ordered: Vec<&PlanItem> = items.iter().collect();
    ordered.sort_by_key(|item| item.position);
    for (index, item) in ordered.iter().enumerate() {
        anyhow::ensure!(
            item.position == index as i64,
            "plan positions must be dense 0..n; item {} sits at {}",
            item.media_id,
            item.position
        );
    }

    let sources = source_keys(store, &ordered)?;
    let vectors = neighbor_vectors(store, &ordered)?;

    let mut transitions = Vec::with_capacity(ordered.len().saturating_sub(1));
    for window in ordered.windows(2) {
        let (left, right) = (window[0], window[1]);
        let similarity = match (
            &vectors[left.position as usize],
            &vectors[right.position as usize],
        ) {
            (Some(left), Some(right)) => Some(cosine(left, right)),
            _ => None,
        };
        let near_duplicate = similarity.is_some_and(|value| value >= NEAR_DUPLICATE_COSINE);
        let same_source = sources[left.position as usize] == sources[right.position as usize];
        transitions.push(Transition {
            position: left.position as usize,
            similarity,
            near_duplicate,
            same_source,
            note: transition_note(
                near_duplicate,
                same_source,
                similarity,
                &sources[left.position as usize],
            ),
        });
    }

    let mut items_out = Vec::with_capacity(ordered.len());
    for (index, item) in ordered.iter().enumerate() {
        let previous = if index > 0 {
            Some(&transitions[index - 1])
        } else {
            None
        };
        let next = transitions.get(index);
        let mut notes = Vec::new();
        let mut neighbor_similarity = None;
        for (side, transition) in [("previous", previous), ("next", next)] {
            let Some(transition) = transition else {
                continue;
            };
            if let Some(similarity) = transition.similarity {
                neighbor_similarity =
                    Some(neighbor_similarity.map_or(similarity, |best: f32| best.max(similarity)));
            }
            if transition.near_duplicate {
                notes.push(format!("Looks near-identical to the {side} item."));
            } else if transition.same_source {
                notes.push(format!("Comes from the same source as the {side} item."));
            }
        }
        items_out.push(ItemSequence {
            position: index,
            media_kind: media_kind_name(item.media_kind).to_owned(),
            media_id: item.media_id.clone(),
            neighbor_similarity,
            notes,
        });
    }

    let summary = summarize(&ordered, &sources, &transitions);
    Ok(SequenceReport {
        items: items_out,
        transitions,
        summary,
    })
}

/// Deterministic one-click suggestions for the near-duplicate adjacencies found in the plan.
/// At most one suggestion per affected item; the move always takes the later twin out of the
/// adjacency by sending it to the end of the sequence — blunt, visible, and undoable, never
/// a silent reorder.
pub fn sequence_suggestions(
    store: &Store,
    items: &[PlanItem],
) -> anyhow::Result<Vec<SequenceSuggestion>> {
    let report = sequence_report(store, items)?;
    let mut ordered: Vec<&PlanItem> = items.iter().collect();
    ordered.sort_by_key(|item| item.position);

    let mut suggestions = Vec::new();
    let mut already_moved: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for transition in &report.transitions {
        if !transition.near_duplicate {
            continue;
        }
        let later = transition.position + 1;
        let item = &ordered[later];
        if !already_moved.insert(item.media_id.clone()) {
            continue;
        }
        let mut order: Vec<OrderedItem> = ordered
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != later)
            .map(|(_, item)| OrderedItem {
                media_kind: media_kind_name(item.media_kind).to_owned(),
                media_id: item.media_id.clone(),
            })
            .collect();
        order.push(OrderedItem {
            media_kind: media_kind_name(item.media_kind).to_owned(),
            media_id: item.media_id.clone(),
        });
        // A move that cannot separate the pair (the twin already at the tail, or a plan of
        // exactly two items) is no-op reordering — say nothing instead of pretending.
        if order
            .iter()
            .map(|entry| entry.media_id.as_str())
            .eq(ordered.iter().map(|item| item.media_id.as_str()))
        {
            continue;
        }
        suggestions.push(SequenceSuggestion {
            position: later,
            media_kind: media_kind_name(item.media_kind).to_owned(),
            media_id: item.media_id.clone(),
            neighbor_position: transition.position,
            note: format!(
                "Items {} and {} look near-identical. Move this one to the end so similar shots are not back-to-back.",
                transition.position + 1,
                later + 1
            ),
            suggested_order: order,
        });
    }
    Ok(suggestions)
}

/// `(media_kind, media_id)` pairs for the store's reorder API, from a suggestion.
pub fn reorder_pairs(suggestion: &SequenceSuggestion) -> Vec<(MediaKind, String)> {
    suggestion
        .suggested_order
        .iter()
        .map(|item| {
            (
                match item.media_kind.as_str() {
                    "photo" => MediaKind::Photo,
                    "span" => MediaKind::Span,
                    _ => MediaKind::Shot,
                },
                item.media_id.clone(),
            )
        })
        .collect()
}

fn media_kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Shot => "shot",
        MediaKind::Span => "span",
    }
}

pub(crate) fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (l, r) in left.iter().zip(right) {
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator > 0.0 {
        dot / denominator
    } else {
        0.0
    }
}

/// Source key per position: photos are their own exhibits; shots group by their source
/// video; imported spans group by their external source id when the import recorded one.
fn source_keys(store: &Store, ordered: &[&PlanItem]) -> anyhow::Result<Vec<String>> {
    let mut keys = Vec::with_capacity(ordered.len());
    for item in ordered {
        let key = match item.media_kind {
            MediaKind::Photo => format!("photo:{}", item.media_id),
            MediaKind::Shot => {
                let context = store
                    .search_shot_context(&item.owner_id, &item.media_id)?
                    .with_context(|| format!("plan item {} no longer exists", item.media_id))?;
                format!("video:{}", context.video_id)
            }
            MediaKind::Span => {
                let provenance: serde_json::Value =
                    serde_json::from_str(&item.provenance_json).unwrap_or(serde_json::Value::Null);
                // The import is the project-level source (Reel Studio segments each carry
                // their own external_id); fall back to the external id when no import was
                // recorded, and to the span itself for unimported manual spans.
                match provenance
                    .get("import_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    Some(import_id) => format!("span-import:{import_id}"),
                    None => {
                        let external = provenance
                            .get("external_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(&item.media_id);
                        format!("span-source:{external}")
                    }
                }
            }
        };
        keys.push(key);
    }
    Ok(keys)
}

fn neighbor_vectors(store: &Store, ordered: &[&PlanItem]) -> anyhow::Result<Vec<Option<Vec<f32>>>> {
    let mut vectors = Vec::with_capacity(ordered.len());
    for item in ordered {
        vectors.push(crate::media_vector(
            store,
            &item.owner_id,
            item.media_kind,
            &item.media_id,
        )?);
    }
    Ok(vectors)
}

fn transition_note(
    near_duplicate: bool,
    same_source: bool,
    similarity: Option<f32>,
    source: &str,
) -> String {
    if near_duplicate {
        return format!(
            "These two neighbors look near-identical (cosine {:.2}).",
            similarity.unwrap_or_default()
        );
    }
    if same_source {
        let similar = match similarity {
            Some(value) if value >= REPETITION_COSINE => {
                format!(" The frames also look similar (cosine {value:.2}).")
            }
            _ => String::new(),
        };
        return format!("Two items in a row come from the same source ({source}).{similar}");
    }
    String::new()
}

fn summarize(
    ordered: &[&PlanItem],
    sources: &[String],
    transitions: &[Transition],
) -> SequenceSummary {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for key in sources {
        *counts.entry(key.clone()).or_insert(0) += 1;
    }
    let distinct = counts.len();
    let busiest = counts.values().max().copied().unwrap_or(0);
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let coverage_note = format!(
        "{} item{} from {} distinct source{}; the busiest source contributes {} item{}.",
        ordered.len(),
        plural(ordered.len()),
        distinct,
        plural(distinct),
        busiest,
        plural(busiest),
    );

    let mut durations: Vec<f64> = ordered
        .iter()
        .filter(|item| item.media_kind != MediaKind::Photo)
        .map(|item| {
            let start = item.start_s.unwrap_or(0.0);
            let end = item.end_s.unwrap_or(start);
            (end - start).abs()
        })
        .filter(|duration| *duration > 0.0)
        .collect();
    durations.sort_by(|a, b| a.total_cmp(b));
    let pacing_note = if durations.is_empty() {
        String::new()
    } else {
        format!(
            "Video item durations run {:.1}s to {:.1}s (median {:.1}s).",
            durations[0],
            durations[durations.len() - 1],
            durations[durations.len() / 2]
        )
    };

    SequenceSummary {
        item_count: ordered.len(),
        distinct_sources: distinct,
        sources: counts,
        coverage_note,
        pacing_note,
        near_duplicate_adjacencies: transitions.iter().filter(|t| t.near_duplicate).count(),
    }
}
