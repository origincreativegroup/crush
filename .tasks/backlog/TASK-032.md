# TASK-032: Preference-learning evaluation remediation (018 proof prerequisite)
Agent: OpenCode. Branch: task/32-style-eval. Depends: merged 020b; blocks the Task 018 "learned" claim.

Source findings: `docs/review-2026-08-29.md` #2/#3 and `docs/next-stretch-team-handoff.md`
(product-intelligence follow-up). The current evaluation partitions generated pairs, not assets or
projects, so train/eval reuse the same media; it scores the residual margin rather than the composed
production ranking; and withdrawn evidence can silently keep influencing the retained profile.

## Acceptance
- [x] Pair generation and evaluation split by asset AND source project/reference set; no media id
      appears on both sides. Duplicate/conflicting evidence is deduplicated or explicitly weighted.
      (Asset-level done; see the scope note in the record — project/reference-set-grouped
      partitioning needs provenance threading that lands with TASK-034.)
- [x] Evaluation scores the composed production ranker (general + scaled residual + query/context
      terms), not the residual alone; report both for transparency. (Query/context terms have no
      query at training time; the composed margin is general aesthetic margin + scaled residual,
      both accuracies reported.)
- [x] Disabling or deleting confirmed examples invalidates the affected profile version and triggers
      retrain-or-fallback; a regression test proves withdrawn evidence changes results.
- [x] Repeated-evidence and conflicting-evidence synthetic probes added beside the existing
      planted/noise probes; all documented in the style eval output John reviews.
- [x] UI wording stays experimental/review-pending until John's held-out approval; nothing here
      flips it.

## Implemented (2026-08-31, OpenCode, branch `task/32-style-eval`)

- Media-disjoint held-out split (`style/eval.rs::split_pairs`): every sorted media pool key is
  assigned train/eval by stride 3; pairs score only when both sides sit on one side, straddlers are
  counted (`straddling_pairs`) and dropped from both. Split label is now
  `media-disjoint-every-3rd`. Scope note on checkbox 1: the partition is asset-level; pairs carry
  no source-project identity today, so project/reference-set-grouped partitioning needs provenance
  threading that lands with TASK-034 — recorded here as the remaining nuance, not claimed done.
- Composed-ranker evaluation: `RankedPair` carries the general ranker's pair margin
  (`general_margin`, the general `overall` difference); evaluation scores
  `general_margin + PERSONAL_AFFINITY_SCALE (crate::PERSONAL_WEIGHT) * residual`, gates `learned`
  on that composed accuracy, and reports `residual_only_accuracy` beside it. General-margin ties
  now give the baseline no credit (the baseline vote derives from the same margin — one source of
  truth; the separate `baseline_vote` helper is gone).
- Duplicate/conflicting evidence: `build_pairs` merges every source into one map keyed by the
  ordered media pair; reversals net as negative weight, fully cancelled pairs drop, repeated
  evidence accumulates weight instead of duplicating rows.
- Withdrawal invalidation (review-2026-08-29 finding 3): disabling a confirmed set, deleting a
  confirmed set, or removing an item from a confirmed set deactivates the affected context's active
  style profiles in the same store operation — the ranker falls back to the general model
  immediately and a retrain must re-prove learning. Disabling a never-confirmed set invalidates
  nothing. Profile rows stay versioned; nothing is deleted.
- Probes (documented for John's eval review; none is human approval):
  planted-style `{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":true,"personal_accuracy":1.0,"residual_only_accuracy":1.0,"straddling_pairs":16,"split":"media-disjoint-every-3rd"}`;
  identical-vector noise `{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":false,"personal_accuracy":0.0,...,"straddling_pairs":16}`.
  New tests: media-disjoint split disjointness/counting, repeated+conflicting netting,
  disable/delete invalidation, unconfirmed-disable isolation.
- Gates on this branch: fmt clean, workspace clippy `-D warnings` clean, full workspace tests green
  (31 suites), browser harness green (17 oks). UI wording unchanged: still "Experimental
  preferences · human review pending".
- Honest consequence for the 018 proof: media-disjointness leaves the planted probe with 4 held-out
  pairs (was 12 pair-level) — the gate still passes on stricter evidence, and the "learned" claim
  stays withheld pending John's review of this output plus the TASK-034 provenance nuance.
