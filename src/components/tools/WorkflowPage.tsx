"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail,
  type DatasetInfo,
  type DatasetProgressEvt,
  btnSmPrimary,
  btnSmSecondary,
  btnSmDanger,
} from "@/lib/datasets/types";
import {
  DICTATION_YAML,
  DICTATION_PARSED,
  parseDefinition,
  type WorkflowDefinition,
  type DatasetFacts,
  type StatusMap,
  type StepDef,
  type StepStatus,
  recompute,
  initialStatuses,
  layoutSteps,
  buildEdges,
  stepById,
  buildPrompt,
  statusLabel,
} from "@/lib/workflow/steps";
import { getStepOp, KNOWN_COMMANDS, type FormField, type RunCtx, type StepOp } from "@/lib/workflow/step-ops";
import { RefreshCw, Workflow as WorkflowIcon, Play, RotateCcw, SkipForward, Check, Copy, Send, Plus, FileJson, Save, FolderSync, Eye, Pencil, FolderOpen, Trash2, X } from "lucide-react";

// react-flow touches browser layout APIs on mount; keep it out of the SSG pass.
const WorkflowGraph = dynamic(() => import("./WorkflowGraph"), { ssr: false });

function runIdFor(uuid: string): string {
  return `dictation-${uuid}`;
}

/** A saved run entry from `workflow_list_runs`. */
interface RunSummary {
  run_id: string;
  name?: string;
  version?: number;
  run_status?: string;
  error?: string;
}

/** The shape `workflow_status` returns (subset we consume). */
interface EngineStatus {
  run_id: string;
  workflow: { name: string; version: number };
  run_status: string;
  steps: Record<string, { status: string }>;
}

/** Render an invoke result as a single concise output line. */
function summarizeResult(r: unknown): string {
  if (typeof r === "string") return r;
  if (typeof r === "number") return `${r} item(s)`;
  try {
    return JSON.stringify(r);
  } catch {
    return String(r);
  }
}

export default function WorkflowPage() {
  const [mounted, setMounted] = useState(false);
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  // Selection key = the dataset uuid when it has one, otherwise the folder path.
  const [selectedKey, setSelectedKey] = useState("");
  const [facts, setFacts] = useState<DatasetFacts | null>(null);
  // Local projection placeholder used only until a run exists on disk.
  const [committed, setCommitted] = useState<StatusMap>(() => initialStatuses(DICTATION_PARSED.steps));
  // Authoritative engine statuses once a run exists (drives the graph + inspector).
  const [engineView, setEngineView] = useState<StatusMap | null>(null);
  const [runMeta, setRunMeta] = useState<{ name: string; version: number; run_status: string } | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Workflow document is YAML-driven; `definition` = last-good parse, `parseError` = live.
  const [mode, setMode] = useState<"view" | "edit">("view");
  const [yamlText, setYamlText] = useState(DICTATION_YAML);
  const [definition, setDefinition] = useState<WorkflowDefinition>(DICTATION_PARSED);
  const [parseError, setParseError] = useState<string | null>(null);
  const steps = definition.steps;

  const [scanning, setScanning] = useState(false);
  const [initing, setIniting] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [notice, setNotice] = useState<string>("");

  // Inspector run state.
  const [running, setRunning] = useState(false);
  const [runningStep, setRunningStep] = useState("");
  const [formState, setFormState] = useState<Record<string, Record<string, string | boolean>>>({});
  const [outputLines, setOutputLines] = useState<string[]>([]);
  const [outputOpen, setOutputOpen] = useState(false);

  const [prompt, setPrompt] = useState("");
  const [infoText, setInfoText] = useState("");
  const [infoOpen, setInfoOpen] = useState(false);
  const [savingInfo, setSavingInfo] = useState(false);

  // Job authoring (persisting custom pipelines as runs).
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [newJobId, setNewJobId] = useState("");
  // Command chosen for the "Add step" skeleton (defaults to the first registry command).
  const [addStepAction, setAddStepAction] = useState<string>(KNOWN_COMMANDS[0] ?? "dataset_get");

  useEffect(() => {
    setMounted(true);
  }, []);

  const positioned = useMemo(() => layoutSteps(steps), [steps]);
  const edges = useMemo(() => buildEdges(steps), [steps]);
  const view: StatusMap = useMemo(
    () => engineView ?? (facts ? recompute(committed, facts, steps) : committed),
    [engineView, committed, facts, steps],
  );

  const selectedDataset = useMemo(
    () => datasets.find((d) => (d.info.uuid || d.path) === selectedKey),
    [datasets, selectedKey],
  );
  const selectedUuid = selectedDataset?.info.uuid ?? "";
  const selectedPath = selectedDataset?.path ?? "";
  const isRaw = !!selectedDataset && !selectedDataset.info.uuid;
  const selectedName = selectedDataset?.info.name ?? selectedKey;
  const runId = runIdFor(selectedUuid);
  const hasRun = engineView !== null;

  const appendLog = useCallback((text: string, level: string = "INFO") => {
    if (!isTauri()) return;
    for (const line of text.split("\n")) {
      invoke("log_frontend_message", { message: line, level, module: "workflow" }).catch(() => {});
    }
  }, []);

  const pushOutput = useCallback((text: string) => {
    setOutputLines((prev) => [...prev.slice(-400), text]);
  }, []);

  // ---- Load dictation datasets ----
  const fetchDatasets = useCallback(async (): Promise<DatasetSummary[] | null> => {
    if (!isTauri()) return null;
    try {
      const res = await invoke<DatasetSummary[]>("dataset_list");
      const kept = res.filter((d) => d.info.uuid !== "dictation-favorites");
      setDatasets(kept);
      return kept;
    } catch (e) {
      appendLog(`dataset_list failed: ${String(e)}`, "ERROR");
      setNotice(`Failed to list datasets: ${String(e)}`);
      return null;
    }
  }, [appendLog]);

  useEffect(() => {
    fetchDatasets();
  }, [fetchDatasets]);

  // ---- Engine status pull (authoritative once a run exists) ----
  const refreshEngine = useCallback(async (id: string): Promise<boolean> => {
    if (!isTauri() || !id) {
      setEngineView(null);
      setRunMeta(null);
      return false;
    }
    try {
      const st = await invoke<EngineStatus>("workflow_status", { runId: id });
      const map: StatusMap = {};
      for (const [k, v] of Object.entries(st.steps ?? {})) map[k] = v.status as StepStatus;
      setEngineView(map);
      setRunMeta({ name: st.workflow?.name, version: st.workflow?.version, run_status: st.run_status });
      return true;
    } catch (e) {
      // No run yet — fall back to the local projection.
      appendLog(`workflow_status('${id}'): ${String(e)}`);
      setEngineView(null);
      setRunMeta(null);
      return false;
    }
  }, [appendLog]);

  // ---- Probe on-disk facts + read info.json (does not touch the engine) ----
  const probeDataset = useCallback(async (uuid: string) => {
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
    setInfoText(JSON.stringify(detail.info, null, 2));
    return probed;
  }, []);

  const clearView = useCallback(() => {
    setFacts(null);
    setCommitted(initialStatuses(steps));
    setEngineView(null);
    setRunMeta(null);
    setSelectedId(null);
    setPrompt("");
    setInfoText("");
    setInfoOpen(false);
    setOutputLines([]);
  }, [steps]);

  const scan = useCallback(async () => {
    if (!isTauri() || !selectedUuid) return;
    setScanning(true);
    setNotice("");
    try {
      await probeDataset(selectedUuid);
      setCommitted(initialStatuses(steps));
      const live = await refreshEngine(runId);
      setNotice(live ? "Re-scanned. Graph reflects the workflow run state." : "Scanned dataset. Run any step to start its workflow run.");
    } catch (e) {
      appendLog(`scan failed: ${String(e)}`, "ERROR");
      setNotice(`Scan failed: ${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, [selectedUuid, runId, probeDataset, refreshEngine, appendLog, steps]);

  // ---- Initialize a raw folder in place, then load it ----
  const handleInit = async () => {
    if (!isTauri() || !isRaw) return;
    const ok = await ask(
      `Initialize this folder as a dictation dataset?\n\n${selectedPath}\n\nThis writes a default info.json (with a new app-generated uuid) into the folder — no files are copied or deleted.`,
      { title: "Initialize dataset", kind: "info", okLabel: "Initialize", cancelLabel: "Cancel" },
    );
    if (!ok) return;
    setIniting(true);
    try {
      const summary = await invoke<DatasetSummary>("dataset_init_dir", { path: selectedPath });
      await fetchDatasets();
      await emit("dataset-list-changed", {});
      setSelectedKey(summary.info.uuid);
      await probeDataset(summary.info.uuid);
      setCommitted(initialStatuses(steps));
      await refreshEngine(runIdFor(summary.info.uuid));
      setInfoOpen(false);
      setNotice(`Initialized dataset “${summary.info.name}” (uuid ${summary.info.uuid}).`);
    } catch (e) {
      appendLog(`init failed: ${String(e)}`, "ERROR");
      setNotice(`Initialize failed: ${String(e)}`);
    } finally {
      setIniting(false);
    }
  };

  // ---- Save edited info.json ----
  const saveInfo = async () => {
    if (!isTauri() || !selectedUuid) return;
    setSavingInfo(true);
    try {
      const saved = await invoke<DatasetInfo>("dataset_info_save", { uuid: selectedUuid, content: infoText });
      setInfoText(JSON.stringify(saved, null, 2));
      await emit("dataset-list-changed", {});
      setNotice("info.json saved.");
    } catch (e) {
      setNotice(`Save failed: ${String(e)}`);
    } finally {
      setSavingInfo(false);
    }
  };

  const handleReload = async () => {
    if (!isTauri()) return;
    const prev = selectedDataset;
    const prevKey = selectedKey;
    setReloading(true);
    try {
      const kept = await fetchDatasets();
      if (!kept) return;
      await emit("dataset-list-changed", {});
      if (!prev || kept.some((d) => (d.info.uuid || d.path) === prevKey)) {
        setNotice(`Reloaded ${kept.length} dataset(s) from disk.`);
        return;
      }
      const renamed = kept.find((d) => d.path === prev.path && d.info.uuid);
      clearView();
      if (!renamed) {
        setSelectedKey("");
        setNotice("The selected dataset is no longer on disk — pick another one.");
        return;
      }
      setSelectedKey(renamed.info.uuid);
      await probeDataset(renamed.info.uuid);
      setCommitted(initialStatuses(steps));
      await refreshEngine(runId);
      setNotice(`Dataset uuid changed on disk: “${renamed.info.name}” is now ${renamed.info.uuid}.`);
    } finally {
      setReloading(false);
    }
  };

  // ---- Live progress from running commands ----
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen<DatasetProgressEvt>("dataset-progress", (evt) => {
      const p = evt.payload;
      if (selectedUuid && p.uuid !== selectedUuid) return;
      pushOutput(`[${p.stage}] (${p.file_index}/${p.total_files}) ${p.current_file}`);
    })
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, [selectedUuid, pushOutput]);

  // ---- Step context builder for the registry ----
  const buildCtx = useCallback((stepId: string): RunCtx => ({
    uuid: selectedUuid,
    name: selectedDataset?.info.name ?? "",
    description: selectedDataset?.info.description ?? "",
    facts,
    form: formState[stepId] ?? {},
  }), [selectedUuid, selectedDataset, facts, formState]);

  // Effective form values = registry defaults overlaid with the user's edits.
  const formValues = useCallback((stepId: string, op?: StepOp): Record<string, string | boolean> => {
    const base = op?.defaults?.({ ...buildCtx(stepId), form: {} }) ?? {};
    return { ...base, ...(formState[stepId] ?? {}) };
  }, [buildCtx, formState]);

  const setField = (stepId: string, name: string, value: string | boolean) => {
    setFormState((prev) => ({ ...prev, [stepId]: { ...(prev[stepId] ?? {}), [name]: value } }));
  };

  // ---- Ensure a run exists ----
  // A fresh run starts fully `pending`: the engine derives every readiness/skip
  // from steps actually run + their recorded outputs, never from on-disk facts.
  // (The former `seedAfterCreate` pre-marked disk-proven steps as complete, which
  // let the graph claim progress it had not earned — removed for an honest run.)
  const ensureRun = useCallback(async (): Promise<string> => {
    if (!selectedUuid) throw new Error("Select a dataset first.");
    if (hasRun) return runId;
    try {
      await invoke("workflow_create_run", { runId, yamlText });
    } catch (e) {
      // Likely already exists — refreshEngine below confirms; a genuine failure
      // surfaces when the caller tries to advance.
      appendLog(`workflow_create_run('${runId}'): ${String(e)}`, "WARN");
    }
    await refreshEngine(runId);
    return runId;
  }, [selectedUuid, hasRun, runId, yamlText, refreshEngine, appendLog]);

  // ---- Run a step through the engine + command twin ----
  const handleRun = async (step: StepDef, op: StepOp) => {
    if (!op.run) return;
    const ctx: RunCtx = { ...buildCtx(step.id), form: formValues(step.id, op) };
    if (op.gate?.(ctx)) return;
    setRunning(true);
    setRunningStep(step.id);
    setOutputOpen(true);
    pushOutput(`$ ${step.title}`);
    let id: string;
    try {
      id = await ensureRun();
    } catch (e) {
      setNotice(String(e));
      setRunning(false);
      setRunningStep("");
      return;
    }
    try {
      await invoke("workflow_advance", { runId: id, step: step.id, agentId: "ui" });
    } catch (e) {
      setNotice(String(e));
      setRunning(false);
      setRunningStep("");
      await refreshEngine(id);
      return;
    }
    try {
      const { command, args } = op.run!(ctx);
      const result = await invoke(command, args);
      pushOutput(summarizeResult(result));
      await invoke("workflow_record", {
        runId: id,
        step: step.id,
        event: "completed",
        detail: { outputs: op.outputs?.(result) ?? {} },
      });
      setNotice(`“${step.title}” completed.`);
      if (op.emitListChanged) await emit("dataset-list-changed", {});
      if (op.reloadAfter && selectedUuid) await probeDataset(selectedUuid);
      await refreshEngine(id);
    } catch (e) {
      pushOutput(`Error: ${String(e)}`);
      try {
        await invoke("workflow_record", { runId: id, step: step.id, event: "failed", detail: { reason: String(e) } });
      } catch { /* ignore */ }
      setNotice(`“${step.title}” failed: ${String(e)}`);
      await refreshEngine(id);
    } finally {
      setRunning(false);
      setRunningStep("");
    }
  };

  const handleClear = async (step: StepDef, op: StepOp) => {
    if (!op.clear || !selectedUuid) return;
    setRunning(true);
    setRunningStep(step.id);
    try {
      const { command, args } = op.clear(buildCtx(step.id));
      pushOutput(`$ clear ${step.title}`);
      await invoke(command, args);
      if (hasRun) {
        try { await invoke("workflow_intervene", { runId, step: step.id, op: "reset" }); } catch { /* ignore */ }
      } else {
        setStep(step.id, "pending");
      }
      await probeDataset(selectedUuid);
      await refreshEngine(runId);
      setNotice(`Cleared “${step.title}” outputs.`);
    } catch (e) {
      setNotice(`Clear failed: ${String(e)}`);
    } finally {
      setRunning(false);
      setRunningStep("");
    }
  };

  // Local projection edit used only when no run exists yet.
  const setStep = (id: string, status: StepStatus) => {
    setCommitted((prev) => ({ ...prev, [id]: status }));
    setPrompt("");
  };

  const intervene = async (step: StepDef, op: "reset" | "skip") => {
    if (hasRun) {
      try {
        await invoke("workflow_intervene", { runId, step: step.id, op });
        await refreshEngine(runId);
      } catch (e) {
        setNotice(String(e));
      }
    } else {
      setStep(step.id, op === "skip" ? "skipped" : "pending");
    }
  };

  const forceStatus = async (step: StepDef, event: "completed" | "failed") => {
    let id: string;
    try {
      id = await ensureRun();
    } catch (e) {
      setNotice(String(e));
      return;
    }
    try { await invoke("workflow_advance", { runId: id, step: step.id, agentId: "ui" }); } catch { /* not ready */ }
    try {
      await invoke("workflow_record", {
        runId: id, step: step.id, event,
        detail: event === "failed" ? { reason: "marked failed in UI" } : { outputs: {} },
      });
      await refreshEngine(id);
    } catch (e) {
      // No run / step not claimable: fall back to the local projection.
      setStep(step.id, event === "failed" ? "failed" : "completed");
      setNotice(String(e));
    }
  };

  // ---- YAML editor ----
  const applyYaml = (text: string) => {
    setYamlText(text);
    try {
      setDefinition(parseDefinition(text));
      setParseError(null);
    } catch (e) {
      setParseError(e instanceof Error ? e.message : String(e));
    }
  };

  const resetYaml = () => applyYaml(DICTATION_YAML);

  const addStep = () => {
    const action = addStepAction || "dataset_get";
    const base = action.replace(/^dataset_/, "");
    const id = `${base}_${steps.length + 1}`;
    const skeleton = `\n  - id: ${id}\n    title: New step\n    action: ${action}\n    description: Describe what this step does.\n    depends_on: []\n`;
    applyYaml(yamlText.trimEnd() + "\n" + skeleton);
  };

  // ---- Job authoring: persist the current YAML as a run + load saved runs ----
  const loadRuns = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<{ runs: RunSummary[] }>("workflow_list_runs", {});
      setRuns(res.runs ?? []);
    } catch {
      setRuns([]);
    }
  }, []);

  useEffect(() => {
    if (mode === "edit") loadRuns();
  }, [mode, loadRuns]);

  const saveJob = async () => {
    if (!isTauri() || !newJobId.trim()) return;
    try {
      await invoke("workflow_create_run", { runId: newJobId.trim(), yamlText });
      setNotice(`Saved job “${newJobId.trim()}”.`);
      setNewJobId("");
      await loadRuns();
    } catch (e) {
      setNotice(`Save job failed: ${String(e)}`);
    }
  };

  const loadJob = async (id: string) => {
    if (!isTauri() || !id) return;
    try {
      const text = await invoke<string>("workflow_get_definition", { runId: id });
      applyYaml(text);
      setNotice(`Loaded job “${id}” into the editor.`);
    } catch (e) {
      setNotice(`Load job failed: ${String(e)}`);
    }
  };

  // ---- Agent prompt (unchanged behavior) ----
  const generatePrompt = () => {
    if (!facts) return;
    const text = buildPrompt({
      datasetName: selectedName,
      datasetUuid: selectedUuid,
      runId,
      view,
      steps,
      definition: { name: definition.name, version: definition.version },
    });
    setPrompt(text);
  };

  const sendToDock = async () => {
    if (!prompt.trim() || !isTauri()) return;
    try {
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

  const sel = selectedId ? stepById(selectedId, steps) : undefined;
  const selOp = sel ? getStepOp(sel.id, sel.action) : undefined;
  // Effective context for the selected node (defaults overlaid with edits), used
  // to compute the gate hint + the command label shown in the inspector.
  const selCtx: RunCtx | undefined = sel
    ? { ...buildCtx(sel.id), form: formValues(sel.id, selOp) }
    : undefined;
  const selHint = selCtx ? selOp?.gate?.(selCtx) : undefined;
  const selCommand = selCtx ? resolveCommand(selOp, selCtx) : undefined;
  const readyCount = Object.values(view).filter((s) => s === "ready").length;
  const doneCount = Object.values(view).filter((s) => s === "completed").length;

  return (
    <div className="flex flex-col w-full h-full min-h-0 p-4 gap-3">
      {/* Header */}
      <div className="flex items-center gap-3 shrink-0">
        <WorkflowIcon size={20} className="text-accent shrink-0" />
        <h1 className="text-[1.3em] font-bold">Workflow</h1>
        <span className="text-xs text-text-tertiary">
          Click a step to run it in-app (the same command an agent calls over MCP), or arrange a multi-step job.
        </span>
      </div>

      {!mounted ? null : !isTauri() ? (
        <p className="text-text-secondary">Workflow is only available in the desktop app.</p>
      ) : (
        <div className="flex-1 min-h-0 flex flex-col gap-3">
          {/* Toolbar */}
          <div className="flex items-center gap-3 flex-wrap shrink-0 rounded-lg border border-border-light bg-bg-card px-3 py-2">
            <span
              className="text-sm font-semibold text-text-primary px-2 py-1 rounded-md bg-bg-muted"
              title={`${definition.name} v${definition.version}`}
            >
              {definition.name}
              <span className="ml-1 text-[10px] font-normal text-text-tertiary">v{definition.version}</span>
            </span>
            <div className="inline-flex rounded-md border border-border-light overflow-hidden">
              <button
                className={`${mode === "view" ? btnSmPrimary : btnSmSecondary} inline-flex items-center gap-1 rounded-none border-0`}
                onClick={() => setMode("view")}
              >
                <Eye size={14} /> View
              </button>
              <button
                className={`${mode === "edit" ? btnSmPrimary : btnSmSecondary} inline-flex items-center gap-1 rounded-none border-0`}
                onClick={() => setMode("edit")}
              >
                <Pencil size={14} /> Edit
              </button>
            </div>
            <div className="ml-auto flex items-center gap-2">
              <button
                className={`${btnSmSecondary} inline-flex items-center gap-1`}
                onClick={handleReload}
                disabled={reloading}
                title="Re-read the dataset list from disk"
              >
                <FolderSync size={14} className={reloading ? "animate-spin" : undefined} />
                Reload
              </button>
              <select
                className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[220px]"
                value={selectedKey}
                onChange={(e) => { setSelectedKey(e.target.value); clearView(); setNotice(""); }}
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
                <button className={`${btnSmPrimary} inline-flex items-center gap-1`} onClick={handleInit} disabled={initing}>
                  <Plus size={14} className={initing ? "animate-spin" : undefined} />
                  {initing ? "Initializing…" : "Initialize as dataset"}
                </button>
              ) : (
                <button className={`${btnSmPrimary} inline-flex items-center gap-1`} onClick={scan} disabled={!selectedUuid || scanning}>
                  <RefreshCw size={14} className={scanning ? "animate-spin" : undefined} />
                  {facts ? "Re-scan" : "Scan status"}
                </button>
              )}
            </div>
          </div>

          {notice && (
            <div className="shrink-0 text-xs px-3 py-2 rounded-md bg-info-bg text-info-text border border-border-light flex items-center justify-between gap-2">
              <span>{notice}</span>
              <button className="text-text-tertiary hover:text-text-primary" onClick={() => setNotice("")} aria-label="Dismiss">
                <X size={13} />
              </button>
            </div>
          )}

          {mode === "edit" ? (
            <div className="flex-1 min-h-0 flex flex-col gap-3">
              <div className="flex items-center gap-2 flex-wrap shrink-0 rounded-lg border border-border-light bg-bg-card px-3 py-2">
                <span className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Jobs</span>
                <select
                  className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[200px]"
                  value=""
                  onChange={(e) => { if (e.target.value) loadJob(e.target.value); }}
                >
                  <option value="">Load a saved job…</option>
                  {runs.map((r) => (
                    <option key={r.run_id} value={r.run_id}>
                      {r.run_id}{r.name ? ` · ${r.name}` : ""}{r.error ? " · (invalid)" : ""}
                    </option>
                  ))}
                </select>
                <input
                  className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[180px]"
                  placeholder="new-job-id"
                  value={newJobId}
                  onChange={(e) => setNewJobId(e.target.value)}
                />
                <button className={`${btnSmPrimary} inline-flex items-center gap-1`} onClick={saveJob} disabled={!newJobId.trim()}>
                  <Save size={14} /> Save job
                </button>
                <div className="ml-auto flex items-center gap-2">
                  <label className="text-[10px] text-text-tertiary leading-tight">Add step<br />action</label>
                  <select
                    className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none min-w-[180px]"
                    value={addStepAction}
                    onChange={(e) => setAddStepAction(e.target.value)}
                    title="Choose the command twin the new step runs"
                  >
                    {KNOWN_COMMANDS.map((c) => (
                      <option key={c} value={c}>{c}</option>
                    ))}
                  </select>
                  <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={addStep} title="Append a step to the YAML">
                    <Plus size={14} /> Add step
                  </button>
                </div>
              </div>
              <div className="flex-1 min-h-0 flex gap-3">
                <div className="flex-1 min-w-0 flex flex-col rounded-lg border border-border-light bg-bg-card overflow-hidden">
                  <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-border-light shrink-0">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary inline-flex items-center gap-1">
                      <FileJson size={13} /> workflow.yaml
                    </h2>
                    <div className="flex items-center gap-2">
                      <span className="text-[10px] text-text-tertiary">{steps.length} step(s) · {parseError ? "last good graph" : "live"}</span>
                      <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={resetYaml}>
                        <RotateCcw size={13} /> Reset
                      </button>
                    </div>
                  </div>
                  {parseError && (
                    <div className="shrink-0 text-[11px] px-3 py-2 bg-error-bg text-error-text border-b border-border-light font-mono">{parseError}</div>
                  )}
                  <textarea
                    className="flex-1 min-h-0 w-full text-[12px] leading-relaxed font-mono p-3 bg-bg-body text-text-primary focus:outline-none resize-none"
                    value={yamlText}
                    onChange={(e) => applyYaml(e.target.value)}
                    spellCheck={false}
                  />
                </div>
                <div className="flex-1 min-w-0 rounded-lg border border-border-light overflow-hidden relative">
                  <div className="absolute left-2 top-2 z-10 text-[11px] px-2 py-1 rounded bg-bg-card/80 border border-border-light text-text-secondary">
                    Preview · {definition.name} v{definition.version}
                  </div>
                  <WorkflowGraph positioned={positioned} edges={edges} view={view} selectedId={selectedId} onSelect={(id) => setSelectedId(id || null)} />
                </div>
              </div>
            </div>
          ) : !facts ? (
            <div className="flex-1 min-h-0 flex items-center justify-center text-sm text-text-tertiary px-6 text-center">
              {isRaw
                ? "This folder has media but no info.json yet — click “Initialize as dataset” to create it."
                : selectedUuid
                  ? "Click “Scan status” to read the dataset and lay out the pipeline."
                  : "Select a dictation dataset above to view its workflow graph."}
            </div>
          ) : (
            <div className="flex-1 min-h-0 flex gap-3">
              {/* Graph */}
              <div className="flex-1 min-w-0 rounded-lg border border-border-light overflow-hidden relative">
                <div className="absolute left-2 top-2 z-10 flex items-center gap-2 text-[11px] px-2 py-1 rounded bg-bg-card/80 border border-border-light text-text-secondary">
                  <span>{doneCount} done · {readyCount} ready</span>
                  {runMeta && (
                    <span className="px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary">run: {runMeta.run_status}</span>
                  )}
                  {!hasRun && <span className="text-text-tertiary">· preview (no run yet)</span>}
                </div>
                <WorkflowGraph positioned={positioned} edges={edges} view={view} selectedId={selectedId} onSelect={(id) => setSelectedId(id || null)} />
              </div>

              {/* Inspector */}
              <div className="w-[440px] max-w-[55%] shrink-0 min-h-0 overflow-y-auto flex flex-col gap-3">
                <StepInspector
                  step={sel}
                  op={selOp}
                  status={sel ? (view[sel.id] ?? "pending") : undefined}
                  values={sel ? formValues(sel.id, selOp) : {}}
                  hint={selHint}
                  command={selCommand}
                  runnable={!!selOp?.run}
                  running={running}
                  runningStep={runningStep}
                  hasRun={hasRun}
                  onChangeField={setField}
                  onRun={handleRun}
                  onClear={handleClear}
                  onIntervene={intervene}
                  onForce={forceStatus}
                  onOpenModels={() => emit("agent-action", { type: "navigate", tab: "models" })}
                  pickDir={async (title) => {
                    const picked = await open({ directory: true, multiple: false, title });
                    return typeof picked === "string" ? picked : null;
                  }}
                />

                {/* Run output */}
                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Run output</h2>
                    <button className={`${btnSmSecondary}`} onClick={() => setOutputOpen((o) => !o)}>{outputOpen ? "Hide" : "Show"}</button>
                  </div>
                  {outputOpen ? (
                    outputLines.length ? (
                      <pre className="max-h-48 overflow-y-auto text-[11px] font-mono bg-bg-body border border-border-light rounded p-2 text-text-secondary whitespace-pre-wrap break-all">
                        {outputLines.join("\n")}
                      </pre>
                    ) : (
                      <p className="text-xs text-text-tertiary">No output yet. Running a step streams progress here.</p>
                    )
                  ) : (
                    <p className="text-xs text-text-tertiary">Progress + result of the last runs.</p>
                  )}
                </section>

                {/* info.json */}
                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary inline-flex items-center gap-1">
                      <FileJson size={13} /> info.json
                    </h2>
                    <button className={`${btnSmSecondary}`} onClick={() => setInfoOpen((o) => !o)}>{infoOpen ? "Hide" : "Edit"}</button>
                  </div>
                  {infoOpen ? (
                    <>
                      <textarea
                        className="w-full h-40 text-[11px] font-mono p-2 rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none resize-y"
                        value={infoText}
                        onChange={(e) => setInfoText(e.target.value)}
                        spellCheck={false}
                      />
                      <div className="flex items-center gap-2">
                        <button className={`${btnSmPrimary} inline-flex items-center gap-1`} onClick={saveInfo} disabled={savingInfo || !infoText.trim()}>
                          <Save size={14} /> {savingInfo ? "Saving…" : "Save"}
                        </button>
                        <span className="text-[10px] text-text-tertiary leading-tight">uuid / type / format / created_at are kept by the app</span>
                      </div>
                    </>
                  ) : (
                    <p className="text-xs text-text-tertiary leading-relaxed">View or edit this dataset’s metadata as raw JSON.</p>
                  )}
                </section>

                {/* Agent prompt */}
                <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Agent prompt</h2>
                    <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={generatePrompt}>
                      <WorkflowIcon size={13} /> Generate
                    </button>
                  </div>
                  {prompt ? (
                    <>
                      <textarea className="w-full h-40 text-[11px] font-mono p-2 rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none resize-none" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
                      <div className="flex gap-2">
                        <button className={`${btnSmPrimary} inline-flex items-center gap-1 flex-1 justify-center`} onClick={sendToDock}><Send size={14} /> Send to Agent Dock</button>
                        <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={copyPrompt}><Copy size={14} /> Copy</button>
                      </div>
                    </>
                  ) : (
                    <p className="text-xs text-text-tertiary leading-relaxed">
                      Generate a driver prompt from the current graph for goose to run the same steps via MCP. In-app Run and the agent share one implementation.
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

// ---------------------------------------------------------------------------
// Inspector (run panel): shows only the selected step's description, parameter
// form, Run / Clear, and the redo/skip/mark controls. The gate hint and command
// label are computed by the parent (which holds the dataset facts/uuid) and
// passed in, so the inspector stays free of the run context.
// ---------------------------------------------------------------------------
function resolveCommand(op: StepOp | undefined, ctx: RunCtx): string | undefined {
  try {
    return op?.run?.(ctx)?.command;
  } catch {
    return undefined;
  }
}

interface InspectorProps {
  step?: StepDef;
  op?: StepOp;
  status?: StepStatus;
  values: Record<string, string | boolean>;
  hint?: string;
  command?: string;
  runnable: boolean;
  running: boolean;
  runningStep: string;
  hasRun: boolean;
  onChangeField: (id: string, name: string, value: string | boolean) => void;
  onRun: (step: StepDef, op: StepOp) => void;
  onClear: (step: StepDef, op: StepOp) => void;
  onIntervene: (step: StepDef, op: "reset" | "skip") => void;
  onForce: (step: StepDef, event: "completed" | "failed") => void;
  onOpenModels: () => void;
  pickDir: (title: string) => Promise<string | null>;
}

function StepInspector({ step, op, status, values, hint, command, runnable, running, runningStep, onChangeField, onRun, onClear, onIntervene, onForce, onOpenModels, pickDir }: InspectorProps) {
  if (!step) {
    return (
      <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Step</h2>
        <p className="text-sm text-text-tertiary">Click a node to see its description, run it, or mark it redo / skip / done.</p>
      </section>
    );
  }

  const isRunning = running && runningStep === step.id;

  return (
    <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Step</h2>
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-text-primary truncate">{step.title}</span>
        <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary shrink-0">{statusLabel(status ?? "pending")}</span>
      </div>
      <div className="font-mono text-[11px] text-text-tertiary">{command ?? step.action}</div>
      <p className="text-xs text-text-secondary leading-relaxed">{step.description}</p>

      {/* Parameter form */}
      {op?.formFields?.length ? (
        <div className="flex flex-col gap-2 mt-1">
          {op.formFields.map((f) => (
            <FieldRow
              key={f.name}
              field={f}
              value={values[f.name] ?? (f.type === "checkbox" ? false : "")}
              disabled={running}
              onChange={(v) => onChangeField(step.id, f.name, v)}
              pickDir={pickDir}
            />
          ))}
        </div>
      ) : null}

      {/* ensure_model shortcut */}
      {step.id === "ensure_model" && (
        <button className={`${btnSmSecondary} inline-flex items-center gap-1 self-start`} onClick={onOpenModels}>
          <Play size={13} /> Open Models page
        </button>
      )}

      {/* Primary actions */}
      {runnable ? (
        <div className="flex flex-wrap items-center gap-2 mt-1">
          {hint ? (
            <span className="text-xs text-text-tertiary">{hint}</span>
          ) : (
            <button className={`${btnSmPrimary} inline-flex items-center gap-1`} onClick={() => onRun(step, op!)} disabled={running}>
              <Play size={14} className={isRunning ? "animate-spin" : undefined} /> {isRunning ? "Running…" : op?.runLabel ?? "Run"}
            </button>
          )}
          {op?.clear && (
            <button className={`${btnSmDanger} inline-flex items-center gap-1`} onClick={() => onClear(step, op!)} disabled={running} title="Delete this step's output">
              <Trash2 size={14} /> Clear
            </button>
          )}
        </div>
      ) : (
        <p className="text-xs text-text-tertiary mt-1">
          {step.id === "ensure_model" ? "Manage the STT model on the Models page, then mark this step done." : "This step has no in-app command twin — run it via the Agent prompt (MCP), or mark it manually."}
        </p>
      )}

      {/* Manual state controls */}
      <div className="flex flex-wrap gap-2 mt-2 pt-2 border-t border-border-light">
        <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={() => onIntervene(step, "reset")} title="Redo this step">
          <RotateCcw size={13} /> Redo
        </button>
        <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={() => onIntervene(step, "skip")} title="Skip this step">
          <SkipForward size={13} /> Skip
        </button>
        <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={() => onForce(step, "completed")} title="Mark done without running">
          <Check size={13} /> Mark done
        </button>
        <button className={`${btnSmDanger} inline-flex items-center gap-1`} onClick={() => onForce(step, "failed")} title="Mark this step failed">
          <X size={13} /> Mark failed
        </button>
      </div>
    </section>
  );
}

interface FieldRowProps {
  field: FormField;
  value: string | boolean;
  disabled?: boolean;
  onChange: (v: string | boolean) => void;
  pickDir: (title: string) => Promise<string | null>;
}

function FieldRow({ field, value, disabled, onChange, pickDir }: FieldRowProps) {
  if (field.type === "checkbox") {
    return (
      <label className="flex items-center gap-1.5 text-xs text-text-secondary cursor-pointer">
        <input type="checkbox" checked={!!value} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
        {field.label}
      </label>
    );
  }
  if (field.type === "radios") {
    return (
      <div className="flex flex-col gap-1 text-xs text-text-secondary">
        <span className="font-medium text-text-primary">{field.label}</span>
        <div className="flex flex-wrap gap-3">
          {field.options?.map((o) => (
            <label key={o.value} className="flex items-start gap-1.5 cursor-pointer">
              <input type="radio" name={field.name} className="mt-0.5" checked={value === o.value} disabled={disabled} onChange={() => onChange(o.value)} />
              <span>
                <span className="font-medium text-text-primary">{o.label}</span>
                {o.hint ? <span className="text-text-tertiary"> — {o.hint}</span> : null}
              </span>
            </label>
          ))}
        </div>
      </div>
    );
  }
  if (field.type === "select") {
    return (
      <label className="flex items-center gap-2 text-xs text-text-secondary">
        <span className="shrink-0">{field.label}:</span>
        <select className="px-1.5 py-0.5 text-xs rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none" value={String(value)} disabled={disabled} onChange={(e) => onChange(e.target.value)}>
          {field.options?.map((o) => (<option key={o.value} value={o.value}>{o.label}</option>))}
        </select>
      </label>
    );
  }
  if (field.type === "folder") {
    return (
      <div className="flex items-center gap-2">
        <input className="px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none flex-1 min-w-0" placeholder={field.placeholder} value={String(value)} disabled={disabled} onChange={(e) => onChange(e.target.value)} />
        <button className={`${btnSmSecondary} inline-flex items-center gap-1`} disabled={disabled} onClick={async () => { const p = await pickDir(field.label); if (p) onChange(p); }}>
          <FolderOpen size={14} /> Browse
        </button>
      </div>
    );
  }
  // text / textarea
  const cls = "px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none w-full";
  if (field.type === "textarea") {
    return (
      <label className="flex flex-col gap-1 text-xs text-text-secondary">
        <span>{field.label}</span>
        <textarea className={cls + " resize-y min-h-[48px]"} placeholder={field.placeholder} value={String(value)} disabled={disabled} onChange={(e) => onChange(e.target.value)} />
      </label>
    );
  }
  return (
    <label className="flex flex-col gap-1 text-xs text-text-secondary">
      <span>{field.label}</span>
      <input className={cls} placeholder={field.placeholder} value={String(value)} disabled={disabled} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}
