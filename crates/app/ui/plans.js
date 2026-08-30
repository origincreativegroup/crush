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
  };
  const kind = (value) => value === "photo" ? "photo" : "video";
  const itemKey = (item) => `${kind(item.mediaKind)}:${item.mediaId}`;
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
  function message(text, error = false) {
    $("plans-message").textContent = text;
    $("plans-message").hidden = false;
    $("plans-message").classList.toggle("error", error);
  }
  function dirty(key, value = true) {
    if (value) state.dirty.add(key); else state.dirty.delete(key);
    $("plan-dirty").hidden = state.dirty.size === 0;
  }
  const preview = {
    root: $("project-preview"), video: $("project-preview-video"), photo: $("project-preview-photo"),
    empty: $("project-preview-empty"), prev: $("project-preview-prev"), play: $("project-preview-play"),
    next: $("project-preview-next"), scrubber: $("project-preview-scrubber"),
    time: $("project-preview-time"), loop: $("project-preview-loop"), label: $("project-preview-label"),
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
    return node("span", origin === "personal" ? `Preference-assisted · profile v${version} · experimental` : "General", `plans-pill ${origin}`);
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
    Object.assign(state, { plan, items, revisions });
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
    $("plan-revisions").replaceChildren(...state.revisions.map((revision) => {
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
    renderPreview();
  }
  function evidence(result) {
    const breakdown = result.score_breakdown;
    if (!breakdown) return `General strong-shot quality ${number(result.aesthetic_score)}. No preference term used in this ordering.`;
    const terms = [["Brief", "semantic"], ["Transcript", "transcript_boost"], ["Editorial", "editorial"], ["General quality", "general_aesthetic"], ["Preference", "personal_affinity"], ["Purpose", "context_fit"], ["Penalties", "penalties"]];
    return terms.map(([name, key]) => `${name} ${number(breakdown[key])}`).join(" · ");
  }
  function renderCandidates() {
    const response = state.candidates;
    $("plan-candidate-status").textContent = response
      ? `${response.general.length} general · ${response.personalized.length} brief candidates. Scores use different scales; compare order within each column.`
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
    if (!item || item.mediaKind !== "shot") return null;
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
    if (item.mediaKind === "shot") {
      form.append(node("p", Number.isFinite(candidate.start_s) ? `Available source ${number(candidate.start_s)}–${number(candidate.end_s)} s. Preview and saved edits stay inside it.` : "Clip edits are validated against the source shot by the store.", "plans-muted"));
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
    actions.append(button("Use as preference example", async () => {
      await invoke("record_feedback", { assetType: kind(item.mediaKind), id: item.mediaId, signal: "pick", value: 1, context: state.plan.brief, contextKey: state.plan.contextKey });
      message(`Preference example recorded for “${state.plan.contextKey}”.`);
    }));
    form.append(actions);
    const details = node("details");
    details.append(node("summary", "Why Crush suggested this"), node("pre", JSON.stringify(frozen, null, 2)));
    form.append(details);
    form.addEventListener("input", (event) => {
      dirty(key);
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
        if (item.mediaKind === "shot" && patch.endS <= patch.startS) throw new Error("Out must be after In.");
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
    try {
      state.candidates = await invoke("selects_candidates", { brief: $("plan-brief").value.trim() || null, context: state.plan.contextKey, top: 12 });
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
  document.addEventListener("crush:plans-shown", () => run(async () => {
    // Navigation must not wipe local drafts. A full refresh is explicit through plan reopen.
    await refreshList();
    if (!state.loaded) { state.loaded = true; if (state.plans.length) await openPlan(state.plans[0].id); }
  }));
})();
