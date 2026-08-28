#!/usr/bin/env python3
"""Regenerate every committed answer-key artifact from the pinned fixtures and models."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import av

from reference_embed import ClipReference
from reference_transcribe import has_audio, load_model, transcribe_clip


ROOT = Path(__file__).resolve().parent.parent
CLIPS_DIR = ROOT / "fixtures" / "clips"
DEFAULT_OUTPUT = ROOT / "fixtures" / "golden"
MODEL_MANIFEST = ROOT / "models" / "manifest.json"
MODEL_NAME = "small"
QUERIES = [
    "a rocket launching into the sky",
    "the Earth seen from space",
    "the Moon's cratered surface",
    "a colorful television test pattern",
    "bright engine flames",
]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def duration_seconds(path: Path) -> float:
    with av.open(str(path)) as container:
        if container.duration is None:
            raise RuntimeError(f"fixture has no duration: {path}")
        return float(container.duration / av.time_base)


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(
            value,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        + "\n",
        encoding="utf-8",
    )


def find_ffmpeg() -> str:
    bundled = ROOT / "sidecars" / "ffmpeg"
    if bundled.is_file():
        return str(bundled)
    executable = shutil.which("ffmpeg")
    if executable:
        return executable
    raise RuntimeError("ffmpeg is missing; place the pinned binary at sidecars/ffmpeg")


def extract_frame(ffmpeg: str, clip: Path, at_seconds: float, destination: Path) -> None:
    subprocess.run(
        [
            ffmpeg,
            "-y",
            "-loglevel",
            "error",
            "-ss",
            f"{at_seconds:.6f}",
            "-i",
            str(clip),
            "-frames:v",
            "1",
            "-threads",
            "1",
            str(destination),
        ],
        check=True,
    )


def detect_scenes(clip: Path) -> dict[str, Any]:
    """Run OpenCV scene detection separately from PyAV to avoid dylib collisions on macOS."""
    completed = subprocess.run(
        [sys.executable, str(ROOT / "reference" / "reference_scenes.py"), str(clip)],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


def validate_image_golden(value: dict[str, Any], name: str) -> None:
    if value.get("tensor_shape") != [1, 3, 224, 224]:
        raise RuntimeError(f"{name}: invalid tensor shape")
    if len(value.get("tensor", [])) != 150_528:
        raise RuntimeError(f"{name}: tensor does not contain 150528 floats")
    validate_embedding(value.get("embedding", []), name)


def validate_text_golden(value: dict[str, Any], name: str) -> None:
    if len(value.get("token_ids", [])) != 77:
        raise RuntimeError(f"{name}: token_ids does not contain 77 integers")
    validate_embedding(value.get("embedding", []), name)


def validate_embedding(values: list[float], name: str) -> None:
    if len(values) != 512:
        raise RuntimeError(f"{name}: embedding does not contain 512 floats")
    if not all(math.isfinite(value) for value in values):
        raise RuntimeError(f"{name}: embedding contains a non-finite value")
    norm = math.sqrt(sum(value * value for value in values))
    if abs(norm - 1.0) > 1e-5:
        raise RuntimeError(f"{name}: embedding norm {norm} is outside 1 ± 1e-5")


def generate(output_dir: Path) -> None:
    clips = sorted(
        path
        for path in CLIPS_DIR.iterdir()
        if path.is_file() and path.suffix.lower() in {".mp4", ".mov"}
    )
    if not 3 <= len(clips) <= 5:
        raise RuntimeError(f"expected 3–5 fixture clips, found {len(clips)}")
    total_bytes = sum(path.stat().st_size for path in clips)
    if total_bytes > 20 * 1024 * 1024:
        raise RuntimeError(f"fixture clips total {total_bytes} bytes, above 20 MiB")
    durations = {clip: duration_seconds(clip) for clip in clips}
    for clip, duration in durations.items():
        if duration > 30.05:
            raise RuntimeError(f"{clip.name} is {duration:.3f}s, above 30s")
    if not MODEL_MANIFEST.is_file():
        raise RuntimeError("models/manifest.json is missing; run make models first")

    output_dir.parent.mkdir(parents=True, exist_ok=True)
    ffmpeg = find_ffmpeg()
    clip_reference = ClipReference()
    whisper_model = (
        load_model(MODEL_NAME) if any(has_audio(clip) for clip in clips) else None
    )

    with tempfile.TemporaryDirectory(
        prefix="crush-golden-", dir=output_dir.parent
    ) as temporary:
        staging = Path(temporary)
        fixture_manifest: dict[str, Any] = {
            "clips": {},
            "queries": QUERIES,
            "whisper_model": MODEL_NAME,
        }
        for clip in clips:
            stem = clip.stem
            duration = durations[clip]
            frame_seconds = duration * 0.4
            # PPM is lossless and is available in the deliberately minimal bundled FFmpeg.
            frame_name = f"{stem}.frame.ppm"
            frame_path = staging / frame_name
            extract_frame(ffmpeg, clip, frame_seconds, frame_path)

            scenes = detect_scenes(clip)
            transcript = transcribe_clip(
                clip,
                model_name=MODEL_NAME,
                model=whisper_model,
            )
            image = clip_reference.image(frame_path, dump_tensor=True)
            image["input"] = f"fixtures/golden/{frame_name}"
            validate_image_golden(image, f"{stem}.image.json")

            write_json(staging / f"{stem}.scenes.json", scenes)
            write_json(staging / f"{stem}.transcript.json", transcript)
            write_json(staging / f"{stem}.image.json", image)
            fixture_manifest["clips"][clip.name] = {
                "bytes": clip.stat().st_size,
                "duration_s": round(duration, 6),
                "frame_s": round(frame_seconds, 6),
                "has_audio": has_audio(clip),
                "sha256": sha256(clip),
            }

        for index, query in enumerate(QUERIES, start=1):
            text = clip_reference.text(query)
            validate_text_golden(text, f"text{index}.json")
            write_json(staging / f"text{index}.json", text)

        shutil.copyfile(MODEL_MANIFEST, staging / "manifest.json")
        write_json(staging / "fixtures-manifest.json", fixture_manifest)
        # Search expectations are human-reviewed after the first Rust fixture run. Preserve that
        # approved contract when regenerating model-derived goldens; never synthesize or overwrite it.
        expected_search = output_dir / "expected_search.json"
        if expected_search.is_file():
            shutil.copyfile(expected_search, staging / expected_search.name)

        output_dir.mkdir(parents=True, exist_ok=True)
        staged_names = {path.name for path in staging.iterdir()}
        for existing in output_dir.iterdir():
            if existing.is_file() and existing.name not in staged_names:
                existing.unlink()
        for generated in staging.iterdir():
            generated.replace(output_dir / generated.name)

    print(
        f"generated {len(clips)} fixture sets and {len(QUERIES)} text goldens "
        f"in {output_dir.relative_to(ROOT)}"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=DEFAULT_OUTPUT)
    return parser.parse_args()


if __name__ == "__main__":
    generate(parse_args().output_dir.resolve())
