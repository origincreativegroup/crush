# TASK-037: First-class spans — adjustable boundaries and one catalogue (Reel Studio unification, step 1)
Agent: OpenCode. Branch: task/37-span-first-class. Depends: 021 merged (touches render.rs; must keep
existing shot/photo/reel output byte-stable so the approved render packet is not invalidated).

Source: John's direction, 2026-08-31 — "Reel Studio should be Crush and vice versa; I own both
projects." Crush and Reel Studio are one product lineage, not an app plus an external catalogue.
The importer's whole point was adjustable clips; today's design defeats it.

## The defect this fixes

Imported spans are treated as frozen containers instead of clips:

- `validate_plan_item_against_media` (store) rejects any span plan-item edit outside the span's own
  imported boundaries — you can only shrink, never extend.
- The reel executor (`render.rs` span resolution) enforces the same clamp at render time.
- The Projects edit form caps In/Out to the span's own boundaries (the frozen candidate carries
  `start_s`/`end_s` = span boundaries).
- The UI warns catalogue timecodes "may be off by up to X s" while making it impossible to extend
  the boundary to correct the drift. The source video is in the library; the clamp is a design
  choice, not a constraint.

## Acceptance

- [ ] Span plan-item boundaries clamp to the **source video's** range (0..duration), not the span's
      own boundaries. The imported span boundaries remain the item's default on import. NOTE
      (found while planning): the clamp is enforced in FOUR places, not two — the store API, the
      reel executor, the Projects form, AND two SQL triggers in migration 0011
      (`plan_item_boundaries_insert/_update`); a schema v12 migration rebuilding the span trigger
      arms is required. See `.tasks/backlog/TASK-037-impl-plan.md`.
- [ ] The Projects edit form's In/Out min/max come from the source video range; the
      boundary-basis/tolerance note stays visible for `catalogue_tc` spans.
- [ ] Adjustments are recorded honestly: provenance gains an `adjusted: true`-style marker (or
      equivalent) when item boundaries differ from the imported span's, without losing
      `import_id`/`external_id` lineage. Re-import never overwrites an adjusted item's boundaries
      (idempotence rule: adjusted items report `unchanged`, not reverted).
- [ ] The reel executor's span clamp changes to the source video range with the same honest
      provenance; shot and photo paths remain byte-stable (the render packet must not be
      invalidated — coordinate with the 021 owner as TASK-035 does).
- [ ] Store tests: extend-beyond-span accepted and clamped to video duration; shrink still works;
      adjusted provenance recorded; re-import after adjustment does not revert.
- [ ] Browser harness: a span item's In/Out can be extended past the imported boundaries within the
      video range; the tolerance note still renders.
- [ ] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness.

## Out of scope (tracked elsewhere)

- Span text into search + Review filtering, and the preference-evidence confirmation flow:
  TASK-034 (being reframed from "imported-evidence bridge" to catalogue unification).
- Running Crush's aesthetic analysis over span intervals (pending John's decision — compute cost
  per clip; would make spans appear in strong-shot candidates).
- The reel v2 treatment vocabulary (captions, music, motion, keyframed crops, extended grades) —
  the long-term native roadmap now that the products are one lineage; stays an honest capability
  error until its frozen contracts exist.
