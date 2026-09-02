// Mock Tauri bridge for the UI harness (crates/app/tests/ui-harness.html).
//
// scripts/ui-harness.mjs injects this file with addInitScript so it exists in every frame —
// including the app iframe that loads ../ui/index.html — before app.js and search.js parse.
// The app therefore sees a fully formed window.__TAURI__ and runs unmodified.
//
// The scenario name rides on the iframe URL (?scenario=...); without one this script is a
// no-op, so the real app is never affected. Scripted transitions (first-run retry, cancel
// completion) are driven by re-emitting the same events the Rust backend emits. No real
// media is loaded: photo previews are inline SVG data URIs and every path is fake. Errors
// are thrown as strings to mirror how Tauri delivers command errors to the UI.
(() => {
  "use strict";

  const scenario = new URLSearchParams(window.location.search).get("scenario");
  if (!scenario) return;

  // Task 034 review fix: observable playback transport. The harness loads no real media,
  // so the drawer's seek-then-play would otherwise be invisible (readyState stays at 0
  // and the loadedmetadata path never runs). Two prototype stubs make it assertable:
  //   1. readyState reports at least HAVE_METADATA (1), so the app takes its immediate
  //      seek branch instead of waiting for an event that never fires.
  //   2. currentTime assignments and play/pause calls are recorded on
  //      window.__mediaCalls for the harness to assert against.
  // Both are inert unless app code drives a media element (photo paths never do).
  window.__mediaCalls = [];
  const mediaCalls = window.__mediaCalls;
  const mediaProto = HTMLMediaElement.prototype;
  const currentTimeDescriptor = Object.getOwnPropertyDescriptor(mediaProto, "currentTime");
  if (currentTimeDescriptor?.set) {
    Object.defineProperty(mediaProto, "currentTime", {
      get() {
        return currentTimeDescriptor.get.call(this);
      },
      set(value) {
        mediaCalls.push({ name: "seek", time: Number(value) });
        currentTimeDescriptor.set.call(this, value);
      },
    });
  }
  const readyStateDescriptor = Object.getOwnPropertyDescriptor(mediaProto, "readyState");
  if (readyStateDescriptor?.get) {
    Object.defineProperty(mediaProto, "readyState", {
      get() {
        return Math.max(readyStateDescriptor.get.call(this), 1);
      },
    });
  }
  for (const method of ["play", "pause"]) {
    const original = mediaProto[method];
    mediaProto[method] = function (...args) {
      mediaCalls.push({ name: method, currentTime: this.currentTime });
      return original ? original.apply(this, args) : Promise.resolve();
    };
  }

  const photoPreview = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='1200' height='800'%3E%3Crect width='1200' height='800' fill='%2322304e'/%3E%3Ccircle cx='760' cy='360' r='180' fill='%23e6a648'/%3E%3Cpath d='M0 650 L420 280 L760 650 Z' fill='%234f8cff'/%3E%3C/svg%3E";
  // Task 040 (C7): a loadable stand-in for video posters — asset:// URLs do not resolve
  // in the harness (no Tauri protocol), so thumbs are inline SVGs like the photo preview.
  const thumbPreview = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='640' height='360'%3E%3Crect width='640' height='360' fill='%23141416'/%3E%3Ccircle cx='320' cy='180' r='60' fill='%234f8cff'/%3E%3C/svg%3E";

  const listeners = new Map();
  const calls = [];
  let modelsPresent = !scenario.startsWith("first-run");
  let downloadAttempts = 0;
  let relocateAttempts = 0;

  const modelFiles = [
    ["clip-image.onnx", 351000000],
    ["clip-text.onnx", 254000000],
    ["bpe_simple_vocab_16e6.txt.gz", 1300000],
    ["ggml-small.bin", 488000000],
  ];

  const video = {
    assetType: "video",
    id: "video-one",
    path: "/Volumes/Footage/Launch Day/rocket-launch.mov",
    durationS: 43.2,
    fps: 29.97,
    width: 3840,
    height: 2160,
    hasAudio: true,
    status: "done",
    indexedAt: "2026-08-28T12:00:00Z",
    shots: 12,
    lastError: null,
    sourceMissing: false,
    // Task 040 (C7): the video's poster — the first shot's thumb, resolved to an
    // absolute path by the real command. A video still indexing (no shots) carries
    // null and the Library keeps the placeholder.
    thumbPath: "/Users/editor/Library/Application Support/dev.crush.app/thumbs/rocket-launch-0000.jpg",
  };
  const photo = {
    assetType: "photo",
    id: "photo-one",
    path: "/Volumes/Photos/Campaign/select.jpg",
    durationS: null,
    fps: null,
    width: 6000,
    height: 4000,
    hasAudio: false,
    status: "done",
    indexedAt: "2026-08-28T12:02:00Z",
    shots: 0,
    lastError: null,
    sourceMissing: false,
    thumbPath: "/Volumes/Photos/Campaign/select.jpg",
  };
  const photoTwo = {
    ...photo,
    id: "photo-two",
    path: "/Volumes/Photos/Campaign/alt.jpg",
    indexedAt: "2026-08-28T12:03:00Z",
  };
  const photoThree = {
    ...photo,
    id: "photo-three",
    path: "/Volumes/Photos/Campaign/third.jpg",
    indexedAt: "2026-08-28T12:04:00Z",
  };

  const library = {
    empty: () => [],
    "first-run-retry": () => [],
    "ingest-cancel": () => [{ ...video, status: "split" }],
    "failed-row": () => [
      {
        ...video,
        status: "failed",
        lastError: "FFmpeg could not decode frame 218 near 00:00:07.08",
      },
    ],
    // Keep a stale projected error deliberately: the newest successful job is the
    // authoritative state even if a list refresh arrives out of order.
    "recovered-row": () => [{ ...video, lastError: "Older failure" }],
    "photo-row": () => [{ ...video }, { ...photo }],
    // Four rows so click / ⌘-click / Shift-click ranges and the all-photo boundary all
    // have room: two photos, one video, one more photo (Task 039 C5). The background
    // snapshot also pre-seeds a STALE cancelled ingest task (below) so the batch
    // re-index here proves cancel detection matches its own job id.
    "library-multiselect": () => [{ ...photo }, { ...photoTwo }, { ...video }, { ...photoThree }],
    // Two photos for the batch re-index queue scenarios: a real mid-batch cancel
    // (the mock surfaces the job as a running background ingest) and a stale asset
    // id that must be skipped, not abort the queue.
    "reindex-cancel": () => [{ ...photo }, { ...photoTwo }],
    "reindex-stale-asset": () => [{ ...photo }, { ...photoTwo }],
    // Two photos where the second remove fails (disk full): the partial-failure
    // message path, its B8 mapping, and the Copy-details clipboard fallback.
    "library-remove-partial": () => [{ ...photo }, { ...photoTwo }],
    "search-error": () => [{ ...video }],
    "dam-home": () => [{ ...video }, { ...photo }],
    feedback: () => [{ ...video }],
    "style-panel": () => [{ ...video }, { ...photo }],
    "style-add-item": () => [{ ...video }],
    // The selected video's file is gone from disk (drive remounted elsewhere): the
    // "Locate moved file…" affordance must be the way back in.
    "relocate-moved-file": () => [{ ...video, sourceMissing: true }],
    "ingest-relinked": () => [{ ...video }],
    "search-span-text": () => [{ ...video }],
    "search-kind-filter": () => [{ ...video }],
    // Task 040 (C7): a video WITH a poster (shot thumb resolved), a video still indexing
    // with no shots (honest null → placeholder), and a photo with its own thumb. Rows are
    // path-sorted like the real backend: rocket-launch, still-indexing, select.jpg.
    "library-thumbnails": () => [
      { ...video },
      { ...video, thumbPath: null, id: "video-no-shots", path: "/Volumes/Footage/Launch Day/still-indexing.mov", status: "split", shots: 0 },
      { ...photo },
    ],
  }[scenario]?.() ?? [];
  let libraryList = library;

  const pipeline = [];
  const background = [];
  if (scenario === "failed-row") {
    pipeline.push({
      id: "job-failed",
      owner_id: "local",
      video_id: "video-one",
      stage: "split",
      status: "failed",
      started_at: "2026-08-28T11:55:00Z",
      finished_at: "2026-08-28T11:55:10Z",
      duration_ms: 10000,
      error: "FFmpeg could not decode frame 218 near 00:00:07.08",
      debug_dir: "/tmp/crush-debug/job-failed",
    });
  }
  if (scenario === "recovered-row") {
    // The real store returns newest first. A successful retry must suppress the older
    // failure rather than leaving a stale expandable error under a Done row.
    pipeline.push(
      {
        id: "job-recovered",
        owner_id: "local",
        video_id: "video-one",
        stage: "transcribe",
        status: "done",
        started_at: "2026-08-28T12:05:00Z",
        finished_at: "2026-08-28T12:05:10Z",
        duration_ms: 10000,
        error: null,
        debug_dir: null,
      },
      {
        id: "job-older-failure",
        owner_id: "local",
        video_id: "video-one",
        stage: "split",
        status: "failed",
        started_at: "2026-08-28T11:55:00Z",
        finished_at: "2026-08-28T11:55:10Z",
        duration_ms: 10000,
        error: "FFmpeg could not decode frame 218 near 00:00:07.08",
        debug_dir: "/tmp/crush-debug/job-older-failure",
      },
    );
  }
  if (scenario === "ingest-cancel") {
    pipeline.push({
      id: "job-active",
      owner_id: "local",
      video_id: "video-one",
      stage: "embed",
      status: "running",
      started_at: "2026-08-28T12:05:00Z",
      finished_at: null,
      duration_ms: null,
      error: null,
      debug_dir: null,
    });
    background.push({
      jobId: "background-active",
      kind: "ingest",
      status: "running",
      detail: "indexing fixtures",
      error: null,
    });
  }
  if (scenario === "ingest-relinked") {
    // A finished ingest that recognized moved content: the backend keeps the counts as
    // structured fields so the Library can report "relinked by content" without parsing.
    background.push({
      jobId: "background-relinked",
      kind: "ingest",
      status: "done",
      detail:
        "discovered=1 photos=0 indexed=0 indexed_photos=0 skipped=1 failed=0 moved=1 renamed=0 duplicated=0 recovered=0 vectors=12",
      error: null,
      moved: 1,
      renamed: 0,
      duplicated: 0,
    });
  }
  if (scenario === "library-multiselect") {
    // A stale ingest task from earlier in the session (review HIGH-1): the backend
    // keeps every background task forever, keyed by job id, so old cancelled tasks
    // sit beside fresh ones in every job_status snapshot. Batch re-index cancel
    // detection must match the job id it started — this stale task must not abort
    // the fresh batch the scenario runs below.
    background.push({
      jobId: "background-stale-cancelled",
      kind: "ingest",
      status: "cancelled",
      detail: "ingest cancelled",
      error: null,
    });
  }

  const listen = async (name, handler) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(handler);
    return () => listeners.get(name)?.delete(handler);
  };

  const emit = (name, payload) => {
    for (const handler of listeners.get(name) || []) {
      queueMicrotask(() => handler({ payload }));
    }
  };

  const snapshot = () => ({
    background: background.map((task) => ({ ...task })),
    pipeline: pipeline.map((job) => ({ ...job })),
  });

  const modelsStatus = () =>
    modelFiles.map(([name, bytes]) => ({
      name,
      bytes,
      status: modelsPresent ? "present" : "missing",
    }));

  function modelsDownload() {
    downloadAttempts += 1;
    if (scenario === "first-run-retry" && downloadAttempts === 1) {
      emit("download-progress", {
        jobId: "models-test",
        name: null,
        downloaded: null,
        total: null,
        status: "failed",
        detail: null,
        error: "The network connection was interrupted.",
      });
      return { jobId: "models-test" };
    }
    emit("download-progress", {
      jobId: "models-test",
      name: "clip-image.onnx",
      downloaded: 182000000,
      total: 351000000,
      status: "running",
      detail: null,
      error: null,
    });
    modelsPresent = true;
    emit("download-progress", {
      jobId: "models-test",
      name: null,
      downloaded: null,
      total: null,
      status: "done",
      detail: "models ready",
      error: null,
    });
    return { jobId: "models-test" };
  }

  // Cancel flips the running background task and pipeline job to terminal states and
  // re-emits the snapshot, exactly like the backend's completion path.
  function cancelIngest() {
    for (const task of background) {
      if (task.status === "running") {
        task.status = "cancelled";
        task.detail = "ingest cancelled";
      }
    }
    for (const job of pipeline) {
      if (job.status === "running") {
        job.status = "cancelled";
        job.error = "Cancelled by user";
        job.finished_at = "2026-08-28T12:05:05Z";
      }
    }
    emit("ingest-progress", snapshot());
    return true;
  }

  // Task 040 review fix: the in-flight window must be observable. The harness arms this
  // gate (holdNextSearch), the next `search` invoke stays pending until releaseSearch()
  // resolves it with the results captured at invoke time, so a kind/Top-N click can land
  // while a search is genuinely in flight. Pure promise parking — no timers — so the
  // frozen clock cannot close the window. Inert unless armed.
  let heldSearch = null;
  const holdNextSearch = () => {
    heldSearch = {};
  };
  const releaseSearch = () => {
    const gate = heldSearch;
    heldSearch = null;
    gate?.resolve?.();
  };

  const searchResults = (q, kind = "all") => {
    if (scenario === "empty" || q === "zzz") return [];
    // Task 040 (C8): the mock honors the search command's kind argument the way the real
    // backend does — each kind returns only its own family, so the harness can assert the
    // server-side filter rather than a client-side post-filter.
    const matchesKind = (result) => kind === "all" || result.asset_type === kind;
    return [
      {
        asset_type: "video",
        asset_id: "shot-1",
        path: video.path,
        start_s: 3.2,
        end_s: 5.95,
        thumb_path: null,
        score: 0.4,
        cosine: 0.29,
        transcript_snippet: "…and then the rocket cleared the tower…",
        catalogue_snippet: null,
        editorial_quality: null,
        aesthetic_score: 0.81,
        personal_style_score: 0.42,
        score_breakdown: {
          semantic: 0.29,
          transcript_boost: 0.06,
          editorial: 0,
          general_aesthetic: 0,
          penalties: 0,
          personal_affinity: 0.05,
          context_fit: 0,
          total: 0,
          text_match_only: false,
        },
        provenance: null,
      },
      ...(scenario === "plans-sequence"
        ? [{
            asset_type: "video",
            asset_id: "shot-2",
            path: video.path,
            start_s: 3.2,
            end_s: 5.95,
            thumb_path: null,
            score: 0.38,
            cosine: 0.27,
            transcript_snippet: null,
            catalogue_snippet: null,
            editorial_quality: null,
            aesthetic_score: 0.79,
            personal_style_score: 0.41,
            score_breakdown: null,
            provenance: null,
          }]
        : []),
      // Task 034: an imported-clip text-match result — no thumbnail, no cosine, the
      // matched catalogue text plus verbatim provenance, and the text-match-only marker.
      // Task 040: also present for the search-kind-filter scenario so the span kind
      // filter has a span to return.
      ...(scenario === "search-span-text" || scenario === "search-kind-filter"
        ? [{
            asset_type: "span",
            asset_id: "span-search-1",
            path: video.path,
            start_s: 3.2,
            end_s: 5.95,
            thumb_path: null,
            score: 0,
            cosine: 0,
            transcript_snippet: null,
            catalogue_snippet: "rocket clear of the tower, crowd cheering",
            editorial_quality: null,
            aesthetic_score: null,
            personal_style_score: null,
            score_breakdown: {
              semantic: 0,
              transcript_boost: 0,
              editorial: 0,
              general_aesthetic: 0,
              penalties: 0,
              personal_affinity: 0,
              context_fit: 0,
              total: 0,
              text_match_only: true,
            },
            provenance: {
              source: "reel_studio",
              external_id: "V1-0001_S1",
              import_id: "import-1",
              imported_at: "2026-08-30T12:00:00Z",
            },
          }]
        : []),
      {
        asset_type: "photo",
        asset_id: "photo-0",
        path: photo.path,
        start_s: null,
        end_s: null,
        thumb_path: photo.path,
        score: 0.35,
        cosine: 0.24,
        transcript_snippet: null,
        catalogue_snippet: null,
        editorial_quality: 5,
        aesthetic_score: 0.89,
        personal_style_score: 0.42,
        score_breakdown: {
          semantic: 0.24,
          transcript_boost: 0,
          editorial: 0,
          general_aesthetic: 0.06,
          penalties: 0,
          personal_affinity: 0.05,
          context_fit: 0,
          total: 0,
          text_match_only: false,
        },
        provenance: null,
      },
    ].filter(matchesKind);
  };

  const shotDetail = (id) => {
    const idx = Number(String(id).split("-")[1]) || 0;
    return {
      id,
      videoId: "video-one",
      videoPath: video.path,
      idx,
      shotCount: 12,
      startS: idx * 3.2,
      endS: idx * 3.2 + 2.75,
      repFrameS: idx * 3.2 + 1,
      fps: 29.97,
      thumbPath: null,
      aestheticScore: 0.81,
      personalStyleScore: idx % 2 ? -0.18 : 0.42,
      technicalScore: 0.76,
      compositionScore: 0.84,
      momentScore: 0.86,
      analysisSummary:
        "General strong-shot score 81/100 from separate technical, design, and moment evidence.",
      transcripts: idx % 2
        ? [{
            id: "seg",
            startS: idx * 3.2,
            endS: idx * 3.2 + 2,
            text: "and then the rocket cleared the tower",
            confidence: 0.9,
          }]
        : [],
    };
  };

  const photoDetail = (id) => ({
    id,
    photoPath: photo.path,
    width: 6000,
    height: 4000,
    format: "jpeg",
    quality: 5,
    aestheticScore: 0.89,
    personalStyleScore: 0.42,
    technicalScore: 0.86,
    compositionScore: 0.92,
    momentScore: 0.75,
    analysisSummary:
      "General strong-shot score 89/100 from separate technical, design, and moment evidence.",
    description: "Warm geometric portrait with deliberate negative space.",
    tags: "warm, geometric, campaign",
    notes: "Prefer the asymmetry.",
  });

  // Task 034 fixtures: one imported clip for the drawer (read-only catalogue evidence) and
  // the Preferences evidence population. `sets`/`confirmed` are derived from styleState so
  // the confirm → confirm-set → disable lifecycle is observable end to end.
  const spanDetailFixture = (id) => ({
    id,
    videoId: "video-one",
    videoPath: video.path,
    startS: 3.2,
    endS: 5.95,
    fps: 29.97,
    source: "reel_studio",
    externalId: "V1-0001_S1",
    importId: "import-1",
    importedAt: "2026-08-30T12:00:00Z",
    boundaryBasis: "catalogue_tc",
    boundaryToleranceS: 1.0,
    description: "Rocket clear of the tower, wide shot",
    subjects: "rocket",
    action: "launching",
    tags: "exhibit",
    shotType: "wide",
    cameraMove: "static",
    notes: "",
    quality: 4,
    standout: true,
    usable: true,
    usedIn: "reel-01",
  });

  const evidenceSpans = scenario === "preferences-span-evidence"
    ? [
        {
          spanId: "span-ev-1",
          externalId: "V1-0001_S1",
          source: "reel_studio",
          importId: "import-1",
          videoPath: video.path,
          startS: 3.2,
          endS: 5.95,
          description: "Rocket clear of the tower",
          quality: 4,
          standout: true,
          usedIn: "reel-01",
          importedAt: "2026-08-30T12:00:00Z",
          sets: [],
          confirmed: false,
        },
        {
          spanId: "span-ev-2",
          externalId: "V1-0001_S5",
          source: "reel_studio",
          importId: "import-1",
          videoPath: video.path,
          startS: 20.0,
          endS: 24.0,
          description: "Crowd cheering",
          quality: null,
          standout: false,
          usedIn: "",
          importedAt: "2026-08-30T12:00:10Z",
          sets: [],
          confirmed: false,
        },
      ]
    : [];

  // Task 018b style mock state: one confirmed set (feeding the "learned" profile) and one
  // unconfirmed set (inert until confirmed). style_profile_status reports the Task 018a
  // eval-gate metrics verbatim; style_profile_reset flips the profile to the general model.
  // style-not-learned serves the other gate outcome: the eval ran but the profile did not
  // pass (personal 0.45 vs baseline 0.61), so the UI must keep the experimental copy.
  const styleState = {
    sets: ["style-panel", "style-add-item", "style-not-learned"].includes(scenario)
      ? [
          {
            id: "set-confirmed",
            name: "Launch selects",
            contextKey: "default",
            description: "Strongest launch coverage",
            scope: "whole_set",
            status: "confirmed",
            itemCount: 4,
            createdAt: "2026-08-28T09:00:00Z",
            confirmedAt: "2026-08-28T09:30:00Z",
          },
          {
            id: "set-unconfirmed",
            name: "Quiet travel film",
            contextKey: "homepage-hero",
            description: "",
            scope: "selected",
            status: "unconfirmed",
            itemCount: 2,
            createdAt: "2026-08-28T10:00:00Z",
            confirmedAt: null,
          },
        ]
      : [],
    profileReset: false,
  };

  const styleProfileStatus = () => {
    const confirmed = styleState.sets.filter((set) => set.status === "confirmed").length;
    const base = {
      hasActiveProfile: false,
      learned: false,
      profileId: null,
      contextKey: null,
      version: null,
      algorithmVersion: null,
      sampleCount: null,
      heldOutMetric: null,
      baselineMetric: null,
      metrics: null,
      referenceSetsTotal: styleState.sets.length,
      referenceSetsConfirmed: confirmed,
    };
    if (styleState.profileReset) return base;
    const learned = scenario !== "style-not-learned";
    return {
      ...base,
      hasActiveProfile: true,
      learned,
      profileId: "profile-demo",
      contextKey: "default",
      version: 3,
      algorithmVersion: "personal-residual-v1",
      sampleCount: 12,
      heldOutMetric: learned ? 0.78 : 0.45,
      baselineMetric: 0.61,
      metrics: {
        held_out_pairs: 6,
        personal_accuracy: learned ? 0.78 : 0.45,
        baseline_accuracy: 0.61,
        learned,
        split: "media-disjoint-every-3rd",
        trainer: "personal-residual-v1",
      },
    };
  };

  const findStyleSet = (setId) => {
    const set = styleState.sets.find((candidate) => candidate.id === setId);
    if (!set) throw `No reference set ${setId}`;
    return set;
  };

  // Task 019b mock state: two photos + one shot, one collection, one version stack, and one
  // saved search. Rows mirror LibraryAssetView's camelCase projection (thumbPath is already
  // resolved to an absolute path by the real command). Flags are annotation defaults except
  // where set: photo-two carries the flagged profile (unusable + blur required + faces).
  const reviewScenario = [
    "library-grid",
    "dam-home",
    "library-bulk",
    "library-collections",
    "library-feedback",
    "library-flags",
    "library-multiselect",
    "library-saved-search",
    "compare-view",
    "compare-advance",
    "compare-advance-reduced",
    // Task 034: Review now lists imported clips beside photos and shots; the search-span
    // scenario also needs a genuinely indexed browse pool behind the DAM browser.
    "review-spans",
    "search-span-text",
    // Task 040: the kind-filter scenario needs the same indexed browse pool behind the
    // DAM browser (refreshBrowse clears hasIndexedShots on an empty pool otherwise).
    "search-kind-filter",
  ].includes(scenario);

  const reviewAssets = reviewScenario
    ? [
        {
          mediaKind: "photo",
          mediaId: "photo-one",
          path: photo.path,
          thumbPath: photo.path,
          status: "done",
          indexedAt: "2026-08-28T12:02:00Z",
          videoId: null,
          startS: null,
          endS: null,
          width: 6000,
          height: 4000,
          quality: 5,
          usable: true,
          standout: true,
          facesVisible: false,
          nametagsVisible: false,
          blurRequired: false,
          tags: "warm, geometric",
          // The collections scenario starts with no memberships so the create-and-add
          // flow is observable end to end.
          collectionIds: scenario === "library-collections" ? [] : ["col-one"],
          stackIds: [],
        },
        {
          mediaKind: "photo",
          mediaId: "photo-two",
          path: "/Volumes/Photos/Campaign/alt.jpg",
          thumbPath: "/Volumes/Photos/Campaign/alt.jpg",
          status: "done",
          indexedAt: "2026-08-28T12:03:00Z",
          videoId: null,
          startS: null,
          endS: null,
          width: 6000,
          height: 4000,
          quality: null,
          usable: false,
          standout: false,
          facesVisible: true,
          nametagsVisible: false,
          blurRequired: true,
          tags: "",
          collectionIds: [],
          stackIds: ["stk-one"],
        },
        {
          mediaKind: "shot",
          mediaId: "shot-1",
          path: video.path,
          thumbPath: null,
          status: "done",
          indexedAt: "2026-08-28T12:00:00Z",
          videoId: "video-one",
          startS: 3.2,
          endS: 5.95,
          width: 3840,
          height: 2160,
          quality: 4,
          usable: true,
          standout: false,
          facesVisible: false,
          nametagsVisible: false,
          blurRequired: false,
          tags: "",
          collectionIds: [],
          stackIds: [],
        },
        // Task 034: an imported clip in the Review grid — catalogue provenance, no
        // thumbnail (and none may be fabricated), and no batch editorial checkbox.
        ...(scenario === "review-spans"
          ? [{
              mediaKind: "span",
              mediaId: "span-rev-1",
              path: video.path,
              thumbPath: null,
              status: "done",
              indexedAt: "2026-08-28T12:00:00Z",
              videoId: "video-one",
              startS: 3.2,
              endS: 5.95,
              width: 3840,
              height: 2160,
              quality: 4,
              usable: true,
              standout: true,
              facesVisible: false,
              nametagsVisible: false,
              blurRequired: false,
              tags: "water,exhibit",
              collectionIds: [],
              stackIds: [],
              source: "reel_studio",
              externalId: "V1-0001_S1",
              importId: "import-1",
              importedAt: "2026-08-30T12:00:00Z",
            }]
          : []),
      ]
    : [];

  // The collections scenario starts with zero collections so the batch bar's honest
  // empty state ("No collections yet…") is asserted before anything is created.
  const reviewCollections = reviewScenario
    ? (scenario === "library-collections"
      ? []
      : [{ id: "col-one", name: "Launch heroes", description: "", createdAt: "2026-08-28T13:00:00Z" }])
    : [];
  const reviewCollectionItems = [];
  const reviewStacks = reviewScenario
    ? [{ id: "stk-one", name: "Launch cut v1", createdAt: "2026-08-28T13:30:00Z" }]
    : [];
  const reviewSavedSearches = reviewScenario
    ? [{
        id: "ss-one",
        name: "Blur review",
        query: "",
        contextKey: "default",
        filtersJson: JSON.stringify({ blurRequired: true }),
        createdAt: "2026-08-28T14:00:00Z",
      }]
    : [];
  const annotationEdits = new Map();
  // Latest editorial outcome per media id, keyed by `photo|<id>` / `shot|<id>`. The mock's
  // review_batch and record_feedback settle these the way the real store appends feedback
  // events, so the new feedback filter has an observable, stateful target.
  const feedbackState = new Map();

  const findReviewAsset = (assetType, mediaId) => {
    const kind = assetType === "photo" ? "photo" : "shot";
    const asset = reviewAssets.find(
      (candidate) => candidate.mediaKind === kind && candidate.mediaId === mediaId,
    );
    if (!asset) throw `No ${kind} ${mediaId}`;
    return asset;
  };

  const reviewFeedback = (asset) =>
    feedbackState.get(`${asset.mediaKind}|${asset.mediaId}`) || null;

  const reviewBrowse = (filter = {}) =>
    reviewAssets
      .filter((asset) => {
        if (filter.kind && asset.mediaKind !== filter.kind) {
          return false;
        }
        if (filter.status && asset.status !== filter.status) return false;
        if (filter.usable !== undefined && filter.usable !== null && asset.usable !== filter.usable) {
          return false;
        }
        if (
          filter.blurRequired !== undefined
          && filter.blurRequired !== null
          && asset.blurRequired !== filter.blurRequired
        ) {
          return false;
        }
        if (filter.feedback && reviewFeedback(asset) !== filter.feedback) return false;
        if (filter.qualityMin != null && (asset.quality == null || asset.quality < filter.qualityMin)) {
          return false;
        }
        if (filter.collectionId && !asset.collectionIds.includes(filter.collectionId)) return false;
        if (filter.stackId && !asset.stackIds.includes(filter.stackId)) return false;
        if (filter.contextKey) return false; // the mock has no collection item contexts
        if (filter.search && !asset.path.toLowerCase().includes(String(filter.search).toLowerCase())) {
          return false;
        }
        return true;
      })
      .map((asset) => ({ ...asset, collectionIds: [...asset.collectionIds], stackIds: [...asset.stackIds] }));

  const reviewAnnotation = (assetType, mediaId) => {
    const asset = findReviewAsset(assetType, mediaId);
    const edits = annotationEdits.get(mediaId) || {};
    return {
      description: edits.description ?? (asset.mediaKind === "photo" && asset.mediaId === "photo-one"
        ? "Warm geometric portrait with deliberate negative space."
        : ""),
      subjects: edits.subjects ?? "",
      action: edits.action ?? "",
      tags: edits.tags ?? asset.tags,
      notes: edits.notes ?? "",
      standout: edits.standout ?? asset.standout ?? false,
      usable: edits.usable ?? asset.usable,
      facesVisible: edits.facesVisible ?? asset.facesVisible,
      nametagsVisible: edits.nametagsVisible ?? asset.nametagsVisible,
      blurRequired: edits.blurRequired ?? asset.blurRequired,
    };
  };

  // Stateful plan mocks mirror the real bridge DTOs and reject invalid boundaries. Calls
  // remain observable so tests can prove edits never secretly append feedback.
  const plans = new Map();
  let nextPlan = 1;
  let failPlanSave = scenario === "plans-errors";
  let failCandidates = scenario === "plans-errors";
  const clone = (value) => JSON.parse(JSON.stringify(value));
  const planFor = (id) => { const plan = plans.get(id); if (!plan) throw "Plan not found"; return plan; };
  const planView = (plan) => {
    const { items, revisions, ...view } = plan;
    return { ...view, itemCount: items.length };
  };
  const planKind = (assetType) => assetType === "photo" ? "photo" : assetType === "span" ? "span" : "shot";
  if (["plans-historical", "plans-span-export"].includes(scenario)) {
    plans.set("plan-hist", {
      id: "plan-hist", name: "Reel Studio · Healthy Earth", contextKey: "default",
      description: "Imported from Reel Studio recipe healthy-earth.json (historical; recipe reel-studio-healthy-earth v1)", brief: "",
      items: [{
        mediaKind: "span", mediaId: "span-hist-1", position: 0, startS: 3.45, endS: 4.45, pacing: null, cropX: null, gradeJson: null,
        reason: "Reel Studio segment V1-0001_S1 (historical choice)",
        signalsJson: JSON.stringify({ candidate: { kind: "span", path: video.path, start_s: 3.2, end_s: 5.95, boundary_basis: "catalogue_tc", boundary_tolerance_s: 1.0 }, historical: { source: "reel_studio", external_id: "V1-0001_S1", used_in: "reel-01" } }),
        origin: "historical", rank: null, profileVersion: null,
        provenanceJson: JSON.stringify({ source: "reel_studio", external_id: "V1-0001_S1", boundary_basis: "catalogue_tc", boundary_tolerance_s: 1.0, imported_start_s: 3.45, imported_end_s: 4.45 }),
        addedAt: "2026-08-30T12:00:00Z",
        // Task 037: span items are adjustable clips — the editable range is the source
        // video, not the imported span (3.2..5.95), which is only the item's default.
        sourceRange: { startS: 0, endS: video.durationS },
      }],
      revisions: [{ revision: 1, label: "imported from Reel Studio", createdAt: "2026-08-30T12:00:00Z" }],
      createdAt: "2026-08-30T12:00:00Z", updatedAt: "2026-08-30T12:00:00Z",
    });
  }
  const planItemFor = (plan, args) => {
    const item = plan.items.find((item) => item.mediaId === args.mediaId && item.mediaKind === planKind(args.assetType));
    if (!item) throw "Plan item not found";
    return item;
  };
  // Task 033 sequence mock: two adjacent shots read as near-identical (the mock has no
  // embeddings), spans group by their provenance external id. Snake_case matches the bridge.
  const sequenceReportFor = (plan) => {
    const transitions = plan.items.slice(0, -1).map((item, index) => {
      const next = plan.items[index + 1];
      const similarity = item.mediaKind === "shot" && next.mediaKind === "shot" ? 0.97 : null;
      const nearDuplicate = similarity != null && similarity >= 0.95;
      const spanSource = (value) => { try { return JSON.parse(value.provenanceJson || "{}").external_id; } catch { return undefined; } };
      const sameSource = item.mediaKind === "span" && next.mediaKind === "span" && spanSource(item) === spanSource(next);
      return {
        position: index,
        similarity,
        near_duplicate: nearDuplicate,
        same_source: sameSource,
        note: nearDuplicate
          ? "These two neighbors look near-identical (cosine 0.97)."
          : sameSource ? "Two items in a row come from the same source (imported catalogue)." : "",
      };
    });
    const distinctSources = plan.items.length;
    return {
      items: plan.items.map((item, index) => ({
        position: index,
        media_kind: item.mediaKind,
        media_id: item.mediaId,
        neighbor_similarity: [transitions[index - 1], transitions[index]].map((value) => value?.similarity).find((value) => value != null) ?? null,
        notes: [["previous", transitions[index - 1]], ["next", transitions[index]]]
          .filter(([, value]) => value?.near_duplicate)
          .map(([side]) => `Looks near-identical to the ${side} item.`),
      })),
      transitions,
      summary: {
        item_count: plan.items.length,
        distinct_sources: distinctSources,
        sources: {},
        coverage_note: `${plan.items.length} items from ${distinctSources} distinct sources; the busiest source contributes 1 item.`,
        pacing_note: plan.items.some((item) => item.mediaKind !== "photo") ? "Video item durations run 1.0s to 1.0s (median 1.0s)." : "",
        near_duplicate_adjacencies: transitions.filter((value) => value.near_duplicate).length,
      },
    };
  };
  const validatePlanItem = (item) => {
    // Task 037: span items clamp to the source video range (sourceRange), not the imported
    // span; shots stay inside their source shot (the mock's stand-in 3.2..5.95).
    if (item.mediaKind === "shot" && !(item.startS >= 3.2 && item.endS <= 5.95 && item.endS > item.startS)) throw "Clip must stay inside source shot";
    if (item.mediaKind === "span") {
      const range = item.sourceRange || { startS: 0, endS: video.durationS };
      if (!(item.startS >= range.startS && item.endS <= range.endS && item.endS > item.startS)) throw "Clip must stay inside the source video";
    }
    if ((item.origin === "historical" || item.origin === "imported") && item.profileVersion != null) throw "Invalid provenance";
    if ((item.origin === "personal") !== (item.profileVersion != null)) throw "Invalid provenance";
  };

  async function invoke(command, args = {}) {
    calls.push({ command, args });
    switch (command) {
      case "selects_candidates": {
        if (failCandidates) { failCandidates = false; throw "Candidate lookup unavailable"; }
        const profile = args.brief && scenario !== "plans-general" ? { id: "profile-demo", version: 3, context_key: args.context, algorithm_version: "personal-residual-v1" } : null;
        const general = searchResults("plans").map((asset) => ({ ...asset, aesthetic_score: asset.asset_type === "video" ? 0.93 : 0.89, score: asset.asset_type === "video" ? 0.93 : 0.89, score_breakdown: null }));
        const personalized = args.brief ? searchResults("plans").reverse().map((asset) => {
          const breakdown = { ...asset.score_breakdown, semantic: asset.asset_type === "photo" ? 0.44 : 0.29, personal_affinity: profile ? 0.05 : 0, context_fit: 0 };
          breakdown.total = Object.entries(breakdown).filter(([key]) => key !== "total").reduce((total, [, value]) => total + value, 0);
          return { ...asset, score: breakdown.total, score_breakdown: breakdown, personal_style_score: profile ? asset.personal_style_score : null };
        }) : [];
        // Mock diversification: per asset_type is the mock's stand-in for per-source.
        const cap = args.duplicateCap ?? null;
        let skipped = 0;
        const seen = {};
        const generalCapped = cap
          ? general.filter((asset) => {
              seen[asset.asset_type] = (seen[asset.asset_type] || 0) + 1;
              if (seen[asset.asset_type] > cap) { skipped += 1; return false; }
              return true;
            })
          : general;
        return clone({ brief: args.brief || "", context_key: args.context, general: generalCapped, personalized, profile, duplicate_cap: cap, skipped_duplicates: skipped });
      }
      case "plan_list": return clone([...plans.values()].map(planView));
      case "plan_create": {
        if (!args.name?.trim() || !args.contextKey?.trim()) throw "Name and context required";
        const plan = { id: `plan-${nextPlan++}`, name: args.name, contextKey: args.contextKey, description: args.description || "", brief: args.brief || "", items: [], revisions: [], createdAt: "2026-08-29T12:00:00Z", updatedAt: "2026-08-29T12:00:00Z" };
        plans.set(plan.id, plan); return clone(planView(plan));
      }
      case "plan_get": return clone(planView(planFor(args.id)));
      case "plan_items": return clone(planFor(args.id).items);
      case "plan_revisions": return clone(planFor(args.id).revisions);
      case "plan_update": {
        const plan = planFor(args.id);
        Object.assign(plan, { name: args.name, description: args.description, brief: args.brief }); return true;
      }
      case "plan_add_item": {
        const plan = planFor(args.id), source = args.item;
        if (plan.items.some((item) => item.mediaKind === planKind(source.assetType) && item.mediaId === source.mediaId)) throw "Duplicate plan item";
        const item = { mediaKind: planKind(source.assetType), mediaId: source.mediaId, position: plan.items.length, startS: source.startS, endS: source.endS, pacing: null, cropX: null, gradeJson: null, reason: source.reason, signalsJson: source.signalsJson, origin: source.origin, rank: source.rank, profileVersion: source.profileVersion, addedAt: "2026-08-29T12:00:00Z" };
        validatePlanItem(item); plan.items.push(item); return clone(item);
      }
      case "plan_update_item": {
        if (failPlanSave) { failPlanSave = false; throw "Disk full — plan not saved"; }
        const item = planItemFor(planFor(args.id), args);
        const updated = { ...item, ...args.patch }; validatePlanItem(updated);
        // Task 037 parity: the real store derives the `adjusted` marker against the item's
        // import-time boundaries (imported_start_s/imported_end_s, else the span candidate).
        if (updated.mediaKind === "span") {
          const provenance = (() => { try { return JSON.parse(updated.provenanceJson || "{}"); } catch { return {}; } })();
          const candidate = (() => { try { return JSON.parse(updated.signalsJson || "{}").candidate || {}; } catch { return {}; } })();
          const defaultStart = provenance.imported_start_s ?? candidate.start_s;
          const defaultEnd = provenance.imported_end_s ?? candidate.end_s;
          const matchesDefault = updated.startS === defaultStart && updated.endS === defaultEnd;
          if (matchesDefault) { delete provenance.adjusted; delete provenance.adjusted_at; }
          else { provenance.adjusted = true; provenance.adjusted_at = "2026-08-29T12:00:00Z"; }
          updated.provenanceJson = JSON.stringify(provenance);
        }
        Object.assign(item, updated); return clone(item);
      }
      case "plan_remove_item": {
        const plan = planFor(args.id), item = planItemFor(plan, args);
        plan.items = plan.items.filter((value) => value !== item);
        plan.items.forEach((value, position) => value.position = position); return true;
      }
      case "plan_reorder_items": {
        const plan = planFor(args.id);
        plan.items = args.items.map((ref) => planItemFor(plan, ref));
        plan.items.forEach((value, position) => value.position = position); return clone(plan.items);
      }
      case "plan_sequence_signals":
        return clone(sequenceReportFor(planFor(args.id)));
      case "plan_sequence_suggestions": {
        const plan = planFor(args.id);
        const report = sequenceReportFor(plan);
        const suggestions = report.transitions
          .filter((value) => value.near_duplicate)
          .flatMap((transition) => {
            const later = transition.position + 1;
            const order = plan.items.filter((_, index) => index !== later).map((item) => ({ media_kind: item.mediaKind, media_id: item.mediaId }));
            order.push({ media_kind: plan.items[later].mediaKind, media_id: plan.items[later].mediaId });
            if (order.map((entry) => entry.media_id).join("|") === plan.items.map((item) => item.mediaId).join("|")) return [];
            return [{
              position: later,
              media_kind: plan.items[later].mediaKind,
              media_id: plan.items[later].mediaId,
              neighbor_position: transition.position,
              note: `Items ${transition.position + 1} and ${later + 1} look near-identical. Move item ${later + 1} to the end so similar shots are not back-to-back.`,
              suggested_order: order,
            }];
          });
        return clone(suggestions);
      }
      case "plan_save_revision": {
        const plan = planFor(args.id);
        const revision = { revision: plan.revisions.length + 1, label: args.label, snapshotJson: JSON.stringify({ items: plan.items, description: plan.description, brief: plan.brief, contextKey: plan.contextKey }), createdAt: "2026-08-29T12:00:00Z" };
        plan.revisions.push(revision); return clone(revision);
      }
      case "plan_restore_revision": {
        const plan = planFor(args.id), revision = plan.revisions.find((value) => value.revision === args.revision);
        Object.assign(plan, JSON.parse(revision.snapshotJson)); return clone(plan.items);
      }
      case "plan_duplicate": {
        const plan = clone(planFor(args.id));
        Object.assign(plan, { id: `plan-${nextPlan++}`, name: args.name, revisions: [] });
        plans.set(plan.id, plan); return clone(planView(plan));
      }
      case "plan_delete": return plans.delete(args.id);
      case "list_render_presets":
        // Mirrors the Rust enum facts (RenderPresetCatalog, camelCase) exactly.
        return clone({
          photo: [
            { id: "jpeg-srgb-v1", label: "JPEG — smaller, easy to share", extension: "jpg", extensions: ["jpg", "jpeg"], mediaType: "image/jpeg", muxer: null },
            { id: "png-srgb-v1", label: "PNG — lossless", extension: "png", extensions: ["png"], mediaType: "image/png", muxer: null },
            { id: "tiff-srgb-v1", label: "TIFF — lossless 8-bit copy", extension: "tif", extensions: ["tif", "tiff"], mediaType: "image/tiff", muxer: null },
          ],
          clip: [
            { id: "mp4-h264-sdr-v1", label: "MP4 — compatible H.264", extension: "mp4", extensions: ["mp4"], mediaType: "video/mp4", muxer: "mp4" },
            { id: "mov-h264-sdr-v1", label: "MOV — editing-friendly H.264", extension: "mov", extensions: ["mov"], mediaType: "video/quicktime", muxer: "mov" },
          ],
        });
      case "render_photo": {
        return {
          jobId: "render-job-detail-photo",
          outputPath: args.destination,
          manifestPath: `${args.destination}.crush-manifest.json`,
          outputSha256: "d".repeat(64),
          manifestSha256: "e".repeat(64),
          sizeBytes: 2097152,
          mediaType: args.preset.startsWith("jpeg") ? "image/jpeg" : args.preset.startsWith("tiff") ? "image/tiff" : "image/png",
          width: 6000,
          height: 4000,
          durationS: null,
          completedAt: "2026-08-29T12:40:00Z",
        };
      }
      case "render_project_photo": {
        if (scenario === "plans-errors") throw "Source photo changed after it was selected";
        return {
          jobId: "render-job-demo",
          outputPath: args.destination,
          manifestPath: `${args.destination}.crush-manifest.json`,
          outputSha256: "a".repeat(64),
          manifestSha256: "b".repeat(64),
          sizeBytes: 3145728,
          mediaType: args.preset.startsWith("jpeg") ? "image/jpeg" : args.preset.startsWith("tiff") ? "image/tiff" : "image/png",
          width: 2400,
          height: 1600,
          durationS: null,
          completedAt: "2026-08-29T12:30:00Z",
        };
      }
      case "render_project_clip": {
        // Task 037: span clips export like shot clips — find the item by id across kinds.
        const plan = planFor(args.projectId);
        const item = plan.items.find((value) => value.mediaId === args.shotId && (value.mediaKind === "shot" || value.mediaKind === "span"));
        if (!item) throw "clip not selected in this project";
        if (item.pacing != null) throw "saved pacing is not supported by single-clip export yet; remove the pacing value before rendering";
        if (item.cropX != null) throw "the saved horizontal crop cannot map exactly to this export; remove it before rendering";
        const grade = JSON.parse(item.gradeJson || "{}");
        if (Object.keys(grade).length && grade.mode !== "basic" && grade.mode !== "none") {
          throw "this clip's saved color treatment cannot be rendered exactly yet; remove it or use a supported basic treatment";
        }
        return {
          jobId: "render-job-clip-demo",
          outputPath: args.destination,
          manifestPath: `${args.destination}.crush-manifest.json`,
          outputSha256: "c".repeat(64),
          manifestSha256: "d".repeat(64),
          sizeBytes: 8388608,
          mediaType: args.preset.startsWith("mp4") ? "video/mp4" : "video/quicktime",
          width: 1920,
          height: 1080,
          durationS: item.endS - item.startS,
          completedAt: "2026-08-29T12:31:00Z",
        };
      }
      case "render_project_reel": {
        const plan = planFor(args.projectId);
        if (!plan.items.length) throw "add at least one clip before rendering a reel";
        // Task 037: imported spans render in the reel; photos still need a versioned
        // duration and framing contract.
        if (plan.items.some((item) => item.mediaKind === "photo")) throw "whole-reel photo holds need a saved duration and framing contract";
        return {
          jobId: "render-job-reel-demo",
          outputPath: args.destination,
          manifestPath: `${args.destination}.crush-manifest.json`,
          outputSha256: "e".repeat(64),
          manifestSha256: "f".repeat(64),
          sizeBytes: 12582912,
          mediaType: args.preset.startsWith("mp4") ? "video/mp4" : "video/quicktime",
          width: 1920,
          height: 1080,
          durationS: plan.items.reduce((total, item) => total + (item.endS - item.startS), 0),
          completedAt: "2026-08-29T12:32:00Z",
        };
      }
      case "cancel_project_render":
        return true;
      case "models_status":
        return modelsStatus();
      case "models_download":
        return modelsDownload();
      case "list_videos":
        return libraryList.map((asset) => ({ ...asset }));
      case "remove_asset": {
        if (scenario === "library-remove-partial" && args.id === "photo-two") {
          // The second remove fails with a mappable backend error: the partial
          // message must surface the mapped headline, with the raw text only
          // reachable through Copy details.
          throw "Disk full — no space left on the device";
        }
        const index = libraryList.findIndex((asset) => asset.id === args.id);
        if (index === -1) throw `No asset ${args.id}`;
        const [removed] = libraryList.splice(index, 1);
        emit("ingest-progress", snapshot());
        return { removed: true, kind: removed.assetType };
      }
      case "job_status": {
        const snap = snapshot();
        // The real backend emits ingest-progress from job_status; mirror that so the
        // ingest-relinked scenario drives the Library's relinked-files message. The real
        // backend also never prunes finished tasks, so it re-fires the same finished job
        // on every event — which is exactly what the scenario's once-per-job assertion
        // exercises.
        if (scenario === "ingest-relinked") emit("ingest-progress", snap);
        return snap;
      }
      case "relink_asset": {
        const asset = libraryList.find((candidate) => candidate.id === args.id);
        if (!asset) throw `No asset ${args.id}`;
        // Mirrors the backend: a different file is refused honestly and nothing changes.
        if (String(args.newPath).includes("DIFFERENT")) {
          throw `relink refused: the file at ${args.newPath} is not the same media Crush indexed (SHA-256 mismatch). Nothing was changed.`;
        }
        asset.path = String(args.newPath);
        asset.sourceMissing = false;
        emit("ingest-progress", snapshot());
        return { mediaKind: asset.assetType, id: asset.id, fromPath: "old", newPath: asset.path };
      }
      case "doctor":
        return "Crush doctor\nffmpeg source=Bundled\nmodels=4/4 present";
      case "cancel_ingest":
        return cancelIngest();
      case "import_reel_studio": {
        const apply = Boolean(args.request?.apply);
        if (!args.request?.catalogue) throw "choose the Reel Studio clips.db first";
        return clone({
          import_id: apply ? "import-apply" : "import-dry",
          mode: apply ? "apply" : "dry_run",
          catalogue_path: args.request.catalogue,
          catalogue_sha256: "abc123",
          context_key: args.request.contextKey,
          sources: [
            { clip_id: "V1-0001", source_file: "V1-0001.mp4", resolved_path: "/Volumes/Footage/2026/V1-0001.mp4", video_id: "video-one", matched_by: "path" },
            { clip_id: "V1-0009", source_file: "V1-0009.mp4", resolved_path: null, video_id: null, matched_by: "missing_file" },
          ],
          segments: [
            { segment_id: "V1-0001_S1", clip_id: "V1-0001", video_id: "video-one", start_s: 3.2, end_s: 5.95, boundary_basis: "catalogue_tc", boundary_tolerance_s: 1.0, outcome: apply ? "unchanged" : "new", reason: "no library folder; catalogue timecodes taken literally with keyframe tolerance" },
            { segment_id: "V1-0009_S1", clip_id: "V1-0009", video_id: null, start_s: 1, end_s: 2, boundary_basis: "catalogue_tc", boundary_tolerance_s: 0, outcome: "skipped", reason: "source clip is not matched to an indexed video" },
          ],
          recipes: [{ file: args.request.recipes?.[0] || "healthy-earth.json", recipe_id: "reel-studio-healthy-earth", plan_name: "Reel Studio · Healthy Earth", items: 1, finished_project: true, outcome: apply ? "unchanged" : "new", reason: null }],
          issues: [{ kind: "missing_source", subject: "V1-0009", detail: "V1-0009.mp4 was not found under the given originals directories" }],
          planned_writes: { manual_spans_insert: apply ? 0 : 1, manual_spans_update: 0, render_recipes_insert: apply ? 0 : 1, plans_insert: apply ? 0 : 1, plan_items_insert: apply ? 0 : 1, plan_revisions_insert: apply ? 0 : 1, feedback_events_insert: 0, reference_sets_insert: 0 },
          reference_set_candidates: ["Reel Studio · Healthy Earth"],
          started_at: "2026-08-30T12:00:00Z",
          finished_at: "2026-08-30T12:00:01Z",
        });
      }
      case "add_folder":
      case "reindex_video":
      case "reindex_asset": {
        if (scenario === "reindex-stale-asset" && args.id === "photo-two") {
          // Simulates the asset being removed mid-batch (the backend answers
          // "asset … was not found"): the queue must skip it and finish the rest.
          throw "asset photo-two was not found";
        }
        if (scenario === "reindex-cancel") {
          // The job really runs: surface it as a running background ingest so the
          // queue waits for it and Cancel is reachable mid-batch.
          background.push({
            jobId: "background-test",
            kind: "ingest",
            status: "running",
            detail: `re-indexing ${args.id}`,
            error: null,
          });
          emit("ingest-progress", snapshot());
        }
        return { jobId: "background-test" };
      }
      case "search": {
        const q = String(args.q || "");
        if (scenario === "search-error" && q === "boom") {
          throw "The vector store is unavailable.";
        }
        const results = searchResults(q, String(args.kind || "all"));
        if (heldSearch && !heldSearch.resolve) {
          // Park this search in flight; releaseSearch() resolves it with the results
          // captured HERE — i.e. the kind/top the search was ISSUED with, which is
          // exactly the stale batch the app must not leave on screen.
          return new Promise((resolve) => {
            heldSearch.resolve = () => resolve(results);
          });
        }
        return results;
      }
      case "shot_detail":
        return shotDetail(args.id);
      case "span_detail":
        return spanDetailFixture(args.id);
      case "photo_detail":
        return photoDetail(args.id);
      case "shot_at_index":
        return `shot-${args.idx}`;
      case "record_feedback": {
        const asset = reviewAssets.find(
          (candidate) =>
            candidate.mediaKind === (args.assetType === "photo" ? "photo" : "shot")
            && candidate.mediaId === args.id,
        );
        if (asset) {
          const signal =
            args.signal === "prefer" ? "prefer"
            : args.signal === "pick" ? "pick"
            : args.signal === "reject" ? "reject"
            : args.signal === "rating" ? "rating"
            : args.signal;
          feedbackState.set(`${asset.mediaKind}|${asset.mediaId}`, signal);
        }
        return "feedback-test";
      }
      case "reference_set_list":
        return styleState.sets.map((set) => ({ ...set }));
      case "reference_set_create": {
        styleState.sets.push({
          id: `set-new-${styleState.sets.length + 1}`,
          name: String(args.name || "Untitled set"),
          contextKey: String(args.contextKey || "default"),
          description: String(args.description || ""),
          scope: args.scope === "selected" ? "selected" : "whole_set",
          status: "unconfirmed",
          itemCount: 0,
          createdAt: "2026-08-28T11:00:00Z",
          confirmedAt: null,
        });
        return null;
      }
      case "reference_set_add_item": {
        findStyleSet(args.setId).itemCount += 1;
        return null;
      }
      case "reference_set_remove_item":
        return true;
      case "reference_set_confirm": {
        const set = findStyleSet(args.setId);
        set.status = "confirmed";
        set.confirmedAt = "2026-08-28T11:05:00Z";
        return true;
      }
      case "reference_set_disable": {
        findStyleSet(args.setId).status = "disabled";
        return true;
      }
      case "reference_set_delete": {
        const index = styleState.sets.findIndex((candidate) => candidate.id === args.setId);
        if (index === -1) throw `No reference set ${args.setId}`;
        styleState.sets.splice(index, 1);
        return true;
      }
      // Task 034: the imported-evidence confirmation flow. Confirming creates (or extends)
      // an UNCONFIRMED set — the second click is the ordinary reference_set_confirm above,
      // and disable/delete withdraw, mirroring the real store.
      case "imported_evidence_list":
        return evidenceSpans.map((item) => {
          const names = [];
          let confirmed = false;
          for (const set of styleState.sets) {
            if (Array.isArray(set.spanIds) && set.spanIds.includes(item.spanId)) {
              names.push(set.name);
              if (set.status === "confirmed") confirmed = true;
            }
          }
          return { ...item, sets: names, confirmed };
        });
      case "imported_evidence_confirm": {
        const ids = Array.isArray(args.spanIds) ? args.spanIds : [];
        if (!ids.length) throw "choose at least one imported clip to confirm";
        const setName = String(args.setName || "").trim() || "Reel Studio · imported evidence";
        let set = styleState.sets.find((candidate) => candidate.name === setName);
        if (!set) {
          set = {
            id: `set-ev-${styleState.sets.length + 1}`,
            name: setName,
            contextKey: "default",
            description: "Confirmed imported catalogue evidence",
            scope: "whole_set",
            status: "unconfirmed",
            itemCount: 0,
            createdAt: "2026-08-28T11:00:00Z",
            confirmedAt: null,
            spanIds: [],
          };
          styleState.sets.push(set);
        }
        if (!Array.isArray(set.spanIds)) set.spanIds = [];
        let added = 0;
        let alreadyPresent = 0;
        for (const id of ids) {
          if (!evidenceSpans.some((item) => item.spanId === id)) throw `No imported clip ${id}`;
          if (set.spanIds.includes(id)) {
            alreadyPresent += 1;
            continue;
          }
          set.spanIds.push(id);
          set.itemCount += 1;
          added += 1;
        }
        return { setId: set.id, setName: set.name, added, alreadyPresent, setStatus: set.status };
      }
      case "style_profile_status":
        return styleProfileStatus();
      case "style_profile_retrain":
        return { trained: !styleState.profileReset, status: styleProfileStatus() };
      case "style_profile_reset": {
        styleState.profileReset = true;
        return 1;
      }
      case "export_clip":
        return { path: args.out, mode: "Copy" };
      case "open_in_finder":
        return null;
      case "library_counts": {
        const flagged = reviewAssets.filter((asset) => !asset.usable || asset.blurRequired).length;
        return {
          photos: reviewAssets.filter((asset) => asset.mediaKind === "photo").length,
          shots: reviewAssets.filter((asset) => asset.mediaKind === "shot").length,
          picks: 0,
          rejects: 0,
          flagged,
        };
      }
      case "library_browse":
        return reviewBrowse(args.filter || {});
      case "collection_list":
        return reviewCollections.map((collection) => ({ ...collection }));
      case "collection_create": {
        const collection = {
          id: `col-new-${reviewCollections.length + 1}`,
          name: String(args.name || "Untitled collection"),
          description: String(args.description || ""),
          createdAt: "2026-08-28T15:00:00Z",
        };
        reviewCollections.push(collection);
        return collection;
      }
      case "collection_add_items": {
        const items = Array.isArray(args.items) ? args.items : [];
        for (const item of items) {
          reviewCollectionItems.push({
            collectionId: args.id,
            mediaKind: item.assetType === "photo" ? "photo" : "shot",
            mediaId: item.mediaId,
            contextKey: item.contextKey ?? null,
          });
          const kind = item.assetType === "photo" ? "photo" : "shot";
          const asset = reviewAssets.find(
            (candidate) => candidate.mediaKind === kind && candidate.mediaId === item.mediaId,
          );
          if (asset && !asset.collectionIds.includes(args.id)) asset.collectionIds.push(args.id);
        }
        return items.length;
      }
      case "collection_items":
        return reviewCollectionItems
          .filter((item) => item.collectionId === args.id)
          .map((item) => ({ ...item, marked: false, addedAt: "2026-08-28T15:05:00Z" }));
      case "stack_list":
        return reviewStacks.map((stack) => ({ ...stack }));
      case "stack_create": {
        const stack = {
          id: `stk-new-${reviewStacks.length + 1}`,
          name: String(args.name || "Untitled stack"),
          createdAt: "2026-08-28T15:30:00Z",
        };
        reviewStacks.push(stack);
        return stack;
      }
      case "stack_add_item": {
        const asset = findReviewAsset(args.assetType, args.mediaId);
        if (args.role === "original" && asset.stackIds.length) {
          // Mirrors the partial unique index: one original per stack.
          throw `asset ${asset.mediaId} is already an original in another stack`;
        }
        if (!asset.stackIds.includes(args.stackId)) asset.stackIds.push(args.stackId);
        return null;
      }
      case "stack_remove_item": {
        const asset = findReviewAsset(args.assetType, args.mediaId);
        asset.stackIds = asset.stackIds.filter((stackId) => stackId !== args.stackId);
        return true;
      }
      case "stacks_for_asset":
        return reviewStacks.filter((stack) => {
          const kind = args.assetType === "photo" ? "photo" : "shot";
          const asset = reviewAssets.find(
            (candidate) => candidate.mediaKind === kind && candidate.mediaId === args.mediaId,
          );
          return asset ? asset.stackIds.includes(stack.id) : false;
        });
      case "saved_search_list":
        return reviewSavedSearches.map((saved) => ({ ...saved }));
      case "saved_search_create": {
        const saved = {
          id: `ss-new-${reviewSavedSearches.length + 1}`,
          name: String(args.name || "Untitled search"),
          query: String(args.query || ""),
          contextKey: String(args.contextKey || "default"),
          filtersJson: String(args.filtersJson || "{}"),
          createdAt: "2026-08-28T16:00:00Z",
        };
        reviewSavedSearches.push(saved);
        return saved;
      }
      case "saved_search_delete": {
        const index = reviewSavedSearches.findIndex((saved) => saved.id === args.id);
        if (index === -1) throw `No saved search ${args.id}`;
        reviewSavedSearches.splice(index, 1);
        return true;
      }
      case "editorial_annotation_get":
        return reviewAnnotation(args.assetType, args.id);
      case "set_safety_flags": {
        const asset = findReviewAsset(args.assetType, args.id);
        asset.facesVisible = Boolean(args.facesVisible);
        asset.nametagsVisible = Boolean(args.nametagsVisible);
        asset.blurRequired = Boolean(args.blurRequired);
        asset.usable = Boolean(args.usable);
        return null;
      }
      case "set_annotation":
        annotationEdits.set(args.id, {
          ...annotationEdits.get(args.id),
          ...(args.fields || {}),
        });
        return null;
      case "review_batch": {
        const ops = Array.isArray(args.ops) ? args.ops : [];
        for (const op of ops) {
          if (op.op === "rate") {
            const asset = findReviewAsset(op.assetType, op.mediaId);
            asset.quality = op.rating;
            feedbackState.set(`${asset.mediaKind}|${asset.mediaId}`, "rating");
          } else if (op.op === "pick") {
            const asset = findReviewAsset(op.assetType, op.mediaId);
            feedbackState.set(`${asset.mediaKind}|${asset.mediaId}`, "pick");
          } else if (op.op === "reject") {
            const asset = findReviewAsset(op.assetType, op.mediaId);
            feedbackState.set(`${asset.mediaKind}|${asset.mediaId}`, "reject");
          } else if (op.op === "add_to_collection") {
            const asset = findReviewAsset(op.assetType, op.mediaId);
            if (!asset.collectionIds.includes(op.collectionId)) {
              asset.collectionIds.push(op.collectionId);
            }
          }
          // flag appends a feedback event; the mock records the call itself.
        }
        return ops.length;
      }
      default:
        throw `Unhandled command: ${command}`;
    }
  }

  window.__TAURI__ = {
    core: {
      convertFileSrc: (path) => {
        if (String(path).endsWith("select.jpg")) return photoPreview;
        if (String(path).includes("/thumbs/")) return thumbPreview;
        return `asset://localhost/${encodeURIComponent(String(path))}`;
      },
      invoke,
    },
    event: { listen },
    window: {
      getCurrentWindow() {
        return {
          async onDragDropEvent(handler) {
            listeners.set("drag-drop", handler);
            return () => listeners.delete("drag-drop");
          },
        };
      },
    },
    dialog: {
      async open(options = {}) {
        calls.push({ command: "dialog.open", args: options });
        if (scenario === "relocate-moved-file") {
          // First attempt: the user picks a different file (refused). Second attempt:
          // the actual moved copy (verified and relinked).
          relocateAttempts += 1;
          return relocateAttempts === 1
            ? "/Volumes/Footage/moved/rocket-launch-DIFFERENT.mov"
            : "/Volumes/Footage/moved/rocket-launch.mov";
        }
        if (scenario !== "import-reel-studio") return null;
        if (options.directory) return options.multiple ? ["/Volumes/Footage/2026"] : "/Volumes/Video Production";
        if (options.filters?.[0]?.extensions?.includes("json")) return ["/Users/john/Desktop/healthy-earth.json"];
        return "/Volumes/Video Production/clips.db";
      },
      async save(options) {
        calls.push({ command: "dialog.save", args: options });
        return `/tmp/${options.defaultPath}`;
      },
    },
    clipboardManager: {
      async writeText(text) {
        calls.push({ command: "clipboard.writeText", args: { text } });
        if (scenario === "library-remove-partial") {
          // Clipboard writes can fail for real (WKWebView permission/focus); the
          // UI must say so instead of failing silently.
          throw "clipboard unavailable";
        }
      },
    },
  };

  window.__crushMock = { calls, emit, holdNextSearch, releaseSearch };
})();
