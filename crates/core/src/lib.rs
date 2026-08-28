//! crush-core — shared contracts: config, paths, tracing, job log types, errors.
//! Every other crate depends on this one. This one depends on nothing in the workspace.

pub mod cancellation;
pub mod config;
pub mod job;
pub mod models;
pub mod paths;
pub mod telemetry;

pub use config::Config;

/// Owner id used everywhere in Phase 1. Kept as a real column so Phase 2 needs no migration.
pub const DEFAULT_OWNER_ID: &str = "local";
