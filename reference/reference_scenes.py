#!/usr/bin/env python3
"""Generate authoritative content-detector shot boundaries for one fixture clip."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from scenedetect import ContentDetector, SceneManager, open_video


ROOT = Path(__file__).resolve().parent.parent


def repository_path(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return resolved.as_posix()


def detect_scenes(
    clip: Path,
    threshold: float = 27.0,
    min_scene_len_s: float = 0.6,
) -> dict[str, Any]:
    video = open_video(str(clip))
    minimum_frames = max(1, round(min_scene_len_s * video.frame_rate))
    manager = SceneManager()
    manager.add_detector(
        ContentDetector(threshold=threshold, min_scene_len=minimum_frames)
    )
    manager.detect_scenes(video, show_progress=False)
    scenes = manager.get_scene_list(start_in_scene=True)
    return {
        "clip": repository_path(clip),
        "min_scene_len_s": min_scene_len_s,
        "shots": [
            {
                "end_s": round(end.get_seconds(), 6),
                "start_s": round(start.get_seconds(), 6),
            }
            for start, end in scenes
        ],
        "threshold": threshold,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("clip", type=Path)
    parser.add_argument("--threshold", type=float, default=27.0)
    parser.add_argument("--min-scene-len-s", type=float, default=0.6)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    result = detect_scenes(args.clip, args.threshold, args.min_scene_len_s)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
