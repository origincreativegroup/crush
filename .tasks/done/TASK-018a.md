# TASK-018: Previous-work examples and personal-style learner

Depends: Tasks 017 and 019. A feedback-centroid baseline already exists.

## Acceptance

- [ ] Users can create named, context-scoped reference sets from previous photo projects, selects,
      finished videos/reels, or individual examples and explicitly mark what represents their style.
- [ ] Uncurated folders are cataloged but contribute no positive training signal until confirmed.
- [ ] Train owner- and context-scoped ranking from pairwise preferences, picks/rejects, ratings,
      curated previous-work examples, crops, grades, exports, publishes, and confirmed Reel Studio
      evidence.
- [ ] Separate semantic relevance, general quality, personal affinity, context fit, and penalties in
      ranking and UI explanations.
- [ ] Version profiles with sample count, feature/model versions, metrics, and reversible reset.
- [ ] Held-out evaluation beats the non-personalized baseline before the UI says “learned.”
- [ ] Sparse-feedback behavior regularizes toward the general model and never invents certainty.
- [ ] Disabling/removing examples or resetting the profile falls back to Task 017 strong-shot
      ranking; the core system never relies solely on personal evidence.

## Record (PR 018a of 2, merged as PR #25)

Implemented by the agent team 2026-08-29 from .tasks/done/TASK-018-impl-plan.md. 0007_reference_sets.sql
(schema v7): owner-scoped reference sets with unconfirmed/confirmed/disabled lifecycle, whole_set vs
selected scope, items with positive/excluded roles, cleanup triggers; confirmed sets are the only
curated positive evidence and unconfirmed folders contribute nothing. Trainer personal-residual-v1
(crates/search/src/style): pairwise logistic loss over prefer/pick/reject/rating plus confirmed reference
positives, lambda regularization shrinking toward the general model, norm cap, minimum-sample floors
(6 default / 4 context) that leave the previous profile untouched, BTreeMap-deterministic pools.
Versioned profiles with activate/reset APIs; rows are never deleted. Held-out eval gate: deterministic
every-3rd-pair split, learned iff >=4 held-out pairs AND strict accuracy improvement over the
non-personalized baseline AND >=0.6; noise feedback refuses. Ranking integration extends ScoreBreakdown
with personal_affinity and context_fit (penalties exported separately); no-profile path is bit-identical
to the general ranker (test-enforced); ranking-time gate re-check ignores unlearned profiles.
crushctl style retrain/status/reset. HUMAN HARD STOP (docs/HANDOFF.md): the held-out style proof output
in PR #25 requires John's review before the UI may claim learned status (018b).

## Held-out style proof review — 2026-08-30 (human hard stop, acting reviewer)

- Probes re-run on current HEAD (`838d557`): planted-direction 12 held-out pairs, personal 1.00 vs
  baseline 0.50 → `learned=true`; identical-vector noise 12 pairs, 0.00 vs 0.50 → `learned=false`.
  Both tests self-label "not human approval". Gate logic in `crates/search/src/style/eval.rs`
  verified: deterministic every-3rd split, ties count as failures, ≥4 pairs and ≥0.6 floors,
  strict improvement required, ranking-time re-check ignores unlearned profiles.
- UI claim status verified on HEAD: `crates/app/ui/style.js` renders even an automated-`learned`
  profile as "Experimental preferences · human review pending"; `plans.js` says "Human proof
  review pending". No surface claims "learned".
- Approval of a "learned" claim is WITHHELD. The recorded probes are synthetic regression checks,
  not unseen-work proof: `docs/review-2026-08-29.md` finding 2 (pair-level split reuses media
  across train/eval; the composed production ranker is not what is evaluated) and finding 3
  (evidence-withdrawal semantics) are unremediated — TASK-032 remains in backlog.
- Gate outcome: Task 018 held-out style proof remains OPEN until asset/project-disjoint
  evaluation of the composed ranker, repeated/conflicting-evidence controls, evidence-withdrawal
  tests, and representative user review are recorded. The honest UI copy is correct as shipped and
  must not change until then.
