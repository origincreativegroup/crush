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
    styleView: $("#style-view"),
    reviewView: $("#review-view"),
    plansView: $("#plans-view"),
    navSearch: $("#nav-search"),
    navLibrary: $("#nav-library"),
    navStyle: $("#nav-style"),
    navReview: $("#nav-review"),
    navPlans: $("#nav-plans"),
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
    detailKind: $("#detail-kind"),
    detailClose: $("#detail-close"),
    detailFile: $("#detail-file"),
    video: $("#detail-video"),
    photo: $("#detail-photo"),
    playerHint: $("#player-hint"),
    playback: $("#detail-playback"),
    play: $("#detail-play"),
    goIn: $("#detail-go-in"),
    scrubber: $("#detail-scrubber"),
    position: $("#detail-position"),
    loop: $("#detail-loop"),
    timecodes: $("#detail-timecodes"),
    shotIndex: $("#detail-shot-index"),
    copy: $("#copy-timecodes"),
    exportClip: $("#export-clip"),
    reveal: $("#reveal-file"),
    prev: $("#prev-shot"),
    next: $("#next-shot"),
    transcript: $("#detail-transcript"),
    notesLabel: $("#detail-notes-label"),
    feedbackPick: $("#feedback-pick"),
    feedbackReject: $("#feedback-reject"),
    feedbackRating: $("#feedback-rating"),
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
  const signedPercent = (value) => `${value >= 0 ? "+" : ""}${Math.round(value * 100)}`;
  const shortTime = (seconds) => {
    const total = Math.max(0, Number(seconds) || 0);
    const minutes = Math.floor(total / 60);
    const remainder = Math.floor(total % 60);
    return `${minutes}:${pad(remainder)}`;
  };

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
    el.styleView.hidden = view !== "style";
    el.reviewView.hidden = view !== "review";
    el.plansView.hidden = view !== "plans";
    el.navSearch.classList.toggle("active", view === "search");
    el.navLibrary.classList.toggle("active", view === "library");
    el.navStyle.classList.toggle("active", view === "style");
    el.navReview.classList.toggle("active", view === "review");
    el.navPlans.classList.toggle("active", view === "plans");
    if (view === "search") {
      el.input.focus();
      el.input.select();
      refreshIndexedState();
    } else if (view === "review") {
      // library.js owns the review panel contents; it refreshes on this event. The shared
      // detail drawer stays usable from the review grid, so the detail is not closed here.
      document.dispatchEvent(new CustomEvent("crush:review-shown"));
    } else {
      closeDetail();
    }
    if (view === "style") {
      // style.js owns the panel contents; it refreshes on this event.
      document.dispatchEvent(new CustomEvent("crush:style-shown"));
    }
    if (view === "plans") document.dispatchEvent(new CustomEvent("crush:plans-shown"));
  }

  // style.js renders the "Add to style set" control in the detail drawer and needs the
  // current asset; a plain DOM event keeps the two modules decoupled (no shared state).
  function notifyDetailChanged() {
    const d = state.detail;
    document.dispatchEvent(new CustomEvent("crush:detail", {
      detail: d ? { kind: d.kind, id: d.id } : null,
    }));
  }

  async function refreshIndexedState() {
    try {
      const videos = await invoke("list_videos");
      state.hasIndexedShots = videos.some(
        (asset) => asset.shots > 0 || (asset.assetType === "photo" && asset.status === "done"),
      );
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

  function resultBreakdownRows(result) {
    const breakdown = result.score_breakdown || {};
    return [
      ["semantic match", breakdown.semantic],
      ["transcript match", breakdown.transcript_boost],
      ["general quality", breakdown.general_aesthetic],
      ["creative fit", breakdown.personal_affinity],
      ["context fit", breakdown.context_fit],
      ["safety penalty", breakdown.penalties],
      [breakdown.editorial < 0 ? "editorial penalty" : "editorial context", breakdown.editorial],
    ].filter(([, value]) => Number.isFinite(value) && Math.abs(value) >= 0.0001);
  }

  function breakdownSummary(result) {
    const headline = `Score ${displayScore(result.score)}`;
    const rows = resultBreakdownRows(result);
    if (!rows.length) return `${headline} · cosine ${Number(result.cosine).toFixed(3)}`;
    return `${headline}: ${rows.map(([label, value]) => `${label} ${signedPercent(value)}`).join(", ")}`;
  }

  function buildBreakdown(result) {
    const rows = resultBreakdownRows(result);
    const details = document.createElement("details");
    details.className = "result-breakdown";
    const summary = document.createElement("summary");
    summary.textContent = "Why this result?";
    const list = document.createElement("ul");
    for (const [label, value] of rows) {
      const item = document.createElement("li");
      item.textContent = `${label} ${signedPercent(value)}`;
      list.append(item);
    }
    details.append(summary, list);
    details.hidden = !rows.length;
    details.addEventListener("click", (event) => event.stopPropagation());
    return details;
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
      play.textContent = result.asset_type === "photo" ? "PHOTO" : "▶";
      const duration = document.createElement("span");
      duration.className = "badge badge-duration mono";
      duration.textContent = result.asset_type === "photo"
        ? (result.editorial_quality ? `★ ${result.editorial_quality}` : "STILL")
        : durationBadge(result.end_s - result.start_s);
      const score = document.createElement("span");
      score.className = "badge badge-score mono";
      score.textContent = String(displayScore(result.score));
      score.title = breakdownSummary(result);
      thumb.append(play, duration, score);

      const name = document.createElement("div");
      name.className = "file-name result-name";
      name.textContent = fileName(result.path);
      name.title = result.path;
      const transcript = document.createElement("div");
      transcript.className = "result-snippet";
      transcript.textContent = result.transcript_snippet || "";
      transcript.hidden = !result.transcript_snippet;
      const aesthetic = Number.isFinite(result.aesthetic_score)
        ? `Strong ${Math.round(result.aesthetic_score * 100)}`
        : "";
      const personal = Number.isFinite(result.personal_style_score)
        ? `Preference fit ${signedPercent(result.personal_style_score)}`
        : "";
      const styleLine = document.createElement("div");
      styleLine.className = "result-style";
      styleLine.textContent = [personal, aesthetic].filter(Boolean).join(" · ");
      styleLine.hidden = !styleLine.textContent;

      card.append(thumb, name, transcript, styleLine, buildBreakdown(result));
      card.addEventListener("click", () => {
        selectResult(index);
        openAssetDetail(result);
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
  async function openAssetDetail(result) {
    if (result.asset_type === "photo") return openPhotoDetail(result.asset_id);
    return openDetail(result.asset_id);
  }

  async function openDetail(shotId) {
    try {
      const detail = await invoke("shot_detail", { id: shotId });
      state.detail = { ...detail, kind: "video" };
      notifyDetailChanged();
      renderDetail();
    } catch (error) {
      showMessage(String(error), { error: true });
    }
  }

  async function openPhotoDetail(photoId) {
    try {
      const detail = await invoke("photo_detail", { id: photoId });
      state.detail = { ...detail, kind: "photo" };
      notifyDetailChanged();
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
    el.photo.removeAttribute("src");
    el.detail.hidden = true;
    el.shell.classList.remove("detail-open");
    state.detail = null;
    notifyDetailChanged();
    if (state.view === "search") el.input.focus();
  }

  function renderDetail() {
    const d = state.detail;
    el.detail.hidden = false;
    el.shell.classList.add("detail-open");
    el.detail.focus();
    const isPhoto = d.kind === "photo";
    el.detailKind.textContent = isPhoto ? "Photo detail" : "Shot detail";
    el.video.hidden = isPhoto;
    el.photo.hidden = !isPhoto;
    el.playerHint.hidden = isPhoto;
    el.playback.hidden = isPhoto;
    el.exportClip.hidden = isPhoto;
    el.prev.hidden = isPhoto;
    el.next.hidden = isPhoto;
    el.notesLabel.textContent = isPhoto ? "Editorial context" : "Transcript";
    el.copy.textContent = isPhoto ? "Copy path" : "Copy path + timecodes";
    if (isPhoto) {
      el.detailFile.textContent = fileName(d.photoPath);
      el.detailFile.title = d.photoPath;
      el.timecodes.textContent = `${d.width} × ${d.height} · ${d.format.toUpperCase()}`;
      const scores = [];
      if (d.quality) scores.push(`editorial ★ ${d.quality}`);
      if (Number.isFinite(d.aestheticScore)) scores.push(`strong shot ${Math.round(d.aestheticScore * 100)}`);
      if (Number.isFinite(d.technicalScore)) scores.push(`technical ${Math.round(d.technicalScore * 100)}`);
      if (Number.isFinite(d.compositionScore)) scores.push(`design ${Math.round(d.compositionScore * 100)}`);
      if (Number.isFinite(d.momentScore)) scores.push(`moment ${Math.round(d.momentScore * 100)}`);
      if (Number.isFinite(d.personalStyleScore)) scores.push(`preference fit ${signedPercent(d.personalStyleScore)}`);
      el.shotIndex.textContent = scores.join(" · ") || "Unreviewed";
      el.photo.src = fileSrc(d.photoPath);
      renderPhotoContext(d);
      return;
    }
    el.detailFile.textContent = fileName(d.videoPath);
    el.detailFile.title = d.videoPath;
    const length = Math.max(0, d.endS - d.startS);
    el.timecodes.textContent = `${timecode(d.startS, d.fps)} → ${timecode(d.endS, d.fps)}  (${length.toFixed(1)} s)`;
    const analysis = [];
    if (Number.isFinite(d.aestheticScore)) analysis.push(`strong shot ${Math.round(d.aestheticScore * 100)}`);
    if (Number.isFinite(d.technicalScore)) analysis.push(`technical ${Math.round(d.technicalScore * 100)}`);
    if (Number.isFinite(d.compositionScore)) analysis.push(`design ${Math.round(d.compositionScore * 100)}`);
    if (Number.isFinite(d.momentScore)) analysis.push(`moment ${Math.round(d.momentScore * 100)}`);
    if (Number.isFinite(d.personalStyleScore)) analysis.push(`preference fit ${signedPercent(d.personalStyleScore)}`);
    el.shotIndex.textContent = [`shot ${d.idx + 1} of ${d.shotCount}`, ...analysis].join(" · ");
    el.prev.disabled = d.idx <= 0;
    el.next.disabled = d.idx + 1 >= d.shotCount;
    renderTranscript(d.transcripts);
    if (d.analysisSummary) {
      const summary = document.createElement("p");
      summary.className = "transcript-text";
      summary.textContent = d.analysisSummary;
      el.transcript.prepend(summary);
    }

    const src = fileSrc(d.videoPath);
    if (el.video.dataset.src !== src) {
      el.video.dataset.src = src;
      el.video.src = src;
      el.video.load();
    }
    el.scrubber.max = String(length);
    el.scrubber.value = "0";
    el.position.textContent = `${shortTime(0)} / ${shortTime(length)}`;
    updatePlayButton();
    seekAndPlay();
  }

  function renderPhotoContext(detail) {
    el.transcript.replaceChildren();
    const values = [detail.analysisSummary, detail.description, detail.tags && `Tags: ${detail.tags}`, detail.notes].filter(Boolean);
    const text = document.createElement("p");
    text.className = values.length ? "transcript-text" : "transcript-empty";
    text.textContent = values.length ? values.join("\n") : "No analysis or editorial feedback yet.";
    el.transcript.append(text);
  }

  function seekAndPlay() {
    const d = state.detail;
    if (!d || d.kind !== "video") return;
    const start = () => {
      el.video.currentTime = d.startS;
      el.video.play().catch(() => {});
    };
    if (el.video.readyState >= 1) start();
    else el.video.addEventListener("loadedmetadata", start, { once: true });
  }

  function updatePlayButton() {
    el.play.textContent = el.video.paused ? "Play" : "Pause";
    el.play.setAttribute("aria-label", el.video.paused ? "Play clip" : "Pause clip");
  }

  function updatePlaybackPosition() {
    const d = state.detail;
    if (!d || d.kind !== "video") return;
    const length = Math.max(0, d.endS - d.startS);
    const relative = Math.max(0, Math.min(length, el.video.currentTime - d.startS));
    el.scrubber.value = String(relative);
    el.position.textContent = `${shortTime(relative)} / ${shortTime(length)}`;
  }

  function setLoop(loop) {
    state.loop = loop;
    el.loop.setAttribute("aria-pressed", String(loop));
    el.loop.textContent = loop ? "Loop on" : "Loop off";
  }

  function toggleDetailPlayback() {
    const d = state.detail;
    if (!d || d.kind !== "video") return;
    if (el.video.paused) {
      if (el.video.currentTime < d.startS || el.video.currentTime >= d.endS - 0.02) el.video.currentTime = d.startS;
      el.video.play().catch(() => {});
    } else el.video.pause();
  }

  el.video.addEventListener("timeupdate", () => {
    const d = state.detail;
    if (!d) return;
    if (el.video.currentTime < d.startS) el.video.currentTime = d.startS;
    if (el.video.currentTime >= d.endS - 0.02) {
      if (state.loop) {
        el.video.currentTime = d.startS;
      } else {
        el.video.pause();
        el.video.currentTime = Math.max(d.startS, d.endS - 0.04);
      }
    }
    updatePlaybackPosition();
  });
  el.video.addEventListener("play", updatePlayButton);
  el.video.addEventListener("pause", updatePlayButton);
  el.video.addEventListener("error", () => {
    showMessage(`Could not play ${fileName(state.detail?.videoPath || "")}. Is the drive mounted?`, { error: true });
  });
  el.photo.addEventListener("error", () => {
    showMessage(`Could not load ${fileName(state.detail?.photoPath || "")}. Is the drive mounted?`, { error: true });
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
    if (!d || d.kind !== "video") return;
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
    const text = d.kind === "photo"
      ? d.photoPath
      : `${d.videoPath}  ${timecode(d.startS, d.fps)} – ${timecode(d.endS, d.fps)}`;
    try {
      await bridge.clipboardManager.writeText(text);
      showMessage("Path and timecodes copied.");
    } catch (error) {
      showMessage(`Could not copy: ${String(error)}`, { error: true });
    }
  }

  async function exportClip() {
    const d = state.detail;
    if (!d || d.kind !== "video") return;
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
      await invoke("open_in_finder", { path: d.kind === "photo" ? d.photoPath : d.videoPath });
    } catch (error) {
      showMessage(String(error), { error: true });
    }
  }

  async function recordFeedback(signal, value = null) {
    const detail = state.detail;
    if (!detail) return;
    try {
      await invoke("record_feedback", {
        assetType: detail.kind,
        id: detail.id,
        signal,
        value,
        context: state.query,
      });
      showMessage(signal === "rating" ? `Rated ${value} of 5.` : signal === "pick" ? "Marked as a pick." : "Marked as rejected.");
    } catch (error) {
      showMessage(`Could not save feedback: ${String(error)}`, { error: true });
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
    const inInput = event.target instanceof HTMLInputElement
      || event.target instanceof HTMLTextAreaElement
      || event.target instanceof HTMLSelectElement;
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
      if (inInput) return;
      if (state.detail?.kind === "photo") return;
      if (event.key === "ArrowLeft") { event.preventDefault(); stepShot(-1); }
      else if (event.key === "ArrowRight") { event.preventDefault(); stepShot(1); }
      else if (event.key === " " && !inInput) {
        event.preventDefault();
        toggleDetailPlayback();
      } else if (event.key.toLowerCase() === "l" && !inInput) {
        setLoop(!state.loop);
      }
      return;
    }
    if (state.view !== "search") return;
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
        openAssetDetail(result);
      }
    }
  }

  // ---------- wiring ----------
  el.navSearch.addEventListener("click", () => showView("search"));
  el.navLibrary.addEventListener("click", () => showView("library"));
  el.navStyle.addEventListener("click", () => showView("style"));
  el.navReview.addEventListener("click", () => showView("review"));
  el.navPlans.addEventListener("click", () => showView("plans"));
  el.goLibrary.addEventListener("click", () => showView("library"));
  // library.js (review grid) opens the shared detail drawer through this event; a plain DOM
  // event keeps the two modules decoupled (same pattern as the style panel).
  document.addEventListener("crush:open-asset", (event) => {
    if (event.detail) openAssetDetail(event.detail);
  });
  el.input.addEventListener("input", scheduleSearch);
  el.input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !state.results.length) {
      clearTimeout(state.debounce);
      runSearch();
    }
  });
  el.top.addEventListener("change", () => state.query && runSearch());
  el.detailClose.addEventListener("click", closeDetail);
  el.play.addEventListener("click", toggleDetailPlayback);
  el.goIn.addEventListener("click", () => {
    if (state.detail?.kind !== "video") return;
    el.video.currentTime = state.detail.startS;
    updatePlaybackPosition();
  });
  el.scrubber.addEventListener("input", () => {
    if (state.detail?.kind !== "video") return;
    el.video.currentTime = state.detail.startS + Number(el.scrubber.value);
    updatePlaybackPosition();
  });
  el.loop.addEventListener("click", () => setLoop(!state.loop));
  el.copy.addEventListener("click", copyTimecodes);
  el.exportClip.addEventListener("click", exportClip);
  el.reveal.addEventListener("click", revealFile);
  el.feedbackPick.addEventListener("click", () => recordFeedback("pick", 1));
  el.feedbackReject.addEventListener("click", () => recordFeedback("reject", -1));
  el.feedbackRating.addEventListener("change", async () => {
    const value = Number(el.feedbackRating.value);
    if (!value) return;
    try {
      await recordFeedback("rating", value);
    } finally {
      // Reset to the placeholder so the same rating can be recorded twice in a row.
      el.feedbackRating.value = "";
    }
  });
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
