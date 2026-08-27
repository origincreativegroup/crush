# UX Spec — Crush Mac app (Phase 1)

One window, four views, one HTML file. Dark, dense, editor-tool feel. The user is an editor with footage open in another app; this is a finder, not a destination.

## Global
- Window 1100×720 min. Sidebar 220 px: **Search**, **Library**, footer shows indexing status ("Indexing 3 of 12 · 42%") and a Doctor link.
- Font: system UI; timecodes and filenames in SF Mono. Base 13 px.
- Colors: bg #141416, panel #1d1d21, text #e8e8ea, muted #8a8a93, accent #4f8cff, danger #ff5c5c. Thumbnail boxes 16:9, bg #26262b while loading.
- Keyboard: Cmd-F focus search, Esc clears/closes detail, ↑↓ move through results, Enter opens detail, Space play/pause in detail.

## 1. First-run
Shown when any model is missing. Centered card: "Downloading models (about 700 MB, one time)". One row per file: name, size, progress bar, state. Retry button on failure with the error text. "Continue" enabled when all present. Never skippable — the app is useless without them.

## 2. Library
Toolbar: **Add Folder…** (native picker; drag-drop onto the list also works), Re-index selected, Cancel (only while indexing).
Table rows: filename · duration · resolution · status pill (Pending / Splitting / Embedding / Transcribing / Done / Failed / Cancelled) · thin progress bar for the active one · shots count.
Failed row: chevron expands to show the job error text and a "Copy details" button (job id, stage, error, log path).
Empty: "No footage yet. Add a folder to start indexing." with the Add button.
Rule: indexing never blocks the UI; the user can search what's already indexed while more indexes.

## 3. Search
Top: search input, placeholder "Describe the shot… e.g. wide shot of the storefront at dusk". Right: result count, Top 25/50/100.
Grid: 4 columns. Card = thumbnail, bottom-left duration badge, bottom-right score (0–100, rounded), filename line truncated, transcript snippet line (muted, one line) if present.
Hover: play-icon overlay. Click/Enter: detail.
Empty (nothing indexed): link to Library. No matches: "No matches. Try broader words — CLIP understands objects, scenes, and actions better than names."
Latency: results replace in place; no spinner under 500 ms.

## 4. Shot detail (slide-over panel, right, 520 px)
Video player (source file, seeks to start, pauses at end, loops on L). Under it: `HH:MM:SS.ff → HH:MM:SS.ff  (3.2 s)` in mono, filename, shot idx of N.
Buttons: **Copy path + timecodes** (format: `/path/file.mov  00:01:12.04 – 00:01:15.10`), **Export clip…** (save dialog, default name `file_shot012.mov`, toast with Reveal), **Reveal in Finder**.
Transcript: overlapping segments, matched words highlighted.
Prev/Next shot in the same video (←/→).

## States summary
| State | Where | Copy |
|---|---|---|
| Downloading | first-run | per-file progress |
| Indexing | footer + Library | "Indexing N of M · %" |
| Failed video | Library | error + copy details |
| Nothing indexed | Search | link to Library |
| No matches | Search | broader-words hint |
| Export done | Detail | toast + Reveal |

## Not in Phase 1
Multi-select, tagging, editing shot boundaries, timeline view, settings screen (config file only), light theme.
