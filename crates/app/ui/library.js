// Review library (Task 019b). Loaded after style.js; owns #review-view and the safety,
// metadata, and version-stack blocks inside the shared asset detail drawer. Follows the
// established module pattern: createElement/textContent only, no innerHTML, no network,
// invoke only. Bulk review goes through review_batch; the safety columns are written only
// by set_safety_flags in response to this explicit user action — machine scores never
// touch them, and reducing protection demands a second confirming click.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;

  const $ = (selector) => document.querySelector(selector);
  const el = {
    message: $("#review-message"),
    counts: $("#review-counts"),
    filters: $("#review-filters"),
    kind: $("#filter-kind"),
    status: $("#filter-status"),
    usable: $("#filter-usable"),
    blur: $("#filter-blur"),
    collection: $("#filter-collection"),
    stack: $("#filter-stack"),
    context: $("#filter-context"),
    search: $("#filter-search"),
    reset: $("#filter-reset"),
    savedSelect: $("#saved-search-select"),
    savedLoad: $("#saved-search-load"),
    savedDelete: $("#saved-search-delete"),
    savedName: $("#saved-search-name"),
    savedForm: $("#saved-search-form"),
    batchBar: $("#batch-bar"),
    batchCount: $("#batch-count"),
    batchPick: $("#batch-pick"),
    batchReject: $("#batch-reject"),
    batchRating: $("#batch-rating"),
    batchCollection: $("#batch-collection"),
    batchAdd: $("#batch-add-collection"),
    batchClear: $("#batch-clear"),
    empty: $("#review-empty"),
    grid: $("#review-grid"),
    safetyFaces: $("#safety-faces"),
    safetyNametags: $("#safety-nametags"),
    safetyBlur: $("#safety-blur"),
    safetyUsable: $("#safety-usable"),
    safetyApply: $("#safety-apply"),
    metaDescription: $("#meta-description"),
    metaSubjects: $("#meta-subjects"),
    metaAction: $("#meta-action"),
    metaTags: $("#meta-tags"),
    metaNotes: $("#meta-notes"),
    metadataSave: $("#metadata-save"),
    detailStack: $("#detail-stack"),
    detailStackRole: $("#detail-stack-role"),
    detailAddStack: $("#detail-add-stack"),
    stackNewName: $("#stack-new-name"),
    stackCreate: $("#stack-create"),
    stackMemberships: $("#detail-stack-memberships"),
  };

  const state = {
    counts: null,
    collections: [],
    stacks: [],
    saved: [],
    assets: [],
    selection: new Map(),
    detail: null,
    safety: null,
    metadata: null,
    stacksForAsset: [],
    messageTimer: null,
    applyArmed: false,
    deleteArmed: null,
    deleteTimer: null,
  };

  const fileSrc = (path) => bridge.core.convertFileSrc(path);
  const fileName = (path) => path.split(/[\\/]/).at(-1) || path;
  // The store talks about photo/shot kinds; the review commands use the UI vocabulary
  // "photo"/"video" (mirroring record_feedback).
  const assetType = (kind) => (kind === "photo" ? "photo" : "video");
  const pad = (value) => String(value).padStart(2, "0");

  function timecode(seconds) {
    if (!Number.isFinite(seconds)) return "—";
    const total = Math.max(0, Math.floor(seconds));
    return `${pad(Math.floor(total / 3600))}:${pad(Math.floor((total % 3600) / 60))}:${pad(total % 60)}`;
  }

  function showMessage(text, error = false) {
    clearTimeout(state.messageTimer);
    el.message.textContent = text;
    el.message.classList.toggle("error", error);
    el.message.hidden = false;
    state.messageTimer = setTimeout(() => {
      el.message.hidden = true;
    }, 5000);
  }

  // ---------- filters ----------
  function filterArgs() {
    const args = {};
    if (el.kind.value) args.kind = el.kind.value;
    if (el.status.value) args.status = el.status.value;
    if (el.usable.value) args.usable = el.usable.value === "true";
    if (el.blur.value) args.blurRequired = el.blur.value === "true";
    if (el.collection.value) args.collectionId = el.collection.value;
    if (el.stack.value) args.stackId = el.stack.value;
    if (el.context.value.trim()) args.contextKey = el.context.value.trim();
    if (el.search.value.trim()) args.search = el.search.value.trim();
    return args;
  }

  function applyFilterArgs(args = {}) {
    el.kind.value = args.kind || "";
    el.status.value = args.status || "";
    el.usable.value = args.usable === undefined || args.usable === null ? "" : String(args.usable);
    el.blur.value =
      args.blurRequired === undefined || args.blurRequired === null ? "" : String(args.blurRequired);
    el.collection.value = state.collections.some((set) => set.id === args.collectionId)
      ? args.collectionId
      : "";
    el.stack.value = state.stacks.some((stack) => stack.id === args.stackId) ? args.stackId : "";
    el.context.value = args.contextKey || "";
    el.search.value = args.search || "";
  }

  function renderFilterOptions() {
    const keep = (select, values) => {
      const current = select.value;
      select.replaceChildren();
      for (const [value, label] of values) {
        const option = document.createElement("option");
        option.value = value;
        option.textContent = label;
        select.append(option);
      }
      select.value = values.some(([value]) => value === current) ? current : "";
    };
    keep(
      el.collection,
      [["", "Any collection"], ...state.collections.map((set) => [set.id, set.name])],
    );
    keep(el.stack, [["", "Any stack"], ...state.stacks.map((stack) => [stack.id, stack.name])]);
  }

  function renderCounts() {
    const counts = state.counts;
    el.counts.textContent = counts
      ? `${counts.photos} photo${counts.photos === 1 ? "" : "s"} · ${counts.shots} shot${counts.shots === 1 ? "" : "s"} · ${counts.picks} picks · ${counts.rejects} rejects · ${counts.flagged} flagged`
      : "";
  }

  // ---------- grid ----------
  const statusTones = {
    done: "done",
    failed: "failed",
    pending: "pending",
  };

  function tileBadge(asset) {
    const badge = document.createElement("span");
    badge.className = "review-kind";
    badge.textContent = asset.mediaKind === "photo" ? "PHOTO" : "▶ SHOT";
    return badge;
  }

  function tileThumb(asset) {
    const thumb = document.createElement("div");
    thumb.className = "review-thumb";
    if (asset.blurRequired) thumb.classList.add("blur");
    if (asset.thumbPath) {
      const img = document.createElement("img");
      img.loading = "lazy";
      img.decoding = "async";
      img.alt = "";
      img.src = fileSrc(asset.thumbPath);
      img.addEventListener("error", () => img.remove());
      thumb.append(img);
    }
    thumb.append(tileBadge(asset));
    const pill = document.createElement("span");
    pill.className = `status-pill ${statusTones[asset.status] || "active"}`;
    pill.textContent = asset.status;
    thumb.append(pill);
    return thumb;
  }

  function tileMeta(asset) {
    const meta = document.createElement("div");
    meta.className = "review-meta";
    if (asset.mediaKind === "shot") {
      meta.textContent = `${timecode(asset.startS)} → ${timecode(asset.endS)}`;
    } else if (asset.width && asset.height) {
      meta.textContent = `${asset.width} × ${asset.height}`;
    }
    return meta;
  }

  function tileFlags(asset) {
    const flags = document.createElement("div");
    flags.className = "review-flags";
    if (asset.quality) {
      const pill = document.createElement("span");
      pill.className = "review-flag-pill quality";
      pill.textContent = `★ ${asset.quality}`;
      flags.append(pill);
    }
    if (asset.blurRequired || !asset.usable) {
      const pill = document.createElement("span");
      pill.className = "review-flag-pill flagged";
      pill.textContent = asset.blurRequired ? "Blur required" : "Unusable";
      flags.append(pill);
    }
    if (asset.stackIds.length) {
      const pill = document.createElement("span");
      pill.className = "review-flag-pill member";
      pill.textContent = `⧉ ${asset.stackIds.length} stack${asset.stackIds.length === 1 ? "" : "s"}`;
      flags.append(pill);
    }
    if (asset.collectionIds.length) {
      const pill = document.createElement("span");
      pill.className = "review-flag-pill member";
      pill.textContent = `▤ ${asset.collectionIds.length} collection${asset.collectionIds.length === 1 ? "" : "s"}`;
      flags.append(pill);
    }
    return flags;
  }

  function tile(asset) {
    const key = `${asset.mediaKind}|${asset.mediaId}`;
    const tile = document.createElement("div");
    tile.className = "review-tile";
    tile.dataset.key = key;
    if (state.selection.has(key)) tile.classList.add("selected");

    const selectLabel = document.createElement("label");
    selectLabel.className = "review-select";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = state.selection.has(key);
    checkbox.setAttribute("aria-label", `Select ${fileName(asset.path)}`);
    checkbox.addEventListener("change", () => {
      if (checkbox.checked) state.selection.set(key, asset);
      else state.selection.delete(key);
      tile.classList.toggle("selected", checkbox.checked);
      renderBatchBar();
    });
    selectLabel.append(checkbox);
    tile.append(selectLabel);

    tile.append(tileThumb(asset));

    const name = document.createElement("div");
    name.className = "file-name review-name";
    name.textContent = fileName(asset.path);
    name.title = asset.path;
    tile.append(name, tileMeta(asset), tileFlags(asset));

    tile.addEventListener("click", (event) => {
      // The select checkbox (or its label) must not open the drawer.
      if (event.target.closest(".review-select")) return;
      document.dispatchEvent(new CustomEvent("crush:open-asset", {
        detail: { asset_type: assetType(asset.mediaKind), asset_id: asset.mediaId },
      }));
    });
    return tile;
  }

  function renderGrid() {
    el.grid.replaceChildren();
    el.empty.hidden = state.assets.length > 0;
    el.grid.hidden = state.assets.length === 0;
    for (const asset of state.assets) el.grid.append(tile(asset));
  }

  // ---------- batch bar ----------
  function renderBatchBar() {
    const count = state.selection.size;
    el.batchBar.hidden = count === 0;
    el.batchCount.textContent = `${count} selected`;
    el.batchPick.disabled = count === 0;
    el.batchReject.disabled = count === 0;
    el.batchAdd.disabled = count === 0 || !el.batchCollection.value;
  }

  function selectionOps(op, extra = {}) {
    const ops = [];
    for (const asset of state.selection.values()) {
      ops.push({
        op,
        assetType: assetType(asset.mediaKind),
        mediaId: asset.mediaId,
        ...extra,
      });
    }
    return ops;
  }

  async function runBatch(ops, summary) {
    try {
      const applied = await invoke("review_batch", { ops });
      showMessage(summary(applied));
      state.selection.clear();
      await refreshReview();
    } catch (error) {
      showMessage(String(error), true);
    }
  }

  // ---------- saved searches ----------
  function renderSavedSearches() {
    const current = el.savedSelect.value;
    el.savedSelect.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = state.saved.length ? "Saved searches…" : "No saved searches yet";
    el.savedSelect.append(placeholder);
    for (const saved of state.saved) {
      const option = document.createElement("option");
      option.value = saved.id;
      option.textContent = saved.name;
      el.savedSelect.append(option);
    }
    el.savedSelect.value = state.saved.some((saved) => saved.id === current) ? current : "";
    const selected = Boolean(el.savedSelect.value);
    el.savedLoad.disabled = !selected;
    el.savedDelete.disabled = !selected;
  }

  function disarmDelete() {
    clearTimeout(state.deleteTimer);
    if (state.deleteArmed) {
      state.deleteArmed.button.textContent = "Delete";
      state.deleteArmed.button.classList.remove("armed");
      state.deleteArmed = null;
    }
  }

  // ---------- rendering ----------
  function renderAll() {
    renderCounts();
    renderFilterOptions();
    renderSavedSearches();
    renderGrid();
    renderBatchBar();
  }

  async function refreshReview() {
    try {
      const [counts, collections, stacks, saved, assets] = await Promise.all([
        invoke("library_counts"),
        invoke("collection_list"),
        invoke("stack_list"),
        invoke("saved_search_list"),
        invoke("library_browse", { filter: filterArgs() }),
      ]);
      state.counts = counts;
      state.collections = collections;
      state.stacks = stacks;
      state.saved = saved;
      state.assets = assets;
      for (const key of [...state.selection.keys()]) {
        if (!assets.some((asset) => `${asset.mediaKind}|${asset.mediaId}` === key)) {
          state.selection.delete(key);
        }
      }
    } catch (error) {
      showMessage(String(error), true);
    }
    renderAll();
  }

  // ---------- detail drawer: safety, metadata, stacks ----------
  function readSafetyFlags() {
    return {
      facesVisible: el.safetyFaces.checked,
      nametagsVisible: el.safetyNametags.checked,
      blurRequired: el.safetyBlur.checked,
      usable: el.safetyUsable.checked,
    };
  }

  function safetyDirty() {
    const loaded = state.safety;
    if (!loaded) return false;
    const flags = readSafetyFlags();
    return flags.facesVisible !== loaded.facesVisible
      || flags.nametagsVisible !== loaded.nametagsVisible
      || flags.blurRequired !== loaded.blurRequired
      || flags.usable !== loaded.usable;
  }

  function renderSafety() {
    const loaded = state.safety;
    el.safetyFaces.checked = loaded ? loaded.facesVisible : false;
    el.safetyNametags.checked = loaded ? loaded.nametagsVisible : false;
    el.safetyBlur.checked = loaded ? loaded.blurRequired : false;
    el.safetyUsable.checked = loaded ? loaded.usable : true;
    disarmApply();
    el.safetyApply.disabled = !loaded;
  }

  function disarmApply() {
    state.applyArmed = false;
    el.safetyApply.textContent = "Apply flags";
    el.safetyApply.classList.remove("armed");
  }

  // Metadata edits are diffs against the loaded annotation so unchanged fields never
  // append spurious feedback signals.
  function metadataPatch() {
    const loaded = state.metadata;
    if (!loaded) return null;
    const patch = {};
    const pairs = [
      ["description", el.metaDescription],
      ["subjects", el.metaSubjects],
      ["action", el.metaAction],
      ["tags", el.metaTags],
      ["notes", el.metaNotes],
    ];
    for (const [field, input] of pairs) {
      if (input.value !== loaded[field]) patch[field] = input.value;
    }
    return patch;
  }

  function renderMetadata() {
    const loaded = state.metadata;
    el.metaDescription.value = loaded ? loaded.description : "";
    el.metaSubjects.value = loaded ? loaded.subjects : "";
    el.metaAction.value = loaded ? loaded.action : "";
    el.metaTags.value = loaded ? loaded.tags : "";
    el.metaNotes.value = loaded ? loaded.notes : "";
    el.metadataSave.disabled = !loaded;
  }

  function renderStacks() {
    el.detailStack.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = state.stacks.length ? "Choose stack…" : "No stacks yet";
    el.detailStack.append(placeholder);
    for (const stack of state.stacks) {
      const option = document.createElement("option");
      option.value = stack.id;
      option.textContent = stack.name;
      el.detailStack.append(option);
    }
    el.detailStack.value = "";
    el.detailAddStack.disabled = true;

    el.stackMemberships.replaceChildren();
    if (!state.stacksForAsset.length) {
      const empty = document.createElement("p");
      empty.className = "stack-memberships-empty";
      empty.textContent = "Not in any version stack.";
      el.stackMemberships.append(empty);
      return;
    }
    for (const stack of state.stacksForAsset) {
      const row = document.createElement("div");
      row.className = "stack-membership-row";
      const name = document.createElement("span");
      name.className = "stack-membership-name";
      name.textContent = stack.name;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "button danger small";
      remove.textContent = "Remove";
      remove.addEventListener("click", async () => {
        remove.disabled = true;
        try {
          await invoke("stack_remove_item", {
            stackId: stack.id,
            assetType: assetType(state.detail.kind),
            mediaId: state.detail.id,
          });
          showMessage(`Removed from “${stack.name}”.`);
          await refreshDetailState();
        } catch (error) {
          showMessage(String(error), true);
          remove.disabled = false;
        }
      });
      row.append(name, remove);
      el.stackMemberships.append(row);
    }
  }

  async function refreshDetailState() {
    const detail = state.detail;
    if (!detail) {
      state.safety = null;
      state.metadata = null;
      state.stacksForAsset = [];
      renderSafety();
      renderMetadata();
      renderStacks();
      return;
    }
    try {
      const [annotation, stacks, memberships] = await Promise.all([
        invoke("editorial_annotation_get", { assetType: detail.kind, id: detail.id }),
        invoke("stack_list"),
        invoke("stacks_for_asset", { assetType: detail.kind, mediaId: detail.id }),
      ]);
      state.safety = annotation;
      state.metadata = annotation;
      state.stacks = stacks;
      state.stacksForAsset = memberships;
    } catch (error) {
      showMessage(String(error), true);
    }
    renderSafety();
    renderMetadata();
    renderStacks();
  }

  // ---------- wiring ----------
  el.filters.addEventListener("submit", (event) => {
    event.preventDefault();
    refreshReview();
  });
  el.reset.addEventListener("click", () => {
    applyFilterArgs({});
    refreshReview();
  });

  el.savedForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const name = el.savedName.value.trim();
    if (!name) return;
    const args = filterArgs();
    el.savedName.disabled = true;
    (async () => {
      await invoke("saved_search_create", {
        name,
        query: "",
        contextKey: args.contextKey || "default",
        filtersJson: JSON.stringify(args),
      });
      el.savedName.value = "";
      showMessage(`Saved search “${name}”.`);
      await refreshReview();
    })().catch((error) => showMessage(String(error), true)).finally(() => {
      el.savedName.disabled = false;
    });
  });

  el.savedSelect.addEventListener("change", () => {
    const selected = Boolean(el.savedSelect.value);
    el.savedLoad.disabled = !selected;
    el.savedDelete.disabled = !selected;
    if (!selected) disarmDelete();
  });

  el.savedLoad.addEventListener("click", () => {
    const saved = state.saved.find((candidate) => candidate.id === el.savedSelect.value);
    if (!saved) return;
    let filters = {};
    try {
      filters = JSON.parse(saved.filtersJson || "{}");
    } catch {
      showMessage("That saved search has unreadable filters.", true);
      return;
    }
    applyFilterArgs(filters);
    refreshReview();
  });

  el.savedDelete.addEventListener("click", () => {
    const saved = state.saved.find((candidate) => candidate.id === el.savedSelect.value);
    if (!saved) return;
    if (state.deleteArmed?.id !== saved.id) {
      disarmDelete();
      state.deleteArmed = { id: saved.id, button: el.savedDelete };
      el.savedDelete.textContent = "Really delete?";
      el.savedDelete.classList.add("armed");
      state.deleteTimer = setTimeout(disarmDelete, 6000);
      return;
    }
    disarmDelete();
    el.savedDelete.disabled = true;
    invoke("saved_search_delete", { id: saved.id })
      .then(() => {
        showMessage(`Deleted “${saved.name}”.`);
        return refreshReview();
      })
      .catch((error) => showMessage(String(error), true));
  });

  el.batchPick.addEventListener("click", () =>
    runBatch(selectionOps("pick"), (applied) => `Marked ${applied} as picks.`));
  el.batchReject.addEventListener("click", () =>
    runBatch(selectionOps("reject"), (applied) => `Marked ${applied} as rejected.`));
  el.batchRating.addEventListener("change", () => {
    const rating = Number(el.batchRating.value);
    el.batchRating.value = "";
    if (rating && state.selection.size) {
      runBatch(selectionOps("rate", { rating }), (applied) => `Rated ${applied} assets.`);
    }
  });
  el.batchCollection.addEventListener("change", renderBatchBar);
  el.batchAdd.addEventListener("click", () => {
    const collectionId = el.batchCollection.value;
    if (!collectionId || !state.selection.size) return;
    runBatch(
      selectionOps("add_to_collection", { collectionId }),
      (applied) => `Added ${applied} asset${applied === 1 ? "" : "s"} to the collection.`,
    );
  });
  el.batchClear.addEventListener("click", () => {
    state.selection.clear();
    renderGrid();
    renderBatchBar();
  });

  el.safetyFaces.addEventListener("change", () => {
    el.safetyApply.disabled = !safetyDirty();
  });
  el.safetyNametags.addEventListener("change", () => {
    el.safetyApply.disabled = !safetyDirty();
  });
  el.safetyBlur.addEventListener("change", () => {
    el.safetyApply.disabled = !safetyDirty();
  });
  el.safetyUsable.addEventListener("change", () => {
    el.safetyApply.disabled = !safetyDirty();
  });

  el.safetyApply.addEventListener("click", async () => {
    const detail = state.detail;
    const loaded = state.safety;
    if (!detail || !loaded || !safetyDirty()) return;
    const flags = readSafetyFlags();
    // Reducing protection (clearing a privacy/blur flag or marking an unusable asset
    // usable again) demands a second, explicit confirming click.
    const risky = (loaded.facesVisible && !flags.facesVisible)
      || (loaded.nametagsVisible && !flags.nametagsVisible)
      || (loaded.blurRequired && !flags.blurRequired)
      || (!loaded.usable && flags.usable);
    if (risky && !state.applyArmed) {
      state.applyArmed = true;
      el.safetyApply.textContent = "Really apply?";
      el.safetyApply.classList.add("armed");
      return;
    }
    disarmApply();
    el.safetyApply.disabled = true;
    try {
      await invoke("set_safety_flags", {
        assetType: detail.kind,
        id: detail.id,
        facesVisible: flags.facesVisible,
        nametagsVisible: flags.nametagsVisible,
        blurRequired: flags.blurRequired,
        usable: flags.usable,
      });
      showMessage("Safety flags updated.");
      await refreshDetailState();
      await refreshReview();
    } catch (error) {
      showMessage(String(error), true);
      el.safetyApply.disabled = false;
    }
  });

  el.metadataSave.addEventListener("click", async () => {
    const detail = state.detail;
    if (!detail) return;
    const fields = metadataPatch();
    if (!fields || !Object.keys(fields).length) {
      showMessage("No metadata changes to save.");
      return;
    }
    el.metadataSave.disabled = true;
    try {
      await invoke("set_annotation", { assetType: detail.kind, id: detail.id, fields });
      showMessage("Metadata saved.");
      await refreshDetailState();
    } catch (error) {
      showMessage(String(error), true);
      el.metadataSave.disabled = false;
    }
  });

  el.detailStack.addEventListener("change", () => {
    el.detailAddStack.disabled = !el.detailStack.value;
  });

  el.detailAddStack.addEventListener("click", async () => {
    const detail = state.detail;
    const stackId = el.detailStack.value;
    if (!detail || !stackId) return;
    el.detailAddStack.disabled = true;
    try {
      await invoke("stack_add_item", {
        stackId,
        assetType: assetType(detail.kind),
        mediaId: detail.id,
        role: el.detailStackRole.value,
      });
      showMessage("Added to the version stack.");
      await refreshDetailState();
      await refreshReview();
    } catch (error) {
      showMessage(String(error), true);
      el.detailAddStack.disabled = false;
    }
  });

  el.stackCreate.addEventListener("click", async () => {
    const name = el.stackNewName.value.trim();
    if (!name) return;
    el.stackCreate.disabled = true;
    try {
      await invoke("stack_create", { name });
      el.stackNewName.value = "";
      showMessage(`Created version stack “${name}”.`);
      await refreshDetailState();
      await refreshReview();
    } catch (error) {
      showMessage(String(error), true);
    } finally {
      el.stackCreate.disabled = false;
    }
  });

  document.addEventListener("crush:detail", (event) => {
    state.detail = event.detail;
    refreshDetailState();
  });
  document.addEventListener("crush:review-shown", () => refreshReview());
})();
