#!/usr/bin/env python3
"""Make the downloaded CLIP vision model's image input shape static for CoreML."""

from pathlib import Path

import onnx


ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "models" / "clip-vision-vit-b-32.onnx"
DESTINATION = ROOT / "models" / "clip-vision-vit-b-32-fixed.onnx"
PIXEL_SHAPE = (1, 3, 224, 224)


def main() -> None:
    model = onnx.load(SOURCE)
    pixel_values = next(value for value in model.graph.input if value.name == "pixel_values")
    dimensions = pixel_values.type.tensor_type.shape.dim
    if len(dimensions) != len(PIXEL_SHAPE):
        raise ValueError(f"unexpected pixel_values rank: {len(dimensions)}")

    for dimension, size in zip(dimensions, PIXEL_SHAPE):
        dimension.ClearField("dim_param")
        dimension.dim_value = size

    fixed = onnx.shape_inference.infer_shapes(model, strict_mode=True, data_prop=True)
    onnx.checker.check_model(fixed)
    onnx.save(fixed, DESTINATION)
    print(f"wrote {DESTINATION} with pixel_values shape {PIXEL_SHAPE}")


if __name__ == "__main__":
    main()
