#!/usr/bin/env python3
"""Validate committed golden structure, dimensions, norms, hashes, and completeness."""

from __future__ import annotations

import json
from pathlib import Path

from generate_goldens import (
    CLIPS_DIR,
    DEFAULT_OUTPUT,
    MODEL_MANIFEST,
    QUERIES,
    duration_seconds,
    sha256,
    validate_image_golden,
    validate_text_golden,
)


def load_json(path: Path):
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def main() -> None:
    golden = DEFAULT_OUTPUT
    clips = sorted(
        path
        for path in CLIPS_DIR.iterdir()
        if path.is_file() and path.suffix.lower() in {".mp4", ".mov"}
    )
    if not 3 <= len(clips) <= 5:
        raise RuntimeError(f"expected 3–5 fixture clips, found {len(clips)}")

    copied_manifest = golden / "manifest.json"
    golden_model_manifest = load_json(copied_manifest)
    release_manifest = load_json(MODEL_MANIFEST)
    stable_fields = (
        "dim",
        "model_name",
        "opset",
        "preprocess",
        "preprocess_version",
        "toolchain",
    )
    if any(
        golden_model_manifest.get(field) != release_manifest.get(field)
        for field in stable_fields
    ):
        raise RuntimeError("golden and release manifests differ on the CLIP contract")
    golden_files = golden_model_manifest.get("files", {})
    if any(
        release_manifest.get("files", {}).get(name) != metadata
        for name, metadata in golden_files.items()
    ):
        raise RuntimeError("golden and release manifests differ on CLIP asset hashes")
    fixture_manifest = load_json(golden / "fixtures-manifest.json")
    if fixture_manifest.get("queries") != QUERIES:
        raise RuntimeError("fixture manifest queries differ from the generator")

    expected_names = {"manifest.json", "fixtures-manifest.json"}
    for clip in clips:
        stem = clip.stem
        names = {
            f"{stem}.frame.ppm",
            f"{stem}.image.json",
            f"{stem}.scenes.json",
            f"{stem}.transcript.json",
        }
        expected_names.update(names)
        missing = [name for name in names if not (golden / name).is_file()]
        if missing:
            raise RuntimeError(f"{clip.name} is missing goldens: {missing}")

        image = load_json(golden / f"{stem}.image.json")
        validate_image_golden(image, f"{stem}.image.json")
        scenes = load_json(golden / f"{stem}.scenes.json")
        if not scenes.get("shots"):
            raise RuntimeError(f"{stem}.scenes.json has no shots")
        transcript = load_json(golden / f"{stem}.transcript.json")
        if "segments" not in transcript:
            raise RuntimeError(f"{stem}.transcript.json has no segments field")

        recorded = fixture_manifest["clips"].get(clip.name)
        if recorded is None:
            raise RuntimeError(f"fixture manifest omits {clip.name}")
        if recorded["sha256"] != sha256(clip):
            raise RuntimeError(f"fixture hash changed for {clip.name}")
        if abs(recorded["duration_s"] - duration_seconds(clip)) > 1e-6:
            raise RuntimeError(f"fixture duration changed for {clip.name}")

    for index, query in enumerate(QUERIES, start=1):
        name = f"text{index}.json"
        expected_names.add(name)
        text = load_json(golden / name)
        if text.get("input") != query:
            raise RuntimeError(f"{name} query differs from the generator")
        validate_text_golden(text, name)

    actual_names = {path.name for path in golden.iterdir() if path.is_file()}
    if actual_names != expected_names:
        raise RuntimeError(
            f"golden file set differs: missing={expected_names - actual_names}, "
            f"unexpected={actual_names - expected_names}"
        )
    print(f"verified {len(clips)} fixture sets and {len(QUERIES)} text goldens")


if __name__ == "__main__":
    main()
