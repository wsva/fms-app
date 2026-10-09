"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail,
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
import { RefreshCw, Workflow as WorkflowIcon, Play, RotateCcw, SkipForward, Check, Copy, Send } from "lucide-react";

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
  const [selectedUuid, setSelectedUuid] = useState("");
  const [facts, setFacts] = useState<DatasetFacts | null>(null);
  const [committed, setCommitted] = useState<StatusMap>(initialStatuses());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [notice, setNotice] = useState<string>("");
  const [prompt, setPrompt] = useState("");

  useEffect(() => {
    setMounted(true);
  }, []);

  const positioned = useMemo(() => layoutSteps(), []);
  const edges = useMemo(() => buildEdges(), []);
  const view: StatusMap = useMemo(
    () => (facts ? recompute(committed, facts) : committed),
    [committed, facts],
  );

  const selectedName = datasets.find((d) => d.info.uuid === selectedUuid)?.info.name ?? selectedUuid;

  // ---- Load dictation datasets (Favorites is an internal dataset, hide it) ----
  const fetchDatasets = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<DatasetSummary[]>("dataset_list");
      setDatasets(
        res.filter(
          (d) => d.info.type === "dictation" && d.info.uuid !== "dictation-favorites",
        ),
      );
    } catch (e) {
      setNotice(`Failed to list datasets: ${String(e)}`);
    }
  }, []);

  useEffect(() => {
    fetchDatasets();
  }, [fetchDatasets]);

  // ---- Scan: probe the dataset's on-disk facts and seed step status ----
  const scan = useCallback(async () => {
    if (!isTauri() || !selectedUuid) return;
    setScanning(true);
    setNotice("");
    try {
      const detail = await invoke<DatasetDetail>("dataset_get", { uuid: selectedUuid });
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
      setPrompt("");
      setNotice("Scanned dataset — step status seeded from files on disk. Adjust any node, then generate the prompt.");
    } catch (e) {
      setNotice(`Scan failed: ${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, [selectedUuid]);

  // Reset graph when switching dataset (forces a fresh scan).
  useEffect(() => {
    setFacts(null);
    setCommitted(initialStatuses());
    setSelectedId(null);
    setPrompt("");
    setNotice("");
  }, [selectedUuid]);

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
                value={selectedUuid}
                onChange={(e) => setSelectedUuid(e.target.value)}
                disabled={scanning}
              >
                <option value="">Select a dictation dataset…</option>
                {datasets.map((ds) => (
                  <option key={ds.path} value={ds.info.uuid}>
                    {ds.info.name}
                  </option>
                ))}
              </select>
              <button
                className={`${btnSmPrimary} inline-flex items-center gap-1`}
                onClick={scan}
                disabled={!selectedUuid || scanning}
              >
                <RefreshCw size={14} className={scanning ? "animate-spin" : undefined} />
                {facts ? "Re-scan status" : "Scan status"}
              </button>
            </div>
          </div>

          {notice && (
            <div className="shrink-0 text-xs px-3 py-2 rounded-md bg-info-bg text-info-text border border-border-light">
              {notice}
            </div>
          )}

          {!facts ? (
            <div className="flex-1 min-h-0 flex items-center justify-center text-sm text-text-tertiary">
              {selectedUuid
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
