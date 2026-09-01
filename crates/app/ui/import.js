// Reel Studio catalogue import (Task 022). Owns #import-dialog. Dry-run first, apply second;
// the apply button only unlocks after a dry run of the same inputs has been shown.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;
  const $ = (selector) => document.querySelector(selector);
  const el = {
    open: $("#import-reel-studio"),
    dialog: $("#import-dialog"),
    close: $("#import-close"),
    form: $("#import-form"),
    catalogue: $("#import-catalogue"),
    pickCatalogue: $("#import-pick-catalogue"),
    originals: $("#import-originals"),
    pickOriginals: $("#import-pick-originals"),
    recipes: $("#import-recipes"),
    pickRecipes: $("#import-pick-recipes"),
    library: $("#import-library"),
    pickLibrary: $("#import-pick-library"),
    context: $("#import-context"),
    hash: $("#import-hash"),
    dryRun: $("#import-dry-run"),
    apply: $("#import-apply"),
    status: $("#import-status"),
    report: $("#import-report"),
    summary: $("#import-summary"),
    writes: $("#import-writes"),
    issues: $("#import-issues"),
    segments: $("#import-segments"),
    recipeRows: $("#import-recipes-rows"),
    candidates: $("#import-candidates"),
  };
  if (!el.dialog) return;

  const state = { originals: [], recipes: [], library: null, catalogue: null, dryRunKey: null, busy: false };
  const node = (tag, text, className) => {
    const element = document.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (className) element.className = className;
    return element;
  };
  const requestKey = () => JSON.stringify(request(false));
  function request(apply) {
    return {
      catalogue: state.catalogue || "",
      originals: state.originals,
      library: state.library,
      recipes: state.recipes,
      contextKey: el.context.value.trim() || "default",
      matchByHash: el.hash.checked,
      apply,
    };
  }
  function refreshInputs() {
    el.catalogue.value = state.catalogue || "";
    el.originals.value = state.originals.join("  ·  ");
    el.recipes.value = state.recipes.map((path) => path.split(/[\\/]/).at(-1)).join(", ");
    el.library.value = state.library || "";
    el.apply.disabled = state.busy || !state.dryRunKey || state.dryRunKey !== requestKey();
    el.dryRun.disabled = state.busy || !state.catalogue;
  }
  async function pick(options) {
    try {
      const picked = await bridge.dialog.open(options);
      if (!picked) return [];
      return Array.isArray(picked) ? picked : [picked];
    } catch (error) {
      el.status.textContent = `Picker failed: ${String(error)}`;
      return [];
    }
  }
  el.pickCatalogue.addEventListener("click", async () => {
    const [path] = await pick({ multiple: false, title: "Choose clips.db", filters: [{ name: "SQLite catalogue", extensions: ["db", "sqlite"] }] });
    if (path) state.catalogue = path;
    refreshInputs();
  });
  el.pickOriginals.addEventListener("click", async () => {
    for (const path of await pick({ directory: true, multiple: true, title: "Original footage folder" })) {
      if (!state.originals.includes(path)) state.originals.push(path);
    }
    refreshInputs();
  });
  el.pickRecipes.addEventListener("click", async () => {
    for (const path of await pick({ multiple: true, title: "Reel Studio recipe JSON", filters: [{ name: "Reel recipe", extensions: ["json"] }] })) {
      if (!state.recipes.includes(path)) state.recipes.push(path);
    }
    refreshInputs();
  });
  el.pickLibrary.addEventListener("click", async () => {
    const [path] = await pick({ directory: true, multiple: false, title: "Reel Studio library folder" });
    if (path) state.library = path;
    refreshInputs();
  });
  for (const input of [el.context, el.hash]) input.addEventListener("input", refreshInputs);

  function renderReport(report) {
    el.report.hidden = false;
    const matched = report.sources.filter((source) => source.video_id).length;
    const count = (outcome) => report.segments.filter((segment) => segment.outcome === outcome).length;
    el.summary.textContent = `${report.mode === "apply" ? "Applied" : "Dry run"}: ${matched} of ${report.sources.length} source clips matched · segments new ${count("new")}, updated ${count("updated")}, unchanged ${count("unchanged")}, skipped ${count("skipped")} · recipes ${report.recipes.filter((recipe) => recipe.outcome === "new").length} new of ${report.recipes.length} · ${report.issues.length} issue${report.issues.length === 1 ? "" : "s"}.`;
    const writes = report.planned_writes;
    el.writes.textContent = `${report.mode === "apply" ? "Wrote" : "Would write"}: ${writes.manual_spans_insert} new spans, ${writes.manual_spans_update} refreshed, ${writes.render_recipes_insert} recipes, ${writes.plans_insert} projects (${writes.plan_items_insert} items). No preference feedback and no reference sets are created by importing.`;
    el.issues.replaceChildren();
    for (const issue of report.issues) {
      const row = node("div", undefined, `import-issue ${issue.kind}`);
      row.append(node("strong", issue.kind.replaceAll("_", " ")), node("span", ` ${issue.subject} — ${issue.detail}`));
      el.issues.append(row);
    }
    el.segments.replaceChildren();
    for (const segment of report.segments) {
      const row = node("tr");
      row.append(
        node("td", segment.segment_id, "mono"),
        node("td", segment.outcome, `import-outcome ${segment.outcome}`),
        node("td", `${segment.start_s.toFixed(2)} – ${segment.end_s.toFixed(2)}`, "mono"),
        node("td", `${segment.boundary_basis.replaceAll("_", " ")} ±${segment.boundary_tolerance_s.toFixed(2)} s`),
        node("td", segment.reason || ""),
      );
      el.segments.append(row);
    }
    el.recipeRows.replaceChildren();
    for (const recipe of report.recipes) {
      const row = node("tr");
      row.append(
        node("td", recipe.file.split(/[\\/]/).at(-1), "mono"),
        node("td", recipe.outcome, `import-outcome ${recipe.outcome}`),
        node("td", String(recipe.items)),
        node("td", recipe.reason || (recipe.finished_project ? "finished project (used_in)" : "")),
      );
      el.recipeRows.append(row);
    }
    el.candidates.hidden = report.reference_set_candidates.length === 0;
    el.candidates.textContent = report.reference_set_candidates.length
      ? `Finished projects you could confirm as previous-work examples in Preferences: ${report.reference_set_candidates.join(", ")}. Importing did not do this for you.`
      : "";
  }

  async function run(apply) {
    if (state.busy || !state.catalogue) return;
    state.busy = true;
    refreshInputs();
    el.status.textContent = apply ? "Applying import…" : "Reading the catalogue…";
    try {
      const report = await invoke("import_reel_studio", { request: request(apply) });
      renderReport(report);
      if (apply) {
        state.dryRunKey = null;
        el.status.textContent = "Import applied. Imported projects are in Projects with the Historical label.";
        document.dispatchEvent(new CustomEvent("crush:library-changed"));
      } else {
        state.dryRunKey = requestKey();
        el.status.textContent = "Dry run complete. Read the report, then apply.";
      }
    } catch (error) {
      el.status.textContent = `Import failed: ${String(error)}`;
    } finally {
      state.busy = false;
      refreshInputs();
    }
  }
  el.form.addEventListener("submit", (event) => {
    event.preventDefault();
    run(false);
  });
  el.apply.addEventListener("click", () => run(true));
  el.open?.addEventListener("click", () => {
    refreshInputs();
    el.dialog.showModal();
  });
  el.close.addEventListener("click", () => el.dialog.close());
  el.dialog.addEventListener("click", (event) => {
    if (event.target === el.dialog) el.dialog.close();
  });
})();
