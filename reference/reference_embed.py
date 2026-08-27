#!/usr/bin/env python3
"""Answer key for the Rust embed stage.
  reference_embed.py image <path> [--dump-tensor]   -> golden JSON on stdout
  reference_embed.py text "<query>"                 -> golden JSON on stdout
Uses the SAME ONNX files the Rust code will use, via onnxruntime (CPU), so the only thing
this checks is Rust's preprocessing + tokenizer + session plumbing — not the model."""
import sys, json, pathlib, numpy as np
from PIL import Image
import onnxruntime as ort
import open_clip

MODELS = pathlib.Path(__file__).resolve().parent.parent / "models"
MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)

def preprocess(path):
    """Mirror of what Rust must do. Resize shorter side to 224 (bicubic), center crop 224, RGB, /255, normalize, CHW."""
    im = Image.open(path).convert("RGB")
    w, h = im.size
    s = 224 / min(w, h)
    im = im.resize((max(224, round(w * s)), max(224, round(h * s))), Image.BICUBIC)
    w, h = im.size
    l, t = (w - 224) // 2, (h - 224) // 2
    im = im.crop((l, t, l + 224, t + 224))
    x = np.asarray(im, dtype=np.float32) / 255.0
    x = (x - MEAN) / STD
    return np.transpose(x, (2, 0, 1))[None]  # 1,3,224,224

def main():
    kind = sys.argv[1]
    if kind == "image":
        x = preprocess(sys.argv[2])
        sess = ort.InferenceSession(str(MODELS / "clip-image.onnx"), providers=["CPUExecutionProvider"])
        emb = sess.run(None, {"pixel_values": x})[0][0]
        out = {"kind": "image", "input": sys.argv[2], "embedding": emb.tolist()}
        if "--dump-tensor" in sys.argv:
            out["tensor_shape"] = list(x.shape)
            out["tensor"] = x.flatten().tolist()
    else:
        tok = open_clip.get_tokenizer("ViT-B-32")
        ids = tok([sys.argv[2]]).numpy().astype(np.int64)
        sess = ort.InferenceSession(str(MODELS / "clip-text.onnx"), providers=["CPUExecutionProvider"])
        emb = sess.run(None, {"input_ids": ids})[0][0]
        out = {"kind": "text", "input": sys.argv[2], "token_ids": ids[0].tolist(), "embedding": emb.tolist()}
    print(json.dumps(out))

if __name__ == "__main__":
    main()
