# TASK-039: UX/UI enhancement track — full pass (craft + workflow, view by view)
Agent: Frontend lane (OpenCode). Branch: task/39-ux-wave1 (stacked on the 021 release branch; PRs
retarget main after #37 merges). Source: docs/ux-enhancement-proposal.md (audit, 2026-08-31) and
John's direction: full pass (craft + workflow), and the first release DMG ships with the enhanced
UI — release packaging waits for this track.

Standing rules (from the 2026-08-29 review + HANDOFF): editor language, never internal vocabulary;
progressive disclosure; do not solve discoverability by adding permanent dropdowns; honest states
everywhere (no fake progress, nothing implies "learned"); the app stays a fast, dense, local
editor tool — enhance it, don't turn it into a web app.

## Wave 1 — frontend-only, harness-safe (week 1)
- [ ] Collections reachable: populate the batch "Add to collection…" select (library.js:367,700)
      and wire collection_create/create-and-add; the user-test route's collection step must work.
- [ ] WCAG AA contrast for metadata text (#6f6f78 ≈ 3.7:1, #777781 ≈ 4.15:1 on 10–11px type).
- [ ] Focus states: visible focus rings on all interactive elements (3 rules exist today).
- [ ] Esc closes the detail drawer in every view (currently Search-only).
- [ ] Arrow-key navigation stops hardcoding 4 columns (auto-fill grid aware).
- [ ] Re-search replaces results in place (no full-height panel jump when stale results render).
- [ ] prefers-reduced-motion support.
- [ ] Message parity fixes from the audit (exact list in the proposal §Track B).
- [ ] Harness scenarios updated in the same commit as any copy/DOM they assert; all 24+ stay green.

## Wave 2 — design-system consolidation (CSS-only PR)
- [ ] Token extraction: colors/spacing/type/radii/motion as CSS custom properties; kill the 108
      one-off hex values; fix the .button.small double-definition (search.css:178 vs import.css:25).
- [ ] Component consolidation: cards, pills, buttons, inputs, drawers, toasts, progress as shared
      patterns; plans.css off-palette blue family resolved.
- [ ] Validated against every harness scenario; zero backend changes.

## Wave 3 — structural (needs product calls or backend work)
- [ ] Projects In/Out as editorial timecode inputs (coordinate with TASK-037 — same form).
- [ ] Library multi-select + batch operations (John: Phase-1 spec said no multi-select — his call
      made 2026-08-31 by requesting the full pass; confirm scope when dispatching).
- [ ] Compare auto-advance default (Review).
- [ ] Backend-contract items become tasks: search kind argument; list_videos thumbnails.
- [ ] Ratify the shipped search placeholder as spec copy (the old spec line is obsolete).

## Acceptance (per wave)
- [ ] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness (all scenarios).
- [ ] No backend contract changes in waves 1–2; wave 3 backend items are separate tasks.
- [ ] Editor language throughout; honest states; no new permanent dropdowns.
- [ ] One PR per wave; harness updated in-step; release DMG is cut only after this track lands.
