# Importing a Reel Studio catalogue (Task 022)

Crush can read an existing Reel Studio library — the `clips.db` catalogue and the JSON reel
recipes the Reel Studio editor exports — and bring the human decisions in it into your local
Crush library. Nothing is copied into the repository or uploaded anywhere; the import writes
only to your own data directory.

## What is imported

| Reel Studio | Crush |
|---|---|
| `segments` rows (tc_in/tc_out on the original clip) | **Imported spans** on the original video: human-decided boundaries that survive re-indexing and may cross Crush's own scene cuts |
| description, subjects, action, tags, shot type, camera move, notes | catalogue evidence on the span |
| quality (1–5), standout, usable | catalogue evidence on the span (not preference feedback) |
| faces_visible, nametags_visible, blur_required | safety evidence on the span |
| crop_x, `used_in` | framing and publish history on the span |
| reel recipe JSON | an immutable reel recipe (schema v2, provenance **historical**) plus a Project named `Reel Studio · <theme>` whose items are the sequence, labelled **Historical · your earlier Reel Studio choice** |

Imported items are never labelled *General* or *Preference-assisted*, and importing never records
feedback or creates previous-work reference sets. Finished projects (recipes whose segments carry
`used_in`) are listed as *eligible* in the report; promoting them to a named reference set is an
explicit step in Preferences.

## Timing honesty

Reel Studio recipe times (`in`, `out`, `crop_kf.t`, `cover.time`) are seconds **within the library
clip**, not the original. Reel Studio's 4K library clips were cut as keyframe-aligned stream copies
(`-ss tc_in -c copy`), so the library clip can start up to one GOP before `tc_in`. Crush therefore
records a **boundary basis** and tolerance on every span:

- `library_probe` — a re-encoded 1080p browse copy was found and matches the catalogue interval
  within a frame; treat boundaries as exact.
- `catalogue_tc` — catalogue timecodes taken literally with a tolerance (default 1 s, `--keyframe-tolerance`).
  The Projects editor says so under the item, and you can nudge In/Out there.

## Running it

Dry run first — nothing is written except an audit row:

```sh
crushctl import reel-studio \
  --catalogue "/Volumes/Video Production/clips.db" \
  --originals /Volumes/Footage/2026 \
  --library "/Volumes/Video Production" \
  --recipe ~/Desktop/healthy-earth.json \
  --match-by-hash
```

The report lists every source (matched by path, by SHA-256, `not_indexed`, or `missing_file`),
every segment with its outcome (`new` / `updated` / `unchanged` / `skipped`) and boundary basis,
every recipe, all issues (`missing_source`, `not_indexed`, `duplicate`, `unsupported`,
`out_of_range`, `unknown_segment`) and the planned writes per table. Add `--json` for the full
structure. Then:

```sh
crushctl import reel-studio ... --apply
```

In the app: **Library → Import Reel Studio…**, pick the catalogue, originals, optional library
folder and recipes, click **Dry run**, read the report, then **Apply**.

## Re-running

Imports are idempotent. Spans are keyed by segment id and keep their Crush id across re-imports
(so Projects referencing them survive); unchanged rows are reported as `unchanged`. A recipe whose
content already exists is `unchanged`; a Project whose name already exists is left untouched and
reported as `skipped` so your edits are never overwritten.

## Limits

- Originals must already be indexed in Crush (`not_indexed` tells you which folder to add).
- Catalogue descriptions are stored on spans; they are not yet part of the search index.
- Reel v2 treatments beyond hard cuts (captions, music, motion, keyframed crops, extended grades)
  are stored faithfully but the Task 021 renderer refuses them with an explicit capability error
  rather than rendering an approximation.
- `used_in` is kept as publish history on the span; it is not turned into feedback events.
