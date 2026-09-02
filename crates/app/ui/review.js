// Pairwise compare (Task 019b). Loaded after library.js; owns #compare-dialog. Reachable
// from the detail drawer ("Compare…"), it shows two assets side by side and records the
// user's preference through record_feedback with the compared asset — the strongest signal
// in the blueprint's feedback table. Keyboard-first: ←/→ focus a side, Enter prefers the
// focused side, p/x pick or reject it, 1–5 rates it, ⌥←/⌥→ swaps side B through the list.
// After an explicit decision (prefer, pick, reject, rating) side B auto-advances to the
// next candidate after a short beat, so a cull run flows (Task 039 C4, John's "yes"):
// passive viewing never advances, and the last comparison completes instead of advancing.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;

  const $ = (selector) => document.querySelector(selector);
  const el = {
    dialog: $("#compare-dialog"),
    close: $("#compare-close"),
    media: { a: $("#compare-media-a"), b: $("#compare-media-b") },
    select: { a: $("#compare-select-a"), b: $("#compare-select-b") },
    status: { a: $("#compare-status-a"), b: $("#compare-status-b") },
    side: { a: $("#compare-side-a"), b: $("#compare-side-b") },
    advanceHint: $("#compare-advance-hint"),
    complete: $("#compare-complete"),
  };

  const state = {
    assets: [],
    current: { a: null, b: null },
    focus: "a",
    detail: null,
    statusTimer: null,
    advanceTimer: null,
    // Session-level discoverability flag: the hint shows once per app session, the first
    // time an auto-advance is scheduled. Never a setting, never persisted.
    advanceHintShown: false,
    prefers: 0,
    picks: 0,
    rejects: 0,
  };

  const fileSrc = (path) => bridge.core.convertFileSrc(path);
  const fileName = (path) => path.split(/[\\/]/).at(-1) || path;
  const assetType = (kind) => (kind === "photo" ? "photo" : "video");

  // The decision beat is motion-system property, not a magic number: it reads the
  // --dur-advance token (600 ms) so the hold lives with the app's motion scale. Reduced-
  // motion users skip the wait entirely — the advance jumps immediately.
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const advanceDelay = () => {
    const raw = getComputedStyle(document.documentElement).getPropertyValue("--dur-advance");
    const value = Number.parseFloat(raw);
    return Number.isFinite(value) && value >= 0 ? value : 600;
  };

  function showStatus(side, text) {
    el.status[side].textContent = text;
    clearTimeout(state.statusTimer);
    state.statusTimer = setTimeout(() => {
      el.status.a.textContent = "";
      el.status.b.textContent = "";
    }, 4000);
  }

  function focusedAsset() {
    return state.current[state.focus];
  }

  function otherAsset() {
    return state.current[state.focus === "a" ? "b" : "a"];
  }

  function setFocus(side) {
    state.focus = side;
    el.side.a.classList.toggle("focused", side === "a");
    el.side.b.classList.toggle("focused", side === "b");
  }

  function renderSide(side) {
    const asset = state.current[side];
    const media = el.media[side];
    media.replaceChildren();
    el.select[side].value = asset ? asset.mediaId : "";
    if (!asset) return;
    const source = asset.thumbPath || asset.path;
    if (source) {
      const img = document.createElement("img");
      img.alt = fileName(asset.path);
      img.src = fileSrc(source);
      img.addEventListener("error", () => img.remove());
      media.append(img);
    }
  }

  function renderSides() {
    renderSide("a");
    renderSide("b");
    setFocus(state.focus);
  }

  // ---------- auto-advance (Task 039 C4) ----------
  // Side A stays anchored; side B walks forward through the pool (no wrap — the manual
  // ⌥arrows keep wrap for free browsing). A candidate equal to side A is skipped, so the
  // pair always shows two different assets. When no forward candidate remains this was the
  // last comparison: the dialog shows the completion count instead of advancing.
  function nextBCandidate() {
    const index = state.assets.findIndex((asset) => sameAsset(asset, state.current.b));
    for (let next = index + 1; next < state.assets.length; next += 1) {
      if (!sameAsset(state.assets[next], state.current.a)) return state.assets[next];
    }
    return null;
  }

  // End-of-pool completion (fires when no forward candidate remains — not a claim that
  // everything in the pool was decided). Counts what this dialog session actually recorded;
  // zero terms are omitted.
  function showCompletion() {
    const parts = [];
    if (state.picks) parts.push(`${state.picks} picked`);
    if (state.rejects) parts.push(`${state.rejects} rejected`);
    if (state.prefers) parts.push(`${state.prefers} preferred`);
    el.complete.textContent = parts.length
      ? `End of pool — ${parts.join(", ")}.`
      : "End of pool — nothing picked or rejected.";
    el.complete.hidden = false;
  }

  function hideCompletion() {
    el.complete.hidden = true;
  }

  function cancelAdvance() {
    clearTimeout(state.advanceTimer);
    state.advanceTimer = null;
  }

  function advance() {
    state.advanceTimer = null;
    if (!el.dialog.open) return;
    const candidate = nextBCandidate();
    if (!candidate) {
      showCompletion();
      return;
    }
    state.current.b = candidate;
    renderSide("b");
  }

  function scheduleAdvance() {
    const candidate = nextBCandidate();
    if (!candidate) {
      cancelAdvance();
      showCompletion();
      return;
    }
    if (!state.advanceHintShown) {
      state.advanceHintShown = true;
      el.advanceHint.hidden = false;
    }
    // One pending advance at a time: a second decision before the beat resets the hold
    // rather than queueing a second jump — only the current side B was judged twice.
    cancelAdvance();
    if (reduceMotion.matches) {
      advance();
      return;
    }
    state.advanceTimer = setTimeout(advance, advanceDelay());
  }

  function fillSelects() {
    for (const side of ["a", "b"]) {
      const select = el.select[side];
      select.replaceChildren();
      for (const asset of state.assets) {
        const option = document.createElement("option");
        option.value = asset.mediaId;
        option.textContent = `${fileName(asset.path)} (${asset.mediaKind === "photo" ? "photo" : "shot"})`;
        option.dataset.key = `${asset.mediaKind}|${asset.mediaId}`;
        select.append(option);
      }
    }
  }

  function assetById(mediaId) {
    return state.assets.find((asset) => asset.mediaId === mediaId) || null;
  }

  function sameAsset(left, right) {
    return left && right && left.mediaKind === right.mediaKind && left.mediaId === right.mediaId;
  }

  async function openCompare() {
    cancelAdvance();
    hideCompletion();
    state.prefers = 0;
    state.picks = 0;
    state.rejects = 0;
    // advanceHintShown is deliberately NOT reset: the hint is once per app session.
    el.advanceHint.hidden = true;
    let excludedSpans = 0;
    try {
      const base = window.__crushContext?.reviewFilters || {};
      const assets = await invoke("library_browse", { filter: base });
      // Task 034: imported clips cannot enter the pairwise pool — prefer needs
      // compared-media semantics and vectors, and spans have neither. They are excluded
      // up front and the pool says so rather than pretending they were judged.
      excludedSpans = assets.filter((asset) => asset.mediaKind === "span").length;
      state.assets = assets.filter((asset) => asset.mediaKind !== "span");
    } catch (error) {
      showStatus("a", String(error));
      return;
    }
    const MAX_POOL = 300;
    if (state.assets.length > MAX_POOL) {
      state.assets = state.assets.slice(0, MAX_POOL);
      showStatus(
        "a",
        `Comparing the first ${MAX_POOL} assets — tighten the Review filters to focus the pool.`,
      );
    }
    if (state.assets.length < 2) {
      showStatus(
        "a",
        excludedSpans
          ? "Imported clips cannot be compared yet — compare needs analysed media. Remove the clip filters to compare photos and shots."
          : "Need at least two assets in this pool to compare.",
      );
      return;
    }
    fillSelects();
    const detailAsset = state.detail
      ? state.assets.find((asset) => asset.mediaKind === (state.detail.kind === "photo" ? "photo" : "shot")
        && asset.mediaId === state.detail.id)
      : null;
    state.current.a = detailAsset || state.assets[0];
    state.current.b = state.assets.find((asset) => !sameAsset(asset, state.current.a));
    state.focus = "a";
    renderSides();
    el.dialog.showModal();
  }

  async function record(signal, value = null) {
    const asset = focusedAsset();
    const compared = otherAsset();
    if (!asset) return;
    if (signal === "prefer" && sameAsset(asset, compared)) {
      showStatus(state.focus, "Choose two different assets first.");
      return;
    }
    const args = {
      assetType: assetType(asset.mediaKind),
      id: asset.mediaId,
      signal,
      value,
      context: null,
    };
    if (signal === "prefer") {
      args.comparedAssetType = assetType(compared.mediaKind);
      args.comparedId = compared.mediaId;
    }
    // Snapshot side B at decision time: while the record is in flight the user can still
    // navigate (⌥arrows, selects), and an advance scheduled after the invoke resolves would
    // jump B off the pair the user manually moved to.
    const bAtDecision = state.current.b;
    try {
      await invoke("record_feedback", args);
      showStatus(
        state.focus,
        signal === "prefer"
          ? `Preferred over the other side.`
          : signal === "pick"
            ? "Marked as a pick."
            : signal === "reject"
              ? "Marked as rejected."
              : `Rated ${value} of 5.`,
      );
      // Auto-advance fires only here, on an explicit recorded decision (prefer, pick,
      // reject, rating) — never on passive viewing, focus changes, or failed records.
      if (signal === "prefer") state.prefers += 1;
      if (signal === "pick") state.picks += 1;
      if (signal === "reject") state.rejects += 1;
      // If B moved manually while the record was in flight, stand down: no auto-jump off
      // the user's navigation and no completion overwrite from the stale decision.
      if (sameAsset(bAtDecision, state.current.b) && el.dialog.open) scheduleAdvance();
    } catch (error) {
      showStatus(state.focus, String(error));
    }
  }

  function shiftB(delta) {
    if (!state.assets.length) return;
    const index = state.assets.findIndex((asset) => sameAsset(asset, state.current.b));
    const next = (index + delta + state.assets.length) % state.assets.length;
    const candidate = state.assets[next];
    if (sameAsset(candidate, state.current.a)) return;
    // Manual navigation takes control back: no pending auto-jump, no stale completion.
    cancelAdvance();
    hideCompletion();
    state.current.b = candidate;
    renderSide("b");
  }

  function onKeydown(event) {
    if (event.target.tagName === "SELECT") return;
    const alt = event.altKey;
    if (alt && event.key === "ArrowRight") {
      event.preventDefault();
      shiftB(1);
      return;
    }
    if (alt && event.key === "ArrowLeft") {
      event.preventDefault();
      shiftB(-1);
      return;
    }
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      setFocus("a");
      return;
    }
    if (event.key === "ArrowRight") {
      event.preventDefault();
      setFocus("b");
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      record("prefer");
      return;
    }
    const key = event.key.toLowerCase();
    if (key === "p") {
      event.preventDefault();
      record("pick", 1);
    } else if (key === "x") {
      event.preventDefault();
      record("reject", -1);
    } else if (["1", "2", "3", "4", "5"].includes(key)) {
      event.preventDefault();
      record("rating", Number(key));
    }
  }

  el.select.a.addEventListener("change", () => {
    const asset = assetById(el.select.a.value);
    if (!asset) return;
    if (sameAsset(asset, state.current.b)) {
      el.select.a.value = state.current.a ? state.current.a.mediaId : "";
      showStatus("a", "That asset is already on side B.");
      return;
    }
    cancelAdvance();
    hideCompletion();
    state.current.a = asset;
    renderSide("a");
  });
  el.select.b.addEventListener("change", () => {
    const asset = assetById(el.select.b.value);
    if (!asset) return;
    if (sameAsset(asset, state.current.a)) {
      el.select.b.value = state.current.b ? state.current.b.mediaId : "";
      showStatus("b", "That asset is already on side A.");
      return;
    }
    cancelAdvance();
    hideCompletion();
    state.current.b = asset;
    renderSide("b");
  });

  el.close.addEventListener("click", () => el.dialog.close());
  el.dialog.addEventListener("click", (event) => {
    if (event.target === el.dialog) el.dialog.close();
  });
  el.dialog.addEventListener("keydown", onKeydown);
  el.dialog.addEventListener("close", () => {
    cancelAdvance();
    hideCompletion();
    el.status.a.textContent = "";
    el.status.b.textContent = "";
  });

  document.querySelector("#compare-open").addEventListener("click", () => {
    openCompare();
  });
  document.addEventListener("crush:detail", (event) => {
    state.detail = event.detail;
  });
})();
