# TASK-022: Reel Studio historical-evidence importer

Depends: Tasks 019–021.

## Acceptance

- [ ] Import catalogue quality, standout, usable, descriptions, subjects, actions, tags, and safety
      fields into the unified DAM schema without copying private media into the repository.
- [ ] Import confirmed crop, grade, recipe membership, sequence, used-in, and publish evidence with
      explicit provenance and owner/context scope.
- [ ] Imported finished projects can become named previous-work reference sets only through an
      explicit user choice; merely discovering them does not train the personal model.
- [ ] Dry-run reports mappings, missing files, duplicates, unsupported data, and planned writes.
- [ ] Re-running is idempotent and never converts an inferred rejection into explicit feedback.
- [ ] Imported recipes can be opened, edited, and rendered by Task 021.
- [ ] Reel Studio manual segments retain their exact source-relative boundaries even when they
      cross Crush auto-detected scene cuts; re-index/resplit cannot erase imported spans.
- [ ] Historical human choices use explicit imported/historical provenance in Projects and its
      pills; they are never mislabeled as general or personalized model selections.
- [ ] The documented Reel Studio recipe surface is preserved: theme/vibe/music/target length,
      beat snap/aspect/music volume/watermark/cover and per-item in/out, crop/keyframes,
      caption/position, transition, speed, motion, natural-audio volume and grade. Unsupported
      legacy formats or treatments appear in dry-run errors instead of being discarded.
