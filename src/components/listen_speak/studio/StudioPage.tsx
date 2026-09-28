"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail as DatasetDetailType,
  type DatasetProgressEvt,
  type AdjustMode,
  type SplitBookMode,
  datasetStatusLabel,
  datasetStatusBadgeClasses,
  btnSmSecondary,
} from "@/lib/datasets/types";
import StageRow from "./components/StageRow";
import { FolderOpen, RefreshCw, Trash2 } from "lucide-react";

export default function StudioPage() {
  // Deferred until after hydration so the first client render matches the
  // server HTML (isTauri() is always false during SSR).
  const [mounted, setMounted] = useState(false);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [locations, setLocations] = useState<string[]>([]);
  const [selectedUuid, setSelectedUuid] = useState<string>("");
  const [detail, setDetail] = useState<DatasetDetailType | null>(null);

  // Create-dataset form
  const [newName, setNewName] = useState("");
  const [newDescription, setNewDescription] = useState("");
  const [createLocation, setCreateLocation] = useState("");

  // Edit-dataset form (populated from the selected dataset)
  const [editName, setEditName] = useState("");
  const [editDescription, setEditDescription] = useState("");

  // Import-media form
  const [sourceDir, setSourceDir] = useState("");
  const [link, setLink] = useState(false);

  // Adjust form
  const [adjustMode, setAdjustMode] = useState<AdjustMode>("new");

  // Split-book engine
  const [splitBookMode, setSplitBookMode] = useState<SplitBookMode>("rust");

  // Run state + log
  const [running, setRunning] = useState(false);
  const [runningStage, setRunningStage] = useState<string>("");
  const [rescanning, setRescanning] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const logRef = useRef<HTMLPreElement>(null);

  const appendLog = useCallback((text: string) => {
    const stamp = new Date().toLocaleTimeString();
    setLog((prev) => [...prev, ...text.split("\n").map((l) => `[${stamp}] ${l}`)]);
  }, []);

  useEffect(() => {
    setMounted(true);
  }, []);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [log]);

  // ---- Data loading ----

  const fetchDatasets = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<DatasetSummary[]>("dataset_list");
      setDatasets(res);
    } catch (e) {
      appendLog(`Failed to list datasets: ${String(e)}`);
    }
  }, [appendLog]);

  const fetchLocations = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<string[]>("dataset_list_locations");
      setLocations(res);
      setCreateLocation((cur) => cur || res[0] || "");
    } catch (e) {
      appendLog(`Failed to list locations: ${String(e)}`);
    }
  }, [appendLog]);

  const loadDetail = useCallback(
    async (uuid: string) => {
      if (!isTauri() || !uuid) {
        setDetail(null);
        return;
      }
      try {
        const res = await invoke<DatasetDetailType>("dataset_get", { uuid });
        setDetail(res);
      } catch (e) {
        setDetail(null);
        appendLog(`Failed to load dataset: ${String(e)}`);
      }
    },
    [appendLog]
  );

  useEffect(() => {
    fetchLocations();
    fetchDatasets();
  }, [fetchLocations, fetchDatasets]);

  useEffect(() => {
    loadDetail(selectedUuid);
  }, [selectedUuid, loadDetail]);

  // Keep the edit form in sync with the selected dataset.
  useEffect(() => {
    if (detail) {
      setEditName(detail.info.name);
      setEditDescription(detail.info.description ?? "");
    }
  }, [detail]);

  // ---- Progress events ----

  useEffect(() => {
    if (!isTauri()) return;
    let unlistenProgress: (() => void) | undefined;
    let unlistenList: (() => void) | undefined;

    listen<DatasetProgressEvt>("dataset-progress", (evt) => {
      const p = evt.payload;
      if (selectedUuid && p.uuid !== selectedUuid) return;
      appendLog(`  [${p.stage}] (${p.file_index}/${p.total_files}) ${p.current_file}`);
    })
      .then((fn) => { unlistenProgress = fn; })
      .catch(() => {});

    // Refresh dataset list when MCP operations add/remove datasets
    listen("dataset-list-changed", () => { fetchDatasets(); })
      .then((fn) => { unlistenList = fn; })
      .catch(() => {});

    return () => {
      if (unlistenProgress) unlistenProgress();
      if (unlistenList) unlistenList();
    };
  }, [appendLog, selectedUuid, fetchDatasets]);

  // ---- Stage runner ----

  const runStage = useCallback(
    async (stage: string, fn: () => Promise<unknown>, refresh = true) => {
      if (!isTauri() || running) return;
      setRunning(true);
      setRunningStage(stage);
      appendLog(`$ ${stage}`);
      try {
        const res = await fn();
        if (typeof res === "string" && res.trim()) appendLog(res);
        if (typeof res === "number") appendLog(`Imported ${res} file(s).`);
        appendLog("--- done ---");
        if (refresh && selectedUuid) await loadDetail(selectedUuid);
      } catch (e) {
        appendLog(`Error: ${String(e)}`);
      } finally {
        setRunning(false);
        setRunningStage("");
      }
    },
    [running, appendLog, loadDetail, selectedUuid]
  );

  // ---- Helpers ----

  async function pickDir(title: string): Promise<string | null> {
    if (!isTauri()) return null;
    const picked = await open({ directory: true, multiple: false, title });
    return typeof picked === "string" ? picked : null;
  }

  // ---- Actions ----

  async function handleCreate() {
    const name = newName.trim();
    if (!name) {
      appendLog("Error: dataset name is required.");
      return;
    }
    await runStage(
      "Create dataset",
      async () => {
        const res = await invoke<DatasetSummary>("dataset_create", {
          name,
          description: newDescription.trim() || null,
          location: createLocation || null,
        });
        setNewName("");
        setNewDescription("");
        await fetchDatasets();
        setSelectedUuid(res.info.uuid);
        return `Created '${res.info.name}' at ${res.path}`;
      },
      false
    );
  }

  async function handleUpdate() {
    if (!detail) return;
    const name = editName.trim();
    if (!name) {
      appendLog("Error: dataset name is required.");
      return;
    }
    const uuid = detail.info.uuid;
    await runStage(
      "Update dataset",
      async () => {
        await invoke("dataset_update", {
          uuid,
          name,
          description: editDescription.trim(),
        });
        await fetchDatasets();
        return `Updated '${name}'.`;
      },
      true
    );
  }

  function handleImportMedia() {
    if (!selectedUuid) return;
    if (!sourceDir.trim()) {
      appendLog("Error: choose a source directory first.");
      return;
    }
    runStage("Import media", () =>
      invoke<number>("dataset_import_media", {
        uuid: selectedUuid,
        sourceDir: sourceDir.trim(),
        link,
      })
    ).then(fetchDatasets);
  }

  // Re-read the dataset from disk so gating (e.g. book.txt) reflects files
  // added or removed outside the app.
  async function handleRescanAlign() {
    if (!selectedUuid || running || rescanning) return;
    setRescanning(true);
    try {
      await loadDetail(selectedUuid);
      appendLog("Rescanned dataset files.");
    } finally {
      setRescanning(false);
    }
  }

  // ---- Derived gating ----

  const editing = !!detail;
  const hasMedia = (detail?.media.length ?? 0) > 0;
  const hasDb = !!detail?.has_database;
  const hasBook = !!detail?.has_book;
  const hasWaveforms = !!detail?.has_waveforms;
  const hasSubtitles = !!detail?.has_subtitles;

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0">
        <h1 className="text-[1.3em] font-bold">Studio</h1>
        <select
          className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[220px]"
          value={selectedUuid}
          onChange={(e) => setSelectedUuid(e.target.value)}
          disabled={running}
        >
          <option value="">Select a dataset…</option>
          {datasets.map((ds) => (
            <option key={ds.path} value={ds.info.uuid}>
              {ds.info.name}
            </option>
          ))}
        </select>
        {detail && (
          <span className={datasetStatusBadgeClasses(detail.status)}>
            {datasetStatusLabel(detail.status)}
          </span>
        )}
        <button
          className={`${btnSmSecondary} inline-flex items-center gap-1 ml-auto`}
          onClick={() => {
            fetchLocations();
            fetchDatasets();
            if (selectedUuid) loadDetail(selectedUuid);
          }}
          disabled={running}
        >
          <RefreshCw size={14} /> Refresh
        </button>
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Studio is only available in the desktop app.</p>
      ) : (
        <div className="flex-1 min-h-0 flex gap-3">
          {/* Stages */}
          <div className="flex-1 min-w-0 overflow-y-auto flex flex-col gap-4 pb-4">
            {/* Create / edit dataset */}
            <section className="flex flex-col gap-2">
              <h2 className="text-sm font-semibold text-text-secondary uppercase tracking-wide">
                Dataset
              </h2>
              <StageRow
                title={editing ? "Edit Dataset" : "Create Dataset"}
                description={
                  editing
                    ? `Update the name and description of '${detail?.info.name}'`
                    : "Create a new empty dataset under a location"
                }
                running={
                  running && runningStage === (editing ? "Update dataset" : "Create dataset")
                }
                disabled={running}
                runLabel={editing ? "Save" : "Create"}
                onRun={editing ? handleUpdate : handleCreate}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <input
                    className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none flex-1 min-w-[160px]"
                    placeholder="Name"
                    value={editing ? editName : newName}
                    onChange={(e) =>
                      editing ? setEditName(e.target.value) : setNewName(e.target.value)
                    }
                    disabled={running}
                  />
                  <input
                    className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none flex-1 min-w-[160px]"
                    placeholder="Description (optional)"
                    value={editing ? editDescription : newDescription}
                    onChange={(e) =>
                      editing
                        ? setEditDescription(e.target.value)
                        : setNewDescription(e.target.value)
                    }
                    disabled={running}
                  />
                </div>
                {!editing && (
                  <select
                    className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                    value={createLocation}
                    onChange={(e) => setCreateLocation(e.target.value)}
                    disabled={running}
                  >
                    {locations.length === 0 && <option value="">No locations configured</option>}
                    {locations.map((loc) => (
                      <option key={loc} value={loc}>
                        {loc}
                      </option>
                    ))}
                  </select>
                )}
              </StageRow>
            </section>

            {!selectedUuid ? (
              <p className="text-sm text-text-tertiary">
                Select a dataset above (or create one) to run the pipeline stages.
              </p>
            ) : (
              <>
                {/* Pipeline */}
                <section className="flex flex-col gap-2">
                  <h2 className="text-sm font-semibold text-text-secondary uppercase tracking-wide">
                    Pipeline
                  </h2>

                  <StageRow
                    title="1. Import Media"
                    description="Copy audio/video files into the dataset's media/ folder"
                    running={running && runningStage === "Import media"}
                    disabled={running}
                    onRun={handleImportMedia}
                  >
                    <div className="flex items-center gap-2">
                      <input
                        className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none flex-1 min-w-0"
                        placeholder="Source directory"
                        value={sourceDir}
                        onChange={(e) => setSourceDir(e.target.value)}
                        disabled={running}
                      />
                      <button
                        className={`${btnSmSecondary} inline-flex items-center gap-1`}
                        disabled={running}
                        onClick={async () => {
                          const p = await pickDir("Select media source directory");
                          if (p) setSourceDir(p);
                        }}
                      >
                        <FolderOpen size={14} /> Browse
                      </button>
                      <label className="flex items-center gap-1 text-xs text-text-secondary shrink-0 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={link}
                          onChange={(e) => setLink(e.target.checked)}
                          disabled={running}
                        />
                        Symlink
                      </label>
                    </div>
                  </StageRow>

                  <StageRow
                    title="2a. Generate Subtitles"
                    description="Transcribe media to VTT subtitles using the selected STT model"
                    running={running && runningStage === "Generate subtitles"}
                    disabled={running}
                    hint={hasMedia ? undefined : "No media. Import media first."}
                    onRun={() =>
                      runStage("Generate subtitles", () =>
                        invoke("dataset_generate_subtitles", { uuid: selectedUuid })
                      )
                    }
                    beforeRun={
                      <button
                        className={`${btnSmSecondary} inline-flex items-center gap-1`}
                        disabled={running}
                        onClick={() =>
                          runStage("Delete subtitles", () =>
                            invoke("dataset_delete_subtitles", { uuid: selectedUuid })
                          )
                        }
                      >
                        <Trash2 size={14} /> Clear
                      </button>
                    }
                  />

                  <StageRow
                    title="3. Build Database"
                    description="Parse subtitles + media → data.sqlite3"
                    running={running && runningStage === "Build database"}
                    disabled={running}
                    hint={hasSubtitles ? undefined : "No subtitles. Generate subtitles first."}
                    onRun={() =>
                      runStage("Build database", () =>
                        invoke("dataset_generate_database", { uuid: selectedUuid })
                      )
                    }
                    beforeRun={
                      <button
                        className={`${btnSmSecondary} inline-flex items-center gap-1`}
                        disabled={running || !hasDb}
                        onClick={() =>
                          runStage("Delete database", () =>
                            invoke("dataset_delete_database", { uuid: selectedUuid })
                          )
                        }
                      >
                        <Trash2 size={14} /> Clear
                      </button>
                    }
                  />

                  <StageRow
                    title="3b. Generate Waveforms"
                    description="Decode audio/video → peak JSON files + write to database"
                    running={running && runningStage === "Generate waveforms"}
                    disabled={running}
                    hint={!hasDb ? "No database. Build the database first." : hasMedia ? undefined : "No media. Import media first."}
                    onRun={() =>
                      runStage("Generate waveforms", () =>
                        invoke("dataset_generate_waveform", { uuid: selectedUuid })
                      )
                    }
                  />

                  <StageRow
                    title="3c. Write Subtitles to Database"
                    description="Re-import VTT subtitles into the existing database"
                    running={running && runningStage === "Write subtitles to database"}
                    disabled={running}
                    hint={!hasDb ? "No database. Build the database first." : !hasSubtitles ? "No subtitles. Generate subtitles first." : undefined}
                    onRun={() =>
                      runStage("Write subtitles to database", () =>
                        invoke<string>("dataset_write_subtitles_to_db", { uuid: selectedUuid })
                      )
                    }
                  />

                  <StageRow
                    title="3d. Sync Cue Times"
                    description="Match fresh VTT cues to DB cues by text similarity, update timestamps"
                    running={running && runningStage === "Sync cue times"}
                    disabled={running}
                    hint={!hasDb ? "No database. Build the database first." : !hasSubtitles ? "No subtitles. Generate subtitles first." : undefined}
                    onRun={() =>
                      runStage("Sync cue times", () =>
                        invoke<string>("dataset_sync_cue_times", { uuid: selectedUuid })
                      )
                    }
                  />
                </section>

                {/* Align */}
                <section className="flex flex-col gap-2">
                  <div className="flex items-center gap-2">
                    <h2 className="text-sm font-semibold text-text-secondary uppercase tracking-wide">
                      Align
                    </h2>
                    <button
                      className="p-1 rounded-md inline-flex items-center text-text-tertiary hover:text-text-primary hover:bg-bg-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                      onClick={handleRescanAlign}
                      disabled={running || rescanning}
                      title="Rescan for book.txt"
                    >
                      <RefreshCw size={12} className={rescanning ? "animate-spin" : undefined} />
                    </button>
                  </div>

                  <StageRow
                    title="4a. Split Book"
                    description="Split book.txt into sentences (book_sentences.txt cache)"
                    running={running && runningStage === "Split book"}
                    disabled={running}
                    hint={!hasDb ? "No database. Build the database first." : !hasBook ? "No book.txt in dataset." : undefined}
                    onRun={() =>
                      runStage("Split book", () =>
                        invoke<string>("dataset_parse_book", {
                          datasetUuid: selectedUuid,
                          mode: splitBookMode,
                        })
                      )
                    }
                  >
                    <div className="flex flex-col gap-1 text-xs text-text-secondary">
                      <label className="flex items-start gap-1.5 cursor-pointer">
                        <input
                          type="radio"
                          name="split-book-mode"
                          className="mt-0.5"
                          checked={splitBookMode === "rust"}
                          onChange={() => setSplitBookMode("rust")}
                          disabled={running}
                        />
                        <span>
                          <span className="font-medium text-text-primary">Built-in (Rust)</span> —
                          fast, no setup, simple heuristic splitting
                        </span>
                      </label>
                      <label className="flex items-start gap-1.5 cursor-pointer">
                        <input
                          type="radio"
                          name="split-book-mode"
                          className="mt-0.5"
                          checked={splitBookMode === "python"}
                          onChange={() => setSplitBookMode("python")}
                          disabled={running}
                        />
                        <span>
                          <span className="font-medium text-text-primary">Python + NLTK</span> —
                          higher quality; requires Python &amp; nltk installed (e.g. via Miniconda)
                        </span>
                      </label>
                    </div>
                  </StageRow>

                  <StageRow
                    title="4b. Align Cues (Book)"
                    description="Align cues against book.txt reference text"
                    running={running && runningStage === "Align cues (book)"}
                    disabled={running}
                    hint={!hasDb ? "No database. Build the database first." : !hasBook ? "No book.txt in dataset." : undefined}
                    onRun={() =>
                      runStage("Align cues (book)", () =>
                        invoke<string>("dataset_align_cues", { datasetUuid: selectedUuid })
                      )
                    }
                  />

                  <StageRow
                    title="4b. Align Cues (Transcript)"
                    description="Align cues against transcript/*.txt reference text"
                    running={running && runningStage === "Align cues (transcript)"}
                    disabled={running}
                    hint={hasDb ? undefined : "No database. Build the database first."}
                    onRun={() =>
                      runStage("Align cues (transcript)", () =>
                        invoke<string>("dataset_align_cues_transcript", { uuid: selectedUuid })
                      )
                    }
                  />

                  <StageRow
                    title="4c. Write Transcripts"
                    description="Import transcript/*.txt files into the database"
                    running={running && runningStage === "Write transcripts"}
                    disabled={running}
                    hint={hasDb ? undefined : "No database. Build the database first."}
                    onRun={() =>
                      runStage("Write transcripts", () =>
                        invoke<string>("dataset_write_transcripts", { datasetUuid: selectedUuid })
                      )
                    }
                  />
                </section>

                {/* Adjust */}
                <section className="flex flex-col gap-2">
                  <h2 className="text-sm font-semibold text-text-secondary uppercase tracking-wide">
                    Adjust Timestamps
                  </h2>

                  <StageRow
                    title="Adjust Cue Time"
                    description="Snap cue boundaries to detected silence using waveforms"
                    running={running && runningStage === "Adjust cue time"}
                    disabled={running}
                    hint={
                      !hasDb
                        ? "No database. Build the database first."
                        : !hasWaveforms
                        ? "No waveforms. Generate waveforms first."
                        : undefined
                    }
                    onRun={() =>
                      runStage("Adjust cue time", () =>
                        invoke<string>("dataset_adjust_cue_time", {
                          uuid: selectedUuid,
                          mode: adjustMode,
                        })
                      )
                    }
                  >
                    <div className="flex items-center gap-3 text-xs text-text-secondary">
                      <label className="flex items-center gap-1 cursor-pointer">
                        <input
                          type="radio"
                          name="adjust-mode"
                          checked={adjustMode === "new"}
                          onChange={() => setAdjustMode("new")}
                          disabled={running}
                        />
                        New subtitle
                      </label>
                      <label className="flex items-center gap-1 cursor-pointer">
                        <input
                          type="radio"
                          name="adjust-mode"
                          checked={adjustMode === "in_place"}
                          onChange={() => setAdjustMode("in_place")}
                          disabled={running}
                        />
                        In place
                      </label>
                    </div>
                  </StageRow>
                </section>
              </>
            )}
          </div>

          {/* Output log */}
          <div className="w-[380px] shrink-0 flex flex-col min-h-0 border border-border-default rounded-lg">
            <div className="flex items-center justify-between px-3 py-2 border-b border-border-default shrink-0">
              <span className="text-sm font-medium text-text-secondary">Output</span>
              <button
                className={`${btnSmSecondary} inline-flex items-center gap-1`}
                onClick={() => setLog([])}
              >
                <Trash2 size={14} /> Clear
              </button>
            </div>
            <pre
              ref={logRef}
              className="flex-1 min-h-0 overflow-auto p-3 text-xs font-mono text-text-secondary whitespace-pre-wrap break-words"
            >
              {log.length === 0 ? "Output will appear here." : log.join("\n")}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
