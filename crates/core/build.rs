//! Embed build provenance from the environment (Task 023).
//!
//! Release/CI builds export `CRUSH_BUILD_COMMIT` before invoking cargo — a
//! short commit hash plus a `-dirty` suffix when the tree is not clean (see
//! `scripts/package-macos.sh` and `docs/release.md`). When the variable is
//! unset, binaries honestly report `unknown-local` instead of guessing a
//! commit. No new dependencies: this only forwards one environment variable
//! to the compiler.

fn main() {
    // Rebuild when the stamp changes so consecutive builds cannot carry a
    // stale commit.
    println!("cargo:rerun-if-env-changed=CRUSH_BUILD_COMMIT");
    let commit =
        std::env::var("CRUSH_BUILD_COMMIT").unwrap_or_else(|_| "unknown-local".to_string());
    println!("cargo:rustc-env=CRUSH_BUILD_COMMIT={commit}");
}
