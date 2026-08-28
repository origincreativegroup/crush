# Fixtures

This directory contains four small, redistribution-safe test clips: hard cuts, gradual motion,
vertical video, clear synthetic speech, and silent footage are all represented. Every clip is at
most 30 seconds and the set is below 20 MiB. Exact origins, transformations, and final hashes are in
`SOURCES.md`.

`golden/` contains the generated answer key. Never edit those files by hand. Regenerate them through
`make -C reference golden`, verify with `make -C reference check`, and explain why they changed in the
commit message.
