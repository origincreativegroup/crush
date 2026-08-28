#!/usr/bin/env python3
"""Generate the authoritative CLIP preprocessing, token, and embedding outputs."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import numpy as np
import onnxruntime as ort
import open_clip
from PIL import Image


ROOT = Path(__file__).resolve().parent.parent
MODELS = ROOT / "models"
MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)


def repository_path(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return resolved.as_posix()


def preprocess(path: Path) -> np.ndarray:
    """Resize shorter side to 224 bicubic, center-crop, RGB, /255, normalize, CHW."""
    with Image.open(path) as source:
        image = source.convert("RGB")
    width, height = image.size
    scale = 224 / min(width, height)
    resized = (
        max(224, round(width * scale)),
        max(224, round(height * scale)),
    )
    image = image.resize(resized, Image.Resampling.BICUBIC)
    width, height = image.size
    left = (width - 224) // 2
    top = (height - 224) // 2
    image = image.crop((left, top, left + 224, top + 224))
    pixels = np.asarray(image, dtype=np.float32) / np.float32(255.0)
    normalized = (pixels - MEAN) / STD
    return np.transpose(normalized, (2, 0, 1))[None].astype(np.float32, copy=False)


class ClipReference:
    def __init__(self) -> None:
        options = ort.SessionOptions()
        options.intra_op_num_threads = 1
        options.inter_op_num_threads = 1
        self.image_session = ort.InferenceSession(
            str(MODELS / "clip-image.onnx"),
            sess_options=options,
            providers=["CPUExecutionProvider"],
        )
        self.text_session = ort.InferenceSession(
            str(MODELS / "clip-text.onnx"),
            sess_options=options,
            providers=["CPUExecutionProvider"],
        )
        self.tokenizer = open_clip.get_tokenizer("ViT-B-32")

    def image(self, path: Path, dump_tensor: bool = True) -> dict[str, Any]:
        tensor = preprocess(path)
        embedding = self.image_session.run(
            ["embedding"], {"pixel_values": tensor}
        )[0][0]
        output: dict[str, Any] = {
            "embedding": embedding.tolist(),
            "input": repository_path(path),
            "kind": "image",
            "tensor_shape": list(tensor.shape),
        }
        if dump_tensor:
            output["tensor"] = tensor.reshape(-1).tolist()
        validate_embedding(embedding)
        return output

    def text(self, text: str) -> dict[str, Any]:
        token_ids = self.tokenizer([text]).numpy().astype(np.int64, copy=False)
        if token_ids.shape != (1, 77):
            raise RuntimeError(f"tokenizer returned unexpected shape {token_ids.shape}")
        embedding = self.text_session.run(
            ["embedding"], {"input_ids": token_ids}
        )[0][0]
        validate_embedding(embedding)
        return {
            "embedding": embedding.tolist(),
            "input": text,
            "kind": "text",
            "token_ids": token_ids[0].tolist(),
        }


def validate_embedding(embedding: np.ndarray) -> None:
    if embedding.shape != (512,):
        raise RuntimeError(f"embedding has unexpected shape {embedding.shape}")
    if not np.isfinite(embedding).all():
        raise RuntimeError("embedding contains non-finite values")
    norm = float(np.linalg.norm(embedding))
    if abs(norm - 1.0) > 1e-5:
        raise RuntimeError(f"embedding L2 norm is {norm}, expected 1 ± 1e-5")


def write_json(value: dict[str, Any]) -> None:
    print(json.dumps(value, sort_keys=True, separators=(",", ":")))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="kind", required=True)
    image = subparsers.add_parser("image")
    image.add_argument("path", type=Path)
    image.add_argument("--dump-tensor", action="store_true")
    text = subparsers.add_parser("text")
    text.add_argument("query")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    reference = ClipReference()
    if args.kind == "image":
        write_json(reference.image(args.path, dump_tensor=args.dump_tensor))
    else:
        write_json(reference.text(args.query))


if __name__ == "__main__":
    main()
