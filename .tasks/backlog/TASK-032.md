# TASK-032: Preference-learning evaluation remediation (018 proof prerequisite)
Agent: OpenCode. Branch: task/32-style-eval. Depends: merged 020b; blocks the Task 018 "learned" claim.

Source findings: `docs/review-2026-08-29.md` #2/#3 and `docs/next-stretch-team-handoff.md`
(product-intelligence follow-up). The current evaluation partitions generated pairs, not assets or
projects, so train/eval reuse the same media; it scores the residual margin rather than the composed
production ranking; and withdrawn evidence can silently keep influencing the retained profile.

## Acceptance
- [x] Pair generation and evaluation split by asset: no media id appears on both sides.
      Duplicate/conflicting evidence is deduplicated or explicitly weighted. Project/reference-set
      grouping is explicitly DEFERRED to TASK-034 (it needs provenance threading) — this box
      covers the asset-level split only.
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
  (`general_margin`, the production general-aesthetic adjustment difference — amended by the
  review fixes below); evaluation scores
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
- Probes (documented for John's eval review; none is human approval; verbatim `metrics_json`):
  planted-style `{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":true,"personal_accuracy":1.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":1.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}`;
  identical-vector noise `{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":false,"personal_accuracy":0.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":0.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}`;
  repeated+conflicting prefer netting `{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":true,"personal_accuracy":1.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":1.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}`
  (two prefer votes for good-0 over bad-0 plus one reversed vote net to exactly one unit of
  pair weight: the trained profile is identical to a single-prefer run — repeats accumulate,
  reversals subtract, nothing duplicates).
  The planted probe now carries opposing general assessments (picked shots `overall` 0.3,
  rejected shots 0.7), so every pair has a nonzero production-scale general margin that
  AGAIN disagrees with the owner: the composed ranker recovers all held-out pairs while the
  general-only baseline scores 0.0 — the composition is genuinely exercised, not a residual
  echo.
  New tests: media-disjoint split disjointness/counting, repeated+conflicting netting,
  disable/delete invalidation, unconfirmed-disable isolation.
- Gates on this branch: fmt clean, workspace clippy `-D warnings` clean, full workspace tests green
  (31 suites), browser harness green (17 oks). UI wording unchanged: still "Experimental
  preferences · human review pending".
- Honest consequence for the 018 proof: media-disjointness leaves the planted probe with 4 held-out
  pairs (was 12 pair-level) — the gate still passes on stricter evidence, and the "learned" claim
  stays withheld pending John's review of this output plus the TASK-034 provenance nuance.

## Review fixes applied (2026-08-31, PR #39 FIX-FIRST findings)

- H-1 — the eval margin is now the production margin. `general_margin` was previously the raw
  `overall` difference (±1.0 scale) while production weights the general term as
  `0.16 × (overall − 0.5)` beside `0.15 ×` personal, over-crediting the general term ~6.25×.
  The trainer now computes the pair margin as the difference of the PRODUCTION adjustment
  (`GENERAL_AESTHETIC_WEIGHT × (overall_plus − 0.5) − GENERAL_AESTHETIC_WEIGHT ×
  (overall_minus − 0.5)`), with a missing side treated as neutral (adjustment 0, i.e.
  `overall` 0.5) exactly like production's missing-assessment behavior. The 0.16 is extracted
  into `crush_search::GENERAL_AESTHETIC_WEIGHT`, used by BOTH the production composition and
  the eval, so they cannot drift. Disclosure: editorial-quality and penalty terms remain
  EXCLUDED from the eval composition (they were silently excluded before this PR too) — the
  eval composes general aesthetic + personal residual only; query/context terms have no query
  at training time and editorial/penalty adjustments are per-asset annotations outside the
  learned-margin comparison.
- H-2 — the composed scoring is now tested. `composed_scoring_uses_the_production_general_margin_scale`
  proves a production-maximum general margin (0.16) beside a −1.0 residual yields a positive
  composed vote (personal credit) while the residual alone is negative, and that a mild
  production-scale margin (0.032) cannot outweigh the scaled residual (the old raw-scale 0.2
  wrongly earned credit there). The planted probe also gained differing `overall` assessments
  (see above) so the documented probe output exercises the composition.
- H-3 — the netting test was vacuous (3 picks + 1 reject of ONE media sat below the 6-sample
  floor, so `retrain_style_profile` returned `Ok(None)` and no assertion ran; same-media
  pick+reject resolves at pool level, never reaching `build_pairs`). Replaced with:
  `reversed_prefer_pairs_net_to_zero_and_are_dropped` and
  `repeated_prefer_pairs_accumulate_weight_not_rows` (direct `build_pairs` unit tests in
  `trainer.rs`), plus the store-level probe `repeated_and_conflicting_prefer_evidence_is_netted_not_duplicated`
  (12 media, prefer-pair reversal; metrics documented above beside the planted/noise probes).
- M-1 — withdrawal invalidation is now transactional. `reference_set_set_status`,
  `reference_set_delete`, and `reference_set_remove_item` each wrap their mutation and the
  profile deactivation in one `transaction_with_behavior(Immediate)` (the `put_style_profile`
  pattern), so a crash between statements can no longer leave withdrawn evidence influencing
  an active profile. `deactivate_style_profiles_for_context` is now a shared helper taking the
  connection (or open transaction) — `reference_set_remove_item` reuses it instead of inlined
  SQL — and `set_status` reads `context_key` before the update, consistent with delete.
- L-1/L-2 — the trainer module doc's stale "every-third-pair" wording is now
  "every-third-media", and the app mock bridge serves the real split label
  `media-disjoint-every-3rd` instead of the retired `loo-every-3rd`.
- Probe caveat for the 018 review: the planted probe's 12 shots all come from one source
  video, so "held-out" means held-out media, not fully unseen footage — project-level
  grouping is the TASK-034 fix.
