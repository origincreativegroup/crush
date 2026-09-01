// Scripted UI harness (Task 027). Drives the real app UI — crates/app/ui/index.html loaded
// inside crates/app/tests/ui-harness.html — with a mock Tauri bridge (crates/app/tests/
// mock-bridge.js) injected before the app's scripts parse. Runs on system Chrome via
// playwright-core; no browser is ever downloaded. Override the binary with CRUSH_CHROME_PATH.
//
// Usage: npm run test:ui  (or: node scripts/ui-harness.mjs [scenario ...])
//
// Determinism: page.clock.install() freezes the app's 850 ms poll interval and 5 s message
// timers so re-renders cannot race assertions; the mock emits its events on microtasks.
// compare-advance uses page.clock.fastForward() to observe the 600 ms auto-advance beat;
// compare-advance-reduced runs with emulated prefers-reduced-motion and must not need it.

import assert from "node:assert/strict";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright-core";

const harnessPath = fileURLToPath(
  new URL("../crates/app/tests/ui-harness.html", import.meta.url),
);
const mockBridgePath = fileURLToPath(
  new URL("../crates/app/tests/mock-bridge.js", import.meta.url),
);

async function poll(predicate, timeoutMs = 5000, stepMs = 50) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await predicate()) return;
    if (Date.now() > deadline) throw new Error("harness condition was not met in time");
    await new Promise((resolve) => setTimeout(resolve, stepMs));
  }
}

async function visibleText(locator) {
  await locator.waitFor({ state: "visible" });
  return (await locator.textContent())?.trim() ?? "";
}

async function mockCalls(page) {
  const frame = page.frames().find((candidate) => candidate.url().includes("ui/index.html"));
  if (!frame) throw new Error("app iframe was not found");
  return frame.evaluate(() => window.__crushMock.calls);
}

/// Evaluate inside the app iframe (FrameLocator has no evaluate).
async function inAppFrame(page, fn) {
  const frame = page.frames().find((candidate) => candidate.url().includes("ui/index.html"));
  if (!frame) throw new Error("app iframe was not found");
  return frame.evaluate(fn);
}

async function createPlan(page, name = "Launch selects") {
  const frame = page.frameLocator("#app-frame");
  await frame.locator("#nav-plans").click();
  await frame.locator("#plan-new-name").fill(name);
  await frame.locator("#plan-new-context").fill("campaign");
  await frame.locator("#plan-create-form button").click();
  await frame.locator("#plan-editor").waitFor({ state: "visible" });
  return frame;
}

const tests = {
  async "dam-home"(page) {
    const frame = page.frameLocator("#app-frame");
    const cards = frame.locator("#results-grid .result-card");
    await poll(async () => (await cards.count()) === 3);
    assert.equal(await visibleText(frame.locator("#dam-context")), "Local library");
    assert.equal(await visibleText(frame.locator("#dam-title")), "All assets");
    assert.equal(await visibleText(frame.locator("#result-count")), "3 assets");
    assert.equal(await frame.locator("#top-control").isHidden(), true);
    assert.equal(await cards.filter({ hasText: "select.jpg" }).count(), 1);
    assert.equal(await cards.filter({ hasText: "rocket-launch.mov" }).count(), 1);

    await frame.locator('.dam-kind[data-kind="photo"]').click();
    await poll(async () => (await cards.count()) === 2);
    assert.equal(await visibleText(frame.locator("#dam-title")), "Photos");
    assert.equal(await visibleText(frame.locator("#result-count")), "2 assets");

    await frame.locator('.dam-kind[data-kind=""]').click();
    await frame.locator("#search-input").fill("rocket at dusk");
    await frame.locator("#search-input").press("Enter");
    await poll(async () => (await visibleText(frame.locator("#dam-context"))) === "Semantic search");
    assert.equal(await visibleText(frame.locator("#dam-title")), "Results for “rocket at dusk”");
    assert.equal(await frame.locator("#top-control").isVisible(), true);
    assert.equal((await mockCalls(page)).some((call) => call.command === "search"), true);

    await frame.locator("#search-input").fill("");
    await poll(async () => (await visibleText(frame.locator("#dam-title"))) === "All assets");
    await poll(async () => (await cards.count()) === 3);
    assert.equal(await frame.locator("#top-control").isHidden(), true);
  },

  async "import-reel-studio"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    await frame.locator("#import-reel-studio").click();
    const dialog = frame.locator("#import-dialog");
    await dialog.waitFor({ state: "visible" });
    // Apply stays locked until a dry run of the current inputs has been shown.
    assert.equal(await frame.locator("#import-apply").isDisabled(), true);
    assert.equal(await frame.locator("#import-dry-run").isDisabled(), true);
    await frame.locator("#import-pick-catalogue").click();
    await frame.locator("#import-pick-originals").click();
    await frame.locator("#import-pick-recipes").click();
    assert.match(await frame.locator("#import-catalogue").inputValue(), /clips\.db$/);
    assert.equal(await frame.locator("#import-dry-run").isDisabled(), false);
    await frame.locator("#import-dry-run").click();
    await frame.locator("#import-report").waitFor({ state: "visible" });
    assert.match(await visibleText(frame.locator("#import-summary")), /^Dry run: 1 of 2 source clips matched · segments new 1.*1 issue\.$/);
    assert.match(await visibleText(frame.locator("#import-writes")), /Would write: 1 new spans.*No preference feedback and no reference sets/);
    assert.match(await visibleText(frame.locator("#import-issues")), /missing source V1-0009/);
    assert.match(await visibleText(frame.locator("#import-candidates")), /Healthy Earth.*did not do this for you/);
    const dry = (await mockCalls(page)).find((call) => call.command === "import_reel_studio");
    assert.equal(dry.args.request.apply, false);
    assert.deepEqual(dry.args.request.originals, ["/Volumes/Footage/2026"]);
    // Changing an input after the dry run re-locks Apply.
    assert.equal(await frame.locator("#import-apply").isDisabled(), false);
    await frame.locator(".import-advanced summary").click();
    await frame.locator("#import-context").fill("museum");
    assert.equal(await frame.locator("#import-apply").isDisabled(), true);
    await frame.locator("#import-dry-run").click();
    await poll(async () => !(await frame.locator("#import-apply").isDisabled()));
    await frame.locator("#import-apply").click();
    await poll(async () => /Import applied/.test(await frame.locator("#import-status").textContent()));
    const applied = (await mockCalls(page)).filter((call) => call.command === "import_reel_studio").at(-1);
    assert.equal(applied.args.request.apply, true);
    assert.equal(applied.args.request.contextKey, "museum");
    assert.match(await visibleText(frame.locator("#import-summary")), /^Applied:/);
    assert.equal(await frame.locator("#import-apply").isDisabled(), true);
  },
  async "plans-historical"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-plans").click();
    await frame.locator("#plans-list .plans-list-button", { hasText: "Reel Studio · Healthy Earth" }).click();
    await frame.locator("#plan-editor").waitFor({ state: "visible" });
    const item = frame.locator("#plan-items .plans-item").first();
    assert.equal(await visibleText(item.locator(".plans-pill")), "Historical · your earlier Reel Studio choice");
    assert.match(await item.locator(".plans-muted").allTextContents().then((t) => t.join(" ")), /Imported boundaries come from the catalogue timecodes and may be off by up to 1(\.0+)? s/);
    // Imported spans keep In/Out editing and preview, but cannot be turned into preference examples here.
    assert.equal(await item.locator('input[name="startS"]').inputValue(), "3.45");
    assert.equal(await item.getByRole("button", { name: "Use as preference example", exact: true }).isDisabled(), true);
    await frame.locator("#project-preview").waitFor({ state: "visible" });
    assert.equal(Number(await frame.locator("#project-preview-scrubber").getAttribute("max")), 1);
    // Saving an edit keeps the historical label and never claims a profile.
    await item.locator('input[name="endS"]').fill("4.2");
    await item.getByRole("button", { name: "Save item", exact: true }).click();
    await poll(async () => (await mockCalls(page)).some((call) => call.command === "plan_update_item" && call.args.assetType === "span"));
    assert.equal(await visibleText(item.locator(".plans-pill")), "Historical · your earlier Reel Studio choice");
  },

  async "plans-span-export"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-plans").click();
    await frame.locator("#plans-list .plans-list-button", { hasText: "Reel Studio · Healthy Earth" }).click();
    await frame.locator("#plan-editor").waitFor({ state: "visible" });
    // The imported span previews like any clip, but single-clip export must stay honestly
    // disabled: the backend only resolves shot items, so an enabled button would fail with
    // the false "clip … is not selected in this project" error.
    await frame.locator("#project-preview").waitFor({ state: "visible" });
    await frame.locator("#project-photo-export").waitFor({ state: "visible" });
    assert.equal(await visibleText(frame.locator("#project-photo-export-step")), "Export selected clip");
    assert.equal(await frame.locator("#project-photo-preset").isDisabled(), true);
    assert.equal(await frame.locator("#project-photo-choose").isDisabled(), true);
    assert.equal(await frame.locator("#project-photo-render").isDisabled(), true);
    assert.match(
      await visibleText(frame.locator("#project-photo-status")),
      /Clip export for imported clips lands with the next update/,
    );
    // Whole-reel export is gated too, with span-specific copy — not the photo message.
    assert.equal(await frame.locator("#project-reel-choose").isDisabled(), true);
    assert.equal(await frame.locator("#project-reel-render").isDisabled(), true);
    assert.match(
      await visibleText(frame.locator("#project-reel-status")),
      /This sequence is built from imported clips\. Whole-reel export for imported clips lands with the next update\./,
    );
    // The honest disabled state means no render command ever fires for the span item.
    const calls = await mockCalls(page);
    assert.equal(calls.some((call) => call.command === "render_project_clip"), false);
    assert.equal(calls.some((call) => call.command === "render_project_reel"), false);
  },

  async "plans-editor"(page) {
    const frame = await createPlan(page);
    await frame.locator("#plan-brief").fill("Quiet launch portraits");
    await frame.locator('#plan-header-form button[type="submit"]').click();
    await frame.locator("#plan-generate").click();
    await poll(async () => await frame.locator("#plan-general .plans-candidate").count() === 2);
    assert.match(await visibleText(frame.locator("#plan-personal-status")), /Experimental preference profile v3.*review pending/);
    await frame.locator("#plan-general .plans-candidate").first().locator("button").click();
    await frame.locator("#project-reel-choose").click();
    assert.match(await frame.locator("#project-reel-destination").inputValue(), /Launch-selects\.mp4$/);
    await frame.locator("#project-reel-render").click();
    await frame.locator("#project-reel-result").waitFor({ state: "visible" });
    assert.match(await visibleText(frame.locator("#project-reel-status")), /Reel rendered and verified/);
    await frame.locator("#project-reel-result summary").click();
    assert.match(await visibleText(frame.locator("#project-reel-verification")), /2\.75 seconds.*Output checksum/s);
    const reelRenderCall = (await mockCalls(page)).find((call) => call.command === "render_project_reel");
    assert.equal(reelRenderCall.args.projectId, "plan-1");
    assert.equal(reelRenderCall.args.preset, "mp4-h264-sdr-v1");
    await frame.locator("#plan-personal .plans-candidate").first().locator("button").click();
    const items = frame.locator("#plan-items .plans-item");
    await poll(async () => await items.count() === 2);
    assert.equal(await visibleText(items.nth(0).locator(".plans-pill")), "General");
    assert.match(await visibleText(items.nth(1).locator(".plans-pill")), /Preference-assisted · profile v3/);
    assert.equal(await frame.locator("#project-reel-render").isDisabled(), true);
    assert.match(await visibleText(frame.locator("#project-reel-status")), /sequence includes photos/);
    // Sequence preview starts on the first clip and exposes visible, boundary-aware controls.
    await frame.locator("#project-preview").waitFor({ state: "visible" });
    assert.equal(await frame.locator("#project-preview-play").isVisible(), true);
    assert.equal(Number(await frame.locator("#project-preview-scrubber").getAttribute("max")), 2.75);
    await frame.locator("#project-photo-export").waitFor({ state: "visible" });
    assert.equal(await visibleText(frame.locator("#project-photo-export-step")), "Export selected clip");
    await frame.locator("#project-photo-preset").selectOption("mov-h264-sdr-v1");
    await frame.locator("#project-clip-export-options summary").click();
    await frame.locator("#project-clip-audio").selectOption("mute");
    await frame.locator("#project-photo-choose").click();
    assert.match(await frame.locator("#project-photo-destination").inputValue(), /launch_export\.mov$/);
    await frame.locator("#project-photo-render").click();
    await frame.locator("#project-photo-result").waitFor({ state: "visible" });
    assert.match(await visibleText(frame.locator("#project-photo-status")), /Rendered and verified.*original video was not changed/);
    const clipRenderCall = (await mockCalls(page)).find((call) => call.command === "render_project_clip");
    assert.equal(clipRenderCall.args.projectId, "plan-1");
    assert.equal(clipRenderCall.args.shotId, "shot-1");
    assert.equal(clipRenderCall.args.preset, "mov-h264-sdr-v1");
    assert.equal(clipRenderCall.args.audio, "mute");
    await frame.locator("#project-preview-next").click();
    assert.equal(await frame.locator("#project-preview-photo").isVisible(), true);
    await frame.locator("#project-photo-export").waitFor({ state: "visible" });
    await frame.locator("#project-photo-preset").selectOption("png-srgb-v1");
    await frame.locator("#project-photo-choose").click();
    assert.match(await frame.locator("#project-photo-destination").inputValue(), /select_export\.png$/);
    await frame.locator("#project-photo-render").click();
    await frame.locator("#project-photo-result").waitFor({ state: "visible" });
    assert.match(await visibleText(frame.locator("#project-photo-status")), /Rendered and verified.*original photo was not changed/);
    assert.match(await visibleText(frame.locator("#project-photo-output-path")), /select_export\.png$/);
    assert.match(await visibleText(frame.locator("#project-photo-manifest-path")), /crush-manifest\.json$/);
    await frame.locator("#project-photo-result summary").click();
    assert.match(await visibleText(frame.locator("#project-photo-verification")), /2400 × 1600.*3.00 MB.*Output checksum/s);
    await frame.locator("#project-photo-show-manifest").click();
    const renderCall = (await mockCalls(page)).find((call) => call.command === "render_project_photo");
    assert.equal(renderCall.args.projectId, "plan-1");
    assert.equal(renderCall.args.photoId, "photo-0");
    assert.equal(renderCall.args.preset, "png-srgb-v1");
    assert.equal((await mockCalls(page)).filter((call) => call.command === "open_in_finder").at(-1).args.path, renderCall.args.destination + ".crush-manifest.json");
    await frame.locator("#project-preview-prev").click();
    assert.equal(await frame.locator("#project-preview-video").isVisible(), true);
    assert.equal(await visibleText(frame.locator("#project-photo-export-step")), "Export selected clip");
    assert.equal(await frame.locator("#plan-general .plans-candidate button:disabled").count(), 2);
    const original = (await mockCalls(page)).filter((call) => call.command === "plan_add_item");
    assert.equal(original[0].args.item.origin, "general");
    assert.equal(original[0].args.item.profileVersion, null);
    assert.equal(original[1].args.item.origin, "personal");
    const frozen = JSON.parse(original[1].args.item.signalsJson);
    assert.equal(frozen.profile.id, "profile-demo");
    assert.equal(frozen.context, "campaign");
    assert.equal(frozen.lane, "personalized");
    assert.equal(frozen.ordinal, 1);

    await items.nth(0).locator('[name="startS"]').fill("3.4");
    await items.nth(0).locator('[name="endS"]').fill("5.2");
    assert.ok(Math.abs(Number(await frame.locator("#project-preview-scrubber").getAttribute("max")) - 1.8) < 0.001);
    await items.nth(0).locator('[name="endS"]').fill("3.3");
    assert.equal(await frame.locator("#project-preview-play").isDisabled(), true);
    assert.equal(await visibleText(frame.locator("#project-preview-time")), "Set a valid In and Out");
    await items.nth(0).locator('[name="endS"]').fill("5.2");
    await items.nth(0).locator('[name="reason"]').fill("Hold the quiet moment");
    await items.nth(0).locator(".plans-item-options summary").click();
    await items.nth(0).locator('[name="pacing"]').fill("0.35");
    await items.nth(0).locator('[name="cropX"]').fill("0.6");
    // Saving intent Crush cannot reproduce must warn inline instead of failing only at export.
    await poll(async () => (await items.nth(0).locator(".plans-warning").isVisible()) === true);
    assert.match(await visibleText(items.nth(0).locator(".plans-warning")), /pacing and horizontal crop.*renderer cannot reproduce/);
    await items.nth(0).locator('[name="gradeJson"]').fill('{"exposure":0.1}');
    // An unsaved second item must survive saving the first.
    await items.nth(1).locator('[name="reason"]').fill("A second draft");
    await items.nth(0).locator('button[type="submit"]').click();
    assert.equal(await items.nth(1).locator('[name="reason"]').inputValue(), "A second draft");
    await items.nth(1).locator('button[type="submit"]').click();
    await frame.locator("#plan-revision-label").fill("First selects");
    await frame.locator("#plan-revision-form button").click();
    await poll(async () => await frame.locator("#plan-revisions button").count() === 1);
    await items.nth(1).getByRole("button", { name: "Move up", exact: true }).click();
    assert.match(await visibleText(items.nth(0).locator(".plans-pill")), /Preference-assisted/);
    await items.nth(0).getByRole("button", { name: "Remove", exact: true }).click();
    await poll(async () => await items.count() === 1);
    assert.equal((await mockCalls(page)).filter((call) => call.command === "record_feedback").length, 0);
    await frame.locator("#plan-revisions button").click();
    await frame.locator('#plan-confirm button[value="confirm"]').click();
    await poll(async () => await items.count() === 2);
    assert.equal(await items.nth(0).locator('[name="startS"]').inputValue(), "3.4");
    assert.equal(await items.nth(0).locator('[name="endS"]').inputValue(), "5.2");
    assert.match(await visibleText(items.nth(1).locator(".plans-pill")), /Preference-assisted · profile v3/);
    await items.nth(0).getByRole("button", { name: "Use as preference example", exact: true }).click();
    const feedback = (await mockCalls(page)).filter((call) => call.command === "record_feedback");
    assert.equal(feedback.length, 1);
    assert.equal(feedback[0].args.contextKey, "campaign");
    assert.equal(feedback[0].args.context, "Quiet launch portraits");
    await frame.locator("#plan-duplicate").click();
    await poll(async () => await frame.locator("#plans-list button").count() === 2);
    assert.equal(await frame.locator("#plan-name").inputValue(), "Launch selects copy");
    await frame.locator("#plan-delete").click();
    await frame.locator('#plan-confirm button[value="cancel"]').click();
    assert.equal(await frame.locator("#plans-list button").count(), 2);
    await frame.locator("#plan-delete").click();
    await frame.locator('#plan-confirm button[value="confirm"]').click();
    await poll(async () => await frame.locator("#plans-list button").count() === 1);
    await frame.locator("#plans-list button").click();
    assert.equal(await items.count(), 2);
    assert.equal(await frame.locator("#plan-revisions button").count(), 1);
  },

  async "plans-general"(page) {
    const frame = await createPlan(page, "Cold start");
    await frame.locator("#plan-generate").click();
    await poll(async () => await frame.locator("#plan-general .plans-candidate").count() === 2);
    assert.equal(await frame.locator("#plan-personal .plans-candidate").count(), 0);
    await frame.locator("#plan-brief").fill("Evening light");
    await frame.locator('#plan-header-form button[type="submit"]').click();
    await frame.locator("#plan-generate").click();
    await poll(async () => await frame.locator("#plan-personal .plans-candidate").count() === 2);
    assert.match(await visibleText(frame.locator("#plan-personal-status")), /Matched to the brief with the general model/);
    await frame.locator("#plan-personal .plans-candidate").first().locator("button").click();
    const addition = (await mockCalls(page)).find((call) => call.command === "plan_add_item");
    assert.equal(addition.args.item.origin, "general");
    assert.equal(addition.args.item.profileVersion, null);
    assert.equal(JSON.parse(addition.args.item.signalsJson).lane, "personalized");
    await frame.locator("#nav-style").click();
    // The pre-verdict hard stop ("no surface claims learned") was resolved by the recorded
    // verdict (docs/style-proof-review.md, 2026-08-31): the label may appear, but only in
    // its bounded form — next to the visible scope line, never claiming more.
    await poll(async () =>
      (await visibleText(frame.locator("#style-status-line"))) === "Learned profile",
    );
    assert.match(await visibleText(frame.locator("#style-scope-note")), /not on unseen future work/);
  },

  async "plans-sequence"(page) {
    const frame = await createPlan(page, "Sequence notes");
    // The visible, adjustable similar-shot cap: echo and skip count land in the status line.
    await frame.locator("#plan-generate").click();
    await poll(async () => await frame.locator("#plan-general .plans-candidate").count() === 3);
    await frame.locator("#plan-duplicate-cap").fill("1");
    await frame.locator("#plan-generate").click();
    await poll(async () =>
      (await frame.locator("#plan-general .plans-candidate").count()) === 2
      && (await visibleText(frame.locator("#plan-candidate-status"))).includes("cap 1 applied"));
    await frame.locator("#plan-duplicate-cap").fill("");
    await frame.locator("#plan-generate").click();
    await poll(async () => await frame.locator("#plan-general .plans-candidate").count() === 3);
    // Two adjacent clip items: sequence notes flag the near-identical pair. A two-item plan
    // cannot separate the pair by reordering, so no chip is offered yet — the note is honest.
    await frame.locator("#plan-general .plans-candidate").first().locator("button").click();
    await poll(async () => await frame.locator("#plan-items .plans-item").count() === 1);
    await frame.locator("#plan-general .plans-candidate").nth(1).locator("button").click();
    await poll(async () => await frame.locator("#plan-items .plans-item").count() === 2);
    const notes = frame.locator("#plan-sequence-notes");
    await notes.waitFor({ state: "visible" });
    assert.match(await visibleText(notes), /near-identical/);
    assert.equal(await notes.locator("button").count(), 0);
    // A third item gives the move somewhere to go: the chip appears, and applying it writes
    // normal plan state (reorder) with a saved version for undo.
    await frame.locator("#plan-general .plans-candidate").nth(2).locator("button").click();
    await poll(async () => await visibleText(notes).then((text) => text.includes("Move item 2 to the end")));
    await notes.locator("button", { hasText: "Apply reorder" }).click();
    await frame.locator('#plan-confirm button[value="confirm"]').click();
    await poll(async () => (await visibleText(frame.locator("#plans-message"))).includes("Reordered"));
    const reorder = (await mockCalls(page)).find((call) => call.command === "plan_reorder_items");
    assert.equal(reorder.args.items.length, 3, "the suggestion reorders the whole plan");
    const saved = (await mockCalls(page)).filter((call) => call.command === "plan_save_revision");
    assert.equal(saved.at(-1).args.label, "Before sequence suggestion");
    // After the move the mock reports no flagged adjacency, so the chip is gone.
    await poll(async () => (await notes.locator("button").count()) === 0);
  },

  async "plans-errors"(page) {
    const frame = await createPlan(page);
    await frame.locator("#plan-generate").click();
    assert.match(await visibleText(frame.locator("#plan-candidate-status")), /lookup failed/);
    await frame.locator("#plan-generate").click();
    await frame.locator("#plan-general .plans-candidate").first().locator("button").click();
    const item = frame.locator("#plan-items .plans-item").first();
    await item.locator('[name="reason"]').fill("Retain this draft");
    await item.locator('button[type="submit"]').click();
    assert.match(await visibleText(frame.locator("#plans-message")), /Disk full/);
    assert.equal(await item.locator('[name="reason"]').inputValue(), "Retain this draft");
    await frame.locator("#plan-revision-form button").click();
    assert.match(await visibleText(frame.locator("#plans-message")), /Save your clip/);
    await frame.locator("#plan-duplicate").click();
    await frame.locator('#plan-confirm button[value="cancel"]').click();
    assert.equal(await item.locator('[name="reason"]').inputValue(), "Retain this draft");
    await item.locator('button[type="submit"]').click();
    assert.match(await visibleText(frame.locator("#plans-message")), /Clip settings saved/);
    const callsBefore = (await mockCalls(page)).filter((call) => call.command === "plan_update_item").length;
    await item.locator(".plans-item-options summary").click();
    await item.locator('[name="gradeJson"]').fill("[]");
    await item.locator('button[type="submit"]').click();
    assert.match(await visibleText(frame.locator("#plans-message")), /JSON object/);
    assert.equal((await mockCalls(page)).filter((call) => call.command === "plan_update_item").length, callsBefore);
    await item.locator('[name="gradeJson"]').fill("{}");
    await item.locator('[name="startS"]').fill("5.8");
    await item.locator('[name="endS"]').fill("3.5");
    await item.locator('button[type="submit"]').click();
    assert.match(await visibleText(frame.locator("#plans-message")), /Out must be after In/);
    assert.equal((await mockCalls(page)).filter((call) => call.command === "plan_update_item").length, callsBefore);
    await item.locator('[name="startS"]').fill("3.4");
    await item.locator('[name="endS"]').fill("5.2");
    await item.locator('[name="gradeJson"]').fill('{"exposure":0.1}');
    await item.locator('button[type="submit"]').click();
    assert.match(await visibleText(frame.locator("#plans-message")), /Clip settings saved/);
    await frame.locator("#project-photo-choose").click();
    await frame.locator("#project-photo-render").click();
    assert.match(await visibleText(frame.locator("#project-photo-status")), /cannot be rendered exactly yet/);
    assert.equal(await frame.locator("#project-photo-render").isDisabled(), false);
    await frame.locator("#plan-general .plans-candidate").nth(1).locator("button").click();
    await poll(async () => await frame.locator("#plan-items .plans-item").count() === 2);
    await frame.locator("#plan-items .plans-item").nth(1).getByRole("button", { name: "Preview", exact: true }).click();
    await frame.locator("#project-photo-choose").click();
    await frame.locator("#project-photo-render").click();
    assert.match(await visibleText(frame.locator("#project-photo-status")), /Render failed:.*Source photo changed/);
    assert.equal(await frame.locator("#project-photo-render").isDisabled(), false);
    assert.equal(await frame.locator("#project-photo-result").isHidden(), true);
  },

  async empty(page) {
    const frame = page.frameLocator("#app-frame");
    // Search is the launch view; the shipped empty-state copy must match index.html.
    assert.equal(
      await visibleText(frame.locator("#search-nothing-indexed h2")),
      "Nothing indexed yet",
    );
    assert.match(
      await visibleText(frame.locator("#search-nothing-indexed p")),
      /index photos and split, describe, and transcribe video shots/,
    );
    await frame.locator("#nav-library").click();
    assert.equal(await visibleText(frame.locator("#empty-library h2")), "No media yet");
    assert.match(
      await visibleText(frame.locator("#empty-library p")),
      /No photos or footage indexed yet/,
    );
  },

  async "first-run-retry"(page) {
    const frame = page.frameLocator("#app-frame");
    await visibleText(frame.locator("#first-run h1"));
    assert.equal(
      await visibleText(frame.locator("#model-error")),
      "The network connection was interrupted.",
    );
    await frame.locator("#retry-models").click();
    const cont = frame.locator("#continue-models");
    await poll(() => cont.isEnabled());
    await cont.click();
    assert.equal(
      await visibleText(frame.locator("#search-nothing-indexed h2")),
      "Nothing indexed yet",
    );
    await frame.locator("#nav-library").click();
    assert.equal(await visibleText(frame.locator("#empty-library h2")), "No media yet");
  },

  async "ingest-cancel"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const cancel = frame.locator("#cancel");
    await cancel.waitFor({ state: "visible" });
    await cancel.click();
    assert.equal(
      await visibleText(frame.locator("#library-message")),
      "Cancelling after the current operation…",
    );
    // The mock flips the background task to a terminal state and re-emits the snapshot.
    await cancel.waitFor({ state: "hidden" });
    assert.equal(
      await visibleText(frame.locator("#video-rows tr.video-row .status-pill")),
      "Cancelled",
    );
  },

  async "failed-row"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const expand = frame.locator('button[aria-label="Show error details"]');
    await expand.waitFor({ state: "visible" });
    await expand.click();
    const panel = frame.locator(".error-panel");
    const panelText = await visibleText(panel);
    assert.match(panelText, /FFmpeg could not decode frame 218 near 00:00:07\.08/);
    assert.match(panelText, /job job-failed/);
    assert.match(panelText, /stage split/);
    assert.equal(await visibleText(frame.locator(".error-panel button")), "Copy details");
  },

  async "recovered-row"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const row = frame.locator("#video-rows tr.video-row");
    await row.waitFor({ state: "visible" });
    assert.equal(await visibleText(row.locator(".status-pill")), "Done");
    assert.equal(await row.locator('button[aria-label="Show error details"]').count(), 0);
    assert.equal(await frame.locator(".error-panel").count(), 0);
  },

  async "photo-row"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const rows = frame.locator("#video-rows tr.video-row");
    await rows.first().waitFor({ state: "visible" });
    assert.equal(await rows.count(), 2);
    const photoRow = rows.nth(1); // rows sort by path; Photos sorts after Footage
    assert.equal(await visibleText(photoRow.locator(".file-name")), "select.jpg");
    assert.equal(await visibleText(photoRow.locator("td.number-column")), "—");
    await photoRow.click();
    // Photos are re-indexable like videos now, not stuck in a failed/unreviewed state.
    assert.equal(await frame.locator("#reindex").isEnabled(), true);
    await frame.locator("#reindex").click();
    await poll(async () =>
      (await mockCalls(page)).some(
        (call) => call.command === "reindex_asset" && call.args.id === "photo-one",
      ),
    );
    // Removing hides a confirmation before forgetting the index; the original stays on disk.
    await frame.locator("#remove-asset").click();
    await frame.locator("#remove-asset-dialog").waitFor({ state: "visible" });
    assert.match(await visibleText(frame.locator("#remove-asset-copy")), /select\.jpg.*original file on disk is never touched/s);
    await frame.locator("#remove-asset-confirm").click();
    await poll(async () =>
      (await mockCalls(page)).some(
        (call) => call.command === "remove_asset" && call.args.id === "photo-one",
      ),
    );
    await poll(async () => (await frame.locator("#video-rows tr.video-row").count()) === 1);
  },

  async "relocate-moved-file"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const row = frame.locator("#video-rows tr.video-row");
    await row.waitFor({ state: "visible" });
    const locate = frame.locator("#locate-asset");
    // The affordance is inert until the selected row's source is really missing.
    assert.equal(await locate.isDisabled(), true);
    await row.click();
    await poll(() => locate.isEnabled());
    // A missing source is not a green Done row: the row carries a plain-language
    // "Source missing" pill in the failed tone while the Locate action is available.
    assert.equal(await visibleText(row.locator(".status-pill.done")), "Done");
    assert.equal(await visibleText(row.locator(".status-pill.failed")), "Source missing");
    // First attempt picks a different file: refused honestly, nothing changes.
    await locate.click();
    await poll(async () =>
      (await visibleText(frame.locator("#library-message"))).includes("SHA-256 mismatch"));
    assert.match(
      await visibleText(frame.locator("#library-message")),
      /not the same media Crush indexed.*Nothing was changed/s,
    );
    assert.equal(await locate.isDisabled(), false);
    assert.equal(await visibleText(row.locator(".file-path")), "/Volumes/Footage/Launch Day");
    // Second attempt picks the real moved copy: verified identical and relinked.
    await locate.click();
    await poll(async () =>
      (await visibleText(frame.locator("#library-message")))
        .includes("The file moved. Crush verified the new copy is identical before relinking"));
    const relink = (await mockCalls(page))
      .filter((call) => call.command === "relink_asset")
      .at(-1);
    assert.equal(relink.args.id, "video-one");
    assert.equal(relink.args.newPath, "/Volumes/Footage/moved/rocket-launch.mov");
    await poll(async () => await locate.isDisabled());
    // The relinked row shows the new location; its source is no longer missing, so the
    // pill is gone with it.
    assert.equal(await visibleText(row.locator(".file-path")), "/Volumes/Footage/moved");
    assert.equal(
      await row.locator(".status-pill.failed").count(),
      0,
      "a verified relink clears the Source missing pill",
    );
  },

  async "ingest-relinked"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    // A finished ingest recognized moved content and re-pointed the existing row; the
    // Library reports that honestly instead of hiding it inside "skipped".
    await poll(async () =>
      (await visibleText(frame.locator("#library-message")))
        .includes("1 file moved or renamed — relinked to the original index by content"));
    // The backend keeps finished tasks forever and re-carries them on every progress
    // event (every job_status call re-fires this finished job in the mock), so the
    // summary must be announced once per job id. Count relink announcements from here on
    // (counting, not visibility: the 5 s auto-hide timer makes visibility flaky) and
    // drive the same refresh path the app itself uses — a second event for the same job
    // must stay silent.
    await inAppFrame(page, () => {
      const original = window.showMessage;
      window.__relinkAnnouncements = 0;
      window.showMessage = (...args) => {
        if (String(args[0]).includes("relinked to the original index by content")) {
          window.__relinkAnnouncements += 1;
        }
        return original(...args);
      };
    });
    await inAppFrame(page, () => window.refreshLibrary());
    await poll(async () =>
      (await mockCalls(page)).filter((call) => call.command === "job_status").length >= 2);
    assert.equal(
      await inAppFrame(page, () => window.__relinkAnnouncements),
      0,
      "the moved/renamed summary must be announced once per job id; a later event for an already-announced job must not re-show it",
    );
  },

  async "library-multiselect"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const rows = frame.locator("#video-rows tr.video-row");
    await rows.first().waitFor({ state: "visible" });
    assert.equal(await rows.count(), 4);
    const bar = frame.locator("#library-batch-bar");
    const count = frame.locator("#library-batch-count");

    // Plain click selects one row; the batch bar's count line always reads "N selected".
    await rows.nth(0).click();
    await poll(async () => bar.isVisible());
    assert.equal(await visibleText(count), "1 selected");

    // ⌘/Ctrl-click toggles rows without losing the rest of the selection.
    await rows.nth(3).click({ modifiers: ["ControlOrMeta"] });
    assert.equal(await visibleText(count), "2 selected");

    // Esc clears the selection and the bar goes away.
    await rows.nth(0).press("Escape");
    await poll(async () => bar.isHidden());
    assert.equal(await rows.nth(0).getAttribute("aria-selected"), "false");

    // Shift-click range-selects from the last-clicked anchor.
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["Shift"] });
    assert.equal(await visibleText(count), "2 selected");

    // A video in the selection disables the photo-scoped editorial controls and the
    // bar says why instead of silently skipping rows.
    await rows.nth(2).click({ modifiers: ["ControlOrMeta"] });
    assert.equal(await visibleText(count), "3 selected");
    assert.equal(await frame.locator("#library-batch-hint").isVisible(), true);
    assert.equal(await frame.locator("#library-batch-pick").isDisabled(), true);
    await rows.nth(2).click({ modifiers: ["ControlOrMeta"] });
    assert.equal(await frame.locator("#library-batch-hint").isHidden(), true);

    // The header checkbox selects everything listed.
    await frame.locator("#library-select-all").check();
    assert.equal(await visibleText(count), "4 selected");
    await frame.locator("#library-select-all").uncheck();
    await poll(async () => bar.isHidden());
    // ⌘A does the same from the keyboard — asserted from one row selected, so the
    // shortcut has to do the selecting itself (review LOW).
    await rows.nth(0).click();
    assert.equal(await visibleText(count), "1 selected");
    await rows.nth(0).press("ControlOrMeta+a");
    assert.equal(await visibleText(count), "4 selected");
    await frame.locator("#library-select-all").uncheck();
    await poll(async () => bar.isHidden());

    // Two photos, batch-added to a brand-new collection through the inline create form.
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["ControlOrMeta"] });
    assert.equal(await visibleText(count), "2 selected");
    const batchSelect = frame.locator("#library-batch-collection");
    await poll(async () => {
      const options = await batchSelect.locator("option").allTextContents();
      return options.includes("New collection…");
    });
    await batchSelect.selectOption({ label: "New collection…" });
    const nameInput = frame.locator("#library-batch-collection-name");
    await nameInput.waitFor({ state: "visible" });
    await nameInput.fill("Library picks");
    await frame.locator("#library-batch-collection-create").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) => call.command === "collection_create" && call.args.name === "Library picks",
      );
    });
    // The created collection becomes the batch target; Add sends one op per selected photo.
    await poll(async () => (await batchSelect.inputValue()) !== "");
    await frame.locator("#library-batch-add-collection").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "review_batch"
          && call.args.ops?.length === 2
          && call.args.ops.every(
            (op) => op.op === "add_to_collection"
              && op.assetType === "photo"
              && Boolean(op.collectionId),
          ),
      );
    });
    await poll(async () =>
      /Added 2 assets to the collection\./.test(
        await visibleText(frame.locator("#library-message")),
      ));
    await poll(async () => bar.isHidden());

    // Batch re-index arms once through the two-step button, then queues one
    // reindex_asset per selected asset (the backend runs one ingest at a time).
    // The background snapshot carries a STALE cancelled ingest task from earlier
    // in the session (mock-bridge pre-seeds it): cancel detection must match this
    // batch's own job id, so the fresh batch still completes (review HIGH-1).
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["ControlOrMeta"] });
    const reindex = frame.locator("#reindex");
    await reindex.click();
    assert.equal(await visibleText(reindex), "Really re-index 2?");
    await reindex.click();
    await poll(async () =>
      (await mockCalls(page)).filter((call) => call.command === "reindex_asset").length === 2);
    await poll(async () =>
      /Re-indexed 2 assets\./.test(await visibleText(frame.locator("#library-message"))));

    // Batch remove reuses the single-remove dialog with the count in the copy.
    // Cancel first (the dialog is dismissable), then run the confirm path for real:
    // one remove_asset per selected asset, in table order, then the success message
    // and both rows gone (review LOW).
    await rows.nth(2).click();
    await rows.nth(3).click({ modifiers: ["ControlOrMeta"] });
    await frame.locator("#remove-asset").click();
    await frame.locator("#remove-asset-dialog").waitFor({ state: "visible" });
    assert.match(
      await visibleText(frame.locator("#remove-asset-copy")),
      /Remove 2 assets from the library\? Originals on disk are never touched\./,
    );
    await frame.locator("#remove-asset-cancel").click();
    assert.equal(await rows.count(), 4);
    await frame.locator("#remove-asset").click();
    await frame.locator("#remove-asset-dialog").waitFor({ state: "visible" });
    await frame.locator("#remove-asset-confirm").click();
    await poll(async () => {
      const removes = (await mockCalls(page))
        .filter((call) => call.command === "remove_asset")
        .map((call) => call.args.id);
      return removes.length === 2
        && removes[0] === "video-one"
        && removes[1] === "photo-three";
    });
    await poll(async () => (await rows.count()) === 2);
    await poll(async () =>
      /Removed 2 assets from your library\. The original files were not changed\./.test(
        await visibleText(frame.locator("#library-message")),
      ));
  },

  async "library-remove-partial"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const rows = frame.locator("#video-rows tr.video-row");
    await rows.first().waitFor({ state: "visible" });
    assert.equal(await rows.count(), 2);
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["ControlOrMeta"] });
    await frame.locator("#remove-asset").click();
    await frame.locator("#remove-asset-dialog").waitFor({ state: "visible" });
    await frame.locator("#remove-asset-confirm").click();
    // The second remove fails with a mappable backend error: the partial-failure
    // message carries the B8-mapped headline (review LOW), not the raw string.
    await poll(async () =>
      /Removed 1 of 2 — 1 could not be removed: Disk full — free up space on the drive, then try again\./.test(
        await visibleText(frame.locator("#library-message")),
      ));
    assert.doesNotMatch(
      await visibleText(frame.locator("#library-message")),
      /no space left on the device/,
    );
    // The raw backend text stays reachable through Copy details…
    const copyButton = frame.locator("#library-message .button.secondary.small");
    await copyButton.waitFor({ state: "visible" });
    await copyButton.click();
    const copyCalls = (await mockCalls(page)).filter(
      (call) => call.command === "clipboard.writeText",
    );
    assert.equal(copyCalls.length, 1);
    assert.match(copyCalls[0].args.text, /no space left on the device/);
    // …and a failed clipboard write says so in the message row instead of
    // failing silently (review LOW).
    await poll(async () =>
      /Could not copy — select the text manually/.test(
        await visibleText(frame.locator("#library-message")),
      ));
    // The removed asset left the selection; the failed one stays selected.
    await poll(async () => (await rows.count()) === 1);
    assert.equal(await visibleText(frame.locator("#library-batch-count")), "1 selected");
  },

  async "reindex-cancel"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const rows = frame.locator("#video-rows tr.video-row");
    await rows.first().waitFor({ state: "visible" });
    assert.equal(await rows.count(), 2);
    const reindex = frame.locator("#reindex");
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["ControlOrMeta"] });
    await reindex.click();
    assert.equal(await visibleText(reindex), "Really re-index 2?");
    await reindex.click();
    // The first asset's job really runs: the mock surfaces it as a running
    // background ingest, so Cancel appears and the queue waits on it.
    await frame.locator("#cancel").waitFor({ state: "visible" });
    await poll(async () =>
      (await mockCalls(page)).some(
        (call) => call.command === "reindex_asset" && call.args.id === "photo-one",
      ));
    // Cancelling the in-flight job stops the queue (review HIGH-1): the stop is
    // detected by matching THIS batch's job id, and the in-flight asset counts in
    // the not-re-indexed tally alongside the never-started one.
    await frame.locator("#cancel").click();
    await poll(async () =>
      /Re-index stopped — 2 assets not re-indexed\./.test(
        await visibleText(frame.locator("#library-message")),
      ));
    const reindexCalls = (await mockCalls(page))
      .filter((call) => call.command === "reindex_asset");
    assert.equal(reindexCalls.length, 1);
    assert.equal(reindexCalls[0].args.id, "photo-one");
    // The queue is really stopped: the second asset is never started.
    await poll(async () => (await frame.locator("#cancel").isHidden()));
    assert.equal(await reindex.isEnabled(), true);
  },

  async "reindex-stale-asset"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-library").click();
    const rows = frame.locator("#video-rows tr.video-row");
    await rows.first().waitFor({ state: "visible" });
    const reindex = frame.locator("#reindex");
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ["ControlOrMeta"] });
    await reindex.click();
    assert.equal(await visibleText(reindex), "Really re-index 2?");
    await reindex.click();
    // The second asset answers "asset … was not found" (removed mid-batch): the
    // queue must skip it, still start every queued asset, and finish with the
    // skip counted in the summary together with the mapped error (review LOW).
    await poll(async () => {
      const ids = (await mockCalls(page))
        .filter((call) => call.command === "reindex_asset")
        .map((call) => call.args.id);
      return ids.length === 2 && ids[0] === "photo-one" && ids[1] === "photo-two";
    });
    await poll(async () =>
      /Re-indexed 1 of 2 assets · 1 skipped — asset photo-two was not found\./.test(
        await visibleText(frame.locator("#library-message")),
      ));
  },

  async "search-error"(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("boom");
    await input.press("Enter");
    // Task 039 B8: internal vocabulary is mapped to editor language; the raw backend
    // text stays reachable through Copy details.
    assert.match(await visibleText(frame.locator("#search-error")), /Search could not run — the local search index is unavailable\./);
    assert.match(await visibleText(frame.locator("#search-error")), /Copy details/);
  },

  async "photo-export-detail"(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("rocket");
    await input.press("Enter");
    const cards = frame.locator(".result-card");
    await poll(async () => (await cards.count()) === 2);
    // Open the photo result; the drawer swaps video-only controls for the photo export path.
    await cards.filter({ hasText: "select.jpg" }).click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    assert.equal(await frame.locator("#export-clip").isHidden(), true);
    assert.equal(await frame.locator("#photo-export").isVisible(), true);
    await frame.locator("#photo-export-preset").selectOption("png-srgb-v1");
    await frame.locator("#export-photo").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "render_photo"
          && call.args.photoId === "photo-0"
          && call.args.preset === "png-srgb-v1"
          && call.args.destination === "/tmp/select_export.png",
      );
    });
    assert.match(await visibleText(frame.locator("#photo-export-status")), /Exported and verified/);
  },

  async "render-presets"(page) {
    // Preset facts are served by list_render_presets from the preset enums; every select
    // must render from that command and every save dialog must filter with its facts.
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("rocket");
    await input.press("Enter");
    const cards = frame.locator(".result-card");
    await poll(async () => (await cards.count()) === 2);
    await cards.filter({ hasText: "select.jpg" }).click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    await poll(async () => (await frame.locator("#photo-export-preset option").count()) === 3);
    const drawerOptions = frame.locator("#photo-export-preset option");
    assert.equal(await drawerOptions.nth(0).getAttribute("value"), "jpeg-srgb-v1");
    assert.equal((await drawerOptions.nth(2).textContent()).trim(), "TIFF — lossless 8-bit copy");
    assert.equal(
      (await mockCalls(page)).some((call) => call.command === "list_render_presets"),
      true,
    );

    // The Projects panel: reel select populated from the command, and the save dialog's
    // default path + filter come from the served preset facts.
    const plansFrame = await createPlan(page);
    await plansFrame.locator("#plan-generate").click();
    await poll(async () => (await plansFrame.locator("#plan-general .plans-candidate").count()) === 2);
    await plansFrame.locator("#plan-general .plans-candidate").first().locator("button").click();
    await poll(async () => (await plansFrame.locator("#plan-items .plans-item").count()) === 1);
    await poll(async () => (await plansFrame.locator("#project-photo-preset option").count()) === 2);
    const clipOptions = plansFrame.locator("#project-photo-preset option");
    assert.equal(await clipOptions.nth(0).getAttribute("value"), "mp4-h264-sdr-v1");
    assert.equal((await clipOptions.nth(1).textContent()).trim(), "MOV — editing-friendly H.264");
    await poll(async () => (await plansFrame.locator("#project-reel-preset option").count()) === 2);
    await plansFrame.locator("#project-reel-choose").click();
    let save = (await mockCalls(page)).filter((call) => call.command === "dialog.save").at(-1);
    assert.match(save.args.defaultPath, /\.mp4$/);
    assert.deepEqual(save.args.filters[0].extensions, ["mp4"]);
    await plansFrame.locator("#project-reel-preset").selectOption("mov-h264-sdr-v1");
    await plansFrame.locator("#project-reel-choose").click();
    save = (await mockCalls(page)).filter((call) => call.command === "dialog.save").at(-1);
    assert.match(save.args.defaultPath, /\.mov$/);
    assert.deepEqual(save.args.filters[0].extensions, ["mov"]);
  },

  async "feedback"(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("rocket");
    await input.press("Enter");
    const card = frame.locator(".result-card").first();
    await card.waitFor({ state: "visible" });
    await card.click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    assert.equal(await frame.locator("#detail-playback").isVisible(), true);
    assert.equal(Number(await frame.locator("#detail-scrubber").getAttribute("max")), 2.75);
    await frame.locator("#detail-loop").click();
    assert.equal(await frame.locator("#detail-loop").getAttribute("aria-pressed"), "true");
    await frame.locator("#feedback-pick").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) => call.command === "record_feedback" && call.args.signal === "pick",
      );
    });
    await frame.locator("#feedback-rating").selectOption("4");
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "record_feedback" && call.args.signal === "rating" && call.args.value === 4,
      );
    });
    // The select resets to the placeholder so the same rating can be recorded twice.
    assert.equal(await frame.locator("#feedback-rating").inputValue(), "");
    await poll(async () => {
      const message = (await frame.locator("#search-message").textContent()) ?? "";
      return message.includes("Rated 4 of 5.");
    });
    // Reopen-guard: closing the detail drawer must reset the player cache key so a later
    // reopen actually re-attaches the source (regression: dataset.src survived closeDetail,
    // the src assignment was skipped, and the player went blank without firing an error).
    await frame.locator("#detail-close").click();
    await frame.locator("#detail").waitFor({ state: "hidden" });
    await card.click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    assert.equal(
      await frame.locator("#detail-video").evaluate((node) => node.hasAttribute("src")),
      true,
      "reopened detail must re-attach the video source",
    );
  },

  async "style-panel"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-style").click();
    // Verdict conditions (docs/style-proof-review.md, 2026-08-31): a gate-passed profile
    // reads "Learned profile" and carries the visible plain-language scope line.
    await poll(async () =>
      (await visibleText(frame.locator("#style-status-line"))) === "Learned profile",
    );
    const scopeNote = frame.locator("#style-scope-note");
    await scopeNote.waitFor({ state: "visible" });
    assert.match(await visibleText(scopeNote), /held-out media/);
    assert.match(await visibleText(scopeNote), /not on unseen future work/);
    assert.match(await visibleText(frame.locator("#style-status-meta")), /Automated pair evaluation/);
    const rows = frame.locator("#style-sets .style-set-row");
    await rows.first().waitFor({ state: "visible" });
    assert.equal(await rows.count(), 2);
    assert.equal(await visibleText(rows.nth(0).locator(".status-pill")), "Confirmed");
    assert.equal(await visibleText(rows.nth(0).locator(".style-set-name")), "Launch selects");
    assert.equal(await visibleText(rows.nth(1).locator(".status-pill")), "Unconfirmed");
    assert.equal(await visibleText(rows.nth(1).locator(".style-set-name")), "Quiet travel film");
    assert.equal(await frame.locator("#style-empty").isHidden(), true);

    // Create: a new set starts unconfirmed — inert until confirmed.
    await frame.locator("#style-set-name").fill("Winter reel selects");
    await frame.locator("#style-set-context").fill("homepage-hero");
    await frame.locator("#style-create").click();
    await poll(async () => (await rows.count()) === 3);
    const created = rows.nth(2);
    assert.equal(await visibleText(created.locator(".style-set-name")), "Winter reel selects");
    assert.equal(await visibleText(created.locator(".status-pill")), "Unconfirmed");

    // Confirm: the status pill shows the state transition.
    await created.locator("button.secondary").click();
    await poll(async () => (await visibleText(created.locator(".status-pill"))) === "Confirmed");

    // Delete is two-step: arm, then really delete.
    const remove = created.locator("button.danger");
    await remove.click();
    assert.equal(await visibleText(remove), "Really delete?");
    await remove.click();
    await poll(async () => (await rows.count()) === 2);

    // Reset is two-step too and flips the profile to the general model.
    const reset = frame.locator("#style-reset");
    await reset.click();
    assert.equal(await visibleText(reset), "Really reset?");
    await reset.click();
    await poll(
      async () => (await visibleText(frame.locator("#style-status-line"))) === "General model only",
    );
    assert.equal(await frame.locator("#style-scope-note").isHidden(), true);
  },

  async "style-not-learned"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-style").click();
    // Verdict condition 1: a profile whose training gate did not pass keeps the
    // experimental copy, and no scope line is shown for it.
    await poll(
      async () =>
        (await visibleText(frame.locator("#style-status-line")))
          === "Experimental preferences · human review pending",
    );
    assert.equal(await frame.locator("#style-scope-note").isHidden(), true);
    assert.match(
      await visibleText(frame.locator("#style-status-meta")),
      /have not beaten the general model on held-out examples/,
    );
  },

  async "style-add-item"(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("rocket");
    await input.press("Enter");
    const card = frame.locator(".result-card").first();
    await card.waitFor({ state: "visible" });
    await card.click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    // The drawer loads the reference sets when the detail opens.
    const select = frame.locator("#detail-style-set");
    await select.waitFor({ state: "visible" });
    await poll(async () => (await select.locator("option").count()) === 3);
    await select.selectOption({ label: "Launch selects (Confirmed)" });
    await frame.locator("#detail-add-style").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "reference_set_add_item" &&
          call.args.setId === "set-confirmed" &&
          call.args.mediaKind === "video" &&
          call.args.mediaId === "shot-1",
      );
    });
    await poll(
      async () => (await visibleText(frame.locator("#detail-add-style"))) === "Added",
    );
  },

  async "library-grid"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    assert.match(await visibleText(frame.locator("#review-counts")), /2 photos · 1 shot/);
    assert.match(await visibleText(frame.locator("#review-counts")), /1 flagged/);
    // Kind badges distinguish photos from shots.
    assert.equal(await visibleText(tiles.nth(0).locator(".review-kind")), "PHOTO");
    assert.equal(await visibleText(tiles.nth(2).locator(".review-kind")), "▶ SHOT");
    // The flagged photo carries its safety pill and a stack indicator.
    assert.equal(await visibleText(tiles.nth(1).locator(".review-flag-pill.flagged")), "Blur required");
    assert.equal(await visibleText(tiles.nth(1).locator(".review-flag-pill.member")), "⧉ 1 stack");
    // Kind filter narrows the grid and reaches library_browse's filter argument.
    await frame.locator("#filter-kind").selectOption("photo");
    await frame.locator("#filter-apply").click();
    await poll(async () => (await tiles.count()) === 2);
    assert.match(await visibleText(frame.locator("#review-active-filters")), /^Showing:\s*Photos ×$/);
    const calls = await mockCalls(page);
    const browse = calls.filter((call) => call.command === "library_browse").at(-1);
    assert.equal(browse.args.filter.kind, "photo");
    await frame.locator("#filter-more").click();
    await frame.locator("#filter-reset").click();
    await poll(async () => (await tiles.count()) === 3);

    // Opening a shot keeps a real 16:9 player visible and reflows Review beside the drawer.
    // This guards the native regression where the flex column collapsed the video to zero
    // height: audio played, but the detail surface covered the grid and showed no picture.
    await frame.locator('.review-tile[data-key="shot|shot-1"]').click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    const playerBox = await frame.locator(".player-box").boundingBox();
    assert.ok(playerBox, "detail player must have a rendered box");
    assert.ok(playerBox.height >= 200, `detail player collapsed to ${playerBox.height}px`);
    assert.ok(
      Math.abs(playerBox.width / playerBox.height - 16 / 9) < 0.08,
      `detail player lost 16:9 layout (${playerBox.width}x${playerBox.height})`,
    );
    assert.equal(await frame.locator("#app-shell").getAttribute("class"), "app-shell detail-open");
    const reviewBox = await frame.locator("#review-view").boundingBox();
    const detailBox = await frame.locator("#detail").boundingBox();
    assert.ok(reviewBox && detailBox, "review and detail panes must both be rendered");
    assert.ok(
      reviewBox.x + reviewBox.width <= detailBox.x + 1,
      "detail pane must not cover the Review grid",
    );
    await frame.locator("#detail-close").click();
    assert.equal(await frame.locator("#app-shell").getAttribute("class"), "app-shell");
  },

  async "library-collections"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    // Select two tiles so the batch bar appears; with no collections yet the target
    // select must say so honestly instead of offering an empty list.
    await frame.locator("#review-grid .review-select input").nth(0).check();
    await frame.locator("#review-grid .review-select input").nth(1).check();
    const bar = frame.locator("#batch-bar");
    await poll(async () => bar.isVisible());
    const batchSelect = frame.locator("#batch-collection");
    assert.equal(
      (await batchSelect.locator("option").first().textContent())?.trim(),
      "No collections yet — create one to group assets",
    );
    assert.equal(await frame.locator("#batch-add-collection").isDisabled(), true);
    // "New collection…" swaps the select for an inline name field (no prompt dialog).
    await batchSelect.selectOption({ label: "New collection…" });
    const nameInput = frame.locator("#batch-collection-name");
    await nameInput.waitFor({ state: "visible" });
    await nameInput.fill("MacBook user test");
    await frame.locator("#batch-collection-create").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) => call.command === "collection_create" && call.args.name === "MacBook user test",
      );
    });
    // The created collection becomes the batch target without a second pick.
    await poll(async () => (await batchSelect.inputValue()) !== "");
    await frame.locator("#batch-add-collection").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "review_batch"
          && call.args.ops?.length === 2
          && call.args.ops.every(
            (op) => op.op === "add_to_collection" && Boolean(op.collectionId),
          ),
      );
    });
    await poll(async () =>
      /Added 2 assets to the collection\./.test(
        await visibleText(frame.locator("#review-message")),
      ));
    // The grid refreshes with the new membership pill on the previously uncollected tile.
    await poll(async () =>
      (await tiles.nth(1).locator(".review-flags").textContent())?.includes("▤ 1 collection") === true);
    // The filter select now offers the created collection too.
    await frame.locator("#filter-more").click();
    await poll(async () => {
      const options = await frame.locator("#filter-collection option").allTextContents();
      return options.includes("MacBook user test");
    });
  },

  async "library-feedback"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);

    // Batch-pick the first two tiles and rate the third ★ 4; the new Editorial filter must
    // reach library_browse as feedback + qualityMin and narrow the grid accordingly.
    await frame.locator("#review-grid .review-select input").nth(0).check();
    await frame.locator("#review-grid .review-select input").nth(1).check();
    await frame.locator("#batch-pick").click();
    await poll(async () => !(await frame.locator("#batch-bar").isVisible()));

    await frame.locator("#review-grid .review-select input").nth(2).check();
    await frame.locator("#batch-rating").selectOption("4");
    await poll(async () => !(await frame.locator("#batch-bar").isVisible()));

    await frame.locator("#filter-more").click();
    await frame.locator("#filter-feedback").selectOption("pick");
    await frame.locator("#filter-apply").click();
    await poll(async () => (await tiles.count()) === 2);
    assert.match(
      await visibleText(frame.locator("#review-active-filters")),
      /Showing:\s*Picked ×/,
    );
    const picked = (await mockCalls(page)).filter((call) => call.command === "library_browse").at(-1);
    assert.equal(picked.args.filter.feedback, "pick");

    // Minimum rating narrows to the assets rated at least that high (photo-one ★5, shot ★4, photo-two unrated).
    await frame.locator("#filter-feedback").selectOption("");
    await frame.locator("#filter-min-rating").selectOption("4");
    await frame.locator("#filter-apply").click();
    await poll(async () => (await tiles.count()) === 2);
    const rated = (await mockCalls(page)).filter((call) => call.command === "library_browse").at(-1);
    assert.equal(rated.args.filter.qualityMin, 4);
    await frame.locator("#filter-min-rating").selectOption("5");
    await frame.locator("#filter-apply").click();
    await poll(async () => (await tiles.count()) === 1);
    const top = (await mockCalls(page)).filter((call) => call.command === "library_browse").at(-1);
    assert.equal(top.args.filter.qualityMin, 5);

    await frame.locator("#filter-reset").click();
    await poll(async () => (await tiles.count()) === 3);
  },

  async "library-bulk"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    const checkboxes = frame.locator("#review-grid .review-select input");
    await checkboxes.nth(0).check();
    await checkboxes.nth(1).check();
    const bar = frame.locator("#batch-bar");
    await poll(async () => bar.isVisible());
    assert.equal(await visibleText(frame.locator("#batch-count")), "2 selected");
    await frame.locator("#batch-pick").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "review_batch"
          && call.args.ops?.length === 2
          && call.args.ops.every((op) => op.op === "pick" && op.assetType === "photo"),
      );
    });
    // Selection clears after the batch and the grid refreshes.
    await poll(async () => !(await bar.isVisible()));
    // Bulk rating flows through review_batch too.
    await checkboxes.nth(0).check();
    await poll(async () => bar.isVisible());
    await frame.locator("#batch-rating").selectOption("4");
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "review_batch"
          && call.args.ops?.length === 1
          && call.args.ops[0].op === "rate"
          && call.args.ops[0].rating === 4,
      );
    });
  },

  async "library-flags"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    // Open the flagged photo's drawer; the safety editor loads its current flags.
    await frame.locator('.review-tile[data-key="photo|photo-two"]').click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    await poll(async () => (await frame.locator("#safety-blur").isChecked()) === true);
    assert.equal(await frame.locator("#safety-usable").isChecked(), false);
    assert.equal(await frame.locator("#safety-faces").isChecked(), true);
    // Unchecking blur reduces protection, so the first click only arms the button.
    await frame.locator("#safety-blur").uncheck();
    const apply = frame.locator("#safety-apply");
    await poll(() => apply.isEnabled());
    await apply.click();
    assert.equal(await visibleText(apply), "Really apply?");
    await apply.click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "set_safety_flags"
          && call.args.blurRequired === false
          && call.args.facesVisible === true
          && call.args.usable === false,
      );
    });
    // Metadata editing diffs against the loaded annotation and reaches set_annotation.
    await frame.locator("#meta-notes").fill("Prefer the wider frame");
    await frame.locator("#metadata-save").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "set_annotation"
          && call.args.fields?.notes === "Prefer the wider frame"
          && call.args.fields.description === undefined,
      );
    });
    // The standout toggle is an explicit editorial decision, saved through set_annotation
    // and reflected by the drawer reload (never inferred from ordinary metadata edits).
    await frame.locator("#detail-standout").check();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "set_annotation"
          && call.args.fields?.standout === true,
      );
    });
    await poll(async () => (await frame.locator("#detail-standout").isChecked()) === true);
  },

  async "library-saved-search"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    await frame.locator("#filter-more").click();
    await frame.locator("#filter-blur").selectOption("true");
    await frame.locator("#filter-apply").click();
    await poll(async () => (await tiles.count()) === 1);
    // Save the current filters as a named search.
    await frame.locator("#saved-search-tools summary").click();
    await frame.locator("#saved-search-name").fill("Blur picks");
    await frame.locator("#saved-search-save").click();
    await poll(async () => {
      const calls = await mockCalls(page);
      const create = calls.find(
        (call) => call.command === "saved_search_create" && call.args.name === "Blur picks",
      );
      if (!create) return false;
      const filters = JSON.parse(create.args.filtersJson);
      return filters.blurRequired === true;
    });
    // Load the pre-seeded saved search; its filters_json replays into the filter bar.
    await frame.locator("#saved-search-select").selectOption({ label: "Blur review" });
    await frame.locator("#saved-search-load").click();
    assert.equal(await frame.locator("#filter-blur").inputValue(), "true");
    await poll(async () => {
      const calls = await mockCalls(page);
      const browse = calls.filter((call) => call.command === "library_browse").at(-1);
      return browse.args.filter.blurRequired === true;
    });
    // Delete is two-step, mirroring the reference-set pattern.
    const remove = frame.locator("#saved-search-delete");
    await remove.click();
    assert.equal(await visibleText(remove), "Really delete?");
    await remove.click();
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) => call.command === "saved_search_delete" && call.args.id === "ss-one",
      );
    });
  },

  async "compare-view"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    // The compare view is reachable from the detail drawer.
    await frame.locator('.review-tile[data-key="photo|photo-one"]').click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    await frame.locator("#compare-open").click();
    const dialog = frame.locator("#compare-dialog");
    await dialog.waitFor({ state: "visible" });
    await poll(async () => (await frame.locator("#compare-media-a img").count()) === 1);
    // Arrow keys focus a side; Enter records a prefer with the compared asset.
    await dialog.press("ArrowRight");
    await dialog.press("Enter");
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "record_feedback"
          && call.args.signal === "prefer"
          && call.args.id === "photo-two"
          && call.args.comparedId === "photo-one"
          && call.args.comparedAssetType === "photo"
          && call.args.value === null,
      );
    });
    await poll(
      async () => (await visibleText(frame.locator("#compare-status-b"))) !== "",
    );
    // p picks the focused side through record_feedback.
    await dialog.press("p");
    await poll(async () => {
      const calls = await mockCalls(page);
      return calls.some(
        (call) =>
          call.command === "record_feedback"
          && call.args.signal === "pick"
          && call.args.value === 1
          && call.args.id === "photo-two",
      );
    });
    await frame.locator("#compare-close").click();
  },

  async "compare-advance"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    await frame.locator('.review-tile[data-key="photo|photo-one"]').click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    await frame.locator("#compare-open").click();
    const dialog = frame.locator("#compare-dialog");
    await dialog.waitFor({ state: "visible" });
    await poll(async () => (await frame.locator("#compare-media-a img").count()) === 1);
    const selectB = frame.locator("#compare-select-b");
    await poll(async () => (await selectB.inputValue()) === "photo-two");
    const hint = frame.locator("#compare-advance-hint");
    const complete = frame.locator("#compare-complete");
    assert.equal(await hint.isHidden(), true);
    assert.equal(await complete.isHidden(), true);

    // Passive viewing never advances: time passes, nothing moves, no hint.
    await page.clock.fastForward(700);
    assert.equal(await selectB.inputValue(), "photo-two");
    assert.equal(await hint.isHidden(), true);

    // An explicit decision schedules the advance; the frozen clock holds it until the
    // harness fast-forwards past the --dur-advance beat (600 ms).
    await dialog.press("Enter"); // prefer focused side A (photo-one over photo-two)
    await poll(async () =>
      (await mockCalls(page)).some(
        (call) => call.command === "record_feedback" && call.args.signal === "prefer",
      ));
    assert.equal(await selectB.inputValue(), "photo-two");
    await page.clock.fastForward(700);
    await poll(async () => (await selectB.inputValue()) === "shot-1");
    assert.match(await visibleText(hint), /Advancing automatically\s+after each decision/);

    // The last comparison completes instead of advancing: the completion line appears
    // immediately (a state, not motion), side B stays put, and further time never wraps it.
    await dialog.press("p"); // pick focused side A (photo-one)
    await poll(async () =>
      (await mockCalls(page)).some(
        (call) => call.command === "record_feedback" && call.args.signal === "pick",
      ));
    await complete.waitFor({ state: "visible" });
    // Copy per the review fix: completion means end-of-pool, not "everything decided",
    // and the earlier prefer is counted (1 picked, 1 preferred).
    assert.equal(await visibleText(complete), "End of pool — 1 picked, 1 preferred.");
    await page.clock.fastForward(700);
    assert.equal(await selectB.inputValue(), "shot-1");
    await frame.locator("#compare-close").click();
  },

  async "compare-advance-reduced"(page) {
    // Context is created with emulated prefers-reduced-motion (see contextOptions below):
    // the advance must jump immediately with no fast-forwarding.
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-review").click();
    const tiles = frame.locator("#review-grid .review-tile");
    await poll(async () => (await tiles.count()) === 3);
    await frame.locator('.review-tile[data-key="photo|photo-one"]').click();
    await frame.locator("#detail").waitFor({ state: "visible" });
    await frame.locator("#compare-open").click();
    const dialog = frame.locator("#compare-dialog");
    await dialog.waitFor({ state: "visible" });
    const selectB = frame.locator("#compare-select-b");
    await poll(async () => (await selectB.inputValue()) === "photo-two");
    await dialog.press("Enter"); // prefer focused side A
    await poll(async () => (await selectB.inputValue()) === "shot-1");
    assert.match(
      await visibleText(frame.locator("#compare-advance-hint")),
      /Advancing automatically\s+after each decision/,
    );
    await frame.locator("#compare-close").click();
  },
};

const contextOptions = {
  "compare-advance-reduced": { reducedMotion: "reduce" },
};

const requested = process.argv.slice(2);
const names = requested.length ? requested : Object.keys(tests);
const unknown = names.filter((name) => !(name in tests));
if (unknown.length) {
  console.error(`Unknown scenario(s): ${unknown.join(", ")}`);
  console.error(`Available: ${Object.keys(tests).join(", ")}`);
  process.exit(1);
}

const browser = await chromium.launch(
  process.env.CRUSH_CHROME_PATH
    ? { executablePath: process.env.CRUSH_CHROME_PATH }
    : { channel: "chrome" },
).catch((error) => {
  console.error(
    `Could not launch system Chrome (${error}). Install Google Chrome or set CRUSH_CHROME_PATH.`,
  );
  process.exit(1);
});

let failures = 0;
for (const name of names) {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 900 },
    ...(contextOptions[name] || {}),
  });
  await context.addInitScript({ path: mockBridgePath });
  const page = await context.newPage();
  await page.clock.install();
  const url = pathToFileURL(harnessPath);
  url.searchParams.set("scenario", name);
  try {
    await page.goto(url.href);
    await tests[name](page);
    console.log(`ok ${name}`);
  } catch (error) {
    failures += 1;
    console.error(`FAIL ${name}: ${error?.stack || error}`);
  } finally {
    await context.close();
  }
}

await browser.close();
if (failures) {
  console.error(`${failures} harness scenario(s) failed`);
  process.exit(1);
}
