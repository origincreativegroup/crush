# TASK-041 — AI provider layer (local Ollama)

**Branch:** `task/41-ai-provider` · **Parent plan:** `docs/nodeo-merge-evaluation.md` §3, §5
**Scope:** provider layer ONLY. No stage, no schema, no app commands, no OpenRouter (TASK-042/043).

## Goal

New crate `crates/ai` (`crush-ai`), depending only on `crush-core`, that gives crush a
local-first vision-description provider abstraction. Ollama on the LAN is the first backend.
"No provider configured" is an honest capability error, never a silent fallback.

## Deliverables

1. **Workspace:** add `crates/ai` to members; add `base64 = "=0.22.1"` to workspace deps
   (needed to inline images for Ollama /api/chat; boring, pinned, justified in PR description).
   If `docs/project-blueprint.md` lists crates, add `crush-ai` there too.
2. **`crates/ai/src/lib.rs`** — public surface:
   - `VisionProvider` trait: `id() -> &'static str`, `model() -> &str`,
     `describe_image(&self, req: &DescribeRequest) -> anyhow::Result<ImageDescription>` (sync;
     the pipeline is synchronous — no async runtime).
   - `DescribeRequest { image_path: PathBuf, prompt_version: &'static str, temperature: f32,
     max_tokens: u32 }` — provider reads image bytes itself from the path.
   - `ImageDescription { description, tags (lowercased, ≤10), objects, scene,
     mood: Option<String>, colors: Option<Vec<String>> }` (serde).
   - `NoneProvider` — returns the standing honest capability error:
     "AI description is not available: no vision provider is configured. Set up local Ollama in
     Preferences (recommended). Nothing else in Crush is affected."
   - `OllamaProvider { host, model }` over pinned `ureq` 3.4.0 (rustls):
     `POST {host}/api/chat`, body
     `{"model": ..., "messages": [{"role":"user","content": <prompt>, "images": ["<base64>"]}],
     "options": {"temperature": 0.3, "num_predict": 300}, "stream": false}`;
     response `{"message": {"content": "..."}}`.
   - `provider_from_config(&AiConfig) -> Box<dyn VisionProvider>` — "none" → NoneProvider,
     "ollama" → OllamaProvider, anything else → honest error naming the valid options.
3. **Prompt:** port the structured-JSON prompt from nodeo's `LLAVA_IMPROVEMENTS.md` verbatim as
   `prompts::DESCRIBE_V1` with `pub const PROMPT_VERSION: &str = "v1"` (description 2–3 sentences,
   5–10 lowercase tags, objects, scene from the fixed list, optional mood/colors; "Respond with
   valid JSON only"). `DescribeRequest` may carry a custom prompt override; default is DESCRIBE_V1.
4. **JSON robustness (port nodeo's hard-won details, fast method only):** strip markdown code
   fences; parse JSON; tolerate `tags` arriving as a comma/string; lowercase + dedupe + cap tags
   at 10; trim strings; missing keys → empty, not error. Malformed beyond repair → `Err` with the
   raw prefix included (per-item honest failure; NO hidden retries, NO legacy 4-call fallback).
5. **Config:** `[ai]` section in `crates/core/src/config.rs`:
   `provider = "none"`, `ollama_host = "http://192.168.50.247:11434"` (ai-srv; do NOT carry over
   nodeo's stale `.248`), `ollama_model = "llava"`, `temperature = 0.3`, `max_tokens = 300`,
   `max_concurrent = 2`. Env overrides: `CRUSH_AI_PROVIDER`, `CRUSH_AI_OLLAMA_HOST`,
   `CRUSH_AI_OLLAMA_MODEL`. Update `crush.example.toml` with the `[ai]` block + comments.
6. **Doctor:** in the CLI's doctor command, add an AI provider check — host reachable + model
   present (`GET {host}/api/tags`) — reported as EVIDENCE, never a failure when absent or
   unreachable. Follow the existing doctor output shape.
7. **Batch helper:** `batch_describe(provider, paths, max_concurrent) -> Vec<(PathBuf,
   Result<ImageDescription, String>)>` — bounded std threads (no async), preserves input order,
   per-item errors never abort the batch.

## Tests (no network in CI, ever)

- Parse fixtures in `crates/ai/tests/fixtures/`: clean JSON, fenced ```json, tags-as-string,
  malformed → error. Assert normalization (lowercase, ≤10, dedupe).
- `FakeProvider` implementing the trait for CI tests; `provider_from_config` mapping tests.
- Config round-trip: defaults serialize/deserialize; env overrides apply.
- Batch helper: order preserved, one bad item doesn't fail the batch, concurrency bound respected.
- Full gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.

## Rules

- Stages read from a path and write results; no in-memory hand-offs between stages (this crate
  is a service used by later stages, not a stage itself — Stage::Describe lands in TASK-043).
- Log nothing user-hostile; no image bytes in logs. Errors carry context, not payloads.
- Do not touch render paths, executors, packaging, or the app crate (the `generate_handler!`
  block is conflict-prone — staying out of `crates/app` keeps this PR clean).
- Add the TASK-041 row to TASKS.md (status: done at PR time).
