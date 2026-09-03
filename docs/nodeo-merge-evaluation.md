# nodeo → crush merge evaluation

**Date:** 2026-09-03 · **Status:** Initial merge evaluation (John's 2026-09-02 directive) — proposal for John's decision
**Decision being recorded:** crush and nodeo are ONE product. nodeo's capabilities fold into crush as a capability port to Rust — no server, no bridge, no standalone nodeo release. The merge is the substance of crush's **next release (0.1.0)**, after the 0.0.1 release candidate clears clean-machine acceptance.
**Inputs read:** `docs/HANDOFF.md`, `docs/project-blueprint.md`, `docs/dam-feedback-blueprint.md`, `docs/platform-architecture.md`, `docs/release.md`, `docs/release-record-0.0.1.md`, `docs/smoke.md`, `TASKS.md`, `crates/store/migrations/*`, `crates/core/src/{config,job}.rs`, `crates/pipeline/src/lib.rs`, `crates/store/src/lib.rs` (relink section), `crates/app/src-tauri/src/lib.rs` (command table), `crush.example.toml`, workspace `Cargo.toml`, `tauri.conf.json`; nodeo `CAPABILITY_AUDIT.md`, `LLAVA_IMPROVEMENTS.md`, `app/ai/llava_client.py`, `app/ai/project_classifier.py`, `app/services/{rename_engine,template_parser,folder_watcher}.py`, `app/config.py`; OpenRouter API reference (fetched 2026-09-03).

---

## 1. Executive summary

**Decision (John, 2026-09-02):** crush and nodeo are one product. Nodeo's capabilities fold into crush as a capability port to Rust — no server, no bridge, no standalone nodeo release. The merge is the substance of crush's **next release (0.1.0)**, after the 0.0.1 release candidate clears clean-machine acceptance.

**What nodeo actually has** (per its own honest `CAPABILITY_AUDIT.md`, confirmed by reading the code): a production-ready LLaVA vision pipeline (single-call structured JSON: description, 5–10 tags, objects, scene, mood, colors), a production-ready template rename engine (70+ variables, 16 predefined templates, preview/backup/rollback), and a folder watcher that is 90% plumbing around a **stubbed** `_process_file()` (`app/services/folder_watcher.py` lines 324–337). Its audio and video "support" was never implemented — that part of the narrative was aspirational, and the audit says so.

**What crush already has that nodeo only claimed:** whisper-rs transcription with Metal, scene detection, CLIP embeddings per shot, video thumbnails, transcripts in FTS, and a hash-verified, transactional relink flow that is exactly the safety primitive a crush-performed rename needs.

**Recommendation (one paragraph):** Port nodeo's two real capabilities — vision description and template renaming — into crush as a new `describe` stage and a safety-hardened rename flow, behind a small local-first AI provider abstraction (`crush-ai` crate): local Ollama on the LAN is the preferred path, OpenRouter is a strictly opt-in, off-by-default, Keychain-stored-key remote option with cost guardrails, and "no provider configured" is an honest capability error that leaves every other crush feature untouched. Rule out nodeo's server stack and cloud integrations entirely; defer the folder watcher and project classification honestly (the watcher never processed a file, so nothing proven is lost). Sequence the work as TASK-041…047, each one branch/one PR with Linux+macOS CI, golden tests for the template engine (nodeo's Python parser becomes the answer key), and zero changes to render recipes or approved render paths so the 0.0.1 RC stays byte-stable. The release lands as 0.1.0 with its own human acceptance gate.

---

## 2. Capability map

| # | nodeo capability | Nodeo state (verified) | Crush disposition | Where it lands |
|---|---|---|---|---|
| 1 | LLaVA image analysis — description, 5–10 tags, objects, scene, mood, colors | Production-ready (`app/ai/llava_client.py`, single-call JSON + legacy fallback) | **PORT to Rust** | TASK-041/043 — `crush-ai` provider layer + `describe` stage |
| 2 | Concurrent batch analysis (semaphore, default 5) + JSON repair (markdown-fence stripping, tag normalization) | Production-ready (`LLAVA_IMPROVEMENTS.md`) | **PORT as design guidance** — bounded worker count, versioned prompt, per-item honest failure | TASK-043 |
| 3 | Template rename engine — 70+ variables, sanitization rules, 16 predefined templates | Production-ready (`app/services/template_parser.py`) | **PORT to Rust, golden-tested** — nodeo's Python parser becomes the answer key | TASK-044 |
| 4 | Rename preview / apply / backup / rollback | Production-ready (`app/services/rename_engine.py`) — but backup = full file **copy** (`.backup_` prefix), rollback restores from copies | **PORT with strengthened safety** — opt-in, previewed, reversible by *inverse rename* (no multi-GB copies), hash-verified atomic catalog update via the existing `relink` machinery | TASK-045 |
| 5 | Project classification (LLM keyword/theme scoring, 0.70 auto-assign, 0.50 review) | Exists (`app/ai/project_classifier.py`) | **DEFER** — crush already has Projects and collections; the valuable piece (suggest a collection from AI tags) is a later task, not release-blocking | Post-0.1.0 |
| 6 | Folder watching (watchdog infra, queue, WebSocket updates) | **90% infra, `_process_file()` is a stub** — files were detected but never processed | **DEFER** — crush has no watch folder (blueprint Task 14, never built). When built: `notify` crate driving the same describe stage. Nothing proven is lost by deferring | Post-0.1.0 |
| 7 | Audio transcription ("Phase 2 planned") | **0% — never implemented** (audit: "the entire audio analysis stack is missing") | **ALREADY EXISTS, crush is strictly ahead** — `whisper-rs` (Metal), `transcripts` table, `transcripts_fts` (schema v1) | Nothing to port. Nodeo's roadmap idea of `{transcript}` template variables is a cheap later add using existing transcripts |
| 8 | Video analysis ("Phase 3 planned") | **5% — file handling + ffprobe metadata only** | **ALREADY EXISTS, crush is strictly ahead** — HSV scene detection, CLIP per-shot vectors, thumbnails (TASK-040), transcripts. LLaVA-on-shot-rep-frames is a natural describe-stage extension, **deferred** (schema's `media_kind` already allows it) | Deferred extension |
| 9 | Nextcloud WebDAV sync | Production-ready in nodeo (`app/storage/nextcloud*.py`) | **RULED OUT** — violates the no-cloud posture (`project-blueprint.md` §6 "Not Yet: cloud anything"; §21 privacy) | — |
| 10 | Cloudflare R2 / Stream | Production-ready in nodeo, flag-gated off | **RULED OUT** — same posture | — |
| 11 | Media metadata probing (ffprobe / magick identify) | Production-ready in nodeo | **ALREADY EXISTS** — bundled ffprobe sidecar, `video_source_metadata` / `photo_source_metadata` | Nothing to port |
| 12 | AI grouping (tag/scene/embedding clusters → ImageGroups) | Exists (`app/services/grouping.py`) | **COVERED** — crush has collections, stacks, and CLIP search; nodeo's grouping is redundant | — |
| 13 | FastAPI/React/Postgres/Redis/Docker stack, WebSocket push | The nodeo product itself | **RULED OUT** (John, 2026-09-02): capability port into Rust, never a service. The app's existing background-task + `jobs`-table patterns replace WebSocket push | — |
| 14 | Activity log | Database + REST in nodeo | **ALREADY EXISTS** — the `jobs` table is the debugging spine (every stage run gets a row) | — |

**Net:** two real ports (vision describe, template rename + apply), two honest defers (watcher, project classification), three ruled-out categories (server stack, cloud, redundant grouping), and two areas where nodeo's claims were aspirational and crush already ships more.

---

## 3. AI provider architecture

### 3.1 Shape

A new small crate, **`crates/ai` (`crush-ai`)**, depending only on `crush-core`. This follows the workspace convention of one crate per concern (`core` = contracts, `stage-*` = pipeline stages, `pipeline` = orchestration). The provider layer is a *service used by* a stage and by app commands, not a stage itself.

```rust
/// One vision capability, honestly reported. No capability → no method call.
pub trait VisionProvider: Send {
    fn id(&self) -> &'static str;                      // "ollama" | "openrouter"
    fn model(&self) -> &str;
    /// Structured extraction; the only AI operation in 0.1.0.
    fn describe_image(&self, req: &DescribeRequest) -> anyhow::Result<ImageDescription>;
}

pub struct DescribeRequest {
    pub image_path: PathBuf,     // stage reads from a path; provider reads bytes itself
    pub prompt_version: &'static str,
    pub temperature: f32,        // 0.3 default, per nodeo's tuned finding
    pub max_tokens: u32,         // 300 default, per nodeo's tuned finding
}

pub struct ImageDescription {
    pub description: String,
    pub tags: Vec<String>,       // lowercased, ≤10, per nodeo normalization
    pub objects: Vec<String>,
    pub scene: String,
    pub mood: Option<String>,
    pub colors: Option<Vec<String>>,
}
```

`provider = "none"` (the default) yields a `NoneProvider` whose `describe_image` returns the standing honest capability error, so callers never need `Option` plumbing:

> *"AI description is not available: no vision provider is configured. Set up local Ollama in Preferences (recommended). Nothing else in Crush is affected."*

This is the same posture as imported-span treatments: unsupported capability → explicit, honest error, never a silent fallback.

### 3.2 Local Ollama backend (preferred path)

Nodeo's client goes through the Python `ollama` lib; the wire protocol it produces is plain HTTP, which Rust speaks with the **already-pinned `ureq` 3.4.0 (rustls)** from the workspace manifest — no new HTTP dependency, no async runtime (see trade-off below).

- **Endpoint:** `POST {host}/api/chat`
- **Body:** `{"model": "...", "messages": [{"role": "user", "content": <prompt>, "images": ["<base64>"]}], "options": {"temperature": 0.3, "num_predict": 300}, "stream": false}`
- **Response:** `{"message": {"content": "<json|text>"}}`
- **Host:** configured, not auto-discovered. Default suggestion `http://192.168.50.247:11434` (ai-srv per the estate table). Note: nodeo's `app/config.py` default is `192.168.50.248` — stale against the current estate; do not carry it over. `crushctl doctor` gains a provider check: host reachable, model present (`GET /api/tags`), reported as evidence, **not** a failure when absent.
- **JSON robustness:** port nodeo's hard-won details verbatim — markdown code-fence stripping, `tags`-as-string fallback, lowercase/limit normalization — with the *fast method only*. Nodeo's legacy 4-call fallback existed because LLaVA sometimes broke JSON; in crush a malformed response is a **per-item honest failure** (job row + error text), and the user retries or switches models. No silent multi-call retry that doubles latency and cost.
- **Concurrency:** bounded worker count in config (default 2, max ~4) so a batch doesn't saturate the single Ollama instance on ai-srv.

### 3.3 OpenRouter backend (opt-in remote, off by default)

Verified against OpenRouter's API reference (2026-09-03):

- **Endpoint:** `POST https://openrouter.ai/api/v1/chat/completions`
- **Headers:** `Authorization: Bearer <key>`, `Content-Type: application/json`; optional `HTTP-Referer` / `X-Title` (attribution — optional, can be omitted).
- **Vision message format** (user-role content parts):
  ```json
  {
    "model": "<vendor/model>",
    "messages": [{
      "role": "user",
      "content": [
        {"type": "text", "text": "<same versioned prompt>"},
        {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,<...>"}}
      ]
    }],
    "temperature": 0.3,
    "max_tokens": 300
  }
  ```
  Base64 data URL is correct here because the image lives on the machine — no hosting needed.
- **Response:** `choices[0].message.content` (same JSON payload to parse), plus `usage.prompt_tokens` / `completion_tokens` / `total_tokens` and `usage.cost` (credits) — **always returned for non-streaming requests**, which is what makes cost guardrails real rather than estimated. `GET /api/v1/generation?id=<id>` exists for post-hoc audit.
- **Model selection:** configurable string in `crush.toml` / Preferences. Recommended default **`google/gemini-2.5-flash`** (fast, cheap, vision-capable, stable vendor prefix on OpenRouter) — *confirm it exists in the catalog at implementation time; the catalog moves, which is exactly why the model is a config string, not a constant.* `crushctl ai check` (new) verifies the key works and the configured model is offered (`GET /api/v1/models`).
- **Key storage: macOS Keychain via the `keyring` crate** (service `dev.crush.app`, account `openrouter`). Never in `crush.toml`, never plaintext. `CRUSH_OPENROUTER_API_KEY` env override for CLI/CI use. `keyring` also covers Windows Credential Manager and Linux Secret Service, which keeps the TASK-028–031 Windows track open. **New dependency — justification:** crush has no secret storage today; the alternative (plaintext config) is worse, and the direct `security-framework` crate would be macOS-only and more code for the same result. This is a blueprint-edit-level stack note.
- **Coexistence with local-first:** OpenRouter is **off by default**, never selected automatically, and **never a silent fallback** when Ollama fails — a failed local call produces an error that says the remote option exists; sending images to OpenRouter only ever happens after the user turns it on and picks it for that run. Every OpenRouter result is stored with `provider='openrouter'` and labeled in the UI: **"Analyzed via OpenRouter — this image left your machine."** This exercises the clause already in `project-blueprint.md` §21 ("if ever added, opt-in with a visible toggle and a one-line privacy note") and requires the matching edit to the Privacy section of `docs/release.md`.
- **Cost guardrails:** (1) before a batch over the local provider fails over — it doesn't; (2) for OpenRouter batches: run **one** image, read `usage.cost`, extrapolate × remaining count, show the estimate, **require explicit confirmation above a configured threshold** (`estimate_confirm_usd`, default $0.50); (3) hard cap per run (`max_cost_per_run_usd`, default $1.00) — the batch aborts honestly mid-way when cumulative `usage.cost` crosses it, with completed items kept and the remainder reported as not-run; (4) cumulative spend per batch recorded alongside the job row.

### 3.4 Trade-offs stated

| Choice | Alternative | Why |
|---|---|---|
| `ureq` (sync, pinned, rustls) | `reqwest` + tokio | The whole pipeline is synchronous by design (blocking work on background threads, `spawn_blocking` at the app boundary). reqwest drags in an async runtime for zero benefit. ureq is already in the dependency tree, pinned, and proven by the model downloader. |
| `keyring` crate | plaintext file / env-only / security-framework direct | Keychain is the requirement; keyring is the boring cross-platform wrapper. Env var covers headless CLI use. |
| Configured host + doctor check | mDNS/DNS-SD auto-discovery of Ollama | Discovery is magic that fails confusingly; crush's posture everywhere is *evidence, not guessing*. John has exactly one Ollama host. |
| Fast JSON method, per-item failure | nodeo's legacy 4-call fallback | 4× the latency as a *hidden* retry is dishonest about cost/time; a visible per-item failure with retry is crush's language. |

---

## 4. Vision → rename pipeline design

### 4.1 Fit with the stage architecture

The rule stands (`HANDOFF.md`): *stages read from the store or a path and write to the store; no in-memory hand-offs.* A new **`Describe`** variant joins the `Stage` enum (`crates/core/src/job.rs` — currently `Split, Embed, Analyze, Transcribe, PhotoIngest`):

```
photos / videos row  (identity = owner_id + sha256; path is just the current pointer)
   │  reads: thumbnail or original path from the row
   ▼
Describe stage  ── VisionProvider trait ──► Ollama (LAN)  |  OpenRouter (opt-in)  |  honest error
   │  writes: vision_descriptions row (provider, model, prompt_version, content_sha256)
   ▼
Rename suggestion  =  pure function( template, vision_descriptions, asset metadata, batch index )
   │  no storage needed to preview — deterministic and recomputable
   ▼
Preview (UI / CLI table)  ── user confirms ──►  Apply
   │  per file: fs::rename (same dir, atomic on APFS)
   │            → sha256 of new path must equal the recorded hash
   │            → Store::relink_video / relink_photo  (hash re-verified INSIDE the
   │              same Immediate transaction that updates the path — crates/store/src/lib.rs
   │              `relink_row`, the exact machinery TASK-038 shipped)
   │            → rename_operations row (audit + rollback)
   ▼
Rollback  =  inverse rename → hash verify → relink back → rename_operations.status = 'rolled_back'
```

Ingest **does not** auto-describe. Describing is an explicit user action (asset detail button, or batch action over a Review selection) so ingest stays deterministic, offline, and fast. A job row with `stage='describe'` and the usual `job_id`+`stage` logging covers every run.

### 4.2 Where the data lands — schema v14 proposal

`crates/store/migrations/0014_vision_describe.sql`:

```sql
-- v14: AI vision descriptions. AI output is NOT user evidence: it never touches
-- editorial_annotations (which feed style training) and always carries provider,
-- model, prompt_version, and the content hash it describes.
CREATE TABLE vision_descriptions (
  owner_id        TEXT NOT NULL REFERENCES owners(id),
  media_kind      TEXT NOT NULL CHECK (media_kind IN ('photo', 'video')),
  media_id        TEXT NOT NULL,
  provider        TEXT NOT NULL,            -- 'ollama' | 'openrouter'
  model           TEXT NOT NULL,
  prompt_version  TEXT NOT NULL,
  description     TEXT NOT NULL DEFAULT '',
  tags_json       TEXT NOT NULL DEFAULT '[]',
  objects_json    TEXT NOT NULL DEFAULT '[]',
  scene           TEXT NOT NULL DEFAULT '',
  mood            TEXT,
  colors_json     TEXT,
  content_sha256  TEXT NOT NULL,            -- sha256 of the media when described
  analyzed_at     TEXT NOT NULL,
  PRIMARY KEY (owner_id, media_kind, media_id)
) STRICT;

-- target-existence + cleanup triggers mirroring 0002's editorial_annotation triggers;
-- index on (owner_id, analyzed_at) for stale sweeps.
```

This copies the proven `aesthetic_assessments` pattern from migration 0002: per-media keyed analysis carrying `model_version` + timestamp, with a `photos_for_analysis`-style stale sweep (`prompt_version`/model changed → row is stale, re-describe available, old row kept until replaced). A `jobs.stage` CHECK extension accompanies it — SQLite can't ALTER a CHECK, so the migration rebuilds `jobs` exactly the way 0004 and 0006 already did (verified precedents).

Deliberate choice: **AI descriptions are a separate table, not `editorial_annotations`.** `editorial_annotations.description/tags` are user judgments — they feed the style trainer, and TASK-034's whole discipline was about what is allowed to count as evidence. Mixing machine output into them would poison that. The UI shows AI text with its provider label next to (never inside) the user's own annotation fields. A later task may add `vision_descriptions_fts` to put AI text into search, mirroring `manual_spans_fts` from v13 — **deferred**, see open decisions.

### 4.3 The rename flow and the safety posture

John's rule: *opt-in, previewed, reversible, atomic catalog path updates, identity by sha256 never path.* Every clause maps to existing, shipped machinery:

- **Opt-in:** no rename happens from a suggestion; apply is a separate confirmed action.
- **Previewed:** the suggestion is a pure function (template engine, §4.4); the preview shows old → new per item, flags collisions (within the batch and against the destination directory) and refuses to apply a batch containing any collision — the same no-clobber posture render publication uses. Sanitization guarantees filenames the filesystem can't choke on.
- **Reversible:** nodeo rolls back from `.backup_` **copies** (`shutil.copy2` in `rename_engine.py`) — for video that doubles storage, and it is the one part of nodeo's design crush should *not* port. A rename is its own inverse: rollback renames back, after verifying the file's sha256 both ways, and flips `rename_operations.status`. Nothing is ever copied or deleted.
- **Atomic catalog path updates:** the fs rename and the catalog write are two steps, so the ordering and recovery rule is explicit: **fs rename first, then `relink_*`** (which re-verifies the hash inside its transaction and fails closed without writing). If the process dies between the two steps, the catalog holds a stale path — which is precisely the state crush already survives: identity is `owner_id+sha256`, ingest reports `moved`/`renamed → relinked`, and `crushctl relink` / "Locate moved file…" repairs it. The merge adds a *producer* of that state; it does not invent a new failure class.
- **Identity by sha256:** unchanged. `relink_row` refuses any hash mismatch; `vision_descriptions` carries `content_sha256` so a description can never silently attach to different content at the same path.

One real integration detail to handle in the app task: at launch, the Tauri asset-protocol scope allowlists each stored video/photo path (`scope.allow_file` in `crates/app/src-tauri/src/lib.rs` `setup`). A rename performed mid-session must add the new path to the scope (or the thumbnail/player view breaks until relaunch). Small, but it will look like a "broken app" bug if missed.

### 4.4 Template engine port (the golden-test crown jewel)

`app/services/template_parser.py` is a pure string transformation — the ideal thing to port with an answer key. The port keeps nodeo's exact semantics: 70+ variables across basic / date-time / media / file / AI / project / utility groups; sanitization (lowercase, spaces → `_`, strip everything outside `[a-z0-9_-]`, collapse `_` runs, 50-char per-component and 100-char final caps); zero-padded `{index}`/`{project_number}`; empty-variable cleanup; the 16 `PREDEFINED_TEMPLATES`.

Variable sourcing in crush:

| Variable group | Source |
|---|---|
| `description`, `tags`, `scene`, `mood`, `colors`, `dominant_object`, `primary_color`, `style` | `vision_descriptions` (+ derived slug rules ported from the parser) |
| `width`, `height`, `resolution`, `orientation`, `duration_s`, `frame_rate`, `codec`, `format`, `media_type` | `photos` / `videos` + existing source-metadata rows |
| `date`, `time`, `year`…, `created_date`, `modified_date` | `captured_at` (EXIF) preferred; file stat fallback |
| `original`, `extension` | current path |
| `index` | batch counter (zero-padded) |
| `project`, `project_name`, `client`, `project_type`, `project_number` | project/collection name where one exists; **honest empty** where crush has no concept (e.g. `client`) — never fabricated |
| `random`, `random4`, `uuid` | generated at apply time (and frozen into the preview so apply matches preview exactly) |

**Goldens:** run nodeo's Python parser once over a fixture matrix of (template × metadata) combos, commit the expected filenames as JSON, and Rust must match byte-for-byte — the blueprint's answer-key discipline applied to a pure function. Never edit the golden file to pass. Plus property tests: output contains only `[a-z0-9_.-]`, length caps hold, no collision within a batch, `{}` unknown-variable validation error matches nodeo's `validate_template` messages.

---

## 5. Release plan (TASK-041 → TASK-047 → 0.1.0)

Standing constraints: `task/NN-short-name`, one task per PR, squash, Linux+macOS CI, fmt + warnings-denied clippy + workspace tests + `npm run test:ui`, doctor output pasted in Mac PRs, golden files never edited. The **0.0.1 RC stays frozen** — the DMG is cut at commit `7d9b3b5` and John's clean-machine acceptance (`docs/smoke.md`) is the gate that completes it; merge tasks touch nothing the clean-machine run exercises (render recipes, executors, packaging scripts). If acceptance findings force fixes, they jump the queue ahead of merge tasks. Watch the known conflict-prone `generate_handler!` block — rebase onto main + rustfmt before every PR (standing HANDOFF warning).

| Task | Branch | Content | Tests / gates |
|---|---|---|---|
| **TASK-041** AI provider layer (local) | `task/41-ai-provider` | `crates/ai` (`crush-ai`): `VisionProvider` trait, `NoneProvider` honest error, Ollama backend over `ureq` (JSON-fence stripping, normalization), `[ai]` config section in `core::config` + `crush.example.toml`, doctor provider check (evidence, not failure), bounded batch worker helper | Parse tests against recorded Ollama JSON fixtures; fake provider for CI (no network in CI, ever); config round-trip |
| **TASK-042** OpenRouter backend + key storage | `task/42-openrouter` | OpenRouter client (verified request/response shape), `keyring` Keychain storage + env override, `crushctl ai check` (key + model existence), cost guardrails: probe-image estimate → confirmation threshold → hard cap with honest mid-batch abort; provider labeling strings | Response-parse goldens from recorded fixtures; cost math unit tests; keyring behind a trait (CI uses memory store); NO live network test in CI |
| **TASK-043** Describe stage + schema v14 | `task/43-describe-stage` | `0014_vision_describe.sql` (table + triggers + `jobs` rebuild with `describe` stage), `Stage::Describe`, pipeline describe-photos flow (batch, cancellable, resumable, per-item failure), `crushctl describe <path|asset> [--batch]`, app command `describe_assets`; prompt ported verbatim from `LLAVA_IMPROVEMENTS.md` as a versioned const | Migration v13→v14 test; store owner-isolation tests (house pattern); fake-provider stage test: rows land with provider/model/prompt_version/content_sha256; stale-sweep test |
| **TASK-044** Template engine port | `task/44-rename-templates` | Pure Rust port of `template_parser.py`: 70+ variables, sanitization, 16 predefined templates, `validate_template` | **Answer-key goldens generated from nodeo's Python** (committed JSON, byte-exact); sanitization property tests; validation-message parity |
| **TASK-045** Rename preview / apply / rollback | `task/45-rename-apply` | `0015_rename_operations.sql` (audit table + `rename` stage), pipeline flows: preview (pure), apply (fs rename → sha256 verify → `relink_*` → audit row), rollback (inverse, hash-verified), collision refusal, `crushctl rename preview|apply|rollback` | Fixture-copy roundtrips: apply→catalog path updated→no duplicate rows→rollback restores; hash-mismatch refusal; crash-between-steps recovery documented and tested; originals' hashes identical before/after |
| **TASK-046** App UI: providers, describe, rename | `task/46-describe-rename-ui` | Preferences: provider section (Ollama host/model; OpenRouter opt-in toggle with the leave-the-machine warning, key entry → Keychain, per-run cost confirmation). Asset detail: Describe button + AI-labeled text beside (not inside) user annotations. Review: batch describe. Rename dialog: template picker (16 presets + custom, live preview table, collision marks) → confirm → result → Undo (rollback). Rename updates the asset-protocol scope in-session | Browser harness with mock bridge (the `relink_asset` pattern, `mock-bridge.js`); honest empty/error states per the UX language rules |
| **TASK-047** Release 0.1.0 | `task/47-release-010` | Version bump 0.0.1 → **0.1.0** (`Cargo.toml [workspace.package]`, `tauri.conf.json`); `docs/ai-providers.md` (new); `docs/release.md` Privacy section update (network-use disclosure) + rename/relink section extension; `docs/release-record-0.1.0.md`; smoke checklist additions; packaging via existing `scripts/package-macos.sh` untouched | `scripts/verify-release.sh`; CI; **human acceptance gate (John)**: describe on real photos via ai-srv Ollama; rename preview→apply→rollback on a sacrificial folder with hash checks; OpenRouter opt-in flow with cost confirmation observed; 0.0.1 clean-machine acceptance remains its own separate gate |

**Stretch (not release-blocking):** TASK-048 `vision_descriptions_fts` so AI tags/descriptions join text search (the v13 `manual_spans_fts` pattern); shot-level describe (`media_kind='shot'` rep-frames); `{transcript}` template variables using existing transcripts.

**Why this order:** 041 unblocks everything; 044 is pure and could run in parallel with 042/043 if two lanes are open; 045 needs 043 (data) + 044 (engine); UI last, matching how every prior crush feature landed (CLI + tests first, app second).

---

## 6. Risks and honest gaps

1. **Nodeo's audio/video claims do not exist and are not being merged.** The audit is unambiguous (0% audio, 5% video). Crush's whisper-rs/scene-detection/CLIP stack already exceeds anything nodeo promised. The merged release notes must not import nodeo's aspirational language.
2. **The folder watcher never worked.** Infrastructure around a stub. Deferring it loses nothing proven; building it now would add a background-lifecycle surface (crush currently ships with "no daemon or login item") to a release that is already carrying two features.
3. **Images leave the machine on OpenRouter — permanently labeled.** Opt-in, off by default, per-run provider choice, visible "this image left your machine" labeling, store provenance in `vision_descriptions.provider`, and a `docs/release.md` privacy-section update. No silent failover from Ollama to OpenRouter, ever.
4. **Ollama on the LAN is not always up.** ai-srv is a Windows box that isn't 24/7. Doctor reports reachability as evidence; describe failures are honest per-item errors with retry; batches are resumable. Nothing in crush's non-AI paths depends on it.
5. **LLaVA JSON reliability.** Nodeo needed fallbacks because small VLMs break JSON format. Crush keeps the tuned prompt (temp 0.3, 300 tokens), ports the fence-stripping and normalization, and treats malformed output as a visible per-item failure — no hidden retries. `prompt_version` in the schema makes prompt improvements trackable and re-describable.
6. **Rename two-step crash window.** fs rename and catalog update cannot be one atomic operation. The window is real but already-survived territory: identity is sha256, ingest relinks, `crushctl relink` repairs. TASK-045 must document and test this state explicitly rather than pretend it can't happen.
7. **OpenRouter cost and model drift.** `usage.cost` makes caps enforceable, but prices change and models come and go; the model is a config string and `crushctl ai check` verifies it. Defaults are conservative ($0.50 confirm, $1.00 cap) and John-adjustable.
8. **Scope/runtime details that will bite if skipped:** the `jobs` CHECK rebuild in v14 (precedent 0004/0006); the asset-protocol scope after in-session renames; the `generate_handler!` merge conflicts (standing warning); nodeo's stale `.248` Ollama host must not become crush's default.
9. **What this merge does NOT include:** any server component, any cloud storage, project auto-classification, folder watching, search over AI text (stretch), shot-level vision description (stretch), speaker diarization or any of nodeo's Phase 2–4 roadmap.

---

## 7. Open decisions (recommended defaults included)

| # | Decision | Recommendation |
|---|---|---|
| 1 | Default OpenRouter vision model | `google/gemini-2.5-flash` — cheap, fast, vision-capable; **confirm in the OpenRouter catalog at implementation time**; always a config string |
| 2 | Auto-describe during ingest? | **No.** Explicit button/batch action only. Ingest stays deterministic and offline; AI is a separate, visible, cancellable step |
| 3 | Scope of describe in 0.1.0 | Photos first; video-level describe only if the TASK-040 thumbnails make it trivial — else stretch. Schema's `media_kind` already allows both |
| 4 | Default Ollama model | Keep `llava` (nodeo's prompts are tuned against it) but configurable; John decides what lives on ai-srv (e.g. a Qwen2.5-VL variant may outperform) |
| 5 | Ollama host default | `http://192.168.50.247:11434` (ai-srv per estate table), configured explicitly + doctor check; no auto-discovery |
| 6 | Search over AI tags/descriptions in 0.1.0? | **Defer** to TASK-048 stretch — keeps the release reviewable; the FTS pattern (v13) is ready when needed |
| 7 | Folder watcher, project classification | **Defer** past 0.1.0, honestly labeled in release notes |
| 8 | Key entry point for OpenRouter | Both: Preferences (writes Keychain) and `CRUSH_OPENROUTER_API_KEY` env for CLI |
| 9 | Merge tasks vs 0.0.1 acceptance ordering | Merge tasks proceed on their own branches now (TASK-022 precedent), but any clean-machine acceptance finding preempts them; RC freeze on render/packaging paths is absolute |
| 10 | Rollback UI depth | One-level undo per batch via `rename_operations` (crush's plan-revision-style reversibility), not a full versioned path history |

---

**Bottom line:** this merge takes nodeo's two genuinely working ideas — structured vision description and template-driven renaming — and lands them on machinery crush already trusts: the stage/store discipline, the sha256 identity model, and the TASK-038 relink primitive. Everything else nodeo claimed is either already in crush or is deferred with its honest status written down. The 0.0.1 RC is untouched; 0.1.0 becomes the release where crush learned what nodeo actually knew.
