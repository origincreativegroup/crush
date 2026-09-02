# TASK-040: Backend contracts for the UX track follow-ups
Agent: Backend lane (OpenCode). Branch: task/40-ux-contracts. Depends: 021 merged. Source:
docs/ux-enhancement-proposal.md Track C backend items + TASK-039 wave discoveries. Each item is
small and independent; land as one PR or split if review prefers.

## Acceptance
- [x] `search` command accepts an optional `kind` argument (photo | video | all) so the UI
      kind-filter can filter server-side instead of client-side (proposal C8). Default `all`
      preserves the current contract; harness mock parity; documented in the Tauri spec table.
      (Implemented 2026-09-01, extended per the C8 source to include `span`: `SearchKind`
      (`crates/search/src/lib.rs`, `parse`/`as_str`) accepts `all` (absent default) | `photo` |
      `video` | `span`; unknown values are refused, never widened. Filtering happens AT THE
      SOURCE in `search_assets_in_context`: excluded families are never fetched or ranked, so a
      filtered search really returns top-k OF THAT KIND — and a `span` search returns only the
      bm25-ordered catalogue text matches, skipping the embedder entirely (spans carry no
      vectors), with no score sort so bm25 order survives. Spans never displace semantic
      results: the `all` path still appends span hits after the semantic top-k. Wired through
      the Tauri `search` command (`kind: Option<String>`, `crates/app/src-tauri/src/lib.rs`) and
      `crushctl search --kind` (default `all`, `crates/cli/src/main.rs`). UI: the Search kind
      selector sends the argument and re-searches on switch; the client-side post-filter now
      applies only to browse mode (see docs/ux-spec.md §3). Mock bridge honors `args.kind`;
      harness scenario `search-kind-filter` asserts each kind's call + result family, and
      `search-span-text` still passes with the re-search wiring. Search-crate tests:
      `search_kind_selects_one_family_server_side`, `span_kind_search_skips_the_embedder`,
      `search_kind_parse_refuses_unknown_values`.)
- [x] `list_videos` (or the asset-list response) exposes a thumbnail reference for video rows so
      the Library can show real thumbnails for videos, not a placeholder (proposal C7). Thumb
      path rules follow the existing photo thumb discipline; no thumbnail fabrication for assets
      that have none — honest placeholder stays.
      (Implemented 2026-09-01: `VideoView.thumb_path` (`crates/app/src-tauri/src/lib.rs`) — for
      videos the FIRST shot's `thumb_rel` in idx order (`first_shot_thumb_rel`; strictly shot 0,
      a later shot never stands in), resolved through the same `store.thumbnail_path` → absolute
      path → `convertFileSrc` discipline as photo thumbs and the Review grid; photos in the same
      response expose their own `thumb_rel` the same way. A video still indexing (no shots) or
      without a first-shot thumb gets `null` and the Library keeps the placeholder; a thumb that
      fails to load falls back to the placeholder too. UI: 16:9 `.library-thumb` cell in the
      Library table (`crates/app/ui/app.js`, `index.html`, `styles.css`); error-row colSpan 7→8.
      Harness scenario `library-thumbnails` asserts poster, honest null placeholder, and the
      photo thumb; mock bridge fixtures carry `thumbPath` with a loadable stand-in. App test
      `video_poster_is_strictly_the_first_shots_thumb`.)
- [x] Render progress events reach the UI (proposal B10): wire the existing ffmpeg progress
      callbacks (currently `|_| {}` in the render executors — see TASK-035's plan, which also
      covers this) to `render_job_set_progress` and a Tauri event the UI already listens to, so
      clip/reel renders show real percentages. Coordinate with TASK-035 to avoid double work —
      if 035 lands first, this item is done by it; verify and check off either way.
      (Checked off 2026-09-01 via TASK-035, PR #45: the executor callbacks now feed real,
      throttled, monotonic ffmpeg progress into `render_job_set_progress` — the job/attempt rows
      carry live 0.1–0.75 values — see `JobProgressWriter` in `crates/pipeline/src/render.rs`.
      Scope note: the UI currently shows an indeterminate busy state during renders and listens
      to no render-progress event, so no UI change was possible without inventing one; when the
      UX track wants live percentages, read the durable progress this wiring already produces.)
- [x] DECISION NEEDED FROM JOHN before implementing: whole-video collection membership. Today
      collections and feedback are photo/shot-scoped (`parse_library_kind` maps video→shot), so
      the Library batch bar honestly disables those ops for video rows (TASK-039 wave 3). Options:
      (a) keep as-is — video shots are collected/reviewed at shot level (recommended: matches the
      shot-identity model); (b) allow whole-video membership (store change: collection items
      kind for videos). Record the decision in this file when made.
      (DECIDED by John 2026-08-31: option (a) — shot-level membership stays. Videos are not
      collected/feedback'd as wholes; shots remain the unit, matching the shot-identity model.
      No store change; nothing to implement. The Library batch bar's honest disabling of
      photo-scoped ops for video rows is the intended behavior.)

## Rules
- No golden edits; render output byte-stable (the progress wiring touches executor callbacks —
  same constraint as TASK-035).
- Full gates: fmt, warnings-denied clippy, workspace tests, browser harness (mock parity for any
  command shape change).
