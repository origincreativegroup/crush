# QA Test Pack — Crush Phase 1

Run on John's MacBook. Record results in this file under a dated heading.

## Preconditions
- Fresh data dir: `CRUSH_DATA_DIR=/tmp/crush-qa crushctl doctor`
- Fixtures in `fixtures/clips/`, goldens committed.

## Automated (must be green before manual)
- [ ] `cargo test --workspace` on CPU
- [ ] `cargo test -p crush-stage-embed` on CoreML (macOS)
- [ ] `cargo clippy --workspace -- -D warnings`

## Happy path
- [ ] Ingest fixtures folder → all videos Done; shot counts sane
- [ ] 5 canned queries → expected shot in top 3
- [ ] Open detail, play, copy path, export clip, play exported clip in QuickTime

## Edge cases
- [ ] Clip with no audio → Done, 0 segments, searchable visually
- [ ] Clip with a single continuous shot → exactly 1 shot
- [ ] Vertical phone clip → thumbnails not stretched; grid box letterboxes
- [ ] Filename with spaces, unicode, apostrophe → ingest, export, reveal all work
- [ ] Same file added twice (two folders) → indexed once
- [ ] File modified after indexing (same name) → new video row; old still searchable
- [ ] 2-hour file → completes; memory stays < 3 GB; laptop usable
- [ ] 10k+ shots library → search < 500 ms

## Failure cases
- [ ] Quit app mid-ingest → relaunch resumes; no duplicates (`doctor --deep` clean)
- [ ] Cancel → stops within 5 s; status consistent
- [ ] Delete a model file → doctor reports missing; first-run screen reappears; re-download works
- [ ] Corrupt video file in folder → that video Failed with error; others complete
- [ ] Unplug network during model download → retry works after reconnect
- [ ] Source video moved after indexing → search still returns it; detail shows "file not found" with the old path; export fails cleanly

## Data checks
- [ ] Every shot has a thumbnail file
- [ ] Every embedded shot has a vector of 512 f32 with norm 1 ± 1e-3
- [ ] `doctor --deep` reports zero problems

## Clean-machine (Phase 2 gate)
- [ ] New macOS user account, no Xcode, no brew: install dmg, launch, models download, index fixtures, search, export

## Smoke
See smoke.md — 10 pre-written queries on ≥ 5 h of real footage, target 8/10.

## Recommendation
Pass / Needs fixes / Blocked — with the bug table below.

| Bug | Severity | Steps | Expected | Actual |
|---|---|---|---|---|
