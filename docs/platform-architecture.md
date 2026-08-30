# Cross-platform architecture roadmap

Status: planned extension to the existing Crush engineering and DAM blueprints.

This roadmap does not replace `docs/project-blueprint.md`,
`docs/dam-feedback-blueprint.md`, or the ordered Tasks 021–023. It makes the platform seams
explicit while those product milestones are implemented, then adds a Windows delivery track.
Crush remains a local-first photo/video DAM and editorial-intelligence product whose primary goal
is strong-shot recognition, explainable personal preference learning, and non-destructive media
creation. Portability is an architectural property of that product, not a new product direction.

## Product and runtime invariants

1. The production core remains Rust. Cataloging, provenance, ranking, feedback, planning, recipe
   validation, job state, verification, and publication cannot depend on Python or a GPU.
2. A CPU path is the correctness baseline on every supported machine. Hardware acceleration may
   improve throughput, but it cannot change the meaning of a result or make a workflow unavailable.
3. PyTorch belongs to model research, training, evaluation, and export. The shipped application
   consumes versioned, validated ONNX artifacts; it does not require a Python environment or a
   PyTorch runtime.
4. General strong-shot assessment remains available without personal examples. Preferences and
   previous-work evidence adapt the general ranking without becoming the sole recognition system.
5. Originals are immutable. Platform-specific decoders and encoders must satisfy the same recipe,
   provenance, no-clobber, cancellation, and verification contracts.
6. Capability reporting is truthful. Missing codecs, unsupported RAW variants, unavailable GPUs,
   or failed provider initialization produce explicit evidence and a safe fallback where one is
   defined; they never silently claim full-quality support.
7. User concepts stay consistent across platforms: Library, Review, Search, Preferences, and
   Projects. Backend/provider names belong in diagnostics, not in the primary creative workflow.

## Layered design

```text
Tauri shell and web UI
        |
Rust application commands and owner-scoped services
        |
DAM, feedback, ranking, project, recipe, job, and manifest contracts
        |
+----------------------+-----------------------+----------------------+
| source decode/probe  | model execution       | media render/process |
| common + OS adapters | ONNX + ASR providers  | ffmpeg + OS adapters |
+----------------------+-----------------------+----------------------+
        |
SQLite, model files, sidecars, cache, staging, and immutable originals
```

The top three layers are platform-neutral and have deterministic tests. Platform modules implement
narrow capability interfaces and return typed evidence. They do not own editorial policy, decide
what is “good,” or write directly around the store/job contracts.

## Portable capability contracts

### Source decoding

Define a source decoder interface with four responsibilities:

- probe a source and report a versioned capability result;
- perform a full-resolution decode into a canonical still-image representation;
- return orientation, dimensions, bit depth, color/profile evidence, and decoder provenance; and
- distinguish a full decode from an embedded preview or working proxy.

The common JPEG/PNG/TIFF path remains the pinned Rust `image` implementation where its fidelity
contract is met. macOS keeps an ImageIO adapter for HEIC/HEIF and supported camera RAW. Windows gets
a separately tested full-decode adapter selected after format, color-management, redistribution,
and licensing evaluation. WIC, LibRaw, libheif, or another candidate is an implementation choice,
not an assumed promise. Each accepted extension must pass real-file fixtures before support is
advertised. Downstream analysis and rendering consume the canonical representation, not an OS API.

### Media probing and rendering

Separate these concerns:

- `MediaProbe`: streams, codecs, duration, time base, dimensions, rotation, audio, and color/HDR;
- `RenderBackend`: validated recipe to a private staged output plus actual execution evidence;
- `ProcessSupervisor`: start, progress, cancellation, timeout, and process-tree cleanup;
- `Publisher`: exclusive no-clobber publication, checksum, recovery, and manifest finalization.

FFmpeg/FFprobe remain bundled, pinned sidecars with recorded hashes and licenses. Recipes describe
the intended result; encoder selection is a capability decision recorded in the output manifest.

| Platform | Preferred video path | Required fallback |
|---|---|---|
| macOS | VideoToolbox when the preset and source permit it | approved bundled software encoder |
| Windows + supported NVIDIA GPU | NVENC when runtime capability checks and verification pass | approved bundled software encoder |
| Other Windows hardware | optional platform acceleration when later approved | approved bundled software encoder |

The fallback codec/build must be explicitly selected and license-reviewed in Task 029; this
roadmap does not silently add GPL components to the current LGPL FFmpeg distribution. Accelerated
and software outputs need not be byte-identical, but they must pass the same preset-specific frame,
duration, audio, dimensions, orientation, and color tolerances. Unix process groups and Windows
Job Objects are implementation details behind `ProcessSupervisor`.

### Model execution and training

The production provider policy is ordered and observable:

| Workload/platform | Preferred provider | Fallback |
|---|---|---|
| visual ONNX on Apple hardware | ONNX Runtime CoreML execution provider | ONNX Runtime CPU |
| ASR on Apple Silicon | Metal-enabled Whisper backend | CPU Whisper backend |
| visual ONNX on Windows + NVIDIA | ONNX Runtime CUDA execution provider | DirectML when enabled, then CPU |
| visual ONNX on other supported Windows GPUs | optional ONNX Runtime DirectML provider | ONNX Runtime CPU |
| all unsupported or failed accelerators | CPU | explicit failure only if CPU also fails |

Provider initialization is runtime capability detection, not GPU-brand inference alone. A failed
accelerator is demoted for that session with a diagnostic; the job retries on the next valid
provider without changing model identity. Manifests and benchmark records include provider,
runtime version, device evidence, model hash, preprocessing contract, and fallback reason.

PyTorch may use CUDA on NVIDIA development machines and MPS/Metal on Apple development machines
for training and experiments. Every releasable model is exported to ONNX and checked against a
fixed PyTorch reference corpus. Acceptance records numerical tolerances, held-out task metrics,
pre/post-processing contract hashes, and CPU/provider parity. Personalized learning remains the
small, auditable Rust-side adaptor unless a future model task proves a replacement; installing
Crush must never install Python, PyTorch, CUDA Toolkit, or compiler tools.

## Seams introduced during the current milestones

Tasks 021–023 remain the active ordered product path and their human stops remain unchanged. The
following seams are requirements on their implementation, not a request to expand their current
acceptance into Windows release acceptance.

### Task 021 — non-destructive render/export

- Store recipes, frozen source snapshots, render jobs, and manifests without macOS-only fields.
- Route source decode, media probe, render execution, cancellation, and publication through narrow
  capability boundaries even if the first concrete adapters are macOS implementations.
- Keep encoder names out of user presets. Record the selected backend and exact command in the
  manifest after capability negotiation.
- Make golden assertions about declared output properties and tolerances rather than an incidental
  VideoToolbox byte stream.
- Preserve the render-golden human hard stop. Windows automation cannot substitute for visual and
  color review of the first accepted Mac output.

### Task 022 — Reel Studio importer

- Keep imported paths, separators, case behavior, and source identities platform-neutral.
- Store recipes and project evidence in the same versioned schema used by Task 021, without
  embedding Apple-only decoder or encoder choices.
- Normalize for comparison without rewriting the user's original path. Missing-file relink uses
  stable content identity and owner scope rather than string equality alone.
- An imported finished project still becomes preference evidence only through explicit user
  choice; cross-platform work does not weaken that DAM rule.

### Task 023 — Mac release packaging and acceptance

- Keep Task 023 as the macOS release/clean-machine milestone, including its human acceptance.
- Make bundle assembly consume a platform manifest listing sidecars, models, hashes, licenses,
  runtime capabilities, and required smoke cases. Task 031 will provide the Windows manifest.
- Split shell actions such as reveal/open, application data locations, and diagnostics behind
  platform services rather than adding more direct Finder commands.
- Keep UI harness scenarios platform-neutral and user-facing. The same Library → Review →
  Preferences → Projects → preview → render path becomes the Windows parity script later.

## Windows delivery track

The Windows track is additive and follows the same DAM/editorial contracts:

1. **Task 028 — portability seams, Windows shell, and CI.** Compile and run the Rust/Tauri product
   on Windows with a CPU-only capability baseline and platform-neutral shell behavior.
2. **Task 029 — Windows source and media backends.** Ship verified sidecars, full-decode support for
   the declared still formats, portable software rendering, optional NVENC, process-tree
   cancellation, and the shared media-golden matrix.
3. **Task 030 — accelerated inference and model-export discipline.** Add optional CUDA and DirectML
   ONNX providers without weakening CPU/CoreML/Metal behavior, and automate PyTorch-to-ONNX parity
   evidence for new model artifacts.
4. **Task 031 — Windows packaging and clean-machine parity.** Produce a signed/checksummed Windows
   installer when credentials are available, then run the complete user workflow on a clean
   supported Windows machine with no developer tools.

Tasks 028–030 may be developed in parallel only after their shared contracts are stable and each
branch keeps the CPU test lane green. Task 031 depends on all three and on the accepted product
workflow from Tasks 021–023. Passing Windows CI is not clean-machine acceptance.

## Cross-platform verification policy

- Linux remains a fast, CPU-only portability and core-correctness lane; it is not a substitute for
  either desktop platform's native tests.
- macOS CI covers CPU plus available CoreML/Metal capability tests and Mac bundle verification.
- Windows CI covers MSVC builds, CPU tests, Tauri shell/UI harness, sidecar verification, path and
  cancellation behavior, and software-render goldens. GPU jobs are supplemental because hosted
  runners do not establish clean-machine hardware behavior.
- Dedicated hardware records cover Apple acceleration, NVIDIA CUDA/NVENC, optional DirectML, and
  fallback after forced provider/encoder failure.
- Provider-specific numerical or encoded-byte differences use documented tolerances. Editorial
  identity, source hashes, recipe snapshots, provenance, manifest completeness, and no-clobber
  behavior remain exact.

## Definition of platform parity

Windows parity is reached only when a clean supported Windows installation can index the declared
photo/video format matrix, review and provide preference evidence, create and boundary-preview a
project, render verified photo/video outputs, cancel and resume work, relink moved sources, and
explain which CPU/GPU/media capabilities were actually used. It must do so without Python, a CUDA
Toolkit, Visual Studio, Rust, or separately installed FFmpeg. Optional acceleration being absent is
not a failure; an unusable CPU fallback is.
