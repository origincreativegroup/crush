//! Personal style learning for search.
//!
//! The general ranker stays intact and first-class; this module trains a bounded linear
//! residual over CLIP embeddings and persisted aesthetic features, scoped per context key,
//! and gates the result through a held-out evaluation before any profile is called "learned".

pub mod eval;
pub mod trainer;

pub use eval::{
    baseline_vote, evaluate, metrics_json, EvalOutcome, RankedPair, MIN_HELD_OUT_PAIRS,
    MIN_PERSONAL_ACCURACY, SPLIT_LABEL,
};
pub use trainer::{
    retrain_style_profile, retrain_style_profile_for_context, DEFAULT_CONTEXT_KEY,
    DEFAULT_MIN_SAMPLES, NAMED_CONTEXT_MIN_SAMPLES, TRAINER_VERSION,
};
