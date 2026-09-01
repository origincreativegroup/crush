# TASK-030: Portable model runtime and optional Windows acceleration

Depends: Task 028 and the existing model/held-out evaluation contracts. Optional acceleration must
not block CPU functionality or weaken the Task 018 human style-proof gate.

## Acceptance

- [ ] Keep ONNX Runtime CPU as the production correctness baseline on macOS and Windows. The
      installed product requires neither Python/PyTorch nor a CUDA Toolkit.
- [ ] Preserve ONNX Runtime CoreML visual inference and Metal-enabled Whisper on supported Apple
      hardware, each with a tested CPU fallback and explicit provider evidence.
- [ ] Add optional ONNX Runtime CUDA inference for supported NVIDIA Windows systems. Detect the
      runtime/device capability, verify provider initialization, and demote safely when unavailable
      or unhealthy instead of failing the user workflow.
- [ ] Add optional ONNX Runtime DirectML as a Windows acceleration path where its supported model
      and operator matrix passes validation; CPU remains the final fallback. Provider ordering and
      the reason for each fallback are deterministic and inspectable.
- [ ] Record provider, device evidence, runtime version, model/preprocessing contract hashes,
      timings, and fallback reason in diagnostics and relevant job evidence without changing model
      identity or presenting GPU availability as learned quality.
- [ ] Define PyTorch as a development/training/export dependency only. New model artifacts are
      exported to versioned ONNX and compared against fixed PyTorch reference inputs, numerical
      tolerances, held-out task metrics, and preprocessing/postprocessing contract hashes before
      they can enter the model manifest.
- [ ] Run CPU parity in ordinary CI and record dedicated Apple CoreML/Metal, NVIDIA CUDA, and
      supported DirectML hardware results. Forced provider failures prove fallback behavior.
- [ ] Benchmark representative photo/video batches and publish honest memory, throughput, and
      thermal observations; acceleration is accepted only when results stay within quality and
      reproducibility tolerances.
