// Search + Shot detail (Task 12c). Loaded after app.js; shares the Tauri bridge and the
// #app-shell sidebar. Owns #search-view and #detail.

(() => {
  const bridge = window.__TAURI__;
  const invoke = bridge?.core?.invoke;
  if (!invoke) return;
  const fileSrc = (path) => bridge.core.convertFileSrc(path);

  const $ = (selector) => document.querySelector(selector);
  const el = {
    shell: $("#app-shell"),
    libraryView: $("#library-view"),
    searchView: $("#search-view"),
    navSearch: $("#nav-search"),
    navLibrary: $("#nav-library"),
    input: $("#search-input"),
    top: $("#top-select"),
    count: $("#result-count"),
    message: $("#search-message"),
    nothingIndexed: $("#search-nothing-indexed"),
    idle: $("#search-idle"),
    noMatches: $("#search-no-matches"),
    error: $("#search-error"),
    grid: $("#results-grid"),
    goLibrary: $("#go-library"),
    detail: $("#detail"),
    detailClose: $("#detail-close"),
    detailFile: $("#detail-file"),
    video: $("#detail-video"),
    timecodes: $("#detail-timecodes"),
    shotIndex: $("#detail-shot-index"),
    copy: $("#copy-timecodes"),
    exportClip: $("#export-clip"),
    reveal: $("#reveal-file"),
    prev: $("#prev-shot"),
    next: $("#next-shot"),
    transcript: $("#detail-transcript"),
  };

  const state = {
    view: "library",
    query: "",
    results: [],
    selected: -1,
    searching: false,
    searched: false,
    pendingQuery: null,
    debounce: null,
    messageTimer: null,
    detail: null,
    loop: false,
    hasIndexedShots: null,
  };

  // ---------- formatting ----------
  const pad = (value, width = 2) => String(value).padStart(width, "0");

  function timecode(seconds, fps) {
    const rate = Number.isFinite(fps) && fps > 0 ? fps : 25;
    const whole = Math.floor(seconds);
    const frames = Math.min(Math.round((seconds - whole) * rate), Math.ceil(rate) - 1);
    const h = Math.floor(whole / 3600);
    const m = Math.floor((whole % 3600) / 60);
    const s = whole % 60;
    return `${pad(h)}:${pad(m)}:${pad(s)}.${pad(frames)}`;
  }

  function durationBadge(seconds) {
    if (!Number.isFinite(seconds)) return "";
    if (seconds < 60) return `${seconds.toFixed(1)}s`;
    const m = Math.floor(seconds / 60);
    return `${m}:${pad(Math.round(seconds % 60))}`;
  }

  const fileName = (path) => path.split(/[\\/]/).at(-1) || path;
  const displayScore = (score) => Math.round(Math.min(1, Math.max(0, score)) * 100);

  function showMessage(text, { error = false, action = null } = {}) {
    clearTimeout(state.messageTimer);
    el.message.replaceChildren();
    const span = document.createElement("span");
    span.textContent = text;
    el.message.append(span);
    if (action) {
      const button = document.createElement("button");
      button.className = "button secondary small";
      button.type = "button";
      button.textContent = action.label;
      button.addEventListener("click", action.run);
      el.message.append(button);
    }
    el.message.classList.toggle("error", error);
    el.message.hidden = false;
    state.messageTimer = setTimeout(() => (el.message.hidden = true), action ? 12000 : 5000);
  }

  // ---------- view switching ----------
  function showView(view) {
    state.view = view;
    el.libraryView.hidden = view !== "library";
    el.searchView.hidden = view !== "search";
    el.navSearch.classList.toggle("active", view === "search");
    el.navLibrary.classList.toggle("active", view === "library");
    if (view === "search") {
      el.input.focus();
      el.input.select();
      refreshIndexedState();
    } else {
      closeDetail();
    }
  }

  async function refreshIndexedState() {
    try {
      const videos = await invoke("list_videos");
      state.hasIndexedShots = videos.some((video) => video.shots > 0);
    } catch {
      state.hasIndexedShots = true; // don't block searching on a transient store error
    }
    renderStates();
  }

  function renderStates() {
    const hasResults = state.results.length > 0;
    const nothing = state.hasIndexedShots === false;
    el.nothingIndexed.hidden = !nothing;
    el.idle.hidden = nothing || hasResults || state.query.length > 0;
    el.noMatches.hidden = nothing || hasResults || !state.query || !state.searched;
    el.grid.hidden = !hasResults;
    if (!hasResults) el.count.textContent = "";
  }

  // ---------- search ----------
  function scheduleSearch() {
    clearTimeout(state.debounce);
    state.debounce = setTimeout(runSearch, 160);
  }

  async function runSearch() {
    const query = el.input.value.trim();
    state.query = query;
    if (!query) {
      state.results = [];
      state.searched = false;
      renderResults();
      return;
    }
    if (state.searching) {
      state.pendingQuery = query;
      return;
    }
    state.searching = true;
    el.error.hidden = true;
    const started = performance.now();
    try {
      const results = await invoke("search", { q: query, top: Number(el.top.value) });
      if (el.input.value.trim() === query) {
        state.results = results;
        state.searched = true;
        state.selected = results.length ? Math.min(Math.max(state.selected, 0), results.length - 1) : -1;
        renderResults();
        const ms = Math.round(performance.now() - started);
        el.count.textContent = results.length
          ? `${results.length} result${results.length === 1 ? "" : "s"} · ${ms} ms`
          : "";
      }
    } catch (error) {
      el.error.textContent = String(error);
      el.error.hidden = false;
    } finally {
      state.searching = false;
      if (state.pendingQuery && state.pendingQuery !== query) {
        state.pendingQuery = null;
        runSearch();
      } else {
        state.pendingQuery = null;
      }
    }
  }

  function renderResults() {
    el.grid.replaceChildren();
    state.results.forEach((result, index) => {
      const card = document.createElement("div");
      card.className = "result-card";
      card.dataset.index = String(index);
      card.tabIndex = -1;
      card.setAttribute("role", "option");
      card.setAttribute("aria-selected", String(index === state.selected));
      if (index === state.selected) card.classList.add("selected");

      const thumb = document.createElement("div");
      thumb.className = "thumb-box";
      if (result.thumb_path) {
        const img = document.createElement("img");
        img.loading = "lazy";
        img.decoding = "async";
        img.alt = "";
        img.src = fileSrc(result.thumb_path);
        img.addEventListener("error", () => img.remove());
        thumb.append(img);
      }
      const play = document.createElement("span");
      play.className = "play-overlay";
      play.setAttribute("aria-hidden", "true");
      play.textContent = "▶";
      const duration = document.createElement("span");
      duration.className = "badge badge-duration mono";
      duration.textContent = durationBadge(result.end_s - result.start_s);
      const score = document.createElement("span");
      score.className = "badge badge-score mono";
      score.textContent = String(displayScore(result.score));
      score.title = `cosine ${result.cosine.toFixed(3)}`;
      thumb.append(play, duration, score);

      const name = document.createElement("div");
      name.className = "file-name result-name";
      name.textContent = fileName(result.video_path);
      name.title = result.video_path;
      const snippet = document.createElement("div");
      snippet.className = "result-snippet";
      snippet.textContent = result.transcript_snippet || "";
      snippet.hidden = !result.transcript_snippet;

      card.append(thumb, name, snippet);
      card.addEventListener("click", () => {
        selectResult(index);
        openDetail(result.shot_id);
      });
      el.grid.append(card);
    });
    renderStates();
  }

  function selectResult(index, { scroll = false } = {}) {
    if (!state.results.length) return;
    state.selected = Math.max(0, Math.min(index, state.results.length - 1));
    for (const card of el.grid.children) {
      const active = Number(card.dataset.index) === state.selected;
      card.classList.toggle("selected", active);
      card.setAttribute("aria-selected", String(active));
      if (active && scroll) card.scrollIntoView({ block: "nearest" });
    }
  }

  // ---------- detail ----------
  async function openDetail(shotId) {
    try {
      const detail = await invoke("shot_detail", { id: shotId });
      state.detail = detail;
      renderDetail();
    } catch (error) {
      showMessage(String(error), { error: true });
    }
  }

  function closeDetail() {
    if (el.detail.hidden) return;
    el.video.pause();
    el.video.removeAttribute("src");
    el.video.load();
    el.detail.hidden = true;
    state.detail = null;
    if (state.view === "search") el.input.focus();
  }

  function renderDetail() {
    const d = state.detail;
    el.detail.hidden = false;
    el.detail.focus();
    el.detailFile.textContent = fileName(d.videoPath);
    el.detailFile.title = d.videoPath;
    const length = Math.max(0, d.endS - d.startS);
    el.timecodes.textContent = `${timecode(d.startS, d.fps)} → ${timecode(d.endS, d.fps)}  (${length.toFixed(1)} s)`;
    el.shotIndex.textContent = `shot ${d.idx + 1} of ${d.shotCount}`;
    el.prev.disabled = d.idx <= 0;
    el.next.disabled = d.idx + 1 >= d.shotCount;
    renderTranscript(d.transcripts);

    const src = fileSrc(d.videoPath);
    if (el.video.dataset.src !== src) {
      el.video.dataset.src = src;
      el.video.src = src;
      el.video.load();
    }
    seekAndPlay();
  }

  function seekAndPlay() {
    const d = state.detail;
    if (!d) return;
    const start = () => {
      el.video.currentTime = d.startS;
      el.video.play().catch(() => {});
    };
    if (el.video.readyState >= 1) start();
    else el.video.addEventListener("loadedmetadata", start, { once: true });
  }

  el.video.addEventListener("timeupdate", () => {
    const d = state.detail;
    if (!d) return;
    if (el.video.currentTime >= d.endS - 0.02) {
      if (state.loop) {
        el.video.currentTime = d.startS;
      } else {
        el.video.pause();
        el.video.currentTime = Math.max(d.startS, d.endS - 0.04);
      }
    }
  });
  el.video.addEventListener("error", () => {
    showMessage(`Could not play ${fileName(state.detail?.videoPath || "")}. Is the drive mounted?`, { error: true });
  });

  function renderTranscript(segments) {
    el.transcript.replaceChildren();
    if (!segments.length) {
      const empty = document.createElement("p");
      empty.className = "transcript-empty";
      empty.textContent = "No speech in this shot.";
      el.transcript.append(empty);
      return;
    }
    const words = state.query.toLowerCase().split(/\s+/).filter((word) => word.length > 2);
    const pattern = words.length
      ? new RegExp(`(${words.map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi")
      : null;
    for (const segment of segments) {
      const row = document.createElement("div");
      row.className = "transcript-row";
      const time = document.createElement("span");
      time.className = "mono transcript-time";
      time.textContent = timecode(segment.startS, state.detail?.fps);
      const text = document.createElement("span");
      text.className = "transcript-text";
      if (pattern) {
        for (const part of segment.text.split(pattern)) {
          if (!part) continue;
          if (pattern.test(part) && words.includes(part.toLowerCase())) {
            const mark = document.createElement("mark");
            mark.textContent = part;
            text.append(mark);
          } else {
            text.append(part);
          }
          pattern.lastIndex = 0;
        }
      } else {
        text.textContent = segment.text;
      }
      row.append(time, text);
      el.transcript.append(row);
    }
  }

  async function stepShot(delta) {
    const d = state.detail;
    if (!d) return;
    const idx = d.idx + delta;
    if (idx < 0 || idx >= d.shotCount) return;
    try {
      const id = await invoke("shot_at_index", { videoId: d.videoId, idx });
      if (id) await openDetail(id);
    } catch (error) {
      showMessage(String(error), { error: true });
    }
  }

  async function copyTimecodes() {
    const d = state.detail;
    if (!d) return;
    const text = `${d.videoPath}  ${timecode(d.startS, d.fps)} – ${timecode(d.endS, d.fps)}`;
    try {
      await bridge.clipboardManager.writeText(text);
      showMessage("Path and timecodes copied.");
    } catch (error) {
      showMessage(`Could not copy: ${String(error)}`, { error: true });
    }
  }

  async function exportClip() {
    const d = state.detail;
    if (!d) return;
    const stem = fileName(d.videoPath).replace(/\.[^.]+$/, "");
    const defaultName = `${stem}_shot${pad(d.idx + 1, 3)}.mov`;
    try {
      const out = await bridge.dialog.save({
        title: "Export clip",
        defaultPath: defaultName,
        filters: [{ name: "QuickTime movie", extensions: ["mov"] }],
      });
      if (!out) return;
      el.exportClip.disabled = true;
      el.exportClip.textContent = "Exporting…";
      const exported = await invoke("export_clip", { id: d.id, out });
      showMessage(`Exported ${fileName(exported.path)} (${exported.mode.toLowerCase()})`, {
        action: { label: "Reveal", run: () => invoke("open_in_finder", { path: exported.path }).catch(() => {}) },
      });
    } catch (error) {
      showMessage(`Export failed: ${String(error)}`, { error: true });
    } finally {
      el.exportClip.disabled = false;
      el.exportClip.textContent = "Export clip…";
    }
  }

  async function revealFile() {
    const d = state.detail;
    if (!d) return;
    try {
      await invoke("open_in_finder", { path: d.videoPath });
    } catch (error) {
      showMessage(String(error), { error: true });
    }
  }

  // ---------- keyboard ----------
  function onKeydown(event) {
    const meta = event.metaKey || event.ctrlKey;
    if (meta && event.key.toLowerCase() === "f") {
      event.preventDefault();
      if (state.view !== "search") showView("search");
      el.input.focus();
      el.input.select();
      return;
    }
    if (state.view !== "search") return;
    const inInput = event.target === el.input;
    const detailOpen = !el.detail.hidden;

    if (event.key === "Escape") {
      event.preventDefault();
      if (detailOpen) closeDetail();
      else if (el.input.value) {
        el.input.value = "";
        runSearch();
      }
      return;
    }
    if (detailOpen) {
      if (event.key === "ArrowLeft") { event.preventDefault(); stepShot(-1); }
      else if (event.key === "ArrowRight") { event.preventDefault(); stepShot(1); }
      else if (event.key === " " && !inInput) {
        event.preventDefault();
        if (el.video.paused) el.video.play().catch(() => {}); else el.video.pause();
      } else if (event.key.toLowerCase() === "l" && !inInput) {
        state.loop = !state.loop;
        showMessage(state.loop ? "Loop on" : "Loop off");
      }
      return;
    }
    if (!state.results.length) return;
    const columns = 4;
    const moves = { ArrowDown: columns, ArrowUp: -columns, ArrowRight: 1, ArrowLeft: -1 };
    if (event.key in moves) {
      if (inInput && (event.key === "ArrowLeft" || event.key === "ArrowRight") && el.input.value) return;
      event.preventDefault();
      selectResult(state.selected < 0 ? 0 : state.selected + moves[event.key], { scroll: true });
    } else if (event.key === "Enter") {
      event.preventDefault();
      const result = state.results[state.selected < 0 ? 0 : state.selected];
      if (result) {
        selectResult(state.selected < 0 ? 0 : state.selected);
        openDetail(result.shot_id);
      }
    }
  }

  // ---------- wiring ----------
  el.navSearch.addEventListener("click", () => showView("search"));
  el.navLibrary.addEventListener("click", () => showView("library"));
  el.goLibrary.addEventListener("click", () => showView("library"));
  el.input.addEventListener("input", scheduleSearch);
  el.input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !state.results.length) {
      clearTimeout(state.debounce);
      runSearch();
    }
  });
  el.top.addEventListener("change", () => state.query && runSearch());
  el.detailClose.addEventListener("click", closeDetail);
  el.copy.addEventListener("click", copyTimecodes);
  el.exportClip.addEventListener("click", exportClip);
  el.reveal.addEventListener("click", revealFile);
  el.prev.addEventListener("click", () => stepShot(-1));
  el.next.addEventListener("click", () => stepShot(1));
  document.addEventListener("keydown", onKeydown);

  // Search is the launch view once the shell is visible (app.js shows it after model checks).
  bridge.event.listen("ingest-progress", () => {
    if (state.view === "search" && state.hasIndexedShots === false) refreshIndexedState();
  });
  const observer = new MutationObserver(() => {
    if (!el.shell.hidden) {
      observer.disconnect();
      showView("search");
    }
  });
  if (el.shell.hidden) observer.observe(el.shell, { attributes: true, attributeFilter: ["hidden"] });
  else showView("search");
})();
