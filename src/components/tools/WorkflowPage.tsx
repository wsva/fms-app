"use client";

import { useState, useEffect, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetAudit,
  type DatasetInfo,
  type DatasetProgressEvt,
  type AuditCheck,
  btnSmPrimary,
  btnSmSecondary,
  btnSmDanger,
} from "@/lib/datasets/types";
import {
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
import {
  type VerifyReport,
  checksById,
  driftReason,
  findingAdvice,
  findingLabel,
  gateReason,
  stepsWithEvidence,
  verdictTitle,
} from "@/lib/workflow/verify";
import type { StepVerdictInfo } from "./WorkflowGraph";
import type { BookMeta } from "@/lib/read/types";
import type { CardDatasetSummary } from "@/lib/types";
import {
  RefreshCw, Workflow as WorkflowIcon, Play, RotateCcw, SkipForward, Check, Copy, Send, Plus,
  FileJson, Save, Eye, Pencil, FolderOpen, Trash2, X, ListChecks,
  Headphones, BookOpen, Layers, AlertTriangle, ChevronDown, ChevronRight, Info,
  type LucideIcon,
} from "lucide-react";

// react-flow touches browser layout APIs on mount; keep it out of the SSG pass.
const WorkflowGraph = dynamic(() => import("./WorkflowGraph"), { ssr: false });

function runIdFor(uuid: string): string {
  return `dictation-${uuid}`;
}

/**
 * List the datasets of one kind through its own command twin, flattened into the
 * shared row shape. Each kind is discovered by a different backend command
 * (`dataset_list` scans dictation roots, `book_list` the book roots, …) because
 * the on-disk layouts differ; only dictation may yield folders without info.json.
 */
async function listDatasetsOfKind(cat: WorkflowCategory): Promise<KindDataset[]> {
  if (cat === "dictation") {
    const res = await invoke<DatasetSummary[]>("dataset_list");
    return res
      .filter((d) => d.info.uuid !== "dictation-favorites")
      .map((d) => ({
        key: d.info.uuid || d.path,
        uuid: d.info.uuid,
        name: d.info.name,
        description: d.info.description,
        path: d.path,
        note: d.info.uuid ? `${d.media_count} media` : "folder · not imported",
        ready: d.status === "ready",
      }));
  }
  if (cat === "book") {
    const res = await invoke<BookMeta[]>("book_list");
    return res.map((b) => ({
      key: b.uuid,
      uuid: b.uuid,
      name: b.name,
      description: "",
      path: b.path,
      note: "",
      ready: true,
    }));
  }
  const res = await invoke<CardDatasetSummary[]>("card_dataset_list");
  return res.map((d) => ({
    key: d.info.uuid,
    uuid: d.info.uuid,
    name: d.info.name,
    description: d.info.description,
    path: d.path,
    note: `${d.card_count} cards`,
    ready: true,
  }));
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

/** The `adoption` block `workflow_adopt` appends to that same status. */
interface AdoptReport {
  adoption?: {
    adopted?: string[];
    /** The run was rebound to the definition text this page just passed it. */
    refreshed?: boolean;
    /** A passed definition that will not validate; the run kept its own. */
    refresh_error?: string;
  };
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

/** One entry from `workflow_builtin_templates`. */
interface BuiltinTemplate {
  /** Dataset type this workflow belongs to — the page tabs on this. */
  category: string;
  id: string;
  name: string;
  version: number;
  step_count: number;
  yaml: string;
}

/** Dataset kinds the Workflow page offers — one breadcrumb level 1 entry each. */
type WorkflowCategory = "dictation" | "book" | "card";

const KIND_ORDER: WorkflowCategory[] = ["dictation", "book", "card"];

const KIND_META: Record<WorkflowCategory, { label: string; blurb: string; icon: LucideIcon }> = {
  dictation: {
    label: "Dictation",
    blurb: "Import a raw folder, generate subtitles / waveforms / the cue DB, then align and adjust cue times.",
    icon: Headphones,
  },
  book: {
    label: "Book",
    blurb: "Create a book, author chapters / sentences, attach audio, build the vocabulary. Agent-facing.",
    icon: BookOpen,
  },
  card: {
    label: "Card",
    blurb: "Create a deck, author cards / tags, rebuild the full-text index, sync online. Agent-facing.",
    icon: Layers,
  },
};

/**
 * Breadcrumb trail position. Each field set adds one level: nothing picked =
 * the kind list, `cat` = that kind's datasets, `datasetKey` = the dataset's
 * templates, `templateId` = the workflow itself (graph + run panels).
 */
interface Nav {
  cat: WorkflowCategory | "";
  datasetKey: string;
  templateId: string;
}

const ROOT_NAV: Nav = { cat: "", datasetKey: "", templateId: "" };

/** Any dataset, flattened into one row shape so every kind shares the list UI. */
interface KindDataset {
  /** Selection key: the uuid, or the folder path for an un-imported dictation dir. */
  key: string;
  /** "" when the folder has no info.json yet (raw dictation dir). */
  uuid: string;
  name: string;
  /** info.json description ("" for kinds that do not carry one). */
  description: string;
  path: string;
  /** Right-hand summary (media / card counts) or a status note. */
  note: string;
  ready: boolean;
}

/** The disk-probed facts, in display order, for the Scan result badges. */
const FACT_LABELS: Array<[keyof DatasetFacts, string]> = [
  ["hasMedia", "Media"],
  ["hasSubtitles", "Subtitles"],
  ["hasWaveforms", "Waveforms"],
  ["hasDatabase", "Cue DB"],
  ["hasBook", "Book text"],
  ["hasTranscript", "Transcript"],
];

/** Severity → glyph and colour, so the report can be read at a glance. */
const AUDIT_LEVEL_STYLE: Record<AuditCheck["level"], { icon: LucideIcon; cls: string }> = {
  problem: { icon: AlertTriangle, cls: "text-error-text" },
  warn: { icon: AlertTriangle, cls: "text-warning-text" },
  info: { icon: Info, cls: "text-info-text" },
  clean: { icon: Check, cls: "text-success-text" },
};

// Wiki-style bar chrome: full-width stacked bars, ghost buttons that only
// change background on hover (see WikiPage / DictationPage for the same pattern).
const toolBtn =
  "flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors shrink-0 text-text-secondary hover:text-text-primary hover:bg-bg-hover disabled:opacity-50 disabled:cursor-not-allowed";
const toolBtnActive =
  "flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors shrink-0 bg-accent text-white hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed";
const iconBtn =
  "p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default";

/** Placeholder document shown until the built-in template finishes loading. */
const EMPTY_DEFINITION: WorkflowDefinition = { name: "workflow", version: 1, steps: [] };

export default function WorkflowPage() {
  const [mounted, setMounted] = useState(false);
  // Breadcrumb drill-down position. The crumbs themselves are the navigation
  // controls (wiki model): jumping to any ancestor level replaces going back.
  const [nav, setNav] = useState<Nav>(ROOT_NAV);

  // Datasets of the kind being browsed, cached per kind so switching back is
  // instant. `datasets` below is the active slice.
  const [kindDatasets, setKindDatasets] = useState<Partial<Record<WorkflowCategory, KindDataset[]>>>({});
  const [loadingDatasets, setLoadingDatasets] = useState(false);

  const [facts, setFacts] = useState<DatasetFacts | null>(null);
  // The full audit the scan produced: per-check findings for the Scan result panel.
  const [audit, setAudit] = useState<DatasetAudit | null>(null);
  // The same probe read through the run's `verify:` bindings (`workflow_verify`):
  // which step each finding speaks for. A projection over the audit + the run's
  // statuses — never stored, and stale the moment either changes.
  const [verifyReport, setVerifyReport] = useState<VerifyReport | null>(null);
  // Local projection placeholder used only until a run exists on disk.
  const [committed, setCommitted] = useState<StatusMap>(() => initialStatuses([]));
  // Authoritative engine statuses once a run exists (drives the graph + inspector).
  const [engineView, setEngineView] = useState<StatusMap | null>(null);
  const [runMeta, setRunMeta] = useState<{ name: string; version: number; run_status: string } | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Workflow document is YAML-driven; `definition` = last-good parse, `parseError` = live.
  const [mode, setMode] = useState<"view" | "edit">("view");
  // Built-in templates fetched once from the Rust binary (see `workflow/templates.rs`),
  // grouped by category. The breadcrumb's kind level selects the category, and the
  // dataset page lists every template of that category to pick from.
  const [templates, setTemplates] = useState<BuiltinTemplate[]>([]);
  const [builtinYaml, setBuiltinYaml] = useState("");
  const [yamlText, setYamlText] = useState("");
  const [definition, setDefinition] = useState<WorkflowDefinition>(EMPTY_DEFINITION);
  const [parseError, setParseError] = useState<string | null>(null);
  const steps = definition.steps;
  // Only Dictation is wired for in-app per-step Run; Book / Card are view/agent-facing.
  const isDictation = nav.cat === "dictation";

  const [scanning, setScanning] = useState(false);
  const [initing, setIniting] = useState(false);
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

  // Which breadcrumb level the content area renders. Derived from how many of the
  // three nav fields are filled, so the trail and the view can never disagree.
  const viewLevel: "kinds" | "datasets" | "templates" | "workflow" =
    !nav.cat ? "kinds" : !nav.datasetKey ? "datasets" : !nav.templateId ? "templates" : "workflow";

  const positioned = useMemo(() => layoutSteps(steps), [steps]);
  const edges = useMemo(() => buildEdges(steps), [steps]);
  const kindTemplates = useMemo(
    () => templates.filter((t) => t.category === nav.cat),
    [templates, nav.cat],
  );
  const activeTemplate = useMemo(
    () => templates.find((t) => t.id === nav.templateId),
    [templates, nav.templateId],
  );
  const view: StatusMap = useMemo(
    () => engineView ?? (facts ? recompute(committed, facts, steps) : committed),
    [engineView, committed, facts, steps],
  );

  // Check id → its audit row, so every label and advice sentence below stays the
  // report's own words instead of a second copy in the page.
  const auditById = useMemo(() => checksById(audit), [audit]);
  // Node badges: one per step the probe actually judged. `unknown` earns no glyph,
  // so it is left out here rather than passed and hidden in the node.
  const nodeVerdicts = useMemo(() => {
    const out: Record<string, StepVerdictInfo> = {};
    if (!verifyReport) return out;
    for (const [id, entry] of Object.entries(verifyReport.verdicts ?? {})) {
      if (entry.verdict === "unknown") continue;
      out[id] = { verdict: entry.verdict, title: verdictTitle(id, verifyReport, auditById) };
    }
    return out;
  }, [verifyReport, auditById]);
  // The edit-mode graph shows the in-editor document, while the verdicts were scored
  // against the definition saved with the run. Overlay them only while the two are
  // the same text — otherwise a badge would vouch for an unverified edit.
  const previewVerdicts = yamlText === builtinYaml ? nodeVerdicts : undefined;

  const datasets = nav.cat ? kindDatasets[nav.cat] ?? [] : [];
  const selectedDataset = useMemo(
    () => datasets.find((d) => d.key === nav.datasetKey),
    [datasets, nav.datasetKey],
  );
  const selectedUuid = selectedDataset?.uuid ?? "";
  const selectedPath = selectedDataset?.path ?? "";
  const isRaw = !!selectedDataset && !selectedDataset.uuid;
  const selectedName = selectedDataset?.name ?? "";
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

  // ---- Breadcrumb navigation ----
  // Every user-initiated level change goes through `navigate`, so the trail and
  // the content are always driven by the same `nav` state.
  const navigate = useCallback((next: Nav) => setNav(next), []);

  // ---- Built-in template loading / switching ----
  // Apply a template's YAML as the working document (resets the graph to a
  // clean all-pending projection) and return the parsed definition, so callers
  // that navigate into the workflow level can probe with the right step list
  // before `definition` state has committed.
  const loadTemplateDoc = (tpl: BuiltinTemplate): WorkflowDefinition | null => {
    setBuiltinYaml(tpl.yaml);
    setYamlText(tpl.yaml);
    try {
      const def = parseDefinition(tpl.yaml);
      setDefinition(def);
      setCommitted(initialStatuses(def.steps));
      setParseError(null);
      return def;
    } catch (e) {
      setParseError(e instanceof Error ? e.message : String(e));
      return null;
    }
  };

  // Drop the dataset-scoped run projection (facts / engine view / prompt / output)
  // without touching `committed` — the caller sets that via loadTemplateDoc.
  const resetRunView = () => {
    setFacts(null);
    setAudit(null);
    setVerifyReport(null);
    setEngineView(null);
    setRunMeta(null);
    setSelectedId(null);
    setPrompt("");
    setInfoText("");
    setInfoOpen(false);
    setOutputLines([]);
  };

  // Level 1 → 2: pick a dataset kind. Always re-reads disk, so datasets imported
  // through another page (or by an agent) show up when you drill back in.
  const openKind = (cat: WorkflowCategory) => {
    resetRunView();
    setMode("view");
    navigate({ cat, datasetKey: "", templateId: "" });
    void loadKindDatasets(cat, true);
  };

  // Level 2 → 3: pick a dataset, which lists the workflows of its kind.
  const openDataset = (key: string) => {
    resetRunView();
    setMode("view");
    navigate({ ...nav, datasetKey: key, templateId: "" });
  };

  // Level 3 → 4: pick a workflow. The dictation pipeline is probed right away so
  // the graph opens on real disk facts; Book / Card stay a read-only preview.
  const openTemplate = (tpl: BuiltinTemplate) => {
    resetRunView();
    setMode("view");
    const def = loadTemplateDoc(tpl);
    navigate({ ...nav, templateId: tpl.id });
    const ds = (kindDatasets[tpl.category as WorkflowCategory] ?? []).find((d) => d.key === nav.datasetKey);
    if (tpl.category === "dictation" && ds?.uuid && def) void probeAndSync(ds.uuid, def.steps, tpl.yaml);
  };

  // Breadcrumb crumbs: jump back one level at a time, always clearing the run
  // projection because a different scope means a different run on disk.
  const goToKinds = () => { resetRunView(); setMode("view"); navigate(ROOT_NAV); };
  const goToDatasets = () => { resetRunView(); setMode("view"); navigate({ ...nav, datasetKey: "", templateId: "" }); };
  const goToTemplates = () => { resetRunView(); setMode("view"); navigate({ ...nav, templateId: "" }); };

  const copyYaml = async () => {
    try {
      await navigator.clipboard.writeText(yamlText);
      setNotice("workflow.yaml copied to clipboard.");
    } catch {
      setNotice("Copy failed.");
    }
  };

  // ---- Load every built-in template once (a dataset page lists its kind's workflows) ----
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke<{ templates: BuiltinTemplate[] }>("workflow_builtin_templates")
      .then((res) => {
        if (cancelled) return;
        setTemplates(res.templates);
        // Seed the YAML editor with the default pipeline. The breadcrumb still
        // starts at the kind list, so nothing is opened on the user's behalf.
        const initial =
          res.templates.find((t) => t.category === "dictation") ?? res.templates[0];
        if (initial) loadTemplateDoc(initial);
      })
      .catch((e) => appendLog(`workflow_builtin_templates failed: ${String(e)}`, "ERROR"));
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appendLog]);

  // ---- Dataset list for the kind being browsed ----
  const loadKindDatasets = useCallback(async (
    cat: WorkflowCategory,
    force = false,
  ): Promise<KindDataset[] | null> => {
    if (!isTauri()) return null;
    // Cached unless an explicit refresh is asked for, so revisiting a level does
    // not re-scan the disk each time.
    if (!force && kindDatasets[cat]) return kindDatasets[cat] ?? null;
    setLoadingDatasets(true);
    try {
      const list = await listDatasetsOfKind(cat);
      setKindDatasets((prev) => ({ ...prev, [cat]: list }));
      return list;
    } catch (e) {
      appendLog(`dataset list (${cat}) failed: ${String(e)}`, "ERROR");
      setNotice(`Failed to list ${KIND_META[cat].label} datasets: ${String(e)}`);
      return null;
    } finally {
      setLoadingDatasets(false);
    }
  }, [kindDatasets, appendLog]);

  // MCP / sync operations import datasets without going through this page, so
  // track the `dataset-list-changed` event rather than relying on a manual button.
  useEffect(() => {
    if (!isTauri() || !nav.cat) return;
    const cat = nav.cat;
    let unlisten: (() => void) | undefined;
    listen("dataset-list-changed", () => { void loadKindDatasets(cat, true); })
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { if (unlisten) unlisten(); };
  }, [nav.cat, loadKindDatasets]);

  // ---- Engine status pull (authoritative once a run exists) ----
  // Dataset-scoped: a run lives inside its dataset's `workflows/` dir, so the
  // dataset uuid selects the store. (Workspace-level editor "jobs" are handled
  // separately by loadRuns/saveJob/loadJob and pass no uuid.)
  const refreshEngine = useCallback(async (id: string, datasetUuid: string): Promise<boolean> => {
    // Any status movement invalidates the last verdicts: they were scored against
    // the state this call is about to replace. They come back with the next scan.
    setVerifyReport(null);
    if (!isTauri() || !id) {
      setEngineView(null);
      setRunMeta(null);
      return false;
    }
    try {
      const st = await invoke<EngineStatus>("workflow_status", { datasetUuid, runId: id });
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

  // ---- Score the run against the evidence (the binding made visible) ----
  // One join, in Rust: this run's `workflow.yaml` `verify:` blocks × the audit's
  // per-check counts × the step statuses. The page only renders the verdicts, so
  // which check speaks for which step is never restated here. No run, no verdicts
  // — a binding needs a claim about progress to be read against.
  const verifyRun = useCallback(async (id: string, datasetUuid: string): Promise<VerifyReport | null> => {
    setVerifyReport(null);
    if (!isTauri() || !id || !datasetUuid) return null;
    try {
      const report = await invoke<VerifyReport>("workflow_verify", { datasetUuid, runId: id });
      setVerifyReport(report);
      return report;
    } catch (e) {
      // Not a scan failure: the run does not exist yet, or its stored definition
      // predates the binding. Say nothing rather than something vacuous.
      appendLog(`workflow_verify('${id}'): ${String(e)}`);
      return null;
    }
  }, [appendLog]);

  // ---- Inherit progress the evidence already proves ----
  // A dataset initialized before this page ever saw it has no run, so its graph
  // would say "not done" about work that is plainly done. `workflow_adopt` scores
  // the same audit evidence against the steps that *opt in* to adopting (`adopt:`
  // in the definition — the agreement steps `init_dataset`/`sync_media` and the
  // artifact steps, each guarded against a vacuous zero) and records those as
  // completed, opening the run first when the dataset has none. It writes an
  // `adopted` event, not a completion, so a green node is still traceable to
  // "inherited from the disk" rather than to work the run performed. A run that
  // only ever inherited its progress also follows the current template, so a
  // binding added after it was opened still applies to it.
  const adoptRun = useCallback(
    async (id: string, datasetUuid: string, yaml: string): Promise<{ adopted: string[]; refreshed: boolean }> => {
      if (!isTauri() || !id || !datasetUuid || !yaml) return { adopted: [], refreshed: false };
      try {
        const res = await invoke<AdoptReport>("workflow_adopt", { datasetUuid, runId: id, yamlText: yaml });
        if (res.adoption?.refresh_error) {
          // The scan still ran against the run's own definition; say why it did not
          // follow the text just passed to it.
          appendLog(`workflow_adopt('${id}') kept its definition: ${res.adoption.refresh_error}`);
        }
        return { adopted: res.adoption?.adopted ?? [], refreshed: !!res.adoption?.refreshed };
      } catch (e) {
        // Not a scan failure: no check provider for this kind, or a definition that
        // will not validate. The audit's own findings below stand without it.
        appendLog(`workflow_adopt('${id}'): ${String(e)}`);
        return { adopted: [], refreshed: false };
      }
    },
    [appendLog],
  );

  // ---- Probe on-disk facts + read info.json (does not touch the engine) ----
  // One command does the whole reconciliation: `dataset_audit` walks media/,
  // subtitle/, waveform/ and transcript/ against the rows in data.sqlite3 and
  // returns the guards' facts, info.json and every finding it made. Attributing a
  // finding to a step is a property of the workflow definition, not of this probe,
  // so that reading comes from `verifyRun` on top of it.
  const probeDataset = useCallback(async (uuid: string): Promise<DatasetAudit> => {
    const report = await invoke<DatasetAudit>("dataset_audit", { uuid });
    setFacts({
      hasMedia: report.facts.has_media,
      hasSubtitles: report.facts.has_subtitles,
      hasDatabase: report.facts.has_database,
      hasWaveforms: report.facts.has_waveforms,
      hasBook: report.facts.has_book,
      hasTranscript: report.facts.has_transcript,
    });
    setAudit(report);
    setInfoText(JSON.stringify(report.info, null, 2));
    return report;
  }, []);

  // Probe the dataset's on-disk facts, reset the local projection, then pull the
  // run state from the engine. Explicit args because this also runs while
  // navigating into the workflow level, before the new `definition`/`nav` state
  // is visible to a closure.
  const probeAndSync = useCallback(async (uuid: string, defs: StepDef[], yaml: string) => {
    if (!isTauri() || !uuid) return;
    setScanning(true);
    setNotice("");
    try {
      const report = await probeDataset(uuid);
      setCommitted(initialStatuses(defs));
      // Adopt before reading the state back, so the graph opens on the state the
      // dataset is in rather than on the absence of a record of it.
      const { adopted, refreshed } = await adoptRun(runIdFor(uuid), uuid, yaml);
      const live = await refreshEngine(runIdFor(uuid), uuid);
      // Verdicts only exist for a run that is on disk; a preview has no claims to check.
      const verdicts = live ? await verifyRun(runIdFor(uuid), uuid) : null;
      // Lead with what needs doing: a scan that only says "done" is not actionable.
      const blocking = report.checks.filter((c) => c.level === "problem").length;
      const drift = report.checks.filter((c) => c.level === "warn").length;
      const contradicted = verdicts?.verification?.drifted?.length ?? 0;
      const message =
        blocking || drift
          ? contradicted
            ? `Audit found ${blocking} blocking and ${drift} warning-level issue(s), and ${contradicted} completed step(s) are contradicted by the evidence their own verify: bindings name. Re-scan after fixing, or let an agent invalidate them.`
            : `Audit found ${blocking} blocking and ${drift} warning-level issue(s) — the Scan result panel names the files and groups them under the step its verify: bindings hold responsible.`
          : live
            ? "Audit is clean. The graph reflects the workflow run state."
            : "Audit is clean. Run any step to start its workflow run.";
      // Steps whose outcome was already on disk get named, because a tick the run
      // did not earn has to say where it came from.
      const inherited = adopted.length
        ? ` Adopted ${adopted.join(", ")} from evidence already on disk — they need not be run.`
        : "";
      // Rebinding rewrote the run's own workflow.yaml, which the page must not do
      // silently — even when it is the honest response to a template that moved on.
      const rebound = refreshed
        ? " The run held only inherited progress, so it followed the current template."
        : "";
      // A run that could not be scored has to say so: a missing badge is otherwise
      // indistinguishable from a clean bill of health.
      setNotice(
        live && !verdicts
          ? `${message}${inherited}${rebound} The run could not be scored against its verify: bindings (see the log).`
          : `${message}${inherited}${rebound}`,
      );
    } catch (e) {
      appendLog(`scan failed: ${String(e)}`, "ERROR");
      setNotice(`Scan failed: ${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, [probeDataset, adoptRun, refreshEngine, verifyRun, appendLog]);

  const scan = useCallback(() => {
    void probeAndSync(selectedUuid, steps, yamlText);
  }, [probeAndSync, selectedUuid, steps, yamlText]);

  // ---- Initialize a raw folder in place, then keep browsing it as a dataset ----
  // Drops back to the dataset's workflow list: the folder now has a uuid, so the
  // next template pick probes it like any other dictation dataset.
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
      await loadKindDatasets("dictation", true);
      await emit("dataset-list-changed", {});
      resetRunView();
      navigate({ cat: "dictation", datasetKey: summary.info.uuid, templateId: "" });
      setNotice(`Initialized dataset “${summary.info.name}” (uuid ${summary.info.uuid}). Pick a workflow to continue.`);
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

  // Re-read the current kind's dataset list from disk. A dictation folder without
  // info.json is keyed by its path, so a folder imported elsewhere only turns up
  // after such a scan; if the selected one gained a uuid in the meantime, follow
  // it so the breadcrumb keeps pointing at the same dataset.
  const refreshKindDatasets = useCallback(async () => {
    if (!isTauri() || !nav.cat) return;
    const cat = nav.cat;
    const prev = selectedDataset;
    const list = await loadKindDatasets(cat, true);
    if (!list) return;
    if (!prev || list.some((d) => d.key === prev.key)) {
      setNotice(`Reloaded ${list.length} ${KIND_META[cat].label} dataset(s) from disk.`);
      return;
    }
    resetRunView();
    const renamed = list.find((d) => d.path === prev.path && d.uuid);
    if (!renamed) {
      navigate({ cat, datasetKey: "", templateId: "" });
      setNotice("The selected dataset is no longer on disk — pick another one.");
      return;
    }
    navigate({ cat, datasetKey: renamed.key, templateId: "" });
    setNotice(`“${renamed.name}” was imported since you opened it — its uuid is now ${renamed.uuid}.`);
  }, [nav, selectedDataset, loadKindDatasets, navigate]);

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
    path: selectedPath,
    name: selectedDataset?.name ?? "",
    description: selectedDataset?.description ?? "",
    facts,
    form: formState[stepId] ?? {},
  }), [selectedUuid, selectedPath, selectedDataset, facts, formState]);

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
      await invoke("workflow_create_run", { datasetUuid: selectedUuid, runId, yamlText });
    } catch (e) {
      // Likely already exists — refreshEngine below confirms; a genuine failure
      // surfaces when the caller tries to advance.
      appendLog(`workflow_create_run('${runId}'): ${String(e)}`, "WARN");
    }
    await refreshEngine(runId, selectedUuid);
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
      await invoke("workflow_advance", { datasetUuid: selectedUuid, runId: id, step: step.id, agentId: "ui" });
    } catch (e) {
      setNotice(String(e));
      setRunning(false);
      setRunningStep("");
      await refreshEngine(id, selectedUuid);
      return;
    }
    try {
      const { command, args } = op.run!(ctx);
      const result = await invoke(command, args);
      pushOutput(summarizeResult(result));
      await invoke("workflow_record", {
        datasetUuid: selectedUuid,
        runId: id,
        step: step.id,
        event: "completed",
        detail: { outputs: op.outputs?.(result) ?? {} },
      });
      setNotice(`“${step.title}” completed.`);
      if (op.emitListChanged) await emit("dataset-list-changed", {});
      if (op.reloadAfter && selectedUuid) await probeDataset(selectedUuid);
      await refreshEngine(id, selectedUuid);
    } catch (e) {
      pushOutput(`Error: ${String(e)}`);
      try {
        await invoke("workflow_record", { datasetUuid: selectedUuid, runId: id, step: step.id, event: "failed", detail: { reason: String(e) } });
      } catch { /* ignore */ }
      setNotice(`“${step.title}” failed: ${String(e)}`);
      await refreshEngine(id, selectedUuid);
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
        try { await invoke("workflow_intervene", { datasetUuid: selectedUuid, runId, step: step.id, op: "reset" }); } catch { /* ignore */ }
      } else {
        setStep(step.id, "pending");
      }
      await probeDataset(selectedUuid);
      await refreshEngine(runId, selectedUuid);
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
        await invoke("workflow_intervene", { datasetUuid: selectedUuid, runId, step: step.id, op });
        await refreshEngine(runId, selectedUuid);
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
    try { await invoke("workflow_advance", { datasetUuid: selectedUuid, runId: id, step: step.id, agentId: "ui" }); } catch { /* not ready */ }
    try {
      await invoke("workflow_record", {
        datasetUuid: selectedUuid,
        runId: id, step: step.id, event,
        detail: event === "failed" ? { reason: "marked failed in UI" } : { outputs: {} },
      });
      await refreshEngine(id, selectedUuid);
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

  const resetYaml = () => applyYaml(builtinYaml);

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
  // The inspector's advisory line. Evidence outranks convention: a reason joined
  // from *this* dataset's findings is more specific than the sentence step-ops
  // writes for every dataset. Drift first — a step the run thinks is done but the
  // probe contradicts is more urgent than one whose inputs are still missing.
  const selFactGate = selCtx ? selOp?.gate?.(selCtx) : undefined;
  const selHint = sel
    ? driftReason(sel.id, verifyReport, auditById) ?? gateReason(sel.id, verifyReport, auditById) ?? selFactGate
    : selFactGate;
  const selCommand = selCtx ? resolveCommand(selOp, selCtx) : undefined;
  const readyCount = Object.values(view).filter((s) => s === "ready").length;
  const doneCount = Object.values(view).filter((s) => s === "completed").length;

  // Step titles per template, for the one-line pipeline summary on the dataset page.
  const templateStepTitles = useMemo(() => {
    const map: Record<string, string[]> = {};
    for (const t of templates) {
      try {
        map[t.id] = parseDefinition(t.yaml).steps.map((s) => s.title || s.id);
      } catch {
        map[t.id] = [];
      }
    }
    return map;
  }, [templates]);

  const kindLabel = nav.cat ? KIND_META[nav.cat].label : "";
  const toolbarLabel =
    viewLevel === "kinds" ? `${templates.length} built-in workflow(s) · pick a dataset kind`
      : viewLevel === "datasets" ? (loadingDatasets ? "Loading datasets…" : `${datasets.length} ${kindLabel.toLowerCase()} dataset(s)`)
        : viewLevel === "templates" ? `${kindTemplates.length} workflow template(s) for “${selectedName}”`
          : `${definition.name} v${definition.version} · ${steps.length} step(s)`;

  return (
    <div className="flex flex-col w-full h-full min-h-0 bg-bg-body min-w-0">
      {!mounted ? null : !isTauri() ? (
        <p className="p-4 text-text-secondary">Workflow is only available in the desktop app.</p>
      ) : (
        <>
        {/* Breadcrumb navigation — the top bar of this page: the "Workflow" crumb
            is the title. Trail: Workflow › kind › dataset › workflow. */}
        <nav className="shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
          <button
            className={`shrink-0 cursor-pointer hover:underline text-left ${viewLevel === "kinds" ? "text-text-primary font-medium" : "text-accent"}`}
            onClick={goToKinds}
          >
            Workflow
          </button>
          {nav.cat && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              <button
                className={`shrink-0 max-w-[240px] cursor-pointer hover:underline truncate text-left ${viewLevel === "datasets" ? "text-text-primary font-medium" : "text-accent"}`}
                onClick={goToDatasets}
                title={kindLabel}
              >
                {kindLabel}
              </button>
            </>
          )}
          {selectedDataset && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              <button
                className={`cursor-pointer hover:underline truncate min-w-0 text-left ${viewLevel === "templates" ? "flex-1 text-text-primary font-medium" : "shrink-0 max-w-[240px] text-accent"}`}
                onClick={goToTemplates}
                title={selectedDataset.path}
              >
                {selectedDataset.name}
              </button>
            </>
          )}
          {viewLevel === "workflow" && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              <span className="text-text-primary font-medium truncate min-w-0 flex-1" title={activeTemplate?.name ?? definition.name}>
                {activeTemplate?.name ?? definition.name}
              </span>
            </>
          )}
        </nav>

        {/* Sub-toolbar — the contextual actions of the current level. Going back is
            the breadcrumb's job, so there is no Back / Forward pair here. The old
            Reload button lives on as a disk re-scan on the two dataset-listing
            levels; the workflow level scans one dataset instead. */}
        <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
          {(viewLevel === "datasets" || viewLevel === "templates") && (
            <button
              onClick={() => void refreshKindDatasets()}
              disabled={loadingDatasets}
              className={iconBtn}
              title="Re-read the dataset list from disk"
            >
              <RefreshCw size={16} className={loadingDatasets ? "animate-spin" : undefined} />
            </button>
          )}

          <div className="text-xs text-text-tertiary truncate flex-1 min-w-0">{toolbarLabel}</div>

          {isDictation && selectedDataset && (
            isRaw ? (
              <button className={toolBtnActive} onClick={handleInit} disabled={initing} title="Write an info.json with a new uuid into this folder">
                <Plus size={14} className={initing ? "animate-spin" : undefined} />
                {initing ? "Initializing…" : "Initialize as dataset"}
              </button>
            ) : viewLevel === "workflow" ? (
              <button className={toolBtnActive} onClick={scan} disabled={!selectedUuid || scanning} title="Probe the dataset on disk and re-read the run state">
                <RefreshCw size={14} className={scanning ? "animate-spin" : undefined} />
                {facts ? "Re-scan" : "Scan status"}
              </button>
            ) : null
          )}

          {viewLevel === "workflow" && (
            <div className="flex items-center gap-1 shrink-0">
              <button className={mode === "view" ? toolBtnActive : toolBtn} onClick={() => setMode("view")} title="Graph and run panels">
                <Eye size={14} /> View
              </button>
              <button className={mode === "edit" ? toolBtnActive : toolBtn} onClick={() => setMode("edit")} title="Edit the workflow YAML / author a job">
                <Pencil size={14} /> Edit
              </button>
            </div>
          )}
        </div>

        {/* Level content */}
        <div className="flex-1 min-h-0 flex flex-col gap-3 p-4 overflow-hidden">

          {notice && (
            <div className="shrink-0 text-xs px-3 py-2 rounded-md bg-info-bg text-info-text border border-border-light flex items-center justify-between gap-2">
              <span>{notice}</span>
              <button className="text-text-tertiary hover:text-text-primary" onClick={() => setNotice("")} aria-label="Dismiss">
                <X size={13} />
              </button>
            </div>
          )}

          {viewLevel === "kinds" ? (
            <div className="flex-1 min-h-0 overflow-y-auto grid gap-2 content-start sm:grid-cols-2 lg:grid-cols-3">
              {KIND_ORDER.map((cat) => {
                const meta = KIND_META[cat];
                const Icon = meta.icon;
                const count = templates.filter((t) => t.category === cat).length;
                return (
                  <button
                    key={cat}
                    className="text-left p-4 border border-border-default rounded-lg transition-colors hover:bg-bg-hover cursor-pointer"
                    onClick={() => openKind(cat)}
                    title={`Browse ${meta.label} datasets`}
                  >
                    <div className="flex items-center gap-2">
                      <Icon size={16} className="shrink-0 text-accent" />
                      <span className="font-medium text-text-primary truncate">{meta.label}</span>
                      <span className="ml-auto text-xs text-text-tertiary shrink-0">{count} workflow{count === 1 ? "" : "s"}</span>
                    </div>
                    <div className="text-xs text-text-tertiary leading-relaxed mt-1">{meta.blurb}</div>
                  </button>
                );
              })}
            </div>
          ) : viewLevel === "datasets" ? (
            <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-2">
              {loadingDatasets && datasets.length === 0 ? (
                <p className="text-text-tertiary">Loading…</p>
              ) : datasets.length === 0 ? (
                <p className="text-text-secondary">
                  No {kindLabel.toLowerCase()} dataset was found in the configured locations. Import one first, then refresh this list.
                </p>
              ) : (
                datasets.map((ds) => (
                  <button
                    key={ds.key}
                    className="text-left p-4 border border-border-default rounded-lg transition-colors hover:bg-bg-hover cursor-pointer"
                    onClick={() => openDataset(ds.key)}
                    title={ds.path}
                  >
                    <div className="flex items-center gap-2">
                      <FolderOpen size={16} className={`shrink-0 ${ds.ready ? "text-accent" : "text-text-tertiary"}`} />
                      <span className="font-medium text-text-primary truncate">{ds.name}</span>
                      {!ds.ready && (
                        <span className="text-xs px-2 py-[0.2em] rounded-full bg-bg-muted text-text-secondary shrink-0">
                          {ds.uuid ? "not ready" : "not imported"}
                        </span>
                      )}
                      <span className="ml-auto text-xs text-text-tertiary shrink-0">{ds.note}</span>
                    </div>
                    <div className="text-xs text-text-tertiary truncate mt-1" title={ds.path}>{ds.path}</div>
                  </button>
                ))
              )}
            </div>
          ) : viewLevel === "templates" ? (
            <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-2">
              {isDictation && isRaw && (
                <div className="shrink-0 text-xs px-3 py-2 rounded-md bg-info-bg text-info-text border border-border-light">
                  This folder has media but no info.json yet — use “Initialize as dataset” in the toolbar, then pick a workflow.
                </div>
              )}
              {kindTemplates.length === 0 ? (
                <p className="text-text-secondary">No built-in workflow ships for the {kindLabel} kind yet.</p>
              ) : (
                kindTemplates.map((t) => (
                  <button
                    key={t.id}
                    className="text-left p-4 border border-border-default rounded-lg transition-colors hover:bg-bg-hover cursor-pointer"
                    onClick={() => openTemplate(t)}
                    title={`${t.name} — open the pipeline for ${selectedName}`}
                  >
                    <div className="flex items-center gap-2">
                      <ListChecks size={16} className="shrink-0 text-accent" />
                      <span className="font-medium text-text-primary truncate">{t.name}</span>
                      <span className="text-xs text-text-tertiary shrink-0">v{t.version}</span>
                      <span className="ml-auto text-xs text-text-tertiary shrink-0">{t.step_count} step{t.step_count === 1 ? "" : "s"}</span>
                    </div>
                    <div className="text-xs text-text-tertiary mt-1 line-clamp-2">
                      {(templateStepTitles[t.id] ?? []).join(" › ")}
                    </div>
                  </button>
                ))
              )}
            </div>
          ) : mode === "edit" ? (
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
                  <WorkflowGraph positioned={positioned} edges={edges} view={view} verdicts={previewVerdicts} selectedId={selectedId} onSelect={(id) => setSelectedId(id || null)} />
                </div>
              </div>
            </div>
          ) : isDictation && !facts ? (
            <div className="flex-1 min-h-0 flex items-center justify-center text-sm text-text-tertiary px-6 text-center">
              {isRaw
                ? "This folder still needs an info.json — use “Initialize as dataset” in the toolbar above."
                : "Press “Scan status” in the toolbar to probe the dataset and refresh the run state."}
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
                <WorkflowGraph positioned={positioned} edges={edges} view={view} verdicts={nodeVerdicts} selectedId={selectedId} onSelect={(id) => setSelectedId(id || null)} />
              </div>

              {/* Inspector */}
              <div className="w-[440px] max-w-[55%] shrink-0 min-h-0 overflow-y-auto flex flex-col gap-3">
                {/* Scan result — what the last probe found on disk, i.e. exactly the
                    values this pipeline's `when:` guards are evaluated against. */}
                {isDictation && (
                  <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                    <div className="flex items-center justify-between gap-2">
                      <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Scan result</h2>
                      <span className="text-[10px] text-text-tertiary truncate min-w-0">
                        {scanning ? "Scanning…" : facts ? selectedName : "not scanned yet"}
                      </span>
                    </div>
                    {facts ? (
                      <div className="flex flex-col gap-2">
                        <div className="flex flex-wrap gap-1.5">
                          {FACT_LABELS.map(([key, label]) => {
                            const present = facts[key];
                            return (
                              <span
                                key={key}
                                className={`inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full ${
                                  present ? "bg-success-bg text-success-text" : "bg-bg-muted text-text-tertiary"
                                }`}
                                title={present ? "present on disk" : "missing on disk — steps guarded by it are skipped"}
                              >
                                {present ? <Check size={11} /> : <X size={11} />}
                                {label}
                              </span>
                            );
                          })}
                        </div>
                        {/* The pipeline-side reading of the same probe: which step each
                            finding belongs to, as this run's `verify:` bindings decide. */}
                        {verifyReport && (
                          <StepEvidence report={verifyReport} byId={auditById} steps={steps} onSelect={setSelectedId} />
                        )}
                        {audit && <AuditChecks key={audit.dataset_uuid} audit={audit} />}
                      </div>
                    ) : (
                      <p className="text-xs text-text-tertiary leading-relaxed">
                        Press “Scan status” to reconcile media/, subtitle/, waveform/ and transcript/ against the rows in data.sqlite3.
                      </p>
                    )}
                  </section>
                )}

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
                  readOnly={!isDictation}
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

                {isDictation && (
                  <>
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
                  </>
                )}

                {!isDictation && (
                  <section className="rounded-lg border border-border-light bg-bg-card p-3 flex flex-col gap-2">
                    <div className="flex items-center justify-between">
                      <h2 className="text-xs font-semibold uppercase tracking-wide text-text-secondary">Agent handoff</h2>
                      <button className={`${btnSmSecondary} inline-flex items-center gap-1`} onClick={copyYaml}><Copy size={14} /> Copy workflow.yaml</button>
                    </div>
                    <p className="text-xs text-text-tertiary leading-relaxed">
                      <span className="font-semibold text-text-secondary">{definition.name}</span> is a view / agent-facing workflow: every step’s action names a real command (an MCP tool or its Tauri-command twin), so an agent runs it via <span className="font-mono">workflow_builtin_templates</span> → <span className="font-mono">workflow_create</span> → the workflow loop. Per-step in-app Run is wired for the Dictation kind only.
                    </p>
                  </section>
                )}
              </div>
            </div>
          )}
        </div>
        </>
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
  /** View / agent-facing (Book, Card): hide the parameter form, Run and manual controls. */
  readOnly?: boolean;
  onChangeField: (id: string, name: string, value: string | boolean) => void;
  onRun: (step: StepDef, op: StepOp) => void;
  onClear: (step: StepDef, op: StepOp) => void;
  onIntervene: (step: StepDef, op: "reset" | "skip") => void;
  onForce: (step: StepDef, event: "completed" | "failed") => void;
  onOpenModels: () => void;
  pickDir: (title: string) => Promise<string | null>;
}

function StepInspector({ step, op, status, values, hint, command, runnable, running, runningStep, readOnly, onChangeField, onRun, onClear, onIntervene, onForce, onOpenModels, pickDir }: InspectorProps) {
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

      {readOnly && (
        <p className="text-xs text-text-tertiary mt-1">View / agent-facing step — no in-app Run wired for this workflow yet.</p>
      )}

      {/* Parameter form */}
      {!readOnly && op?.formFields?.length ? (
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
      {!readOnly && step.id === "ensure_model" && (
        <button className={`${btnSmSecondary} inline-flex items-center gap-1 self-start`} onClick={onOpenModels}>
          <Play size={13} /> Open Models page
        </button>
      )}

      {!readOnly && (
      <>
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
      </>
      )}
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

// ---------------------------------------------------------------------------
// Scan result — the audit read through the run's `verify:` bindings
// ---------------------------------------------------------------------------

interface StepEvidenceProps {
  report: VerifyReport;
  /** check id → its audit row, so every label and advice stays the report's words. */
  byId: Map<string, AuditCheck>;
  steps: StepDef[];
  onSelect: (id: string) => void;
}

/**
 * The pipeline-side view of one probe: which step the evidence holds responsible,
 * as the definition's `verify:` blocks decide and Rust joins. Rows are only the
 * steps with something to say — a drifted step first, then the steps whose `blocks`
 * evidence is still outstanding.
 *
 * Nothing here can change a status. Invalidation is an explicit, evented act
 * (`workflow_verify` with `apply=true`, which only ever moves a step *down*), and a
 * scan is not consent to change data.
 */
function StepEvidence({ report, byId, steps, onSelect }: StepEvidenceProps) {
  const rows = stepsWithEvidence(report);
  const ordered = [
    ...rows.filter((r) => r.entry.verdict === "drifted"),
    ...rows.filter((r) => r.entry.verdict !== "drifted"),
  ];

  return (
    <div className="flex flex-col gap-1">
      <div className="text-[11px] font-medium text-text-secondary">
        {ordered.length
          ? `${ordered.length} step(s) accountable for these findings`
          : "No step binding disagrees with the audit"}
      </div>

      {ordered.map((row) => {
        const step = stepById(row.stepId, steps);
        const contradicted = row.entry.verdict === "drifted";
        const Icon = contradicted ? AlertTriangle : Info;
        const cls = contradicted ? "text-warning-text" : "text-info-text";
        return (
          <div key={row.stepId} className="rounded-md border border-border-light bg-bg-body/60 px-2 py-1">
            <button
              className="flex items-center gap-1.5 w-full text-left cursor-pointer"
              onClick={() => onSelect(row.stepId)}
              title={verdictTitle(row.stepId, report, byId)}
            >
              <Icon size={12} className={`shrink-0 ${cls}`} />
              <span className="text-[11px] text-text-primary truncate">{step?.title || row.stepId}</span>
              <span className={`ml-auto shrink-0 text-[10px] ${cls}`}>
                {contradicted ? "contradicted" : "not yet"}
              </span>
            </button>
            <div className="mt-0.5 flex flex-col gap-0.5 pl-[18px]">
              {row.findings.map((f) => {
                const advice = findingAdvice(f, byId);
                return (
                  <p key={f.check} className="text-[10px] leading-snug text-text-tertiary" title={`verify: ${f.check}`}>
                    <span className="text-text-secondary">{findingLabel(f, byId)}</span>
                    {advice && <> — {advice}</>}
                  </p>
                );
              })}
            </div>
          </div>
        );
      })}

      {report.verify_hint && (
        <p className="text-[10px] leading-snug text-text-tertiary">{report.verify_hint}</p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Scan result — the audit report
// ---------------------------------------------------------------------------

interface AuditChecksProps {
  audit: DatasetAudit;
}

/**
 * The `dataset_audit` report: one row per check, severity-ordered, with the
 * exact offender count in the header and the offending files behind an expander.
 * Purely informational — a finding neither completes its step nor links to it.
 * Which step a finding belongs to is the binding above, judged from the run's own
 * definition; the graph keeps reflecting the run state instead of wishes about
 * what is on disk.
 */
function AuditChecks({ audit }: AuditChecksProps) {
  const [showPassed, setShowPassed] = useState(false);
  const flagged = audit.checks.filter((c) => c.level !== "clean");
  const passed = audit.checks.filter((c) => c.level === "clean");
  const blocking = flagged.filter((c) => c.level === "problem").length;

  return (
    <div className="flex flex-col gap-1">
      <div className="text-[10px] text-text-tertiary leading-snug">
        {audit.media_on_disk} media file(s) on disk · {audit.media_in_db} registered · {audit.subtitles_in_db} subtitle(s) · {audit.cues_in_db} current cue(s)
      </div>

      <div className="text-[11px] font-medium text-text-secondary">
        {flagged.length
          ? `${flagged.length} of ${audit.checks.length} checks flagged${blocking ? ` · ${blocking} blocking` : ""}`
          : `All ${audit.checks.length} checks passed`}
      </div>

      {flagged.map((c) => (
        <AuditCheckRow key={c.id} check={c} />
      ))}

      {passed.length > 0 && (
        <div className="flex flex-col gap-1">
          <button
            className="flex items-center gap-1.5 text-[11px] text-text-tertiary hover:text-text-secondary cursor-pointer"
            onClick={() => setShowPassed((s) => !s)}
          >
            {showPassed ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            <Check size={11} className="text-success-text" />
            {passed.length} check(s) passed
          </button>
          {showPassed && (
            <div className="flex flex-wrap gap-1 pl-4">
              {passed.map((c) => (
                <span key={c.id} className="text-[10px] px-1.5 py-0.5 rounded bg-bg-muted text-text-tertiary" title={c.advice}>
                  {c.label}
                </span>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function AuditCheckRow({ check }: { check: AuditCheck }) {
  const [open, setOpen] = useState(false);
  const { icon: Icon, cls } = AUDIT_LEVEL_STYLE[check.level];

  return (
    <div className="rounded-md border border-border-light bg-bg-body/60">
      <div className="flex items-center gap-1.5 px-2 py-1">
        <Icon size={12} className={`shrink-0 ${cls}`} />
        <button
          className="flex items-center gap-1.5 min-w-0 flex-1 text-left cursor-pointer"
          onClick={() => setOpen((o) => !o)}
          title={open ? "Collapse this check" : "Show the advice and the affected files"}
        >
          <ChevronRight size={11} className={`shrink-0 text-text-tertiary transition-transform ${open ? "rotate-90" : ""}`} />
          <span className="text-[11px] text-text-primary truncate">{check.label}</span>
          <span className={`ml-auto shrink-0 text-[11px] font-semibold ${cls}`}>{check.count > 0 ? check.count : "ok"}</span>
        </button>
      </div>

      {/* Collapsed rows carry just the first finding as a teaser, so the panel
          stays scannable while still naming a concrete file. */}
      {!open && check.items.length > 0 && (
        <p className="px-2 pb-1.5 text-[10px] text-text-tertiary truncate" title={check.items[0].detail}>
          <span className="font-mono">{check.items[0].source}</span> — {check.items[0].detail}
        </p>
      )}

      {open && (
        <div className="px-2 pb-1.5 pt-0.5 flex flex-col gap-1 border-t border-border-light">
          <p className="text-[11px] text-text-secondary leading-relaxed">{check.advice}</p>
          {check.items.map((it, i) => (
            <div key={`${check.id}-${i}`} className="text-[10px] leading-snug">
              <span className="font-mono text-text-primary break-all">{it.source}</span>
              <span className="text-text-tertiary"> — {it.detail}</span>
            </div>
          ))}
          {check.truncated && (
            <p className="text-[10px] text-text-tertiary">
              {check.count} in total — the report lists the first {check.items.length}. Fix those and re-scan for the rest.
            </p>
          )}
        </div>
      )}
    </div>
  );
}
