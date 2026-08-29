// Pairwise compare (Task 019b). Loaded after library.js; owns #compare-dialog. Reachable
// from the detail drawer ("Compare…"), it shows two assets side by side and records the
// user's preference through record_feedback with the compared asset — the strongest signal
// in the blueprint's feedback table. Keyboard-first: ←/→ focus a side, Enter prefers the
// focused side, p/x pick or reject it, 1–5 rates it, ⌥←/⌥→ swaps side B through the list.

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
  };

  const state = {
    assets: [],
    current: { a: null, b: null },
    focus: "a",
    detail: null,
    statusTimer: null,
  };

  const fileSrc = (path) => bridge.core.convertFileSrc(path);
  const fileName = (path) => path.split(/[\\/]/).at(-1) || path;
  const assetType = (kind) => (kind === "photo" ? "photo" : "video");

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
    try {
      state.assets = await invoke("library_browse", { filter: {} });
    } catch (error) {
      showStatus("a", String(error));
      return;
    }
    if (state.assets.length < 2) {
      showStatus("a", "Need at least two assets in the library to compare.");
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
    state.current.b = asset;
    renderSide("b");
  });

  el.close.addEventListener("click", () => el.dialog.close());
  el.dialog.addEventListener("click", (event) => {
    if (event.target === el.dialog) el.dialog.close();
  });
  el.dialog.addEventListener("keydown", onKeydown);
  el.dialog.addEventListener("close", () => {
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
