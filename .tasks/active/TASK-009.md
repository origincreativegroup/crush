# TASK-009: Search + hybrid ranking
Agent: Codex. Branch: task/09-search. Depends: 008 merged (for text embedding) — write against a trait so the search crate compiles without ort. Status: active.

## Goal
`crush-search`: text query → ranked shots. In-process, no server.

## Instructions
1. `VectorIndex::load(store, owner_id)`: matrix `Vec<f32>` (n×512) + `Vec<ShotId>`. Reload on demand after ingest.
2. `search(query_vec, top_k, owner_id)`: dot product against every row (vectors are unit-norm so dot = cosine). Plain loop with `chunks_exact(512)`; optionally `rayon` if n > 20k. Return top-k via a bounded heap.
3. Hybrid: query words (lowercased, ≥3 chars, stopwords removed) → FTS5 match on `transcripts_fts` → set of (video_id, segment spans) → a shot gets `+0.15` if any matching segment overlaps it. Formula from blueprint: `score = cosine + 0.15 * hit`. Constants in config.
4. Result struct: shot_id, video path, start_s, end_s, thumb path, score, cosine, transcript_snippet (the overlapping segment text, ≤ 200 chars).
5. CLI `search "<q>" --top N --json` prints a table or JSON.
6. Refuse to search if `embedding_meta.model_sha256` ≠ manifest sha — print "models changed, run `crushctl reembed --all`".

## Acceptance
- [x] Unit: 10k random unit vectors, top-1 equals brute-force argmax; runtime < 30 ms
- [ ] Fixture integration (needs 008): the 5 canned queries from reference/Makefile each return the expected shot in top 3 (expected shot ids listed in `fixtures/golden/expected_search.json`, filled by John after first run)
- [x] Hybrid: a query word present only in a transcript lifts that shot above an otherwise-equal one

## Do not
- Add a vector DB or ANN index. Tune the 0.15 without the smoke table.

## Human review
Try three of your own queries on fixtures.

## Implementation record

- `crush-search` depends on a `TextEmbedder` trait rather than `ort`; the CLI supplies the Task 8
  embedder through a closure.
- A contiguous owner-scoped 512-float matrix is ranked with a bounded heap. The deterministic 10k
  acceptance run completed in 7.55 ms and returned the same top result as exhaustive argmax.
- FTS query terms are lowercase, at least three characters, deduplicated, and stripped of common
  stopwords. Any overlapping matched segment adds the configured default boost of exactly 0.15.
- Search refuses stale embedding metadata, hydrates paths/timecodes/thumbnails from SQLite, limits
  Unicode transcript snippets to 200 characters, and supports table or JSON CLI output.
- The first full CPU fixture run produced semantically correct top-ranked candidates for all five
  canned queries. `expected_search.json` remains intentionally absent until John approves them.
