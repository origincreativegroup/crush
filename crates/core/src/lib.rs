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

/// Build provenance stamped at compile time by `build.rs` from the
/// `CRUSH_BUILD_COMMIT` environment variable (short commit, plus `-dirty`
/// when the build tree was not clean). `unknown-local` means the build command
/// did not stamp a commit — an honest label, never a guessed hash. Surfaced by
/// `crushctl --version` and `crush-app --build-info`, and read from the bundle
/// by `scripts/verify-release.sh`.
pub const BUILD_COMMIT: &str = env!("CRUSH_BUILD_COMMIT");

#[cfg(test)]
mod build_info_tests {
    use super::BUILD_COMMIT;

    #[test]
    fn build_commit_is_never_empty_or_silently_unknown() {
        // Either an honestly unstamped local build or a stamped commit —
        // never empty and never a silent "unknown".
        assert!(!BUILD_COMMIT.is_empty());
        assert_ne!(BUILD_COMMIT, "unknown");
    }

    #[test]
    fn stamped_commit_looks_like_a_short_sha() {
        if BUILD_COMMIT != "unknown-local" {
            assert!(
                BUILD_COMMIT
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '-'),
                "stamped commit should be a hex short sha (optionally -dirty), got {BUILD_COMMIT}"
            );
        }
    }
}
