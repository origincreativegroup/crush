# Installing Crush

Crush is a Mac-only app for Apple silicon (M1 or later, macOS 10.15+; tested on macOS 26). It
needs no developer tools, no Python, no Homebrew. Everything it runs — ffmpeg, the CLIP and
Whisper models — is either inside the app bundle or downloaded once on first launch.

## 1. Get the disk image

Download `Crush-<version>-aarch64.dmg` from the GitHub release for the tag you want
(`https://github.com/origincreativegroup/crush/releases`). The `.sha256` file next to it lets
you check the download:

```sh
shasum -a 256 -c Crush-<version>-aarch64.dmg.sha256
```

## 2. Install

1. Open the `.dmg`.
2. Drag **Crush** onto the **Applications** folder.
3. Eject the disk image.

## 3. First launch

**Signed release** (the release page's job summary says "Signed with Developer ID and
notarized"): double-click Crush in Applications. Done.

**Unsigned release** (job summary says "UNSIGNED build"): macOS Gatekeeper will refuse a plain
double-click with "cannot be opened because the developer cannot be verified". Do this once:

1. In Applications, **right-click (or Control-click) Crush → Open**.
2. In the dialog, click **Open** again.

macOS remembers the choice; from then on it launches normally. If you instead see "Crush is
damaged and can't be opened", the quarantine flag is fighting the ad-hoc signature. Clear it and
retry the right-click → Open step:

```sh
xattr -dr com.apple.quarantine /Applications/Crush.app
```

## 4. Model download (one time, about 1.2 GB)

The first-run screen downloads the pinned CLIP and Whisper models into
`~/Library/Application Support/dev.crush.app/models/`. It is resumable — if the network drops,
press **Retry** and it continues from where it stopped. Every file is SHA-256 verified before it
is used. The app cannot be used until all files are present; there is no skip.

The first search or index also compiles the CLIP model for the Apple Neural Engine. That takes
one to three minutes once and is cached by macOS afterwards.

## 5. Check the runtime

Click **Doctor** in the sidebar footer. A healthy report shows:

- `ffmpeg source=Bundled` — the app is using its own ffmpeg, not one from your PATH
- `models=5/5 present`
- a data dir under `~/Library/Application Support/dev.crush.app`

## 6. Index and search

**Library → Add Folder…**, pick a folder of footage. Indexing runs in the background at low
priority; you can search whatever is already done while the rest indexes. Then **Search**, type a
description ("wide shot of the storefront at dusk"), press Enter.

## Where things live

| What | Where |
|---|---|
| Database, thumbnails, models, logs | `~/Library/Application Support/dev.crush.app/` |
| Logs | `.../dev.crush.app/logs/crush.log` |
| Optional config | `~/Library/Application Support/dev.crush.app/crush.toml` (see `crush.example.toml`) |

## Uninstall

Drag Crush from Applications to the Trash and delete
`~/Library/Application Support/dev.crush.app`. Nothing else is written anywhere.

## Clean-machine acceptance (Task 13)

Run this on a **fresh macOS user account** with no Xcode, Homebrew, Rust, or Python:

1. Install from the `.dmg` as above; launch (right-click → Open if unsigned).
2. First-run download completes; Doctor shows `ffmpeg source=Bundled` and `models=5/5 present`.
3. Add the `fixtures/` folder (copy it to the account first); all four videos reach **Done**.
4. Search "rocket launch" — the rocket fixture is in the top results.
5. Record the result in `docs/smoke.md`.
