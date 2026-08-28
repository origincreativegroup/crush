#!/usr/bin/env python3
"""Generate authoritative faster-whisper segments for one fixture clip."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import Any

import av
from faster_whisper import WhisperModel


ROOT = Path(__file__).resolve().parent.parent
MODEL_CACHE = ROOT / "models" / "faster-whisper"


def repository_path(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return resolved.as_posix()


def has_audio(clip: Path) -> bool:
    with av.open(str(clip)) as container:
        return bool(container.streams.audio)


def load_model(model_name: str) -> WhisperModel:
    return WhisperModel(
        model_name,
        device="cpu",
        compute_type="int8",
        cpu_threads=max(1, (os.cpu_count() or 4) - 2),
        num_workers=1,
        download_root=str(MODEL_CACHE),
    )


def transcribe_clip(
    clip: Path,
    model_name: str = "small",
    model: WhisperModel | None = None,
) -> dict[str, Any]:
    if not has_audio(clip):
        return {
            "clip": repository_path(clip),
            "language": None,
            "model": model_name,
            "segments": [],
        }

    active_model = model or load_model(model_name)
    segments, info = active_model.transcribe(
        str(clip),
        language="en",
        beam_size=5,
        temperature=0.0,
        vad_filter=True,
        condition_on_previous_text=False,
        word_timestamps=False,
    )
    return {
        "clip": repository_path(clip),
        "language": info.language,
        "model": model_name,
        "segments": [
            {
                "end_s": round(segment.end, 6),
                "start_s": round(segment.start, 6),
                "text": segment.text.strip(),
            }
            for segment in segments
        ],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("clip", type=Path)
    parser.add_argument("--model", default="small")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    result = transcribe_clip(args.clip, model_name=args.model)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
