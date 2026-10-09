"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail,
  type DatasetInfo,
  btnSmPrimary,
  btnSmSecondary,
  btnSmDanger,
} from "@/lib/datasets/types";
import {
  DICTATION_STEPS,
  DICTATION_DEFINITION,
  type DatasetFacts,
  type StatusMap,
  type StepStatus,
  recompute,
  seedFromDataset,
  initialStatuses,
  layoutSteps,
  buildEdges,
  stepById,
  buildPrompt,
  statusLabel,
} from "@/lib/workflow/steps";
import { RefreshCw, Workflow as WorkflowIcon, Play, RotateCcw, SkipForward, Check, Copy, Send, Plus, FileJson, Save } from "lucide-react";

// react-flow touches browser layout APIs on mount; keep it out of the SSG pass.
const WorkflowGraph = dynamic(() => import("./WorkflowGraph"), { ssr: false });

// A workflow "section" — Dataset Dictation is the first; more get added later.
interface WorkflowSection {
  id: string;
  label: string;
  steps: typeof DICTATION_STEPS;
}
const SECTIONS: WorkflowSection[] = [
  { id: "dictation", label: "Dataset Dictation", steps: DICTATION_STEPS },
];

function runIdFor(uuid: string): string {
  return `dictation-${uuid}`;
}

export default function WorkflowPage() {
  const [mounted, setMounted] = useState(false);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  // Selection key = the dataset uuid when it has one, otherwise the folder path
  // (a raw media folder not yet initialized has an empty uuid and is addressed
  // only by its path).
  const [selectedKey, setSelectedKey] = useState("");
  const [facts, setFacts] = useState<DatasetFacts | null>(null);
  const [committed, setCommitted] = useState<StatusMap>(initialStatuses());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [initing, setIniting] = useState(false);
  const [notice, setNotice] = useState<string>("");
  const [prompt, setPrompt] = useState("");
  const [infoText, setInfoText] = useState("");
  const [infoOpen, setInfoOpen] = useState(false);
  const [savingInfo, setSavingInfo] = useState(false);

  useEffect(() => {
    setMounted(true);
  }, []);

  const positioned = useMemo(() => layoutSteps(), []);
  const edges = useMemo(() => buildEdges(), []);
  const view: StatusMap = useMemo(
    () => (facts ? recompute(committed, facts) : committed),
    [committed, facts],
  );

  const selectedDataset = useMemo(
    () => datasets.find((d) => (d.info.uuid || d.path) === selectedKey),
    [datasets, selectedKey],
  );
  const selectedUuid = selectedDataset?.info.uuid ?? "";
  const selectedPath = selectedDataset?.path ?? "";
  const isRaw = !!selectedDataset && !selectedDataset.info.uuid;
  const selectedName = selectedDataset?.info.name ?? selectedKey;

  // Frontend diagnostics → the app log (module "workflow"), so a dataset that
  // won't select can be traced from the Logs page without a debugger.
  const appendLog = useCallback((text: string, level: string = "INFO") => {
    if (!isTauri()) return;
    for (const line of text.split("\n")) {
      invoke("log_frontend_message", { message: line, level, module: "workflow" }).catch(() => {});
    }
  }, []);

  // ---- Load dictation datasets ----
  // `dataset_list` already scans only the dictation roots, so every entry is a
  // dictation dataset (ready AND not_ready). Favorites is internal — hide it. A
  // raw media folder that was never imported has an EMPTY uuid (it isn't a
  // dataset yet); keep it visible but disabled rather than letting its "" option
  // collide with the placeholder and silently block selection.
  const fetchDatasets = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<DatasetSummary[]>("dataset_list");
      appendLog(`dataset_list returned ${res.length} entr(ies):`);
      res.forEach((d) =>
        appendLog(
          `  - name='${d.info.name}' type='${d.info.type}' status='${d.status}' uuid='${d.info.uuid || "(EMPTY)"}' path='${d.path}'`,
        ),
      );
      const kept = res.filter((d) => d.info.uuid !== "dictation-favorites");
      const noUuid = kept.filter((d) => !d.info.uuid);
      if (noUuid.length) {
        appendLog(
          `${noUuid.length} folder(s) have no uuid (not imported yet) — shown but disabled: ${noUuid.map((d) => d.info.name).join(", ")}`,
          "WARN",
        );
      }
      setDatasets(kept);
    } catch (e) {
      appendLog(`dataset_list failed: ${String(e)}`, "ERROR");
      setNotice(`Failed to list datasets: ${String(e)}`);
    }
  }, [appendLog]);

  useEffect(() => {
    fetchDatasets();
  }, [fetchDatasets]);

  // ---- Load dataset: probe on-disk facts, seed steps, and read its info.json ----
  const loadDataset = useCallback(async (uuid: string) => {
    const detail = await invoke<DatasetDetail>("dataset_get", { uuid });
    const probed: DatasetFacts = {
      hasMedia: (detail.media?.length ?? 0) > 0,
      hasSubtitles: !!detail.has_subtitles,
      hasDatabase: !!detail.has_database,
      hasWaveforms: !!detail.has_waveforms,
      hasBook: !!detail.has_book,
      hasTranscript: !!detail.media?.some((m) => m.has_transcript),
    };
    setFacts(probed);
    setCommitted(seedFromDataset(probed));
    setInfoText(JSON.stringify(detail.info, null, 2));
    setPrompt("");
    return probed;
  }, []);

  const scan = useCallback(async () => {
    if (!isTauri() || !selectedUuid) return;
    setScanning(true);
    setNotice("");
    appendLog(`scan: dataset_get uuid='${selectedUuid}' name='${selectedName}'`);
    try {
      const probed = await loadDataset(selectedUuid);
      appendLog(`scan: facts=${JSON.stringify(probed)}`);
      setNotice("Scanned dataset — step status seeded from files on disk. Adjust any node, then generate the prompt.");
    } catch (e) {
      appendLog(`scan failed for uuid='${selectedUuid}': ${String(e)}`, "ERROR");
      setNotice(`Scan failed: ${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, [selectedUuid, selectedName, appendLog, loadDataset]);

  // ---- Initialize a raw folder in place, then load it as the new dataset ----
  const handleInit = async () => {
    if (!isTauri() || !isRaw) return;
    const ok = await ask(
      `Initialize this folder as a dictation dataset?\n\n${selectedPath}\n\nThis writes a default info.json (with a new app-generated uuid) into the folder — no files are copied or deleted. You can edit it next.`,
      { title: "Initialize dataset", kind: "info", okLabel: "Initialize", cancelLabel: "Cancel" },
    );
    if (!ok) return;
    setIniting(true);
    appendLog(`init: dataset_init_dir path='${selectedPath}'`);
    try {
      const summary = await invoke<DatasetSummary>("dataset_init_dir", { path: selectedPath });
      appendLog(`init ok: uuid='${summary.info.uuid}'`);
      await fetchDatasets();
      setSelectedKey(summary.info.uuid);
      await loadDataset(summary.info.uuid);
      setInfoOpen(true);
      setNotice(`Initialized dataset “${summary.info.name}” (uuid ${summary.info.uuid}). Edit its info.json, then generate the Agent prompt.`);
    } catch (e) {
      appendLog(`init failed for path='${selectedPath}': ${String(e)}`, "ERROR");
      setNotice(`Initialize failed: ${String(e)}`);
    } finally {
      setIniting(false);
    }
  };

  // ---- Save edited info.json (identity fields are enforced server-side) ----
  const saveInfo = async () => {
    if (!isTauri() || !selectedUuid) return;
    setSavingInfo(true);
    appendLog(`info save: uuid='${selectedUuid}'`);
    try {
      const saved = await invoke<DatasetInfo>("dataset_info_save", { uuid: selectedUuid, content: infoText });
      setInfoText(JSON.stringify(saved, null, 2));
      appendLog("info save ok");
      setNotice("info.json saved.");
    } catch (e) {
      appendLog(`info save failed: ${String(e)}`, "ERROR");
      setNotice(`Save failed: ${String(e)}`);
    } finally {
      setSavingInfo(false);
    }
  };

  // ---- Node controls: mutate the committed status, recompute drives the view ----
  const setStep = (id: string, status: StepStatus) => {
    setCommitted((prev) => ({ ...prev, [id]: status }));
    setPrompt("");
  };

  const generatePrompt = () => {
    if (!facts) return;
    const text = buildPrompt({
      datasetName: selectedName,
      datasetUuid: selectedUuid,
      runId: runIdFor(selectedUuid),
      view,
    });
    setPrompt(text);
  };

  const sendToDock = async () => {
    if (!prompt.trim() || !isTauri()) return;
    try {
      // Hand the reviewed text to the Agent Dock (opens it + fills the input);
      // the user sends it there once connected, keeping the final check in-dock.
      await emit("agent-prefill", { text: prompt });
      setNotice("Prompt sent to the Agent Dock — open it (Ctrl+Space) and send when connected.");
    } catch (e) {
      setNotice(`Send failed: ${String(e)}`);
    }
  };

  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(prompt);
      setNotice("Prompt copied to clipboard.");
    } catch {
      setNotice("Copy failed.");
    }
  };

  const sel = selectedId ? stepById(selectedId) : undefined;
  const readyCount = Object.values(view).filter((s) => s === "ready").length;

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0">
        <WorkflowIcon size={20} className="text-accent shrink-0" />
        <h1 className="text-[1.3em] font-bold">Workflow</h1>
        <span className="text-xs text-text-tertiary">
          Visualize a pipeline as a DAG, mark what to redo/skip, then hand a driver prompt to the Agent.
        </span>
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Workflow is only available in the desktop app.</p>
      ) : (
        <div className="flex-1 min-h-0 flex flex-col gap-3">
          {/* Sections — only Dataset Dictation for now */}
          <div className="flex items-center gap-3 flex-wrap shrink-0 rounded-lg border border-border-light bg-bg-card px-3 py-2">
            {SECTIONS.map((s) => (
              <span
                key={s.id}
                className="text-sm font-semibold text-text-primary px-2 py-1 rounded-md bg-bg-muted"
                title={`${DICTATION_DEFINITION.name} v${DICTATION_DEFINITION.version}`}
              >
                {s.label}
              </span>
            ))}
            <div className="ml-auto flex items-center gap-2">
              <select
                className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[220px]"
                value={selectedKey}
                onChange={(e) => {
                  const key = e.target.value;
                  setSelectedKey(key);
                  // Clear the derived view only on an explicit user switch, so a
                  // programmatic re-select after "Initialize" is not wiped.
                  setFacts(null);
                  setCommitted(initialStatuses());
                  setSelectedId(null);
                  setPrompt("");
                  setNotice("");
                  setInfoText("");
                  setInfoOpen(false);
                  appendLog(`selection changed to '${key}'`);
                }}
                disabled={scanning || initing}
              >
                <option value="">Select a dictation dataset…</option>
                {datasets.map((ds) => (
                  <option key={ds.path} value={ds.info.uuid || ds.path}>
                    {ds.info.name}
                    {ds.info.uuid ? (ds.status === "ready" ? "" : " · not ready") : " · not imported"}
                  </option>
                ))}
              </select>
              {isRaw ? (
                <button
                  className={`${btnSmPrimary} inline-flex items-center gap-1`}
                  onClick={handleInit}
                  disabled={initing}
                >
                  <Plus size={14} className={initing ? "animate-spin" : undefined} />
                  {initing ? "Initializing…" : "Initialize as dataset"}
                </button>
              ) : (
                <button
                  className={`${btnSmPrimary} inline-flex items-center gap-1`}
                  onClick={scan}
                  disabled={!selectedUuid || scanning}
                >
                  <RefreshCw size={14} className={scanning ? "animate-spin" : undefined} />
                  {facts ? "Re-scan status" : "Scan status"}
                </button>
              )}
            </div>
          </div>

          {notice && (
            <div className="shrink-0 text-xs px-3 py-2 rounded-md bg-info-bg text-info-text border border-border-light">
              {notice}
            </div>
          )}

          {!facts ? (
            <div className="flex-1 min-h-0 flex items-center justify-center text-sm text-text-tertiary px-6 text-center">
              {isRaw
                ? "This folder has media but no info.json yet — click “Initialize as dataset” to create it, then edit its metadata and lay out the pipeline."
                : selectedUuid
                  ? "Click “Scan status” to read the dataset and lay out the pipeline."
                  : "Select a Dataset Dictation dataset above to view its workflow graph."}
            </div>
          ) : (
            <div className="flex-1 min-h-0 flex gap-3">
              {/* Graph */}
              <div className="flex-1 min-w-0 rounded-lg border border-border-light overflow-hidden relative">
                <div className="absolute left-2 top-2 z-10 text-[11px] px-2 py-1 rounded bg-bg-card/80 border border-border-light text-text-secondary">
                  {readyCount} step(s) ready · {DICTATION_DEFINITION.yamlPath}
                </div>
                <WorkflowGraph
                  positioned={positioned}
                  edges={edges}
                  view={view}
                  selectedId={selectedId}
                  onSelect={(id) => setSelectedId(id || null)}
                />
              </div>

              {/* Inspector + prompt */}
              <div className="w-[340px] shrink-0 min-h-0 overflow-y-auto flex flex-col gap-3">
                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary inline-flex items-center gap-1">
                      <FileJson size={13} /> info.json
                    </h2>
                    <button
                      className={`${btnSmSecondary} inline-flex items-center gap-1`}
                      onClick={() => setInfoOpen((o) => !o)}
                    >
                      {infoOpen ? "Hide" : "Edit"}
                    </button>
                  </div>
                  {infoOpen ? (
                    <>
                      <textarea
                        className="w-full h-48 text-[11px] font-mono p-2 rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none resize-y"
                        value={infoText}
                        onChange={(e) => setInfoText(e.target.value)}
                        spellCheck={false}
                      />
                      <div className="flex items-center gap-2">
                        <button
                          className={`${btnSmPrimary} inline-flex items-center gap-1`}
                          onClick={saveInfo}
                          disabled={savingInfo || !infoText.trim()}
                        >
                          <Save size={14} /> {savingInfo ? "Saving…" : "Save"}
                        </button>
                        <span className="text-[10px] text-text-tertiary leading-tight">
                          uuid / type / format / created_at are kept by the app
                        </span>
                      </div>
                    </>
                  ) : (
                    <p className="text-xs text-text-tertiary leading-relaxed">
                      View or edit this dataset’s metadata as raw JSON.
                    </p>
                  )}
                </section>

                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">
                    Step
                  </h2>
                  {!sel ? (
                    <p className="text-sm text-text-tertiary">Click a node to redo, skip, or mark it done.</p>
                  ) : (
                    <>
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-sm font-semibold text-text-primary truncate">{sel.title}</span>
                        <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary shrink-0">
                          {statusLabel(view[sel.id] ?? "pending")}
                        </span>
                      </div>
                      <div className="font-mono text-[11px] text-text-tertiary">{sel.action}</div>
                      <p className="text-xs text-text-secondary leading-relaxed">{sel.description}</p>
                      {sel.params && Object.keys(sel.params).length > 0 && (
                        <pre className="text-[11px] bg-bg-body border border-border-light rounded p-2 overflow-x-auto text-text-secondary">
                          {JSON.stringify(sel.params, null, 2)}
                        </pre>
                      )}
                      <div className="flex flex-wrap gap-2 mt-1">
                        <button
                          className={`${btnSmSecondary} inline-flex items-center gap-1`}
                          onClick={() => setStep(sel.id, "pending")}
                          title="Redo this step (back to pending / ready)"
                        >
                          <RotateCcw size={13} /> Redo
                        </button>
                        <button
                          className={`${btnSmSecondary} inline-flex items-center gap-1`}
                          onClick={() => setStep(sel.id, "skipped")}
                          title="Skip this step (satisfies dependents)"
                        >
                          <SkipForward size={13} /> Skip
                        </button>
                        <button
                          className={`${btnSmSecondary} inline-flex items-center gap-1`}
                          onClick={() => setStep(sel.id, "completed")}
                          title="Mark done without running"
                        >
                          <Check size={13} /> Mark done
                        </button>
                        <button
                          className={`${btnSmDanger} inline-flex items-center gap-1`}
                          onClick={() => setStep(sel.id, "failed")}
                          title="Mark this step failed"
                        >
                          <Play size={13} /> Mark failed
                        </button>
                      </div>
                    </>
                  )}
                </section>

                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">
                      Agent prompt
                    </h2>
                    <button
                      className={`${btnSmSecondary} inline-flex items-center gap-1`}
                      onClick={generatePrompt}
                    >
                      <WorkflowIcon size={13} /> Generate
                    </button>
                  </div>
                  {prompt ? (
                    <>
                      <textarea
                        className="w-full h-56 text-[11px] font-mono p-2 rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none resize-none"
                        value={prompt}
                        onChange={(e) => setPrompt(e.target.value)}
                      />
                      <div className="flex gap-2">
                        <button
                          className={`${btnSmPrimary} inline-flex items-center gap-1 flex-1 justify-center`}
                          onClick={sendToDock}
                        >
                          <Send size={14} /> Send to Agent Dock
                        </button>
                        <button
                          className={`${btnSmSecondary} inline-flex items-center gap-1`}
                          onClick={copyPrompt}
                        >
                          <Copy size={14} /> Copy
                        </button>
                      </div>
                    </>
                  ) : (
                    <p className="text-xs text-text-tertiary leading-relaxed">
                      Generate a driver prompt from the current graph (ready / skipped / done steps)
                      for goose to run the pipeline via MCP. Edit it before sending.
                    </p>
                  )}
                </section>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
