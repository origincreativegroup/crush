#!/usr/bin/env python3
"""Export the pinned OpenAI CLIP ViT-B/32 encoders with fixed ONNX shapes."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort
import open_clip
import torch


ROOT = Path(__file__).resolve().parent.parent
OUTPUT_DIR = ROOT / "models"
MODEL_NAME = "ViT-B-32-quickgelu"
PRETRAINED = "openai"
OPSET = 17
IMAGE_SHAPE = (1, 3, 224, 224)
TEXT_SHAPE = (1, 77)
MEAN = [0.48145466, 0.4578275, 0.40821073]
STD = [0.26862954, 0.26130258, 0.27577711]


class ImageEncoder(torch.nn.Module):
    def __init__(self, model: torch.nn.Module) -> None:
        super().__init__()
        self.model = model

    def forward(self, pixels: torch.Tensor) -> torch.Tensor:
        features = self.model.encode_image(pixels)
        return features / features.norm(dim=-1, keepdim=True)


class TextEncoder(torch.nn.Module):
    def __init__(self, model: torch.nn.Module) -> None:
        super().__init__()
        self.model = model

    def forward(self, tokens: torch.Tensor) -> torch.Tensor:
        features = self.model.encode_text(tokens)
        return features / features.norm(dim=-1, keepdim=True)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def export_model(
    module: torch.nn.Module,
    sample: torch.Tensor,
    destination: Path,
    input_name: str,
) -> None:
    module.eval()
    with torch.inference_mode():
        torch.onnx.export(
            module,
            sample,
            destination,
            input_names=[input_name],
            output_names=["embedding"],
            opset_version=OPSET,
            dynamic_axes=None,
            do_constant_folding=True,
            dynamo=False,
        )
    exported = onnx.load(destination)
    onnx.checker.check_model(exported, full_check=True)


def verify_export(
    module: torch.nn.Module,
    sample: torch.Tensor,
    path: Path,
    input_name: str,
) -> float:
    with torch.inference_mode():
        expected = module(sample).detach().cpu().numpy()
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    actual = session.run(["embedding"], {input_name: sample.cpu().numpy()})[0]
    if actual.shape != (1, 512):
        raise RuntimeError(f"{path.name} returned unexpected shape {actual.shape}")
    maximum_error = float(np.max(np.abs(expected - actual)))
    if maximum_error > 1e-4:
        raise RuntimeError(
            f"{path.name} differs from PyTorch by {maximum_error}, above 1e-4"
        )
    return maximum_error


def package_version(name: str) -> str:
    return importlib.metadata.version(name)


def main() -> None:
    torch.manual_seed(0)
    torch.set_num_threads(1)
    # The optimized inference fast path lowers attention to
    # aten::_native_multi_head_attention, which the opset-17 exporter cannot encode.
    torch.backends.mha.set_fastpath_enabled(False)
    OUTPUT_DIR.mkdir(exist_ok=True)

    model, _, _ = open_clip.create_model_and_transforms(
        MODEL_NAME,
        pretrained=PRETRAINED,
        device="cpu",
    )
    model.eval()
    image_encoder = ImageEncoder(model)
    text_encoder = TextEncoder(model)
    image_sample = torch.zeros(IMAGE_SHAPE, dtype=torch.float32)
    text_sample = torch.zeros(TEXT_SHAPE, dtype=torch.int64)

    image_path = OUTPUT_DIR / "clip-image.onnx"
    text_path = OUTPUT_DIR / "clip-text.onnx"
    export_model(image_encoder, image_sample, image_path, "pixel_values")
    export_model(text_encoder, text_sample, text_path, "input_ids")

    import open_clip.tokenizer as tokenizer

    vocab_source = Path(tokenizer.default_bpe())
    vocab_path = OUTPUT_DIR / vocab_source.name
    vocab_path.write_bytes(vocab_source.read_bytes())

    verification = {
        "image_max_abs_error": verify_export(
            image_encoder, image_sample, image_path, "pixel_values"
        ),
        "text_max_abs_error": verify_export(
            text_encoder, text_sample, text_path, "input_ids"
        ),
    }
    files = [image_path, text_path, vocab_path]
    manifest = {
        "dim": 512,
        "files": {
            path.name: {"bytes": path.stat().st_size, "sha256": sha256(path)}
            for path in files
        },
        "model_name": "clip-vit-b-32-quickgelu-openai",
        "opset": OPSET,
        "preprocess": {
            "center_crop": 224,
            "interpolation": "bicubic",
            "mean": MEAN,
            "resize_shorter": 224,
            "std": STD,
        },
        "preprocess_version": 1,
        "toolchain": {
            "numpy": package_version("numpy"),
            "onnx": package_version("onnx"),
            "onnxruntime": package_version("onnxruntime"),
            "open_clip_torch": package_version("open_clip_torch"),
            "torch": package_version("torch"),
        },
        "verification": verification,
    }
    manifest_path = OUTPUT_DIR / "manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(manifest, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
