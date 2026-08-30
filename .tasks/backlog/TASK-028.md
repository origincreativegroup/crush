# TASK-028: Cross-platform contracts, Windows shell, and CI

Depends: Task 021 contracts. May begin after the Task 021 interfaces stabilize; it does not replace
the ordered Mac/product work in Tasks 021–023.

## Acceptance

- [ ] Move the Tauri application and command bridge out of the macOS-only compile gate; the same
      Rust application services build under MSVC and launch in a supported Windows Tauri shell.
- [ ] Keep a CPU-only runtime as the Windows correctness baseline. Startup, doctor, catalog,
      mixed-media search, general strong-shot ranking, Preferences evidence, Projects editing,
      and saved state work without a GPU or developer toolchain.
- [ ] Define typed platform services for reveal/open, application-data paths, process supervision,
      capability discovery, and bundle-resource lookup. Windows behavior does not emulate Finder
      commands or assume Unix path/process semantics.
- [ ] Preserve owner scoping, immutable originals, no-clobber publication, and append-only feedback
      semantics on Windows filesystems, including case-insensitive paths, locked files, long paths,
      non-UTF-8/error boundaries, and interrupted operations where applicable.
- [ ] Add a Windows MSVC CI lane for formatting, clippy, workspace tests, app compilation, the
      stateful UI harness, and CPU model contract tests. Linux/macOS lanes remain green.
- [ ] `doctor` reports OS/architecture, CPU baseline, decoder/media/model providers, sidecar/model
      hashes, application-data location, and actionable failures without exposing internal terms in
      the primary workflow.
- [ ] Document the supported Windows version/architecture baseline from tested evidence; do not
      claim Windows support from successful cross-compilation alone.
