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

  const photoPreview = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='1200' height='800'%3E%3Crect width='1200' height='800' fill='%2322304e'/%3E%3Ccircle cx='760' cy='360' r='180' fill='%23e6a648'/%3E%3Cpath d='M0 650 L420 280 L760 650 Z' fill='%234f8cff'/%3E%3C/svg%3E";

  const listeners = new Map();
  const calls = [];
  let modelsPresent = !scenario.startsWith("first-run");
  let downloadAttempts = 0;

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
    "photo-row": () => [{ ...video }, { ...photo }],
    "search-error": () => [{ ...video }],
    feedback: () => [{ ...video }],
  }[scenario]?.() ?? [];

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

  const searchResults = (q) => {
    if (scenario === "empty" || q === "zzz") return [];
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
        editorial_quality: null,
        aesthetic_score: 0.81,
        personal_style_score: 0.42,
        breakdown: {
          semantic: 0.29,
          transcript_boost: 0.06,
          editorial: 0,
          general_aesthetic: 0,
          personal_style: 0.05,
        },
      },
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
        editorial_quality: 5,
        aesthetic_score: 0.89,
        personal_style_score: 0.42,
        breakdown: {
          semantic: 0.24,
          transcript_boost: 0,
          editorial: 0,
          general_aesthetic: 0.06,
          personal_style: 0.05,
        },
      },
    ];
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

  async function invoke(command, args = {}) {
    calls.push({ command, args });
    switch (command) {
      case "models_status":
        return modelsStatus();
      case "models_download":
        return modelsDownload();
      case "list_videos":
        return library.map((asset) => ({ ...asset }));
      case "job_status":
        return snapshot();
      case "doctor":
        return "Crush doctor\nffmpeg source=Bundled\nmodels=4/4 present";
      case "cancel_ingest":
        return cancelIngest();
      case "add_folder":
      case "reindex_video":
        return { jobId: "background-test" };
      case "search": {
        const q = String(args.q || "");
        if (scenario === "search-error" && q === "boom") {
          throw "The vector store is unavailable.";
        }
        return searchResults(q);
      }
      case "shot_detail":
        return shotDetail(args.id);
      case "photo_detail":
        return photoDetail(args.id);
      case "shot_at_index":
        return `shot-${args.idx}`;
      case "record_feedback":
        return "feedback-test";
      case "export_clip":
        return { path: args.out, mode: "Copy" };
      case "open_in_finder":
        return null;
      default:
        throw `Unhandled command: ${command}`;
    }
  }

  window.__TAURI__ = {
    core: {
      convertFileSrc: (path) =>
        String(path).endsWith("select.jpg")
          ? photoPreview
          : `asset://localhost/${encodeURIComponent(String(path))}`,
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
      async open() {
        return null;
      },
      async save(options) {
        return `/tmp/${options.defaultPath}`;
      },
    },
    clipboardManager: {
      async writeText() {},
    },
  };

  window.__crushMock = { calls, emit };
})();
