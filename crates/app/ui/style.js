// Preferences panel (Task 018b). Loaded after search.js; owns #style-view and the preference evidence
// block inside the asset detail drawer. Confirmed sets are evidence, not human acceptance
// of the model. The "learned" wording follows the recorded verdict in
// docs/style-proof-review.md (2026-08-31): gate-passed profiles say "Learned profile" with
// its scope line; gate-failed profiles keep the experimental copy.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;

  const $ = (selector) => document.querySelector(selector);
  const el = {
    statusLine: $("#style-status-line"),
    statusMeta: $("#style-status-meta"),
    scopeNote: $("#style-scope-note"),
    message: $("#style-message"),
    form: $("#style-create-form"),
    name: $("#style-set-name"),
    context: $("#style-set-context"),
    description: $("#style-set-description"),
    scope: $("#style-set-scope"),
    create: $("#style-create"),
    sets: $("#style-sets"),
    empty: $("#style-empty"),
    reset: $("#style-reset"),
    retrain: $("#style-retrain"),
    detailSet: $("#detail-style-set"),
    detailAdd: $("#detail-add-style"),
    evidence: $("#imported-evidence"),
    evidenceList: $("#imported-evidence-list"),
    evidenceEmpty: $("#imported-evidence-empty"),
    evidenceActions: $("#imported-evidence-actions"),
    evidenceConfirm: $("#imported-evidence-confirm"),
    evidenceSkip: $("#imported-evidence-skip"),
    evidenceCount: $("#imported-evidence-count"),
  };

  const state = {
    sets: [],
    status: null,
    detail: null,
    selectedSetId: "",
    messageTimer: null,
    deleteArmed: null,
    deleteTimer: null,
    resetArmed: false,
    resetTimer: null,
    addedTimer: null,
    evidence: [],
    evidenceSelection: new Set(),
    // Task 034: Skip is a local UI decision only — no store write, so re-imports can
    // never resurrect or revoke it; the list is the only thing it hides. Persisted in
    // localStorage so the panel does not nag across restarts, and clearable below.
    evidenceSkipped: (() => {
      try {
        return new Set(JSON.parse(localStorage.getItem("crush-skipped-evidence") || "[]"));
      } catch {
        return new Set();
      }
    })(),
  };

  const scopeLabels = { whole_set: "whole set", selected: "selected examples" };
  const statusLabels = { confirmed: "Confirmed", unconfirmed: "Unconfirmed", disabled: "Disabled" };
  const pillTones = { confirmed: "done", unconfirmed: "pending", disabled: "cancelled" };

  const metric = (value) => (Number.isFinite(value) ? value.toFixed(2) : "—");

  function showMessage(text, error = false) {
    clearTimeout(state.messageTimer);
    el.message.textContent = text;
    el.message.classList.toggle("error", error);
    el.message.hidden = false;
    state.messageTimer = setTimeout(() => {
      el.message.hidden = true;
    }, 5000);
  }

  // Task 039 B8 — backend failures speak editor language (shared mapping in app.js);
  // the raw text stays reachable through a "Copy details" button when mapped.
  function showError(error) {
    const raw = String(error);
    const mapped = window.crushErrorText ? window.crushErrorText(error) : raw;
    showMessage(mapped, true);
    if (mapped !== raw) el.message.append(window.crushCopyDetailsButton(raw));
  }

  // ---------- status ----------
  // Wording per the recorded verdict (docs/style-proof-review.md, 2026-08-31): the
  // "Learned profile" label appears ONLY for profiles whose training gate actually passed
  // (status.learned — held-out media-disjoint improvement), and it always travels with the
  // plain-language scope line so the claim stays bounded (conditions 1 and 2). Profiles that
  // did not pass keep the experimental copy. No surface ever claims more than the gate
  // measured: probe evidence over held-out media, not unseen future work.
  const LEARNED_SCOPE_TEXT =
    "This claim rests on synthetic probe evidence over held-out media from your indexed library — not on unseen future work.";

  function setProfileClasses(learned) {
    el.statusLine.classList.toggle("learned", learned);
    el.statusLine.classList.toggle("general", !learned);
  }

  function renderStatus() {
    const status = state.status;
    if (!status || !status.hasActiveProfile) {
      el.statusLine.textContent = "General model only";
      setProfileClasses(false);
      el.statusMeta.textContent =
        "Recommendations use the general strong-shot model. Confirm an example set, then update recommendations.";
      el.scopeNote.hidden = true;
      return;
    }
    if (!status.learned) {
      // Gate not passed: the profile exists but never beat the general model on held-out
      // media, so the cautious experimental copy stays (verdict condition 1).
      el.statusLine.textContent = "Experimental preferences · human review pending";
      setProfileClasses(false);
      el.statusMeta.textContent =
        "Your preference examples have not beaten the general model on held-out examples yet.";
      el.scopeNote.hidden = true;
      return;
    }
    el.statusLine.textContent = "Learned profile";
    setProfileClasses(true);
    const parts = [`Automated pair evaluation ${metric(status.heldOutMetric)} vs baseline ${metric(status.baselineMetric)}`];
    if (Number.isFinite(status.sampleCount)) parts.push(`${status.sampleCount} samples`);
    if (status.contextKey) parts.push(`context ${status.contextKey}`);
    parts.push(
      `${status.referenceSetsConfirmed} of ${status.referenceSetsTotal} reference set` +
      `${status.referenceSetsTotal === 1 ? "" : "s"} confirmed`,
    );
    el.statusMeta.textContent = parts.join(" · ");
    // Condition 2: the scope line is visible copy next to the label, never a tooltip.
    el.scopeNote.textContent = LEARNED_SCOPE_TEXT;
    el.scopeNote.hidden = false;
  }

  // ---------- reference sets ----------
  function disarmDelete() {
    clearTimeout(state.deleteTimer);
    if (state.deleteArmed) {
      state.deleteArmed.button.textContent = "Delete";
      state.deleteArmed.button.classList.remove("armed");
      state.deleteArmed = null;
    }
  }

  function disarmReset() {
    clearTimeout(state.resetTimer);
    state.resetArmed = false;
    el.reset.textContent = "Reset recommendations";
    el.reset.classList.remove("armed");
  }

  async function withButton(button, run) {
    button.disabled = true;
    try {
      await run();
    } catch (error) {
      showMessage(String(error), true);
    } finally {
      button.disabled = false;
    }
  }

  function setRow(set) {
    const row = document.createElement("div");
    row.className = "style-set-row";
    row.dataset.setId = set.id;

    const main = document.createElement("div");
    main.className = "style-set-main";
    const name = document.createElement("div");
    name.className = "style-set-name";
    name.textContent = set.name;
    const meta = document.createElement("div");
    meta.className = "style-set-meta";
    const bits = [
      `context ${set.contextKey}`,
      scopeLabels[set.scope] || set.scope,
      `${set.itemCount} item${set.itemCount === 1 ? "" : "s"}`,
    ];
    if (set.description) bits.push(set.description);
    meta.textContent = bits.join(" · ");
    main.append(name, meta);

    const pill = document.createElement("span");
    pill.className = `status-pill ${pillTones[set.status] || ""}`;
    pill.textContent = statusLabels[set.status] || set.status;

    const actions = document.createElement("div");
    actions.className = "style-set-actions";
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "button secondary small";
    if (set.status === "confirmed") {
      toggle.textContent = "Disable";
      toggle.addEventListener("click", () =>
        withButton(toggle, async () => {
          await invoke("reference_set_disable", { setId: set.id });
          showMessage(`Disabled “${set.name}” — its examples are ignored until re-confirmed.`);
          await refreshStyle();
        }));
    } else {
      toggle.textContent = "Confirm";
      toggle.addEventListener("click", () =>
        withButton(toggle, async () => {
          await invoke("reference_set_confirm", { setId: set.id });
          showMessage(`Confirmed “${set.name}” — its examples can now shape recommendations.`);
          await refreshStyle();
        }));
    }
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button danger small";
    remove.textContent = "Delete";
    remove.addEventListener("click", () =>
      withButton(remove, async () => {
        if (state.deleteArmed?.id !== set.id) {
          disarmDelete();
          state.deleteArmed = { id: set.id, button: remove };
          remove.textContent = "Really delete?";
          remove.classList.add("armed");
          state.deleteTimer = setTimeout(disarmDelete, 6000);
          return;
        }
        disarmDelete();
        await invoke("reference_set_delete", { setId: set.id });
        showMessage(`Deleted “${set.name}”.`);
        await refreshStyle();
      }));
    actions.append(toggle, remove);

    row.append(main, pill, actions);
    return row;
  }

  function renderSets() {
    el.sets.replaceChildren();
    el.empty.hidden = state.sets.length > 0;
    for (const set of state.sets) el.sets.append(setRow(set));
  }

  function renderDetailSets() {
    const previous = state.selectedSetId;
    el.detailSet.replaceChildren();
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = state.sets.length ? "Choose set…" : "No reference sets yet";
    el.detailSet.append(placeholder);
    for (const set of state.sets) {
      const option = document.createElement("option");
      option.value = set.id;
      option.textContent = `${set.name} (${statusLabels[set.status] || set.status})`;
      el.detailSet.append(option);
    }
    el.detailSet.value = previous && state.sets.some((set) => set.id === previous)
      ? previous
      : "";
    el.detailAdd.disabled = !state.detail || !el.detailSet.value;
  }

  // ---------- imported evidence (Task 034) ----------
  // Span evidence becomes preference evidence ONLY through this explicit confirmation, and
  // the honest copy says what it does today: confirmed spans are catalogued evidence and do
  // NOT train the current model (they have no vectors) — that starts when span analysis
  // lands. Nothing here ever claims "learned".
  const sourceLabels = { reel_studio: "Imported · Reel Studio", manual: "Manual clip" };

  function persistSkippedEvidence() {
    try {
      localStorage.setItem(
        "crush-skipped-evidence",
        JSON.stringify([...state.evidenceSkipped]),
      );
    } catch {
      // Storage can be unavailable (private mode); skipping still works for the session.
    }
  }

  function evidenceSummary(item) {
    const bits = [];
    if (item.description) bits.push(item.description);
    if (item.quality) bits.push(`★ ${item.quality}`);
    if (item.standout) bits.push("Standout");
    if (item.used_in) bits.push(`Used in ${item.used_in}`);
    return bits.join(" · ");
  }

  function evidenceSetLine(item) {
    if (item.confirmed) {
      const names = item.sets.length ? ` (${item.sets.join(", ")})` : "";
      return `Confirmed as evidence${names}`;
    }
    if (item.sets.length) {
      return `In “${item.sets.join(", ")}” — confirm that set to finish`;
    }
    return "";
  }

  function evidenceRow(item) {
    const row = document.createElement("div");
    row.className = "style-set-row";
    row.dataset.spanId = item.spanId;

    const select = document.createElement("label");
    select.className = "review-select";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = state.evidenceSelection.has(item.spanId);
    checkbox.setAttribute("aria-label", `Confirm ${item.externalId || item.spanId} as evidence`);
    checkbox.addEventListener("change", () => {
      if (checkbox.checked) state.evidenceSelection.add(item.spanId);
      else state.evidenceSelection.delete(item.spanId);
      renderEvidenceControls();
    });
    select.append(checkbox);

    const main = document.createElement("div");
    main.className = "style-set-main";
    const name = document.createElement("div");
    name.className = "style-set-name";
    name.textContent = item.externalId || item.spanId;
    const meta = document.createElement("div");
    meta.className = "style-set-meta";
    const bits = [sourceLabels[item.source] || item.source, evidenceSummary(item)].filter(Boolean);
    meta.textContent = bits.join(" · ");
    main.append(name, meta);
    const setLine = evidenceSetLine(item);
    if (setLine) {
      const sets = document.createElement("div");
      sets.className = "style-set-meta style-set-sets";
      sets.textContent = setLine;
      main.append(sets);
    }

    const pill = document.createElement("span");
    pill.className = `status-pill ${item.confirmed ? "done" : "pending"}`;
    pill.textContent = item.confirmed ? "Confirmed" : "Awaiting decision";

    const actions = document.createElement("div");
    actions.className = "style-set-actions";
    if (!item.confirmed) {
      const confirm = document.createElement("button");
      confirm.type = "button";
      confirm.className = "button secondary small";
      confirm.textContent = "Confirm";
      confirm.addEventListener("click", () =>
        withButton(confirm, async () => {
          const outcome = await invoke("imported_evidence_confirm", {
            spanIds: [item.spanId],
          });
          showMessage(
            `Added to “${outcome.setName}”. The set stays inert until you confirm it — and ` +
              "confirmed clips do not train recommendations until clip analysis lands.",
          );
          await refreshStyle();
        }));
      actions.append(confirm);
    }
    const skip = document.createElement("button");
    skip.type = "button";
    skip.className = "button danger small";
    skip.textContent = "Skip";
    skip.addEventListener("click", () => {
      state.evidenceSkipped.add(item.spanId);
      state.evidenceSelection.delete(item.spanId);
      persistSkippedEvidence();
      showMessage(
        "Skipped. Nothing was written to the library — skipped clips stay out of this list.",
      );
      renderEvidence();
    });
    actions.append(skip);

    row.append(select, main, pill, actions);
    return row;
  }

  function renderEvidenceControls() {
    const visible = state.evidence.filter(
      (item) => !item.confirmed && !state.evidenceSkipped.has(item.spanId),
    );
    el.evidenceActions.hidden = visible.length === 0 && state.evidenceSkipped.size === 0;
    const count = state.evidenceSelection.size;
    el.evidenceConfirm.disabled = count === 0;
    el.evidenceSkip.disabled = count === 0;
    el.evidenceCount.textContent = [
      count ? `${count} selected` : "",
      state.evidenceSkipped.size
        ? `${state.evidenceSkipped.size} skipped — Skip is local only, re-import never changes it`
        : "",
    ]
      .filter(Boolean)
      .join(" · ");
    el.evidenceConfirm.textContent = count
      ? `Confirm ${count} as evidence`
      : "Confirm selected as evidence";
  }

  function renderEvidence() {
    el.evidenceList.replaceChildren();
    const visible = state.evidence.filter(
      (item) => !state.evidenceSkipped.has(item.spanId),
    );
    el.evidenceEmpty.hidden = visible.length > 0;
    el.evidenceList.hidden = visible.length === 0;
    for (const item of visible) el.evidenceList.append(evidenceRow(item));
    renderEvidenceControls();
  }

  async function confirmSelectedEvidence() {
    const spanIds = [...state.evidenceSelection];
    if (!spanIds.length) return;
    el.evidenceConfirm.disabled = true;
    try {
      const outcome = await invoke("imported_evidence_confirm", { spanIds });
      state.evidenceSelection.clear();
      showMessage(outcome.added === 0
        ? `Already in “${outcome.setName}” — nothing to add. Confirmed clips are catalogued ` +
          "evidence; they do not train recommendations until clip analysis lands."
        : `Added ${outcome.added} imported clip${outcome.added === 1 ? "" : "s"} to ` +
          `“${outcome.setName}” (${outcome.alreadyPresent} already there). Now confirm the ` +
          "set below to make it count — and note: confirmed clips are catalogued evidence, " +
          "they do not train recommendations until clip analysis lands.");
      await refreshStyle();
    } catch (error) {
      showError(error);
      el.evidenceConfirm.disabled = false;
    }
  }

  function skipSelectedEvidence() {
    for (const id of state.evidenceSelection) state.evidenceSkipped.add(id);
    state.evidenceSelection.clear();
    persistSkippedEvidence();
    showMessage("Skipped. Nothing was written to the library — skipped clips stay out of this list.");
    renderEvidence();
  }

  function renderAll() {
    renderStatus();
    renderSets();
    renderDetailSets();
    renderEvidence();
  }

  async function refreshStyle() {
    try {
      const [sets, status, evidence] = await Promise.all([
        invoke("reference_set_list"),
        invoke("style_profile_status"),
        invoke("imported_evidence_list"),
      ]);
      state.sets = sets;
      state.status = status;
      state.evidence = evidence;
    } catch (error) {
      showMessage(String(error), true);
    }
    renderAll();
  }

  // ---------- actions ----------
  el.form.addEventListener("submit", (event) => {
    event.preventDefault();
    withButton(el.create, async () => {
      const name = el.name.value.trim();
      if (!name) return;
      await invoke("reference_set_create", {
        name,
        contextKey: el.context.value.trim() || "default",
        description: el.description.value.trim(),
        scope: el.scope.value,
      });
      el.name.value = "";
      el.description.value = "";
      showMessage(`Created “${name}”. It stays inert until you confirm it.`);
      await refreshStyle();
    });
  });

  el.reset.addEventListener("click", () =>
    withButton(el.reset, async () => {
      if (!state.resetArmed) {
        disarmReset();
        state.resetArmed = true;
        el.reset.textContent = "Really reset?";
        el.reset.classList.add("armed");
        state.resetTimer = setTimeout(disarmReset, 6000);
        return;
      }
      disarmReset();
      const count = await invoke("style_profile_reset");
      showMessage(count > 0
        ? "Recommendations reset to the general model."
        : "Recommendations already use the general model.");
      await refreshStyle();
    }));

  el.retrain.addEventListener("click", () =>
    withButton(el.retrain, async () => {
      el.retrain.textContent = "Updating…";
      try {
        const outcome = await invoke("style_profile_retrain");
        showMessage(outcome.trained
          ? "Recommendations updated from your confirmed examples."
          : "Not enough evidence yet — recommendations are unchanged.");
        await refreshStyle();
      } finally {
        el.retrain.textContent = "Update recommendations";
      }
    }));

  async function addToSet() {
    const detail = state.detail;
    const setId = el.detailSet.value;
    if (!detail || !setId || el.detailAdd.disabled) return;
    el.detailAdd.disabled = true;
    try {
      await invoke("reference_set_add_item", {
        setId,
        mediaKind: detail.kind,
        mediaId: detail.id,
      });
      state.selectedSetId = setId;
      el.detailAdd.textContent = "Added";
      clearTimeout(state.addedTimer);
      state.addedTimer = setTimeout(() => {
        el.detailAdd.textContent = "Add to set";
      }, 2000);
      await refreshStyle();
    } catch (error) {
      showMessage(String(error), true);
    } finally {
      el.detailAdd.disabled = false;
    }
  }

  el.detailSet.addEventListener("change", () => {
    state.selectedSetId = el.detailSet.value;
    el.detailAdd.disabled = !el.detailSet.value;
  });
  el.detailAdd.addEventListener("click", addToSet);

  el.evidenceConfirm.addEventListener("click", confirmSelectedEvidence);
  el.evidenceSkip.addEventListener("click", skipSelectedEvidence);

  // ---------- wiring ----------
  document.addEventListener("crush:detail", (event) => {
    state.detail = event.detail;
    if (state.detail) refreshStyle();
    else renderDetailSets();
  });
  document.addEventListener("crush:style-shown", () => refreshStyle());
})();
