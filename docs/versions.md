# Task 0 feasibility results

Run date: 2026-08-27

## Recommendation

**GO**, subject to the required human approval. The release spike passed all three risk checks on
Apple Silicon. CoreML executed the complete fixed-shape CLIP vision graph and averaged 5.86 ms per
image, comfortably below the 50 ms go/no-go threshold. Whisper used the Metal backend and
transcribed the 10-second fixture. The locally built LGPL FFmpeg sidecar launched successfully.

The main productization risk is CoreML's first-session compile time: 201,110.38 ms in this clean
spike run. Later implementation must cache the compiled CoreML model or build it ahead of use, and
must expose progress rather than making the application look hung.

## Test machine and toolchain

| Item | Exact value |
|---|---|
| macOS | 26.5.2 (25F84) |
| Architecture | arm64 |
| Chip | Apple M4 Pro |
| Memory | 25,769,803,776 bytes (24 GiB) |
| Rust | rustc 1.93.0 (254b59607 2026-01-19) |
| Cargo | cargo 1.93.0 (083ac5135 2025-12-15) |
| CMake | 4.4.3 |
| Apple developer tools | Command Line Tools at `/Library/Developer/CommandLineTools` |

No build environment variables were required. CMake was the only missing prerequisite and was
installed with Homebrew.

## Pinned spike dependencies

| Dependency | Version and feature |
|---|---|
| `anyhow` | 1.0.104 |
| `hound` | 3.5.1 |
| `ort` / `ort-sys` | 2.0.0-rc.13 with `coreml` |
| ONNX Runtime bundled by `ort-sys` | 1.28.0, arm64 macOS CoreML distribution |
| `whisper-rs` | 0.16.0 with `metal` |
| `whisper-rs-sys` | 0.15.0 |
| whisper.cpp reported at runtime | 1.8.3 |

The exact transitive graph is locked in `spike/Cargo.lock`.

## Inputs and provenance

| Asset | Provenance | SHA-256 |
|---|---|---|
| Dynamic CLIP vision ONNX | `Xenova/clip-vit-base-patch32`, revision `d15189d7028b43f1d3e65039190477f6af591c2a`, `onnx/vision_model.onnx` | `fd6e1402a588279d1723c7534d4bcba5bc0b14b47dfab0e46f8c47b8270d7d40` |
| Fixed-shape CLIP vision ONNX | Generated from the preceding file by `spike/fix_clip_shape.py` | `2483a4b8250460cf1e03f766fa0e839cf3784c9dfa8e0f7cdf1a8191cf3100e0` |
| Whisper `base.en` model | `ggerganov/whisper.cpp`, revision `5359861c739e955e79d9a303bcbc70fb988958b1`, `ggml-base.en.bin` | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| 10-second WAV fixture | `ggerganov/whisper.cpp`, revision `978113305b2ead22249b881deafa131dc8884911`, `samples/jfk.wav` | `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e` |

The ONNX preparation environment used Python packages `onnx==1.17.0`, `numpy==2.0.2`, and
`protobuf==6.33.6`. `spike/fix_clip_shape.py` fixes `pixel_values` to `[1, 3, 224, 224]`, reruns
strict shape inference, checks the model, and writes the derived file. Model files remain ignored;
the small WAV fixture is committed so the test input remains stable.

## Release benchmark

Command: `cargo run --release --manifest-path spike/Cargo.toml`

| Step | Result |
|---|---|
| FFmpeg `-version` spawn | 4.98 ms |
| CoreML session creation and compilation | 201,110.38 ms |
| CLIP warm run | completed before measurement |
| CLIP image encode, 10 measured runs | 58.65 ms total; **5.86 ms mean** |
| Whisper context/model initialization | 73.38 ms |
| Whisper inference on 10.0 s audio | **79.04 ms** |
| Whole spike | 201,425.14 ms; exit 0 |

The Whisper output was:

> And so my fellow Americans, ask not what your country can do for you, ask what you can do for
> your country.

## Acceleration evidence and warnings

- ONNX Runtime reported one CoreML partition containing all 685 graph nodes: 685/685 supported.
- The ONNX Runtime profile contained 22 `CoreMLExecutionProvider` entries and no
  `CPUExecutionProvider` entries.
- The CoreML compute-plan log assigned the model to Apple M4 Pro GPU compute, with some internal
  CoreML operations also eligible for CPU execution. This is CoreML's own scheduling, not an ONNX
  Runtime CPU-provider fallback.
- `whisper.cpp` reported GPU name `Apple M4 Pro` and `using Metal backend`.
- CoreML used the ML Program format, all compute units, static input shapes, and compute-plan
  profiling.
- The upstream dynamic-shape ONNX was not acceptable as-is: CoreML supported only 126–129 of 828
  nodes across 13 partitions and macOS emitted teardown shape exceptions. Fixing the image input
  shape and rerunning ONNX shape inference produced complete 685/685 coverage and removed that
  behavior.
- The current `ort` release candidate has a recoverable-builder error type that is not directly
  convertible to `anyhow::Error`; the spike converts those builder errors through their display
  text. Reassess this workaround when pinning the production dependency.
- The benchmark uses a zero-filled tensor to isolate runtime feasibility and provider performance;
  preprocessing correctness belongs to the later golden-test task.

FFmpeg build provenance, license configuration, binary hashes, and dynamic-system-library audit are
recorded in `sidecars/SOURCES.md`.
