// Plans are editable documents. Only the explicit context-feedback button trains anything.
// All bridge results retain their shipped casing: candidates snake_case, plan DTOs camelCase.
(() => {
  const bridge = window.__TAURI__;
  if (!bridge?.core?.invoke) return;
  const invoke = bridge.core.invoke;
  const $ = (id) => document.getElementById(id);
  const state = { plans: [], plan: null, items: [], revisions: [], candidates: null, busy: false, dirty: new Set(), loaded: false };
  const kind = (value) => value === "photo" ? "photo" : "video";
  const itemKey = (item) => `${kind(item.mediaKind)}:${item.mediaId}`;
  const candidateKey = (item) => `${item.asset_type}:${item.asset_id}`;
  const number = (value) => Number.isFinite(value) ? value.toFixed(3) : "—";
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
    return state.dirty.size === 0 || confirmAction("This action replaces unsaved plan edits. Discard those edits?");
  }
  function button(text, action, secondary = true) {
    const control = node("button", text, `button ${secondary ? "secondary" : "primary"} small`);
    control.type = "button";
    control.addEventListener("click", () => run(action));
    return control;
  }
  function pill(origin, version) {
    return node("span", origin === "personal" ? `Personalized · profile v${version} · experimental` : "General", `plans-pill ${origin}`);
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
    $("plan-context").value = state.plan.contextKey;
    $("plan-brief").value = state.plan.brief;
    $("plan-dirty").hidden = true;
    $("plan-items").replaceChildren(...state.items.map(renderItem));
    $("plan-items-empty").hidden = state.items.length > 0;
    $("plan-item-count").textContent = `(${state.items.length})`;
    $("plan-revisions").replaceChildren(...state.revisions.map((revision) => {
      const row = node("div", undefined, "plans-actions");
      row.append(node("span", `v${revision.revision} · ${revision.label || "Untitled version"}`));
      row.append(button("Restore…", async () => {
        if (!await confirmAction(`Restore version ${revision.revision}? This replaces the current working plan, including unsaved edits. Saved versions remain unchanged.`)) return;
        await invoke("plan_restore_revision", { id: state.plan.id, revision: revision.revision });
        await openPlan(state.plan.id);
        message(`Restored version ${revision.revision}.`);
      }));
      return row;
    }));
    renderCandidates();
  }
  function evidence(result) {
    const breakdown = result.score_breakdown;
    if (!breakdown) return `General strong-shot quality ${number(result.aesthetic_score)}. No style term used in this ordering.`;
    const terms = [["Brief", "semantic"], ["Transcript", "transcript_boost"], ["Editorial", "editorial"], ["General aesthetic", "general_aesthetic"], ["Personal", "personal_affinity"], ["Context", "context_fit"], ["Penalties", "penalties"]];
    return terms.map(([name, key]) => `${name} ${number(breakdown[key])}`).join(" · ");
  }
  function renderCandidates() {
    const response = state.candidates;
    $("plan-candidate-status").textContent = response
      ? `${response.general.length} general · ${response.personalized.length} brief candidates. Scores use different scales; compare order within each column.`
      : "Refresh to find strong shots. Add a brief for the second ordering.";
    $("plan-personal-status").textContent = response?.profile
      ? `Experimental profile v${response.profile.version} · ${response.profile.context_key}. Human style-proof review pending.`
      : response?.brief ? "General brief matching only — no eligible personal profile. Selections retain General provenance." : "Add a brief. Without an eligible profile, this is general brief matching.";
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
        const add = button(exists ? "In plan" : "Add to plan", async () => {
          if (!await discardDrafts()) return;
          const signals = { schema_version: 1, lane, ordinal: index + 1, brief: response.brief, context: response.context_key, profile, candidate: result };
          await invoke("plan_add_item", { id: state.plan.id, item: {
            assetType: result.asset_type, mediaId: result.asset_id,
            startS: result.start_s, endS: result.end_s, reason: evidence(result),
            signalsJson: JSON.stringify(signals), origin: profile ? "personal" : "general",
            rank: result.score, profileVersion: profile?.version ?? null,
          } });
          await openPlan(state.plan.id, false);
          message("Added to the plan. No feedback was recorded.");
        }, false);
        add.disabled = exists; card.append(add);
        return card;
      }));
    }
  }
  function renderItem(item, index) {
    const form = node("form", undefined, "plans-item");
    const key = itemKey(item);
    form.dataset.assetKey = key;
    const frozen = parse(item.signalsJson);
    const candidate = frozen.candidate || {};
    const title = filename(candidate.path || item.mediaId);
    form.append(node("strong", `${index + 1}. ${title}`), pill(item.origin, item.profileVersion));
    const fields = node("div", undefined, "plans-fields");
    if (item.mediaKind === "shot") {
      form.append(node("p", Number.isFinite(candidate.start_s) ? `Source shot ${number(candidate.start_s)}–${number(candidate.end_s)} s. Edits must stay inside it.` : "Clip edits are validated against the source shot by the store.", "plans-muted"));
      for (const [label, name, value] of [["In (seconds)", "startS", item.startS], ["Out (seconds)", "endS", item.endS]]) {
        fields.append(input(label, name, value, { type: "number", min: candidate.start_s ?? 0, ...(candidate.end_s != null ? { max: candidate.end_s } : {}), step: "any", required: true }));
      }
    }
    fields.append(input("Pacing (0–1)", "pacing", item.pacing, { type: "number", min: 0, max: 1, step: "any" }));
    fields.append(input("Horizontal crop (0–1)", "cropX", item.cropX, { type: "number", min: 0, max: 1, step: "any" }));
    form.append(fields, input("Why this select?", "reason", item.reason, { multiline: true }), input("Grade JSON (recipe intent, not rendered)", "gradeJson", item.gradeJson ?? "{}", { multiline: true }));
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
      await openPlan(state.plan.id, false); message("Removed from the plan. No rejection or other feedback was inferred.");
    }));
    actions.append(button("Pick for this context", async () => {
      await invoke("record_feedback", { assetType: kind(item.mediaKind), id: item.mediaId, signal: "pick", value: 1, context: state.plan.brief, contextKey: state.plan.contextKey });
      message(`Explicit pick recorded in context “${state.plan.contextKey}”.`);
    }));
    form.append(actions);
    const details = node("details");
    details.append(node("summary", "Selection provenance (frozen)"), node("pre", JSON.stringify(frozen, null, 2)));
    form.append(details);
    form.addEventListener("input", () => dirty(key));
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
        message("Item saved. Source media and selection provenance are unchanged.");
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
      Object.assign(state.plan, fields); dirty("header", false); await refreshList(); message("Plan details saved.");
    });
  });
  $("plan-create-form").addEventListener("submit", (event) => {
    event.preventDefault();
    run(async () => {
      if (!await discardDrafts()) return;
      const plan = await invoke("plan_create", { name: $("plan-new-name").value.trim(), contextKey: $("plan-new-context").value.trim() });
      await openPlan(plan.id); $("plan-new-name").value = ""; message("Plan created.");
    });
  });
  $("plan-generate").addEventListener("click", () => run(async () => {
    $("plan-candidate-status").textContent = "Finding candidates…";
    try {
      state.candidates = await invoke("selects_candidates", { brief: $("plan-brief").value.trim() || null, context: state.plan.contextKey, top: 12 });
      renderCandidates();
    } catch (error) {
      state.candidates = null; renderCandidates();
      $("plan-candidate-status").textContent = "Candidate lookup failed. Your plan is unchanged; refresh to retry.";
      throw error;
    }
  }));
  $("plan-duplicate").addEventListener("click", () => run(async () => {
    if (!await discardDrafts()) return;
    const copy = await invoke("plan_duplicate", { id: state.plan.id, name: `${state.plan.name} copy` });
    await openPlan(copy.id); message("Duplicated the saved plan, with provenance preserved.");
  }));
  $("plan-delete").addEventListener("click", () => run(async () => {
    if (!await confirmAction(`Delete “${state.plan.name}”, its items and saved versions? Originals and feedback will not be deleted.`)) return;
    await invoke("plan_delete", { id: state.plan.id });
    state.plan = null; state.items = []; state.candidates = null; state.dirty.clear();
    renderEditor(); await refreshList(); message("Plan deleted. Original media is unchanged.");
  }));
  $("plan-revision-form").addEventListener("submit", (event) => {
    event.preventDefault();
    run(async () => {
      if (state.dirty.size) throw new Error("Save your item and plan-detail edits before saving a version.");
      const revision = await invoke("plan_save_revision", { id: state.plan.id, label: $("plan-revision-label").value });
      await openPlan(state.plan.id, false); $("plan-revision-label").value = ""; message(`Saved version ${revision.revision}.`);
    });
  });
  document.addEventListener("crush:plans-shown", () => run(async () => {
    // Navigation must not wipe local drafts. A full refresh is explicit through plan reopen.
    await refreshList();
    if (!state.loaded) { state.loaded = true; if (state.plans.length) await openPlan(state.plans[0].id); }
  }));
})();
