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

## Adjusting imported clips

An imported span is a clip, not a frozen container. Its imported boundaries are the item's
**default**, not a limit: in Projects you can extend or shrink In/Out anywhere inside the source
video (0 to its duration). The first time an item's In/Out move away from the imported boundaries,
Crush records `adjusted: true` (with a timestamp) in the item's provenance — derived by the store
itself, so it always matches the saved boundaries; moving back to the imported boundaries clears
it. `import_id`/`external_id` lineage is never lost.

Re-importing never reverts an adjustment: a project whose name already exists is left untouched
(reported as `skipped` or `unchanged`), and a refreshed span (catalogue evidence changed) does not
invalidate items that were extended past the old span — the clamp is the video, not the span. One
caveat: if you **delete** the project and re-import, the recipe is recreated with the catalogue's
original boundaries; the adjustment lived on the deleted project.

## Confirming imported evidence (Task 034)

Imported catalogue evidence (quality, standout, `used_in`) and imported finished projects become
preference evidence ONLY through an explicit confirmation in **Preferences → Imported evidence**.
Confirming is a two-step, reversible act: the first click adds the clips to a named previous-work
reference set (e.g. `Reel Studio · imported evidence`) that starts **unconfirmed** and therefore
inert; the second click confirms the set, using the same confirm / disable / delete machinery as
any other reference set — disabling or deleting it withdraws the evidence and invalidates anything
it influenced. Skip records a decision local to this device only; nothing is written to the library,
re-importing never resurrects a skipped clip as new, and a skipped clip can be brought back with its
Unskip button in Preferences.

Honesty note, stated in the panel itself: confirmed imported clips are **catalogued evidence**.
Spans carry no embedding vectors, so they do not train the current preference model — that starts
when clip (span interval) analysis lands. Nothing in the interface claims "learned" for them.

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

- Originals must already be indexed in Crush (`not_indexed` tells you which folder to add). A
  matched source whose duration was never probed (indexing never finished) is also reported as
  `not_indexed` per segment: span clips clamp to the source video, so a video of unknown length
  cannot host them until it is re-indexed.
- Catalogue text on spans is indexed for **text-match** search results and Review filtering
  (Task 034). Spans have no embedding vectors, so these results never join the semantic cosine
  ranking — they are labeled text matches with their catalogue provenance.
- Confirmed span evidence does not yet train the preference model (no vectors). It is stored,
  readable and reversible, and the trainer skips it without counting it as a sample.
- Spans cannot enter collections or version stacks yet; those filters exclude spans, and the
  pairwise compare dialog excludes spans too (prefer needs compared-media semantics and vectors).
- Reel v2 treatments beyond hard cuts (captions, music, motion, keyframed crops, extended grades)
  are stored faithfully but the Task 021 renderer refuses them with an explicit capability error
  rather than rendering an approximation.
- `used_in` is kept as publish history on the span; it is not turned into feedback events.
