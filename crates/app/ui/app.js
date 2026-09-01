const bridge = window.__TAURI__;
const invoke = bridge?.core?.invoke;

// Task 039 B8 — error-language pass. Backend failures used to surface verbatim
// ("Disk full", "The vector store is unavailable."); this maps the common ones into
// editor language while the raw text stays available through a "Copy details" button
// (same affordance as the Library failure row). Unmatched errors pass through
// unchanged — an unmapped message is honest; an invented one is not.
const errorRules = [
  [/disk full|no space left/i,
    "Disk full — free up space on the drive, then try again."],
  [/vector store/i,
    "Search could not run — the local search index is unavailable. Try again in a moment; if it keeps failing, run Doctor from the sidebar."],
  [/permission denied|eperm|eacces|operation not permitted/i,
    "Crush does not have permission to read that location — grant access, then try again."],
  [/database.*(locked|busy)|locked.*database/i,
    "The library database is busy — try again in a moment."],
];

window.crushErrorText = (error) => {
  const raw = String(error ?? "");
  return errorRules.find(([pattern]) => pattern.test(raw))?.[1] ?? raw;
};

// Returns a "Copy details" button carrying the untouched backend text, so a mapped
// headline never destroys the detail someone may need to report a bug.
window.crushCopyDetailsButton = (raw) => {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "button secondary small";
  button.textContent = "Copy details";
  button.title = "Copy the original error text";
  button.addEventListener("click", () => {
    bridge?.clipboardManager?.writeText(`Crush error: ${String(raw)}`).catch(() => {});
  });
  return button;
};

const elements = {
  boot: document.querySelector("#boot"),
  firstRun: document.querySelector("#first-run"),
  appShell: document.querySelector("#app-shell"),
  modelList: document.querySelector("#model-list"),
  modelError: document.querySelector("#model-error"),
  retryModels: document.querySelector("#retry-models"),
  continueModels: document.querySelector("#continue-models"),
  addFolder: document.querySelector("#add-folder"),
  emptyAddFolder: document.querySelector("#empty-add-folder"),
  reindex: document.querySelector("#reindex"),
  locateAsset: document.querySelector("#locate-asset"),
  removeAsset: document.querySelector("#remove-asset"),
  removeDialog: document.querySelector("#remove-asset-dialog"),
  removeCancel: document.querySelector("#remove-asset-cancel"),
  removeConfirm: document.querySelector("#remove-asset-confirm"),
  removeCopy: document.querySelector("#remove-asset-copy"),
  cancel: document.querySelector("#cancel"),
  emptyLibrary: document.querySelector("#empty-library"),
  videoTableWrap: document.querySelector("#video-table-wrap"),
  videoRows: document.querySelector("#video-rows"),
  indexingStatus: document.querySelector("#indexing-status"),
  libraryMessage: document.querySelector("#library-message"),
  libraryView: document.querySelector("#library-view"),
  selectAll: document.querySelector("#library-select-all"),
  batchBar: document.querySelector("#library-batch-bar"),
  batchCount: document.querySelector("#library-batch-count"),
  batchPick: document.querySelector("#library-batch-pick"),
  batchReject: document.querySelector("#library-batch-reject"),
  batchRating: document.querySelector("#library-batch-rating"),
  batchCollection: document.querySelector("#library-batch-collection"),
  batchNew: document.querySelector("#library-batch-collection-new"),
  batchNewName: document.querySelector("#library-batch-collection-name"),
  batchNewCreate: document.querySelector("#library-batch-collection-create"),
  batchNewCancel: document.querySelector("#library-batch-collection-cancel"),
  batchAdd: document.querySelector("#library-batch-add-collection"),
  batchHint: document.querySelector("#library-batch-hint"),
  batchClear: document.querySelector("#library-batch-clear"),
  dropOverlay: document.querySelector("#drop-overlay"),
  doctorLink: document.querySelector("#doctor-link"),
  doctorDialog: document.querySelector("#doctor-dialog"),
  closeDoctor: document.querySelector("#close-doctor"),
  runDoctor: document.querySelector("#run-doctor"),
  doctorResult: document.querySelector("#doctor-result"),
};

const state = {
  models: [],
  modelProgress: new Map(),
  modelJob: null,
  modelFailure: null,
  videos: [],
  jobs: { background: [], pipeline: [] },
  // Multi-select (Task 039 C5). selectedIds is the source of truth and survives
  // re-renders (the table repaints on every 850 ms poll while indexing); anchorId
  // is the last row the user clicked, the fixed end of a Shift-click range.
  selectedIds: new Set(),
  anchorId: null,
  expandedVideoIds: new Set(),
  pendingRemoveIds: null,
  poll: null,
  messageTimer: null,
  // Batch re-index queue (C5): the backend runs one ingest at a time, so a batch
  // re-index is an honest sequential queue — each asset's real job runs, reports
  // progress, and finishes before the next starts.
  reindexQueue: null,
  reindexTotal: 0,
  reindexFailed: 0,
  // Assets that could not start at all (e.g. removed mid-batch → "asset … was not
  // found"): skipped, not aborting the queue, and reported separately in the
  // summary because there is no failed row to point at.
  reindexSkipped: 0,
  reindexSkipReason: null,
  reindexCurrentId: null,
  // The background job id of the asset currently being re-indexed (from
  // reindex_asset's TaskStarted). The backend keeps every background task forever
  // keyed by UUID, so cancel detection must match THIS job's id — matching by
  // kind alone would trip over stale tasks from earlier in the session.
  reindexCurrentJobId: null,
  reindexBusy: false,
  reindexArmed: false,
  reindexArmTimer: null,
  // Batch bar collections (loaded on first use, not at boot).
  collections: null,
  collectionsLoading: false,
  // Job ids whose finished-ingest relink summary has already been announced (Task 038).
  // Backend tasks are never pruned and every progress event re-carries them, so this
  // is what keeps the "moved or renamed" message from re-firing forever.
  announcedIngestJobs: new Set(),
};

function humanBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${(bytes / 1_000_000).toFixed(bytes >= 100_000_000 ? 0 : 1)} MB`;
}

function fileParts(path) {
  const parts = path.split(/[\\/]/);
  return {
    name: parts.at(-1) || path,
    directory: parts.slice(0, -1).join("/") || "/",
  };
}

function formatDuration(seconds) {
  if (!Number.isFinite(seconds)) return "—";
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remaining = total % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remaining).padStart(2, "0")}`
    : `${minutes}:${String(remaining).padStart(2, "0")}`;
}

function formatResolution(video) {
  return video.width && video.height ? `${video.width} × ${video.height}` : "—";
}

function setVisible(element, visible) {
  element.hidden = !visible;
}

function showMessage(message, error = false) {
  clearTimeout(state.messageTimer);
  elements.libraryMessage.textContent = message;
  elements.libraryMessage.classList.toggle("error", error);
  setVisible(elements.libraryMessage, true);
  state.messageTimer = setTimeout(() => setVisible(elements.libraryMessage, false), 5000);
}

function renderModels() {
  elements.modelList.replaceChildren();
  for (const model of state.models) {
    const progress = state.modelProgress.get(model.name);
    const present = model.status === "present";
    const downloaded = present ? model.bytes : (progress?.downloaded ?? 0);
    const total = progress?.total || model.bytes;
    const percent = present ? 100 : total > 0 ? Math.min(100, (downloaded / total) * 100) : 0;

    const row = document.createElement("div");
    row.className = "model-row";
    const heading = document.createElement("div");
    heading.className = "model-heading";
    const name = document.createElement("span");
    name.className = "model-name";
    name.textContent = model.name;
    const meta = document.createElement("span");
    meta.className = "model-meta";
    if (present) {
      meta.textContent = `${humanBytes(model.bytes)} · Ready`;
    } else if (progress) {
      meta.textContent = `${humanBytes(downloaded)} of ${humanBytes(total)}`;
    } else {
      meta.textContent = `${humanBytes(model.bytes)} · Waiting`;
    }
    heading.append(name, meta);

    const track = document.createElement("div");
    track.className = "progress-track";
    track.setAttribute("role", "progressbar");
    track.setAttribute("aria-valuemin", "0");
    track.setAttribute("aria-valuemax", "100");
    track.setAttribute("aria-valuenow", String(Math.round(percent)));
    const fill = document.createElement("div");
    fill.className = "progress-fill";
    fill.style.width = `${percent}%`;
    track.append(fill);
    row.append(heading, track);
    elements.modelList.append(row);
  }

  const ready = state.models.length > 0 && state.models.every((model) => model.status === "present");
  elements.continueModels.disabled = !ready;
  setVisible(elements.retryModels, Boolean(state.modelFailure));
  setVisible(elements.modelError, Boolean(state.modelFailure));
  elements.modelError.textContent = state.modelFailure || "";
}

async function refreshModels() {
  state.models = await invoke("models_status");
  renderModels();
  return state.models.every((model) => model.status === "present");
}

async function downloadModels() {
  if (state.modelJob) return;
  state.modelJob = "starting";
  state.modelFailure = null;
  state.modelProgress.clear();
  renderModels();
  try {
    const started = await invoke("models_download");
    if (state.modelJob) state.modelJob = started.jobId;
  } catch (error) {
    state.modelJob = null;
    state.modelFailure = String(error);
    renderModels();
  }
}

async function onDownloadProgress(event) {
  const progress = event.payload;
  if (progress.name) state.modelProgress.set(progress.name, progress);
  if (progress.status === "failed") {
    state.modelJob = null;
    state.modelFailure = progress.error || "Model download failed.";
  } else if (progress.status === "done") {
    state.modelJob = null;
    state.modelFailure = null;
    try {
      await refreshModels();
    } catch (error) {
      state.modelFailure = String(error);
    }
  }
  renderModels();
}

function latestJob(videoId) {
  return state.jobs.pipeline.find((job) => job.video_id === videoId) || null;
}

function videoPresentation(video) {
  const job = latestJob(video.id);
  if (job?.status === "running" || job?.status === "queued") {
    const stages = {
      split: ["Splitting", 18],
      embed: ["Embedding", 56],
      analyze: ["Analyzing", 70],
      transcribe: ["Transcribing", 84],
    };
    const [label, progress] = stages[job.stage] || ["Indexing", 10];
    return { label, progress, tone: "active", active: true, job };
  }
  if (job?.status === "cancelled") {
    return { label: "Cancelled", progress: 0, tone: "cancelled", active: false, job };
  }
  const statuses = {
    pending: ["Pending", 6, "pending"],
    split: ["Split", 34, "active"],
    embedded: ["Embedded", 68, "active"],
    transcribed: ["Transcribed", 92, "active"],
    done: ["Done", 100, "done"],
    failed: ["Failed", 0, "failed"],
  };
  const [label, progress, tone] = statuses[video.status] || [video.status, 0, "pending"];
  return { label, progress, tone, active: false, job };
}

function errorDetails(video, presentation) {
  // `latestJob` follows the backend's newest-first ordering. Do not surface an older
  // failure after a later retry has queued, completed, or been cancelled.
  const failedJob = presentation.job?.status === "failed" ? presentation.job : null;
  if (!failedJob) return null;
  return {
    error: video.lastError || failedJob?.error || "Unknown indexing error",
    jobId: failedJob?.id || "unknown",
    stage: failedJob?.stage || presentation.job?.stage || "unknown",
    logPath: failedJob?.debug_dir || "No debug log was retained",
  };
}

function cell(className, text) {
  const element = document.createElement("td");
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}

// Selection helpers (Task 039 C5). The selection is a Set of asset ids ordered by
// the table (state.videos), so batch operations run top-to-bottom regardless of the
// click order.
function selectedAssets() {
  return state.videos.filter((video) => state.selectedIds.has(video.id));
}

function disarmReindex() {
  clearTimeout(state.reindexArmTimer);
  state.reindexArmed = false;
}

function renderVideos() {
  elements.videoRows.replaceChildren();
  setVisible(elements.emptyLibrary, state.videos.length === 0);
  setVisible(elements.videoTableWrap, state.videos.length > 0);

  // Rows that left the list (removed, or replaced between polls) drop out of the
  // selection; everything still listed keeps its selected state across re-renders.
  for (const id of [...state.selectedIds]) {
    if (!state.videos.some((video) => video.id === id)) state.selectedIds.delete(id);
  }
  const selectionCount = state.selectedIds.size;
  // Locate is inherently per-asset (one file → one new path): the toolbar button
  // lights up only for exactly one selected asset whose source is missing, and each
  // missing row also carries its own Locate action (rendered below) so the remedy
  // stays reachable no matter what else is selected.
  const selectedAsset = selectionCount === 1 ? selectedAssets()[0] : null;
  const queueRunning = state.reindexQueue !== null;
  elements.reindex.disabled = selectionCount === 0 || isIngestActive() || queueRunning;
  elements.removeAsset.disabled = selectionCount === 0 || isIngestActive();
  elements.reindex.textContent = state.reindexArmed
    ? `Really re-index ${selectionCount}?`
    : "Re-index selected";
  elements.reindex.classList.toggle("armed", state.reindexArmed);
  elements.locateAsset.disabled =
    selectionCount !== 1 || isIngestActive() || !selectedAsset?.sourceMissing;
  elements.selectAll.disabled = state.videos.length === 0;
  elements.selectAll.checked = state.videos.length > 0 && selectionCount === state.videos.length;
  elements.selectAll.indeterminate = selectionCount > 0 && selectionCount < state.videos.length;

  for (const video of state.videos) {
    const presentation = videoPresentation(video);
    const details = errorDetails(video, presentation);
    const selected = state.selectedIds.has(video.id);
    const expanded = state.expandedVideoIds.has(video.id);
    const parts = fileParts(video.path);

    const row = document.createElement("tr");
    row.className = `video-row${selected ? " selected" : ""}`;
    row.dataset.videoId = video.id;
    row.tabIndex = 0;
    row.setAttribute("aria-selected", String(selected));
    row.addEventListener("click", (event) => {
      selectVideo(video.id, event);
    });
    // Shift-click extends the selection, not the DOM text selection.
    row.addEventListener("mousedown", (event) => {
      if (event.shiftKey) event.preventDefault();
    });
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectVideo(video.id, event);
      } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        moveRowSelection(video.id, event.key === "ArrowDown" ? 1 : -1);
      }
    });

    const selectCell = cell("select-column");
    const selectDot = document.createElement("span");
    selectDot.className = "select-dot";
    selectCell.append(selectDot);

    const nameCell = cell("");
    const name = document.createElement("div");
    name.className = "file-name";
    name.textContent = parts.name;
    name.title = video.path;
    const path = document.createElement("div");
    path.className = "file-path";
    path.textContent = parts.directory;
    nameCell.append(name, path);

    const durationCell = cell("mono", formatDuration(video.durationS));
    const resolutionCell = cell("mono", formatResolution(video));
    const statusCell = cell("");
    const statusBox = document.createElement("div");
    statusBox.className = "status-cell";
    const pill = document.createElement("span");
    pill.className = `status-pill ${presentation.tone}`;
    pill.textContent = presentation.label;
    statusBox.append(pill);
    if (video.sourceMissing) {
      // The row's recorded file is not on disk right now. A bare green Done would hide
      // that, so the row carries the same failed tone as a Failed row, in plain
      // language, with the row's own "Locate…" action right under it — relinking is
      // per-asset, so it must not depend on what else happens to be selected.
      const missing = document.createElement("span");
      missing.className = "status-pill failed";
      missing.textContent = "Source missing";
      const locate = document.createElement("button");
      locate.className = "button secondary small";
      locate.type = "button";
      locate.textContent = "Locate…";
      locate.setAttribute("aria-label", `Locate moved file for ${parts.name}`);
      locate.addEventListener("click", (event) => {
        event.stopPropagation();
        locateMovedFile(video.id);
      });
      // Enter/Space must activate the button, not also toggle the row selection.
      locate.addEventListener("keydown", (event) => event.stopPropagation());
      statusBox.append(missing, locate);
    }
    if (presentation.active) {
      const progress = document.createElement("div");
      progress.className = "progress-track row-progress";
      const fill = document.createElement("div");
      fill.className = "progress-fill";
      fill.style.width = `${presentation.progress}%`;
      progress.append(fill);
      statusBox.append(progress);
    }
    statusCell.append(statusBox);

    const shotsCell = cell("number-column mono", video.assetType === "photo" ? "—" : String(video.shots));
    const expandCell = cell("expand-column");
    if (details) {
      const expand = document.createElement("button");
      expand.className = `chevron-button${expanded ? " open" : ""}`;
      expand.type = "button";
      expand.textContent = "›";
      expand.setAttribute("aria-label", expanded ? "Hide error details" : "Show error details");
      expand.addEventListener("click", (event) => {
        event.stopPropagation();
        toggleError(video.id);
      });
      expandCell.append(expand);
    }

    row.append(selectCell, nameCell, durationCell, resolutionCell, statusCell, shotsCell, expandCell);
    elements.videoRows.append(row);

    if (details && expanded) {
      const errorRow = document.createElement("tr");
      errorRow.className = "error-row";
      const errorCell = document.createElement("td");
      errorCell.colSpan = 7;
      const panel = document.createElement("div");
      panel.className = "error-panel";
      const copy = document.createElement("div");
      const strong = document.createElement("strong");
      strong.textContent = details.error;
      const metadata = document.createElement("pre");
      metadata.textContent = `job ${details.jobId}\nstage ${details.stage}\nlog ${details.logPath}`;
      copy.append(strong, metadata);
      const copyButton = document.createElement("button");
      copyButton.className = "button secondary";
      copyButton.type = "button";
      copyButton.textContent = "Copy details";
      copyButton.addEventListener("click", () => copyErrorDetails(details));
      panel.append(copy, copyButton);
      errorCell.append(panel);
      errorRow.append(errorCell);
      elements.videoRows.append(errorRow);
    }
  }

  renderIndexingStatus();
  renderLibraryBatchBar();
}

// Click model (Task 039 C5): plain click selects one row (clicking the only selected
// row clears — the pre-existing toggle), ⌘/Ctrl-click toggles a row in or out, and
// Shift-click selects the range from the last-clicked anchor. Modified clicks never
// collapse the rest of the selection.
function selectVideo(videoId, event = {}) {
  const meta = event.metaKey || event.ctrlKey;
  const shift = event.shiftKey;
  if (meta) {
    if (state.selectedIds.has(videoId)) state.selectedIds.delete(videoId);
    else state.selectedIds.add(videoId);
    state.anchorId = videoId;
  } else if (shift) {
    const anchor = state.anchorId && state.videos.some((video) => video.id === state.anchorId)
      ? state.anchorId
      : videoId;
    const from = state.videos.findIndex((video) => video.id === anchor);
    const to = state.videos.findIndex((video) => video.id === videoId);
    const [start, end] = from <= to ? [from, to] : [to, from];
    state.selectedIds = new Set(
      state.videos.slice(start, end + 1).map((video) => video.id),
    );
  } else if (state.selectedIds.size === 1 && state.selectedIds.has(videoId)) {
    state.selectedIds.clear();
    state.anchorId = null;
  } else {
    state.selectedIds = new Set([videoId]);
    state.anchorId = videoId;
  }
  disarmReindex();
  renderVideos();
}

// Arrow keys move the row selection (Task 039 B11). Selection follows the cursor
// instead of toggling, and focus is restored after renderVideos rebuilds the rows.
function moveRowSelection(fromId, delta) {
  const index = state.videos.findIndex((video) => video.id === fromId);
  const next = state.videos[index + delta];
  if (!next) return;
  state.selectedIds = new Set([next.id]);
  state.anchorId = next.id;
  disarmReindex();
  renderVideos();
  elements.videoRows
    .querySelector(`tr[data-video-id="${CSS.escape(next.id)}"]`)
    ?.focus();
}

function toggleError(videoId) {
  if (state.expandedVideoIds.has(videoId)) state.expandedVideoIds.delete(videoId);
  else state.expandedVideoIds.add(videoId);
  renderVideos();
}

async function copyErrorDetails(details) {
  const text = `Crush indexing error\njob: ${details.jobId}\nstage: ${details.stage}\nerror: ${details.error}\nlog: ${details.logPath}`;
  try {
    await bridge.clipboardManager.writeText(text);
    showMessage("Error details copied.");
  } catch (error) {
    showMessage(`Could not copy details: ${String(error)}`, true);
  }
}

function activeBackgroundTask(kind) {
  return state.jobs.background.find((task) => task.kind === kind && task.status === "running");
}

function isIngestActive() {
  return Boolean(activeBackgroundTask("ingest"));
}

function renderIndexingStatus() {
  const active = isIngestActive();
  elements.cancel.hidden = !active;
  elements.addFolder.disabled = active;
  const selectionCount = state.selectedIds.size;
  const selectedAsset = selectionCount === 1 ? selectedAssets()[0] : null;
  elements.reindex.disabled = selectionCount === 0 || active || state.reindexQueue !== null;
  elements.removeAsset.disabled = selectionCount === 0 || active;
  elements.locateAsset.disabled =
    selectionCount !== 1 || active || !selectedAsset?.sourceMissing;
  const dot = document.createElement("span");
  dot.className = `status-dot${active ? "" : " idle"}`;
  dot.setAttribute("aria-hidden", "true");
  const text = document.createElement("span");
  if (active) {
    const done = state.videos.filter((video) => video.status === "done").length;
    const total = Math.max(state.videos.length, 1);
    const percentages = state.videos.map((video) => videoPresentation(video).progress);
    const percent = percentages.length
      ? Math.round(percentages.reduce((sum, value) => sum + value, 0) / percentages.length)
      : 0;
    text.textContent = `Indexing ${done} of ${total} · ${percent}%`;
  } else {
    text.textContent = state.videos.length
      ? `${state.videos.length} asset${state.videos.length === 1 ? "" : "s"} indexed`
      : "Library idle";
  }
  elements.indexingStatus.replaceChildren(dot, text);
}

// ---------- Library batch bar (Task 039 C5) ----------
// Mirrors the Review batch bar (wave 1): count line, editorial actions, inline
// create-and-add collection flow. Editorial actions are photo-scoped: collections
// hold photos and shots, and pick/reject/rating is defined per photo or shot — a
// whole video's verdicts live on its shots in Review. Rather than pretend otherwise,
// the controls disable with an honest hint while a video is in the selection.
function renderLibraryBatchBar() {
  const count = state.selectedIds.size;
  if (count === 0 && !elements.batchNew.hidden) {
    elements.batchNew.hidden = true;
    elements.batchCollection.hidden = false;
    elements.batchCollection.value = "";
  }
  elements.batchBar.hidden = count === 0;
  elements.batchCount.textContent = `${count} selected`;
  if (count > 0) loadLibraryCollections();

  const photosOnly = selectedAssets().every((asset) => asset.assetType === "photo");
  const editorialReady = count > 0 && photosOnly;
  const target = elements.batchCollection.value;
  elements.batchPick.disabled = !editorialReady;
  elements.batchReject.disabled = !editorialReady;
  elements.batchRating.disabled = !editorialReady;
  elements.batchCollection.disabled = !editorialReady;
  elements.batchAdd.disabled = !editorialReady || !target || target === "new";
  elements.batchHint.hidden = count === 0 || photosOnly;
}

// The batch target list loads when the bar first appears (not at boot) and mirrors
// the Review bar's option shape, including the honest empty state.
async function loadLibraryCollections() {
  if (state.collections || state.collectionsLoading) return;
  state.collectionsLoading = true;
  try {
    state.collections = await invoke("collection_list");
  } catch (error) {
    showMessage(crushErrorText(error), true);
    return;
  } finally {
    state.collectionsLoading = false;
  }
  renderLibraryCollectionOptions();
}

function renderLibraryCollectionOptions(selectedId = null) {
  if (!state.collections) return;
  const current = selectedId ?? elements.batchCollection.value;
  elements.batchCollection.replaceChildren();
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = state.collections.length
    ? "Add to collection…"
    : "No collections yet — create one to group assets";
  elements.batchCollection.append(placeholder);
  for (const collection of state.collections) {
    const option = document.createElement("option");
    option.value = collection.id;
    option.textContent = collection.name;
    elements.batchCollection.append(option);
  }
  const create = document.createElement("option");
  create.value = "new";
  create.textContent = "New collection…";
  elements.batchCollection.append(create);
  elements.batchCollection.value = state.collections.some((set) => set.id === current)
    ? current
    : "";
  renderLibraryBatchBar();
}

function closeLibraryBatchNewForm() {
  elements.batchNew.hidden = true;
  elements.batchCollection.hidden = false;
  elements.batchCollection.value = "";
  renderLibraryBatchBar();
}

async function createLibraryBatchCollection() {
  const name = elements.batchNewName.value.trim();
  if (!name) return;
  // Double-submit guard (mirrors the Review bar): a second Enter while the first
  // invoke is pending would create a duplicate collection.
  if (elements.batchNewCreate.disabled) return;
  elements.batchNewCreate.disabled = true;
  try {
    const created = await invoke("collection_create", { name });
    elements.batchNew.hidden = true;
    elements.batchCollection.hidden = false;
    showMessage(`Created collection “${name}”.`);
    state.collections = await invoke("collection_list");
    renderLibraryCollectionOptions(created.id);
    elements.batchCollection.focus();
  } catch (error) {
    showMessage(crushErrorText(error), true);
  } finally {
    elements.batchNewCreate.disabled = false;
  }
}

// Photo members of the selection, in table order — the only assets the editorial
// ops can honestly act on.
function selectedPhotoOps(op, extra = {}) {
  return selectedAssets()
    .filter((asset) => asset.assetType === "photo")
    .map((asset) => ({ op, assetType: "photo", mediaId: asset.id, ...extra }));
}

async function runLibraryBatch(ops, summary) {
  try {
    const applied = await invoke("review_batch", { ops });
    showMessage(summary(applied));
    state.selectedIds.clear();
    state.anchorId = null;
    await refreshLibrary();
  } catch (error) {
    showMessage(crushErrorText(error), true);
    if (window.crushErrorText(error) !== String(error)) {
      elements.libraryMessage.append(crushCopyDetailsButton(String(error)));
    }
  }
}

async function refreshLibrary() {
  const [videos, jobs] = await Promise.all([invoke("list_videos"), invoke("job_status")]);
  state.videos = videos;
  state.jobs = jobs;
  renderVideos();
  managePolling();
  // A queued re-index starts its next asset from here (and from ingest-progress
  // events) once the previous ingest reaches a terminal state.
  pumpReindexQueue();
}

function managePolling() {
  if (isIngestActive() && !state.poll) {
    state.poll = setInterval(() => refreshLibrary().catch(() => {}), 850);
  } else if (!isIngestActive() && state.poll) {
    clearInterval(state.poll);
    state.poll = null;
  }
}

async function onIngestProgress(event) {
  state.jobs = event.payload;
  // Ingest reports moved/renamed outcomes honestly: the file was re-pointed to the
  // existing identity row after the same content was recognized at the new path. Each
  // finished job announces its summary exactly once — the backend keeps finished tasks
  // forever and re-emits them on every event, so without the guard the message would
  // re-fire on every poll for the rest of the session.
  for (const task of state.jobs.background) {
    if (task.kind === "ingest" && (task.status === "done" || task.status === "cancelled")) {
      announceIngestRelinks(task);
    }
  }
  try {
    state.videos = await invoke("list_videos");
  } catch {
    // Keep the prior table if the database is briefly busy during a stage transition.
  }
  renderVideos();
  managePolling();
  pumpReindexQueue();
}

function announceIngestRelinks(task) {
  // moved/renamed count only files whose old copy is really gone; a same-content file
  // whose old path still exists is a duplicate copy.
  const moved = (task.moved ?? 0) + (task.renamed ?? 0);
  const duplicated = task.duplicated ?? 0;
  if (moved === 0 && duplicated === 0) return;
  if (state.announcedIngestJobs.has(task.jobId)) return;
  // Bounded FIFO: only recent job ids matter for repeat suppression, and the set grows
  // by one per ingest, so evicting the oldest id at 200 keeps it negligible.
  if (state.announcedIngestJobs.size >= 200) {
    state.announcedIngestJobs.delete(state.announcedIngestJobs.values().next().value);
  }
  state.announcedIngestJobs.add(task.jobId);
  const parts = [];
  if (moved > 0) parts.push(`${moved} file${moved === 1 ? "" : "s"} moved or renamed`);
  if (duplicated > 0) {
    parts.push(`${duplicated} duplicate cop${duplicated === 1 ? "y" : "ies"} found`);
  }
  showMessage(`${parts.join(" and ")} — relinked to the original index by content.`);
}

async function addPath(path) {
  if (!path || isIngestActive()) return;
  try {
    const started = await invoke("add_folder", { path });
    showMessage(`Indexing started · job ${started.jobId.slice(0, 8)}`);
    await refreshLibrary();
  } catch (error) {
    showMessage(String(error), true);
  }
}

async function chooseFolder() {
  try {
    const path = await bridge.dialog.open({
      directory: true,
      multiple: false,
      title: "Add photo or video folder",
    });
    if (typeof path === "string") await addPath(path);
  } catch (error) {
    showMessage(`Folder picker failed: ${String(error)}`, true);
  }
}

async function cancelIngest() {
  try {
    const requested = await invoke("cancel_ingest");
    showMessage(requested ? "Cancelling after the current operation…" : "No ingest is running.");
  } catch (error) {
    showMessage(String(error), true);
  }
}

async function reindexSelected() {
  const ids = selectedAssets().map((asset) => asset.id);
  if (!ids.length || isIngestActive() || state.reindexQueue) return;
  // Single selection keeps today's flow: no confirm, one job, one message.
  if (ids.length === 1) {
    try {
      const started = await invoke("reindex_asset", { id: ids[0] });
      showMessage(`Re-index started · job ${started.jobId.slice(0, 8)}`);
      await refreshLibrary();
    } catch (error) {
      showMessage(crushErrorText(error), true);
    }
    return;
  }
  // Batch re-index asks once through the two-step armed-button pattern the rest of
  // the app uses (saved-search delete, safety apply), then runs the honest queue.
  if (!state.reindexArmed) {
    state.reindexArmed = true;
    renderVideos();
    state.reindexArmTimer = setTimeout(() => {
      state.reindexArmed = false;
      renderVideos();
    }, 6000);
    return;
  }
  disarmReindex();
  state.reindexQueue = ids;
  state.reindexTotal = ids.length;
  state.reindexFailed = 0;
  state.reindexSkipped = 0;
  state.reindexSkipReason = null;
  state.reindexCurrentId = null;
  state.reindexCurrentJobId = null;
  showMessage(`Re-indexing ${ids.length} assets one at a time — progress shows below.`);
  renderVideos();
  await pumpReindexQueue();
}

// The backend runs one ingest at a time (reindex_asset answers "ingest … is already
// running" otherwise), so a batch re-index is a frontend queue that starts the next
// asset only after the previous ingest reaches a terminal state. Every job reports
// its real progress through the existing ingest-progress events; nothing here fakes
// completion. Cancel detection matches the job id this batch started (review
// HIGH-1): the background snapshot keeps every task from the whole session, so
// matching by kind alone would read a stale task and both miss a real cancel and
// abort a fresh batch on an old one.
async function pumpReindexQueue() {
  if (!state.reindexQueue || state.reindexBusy) return;
  state.reindexBusy = true;
  try {
    while (state.reindexQueue && !isIngestActive()) {
      const currentTask = state.reindexCurrentJobId
        ? state.jobs.background.find(
            (task) => task.kind === "ingest" && task.jobId === state.reindexCurrentJobId,
          )
        : null;
      if (currentTask?.status === "cancelled") {
        // The in-flight asset was cancelled along with the never-started rest of
        // the queue — both count as not re-indexed.
        const remaining = state.reindexQueue.length + (state.reindexCurrentId ? 1 : 0);
        state.reindexQueue = null;
        state.reindexCurrentId = null;
        state.reindexCurrentJobId = null;
        showMessage(
          `Re-index stopped — ${remaining} asset${remaining === 1 ? "" : "s"} not re-indexed.`,
          true,
        );
        renderVideos();
        return;
      }
      if (state.reindexCurrentId) {
        // The ingest just finished: the refreshed row status is the real outcome.
        const asset = state.videos.find((video) => video.id === state.reindexCurrentId);
        if (asset?.status === "failed") state.reindexFailed += 1;
        state.reindexCurrentId = null;
        state.reindexCurrentJobId = null;
      }
      const nextId = state.reindexQueue.shift();
      if (!nextId) {
        const { reindexTotal, reindexFailed, reindexSkipped, reindexSkipReason } = state;
        state.reindexQueue = null;
        const notIndexed = reindexFailed + reindexSkipped;
        const notes = [
          reindexFailed
            ? `${reindexFailed} failed — see the failed rows for details`
            : null,
          reindexSkipped ? `${reindexSkipped} skipped — ${reindexSkipReason}` : null,
        ].filter(Boolean);
        showMessage(
          notIndexed
            ? `Re-indexed ${reindexTotal - notIndexed} of ${reindexTotal} assets · ${notes.join(" · ")}.`
            : `Re-indexed ${reindexTotal} asset${reindexTotal === 1 ? "" : "s"}.`,
          notIndexed > 0,
        );
        renderVideos();
        return;
      }
      state.reindexCurrentId = nextId;
      const done = state.reindexTotal - state.reindexQueue.length;
      showMessage(`Re-indexing ${done} of ${state.reindexTotal}…`);
      try {
        const started = await invoke("reindex_asset", { id: nextId });
        state.reindexCurrentJobId = started.jobId;
        // Reflects the now-active ingest (or, in tests/mock, its instant completion)
        // before the loop decides to start the next asset.
        await refreshLibrary();
      } catch (error) {
        if (/was not found/i.test(String(error))) {
          // The asset was removed (or vanished) mid-batch. Skip it and keep the
          // queue going — one stale id must not abort the remaining assets
          // (review LOW). The summary counts it with the mapped error, since a
          // skipped asset leaves no failed row to point at.
          state.reindexSkipped += 1;
          state.reindexSkipReason = state.reindexSkipReason || crushErrorText(error);
          state.reindexCurrentId = null;
          continue;
        }
        state.reindexQueue = null;
        state.reindexCurrentId = null;
        state.reindexCurrentJobId = null;
        showMessage(`Re-index stopped: ${crushErrorText(error)}`, true);
        renderVideos();
        return;
      }
    }
  } finally {
    state.reindexBusy = false;
  }
}

async function locateMovedFile(assetId) {
  // Relinking is per-asset: one file moves to one new path. The row-level button
  // passes its own id; the toolbar path passes nothing and resolves the single
  // selected asset (the toolbar button is disabled for any other selection).
  const id = assetId ?? (state.selectedIds.size === 1 ? [...state.selectedIds][0] : null);
  const asset = id ? state.videos.find((candidate) => candidate.id === id) : null;
  if (!asset || isIngestActive() || !asset.sourceMissing) return;
  try {
    const picked = await bridge.dialog.open({
      directory: false,
      multiple: false,
      title: "Locate the moved file",
    });
    if (typeof picked !== "string" || !picked) return;
    const outcome = await invoke("relink_asset", { id: asset.id, newPath: picked });
    showMessage(
      `The file moved. Crush verified the new copy is identical before relinking · ${outcome.newPath}`,
    );
    await refreshLibrary();
  } catch (error) {
    showMessage(String(error), true);
  }
}

function confirmRemove() {
  const assets = selectedAssets();
  if (!assets.length || isIngestActive()) return;
  state.pendingRemoveIds = assets.map((asset) => asset.id);
  if (assets.length === 1) {
    const selectedAsset = assets[0];
    elements.removeCopy.textContent =
      `Remove “${fileParts(selectedAsset.path).name}” from the Crush library?` +
      " The original file on disk is never touched. Crush forgets its index, previews, " +
      "analysis, choices and project references to it.";
  } else {
    elements.removeCopy.textContent =
      `Remove ${assets.length} assets from the library? Originals on disk are never touched. ` +
      "Crush forgets their index, previews, analysis, choices and project references to them.";
  }
  elements.removeDialog.showModal();
}

async function removeConfirmed() {
  const ids = state.pendingRemoveIds || [];
  state.pendingRemoveIds = null;
  elements.removeDialog.close();
  if (!ids.length) return;
  elements.removeConfirm.disabled = true;
  let removed = 0;
  let firstError = null;
  let removedKind = null;
  try {
    for (const id of ids) {
      try {
        const outcome = await invoke("remove_asset", { id });
        removed += 1;
        removedKind = removedKind || outcome.kind;
      } catch (error) {
        firstError = firstError || String(error);
      }
    }
    if (removed === ids.length) {
      showMessage(
        ids.length === 1
          ? `Removed the ${removedKind || "asset"} from your library. The original file was not changed.`
          : `Removed ${removed} assets from your library. The original files were not changed.`,
      );
    } else {
      showMessage(
        `Removed ${removed} of ${ids.length} — ${ids.length - removed} could not be removed: ${firstError}`,
        true,
      );
    }
    // Selection hygiene: renderVideos prunes ids that left the list, so removed
    // assets drop out while any that failed stay selected — the honest outcome.
    await refreshLibrary();
  } finally {
    elements.removeConfirm.disabled = false;
  }
}

async function installDragDrop() {
  const currentWindow = bridge.window.getCurrentWindow();
  await currentWindow.onDragDropEvent(async (event) => {
    if (event.payload.type === "over") {
      setVisible(elements.dropOverlay, true);
    } else if (event.payload.type === "drop") {
      setVisible(elements.dropOverlay, false);
      const paths = event.payload.paths || [];
      const [path] = paths;
      if (paths.length > 1) {
        showMessage(`Dropped ${paths.length} folders — indexing the first one only.`);
      }
      if (path) await addPath(path);
    } else {
      setVisible(elements.dropOverlay, false);
    }
  });
}

async function showLibrary() {
  setVisible(elements.boot, false);
  setVisible(elements.firstRun, false);
  setVisible(elements.appShell, true);
  try {
    await refreshLibrary();
  } catch (error) {
    // The shell is already visible, so render the empty state (or the last known table)
    // instead of leaving the user on a blank screen.
    showMessage(`Could not load the library: ${String(error)}`, true);
    renderVideos();
    managePolling();
  }
}

async function runDoctor() {
  elements.runDoctor.disabled = true;
  elements.runDoctor.textContent = "Running…";
  elements.doctorResult.textContent = "Checking local runtime…";
  try {
    elements.doctorResult.textContent = await invoke("doctor");
  } catch (error) {
    elements.doctorResult.textContent = `Doctor failed\n${String(error)}`;
  } finally {
    elements.runDoctor.disabled = false;
    elements.runDoctor.textContent = "Run Doctor";
  }
}

// Library-scoped keyboard (Task 039 C5): ⌘/Ctrl-A selects all listed assets, Esc
// clears the selection. Both are guarded to the Library view, to no open dialog, and
// to focus outside text controls — they never steal keys from the search field, the
// drawer, or a modal (search.js's global handler keeps its own Search-view scope).
function onLibraryKeydown(event) {
  if (elements.libraryView.hidden || document.querySelector("dialog[open]")) return;
  const target = event.target;
  const inTextControl = target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement;
  if (inTextControl) return;
  const meta = event.metaKey || event.ctrlKey;
  if (meta && event.key.toLowerCase() === "a" && state.videos.length) {
    event.preventDefault();
    window.getSelection()?.removeAllRanges();
    state.selectedIds = new Set(state.videos.map((video) => video.id));
    disarmReindex();
    renderVideos();
  } else if (event.key === "Escape" && state.selectedIds.size) {
    event.preventDefault();
    state.selectedIds.clear();
    state.anchorId = null;
    disarmReindex();
    renderVideos();
  }
}

function bindActions() {
  elements.retryModels.addEventListener("click", downloadModels);
  elements.continueModels.addEventListener("click", showLibrary);
  elements.addFolder.addEventListener("click", chooseFolder);
  elements.emptyAddFolder.addEventListener("click", chooseFolder);
  elements.cancel.addEventListener("click", cancelIngest);
  elements.reindex.addEventListener("click", reindexSelected);
  elements.locateAsset.addEventListener("click", () => locateMovedFile());
  elements.removeAsset.addEventListener("click", confirmRemove);
  elements.selectAll.addEventListener("change", () => {
    // Tri-state header checkbox: checked selects everything listed, unchecked clears;
    // the indeterminate middle state always resolves to clear-first for predictability.
    if (elements.selectAll.checked) {
      state.selectedIds = new Set(state.videos.map((video) => video.id));
    } else {
      state.selectedIds.clear();
    }
    state.anchorId = null;
    disarmReindex();
    renderVideos();
  });
  elements.batchPick.addEventListener("click", () => {
    const ops = selectedPhotoOps("pick");
    if (ops.length) runLibraryBatch(ops, (applied) => `Marked ${applied} as picks.`);
  });
  elements.batchReject.addEventListener("click", () => {
    const ops = selectedPhotoOps("reject");
    if (ops.length) runLibraryBatch(ops, (applied) => `Marked ${applied} as rejected.`);
  });
  elements.batchRating.addEventListener("change", () => {
    const rating = Number(elements.batchRating.value);
    elements.batchRating.value = "";
    const ops = rating ? selectedPhotoOps("rate", { rating }) : [];
    if (ops.length) runLibraryBatch(ops, (applied) => `Rated ${applied} photo${applied === 1 ? "" : "s"}.`);
  });
  elements.batchCollection.addEventListener("change", () => {
    const creating = elements.batchCollection.value === "new";
    elements.batchNew.hidden = !creating;
    elements.batchCollection.hidden = creating;
    if (creating) {
      elements.batchNewName.value = "";
      elements.batchNewName.focus();
    }
    renderLibraryBatchBar();
  });
  elements.batchNewCancel.addEventListener("click", closeLibraryBatchNewForm);
  elements.batchNewCreate.addEventListener("click", createLibraryBatchCollection);
  elements.batchNewName.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      createLibraryBatchCollection();
    } else if (event.key === "Escape") {
      // Cancel the inline form, not anything behind it — same precedence as Review.
      event.stopPropagation();
      closeLibraryBatchNewForm();
    }
  });
  elements.batchAdd.addEventListener("click", () => {
    const collectionId = elements.batchCollection.value;
    if (!collectionId || collectionId === "new") return;
    const ops = selectedPhotoOps("add_to_collection", { collectionId });
    if (ops.length) {
      runLibraryBatch(
        ops,
        (applied) => `Added ${applied} asset${applied === 1 ? "" : "s"} to the collection.`,
      );
    }
  });
  elements.batchClear.addEventListener("click", () => {
    state.selectedIds.clear();
    state.anchorId = null;
    disarmReindex();
    renderVideos();
  });
  document.addEventListener("keydown", onLibraryKeydown);
  elements.removeCancel.addEventListener("click", () => {
    state.pendingRemoveIds = null;
    elements.removeDialog.close();
  });
  elements.removeConfirm.addEventListener("click", removeConfirmed);
  elements.removeDialog.addEventListener("click", (event) => {
    if (event.target === elements.removeDialog) {
      state.pendingRemoveIds = null;
      elements.removeDialog.close();
    }
  });
  elements.doctorLink.addEventListener("click", () => elements.doctorDialog.showModal());
  elements.closeDoctor.addEventListener("click", () => elements.doctorDialog.close());
  elements.runDoctor.addEventListener("click", runDoctor);
  elements.doctorDialog.addEventListener("click", (event) => {
    if (event.target === elements.doctorDialog) elements.doctorDialog.close();
  });
}

async function initialize() {
  bindActions();
  if (!invoke) {
    elements.boot.querySelector("p").textContent = "Crush must be opened as a desktop app.";
    return;
  }
  await bridge.event.listen("download-progress", onDownloadProgress);
  await bridge.event.listen("ingest-progress", onIngestProgress);
  await installDragDrop();
  const ready = await refreshModels();
  setVisible(elements.boot, false);
  if (ready) {
    await showLibrary();
  } else {
    setVisible(elements.firstRun, true);
    await downloadModels();
  }
}

initialize().catch((error) => {
  setVisible(elements.boot, true);
  elements.boot.querySelector("p").textContent = `Could not open Crush: ${String(error)}`;
});
