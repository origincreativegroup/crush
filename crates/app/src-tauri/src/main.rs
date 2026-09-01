// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Task 023 build provenance: `crush-app --build-info` (and --version) lets
    // release tooling read the commit stamped at build time from the bundle
    // binary without launching the GUI. scripts/verify-release.sh parses the
    // exact "build commit: <value>" line.
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--build-info" || arg == "--version")
    {
        println!("crush-app {}", env!("CARGO_PKG_VERSION"));
        println!("build commit: {}", crush_core::BUILD_COMMIT);
        return;
    }
    crush_app_lib::run();
}
