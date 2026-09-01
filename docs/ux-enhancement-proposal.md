# UX enhancement proposal — Crush Mac app

Written 2026-09-01 against `task/21-render-export`. Read-only audit; this document is the only
file it touches. Every claim about current behavior cites file:line from `crates/app/ui/` or the
docs listed at the bottom. Nothing here was invented from imagination — where the app is
already good, it says so.

**Positioning is fixed and respected throughout:** fast, dense, local, editor-tool. A finder,
not a destination (docs/ux-spec.md). Nothing below turns Crush into a web app.

**Priority scale** (from docs/user-testing-macbook.md): P0 data-loss/blocker · P1 primary
workflow blocked or a misleading "learned/rendered" claim · P2 works but confusing/slow ·
P3 polish.

---

## 1. Current-state inventory

| Surface | What it does today | Top friction (from code/spec/test docs) |
|---|---|---|
| **Boot / first-run** (`index.html:20-42`, `app.js:89-175`) | Centered card with per-model progress bars, error text, Retry, Continue gated on all models present. Matches the Phase-1 spec line for line. | None found. This surface is in good shape — honest progress, real error text, no fake states. |
| **Library** (`index.html:81-130`, `app.js:228-395`) | Asset table (file, duration, resolution, status pill, shots), row selection, Re-index/Remove/Cancel/Import/Add Folder, failed-row chevron with job id + stage + log path + Copy details, drag-drop overlay, sidebar "Indexing N of M · %" footer. | Single-select only: Re-index and Remove work on one asset at a time (`app.js:237-238`). No thumbnails in the table, so video vs photo is text-only until you read the Shots column ("—" for photos, `app.js:295`). |
| **Search / All-assets browser** (`index.html:301-362`, `search.js`) | Launch view. Semantic search with ms-true latency readout, Top 25/50/100, All/Photos/Video segmented switch, browse mode showing the whole local library, "Why this result?" score breakdown, shortcut help panel. | The "Searching your library…" panel and the results grid can be visible at the same time on a re-search (`search.js:198,200`), inserting a full-height block above stale results instead of replacing in place (spec promise, `ux-spec.md:26`). Arrow-key navigation assumes exactly 4 grid columns (`search.js:838`) but the grid is `auto-fill` (`search.css:184-188`) — wrong at most window widths. |
| **Shot detail drawer** (`index.html:542-651`, `search.js:456-704`) | Third-column inspector: player with boundary-safe playback (In/Out clamp, loop, scrubber), mono timecodes, Copy path + timecodes, Export clip…, photo export, Reveal, Compare…, Pick/Reject/Rating/Standout, preference-evidence, safety flags, metadata, version stacks, transcript with highlighted query words, prev/next shot. | Very long single scroll — playback and export (the 20% used 80% of the time) sit above six always-expanded secondary blocks. Esc only closes it from Search (`search.js:814-823`); open it from Review and Esc does nothing. |
| **Review** (`index.html:410-539`, `library.js`) | Mixed photo/shot grid with kind badges, safety blur, status pills, flag pills; progressive-disclosure filters ("More filters") with removable active-filter chips; saved searches; batch bar (pick/reject/rate/add-to-collection); counts line. | **Collections are unreachable:** the batch "Add to collection…" select is never populated (`library.js:367,700-708` reference it; nothing fills it) and no UI anywhere calls `collection_create` (grep of `crates/app/ui/*.js` returns nothing). The user-test route step 4.3 — "Create a collection named `MacBook user test`" — cannot be performed. Filters also require a "Show results" click after every change (`library.js:613-616`). |
| **Compare dialog** (`index.html:654-678`, `review.js`) | Two-up A/B with full keyboard model (←→ focus, Enter prefer, p/x, 1–5, ⌥←→ swap B), seeded from the current Review filters, 300-asset pool cap with honest notice. | After a "prefer" the pair doesn't advance — an editor rating a run of candidates must manually swap B each time (`review.js:148-163` records but never advances). |
| **Preferences** (`index.html:364-408`, `style.js`) | Reference sets with Confirm/Disable/Delete (two-step), profile status line that can only ever say "General model only" or "Experimental preferences · human review pending", two-step Reset, Update recommendations. | Honest-state machinery is exemplary. Minor: the create form exposes "Context (default)" and scope before a new user understands what a context is (`index.html:385-398`) — a copy/sequencing issue, not a blocker. |
| **Projects** (`index.html:132-299`, `plans.js`) | Guided Step 1–4 flow: candidates in General/For-this-project columns with provenance pills and score evidence; sequence editor with boundary-clamped In/Out, preview player with boundary-safe controls; version save/restore; photo/clip/reel export with verification manifests; honest disabled states for photos-in-reel and imported spans. | In/Out are raw decimal-second number fields ("In (seconds)", `plans.js:491-493`) — editors think in timecodes. Reorder is Move up/Move down buttons only (`plans.js:528-537`). The four steps are one long scroll with no step navigation. Render progress is an indeterminate bar that never receives a value (`plans.js:301-318`). |
| **Import Reel Studio** (`index.html:689-739`, `import.js`) | Dry-run-first importer: pickers for catalogue/originals/recipes, advanced options folded away, Apply locked until a dry run of the *current* inputs has been shown, full mapping report with issues. | Genuinely good. Only polish-level friction: three separate picker clicks, no drag-drop of `clips.db`. |
| **Toasts / messages** | Five separate implementations of the same pattern: `app.js:81-87`, `search.js:121-138`, `library.js:104-112`, `style.js:48-56` (all 5 s auto-hide), `plans.js:31-35` (never hides). | Inconsistent behavior between views for identical situations; Plans messages accumulate forever. |
| **Empty / error states** | Every view has one: Library (`index.html:99-104`), Search nothing-indexed/idle/busy/no-matches/error (`index.html:332-348`), Review empty (`index.html:535-537`), Preferences empty (`index.html:401-404`), Projects empty + no-selection + items-empty (`index.html:147,150,242`). Harness-asserted (`empty` scenario). | Coverage is genuinely strong — this is a strength. Gaps: Review has no busy indicator while `library_browse` runs (`library.js:433-462`); raw backend error strings surface verbatim in most catch blocks (e.g. `library.js:459`, `style.js:105`) where the good pattern already exists ("Could not play X. Is the drive mounted?", `search.js:632`). |

---

## 2. Design-system audit — the craft layer

### Tokens: none exist

There are **zero CSS custom properties** in the four stylesheets (`var(` never appears). Every
value is a magic literal, repeated by hand:

- **Color: 303 hex occurrences, 108 distinct hex values** across `styles.css` (158), `search.css` (65), `plans.css` (59), `import.css` (21). The spec's palette (`ux-spec.md:8`) is nominally followed, but the long tail is real: 68 of the 108 values appear exactly once. `plans.css` uses an entire off-palette blue (`#719ef8` ×5, plus `#26334a`, `#253041`, `#26334a` chips) that exists nowhere else.
- **Type: 15 distinct font-size values** (9–34 px). `11px` alone appears 32 times. The spec says "Base 13 px" (`ux-spec.md:7`) but 13 px appears only 4 times; the de-facto body size is 11–12 px. Monospace is declared three different ways (`styles.css:172`, `styles.css:611`, `search.css:169` — `"SFMono-Regular", Consolas` vs `ui-monospace, SF Mono, Menlo`).
- **Spacing: 27 distinct px values** in padding/margin/gap (8 px ×48 and 10 px ×34 dominate, but 3, 5, 7, 9, 22, 26 px one-offs are scattered through).
- **Radius: 14 distinct border-radius values** (3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 16 px, 999 px, 50%, inherit).

### Component consistency: patterns exist, then drift

- **`.button.small` is defined twice with different values** — `search.css:178` (min-height 26 px) and `import.css:25` (min-height 28 px). Both files load (`index.html:7-10`); import.css wins globally, silently resizing every small button app-wide. This is the clearest evidence the layers have drifted.
- **Six pill/badge patterns**: `.status-pill` (`styles.css:537`), `.review-flag-pill` (`styles.css:1031`), `.plans-pill` (`plans.css:54`), `.active-filter-chip` (`styles.css:870`), `.badge` (`search.css:250`), `.review-kind` (`styles.css:995`) — similar jobs, six different padding/radius/size combos.
- **At least 8 input/select styles**: `#plans-view input` (radius 5, bg `#17171b`, `plans.css:12`), `.style-create-form input` (radius 7, bg `#141416`, `styles.css:720`), `.filter-control select` (height 30, bg `#26262b`, `styles.css:895`), `.detail-metadata input` (`styles.css:1096`), `#search-input` (height 38, radius 9, `search.css:23`), `.import-picker input` (height 32, `import.css:7`), `#detail-style-set` / `.stack-controls select` / `.compare-side select` (radius 6, bg `#24242a`, `styles.css:813,1112,1220`), `.project-reel-export select` (radius 5, bg `#17171b`, `plans.css:28`).
- **What is consistent and should be kept as the seed of the system**: `.button` variants (`styles.css:216-253`), the dialog pattern (`.doctor-dialog` reused by confirm/remove/import), the toast pattern, the empty-state pattern, and the status-pill tone system (done/active/failed). The bones are good; the audit is about naming what already works.

### Accessibility

- **Focus states: 3 rules total.** `#plans-view input:focus` (`plans.css:13`), `#search-input:focus` (`search.css:44`), and `.compare-side.focused` — which is a class, not a focus state. No `:focus-visible` anywhere. `.result-card` sets `outline: none` (`search.css:196`) with no replacement. Buttons, nav items, table rows, and review tiles rely on whatever the UA defaults to.
- **Keyboard vs the spec's promises** (`ux-spec.md:9`):
  - Cmd-F focus search — **wired**, global (`search.js:801-808`). ✓
  - Esc clears/closes — **Search view only** (`search.js:814-823`). Detail drawer opened from Review/Library/Plans cannot be closed with Esc. (History note: the 2026-08-30 review flagged that this handler had *lost* its Search-view guard and the guard was deliberately restored — `docs/HANDOFF.md:154-157`. Any Esc enhancement must be a scoped addition, not a guard removal.)
  - ↑↓ through results — **wired but wrong**: hardcoded `columns = 4` (`search.js:838`) vs an auto-fill grid (`search.css:186`); at window widths giving 5–6 columns, up/down jumps the wrong distance. Selection also never moves DOM focus (`search.js:445-454` toggles classes only), so screen readers don't track it.
  - Enter opens detail — ✓ in Search (`search.js:844-851`).
  - Space play/pause — ✓ in detail (`search.js:829-831`); Library rows also support Enter/Space select (`app.js:253-258`). ✓
  - **Not covered anywhere**: Review tiles are plain divs with click handlers — no tabindex, no role, no key handler (`library.js:313-351`); a keyboard user can reach the select checkbox but cannot open an asset. Library table rows are focusable but have no ↑↓ movement. No view-switch shortcuts exist.
- **Contrast on muted text** (computed WCAG ratios against the actual backgrounds): the workhorse muted `#8a8a93` on `#141416` ≈ **5.4:1 — passes AA**. But `#6f6f78` ≈ **3.7:1** and `#777781` ≈ **4.15:1 — both fail AA (4.5:1)**, and they carry the densest metadata in the app: file paths under table rows (`styles.css:526`), transcript timecodes (`search.css:517`), search placeholder (`search.css:41`), compare hint (`styles.css:1237`), table headers (`styles.css:448`), doctor link (`styles.css:346`), stack-membership empty text (`styles.css:1156`). On the detail panel's `#1d1d21` background, `#6f6f78` drops to ≈ 3.4:1.
- **Reduced motion: zero** `prefers-reduced-motion` rules anywhere. Exposure is small (4 transitions total: progress fill, chevron, two opacity fades) so this is a 10-line fix, not a retrofit.

---

## 3. Interaction/workflow audit — the smoothness layer

### Latency feedback

- The spec promises "results replace in place; no spinner under 500 ms" (`ux-spec.md:26`). Reality: a 160 ms debounce (`search.js:209`), then the busy panel shows **immediately** on every search (`search.js:198`). On a *first* search that's the only feedback — correct. On a *re-search*, `state.results` still holds the previous results, so grid and busy panel render together and the full-height "Searching your library…" block (`styles.css:400-406` min-height) shoves the stale grid down the page. That is the opposite of "replace in place."
- The good news worth keeping: the result count reports real measured latency ("3 matches · 42 ms", `search.js:236-239`) — exactly the honest instrument an editor tool should have.
- The user-test doc warns "the first search can be slower while the local encoder initializes" (`user-testing-macbook.md:90-91`). The UI gives no hint of this; a slow first search looks identical to a hang.
- Long renders (reel export) show an indeterminate `<progress>` that never receives a value and static text (`plans.js:301-318,782`). Honest, but a multi-minute reel render gives zero progress signal.

### State coverage per view

Strong overall (see inventory table). Remaining gaps: Review has no busy state during refresh; the detail drawer shows the previous asset's content until the next `shot_detail`/`photo_detail` resolves (fine locally, jarring on a cold drive); Plans messages never auto-hide while every other view's do.

### Click-depth, top 5 workflows

1. **Find a shot → export a clip**: ⌘F → type → Enter → "Export clip…" → save dialog. Two clicks plus typing. **Good as built.**
2. **Build a project → render**: create project → Find suggestions → Add candidate(s) → set In/Out → Save item → scroll to Step 4 → Choose… → Render. ~8 interactions across one long scroll with no step anchors; In/Out in decimal seconds is the biggest single friction. **Workable, needs smoothing.**
3. **Review → standout**: Review → tile → Standout checkbox. Two clicks. **Good.**
4. **Import Reel Studio**: Library → Import… → 3 pickers → Dry run → read → Apply. Six deliberate clicks for a destructive-adjacent operation — the friction is the honesty, and that's correct. **Good.**
5. **First-run → first search**: automatic; search view self-focuses when the shell appears (`search.js:927-934`). **Good.**

### Keyboard-first gaps (summary)

Esc dead outside Search; arrow columns wrong; Review grid mouse-only; no roving focus in the results listbox; no view-switch keys. The compare dialog, by contrast, is exemplary (`review.js:179-218`) and should be the template.

### Known friction from the 2026-08-29/30 reviews that remains

- Landed and verified in code: Plans→Projects, Style→Preferences, More-filters disclosure, removable filter chips, boundary-safe playback, Standout, Pick/Reject/Rating filters, treatment warnings, searching state, editor-language status labels, timecodes, shortcut help, detail reopen fix (harness `feedback` guards it, `ui-harness.mjs:556-567`).
- **Still open and UI-relevant:** collections cannot be created from the UI at all (see P1 below); raw backend error strings surface verbatim; first-search warmup is silent; render progress is indeterminate. (The executor-side items — cancel-before-published ordering, swallowed `render_job_fail` — belong to the 021 owner per `docs/HANDOFF.md:154-157` and are out of scope here.)

---

## 4. The proposal — three tracks

Effort: **S** ≤ half a day · **M** 1–3 days · **L** 3+ days. "Harness" lists which of the 24
scenarios in `scripts/ui-harness.mjs` assert affected copy/DOM. "Backend" marks items that
need more than a bridge call that already exists — those become tasks, not CSS.

### Track A — Design-system pass

| # | Pri | Item | What changes | Why | Effort | Harness | Backend |
|---|---|---|---|---|---|---|---|
| A1 | P2 | **Token extraction** | Add `:root` custom properties for color (bg/panel/text/muted tiers/accent/danger + the plans blue folded into accent), spacing scale, type scale, radius scale, motion durations, focus ring. Then mechanically replace literals. 4 files touched, ~0 JS. | 108 one-off hex values and 15 type sizes are why every new panel re-invents styles; tokens make Track C cheap. | M | None assert computed styles; `library-grid` asserts player 16:9 and `app-shell` classes — preserve layout exactly. | No |
| A2 | P2 | **Component consolidation** | One pill/badge family, one input/select family, one toast helper (single module, per-view mount point, 5 s auto-hide for confirmations, sticky for errors). Delete the duplicate `.button.small` (keep one height). | Kills the six-pill/eight-input drift and the silent `search.css:178` vs `import.css:25` override. | M | None assert button heights or toast timing; `plans-errors` reads `#plans-message` text — keep the element. | No |
| A3 | P2 | **Focus-visible pass** | One `:focus-visible` token (2 px accent ring, offset) applied to buttons, nav, tiles, cards, rows; remove the bare `outline: none` on `.result-card` or pair it with a selected style. | Only 3 focus rules exist today; keyboard users currently get UA defaults or nothing. | S | None. | No |
| A4 | P2 | **Contrast fix for metadata text** | Lift `#6f6f78` → `#8a8a93`-tier and `#777781` → ≥4.5:1 equivalent for the 10–11 px metadata roles (paths, timecodes, headers, hints). | The densest, most-read text in the app currently fails AA (≈3.7:1 and ≈4.15:1). | S | None — colors only, copy untouched. | No |
| A5 | P3 | **Reduced motion** | `@media (prefers-reduced-motion: reduce)` zeroing the 4 transitions. | Standard care; trivial now, annoying to retrofit later. | S | None. | No |
| A6 | P3 | **Palette reunification** | Replace `plans.css`'s `#719ef8` family with the accent token; align `.workflow-step`, focus rings, preview-highlight. | One view speaking a different blue undermines the "one tool" feel. | S | None (visual only). | No |

**Track A totals:** 0 × P0, 0 × P1, 4 × P2, 2 × P3. Harness impact overall: **zero scenarios
assert anything these change** — this track is the safest place to start.

### Track B — Workflow smoothness pass

| # | Pri | Item | What changes | Why | Effort | Harness | Backend |
|---|---|---|---|---|---|---|---|
| B1 | **P1** | **Make collections reachable** | Populate the batch "Add to collection…" select from `collection_list` (the filter select already is, `library.js:158-162`); add an inline "New collection…" entry that calls the existing `collection_create` command, then batch-add via the existing `review_batch` op. | Primary organizational workflow is blocked: you cannot create a collection anywhere in the UI, and the batch target list is permanently empty — the user-test route's step 4.3 is impossible today. Commands already exist in the bridge. | S | Add one scenario; existing ones unaffected. | No — commands exist |
| B2 | P2 | **Esc closes the detail drawer everywhere** | Scoped handler: if the drawer is open, Esc closes it, regardless of view — an *addition* that keeps the Search-view guard for the clear-search behavior intact (per the 022 fix history, `docs/HANDOFF.md:154-157`). | Spec promises "Esc clears/closes detail" (`ux-spec.md:9`); today it only works in Search. | S | None assert Esc outside Search; `feedback` asserts the close *button*, which stays. | No |
| B3 | P2 | **Correct arrow-key columns** | Compute columns from `getComputedStyle(grid).gridTemplateColumns` instead of the hardcoded 4 (`search.js:838`). | Arrow navigation jumps the wrong row at any window width that isn't exactly 4 columns. | S | None assert grid arrows. | No |
| B4 | P2 | **Review grid keyboard access** | Roving tabindex on tiles + Enter/Space opens the drawer; mirror the results-grid pattern. | Review tiles are mouse-only (`library.js:313-351`); the spec's keyboard posture should hold in Review too. | M | None; clicks unchanged. | No |
| B5 | P2 | **In-place re-search** | Show the busy panel only when there are no stale results to keep in place (`search.js:198` — gate on `!hasResults` too); keep old results visible until new ones land. | Fulfills the spec's "results replace in place" and kills the full-height layout jump on every re-search. | S | `dam-home` asserts titles/counts, not busy visibility — safe. | No |
| B6 | P2 | **Auto-apply Review filters** | Apply on filter change (debounced); keep the "Show results" button as the explicit fallback. | Today every filter tweak needs an extra click (`library.js:613-616`) — death by a thousand applies in a review session. | S | `library-grid`, `library-feedback`, `library-saved-search` click `#filter-apply` — keep the button and they still pass. | No |
| B7 | P2 | **First-search warmup honesty** | If the first search exceeds ~1.5 s, extend the busy copy: "Loading the local encoder — later searches are faster." | The test doc documents this exact stall (`user-testing-macbook.md:90-91`); silence reads as brokenness. | S | `search-error` asserts error text — ensure the timer never delays error rendering. | No |
| B8 | P2 | **Error-language pass** | Map common backend errors to editor language with the detail preserved (extend the existing good pattern at `search.js:632`); keep raw text in a "Copy details" affordance like the Library failure row. | Raw strings like "Disk full" surface verbatim in Review/Preferences/Projects catches (`library.js:459`, `style.js:105`, `plans.js:80`). | M | `plans-errors` asserts `/Disk full/`, `/JSON object/`, `/Out must be after In/` — either keep those substrings in the new copy or update the scenario. | No |
| B9 | P3 | **Plans message parity** | Auto-hide informational messages after 5 s; keep errors sticky until resolved. | `plans.js:31-35` never hides; every other view hides at 5 s. | S | Safe — harness clocks are frozen (`ui-harness.mjs:8-9`), so 5 s timers never fire mid-test. | No |
| B10 | P2 | **Render progress %** | Stream real render progress into the photo/reel `<progress>` elements (they currently never receive a value, `plans.js:301-318`). | A multi-minute reel render with an indeterminate bar fails the "honest states" bar from the wrong side — the user can't tell progress from a hang. | M | `plans-editor`/`plans-errors` assert status text, not progress values — safe. | **Yes** — needs a render-progress event on the bridge; file as a task |
| B11 | P3 | **Library row ↑↓ navigation** | Arrow keys move row selection in the Library table. | Rows are focusable (`app.js:250`) but only tab/click moves between them. | S | None. | No |
| B12 | P3 | **Review refresh busy cue** | `aria-busy` + slight dim on the grid while `library_browse` runs. | No feedback between "Show results" and the grid updating on large libraries. | S | None. | No |

**Track B totals:** 0 × P0, **1 × P1** (B1), 7 × P2, 3 × P3. Only B10 touches backend.

### Track C — View-by-view enhancements (structural ideas)

| # | Pri | Item | What changes | Why | Value / Effort / Risk | Harness | Backend |
|---|---|---|---|---|---|---|---|
| C1 | P2 | **Projects step navigation** | Sticky step rail (1 Find · 2 Arrange · 3 Version · 4 Export) that scroll-links and ticks off completed steps; add a "next step" affordance after adding items. Do **not** collapse steps into accordions — the harness asserts Step 3/4 controls are visible without interaction. | The guided flow exists but is one long scroll; after Step 1 the editor must hunt for Step 4. | High / M | `plans-editor`, `plans-general`, `plans-errors`, `plans-historical`, `plans-span-export` all assert Step 2–4 elements — anchor links keep everything rendered; collapsing would break 5 scenarios. | No |
| C2 | P2 | **Timecode-format In/Out** | Keep storing seconds (contract unchanged) but accept `00:00:03.12` *or* `3.45` in the In/Out fields and show the converted timecode beside the field; preview label already shows `0:00 / 0:00` style. | Editors think in timecodes; the spec itself formats everything else in `HH:MM:SS.ff` (`ux-spec.md:29`). Decimal seconds are machine vocabulary. | High / M | `plans-historical` asserts `inputValue() === "3.45"` and `plans-editor`/`plans-errors` fill raw seconds — keep accepting/filling seconds so all three pass untouched. | No |
| C3 | P3 | **Drag to reorder** | HTML5 drag-and-drop on sequence items; keep Move up/Move down buttons. | Reordering a 20-item reel by button clicks is slow; drag is the editor's reflex. | Med / S | `plans-editor` uses Move up — keep the buttons and it passes. | No |
| C4 | P2 | **Compare auto-advance** | After "prefer", advance side B to the next candidate (toggle, default on) so a rating run flows. | Today each verdict requires a manual B swap (`review.js:148-163`), which turns a 50-asset cull into 100+ keystrokes. | High / S | `compare-view` presses Enter then `p` and asserts the *same* asset gets picked — auto-advance would change the target. Ship default-off, or update the scenario. | No |
| C5 | P2 | **Library batch operations** | Multi-select rows → batch Re-index / Remove (confirmation lists count). | Re-indexing or cleaning up 30 files is 30× the clicks today (`app.js:237-238`); Review already proves the batch-bar pattern. | Med / M | `photo-row`, `ingest-cancel`, `failed-row`, `recovered-row` assert single-row behaviors — keep single-select semantics working. | **Yes, preferred** — batch `reindex`/`remove` commands (a loop of single invokes works but is chatty); file as a task |
| C6 | P3 | **Detail drawer progressive disclosure** | Collapse Safety flags / Metadata / Version stacks into `<details>` (open on first use, remember per session); playback, timecodes, export, and feedback stay on top. | The drawer is the app's most-used surface and currently scrolls through six always-open secondary blocks (`index.html:581-650`). | Med / S | `library-flags` unchecks `#safety-blur` and `style-add-item` selects `#detail-style-set` — both need visibility. Ship with sections default-open on first run, or update those two scenarios. | No |
| C7 | P3 | **Library thumbnails** | 16:9 thumb cell in the Library table (photos/videos visually distinguishable at a glance, per the test route's step 2.1). | The table is text-only today; the test doc explicitly asks whether video and photo cards are visually distinguishable. | Med / S | `photo-row` asserts file-name and shots-column text — an added column is safe. | **Possibly** — confirm `list_videos` already returns a thumb path; if not, backend task |
| C8 | P3 | **Kind filter honesty in search** | When Photos/Video is active during a semantic search, either pass the kind to the `search` command or label the count "of N matches". | Kind filtering of search results is client-side post-filtering (`search.js:874-885`), so "Top 50" can silently mean "Top 50, then filtered". | Med / S | `dam-home` asserts counts after kind switches — coordinate any copy change with it. | **Yes** if done server-side; the label fix alone is frontend-only |

**Track C totals:** 0 × P0, 0 × P1, 4 × P2, 4 × P3. Backend-touching: C5 (preferred), C7 (maybe), C8 (server-side option).

---

## 5. Sequencing recommendation

The release candidate is one human verdict from merging (Task 021 render-golden review).
**Nothing here should touch the RC.** Everything below assumes post-merge work on this branch's
successor, one task per PR, harness green before each merge.

**Week 1 (high-value, low-risk — all frontend-only, near-zero harness impact):**
B1 collections wiring (the only P1), A4 contrast, A3 focus-visible, A5 reduced-motion,
B2 Esc, B3 arrow columns, B5 in-place re-search, B6 auto-apply filters, B9 plans-message
parity, B11/B12. These are small, independently shippable, and each one retires a named
friction or an accessibility failure with copy the harness doesn't pin.

**Second wave (needs a full harness run and design attention):** A1 token extraction and
A2 component consolidation as one CSS-only PR (run all 24 scenarios; they assert no styling,
so risk is layout regressions only — `library-grid`'s 16:9 assertion is the canary), then
A6, B4, B7, B8, C1, C2, C3, C6. C4 ships default-off or with a harness update.

**Needs John's product decisions first (flagged, not built):**
1. **C5 Library multi-select/batch** — the Phase-1 spec explicitly deferred multi-select
   (`ux-spec.md:45`); it now conflicts with real usage. Batch commands are backend tasks.
2. **C4 compare auto-advance default** — changes the muscle memory of the compare flow.
3. **C8 kind-filter semantics** — client-side filtering of ranked results is quietly
   un-TOP-N; decide whether `search` should take a kind argument (backend contract).
4. **C7 Library thumbnails** — confirm whether `list_videos` already exposes thumbs.
5. **Ratify the spec drift**: the shipped search placeholder ("Search subjects, actions,
   places, or mood…", `index.html:306`) replaced the spec's "Describe the shot…" copy
   (`ux-spec.md:22`). The shipped copy is better editor language — update the spec, not the app.

**Standing rules honored throughout:** editor language (B8, C2), progressive disclosure
(C6, and the existing More-filters pattern left intact), no new permanent dropdowns for
discoverability (B1 reuses the existing batch bar; nothing here adds a dropdown),
honest states everywhere (B7, B10 — real progress or none; no fake completion; the
"Experimental · human review pending" boundary is untouched and nothing in this proposal
touches the learned-claim gate).

---

### Sources

- `docs/ux-spec.md` — Phase-1 spec (keyboard promises line 9, latency line 26, colors line 8, "Not in Phase 1" line 45)
- `docs/user-testing-macbook.md` — test route, P0–P3 scale (lines 150–158), first-search warmup (lines 90–91), collection step (line 103)
- `docs/review-2026-08-29.md` — direction-setting review
- `docs/HANDOFF.md` — product direction (lines 10–22), 022 continuation and Esc-guard history (lines 153–158)
- `crates/app/ui/` — index.html, styles.css, search.css, plans.css, import.css, app.js, search.js, library.js, plans.js, style.js, review.js, import.js (all line citations inline)
- `scripts/ui-harness.mjs` — the 24 scenarios and their asserted copy/DOM
- `crates/app/tests/mock-bridge.js` — bridge command surface used to mark backend vs frontend items
