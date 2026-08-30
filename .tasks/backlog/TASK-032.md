# TASK-032: Preference-learning evaluation remediation (018 proof prerequisite)
Agent: OpenCode. Branch: task/32-style-eval. Depends: merged 020b; blocks the Task 018 "learned" claim.

Source findings: `docs/review-2026-08-29.md` #2/#3 and `docs/next-stretch-team-handoff.md`
(product-intelligence follow-up). The current evaluation partitions generated pairs, not assets or
projects, so train/eval reuse the same media; it scores the residual margin rather than the composed
production ranking; and withdrawn evidence can silently keep influencing the retained profile.

## Acceptance
- [ ] Pair generation and evaluation split by asset AND source project/reference set; no media id
      appears on both sides. Duplicate/conflicting evidence is deduplicated or explicitly weighted.
- [ ] Evaluation scores the composed production ranker (general + scaled residual + query/context
      terms), not the residual alone; report both for transparency.
- [ ] Disabling or deleting confirmed examples invalidates the affected profile version and triggers
      retrain-or-fallback; a regression test proves withdrawn evidence changes results.
- [ ] Repeated-evidence and conflicting-evidence synthetic probes added beside the existing
      planted/noise probes; all documented in the style eval output John reviews.
- [ ] UI wording stays experimental/review-pending until John's held-out approval; nothing here
      flips it.
