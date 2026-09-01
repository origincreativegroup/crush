// Preferences panel (Task 018b). Loaded after search.js; owns #style-view and the preference evidence
// block inside the asset detail drawer. Confirmed sets are evidence, not human acceptance
// of the model. Automated eval success must not bypass HANDOFF's held-out proof review.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;

  const $ = (selector) => document.querySelector(selector);
  const el = {
    statusLine: $("#style-status-line"),
    statusMeta: $("#style-status-meta"),
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

  // ---------- status ----------
  function renderStatus() {
    const status = state.status;
    if (!status || !status.hasActiveProfile || !status.learned) {
      el.statusLine.textContent = "General model only";
      el.statusLine.classList.remove("learned");
      el.statusLine.classList.add("general");
      el.statusMeta.textContent = status && status.hasActiveProfile
        ? "Your preference examples have not beaten the general model on held-out examples yet."
        : "Recommendations use the general strong-shot model. Confirm an example set, then update recommendations.";
      return;
    }
    el.statusLine.textContent = "Experimental preferences · human review pending";
    el.statusLine.classList.remove("learned");
    el.statusLine.classList.add("general");
    const parts = [`Automated pair evaluation ${metric(status.heldOutMetric)} vs baseline ${metric(status.baselineMetric)}`];
    if (Number.isFinite(status.sampleCount)) parts.push(`${status.sampleCount} samples`);
    if (status.contextKey) parts.push(`context ${status.contextKey}`);
    parts.push(
      `${status.referenceSetsConfirmed} of ${status.referenceSetsTotal} reference set` +
      `${status.referenceSetsTotal === 1 ? "" : "s"} confirmed`,
    );
    el.statusMeta.textContent = parts.join(" · ");
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

  function renderAll() {
    renderStatus();
    renderSets();
    renderDetailSets();
  }

  async function refreshStyle() {
    try {
      const [sets, status] = await Promise.all([
        invoke("reference_set_list"),
        invoke("style_profile_status"),
      ]);
      state.sets = sets;
      state.status = status;
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

  // ---------- wiring ----------
  document.addEventListener("crush:detail", (event) => {
    state.detail = event.detail;
    if (state.detail) refreshStyle();
    else renderDetailSets();
  });
  document.addEventListener("crush:style-shown", () => refreshStyle());
})();
