// Scripted UI harness (Task 027). Drives the real app UI — crates/app/ui/index.html loaded
// inside crates/app/tests/ui-harness.html — with a mock Tauri bridge (crates/app/tests/
// mock-bridge.js) injected before the app's scripts parse. Runs on system Chrome via
// playwright-core; no browser is ever downloaded. Override the binary with CRUSH_CHROME_PATH.
//
// Usage: npm run test:ui  (or: node scripts/ui-harness.mjs [scenario ...])
//
// Determinism: page.clock.install() freezes the app's 850 ms poll interval and 5 s message
// timers so re-renders cannot race assertions; the mock emits its events on microtasks.

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

const tests = {
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
    assert.equal(await frame.locator("#reindex").isDisabled(), true);
  },

  async "search-error"(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("boom");
    await input.press("Enter");
    assert.equal(
      await visibleText(frame.locator("#search-error")),
      "The vector store is unavailable.",
    );
  },

  async feedback(page) {
    const frame = page.frameLocator("#app-frame");
    const input = frame.locator("#search-input");
    await input.waitFor({ state: "visible" });
    await input.fill("rocket");
    await input.press("Enter");
    const card = frame.locator(".result-card").first();
    await card.waitFor({ state: "visible" });
    await card.click();
    await frame.locator("#detail").waitFor({ state: "visible" });
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
  },

  async "style-panel"(page) {
    const frame = page.frameLocator("#app-frame");
    await frame.locator("#nav-style").click();
    // Profile status comes from real profile data: learned=1 with eval-gate metrics.
    await poll(async () =>
      /Learned · held-out 0\.78 vs baseline 0\.61/.test(
        await visibleText(frame.locator("#style-status-line")),
      ));
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
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
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
