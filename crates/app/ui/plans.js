// Projects are editable documents. Only the explicit preference-example button trains anything.
// All bridge results retain their shipped casing: candidates snake_case, plan DTOs camelCase.
(() => {
  const bridge = window.__TAURI__;
  if (!bridge?.core?.invoke) return;
  const invoke = bridge.core.invoke;
  const $ = (id) => document.getElementById(id);
  const state = {
    plans: [], plan: null, items: [], revisions: [], candidates: null,
    busy: false, dirty: new Set(), loaded: false, previewKey: null, previewLoop: false,
    photoExportKey: null, photoExportKind: null, photoExportBusy: false, photoExportResult: null,
    reelExportKey: null, reelExportBusy: false, reelExportResult: null,
    sequence: null, sequenceSuggestions: [],
  };
  const kind = (value) => value === "photo" ? "photo" : value === "span" ? "span" : "video";
  const itemKey = (item) => `${kind(item.mediaKind)}:${item.mediaId}`;
  const isClip = (item) => item.mediaKind === "shot" || item.mediaKind === "span";
  const candidateKey = (item) => `${item.asset_type}:${item.asset_id}`;
  const number = (value) => Number.isFinite(value) ? value.toFixed(3) : "—";
  const shortTime = (seconds) => {
    const total = Math.max(0, Number(seconds) || 0);
    return `${Math.floor(total / 60)}:${String(Math.floor(total % 60)).padStart(2, "0")}`;
  };
  const filename = (path) => String(path).split(/[\\/]/).at(-1);
  const parse = (json) => { try { return JSON.parse(json); } catch { return {}; } };
  const node = (tag, text, className) => {
    const element = document.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (className) element.className = className;
    return element;
  };
  let messageTimer = null;
  function message(text, error = false) {
    const element = $("plans-message");
    clearTimeout(messageTimer);
    element.textContent = text;
    element.hidden = false;
    element.classList.toggle("error", error);
    // Message parity (Task 039 B9): every other view auto-hides confirmations after
    // 5 s; errors stay on screen until the next message replaces them.
    if (!error) messageTimer = setTimeout(() => { element.hidden = true; }, 5000);
  }
  function dirty(key, value = true) {
    if (value) state.dirty.add(key); else state.dirty.delete(key);
    $("plan-dirty").hidden = state.dirty.size === 0;
    renderReelExport();
  }
  const preview = {
    root: $("project-preview"), video: $("project-preview-video"), photo: $("project-preview-photo"),
    empty: $("project-preview-empty"), prev: $("project-preview-prev"), play: $("project-preview-play"),
    next: $("project-preview-next"), scrubber: $("project-preview-scrubber"),
    time: $("project-preview-time"), loop: $("project-preview-loop"), label: $("project-preview-label"),
  };
  const photoExport = {
    root: $("project-photo-export"), preset: $("project-photo-preset"),
    step: $("project-photo-export-step"), title: $("project-photo-export-title"),
    copy: $("project-photo-export-copy"), clipOptions: $("project-clip-export-options"),
    audio: $("project-clip-audio"), outputLabel: $("project-photo-output-label"),
    destination: $("project-photo-destination"), choose: $("project-photo-choose"),
    render: $("project-photo-render"), progress: $("project-photo-progress"),
    status: $("project-photo-status"), result: $("project-photo-result"),
    outputPath: $("project-photo-output-path"), manifestPath: $("project-photo-manifest-path"),
    showOutput: $("project-photo-show-output"), showManifest: $("project-photo-show-manifest"),
    verification: $("project-photo-verification"),
  };
  const reelExport = {
    root: $("project-reel-export"), preset: $("project-reel-preset"),
    audio: $("project-reel-audio"), destination: $("project-reel-destination"),
    choose: $("project-reel-choose"), render: $("project-reel-render"),
    cancel: $("project-reel-cancel"),
    progress: $("project-reel-progress"), status: $("project-reel-status"),
    result: $("project-reel-result"), outputPath: $("project-reel-output-path"),
    manifestPath: $("project-reel-manifest-path"), showOutput: $("project-reel-show-output"),
    showManifest: $("project-reel-show-manifest"), verification: $("project-reel-verification"),
  };
  const photoPresets = {
    "jpeg-srgb-v1": { extension: "jpg", filter: { name: "JPEG image", extensions: ["jpg", "jpeg"] } },
    "png-srgb-v1": { extension: "png", filter: { name: "PNG image", extensions: ["png"] } },
    "tiff-srgb-v1": { extension: "tif", filter: { name: "TIFF image", extensions: ["tif", "tiff"] } },
    "mp4-h264-sdr-v1": { extension: "mp4", filter: { name: "MP4 video", extensions: ["mp4"] } },
    "mov-h264-sdr-v1": { extension: "mov", filter: { name: "MOV video", extensions: ["mov"] } },
  };
  async function run(action) {
    if (state.busy) return;
    state.busy = true;
    $("plans-controls").disabled = true;
    try { await action(); } catch (error) { message(String(error), true); }
    finally { state.busy = false; $("plans-controls").disabled = false; }
  }
  function confirmAction(copy) {
    const dialog = $("plan-confirm");
    $("plan-confirm-copy").textContent = copy;
    dialog.returnValue = "cancel";
    return new Promise((resolve) => {
      dialog.addEventListener("close", () => resolve(dialog.returnValue === "confirm"), { once: true });
      dialog.showModal();
    });
  }
  async function discardDrafts() {
    return state.dirty.size === 0 || confirmAction("This action replaces unsaved project edits. Discard those edits?");
  }
  function button(text, action, secondary = true) {
    const control = node("button", text, `button ${secondary ? "secondary" : "primary"} small`);
    control.type = "button";
    control.addEventListener("click", () => run(action));
    return control;
  }
  function pill(origin, version) {
    const label = origin === "personal" ? `Preference-assisted · profile v${version} · experimental`
      : origin === "historical" ? "Historical · your earlier Reel Studio choice"
      : origin === "imported" ? "Imported · catalogue evidence"
      : "General";
    return node("span", label, `plans-pill ${origin}`);
  }
  function input(label, name, value, options = {}) {
    const wrapper = node("label", label);
    const control = document.createElement(options.multiline ? "textarea" : "input");
    control.name = name;
    if (!options.multiline) control.type = options.type || "text";
    control.value = value ?? "";
    for (const key of ["min", "max", "step", "required"]) if (key in options) control[key] = options[key];
    wrapper.append(control);
    return wrapper;
  }
  function renderList() {
    $("plans-list").replaceChildren(...state.plans.map((plan) => {
      const control = button(`${plan.name} · ${plan.itemCount}`, async () => {
        if (plan.id !== state.plan?.id && await discardDrafts()) await openPlan(plan.id);
      });
      control.classList.add("plans-list-button");
      control.dataset.planId = plan.id;
      control.setAttribute("aria-current", String(plan.id === state.plan?.id));
      return control;
    }));
    $("plans-empty").hidden = state.plans.length > 0;
  }
  async function refreshList() { state.plans = await invoke("plan_list"); renderList(); }
  async function openPlan(id, clearCandidates = true) {
    // Commit only a complete successful read; failed reopen cannot mix two plans' data.
    const [plan, items, revisions] = await Promise.all([
      invoke("plan_get", { id }), invoke("plan_items", { id }), invoke("plan_revisions", { id }),
    ]);
    // Sequence notes are advisory reads; a bridge without them must not break the editor.
    const [sequence, sequenceSuggestions] = await Promise.all([
      invoke("plan_sequence_signals", { id }).catch(() => null),
      invoke("plan_sequence_suggestions", { id }).catch(() => []),
    ]);
    Object.assign(state, { plan, items, revisions, sequence, sequenceSuggestions });
    if (!state.items.some((item) => itemKey(item) === state.previewKey)) {
      state.previewKey = state.items.length ? itemKey(state.items[0]) : null;
    }
    state.dirty.clear();
    if (clearCandidates) state.candidates = null;
    renderEditor();
    await refreshList();
  }
  function renderEditor() {
    $("plan-editor").hidden = !state.plan;
    $("plan-no-selection").hidden = Boolean(state.plan);
    if (!state.plan) return;
    $("plan-name").value = state.plan.name;
    $("plan-description").value = state.plan.description;
    $("plan-context").value = state.plan.contextKey === "default" ? "General" : state.plan.contextKey;
    $("plan-brief").value = state.plan.brief;
    $("plan-dirty").hidden = true;
    $("plan-items").replaceChildren(...state.items.map(renderItem));
    $("plan-items-empty").hidden = state.items.length > 0;
    $("plan-item-count").textContent = `(${state.items.length})`;
    $("plan-revisions").replaceChildren(...state.revisions
      // Exports snapshot the frozen sequence for the render job's audit trail; presenting
      // every export as a Step 3 version would blur the two. Keep those out of the list.
      .filter((revision) => !String(revision.label || "").startsWith("Export · "))
      .map((revision) => {
        const row = node("div", undefined, "plans-actions");
        row.append(node("span", `v${revision.revision} · ${revision.label || "Untitled version"}`));
        row.append(button("Restore…", async () => {
          if (!await confirmAction(`Restore version ${revision.revision}? This replaces the current working project, including unsaved edits. Saved versions remain unchanged.`)) return;
          await invoke("plan_restore_revision", { id: state.plan.id, revision: revision.revision });
          await openPlan(state.plan.id);
          message(`Restored version ${revision.revision}.`);
        }));
        return row;
      }));
    renderCandidates();
    renderSequence();
    renderPreview();
    renderReelExport();
  }
  // Sequence notes (Task 033): describe what was measured, in editor language. Applying a
  // suggestion writes normal plan state through reorder, with a saved version as the undo.
  function renderSequence() {
    const root = $("plan-sequence-notes");
    const report = state.sequence;
    const suggestions = state.sequenceSuggestions || [];
    root.hidden = !report && suggestions.length === 0;
    if (!report && suggestions.length === 0) return;
    const rows = [];
    // An empty plan has nothing to judge; "0 items from 0 distinct sources" is noise.
    if (report?.summary?.item_count === 0) {
      root.hidden = true;
      root.replaceChildren();
      return;
    }
    if (report?.summary) {
      const lines = [report.summary.coverage_note];
      if (report.summary.pacing_note) lines.push(report.summary.pacing_note);
      if (report.summary.near_duplicate_adjacencies > 0) {
        lines.push(`${report.summary.near_duplicate_adjacencies} adjacent pair${report.summary.near_duplicate_adjacencies === 1 ? "" : "s"} of shots look near-identical.`);
      }
      rows.push(node("p", lines.join(" "), "plans-muted"));
    }
    for (const transition of report?.transitions || []) {
      if (!transition.note) continue;
      rows.push(node("p", `Between items ${transition.position + 1} and ${transition.position + 2}: ${transition.note}`, "plans-muted"));
    }
    for (const suggestion of suggestions) {
      const row = node("div", undefined, "plans-actions");
      row.append(node("span", suggestion.note));
      row.append(button("Apply reorder…", async () => {
        if (!await confirmAction("Move this item to the end of the sequence? A version is saved first so you can undo.")) return;
        await invoke("plan_save_revision", { id: state.plan.id, label: "Before sequence suggestion" });
        await invoke("plan_reorder_items", {
          id: state.plan.id,
          items: suggestion.suggested_order.map((entry) => ({ assetType: entry.media_kind, mediaId: entry.media_id })),
        });
        await openPlan(state.plan.id, false);
        message("Reordered. The previous order is saved under Versions.");
      }));
      rows.push(row);
    }
    root.replaceChildren(...rows);
  }
  function evidence(result) {
    const breakdown = result.score_breakdown;
    if (!breakdown) return `General strong-shot quality ${number(result.aesthetic_score)}. No preference term used in this ordering.`;
    const terms = [["Brief", "semantic"], ["Transcript", "transcript_boost"], ["Editorial", "editorial"], ["General quality", "general_aesthetic"], ["Preference", "personal_affinity"], ["Purpose", "context_fit"], ["Penalties", "penalties"]];
    return terms.map(([name, key]) => `${name} ${number(breakdown[key])}`).join(" · ");
  }
  function renderCandidates() {
    const response = state.candidates;
    const capNote = response?.duplicate_cap
      ? ` · similar-shot cap ${response.duplicate_cap} applied (${response.skipped_duplicates} skipped)`
      : "";
    $("plan-candidate-status").textContent = response
      ? `${response.general.length} general · ${response.personalized.length} brief candidates${capNote}. Scores use different scales; compare order within each column.`
      : "Find strong shots. Add a brief to also rank them for this project's story.";
    $("plan-personal-status").textContent = response?.profile
      ? `Experimental preference profile v${response.profile.version} · ${response.profile.context_key}. Human proof review pending.`
      : response?.brief ? "Matched to the brief with the general model. Confirmed preference examples are not available for this purpose yet." : "Add a brief to match the story you want. Confirmed examples can refine this ordering.";
    for (const [id, list, lane] of [["plan-general", response?.general || [], "general"], ["plan-personal", response?.personalized || [], "personalized"]]) {
      $(id).replaceChildren(...list.map((result, index) => {
        const card = node("article", undefined, "plans-candidate");
        card.dataset.assetKey = candidateKey(result);
        if (result.thumb_path) {
          const image = document.createElement("img");
          image.src = bridge.core.convertFileSrc(result.thumb_path);
          image.alt = filename(result.path); image.loading = "lazy";
          card.append(image);
        }
        card.append(node("strong", `${index + 1}. ${filename(result.path)}`));
        const profile = lane === "personalized" ? response.profile : null;
        card.append(pill(profile ? "personal" : "general", profile?.version));
        card.append(node("p", `Quality ${number(result.aesthetic_score)} · rank score ${number(result.score)}`, "plans-muted"));
        card.append(node("p", evidence(result), "plans-breakdown"));
        if (result.start_s != null) card.append(node("p", `Source ${number(result.start_s)}–${number(result.end_s)} s`, "plans-muted"));
        if (result.transcript_snippet) card.append(node("p", result.transcript_snippet, "plans-muted"));
        const exists = state.items.some((item) => itemKey(item) === candidateKey(result));
        const add = button(exists ? "In project" : "Add to project", async () => {
          if (!await discardDrafts()) return;
          const signals = { schema_version: 1, lane, ordinal: index + 1, brief: response.brief, context: response.context_key, profile, candidate: result };
          await invoke("plan_add_item", { id: state.plan.id, item: {
            assetType: result.asset_type, mediaId: result.asset_id,
            startS: result.start_s, endS: result.end_s, reason: evidence(result),
            signalsJson: JSON.stringify(signals), origin: profile ? "personal" : "general",
            rank: result.score, profileVersion: profile?.version ?? null,
          } });
          await openPlan(state.plan.id, false);
          message("Added to the project. No feedback was recorded.");
        }, false);
        add.disabled = exists; card.append(add);
        return card;
      }));
    }
  }
  function previewItem() {
    return state.items.find((item) => itemKey(item) === state.previewKey) || null;
  }
  function previewRange(item) {
    if (!item || !isClip(item)) return null;
    const frozen = parse(item.signalsJson);
    const candidate = frozen.candidate || {};
    const form = $("plan-items").querySelector(`[data-asset-key="${CSS.escape(itemKey(item))}"]`);
    const draftStart = Number(form?.elements?.startS?.value ?? item.startS);
    const draftEnd = Number(form?.elements?.endS?.value ?? item.endS);
    const sourceStart = Number.isFinite(candidate.start_s) ? candidate.start_s : item.startS;
    const sourceEnd = Number.isFinite(candidate.end_s) ? candidate.end_s : item.endS;
    const start = Math.max(sourceStart, Math.min(sourceEnd, draftStart));
    const end = Math.max(sourceStart, Math.min(sourceEnd, draftEnd));
    return Number.isFinite(start) && Number.isFinite(end) && end > start ? { start, end } : null;
  }
  function clearPhotoExport(item = null) {
    const key = item ? itemKey(item) : null;
    if (state.photoExportKey === key) return;
    state.photoExportKey = key;
    state.photoExportResult = null;
    photoExport.destination.value = "";
    photoExport.result.hidden = true;
    photoExport.status.textContent = "Choose where to save the finished copy.";
    photoExport.status.classList.remove("error");
    photoExport.render.disabled = true;
  }
  function renderPhotoExport(item) {
    const exportable = item && ["photo", "shot", "span"].includes(item.mediaKind);
    photoExport.root.hidden = !exportable;
    if (!exportable) { clearPhotoExport(null); return; }
    const exportKind = item.mediaKind === "photo" ? "photo" : "clip";
    if (state.photoExportKind !== exportKind) {
      state.photoExportKind = exportKind;
      const options = exportKind === "photo"
        ? [["jpeg-srgb-v1", "JPEG — smaller, easy to share"], ["png-srgb-v1", "PNG — lossless"], ["tiff-srgb-v1", "TIFF — high-quality archive"]]
        : [["mp4-h264-sdr-v1", "MP4 — compatible H.264"], ["mov-h264-sdr-v1", "MOV — editing-friendly H.264"]];
      photoExport.preset.replaceChildren(...options.map(([value, label]) => {
        const option = node("option", label); option.value = value; return option;
      }));
      photoExport.audio.value = "source";
    }
    const isPhoto = exportKind === "photo";
    photoExport.step.textContent = `Export selected ${isPhoto ? "photo" : "clip"}`;
    photoExport.title.textContent = `Create a finished ${isPhoto ? "copy" : "clip"}`;
    photoExport.copy.textContent = `Your original ${isPhoto ? "photo" : "video"} stays untouched. Crush verifies the new file before showing it here.`;
    photoExport.clipOptions.hidden = isPhoto;
    photoExport.render.textContent = `Render ${isPhoto ? "photo" : "clip"}`;
    photoExport.outputLabel.textContent = `Finished ${isPhoto ? "photo" : "clip"}`;
    clearPhotoExport(item);
    if (item.mediaKind === "span") {
      // Imported spans render through the reel executor, but single-clip export for them is not
      // wired through the app yet. Keep the panel visible and honest instead of letting the
      // render fail with a false "not selected in this project" error.
      photoExport.preset.disabled = true;
      photoExport.choose.disabled = true;
      photoExport.render.disabled = true;
      photoExport.status.textContent = "Clip export for imported clips lands with the next update — the sequence can still be rendered as a reel once span export is enabled.";
    }
  }
  function showPhotoRenderResult(result) {
    state.photoExportResult = result;
    photoExport.outputPath.textContent = result.outputPath;
    photoExport.outputPath.title = result.outputPath;
    photoExport.manifestPath.textContent = result.manifestPath;
    photoExport.manifestPath.title = result.manifestPath;
    const facts = [
      ["Dimensions", result.width && result.height ? `${result.width} × ${result.height}` : "Verified"],
      ["File size", `${(result.sizeBytes / 1048576).toFixed(2)} MB`],
      ["Media type", result.mediaType],
      ["Output checksum", result.outputSha256],
      ["Manifest checksum", result.manifestSha256],
    ];
    photoExport.verification.replaceChildren(...facts.flatMap(([term, detail]) => [node("dt", term), node("dd", detail)]));
    photoExport.result.hidden = false;
  }
  function setPhotoExportBusy(value) {
    state.photoExportBusy = value;
    const spanLocked = previewItem()?.mediaKind === "span";
    photoExport.preset.disabled = value || spanLocked;
    photoExport.choose.disabled = value || spanLocked;
    photoExport.render.disabled = value || spanLocked || !photoExport.destination.value;
    photoExport.progress.hidden = !value;
  }
  function setReelExportBusy(value) {
    state.reelExportBusy = value;
    reelExport.preset.disabled = value;
    reelExport.audio.disabled = value;
    reelExport.choose.disabled = value || !state.items.length || state.items.some((item) => item.mediaKind !== "shot");
    reelExport.render.disabled = value || !reelExport.destination.value || state.dirty.size > 0;
    reelExport.cancel.hidden = !value;
    reelExport.cancel.disabled = !value;
    reelExport.progress.hidden = !value;
  }
  function renderReelExport() {
    if (!reelExport.root) return;
    const key = state.plan?.id || null;
    reelExport.root.hidden = !key;
    if (state.reelExportKey !== key) {
      state.reelExportKey = key;
      state.reelExportResult = null;
      reelExport.destination.value = "";
      reelExport.result.hidden = true;
    }
    if (!key) return;
    const hasPhotos = state.items.some((item) => item.mediaKind === "photo");
    const hasSpans = state.items.some((item) => item.mediaKind === "span");
    const hasUnsupportedItems = hasPhotos || hasSpans;
    reelExport.choose.disabled = state.reelExportBusy || !state.items.length || hasUnsupportedItems;
    reelExport.render.disabled = state.reelExportBusy || !reelExport.destination.value || state.dirty.size > 0 || !state.items.length || hasUnsupportedItems;
    reelExport.status.classList.remove("error");
    if (!state.items.length) {
      reelExport.status.textContent = "Add at least one clip before exporting a reel.";
    } else if (hasPhotos) {
      reelExport.status.textContent = "This sequence includes photos. Export those individually above; whole-reel photo timing is not enabled yet.";
    } else if (hasSpans) {
      reelExport.status.textContent = "This sequence is built from imported clips. Whole-reel export for imported clips lands with the next update.";
    } else if (state.dirty.size) {
      reelExport.status.textContent = "Save every clip edit before rendering so the reel matches this sequence.";
    } else if (!reelExport.destination.value) {
      reelExport.status.textContent = `${state.items.length} clips ready. Choose where to save the finished reel.`;
    } else {
      reelExport.status.textContent = `${state.items.length} clips ready to render in this order.`;
    }
  }
  function showReelRenderResult(result) {
    state.reelExportResult = result;
    reelExport.outputPath.textContent = result.outputPath;
    reelExport.outputPath.title = result.outputPath;
    reelExport.manifestPath.textContent = result.manifestPath;
    reelExport.manifestPath.title = result.manifestPath;
    const facts = [
      ["Dimensions", result.width && result.height ? `${result.width} × ${result.height}` : "Verified"],
      ["Duration", result.durationS != null ? `${Number(result.durationS).toFixed(2)} seconds` : "Verified"],
      ["File size", `${(result.sizeBytes / 1048576).toFixed(2)} MB`],
      ["Media type", result.mediaType],
      ["Output checksum", result.outputSha256],
      ["Manifest checksum", result.manifestSha256],
    ];
    reelExport.verification.replaceChildren(...facts.flatMap(([term, detail]) => [node("dt", term), node("dd", detail)]));
    reelExport.result.hidden = false;
  }
  function setPreviewLoop(value) {
    state.previewLoop = value;
    preview.loop.setAttribute("aria-pressed", String(value));
    preview.loop.textContent = value ? "Loop on" : "Loop off";
  }
  function updatePreviewPosition() {
    const range = previewRange(previewItem());
    if (!range) return;
    const duration = range.end - range.start;
    const relative = Math.max(0, Math.min(duration, preview.video.currentTime - range.start));
    preview.scrubber.value = String(relative);
    preview.time.textContent = `${shortTime(relative)} / ${shortTime(duration)}`;
  }
  function updatePreviewPlayButton() {
    preview.play.textContent = preview.video.paused ? "Play clip" : "Pause";
    preview.play.setAttribute("aria-label", preview.video.paused ? "Play selected clip" : "Pause selected clip");
  }
  function refreshPreviewRange({ seekIfOutside = true } = {}) {
    const range = previewRange(previewItem());
    const valid = Boolean(range);
    preview.play.disabled = !valid;
    preview.scrubber.disabled = !valid;
    if (!valid) {
      preview.video.pause();
      preview.scrubber.max = "0";
      preview.scrubber.value = "0";
      preview.time.textContent = "Set a valid In and Out";
      return;
    }
    const duration = range.end - range.start;
    preview.scrubber.max = String(duration);
    if (seekIfOutside && (preview.video.currentTime < range.start || preview.video.currentTime >= range.end)) {
      preview.video.currentTime = range.start;
    }
    updatePreviewPosition();
  }
  function renderPreview() {
    const item = previewItem();
    preview.root.hidden = state.items.length === 0;
    if (!item) {
      renderPhotoExport(null);
      preview.video.pause();
      preview.video.removeAttribute("src");
      preview.video.removeAttribute("data-src");
      preview.photo.removeAttribute("src");
      preview.video.hidden = true;
      preview.photo.hidden = true;
      preview.empty.hidden = false;
      preview.label.textContent = "";
      return;
    }
    const index = state.items.indexOf(item);
    const frozen = parse(item.signalsJson);
    const candidate = frozen.candidate || {};
    const path = candidate.path || "";
    const title = filename(path || item.mediaId);
    const isPhoto = item.mediaKind === "photo";
    renderPhotoExport(item);
    preview.empty.hidden = true;
    preview.video.hidden = isPhoto;
    preview.photo.hidden = !isPhoto;
    preview.play.hidden = isPhoto;
    preview.scrubber.hidden = isPhoto;
    preview.time.hidden = isPhoto;
    preview.loop.hidden = isPhoto;
    preview.prev.disabled = index <= 0;
    preview.next.disabled = index + 1 >= state.items.length;
    preview.label.textContent = `${index + 1} of ${state.items.length} · ${title}${isPhoto ? " · Photo select" : " · Source clip preview"}`;
    for (const form of $("plan-items").querySelectorAll(".plans-item")) {
      form.classList.toggle("previewing", form.dataset.assetKey === state.previewKey);
      const control = form.querySelector("[data-preview-control]");
      if (control) {
        const selected = form.dataset.assetKey === state.previewKey;
        control.textContent = selected ? "Previewing" : "Preview";
        control.setAttribute("aria-pressed", String(selected));
      }
    }
    if (isPhoto) {
      preview.video.pause();
      preview.video.removeAttribute("src");
      preview.video.removeAttribute("data-src");
      if (path) preview.photo.src = bridge.core.convertFileSrc(path);
      return;
    }
    preview.photo.removeAttribute("src");
    const src = path ? bridge.core.convertFileSrc(path) : "";
    if (src && preview.video.dataset.src !== src) {
      preview.video.pause();
      preview.video.dataset.src = src;
      preview.video.src = src;
      preview.video.load();
    }
    refreshPreviewRange();
    updatePreviewPlayButton();
  }
  function selectPreview(index) {
    const item = state.items[index];
    if (!item) return;
    state.previewKey = itemKey(item);
    renderPreview();
  }
  function renderItem(item, index) {
    const form = node("form", undefined, "plans-item");
    const key = itemKey(item);
    form.dataset.assetKey = key;
    form.classList.toggle("previewing", key === state.previewKey);
    const frozen = parse(item.signalsJson);
    const candidate = frozen.candidate || {};
    const title = filename(candidate.path || item.mediaId);
    const heading = node("div", undefined, "plans-item-heading");
    const previewControl = button(key === state.previewKey ? "Previewing" : "Preview", async () => {
      state.previewKey = key;
      renderPreview();
    });
    previewControl.dataset.previewControl = "true";
    previewControl.setAttribute("aria-pressed", String(key === state.previewKey));
    heading.append(node("strong", `${index + 1}. ${title}`), pill(item.origin, item.profileVersion), previewControl);
    form.append(heading);
    const fields = node("div", undefined, "plans-fields");
    if (isClip(item)) {
      const basis = item.mediaKind === "span" && candidate.boundary_basis === "catalogue_tc" && candidate.boundary_tolerance_s > 0
        ? ` Imported boundaries come from the catalogue timecodes and may be off by up to ${number(candidate.boundary_tolerance_s)} s.`
        : "";
      form.append(node("p", Number.isFinite(candidate.start_s) ? `Available source ${number(candidate.start_s)}–${number(candidate.end_s)} s. Preview and saved edits stay inside it.${basis}` : "Clip edits are validated against the source shot by the store.", "plans-muted"));
      for (const [label, name, value] of [["In (seconds)", "startS", item.startS], ["Out (seconds)", "endS", item.endS]]) {
        fields.append(input(label, name, value, { type: "number", min: candidate.start_s ?? 0, ...(candidate.end_s != null ? { max: candidate.end_s } : {}), step: "any", required: true }));
      }
    }
    form.append(fields, input("Edit note", "reason", item.reason, { multiline: true }));
    const options = node("details", undefined, "plans-item-options");
    options.append(node("summary", "Optional treatment"));
    const treatment = node("div", undefined, "plans-fields");
    treatment.append(input("Pacing (0–1)", "pacing", item.pacing, { type: "number", min: 0, max: 1, step: "any" }));
    treatment.append(input("Horizontal crop (0–1)", "cropX", item.cropX, { type: "number", min: 0, max: 1, step: "any" }));
    options.append(treatment, input("Color recipe JSON (advanced)", "gradeJson", item.gradeJson ?? "{}", { multiline: true }));
    const treatmentWarn = node("p", "", "plans-warning");
    treatmentWarn.hidden = true;
    const updateTreatmentWarn = () => {
      const data = new FormData(form);
      const pacing = data.get("pacing");
      const cropX = data.get("cropX");
      const grade = String(data.get("gradeJson") || "").trim();
      const notes = [];
      if (pacing !== null && pacing !== "") notes.push("pacing");
      if (cropX !== null && cropX !== "") notes.push("horizontal crop");
      if (grade && grade !== "{}" && grade !== "{\"mode\":\"none\"}") notes.push("color treatment");
      if (!notes.length) {
        treatmentWarn.hidden = true;
        treatmentWarn.textContent = "";
        return;
      }
      treatmentWarn.hidden = false;
      treatmentWarn.textContent =
        `Saved ${notes.join(" and ")} is stored intent only. The current renderer cannot reproduce it exactly, so single-clip and reel export will ask you to remove it before rendering.`;
    };
    options.append(treatmentWarn);
    updateTreatmentWarn();
    form.append(options);
    const actions = node("div", undefined, "plans-actions");
    const save = node("button", "Save item", "button primary small"); save.type = "submit";
    actions.append(save);
    for (const [label, offset] of [["Move up", -1], ["Move down", 1]]) {
      const move = button(label, async () => {
        if (!await discardDrafts()) return;
        const ordered = [...state.items];
        [ordered[index], ordered[index + offset]] = [ordered[index + offset], ordered[index]];
        await invoke("plan_reorder_items", { id: state.plan.id, items: ordered.map((value) => ({ assetType: kind(value.mediaKind), mediaId: value.mediaId })) });
        await openPlan(state.plan.id, false);
      });
      move.disabled = index + offset < 0 || index + offset >= state.items.length;
      actions.append(move);
    }
    actions.append(button("Remove", async () => {
      if (!await discardDrafts()) return;
      await invoke("plan_remove_item", { id: state.plan.id, assetType: kind(item.mediaKind), mediaId: item.mediaId });
      await openPlan(state.plan.id, false); message("Removed from the project. No rejection or other feedback was inferred.");
    }));
    const example = button("Use as preference example", async () => {
      await invoke("record_feedback", { assetType: kind(item.mediaKind), id: item.mediaId, signal: "pick", value: 1, context: state.plan.brief, contextKey: state.plan.contextKey });
      message(`Preference example recorded for “${state.plan.contextKey}”.`);
    });
    if (item.mediaKind === "span") {
      example.disabled = true;
      example.title = "Imported Reel Studio spans are historical evidence; confirm them as examples from Preferences once the catalogue import is reviewed.";
    }
    actions.append(example);
    form.append(actions);
    const details = node("details");
    details.append(node("summary", "Why Crush suggested this"), node("pre", JSON.stringify(frozen, null, 2)));
    form.append(details);
    form.addEventListener("input", (event) => {
      dirty(key);
      updateTreatmentWarn();
      if (key === state.previewKey && ["startS", "endS"].includes(event.target.name)) refreshPreviewRange();
    });
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      // Read before disabling the fieldset (disabled fields are omitted by FormData).
      const data = new FormData(form);
      const patch = { reason: data.get("reason"), gradeJson: data.get("gradeJson") };
      for (const name of ["startS", "endS", "pacing", "cropX"]) {
        const value = data.get(name);
        if (value != null && value !== "") patch[name] = Number(value);
      }
      run(async () => {
        const grade = JSON.parse(patch.gradeJson);
        if (!grade || Array.isArray(grade) || typeof grade !== "object") throw new Error("Grade must be a JSON object.");
        if (isClip(item) && patch.endS <= patch.startS) throw new Error("Out must be after In.");
        for (const name of ["pacing", "cropX"]) {
          if (item[name] != null && !(name in patch)) throw new Error("Blank values do not clear saved treatment. Enter a number to change it.");
        }
        const saved = await invoke("plan_update_item", { id: state.plan.id, assetType: kind(item.mediaKind), mediaId: item.mediaId, patch });
        state.items[index] = saved;
        dirty(key, false);
        // Replace only this form: another item's unsaved draft must survive.
        form.replaceWith(renderItem(saved, index));
        renderPreview();
        message("Clip settings saved. Source media and suggestion evidence are unchanged.");
      });
    });
    return form;
  }
  $("plan-header-form").addEventListener("input", () => dirty("header"));
  $("plan-header-form").addEventListener("submit", (event) => {
    event.preventDefault();
    run(async () => {
      const fields = { name: $("plan-name").value.trim(), description: $("plan-description").value, brief: $("plan-brief").value };
      await invoke("plan_update", { id: state.plan.id, ...fields });
      Object.assign(state.plan, fields); dirty("header", false); await refreshList(); message("Project details saved.");
    });
  });
  $("plan-create-form").addEventListener("submit", (event) => {
    event.preventDefault();
    run(async () => {
      if (!await discardDrafts()) return;
      const plan = await invoke("plan_create", { name: $("plan-new-name").value.trim(), contextKey: $("plan-new-context").value.trim() || "default" });
      await openPlan(plan.id); $("plan-new-name").value = ""; message("Project created.");
    });
  });
  $("plan-generate").addEventListener("click", () => run(async () => {
    $("plan-candidate-status").textContent = "Finding candidates…";
    const capValue = Number.parseInt($("plan-duplicate-cap").value, 10);
    try {
      state.candidates = await invoke("selects_candidates", {
        brief: $("plan-brief").value.trim() || null,
        context: state.plan.contextKey,
        top: 12,
        duplicateCap: Number.isFinite(capValue) && capValue > 0 ? capValue : null,
      });
      renderCandidates();
    } catch (error) {
      state.candidates = null; renderCandidates();
      $("plan-candidate-status").textContent = "Suggestion lookup failed. Your project is unchanged; retry when ready.";
      throw error;
    }
  }));
  $("plan-duplicate").addEventListener("click", () => run(async () => {
    if (!await discardDrafts()) return;
    const copy = await invoke("plan_duplicate", { id: state.plan.id, name: `${state.plan.name} copy` });
    await openPlan(copy.id); message("Duplicated the saved project, with suggestion evidence preserved.");
  }));
  $("plan-delete").addEventListener("click", () => run(async () => {
    if (!await confirmAction(`Delete “${state.plan.name}”, its items and saved versions? Originals and feedback will not be deleted.`)) return;
    await invoke("plan_delete", { id: state.plan.id });
    state.plan = null; state.items = []; state.candidates = null; state.previewKey = null; state.dirty.clear();
    renderEditor(); renderPreview(); await refreshList(); message("Project deleted. Original media is unchanged.");
  }));
  $("plan-revision-form").addEventListener("submit", (event) => {
    event.preventDefault();
    run(async () => {
      if (state.dirty.size) throw new Error("Save your clip and project-detail edits before saving a version.");
      const revision = await invoke("plan_save_revision", { id: state.plan.id, label: $("plan-revision-label").value });
      await openPlan(state.plan.id, false); $("plan-revision-label").value = ""; message(`Saved version ${revision.revision}.`);
    });
  });
  preview.prev.addEventListener("click", () => selectPreview(state.items.indexOf(previewItem()) - 1));
  preview.next.addEventListener("click", () => selectPreview(state.items.indexOf(previewItem()) + 1));
  preview.play.addEventListener("click", () => {
    const range = previewRange(previewItem());
    if (!range) return;
    if (preview.video.paused) {
      if (preview.video.currentTime < range.start || preview.video.currentTime >= range.end - 0.02) preview.video.currentTime = range.start;
      preview.video.play().catch(() => {});
    } else preview.video.pause();
  });
  preview.scrubber.addEventListener("input", () => {
    const range = previewRange(previewItem());
    if (!range) return;
    const relative = Math.max(0, Math.min(range.end - range.start, Number(preview.scrubber.value) || 0));
    preview.video.currentTime = range.start + relative;
    updatePreviewPosition();
  });
  preview.loop.addEventListener("click", () => setPreviewLoop(!state.previewLoop));
  preview.video.addEventListener("loadedmetadata", () => refreshPreviewRange());
  preview.video.addEventListener("play", updatePreviewPlayButton);
  preview.video.addEventListener("pause", updatePreviewPlayButton);
  preview.video.addEventListener("timeupdate", () => {
    const range = previewRange(previewItem());
    if (!range) { preview.video.pause(); return; }
    if (preview.video.currentTime < range.start) preview.video.currentTime = range.start;
    if (preview.video.currentTime >= range.end - 0.02) {
      if (state.previewLoop) preview.video.currentTime = range.start;
      else {
        preview.video.pause();
        preview.video.currentTime = Math.max(range.start, range.end - 0.04);
      }
    }
    updatePreviewPosition();
  });
  preview.video.addEventListener("error", () => message(`Could not preview ${filename(parse(previewItem()?.signalsJson).candidate?.path || "this clip")}. Is its drive mounted?`, true));
  preview.photo.addEventListener("error", () => message(`Could not preview ${filename(parse(previewItem()?.signalsJson).candidate?.path || "this photo")}. Is its drive mounted?`, true));
  photoExport.preset.addEventListener("change", () => {
    photoExport.destination.value = "";
    photoExport.result.hidden = true;
    state.photoExportResult = null;
    photoExport.status.textContent = "Choose where to save the finished copy.";
    photoExport.status.classList.remove("error");
    photoExport.render.disabled = true;
  });
  photoExport.choose.addEventListener("click", async () => {
    if (state.photoExportBusy) return;
    const item = previewItem();
    if (!item || !["photo", "shot", "span"].includes(item.mediaKind)) return;
    const isPhoto = item.mediaKind === "photo";
    const preset = photoPresets[photoExport.preset.value];
    const candidate = parse(item.signalsJson).candidate || {};
    const sourceName = filename(candidate.path || item.mediaId).replace(/\.[^.]+$/, "") || "photo";
    try {
      const destination = await bridge.dialog.save({
        title: `Export selected ${isPhoto ? "photo" : "clip"}`,
        defaultPath: `${sourceName}_export.${preset.extension}`,
        filters: [preset.filter],
      });
      if (!destination) return;
      photoExport.destination.value = destination;
      photoExport.render.disabled = false;
      photoExport.result.hidden = true;
      state.photoExportResult = null;
      photoExport.status.textContent = "Ready to render a new verified copy.";
      photoExport.status.classList.remove("error");
    } catch (error) {
      photoExport.status.textContent = `Could not choose a destination: ${String(error)}`;
      photoExport.status.classList.add("error");
    }
  });
  photoExport.render.addEventListener("click", async () => {
    if (state.photoExportBusy || state.busy) return;
    const item = previewItem();
    if (!state.plan || !item || !["photo", "shot", "span"].includes(item.mediaKind) || !photoExport.destination.value) return;
    const expectedKey = itemKey(item);
    const isPhoto = item.mediaKind === "photo";
    if (state.dirty.has(expectedKey)) {
      photoExport.status.textContent = "Save this item's edits before rendering so the finished file matches the visible In, Out, and treatment.";
      photoExport.status.classList.add("error");
      return;
    }
    photoExport.result.hidden = true;
    photoExport.status.textContent = "Rendering and verifying the finished copy…";
    photoExport.status.classList.remove("error");
    setPhotoExportBusy(true);
    try {
      const shared = { projectId: state.plan.id, preset: photoExport.preset.value, destination: photoExport.destination.value };
      const result = isPhoto
        ? await invoke("render_project_photo", { ...shared, photoId: item.mediaId })
        : await invoke("render_project_clip", { ...shared, shotId: item.mediaId, audio: photoExport.audio.value });
      if (state.photoExportKey !== expectedKey) return;
      showPhotoRenderResult(result);
      photoExport.status.textContent = `Rendered and verified. Your original ${isPhoto ? "photo" : "video"} was not changed.`;
    } catch (error) {
      if (state.photoExportKey !== expectedKey) return;
      photoExport.status.textContent = `Render failed: ${String(error)}`;
      photoExport.status.classList.add("error");
    } finally {
      setPhotoExportBusy(false);
    }
  });
  for (const [control, field] of [[photoExport.showOutput, "outputPath"], [photoExport.showManifest, "manifestPath"]]) {
    control.addEventListener("click", async () => {
      const path = state.photoExportResult?.[field];
      if (!path) return;
      try { await invoke("open_in_finder", { path }); }
      catch (error) {
        photoExport.status.textContent = `Could not show that file: ${String(error)}`;
        photoExport.status.classList.add("error");
      }
    });
  }
  reelExport.preset.addEventListener("change", () => {
    reelExport.destination.value = "";
    reelExport.result.hidden = true;
    state.reelExportResult = null;
    renderReelExport();
  });
  reelExport.choose.addEventListener("click", async () => {
    if (state.reelExportBusy || !state.plan || !state.items.length || state.items.some((item) => item.mediaKind !== "shot")) return;
    const preset = photoPresets[reelExport.preset.value];
    const projectName = String(state.plan.name || "project").trim().replace(/[^a-z0-9_-]+/gi, "-").replace(/^-|-$/g, "") || "project";
    try {
      const destination = await bridge.dialog.save({
        title: "Export finished reel",
        defaultPath: `${projectName}.${preset.extension}`,
        filters: [preset.filter],
      });
      if (!destination) return;
      reelExport.destination.value = destination;
      reelExport.result.hidden = true;
      state.reelExportResult = null;
      renderReelExport();
    } catch (error) {
      reelExport.status.textContent = `Could not choose a destination: ${String(error)}`;
      reelExport.status.classList.add("error");
    }
  });
  reelExport.render.addEventListener("click", async () => {
    if (state.reelExportBusy || state.busy || !state.plan || !reelExport.destination.value) return;
    if (state.dirty.size) {
      renderReelExport();
      return;
    }
    const projectId = state.plan.id;
    reelExport.result.hidden = true;
    reelExport.status.textContent = "Rendering the clip order and verifying the finished reel…";
    reelExport.status.classList.remove("error");
    reelExport.render.textContent = "Render reel";
    setReelExportBusy(true);
    try {
      const result = await invoke("render_project_reel", {
        projectId,
        preset: reelExport.preset.value,
        audio: reelExport.audio.value,
        destination: reelExport.destination.value,
      });
      if (state.plan?.id !== projectId) return;
      showReelRenderResult(result);
      reelExport.destination.value = "";
      reelExport.status.textContent = "Reel rendered and verified. Source media was not changed.";
    } catch (error) {
      if (state.plan?.id !== projectId) return;
      reelExport.status.textContent = `Reel render failed: ${String(error)}`;
      reelExport.status.classList.add("error");
      reelExport.render.textContent = "Retry render";
    } finally {
      setReelExportBusy(false);
    }
  });
  reelExport.cancel.addEventListener("click", async () => {
    if (!state.reelExportBusy || !state.plan) return;
    reelExport.cancel.disabled = true;
    reelExport.status.textContent = "Cancelling after the current media operation stops safely…";
    try {
      const requested = await invoke("cancel_project_render", { projectId: state.plan.id });
      if (!requested) reelExport.status.textContent = "The render already finished; checking its result…";
    } catch (error) {
      reelExport.status.textContent = `Could not request cancellation: ${String(error)}`;
      reelExport.status.classList.add("error");
      reelExport.cancel.disabled = false;
    }
  });
  for (const [control, field] of [[reelExport.showOutput, "outputPath"], [reelExport.showManifest, "manifestPath"]]) {
    control.addEventListener("click", async () => {
      const path = state.reelExportResult?.[field];
      if (!path) return;
      try { await invoke("open_in_finder", { path }); }
      catch (error) {
        reelExport.status.textContent = `Could not show that file: ${String(error)}`;
        reelExport.status.classList.add("error");
      }
    });
  }
  document.addEventListener("crush:plans-shown", () => run(async () => {
    // Navigation must not wipe local drafts. A full refresh is explicit through plan reopen.
    await refreshList();
    if (!state.loaded) { state.loaded = true; if (state.plans.length) await openPlan(state.plans[0].id); }
  }));
})();
