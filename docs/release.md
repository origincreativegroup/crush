# Crush release notes — install, privacy, data, backup, uninstall

This is the user-facing release companion for the Mac app (Task 023). It documents what a
photographer/editor installs, where their data lives, what privacy guarantees hold, and how to
back up, relink, and uninstall. It is not a substitute for the clean-machine acceptance record in
`docs/smoke.md`; a DMG and green CI do not make a release.

## Install

- **Signed & notarized**: drag the DMG's `Crush.app` into `/Applications`, launch, and grant any
  OS prompt. The `.dmg.sha256` in the release record must match the downloaded file before
  installing.
- **Ad-hoc local build** (development): open the `.app` once via `right-click → Open` (or `open`
  from a terminal) to satisfy Gatekeeper; it is explicit that this build is not notarized.
- First launch downloads the pinned models (~1.2 GB) into your Application Support directory, then
  shows the empty Library. No account, no sign-in, no cloud.

### Telling an ad-hoc build from a notarized one before installing

Ad-hoc builds (produced when no Developer ID signing secrets are present, e.g. on a dev
machine) are labeled unmistakably: the packaging step writes a `BUILD-ADHOC.txt` marker file
next to the DMG containing the ad-hoc codesign evidence, the build date, the build commit,
and the DMG checksum. A notarized release has **no** `BUILD-ADHOC.txt` and its release record
carries the notarization ticket. If a download ships with a `BUILD-ADHOC.txt` next to it,
treat it as a local development build — install it via `right-click → Open`, and never
publish it as a release.

## Privacy

- Crush is local-first: photos, videos, thumbnails, search vectors, transcripts, feedback, and
  style profiles stay on this Mac. Nothing is uploaded.
- The only network usage is: the one-time pinned-model download (SHA-256 verified against the
  release manifest) and, on raw model/hash changes, checking whether a newer pinned release is
  referenced. No media or query text ever leaves the machine.
- Feedback and preferences are owner-scoped. There is no cross-user pooling.
- Expression/safety flags are written only by explicit user action; machine scores never clear a
  privacy flag.

## Where your data lives

| What | Location |
|---|---|
| Catalog database, vectors, transcripts, feedback | `~/Library/Application Support/dev.crush.app/library.db` |
| Thumbnails | `~/Library/Application Support/dev.crush.app/thumbs/` |
| Working proxies | `~/Library/Application Support/dev.crush.app/proxies/` |
| Downloadable models | `~/Library/Application Support/dev.crush.app/models/` |
| Logs | `~/Library/Application Support/dev.crush.app/logs/` |
| Pre-upgrade database backups | `~/Library/Application Support/dev.crush.app/backups/` |

Originals are never copied into Crush: the catalog points at the media in place. Deleting a folder
from Crush never deletes the files on disk.

## Backup and restore

The catalogue is the only state that matters for recovery; originals and any renders you exported
already live outside the app. Back up the data directory while Crush is closed:

```sh
rsync -a --delete \
  "$HOME/Library/Application Support/dev.crush.app" \
  "/Volumes/Backup/crush-catalog"
```

A pre-migration snapshot is written automatically to `backups/` before schema upgrades. To restore:

1. Quit Crush.
2. Replace the data directory with the backup (or copy the named `.db` back over `library.db`).
3. Reopen Crush and run **Run Doctor** (or `crushctl doctor --deep`) to confirm the catalogue is
   consistent before trusting it.

Backing up only `library.db` loses thumbnails/proxies (re-buildable by re-indexing) but never
touches originals.

## Relink a moved drive

Crush records the original absolute path; if a drive is remounted at a different path (or a file
moves), the asset is reported honestly as missing rather than silently re-linked. To re-add media
after a move:

1. Add the folder again (Library → **Add Folder…**). Matching content hashes are detected and the
   catalog points at the new path without duplicating; deliberately the app never rewrites an
   original to "fix" a path.

## Uninstall

- Quit Crush and drag `/Applications/Crush.app` to Trash. Remove the data directory
  `~/Library/Application Support/dev.crush.app` to delete the catalogue, thumbnails, proxies, and
  models. Originals and exported renders are untouched and remain where you saved them.
- The bundled sidecars and models are inside the app bundle / Application Support, so there is no
  separate cleanup step; there is no daemon or login item.

## Supported formats (summary)

| Media | Support |
|---|---|
| JPEG, PNG, TIFF | Full decode everywhere; color-aware working proxies |
| HEIC/HEIF, DNG, camera RAW (CR2/CR3, NEF, ARW, ORF, RAF, RW2) | On Mac, capability-gated via macOS ImageIO with a full render before acceptance |
| MOV/MP4/M4V/MXF | Container-agnostic; codec-profiled. ProRes/DNxHD and common H.264 processed directly; expensive/unknown codecs get a working proxy |
| H.265/HEVC | Working proxy |
| BRAW, R3D, ProRes RAW | Explicitly unsupported (decoder licensing) |

See `docs/media-format-support.md` for the exact matrix and the guaranteed error text for
unsupported acquisition formats. Any format is never "supported" by extension alone.

## Verifying an installation

```sh
curl ... # download the DMG with its SHA-256 from the release record
shasum -a 256 Crush.dmg            # must equal the recorded digest
crushctl doctor --deep             # runtime + library integrity
```

`crushctl` is the CLI companion shipped for diagnostics; the installed app is tested through the
README/`docs/smoke.md` workflow, and the SHA-256 of the installed `.app` is recorded by
`scripts/verify-release.sh` so a build can be identified from its bundle alone.

## Packaging (maintainers)

One command produces the provenance-stamped `.app` + DMG, its `.sha256`, and the ad-hoc label
when applicable:

```sh
scripts/package-macos.sh
```

It stamps `CRUSH_BUILD_COMMIT` (`git rev-parse --short HEAD`, plus `-dirty` when the tree is
not clean; `unknown-local` when unset — never a fake commit) into both `crush-app` and
`crushctl`, so `crush-app --build-info`, `crushctl --version`, and `scripts/verify-release.sh`
all report the same build commit. A tagged CI release workflow must export `CRUSH_BUILD_COMMIT`
the same way before `cargo tauri build`; `scripts/verify-release.sh` fails visibly on any
artifact that cannot self-report a stamped commit.