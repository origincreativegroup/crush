# Crush

Crush is a private, local-first editorial intelligence system for photos and video. It recognizes
strong shots, learns how a particular user defines even better and more consistent work, plans
selects and clips, and renders finished media without uploading originals. A searchable DAM is the
foundation that makes those decisions traceable; cataloging is not the product's only purpose.

**Status:** pre-alpha. Mixed-media ingest/search, capability-gated RAW/HEIF/TIFF support,
general strong-shot analysis, review tools and editable select plans are implemented. Personal
profiles are experimental: held-out style proof still requires human review. Full recipe-based
photo/reel rendering is in progress, followed by the Reel Studio importer and release acceptance.
See `TASKS.md` for implemented scope and the remaining human gates.

Clip export requires a new destination filename; it will not replace an original or an existing
export. Rendering uses private staging and exclusive publication. If the destination filesystem
does not support hard links, export fails safely; choose a supported local filesystem instead.

- Blueprint: `docs/project-blueprint.md`
- DAM and feedback direction: `docs/dam-feedback-blueprint.md`
- Review + build protocol: `docs/blueprint-review.md`
- Agents start here: `docs/HANDOFF.md`
- Testing on the Mac: `docs/testing-on-macbook.md`

License: Apache-2.0. Bundled ffmpeg is an LGPL build — see `THIRD_PARTY.md`.
