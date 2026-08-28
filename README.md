# Crush

Crush is a private, local-first editorial intelligence system for photos and video. It recognizes
strong shots, learns how a particular user defines even better and more consistent work, plans
selects and clips, and renders finished media without uploading originals. A searchable DAM is the
foundation that makes those decisions traceable; cataloging is not the product's only purpose.

**Status:** pre-alpha. Video ingest/search/export and the JPEG/PNG DAM vertical slice work. The
next milestone adds RAW/HEIF/TIFF photo sources, production-video proxies and metadata. The core
general-quality model must work before personalization; user feedback and explicitly added examples
of previous work then refine its style match. See `TASKS.md`.

- Blueprint: `docs/project-blueprint.md`
- DAM and feedback direction: `docs/dam-feedback-blueprint.md`
- Review + build protocol: `docs/blueprint-review.md`
- Agents start here: `docs/HANDOFF.md`
- Testing on the Mac: `docs/testing-on-macbook.md`

License: Apache-2.0. Bundled ffmpeg is an LGPL build — see `THIRD_PARTY.md`.
