#!/usr/bin/env python3
"""Export CLIP ViT-B/32 image + text encoders to ONNX for the Rust `ort` runtime.
Fixed shapes, opset 17, fp32. Also copies the BPE vocab. Prints sha256 for the manifest.
Run once; outputs go to ../models/. Untested here — run on the Mac in reference/.venv."""
import hashlib, json, sys, pathlib
import torch, open_clip

OUT = pathlib.Path(__file__).resolve().parent.parent / "models"
OUT.mkdir(exist_ok=True)
MODEL, PRETRAINED = "ViT-B-32", "openai"

model, _, _ = open_clip.create_model_and_transforms(MODEL, pretrained=PRETRAINED)
model.eval()

class ImageEnc(torch.nn.Module):
    def __init__(self, m): super().__init__(); self.m = m
    def forward(self, x):
        f = self.m.encode_image(x)
        return f / f.norm(dim=-1, keepdim=True)

class TextEnc(torch.nn.Module):
    def __init__(self, m): super().__init__(); self.m = m
    def forward(self, tokens):
        f = self.m.encode_text(tokens)
        return f / f.norm(dim=-1, keepdim=True)

img_path, txt_path = OUT / "clip-image.onnx", OUT / "clip-text.onnx"
torch.onnx.export(ImageEnc(model), torch.zeros(1, 3, 224, 224), img_path,
                  input_names=["pixel_values"], output_names=["embedding"], opset_version=17, dynamic_axes=None)
torch.onnx.export(TextEnc(model), torch.zeros(1, 77, dtype=torch.int64), txt_path,
                  input_names=["input_ids"], output_names=["embedding"], opset_version=17, dynamic_axes=None)

# vocab: open_clip ships bpe_simple_vocab_16e6.txt.gz
import open_clip.tokenizer as tk
vocab_src = pathlib.Path(tk.default_bpe())
vocab_dst = OUT / vocab_src.name
vocab_dst.write_bytes(vocab_src.read_bytes())

def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
manifest = {
    "model_name": "clip-vit-b-32-openai", "dim": 512, "preprocess_version": 1, "opset": 17,
    "files": {p.name: {"sha256": sha(p), "bytes": p.stat().st_size} for p in [img_path, txt_path, vocab_dst]},
    "preprocess": {"resize_shorter": 224, "center_crop": 224, "interpolation": "bicubic",
                    "mean": [0.48145466, 0.4578275, 0.40821073], "std": [0.26862954, 0.26130258, 0.27577711]},
}
(OUT / "manifest.json").write_text(json.dumps(manifest, indent=2))
print(json.dumps(manifest, indent=2))
