const bridge = window.__TAURI__;
const invoke = bridge?.core?.invoke;

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
  cancel: document.querySelector("#cancel"),
  emptyLibrary: document.querySelector("#empty-library"),
  videoTableWrap: document.querySelector("#video-table-wrap"),
  videoRows: document.querySelector("#video-rows"),
  indexingStatus: document.querySelector("#indexing-status"),
  libraryMessage: document.querySelector("#library-message"),
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
  selectedVideoId: null,
  expandedVideoIds: new Set(),
  poll: null,
  messageTimer: null,
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
  const failedJob = state.jobs.pipeline.find(
    (job) => job.video_id === video.id && job.status === "failed",
  );
  if (!video.lastError && !failedJob) return null;
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

function renderVideos() {
  elements.videoRows.replaceChildren();
  setVisible(elements.emptyLibrary, state.videos.length === 0);
  setVisible(elements.videoTableWrap, state.videos.length > 0);

  if (state.selectedVideoId && !state.videos.some((video) => video.id === state.selectedVideoId)) {
    state.selectedVideoId = null;
  }
  const selectedAsset = state.videos.find((asset) => asset.id === state.selectedVideoId);
  elements.reindex.disabled = !selectedAsset || selectedAsset.assetType === "photo" || isIngestActive();

  for (const video of state.videos) {
    const presentation = videoPresentation(video);
    const details = errorDetails(video, presentation);
    const selected = state.selectedVideoId === video.id;
    const expanded = state.expandedVideoIds.has(video.id);
    const parts = fileParts(video.path);

    const row = document.createElement("tr");
    row.className = `video-row${selected ? " selected" : ""}`;
    row.dataset.videoId = video.id;
    row.tabIndex = 0;
    row.setAttribute("aria-selected", String(selected));
    row.addEventListener("click", () => selectVideo(video.id));
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectVideo(video.id);
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
}

function selectVideo(videoId) {
  state.selectedVideoId = state.selectedVideoId === videoId ? null : videoId;
  renderVideos();
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
  const selectedAsset = state.videos.find((asset) => asset.id === state.selectedVideoId);
  elements.reindex.disabled = !selectedAsset || selectedAsset.assetType === "photo" || active;
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

async function refreshLibrary() {
  const [videos, jobs] = await Promise.all([invoke("list_videos"), invoke("job_status")]);
  state.videos = videos;
  state.jobs = jobs;
  renderVideos();
  managePolling();
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
  try {
    state.videos = await invoke("list_videos");
  } catch {
    // Keep the prior table if the database is briefly busy during a stage transition.
  }
  renderVideos();
  managePolling();
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
  const selectedAsset = state.videos.find((asset) => asset.id === state.selectedVideoId);
  if (!selectedAsset || selectedAsset.assetType === "photo" || isIngestActive()) return;
  try {
    const started = await invoke("reindex_video", { id: state.selectedVideoId });
    showMessage(`Re-index started · job ${started.jobId.slice(0, 8)}`);
    await refreshLibrary();
  } catch (error) {
    showMessage(String(error), true);
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

function bindActions() {
  elements.retryModels.addEventListener("click", downloadModels);
  elements.continueModels.addEventListener("click", showLibrary);
  elements.addFolder.addEventListener("click", chooseFolder);
  elements.emptyAddFolder.addEventListener("click", chooseFolder);
  elements.cancel.addEventListener("click", cancelIngest);
  elements.reindex.addEventListener("click", reindexSelected);
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
