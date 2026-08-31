//! Personal style learning for search.
//!
//! The general ranker stays intact and first-class; this module trains a bounded linear
//! residual over CLIP embeddings and persisted aesthetic features, scoped per context key,
//! and gates the result through a held-out evaluation before any profile is called "learned".

pub mod eval;
pub mod trainer;

pub use eval::{
    evaluate, metrics_json, split_pairs, EvalOutcome, RankedPair, Split, MIN_HELD_OUT_PAIRS,
    MIN_PERSONAL_ACCURACY, PERSONAL_AFFINITY_SCALE, SPLIT_LABEL,
};
pub use trainer::{
    retrain_style_profile, retrain_style_profile_for_context, DEFAULT_CONTEXT_KEY,
    DEFAULT_MIN_SAMPLES, NAMED_CONTEXT_MIN_SAMPLES, TRAINER_VERSION,
};
