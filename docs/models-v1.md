# Models v1 release record

Run date: 2026-08-27

The `models-v1` release contains the fixed-shape OpenAI CLIP ViT-B/32 QuickGELU image and text
encoders, their BPE vocabulary, and the multilingual Whisper `base` and `small` ggml models. The
tracked manifest in `crates/core/model-manifest-v1.json` is the release source of truth. The five
model assets total 1,242,702,823 bytes; every individual asset remains below GitHub's 2 GiB limit.

## Reference verification

The image export was compared with PyTorch using `fixtures/golden/synthetic-speech.frame.ppm`, not a
synthetic zero tensor. CPU ONNX Runtime produced cosine `1.0` with maximum absolute error
`5.541369318962097e-07`. The text export produced cosine `1.0000001192092896` with maximum absolute
error `1.7881393432617188e-07`. Both exceed the required cosine of `0.9999`.

ONNX Runtime successfully created CoreML sessions and returned `[1, 512]` from both encoders. Its
partition diagnostics reported:

- image encoder: 466 of 467 graph nodes supported by CoreML, across two CoreML partitions;
- text encoder: 476 of 478 graph nodes supported by CoreML, across three CoreML partitions.

The remaining one image node and two text nodes fall back to the CPU execution provider. This is a
small but explicit difference from the Task 0 upstream vision-model spike, which had full CoreML
coverage after shape fixing.

## Release assets

| Asset | Bytes | SHA-256 |
|---|---:|---|
| `clip-image.onnx` | 351,605,911 | `9a4c3b87ec5c78e3951b6e7e041981e55468efe8978ec5411b00510ae8726d01` |
| `clip-text.onnx` | 254,186,563 | `cab3f1bb08bcb3a46e033c69b43015ba49c1308a3b7ff24bce61e08a41e476d8` |
| `bpe_simple_vocab_16e6.txt.gz` | 1,356,917 | `924691ac288e54409236115652ad4aa250f48203de50a9e4722a6ecd48d6804a` |
| `ggml-base.bin` | 147,951,465 | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` |
| `ggml-small.bin` | 487,601,967 | `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b` |

The Whisper weights came from `ggerganov/whisper.cpp` at revision
`5359861c739e955e79d9a303bcbc70fb988958b1`. The combined embedding contract identifier is
`c29c784ed9a2ee4de5e5856b6551b7fbe40e9b25673788eb4e58f538074acb3e`; it hashes the names and
SHA-256 values of the two CLIP encoders and vocabulary in sorted order.
