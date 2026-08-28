# TASK-012c: Search + Shot detail (done)
Agent: Codex on the Mac. Branch: task/12c-search. Depends: 012b. UX spec: docs/ux-spec.md.

## Acceptance
- [x] Search box focused on launch and on Cmd-F; results grid of thumbnails (score, duration, filename) within 500 ms for 5k shots
- [x] Shot detail: `<video>` element playing the source file from start_s (Tauri asset protocol), stops at end_s; timecodes in `HH:MM:SS.ff`; transcript snippet; Copy path+timecodes; Export clip (save dialog); Reveal in Finder
- [x] "No matches" and "nothing indexed" states
- [x] Dark theme, monospace timecodes, no layout jank while thumbnails load (fixed aspect boxes)

## Human review

The original video-only hard stop was superseded by the DAM pivot. The completed Search and Shot
detail implementation was merged in PR #14 and now underpins mixed photo/video review.

## Implementation record (2026-08-28, branch `task/12c-search`)

Rebased onto the merged Task 12b implementation before review.

- Rust: `tauri` gains the `protocol-asset` feature; `assetProtocol` is enabled with an empty static
  scope that grows at runtime — the thumbs dir and every previously indexed video at startup,
  the folder in `add_folder`, and the source file in `shot_detail`. New `shot_at_index(video_id, idx)`
  command backs Prev/Next. `dialog:allow-save` added for Export clip.
- UI: `ui/search.js` + `ui/search.css` (new files; `index.html` gets the search view, the 520 px
  slide-over, and two link tags). Search is the launch view once models are present; Cmd-F focuses
  it from anywhere. Debounced (160 ms) in-place results, 4-column grid of fixed 16:9 boxes with
  duration + 0–100 score badges, filename, one-line transcript snippet, hover play overlay.
  ↑↓←→ move through results, Enter opens, Esc closes/clears. Detail: `<video>` via
  `convertFileSrc`, seeks to `start_s`, pauses at `end_s`, `L` loops, Space play/pause, ←/→ prev/next
  shot; `HH:MM:SS.ff` timecodes from the video fps; Copy path + timecodes (`/path/file.mov  HH:MM:SS.ff – HH:MM:SS.ff`);
  Export clip via native save dialog (default `file_shot012.mov`) with a toast + Reveal; Reveal in Finder;
  transcript segments with query words highlighted.
- States: nothing indexed (link to Library), idle, no matches (broader-words hint), error.
- Harness: `tests/ui-harness.html` mocks `search`, `shot_detail`, `shot_at_index`, `export_clip`,
  `dialog.save`, `convertFileSrc`. A headless Chrome run (playwright-core driving Google Chrome)
  passed 20 checks: launch focus, results, keyboard selection, timecode format, prev/next, highlight,
  export toast, Esc behaviour, no-matches, Library/Cmd-F round trip, nothing-indexed.
- Search uses the CPU text encoder so CoreML graph compilation stays on the batch-ingest path. A cold
  CLI query against the real app database completed in 0.38 s and returned the indexed Earth shots;
  the 10k-vector benchmark completes in 0.15 s.
- `cargo fmt`, strict workspace Clippy, workspace tests, the macOS app bundle build, and strict deep
  code-signature verification pass.

The next end-to-end human acceptance is the mixed-media clean-machine gate in Task 023.
