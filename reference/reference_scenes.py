#!/usr/bin/env python3
"""Answer key for the Rust scene detector. Same algorithm family (content/HSV delta).
Usage: reference_scenes.py <clip> [threshold=27] [min_scene_len_s=0.6] -> JSON list of cut times (s)."""
import sys, json
from scenedetect import open_video, SceneManager, ContentDetector

clip = sys.argv[1]
thr = float(sys.argv[2]) if len(sys.argv) > 2 else 27.0
min_len = float(sys.argv[3]) if len(sys.argv) > 3 else 0.6
video = open_video(clip)
sm = SceneManager()
sm.add_detector(ContentDetector(threshold=thr, min_scene_len=int(min_len * video.frame_rate)))
sm.detect_scenes(video)
scenes = sm.get_scene_list()
print(json.dumps({"clip": clip, "threshold": thr, "min_scene_len_s": min_len,
                  "shots": [{"start_s": s.get_seconds(), "end_s": e.get_seconds()} for s, e in scenes]}))
