# Task 018 style-proof review packet

This is the review packet for the Task 018 human hard stop: the held-out style proof. The
personal-style learner was built in Task 018a (PR #25) and its evaluation was remediated by
TASK-032 (merged as PR #39, commit `2766843`, 2026-08-31). The "learned" claim stays withheld
until a human records a verdict in the last section of this document. Automated gates and
synthetic probes passed before this file was written; that is not approval. This document
presents evidence; it grants nothing.

Until a verdict is recorded below, the UI keeps its current wording: `crates/app/ui/style.js`
renders even an automated-`learned` profile as "Experimental preferences · human review pending",
and `plans.js` says "Human proof review pending". No surface claims "learned".

## What this gate decides

Plainly: whether Crush may replace the "Experimental preferences · human review pending" wording
with "learned" wording, for profiles that pass the automated held-out gate and receive approval
here.

What it does NOT decide:

- **Not a claim that recommendations are optimal.** "Learned" would mean exactly one thing: on
  preference pairs the trainer never saw, the composed ranker (general model + personal residual)
  scored strictly better than the general-only baseline. It says nothing about beating a human
  editor's picks, and nothing about quality beyond that measurement.
- **Not permission to pool data across owners.** Profiles stay owner- and context-scoped, and
  training negatives are drawn from the same owner's pool, never another owner's (privacy rule
  from the 018 implementation plan). Nothing here changes that.
- **Not a color or treatment claim.** The personal term reorders search results; it does not
  alter pixels, grades, or renders. Render and color approval is the separate Task 021 human gate
  (`docs/task-021-render-review.md`).

## The evidence, verbatim

All three probe outputs below are copied byte-for-byte from `.tasks/backlog/TASK-032.md`
("Probes", lines 48–59). The PR #39 body carries the same three lines, byte-identical — no
metric disagrees between the two sources.

**The split, in plain terms.** Every media asset (sorted by its pool key) is assigned to the
training side or the held-out side by taking every third one. A preference pair is evaluated
only when both of its media sit on the held-out side, and trained only when neither does. Pairs
that straddle the line — one media on each side — are dropped from both sides and counted, so
the count is visible rather than hidden. Split label: `media-disjoint-every-3rd`. No media
appears on both the train and eval sides.

**The gate rule.** A profile is marked `learned` only if it has at least 4 held-out pairs, scores
strictly higher than the general-only baseline on them (ties count as failures), and scores at
least 0.6.

### Probe 1 — planted style

```json
{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":true,"personal_accuracy":1.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":1.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}
```

Plain reading: the test owner was given a planted style — preferences for shots the general
model scores low and rejections for shots it scores high (picked shots rated `overall` 0.3,
rejected shots 0.7), so the general model disagrees with the owner on every pair. On the 4
held-out pairs the trainer never saw, the composed ranker got all 4 right
(`personal_accuracy` 1.0) while the general-only baseline got all 4 wrong
(`baseline_accuracy` 0.0), so the gate marks `learned: true`. Check it yourself: 1.0 vs 0.0 on
4 pairs. (`personal_scale` 0.15000000596046448 is the personal term's weight, 0.15, as the
computer stores it — the long decimal is float storage, not a different number.)

### Probe 2 — identical-vector noise

```json
{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":false,"personal_accuracy":0.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":0.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}
```

Plain reading: when the "preferences" carry no real signal (identical vectors), the trainer
scores 0.0 on the same 4 held-out pairs and the gate refuses (`learned: false`). Noise cannot
earn the label even though the baseline is also 0.0, because the gate requires the personal
ranker to strictly beat the baseline. Check it yourself: 0.0 vs 0.0 is not an improvement, and
`learned` is false.

### Probe 3 — repeated and conflicting evidence nets

```json
{"baseline_accuracy":0.0,"held_out_pairs":4,"learned":true,"personal_accuracy":1.0,"personal_scale":0.15000000596046448,"residual_only_accuracy":1.0,"split":"media-disjoint-every-3rd","straddling_pairs":16,"trainer":"personal-residual-v1"}
```

Plain reading: stating the same preference twice and then reversing it once trains a profile
byte-identical to stating the preference once — repeats add weight, reversals subtract, and
nothing is double-counted. The proof is that the trained profile (weights, feature JSON,
metrics) is identical to a single-prefer run; this metrics line is that run's. Check it
yourself: the line is identical to Probe 1's, which is the point.

### Honest consequence of the stricter split

Holding out whole media assets leaves the planted probe 4 held-out pairs. The pre-remediation
pair-level split reported 12 held-out pairs (planted 1.00 vs baseline 0.50; noise 0.00 vs 0.50;
split `loo-every-3rd` — recorded in `docs/review-2026-08-29.md` and
`.tasks/done/TASK-018a.md`). The gate now passes on fewer but cleaner pairs: none of the 4
evaluated pairs reuses media the trainer saw.

### Where these numbers live, and source notes

- Verbatim `metrics_json`: `.tasks/backlog/TASK-032.md` (this branch) and the PR #39 body —
  byte-identical between the two.
- Code on this branch: split and gate in `crates/search/src/style/eval.rs` (split label
  `media-disjoint-every-3rd`, floors of 4 pairs and 0.6 accuracy), evidence netting in
  `crates/search/src/style/trainer.rs`, shared weights in `crates/search/src/lib.rs`
  (`PERSONAL_WEIGHT` 0.15, `GENERAL_AESTHETIC_WEIGHT` 0.16).
- To re-run the probes yourself on this checkout (`/Users/origin/GitHub/crush`, branch
  `task/21-render-export`): `cargo test -p crush-search`. The relevant tests are
  `composed_scoring_uses_the_production_general_margin_scale`,
  `reversed_prefer_pairs_net_to_zero_and_are_dropped`,
  `repeated_prefer_pairs_accumulate_weight_not_rows`, and
  `repeated_and_conflicting_prefer_evidence_is_netted_not_duplicated` (names verbatim from
  TASK-032.md / PR #39).
- PR #25's body flags this hard stop but does not paste the original probe output; the
  pre-remediation numbers live in `docs/review-2026-08-29.md` and `.tasks/done/TASK-018a.md`.
  The current evidence lives in the two sources named above.
- Differences found while assembling this packet, none affecting the metrics: (1) PR #39
  records a follow-up commit TASK-032.md does not mention — `06f4ef9` fixed the composition
  test, which had used 2.0 × `GENERAL_AESTHETIC_WEIGHT` (0.32) instead of the true production
  maximum (0.16) and so tolerated 2× drift in the shared weight; all gates re-run green after
  the fix. (2) Gate framing differs harmlessly: TASK-032.md says "full workspace tests green
  (31 suites)"; PR #39 says "workspace tests green (136 passed, 0 failed)" — suites vs
  individual tests. (3) `docs/review-2026-08-29.md` quotes the UI string as "Experimental
  profile · human review pending"; the shipped string in `crates/app/ui/style.js` is
  "Experimental preferences · human review pending" (verified on this branch — the code is the
  truth, the review doc's quote is loose).

## What the evaluation now guarantees

Honest and technical, per TASK-032 and the PR #39 review fixes:

- **No media on both sides.** The train/eval partition is over media assets, not generated
  pairs; straddling pairs are dropped and counted (`straddling_pairs` in every metrics line).
  A structural test enforces the disjointness and the counting.
- **The scored ranker is the production composition.** Evaluation scores the general aesthetic
  term at its production weight plus the personal residual at its production weight
  (`GENERAL_AESTHETIC_WEIGHT` 0.16 and `PERSONAL_WEIGHT` 0.15 are single shared constants used
  by both the production ranking and the eval, so they cannot drift). The residual-only
  accuracy is reported beside the composed accuracy for transparency. Editorial-quality and
  penalty terms are excluded from the eval composition — disclosed here, and they were silently
  excluded before PR #39 as well; query/context terms have no query at training time.
- **Withdrawn evidence invalidates the profile, transactionally.** Disabling a confirmed
  reference set, deleting one, or removing an item from one deactivates the affected context's
  active style profiles in the same database transaction as the withdrawal itself — a crash
  between the two can no longer leave withdrawn evidence influencing an active profile. The
  ranker falls back to the general model immediately, and any retrain must re-prove learning.
  Profile rows stay versioned; nothing is deleted. Disabling a never-confirmed set invalidates
  nothing.
- **Repeated and conflicting evidence nets.** All evidence sources merge into one map keyed by
  the ordered media pair: reversals net as negative weight, fully cancelled pairs drop, repeats
  accumulate weight instead of duplicating rows.
- **Defense in depth.** The gate is re-checked at ranking time — a profile that is not
  `learned` is ignored by scoring even if somehow active — and the no-profile path is
  bit-identical to the general ranker (test-enforced).

## Known limits — stated plainly

From TASK-032's own disclosures; these are the candidate gaps if the answer below is "no":

- **One source video.** The planted probe's 12 shots all come from one source video, so
  "held-out" means held-out media, not fully unseen footage. Project/reference-set-level
  grouping needs provenance threading (pairs carry no source-project identity today) and is
  deferred to TASK-034.
- **General + personal only.** The eval composes the general aesthetic term and the personal
  residual only. Editorial/penalty terms are excluded (disclosed above) and query/context terms
  are absent at training time — this is not the full production score.
- **Synthetic probes, not real feedback.** The proof is planted/noise/netting probes, not a
  large corpus of real owner feedback. The 2026-08-30 reviewer record already noted the probes
  are synthetic regression checks, not unseen-work proof; that character still holds.
- **Small held-out set, constructed baseline.** Four held-out pairs per probe is stricter than
  the old twelve but still small, and the 0.0 baseline exists because the probe deliberately
  plants general assessments that disagree with the owner — a real baseline will not be 0.0.

## How to review it

Review order:

1. Read the three metrics lines above and check each plain-language sentence against the
   numbers.
2. Check the split facts in every line: `split` is `media-disjoint-every-3rd`,
   `straddling_pairs` is 16, `held_out_pairs` is 4.
3. Read "What the evaluation now guarantees" and "Known limits — stated plainly" before
   deciding; the limits are the candidate gaps.

Approval questions:

- Does the held-out evidence justify replacing "Experimental preferences · human review
  pending" with "learned" wording for approved profiles — yes or no?
- If no, which gap blocks it: the single-source-video held-out media (TASK-034), the
  general+personal-only composition, the synthetic-probe basis, or the small held-out set?
- If yes, is the approval unconditional, or conditional on TASK-034's project-level grouping
  landing before any stronger claim is made?

Record the decision and date in the verdict section below. Do not flip the UI wording on the
strength of automated gates alone; the wording change follows this verdict, not the other way
around.

## Verdict — for John only (intentionally empty)

> No agent fills this section. It stays empty until John records his decision.

- Decision (approve "learned" wording / withhold): ____________
- Date: ____________
- Conditions, or the blocking gap if withholding: ____________
