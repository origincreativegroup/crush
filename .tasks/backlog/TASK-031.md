# TASK-031: Windows packaging and clean-machine parity acceptance

Depends: Tasks 023, 028, 029, and 030. Task 023 remains the Mac release milestone; this task reuses
its accepted user workflow and platform-manifest structure for Windows.

## Acceptance

- [ ] A tagged/manual release job produces a checksummed Windows installer for the declared
      architecture, signs it when release credentials are present, and labels unsigned development
      artifacts unmistakably. Exact installer technology is chosen from tested Tauri support.
- [ ] The installer bundles or securely bootstraps every required model, runtime, sidecar, license,
      and WebView requirement with pinned hashes. Normal installation requires no Rust, Visual
      Studio, Python, PyTorch, CUDA Toolkit, or separately installed FFmpeg.
- [ ] Installation, update, uninstall, application-data retention/removal, cache/model locations,
      backups, privacy, format support, GPU fallbacks, and troubleshooting are documented.
- [ ] Bundle verification rejects missing, wrong-architecture, unlicensed, or hash-mismatched
      sidecars/models and checks that the CPU baseline is usable before release publication.
- [ ] On a clean supported Windows machine with no developer tools, a tester can complete first
      run, index the declared photo/video fixtures, review with progressive filters, add creative
      Preferences evidence, create and boundary-preview a reel in Projects, render verified photo
      and video outputs, cancel/resume a job, relink moved media, and locate the manifests.
- [ ] The clean-machine path uses the same user language and playback/editor acceptance as Task
      023. It does not require knowledge of plans, recipes, context keys, execution providers, or
      style profiles.
- [ ] Test once with no compatible GPU to prove CPU/software fallbacks and separately on supported
      NVIDIA hardware to prove CUDA/NVENC use plus forced fallback. Optional DirectML evidence is
      recorded for every hardware class Crush advertises.
- [ ] Windows parity is approved only from the recorded clean-machine smoke and reviewed render
      evidence; green CI or a successful installer build alone is insufficient.
