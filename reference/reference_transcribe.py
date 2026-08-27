#!/usr/bin/env python3
"""Answer key for the Rust transcribe stage. Usage: reference_transcribe.py <clip> [model=small]"""
import sys, json
from faster_whisper import WhisperModel
clip = sys.argv[1]; size = sys.argv[2] if len(sys.argv) > 2 else "small"
m = WhisperModel(size, device="cpu", compute_type="int8")
segs, info = m.transcribe(clip, vad_filter=True)
print(json.dumps({"clip": clip, "model": size, "language": info.language,
                  "segments": [{"start_s": s.start, "end_s": s.end, "text": s.text.strip()} for s in segs]}))
