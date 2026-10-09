// ---------------------------------------------------------------------------
// Workflow step definitions + a small client-side mirror of the Rust engine.
//
// The authoritative engine lives in `src-tauri/src/workflow/core` and persists
// runs as `state.json`. That backend surface is MCP-only today, so the Workflow
// page drives a *local* projection of the DAG: it seeds step status from the
// dataset's on-disk facts (`dataset_get`), then recomputes readiness the same way
// `engine::recompute` does (when-guards → skipped, satisfied deps → ready). When
// the `workflow_*` Tauri command twins land, they replace `seedFromDataset` +
// the local `recompute` with the real run state — the graph and prompt code stay.
// ---------------------------------------------------------------------------

/** The seven step states, mirroring `workflow::core::Status`. */
export type StepStatus =
  | "pending"
  | "ready"
  | "running"
  | "completed"
  | "failed"
  | "blocked"
  | "skipped";

/** Facts probed from the dataset that drive `when` guards and seeding. */
export interface DatasetFacts {
  hasMedia: boolean;
  hasSubtitles: boolean;
  hasDatabase: boolean;
  hasWaveforms: boolean;
  hasBook: boolean;
  hasTranscript: boolean;
}

/** One node of the workflow DAG — a subset of the YAML `Step` schema. */
export interface StepDef {
  id: string;
  /** Human label for the node (short, from the YAML stage name). */
  title: string;
  /** MCP tool / Tauri command the agent calls to perform this step. */
  action: string;
  description: string;
  dependsOn: string[];
  /** Guard key into `DatasetFacts`; false ⇒ step is skipped, not failed. */
  when?: keyof DatasetFacts;
  /** Static params surfaced into the generated prompt. */
  params?: Record<string, unknown>;
}

// The `dataset_dictation.yaml` DAG (docs/ai/workflow/dataset_dictation.yaml),
// kept in topological-ish source order. Edges are derived from `dependsOn`.
export const DICTATION_STEPS: StepDef[] = [
  {
    id: "ensure_model",
    title: "Ensure STT model",
    action: "model_status",
    description: "Check model_status; download + load the preferred STT model if needed.",
    dependsOn: [],
  },
  {
    id: "create_dataset",
    title: "Create dataset",
    action: "dataset_create",
    description: "Create the empty dictation dataset (or import an existing one).",
    dependsOn: [],
    params: { name: "my_dictation" },
  },
  {
    id: "import_media",
    title: "Import media",
    action: "dataset_import_media",
    description: "Copy audio/video files from a source directory into the dataset.",
    dependsOn: ["create_dataset"],
    params: { source_dir: "C:/path/to/audio", link: false },
  },
  {
    id: "generate_subtitles",
    title: "Generate subtitles",
    action: "dataset_generate_subtitles",
    description: "STT-transcribe every media file into VTT subtitles (long-running).",
    dependsOn: ["ensure_model", "import_media"],
  },
  {
    id: "generate_waveforms",
    title: "Generate waveforms",
    action: "dataset_generate_waveforms",
    description: "Generate waveform JSON via Symphonia peak detection.",
    dependsOn: ["import_media"],
  },
  {
    id: "generate_database",
    title: "Build cue database",
    action: "dataset_generate_database",
    description: "Parse VTT subtitles into listen_media / listen_subtitle / cue tables.",
    dependsOn: ["generate_subtitles"],
  },
  {
    id: "detect_reference",
    title: "Detect reference",
    action: "dataset_list_files",
    description: "List dataset files to see whether book.txt and/or transcript/ exist.",
    dependsOn: ["generate_database"],
  },
  {
    id: "align_cues",
    title: "Align cues (book)",
    action: "dataset_align_cues",
    description: "Align cue text against book.txt with multi-pass anchor DP matching.",
    dependsOn: ["detect_reference"],
    when: "hasBook",
  },
  {
    id: "align_cues_transcript",
    title: "Align cues (transcript)",
    action: "dataset_align_cues_transcript",
    description: "Align cue text against per-subtitle files in transcript/ instead.",
    dependsOn: ["detect_reference"],
    when: "hasTranscript",
  },
  {
    id: "adjust_cue_times",
    title: "Adjust cue times",
    action: "dataset_adjust_cue_time",
    description: "Snap cue boundaries to silence detected in the audio.",
    dependsOn: ["generate_database"],
    params: { mode: "new", strategy: "noise_floor" },
  },
  {
    id: "validate",
    title: "Validate",
    action: "dataset_get_summary",
    description: "Summarize counts and spot-check cues: subtitle per media, ordered cues.",
    dependsOn: ["generate_waveforms", "align_cues", "align_cues_transcript", "adjust_cue_times"],
  },
  {
    id: "mark_ready",
    title: "Mark ready",
    action: "dataset_info_update",
    description: "Record final counts and note the dataset is processed and practice-ready.",
    dependsOn: ["validate"],
    params: { description: "Processed: subtitles + waveforms + cue DB generated and validated." },
  },
];

export const DICTATION_DEFINITION = {
  name: "process_dictation_dataset",
  version: 1,
  yamlPath: "docs/ai/workflow/dataset_dictation.yaml",
};

const byId = new Map(DICTATION_STEPS.map((s) => [s.id, s]));

export function stepById(id: string): StepDef | undefined {
  return byId.get(id);
}

/** Committed statuses the UI mutates directly; readiness is derived on top. */
export type StatusMap = Record<string, StepStatus>;

/** Kahn's algorithm over `dependsOn`; source order preserved on ties. */
export function topoOrder(steps: StepDef[] = DICTATION_STEPS): string[] {
  const indeg = new Map<string, number>(steps.map((s) => [s.id, 0]));
  const adj = new Map<string, string[]>();
  for (const s of steps) {
    for (const d of s.dependsOn) {
      indeg.set(s.id, (indeg.get(s.id) ?? 0) + 1);
      let list = adj.get(d);
      if (!list) {
        list = [];
        adj.set(d, list);
      }
      list.push(s.id);
    }
  }
  const queue = steps.filter((s) => (indeg.get(s.id) ?? 0) === 0).map((s) => s.id);
  const order: string[] = [];
  while (queue.length > 0) {
    const id = queue.shift()!;
    order.push(id);
    for (const nxt of adj.get(id) ?? []) {
      const deg = (indeg.get(nxt) ?? 1) - 1;
      indeg.set(nxt, deg);
      if (deg === 0) queue.push(nxt);
    }
  }
  // Cyclic leftovers shouldn't happen (validate.rs guards it server-side), but
  // append anything unresolved so the layout never drops a node.
  for (const s of steps) if (!order.includes(s.id)) order.push(s.id);
  return order;
}

function satisfied(st: StepStatus | undefined): boolean {
  return st === "completed" || st === "skipped";
}

/**
 * Derive the view status for every step from its committed status + dataset facts.
 * Mirrors `engine::recompute`: a `pending` step whose `when` guard is false becomes
 * `skipped`; a `pending` step whose deps are all satisfied becomes `ready`.
 * Committed states (completed/failed/running/skipped) are left untouched.
 */
export function recompute(committed: StatusMap, facts: DatasetFacts): StatusMap {
  const out: StatusMap = {};
  for (const id of topoOrder()) {
    const step = byId.get(id)!;
    let st = committed[id] ?? "pending";
    if (st === "pending" && step.when && facts[step.when] === false) {
      st = "skipped";
    }
    if (st === "pending") {
      const ready = step.dependsOn.every((d) => satisfied(out[d]));
      if (ready) st = "ready";
    }
    out[id] = st;
  }
  return out;
}

/**
 * Seed committed statuses from a freshly probed dataset — the "no state.json, so
 * determine current progress" path. Steps the dataset can't prove (align / adjust /
 * validate / mark_ready) stay `pending`; `recompute` then marks the guard-skippable
 * and dependency-ready ones. The user fine-tunes the rest from the graph.
 */
export function seedFromDataset(facts: DatasetFacts): StatusMap {
  const seed: StatusMap = {};
  seed.create_dataset = "completed";
  seed.ensure_model = "completed"; // a model is configured; agent re-checks if not
  if (facts.hasMedia) seed.import_media = "completed";
  if (facts.hasSubtitles) seed.generate_subtitles = "completed";
  if (facts.hasDatabase) seed.generate_database = "completed";
  if (facts.hasWaveforms) seed.generate_waveforms = "completed";
  seed.detect_reference = "completed"; // we just probed the files
  return seed;
}

/** Empty status map — nothing run yet. */
export function initialStatuses(): StatusMap {
  const m: StatusMap = {};
  for (const s of DICTATION_STEPS) m[s.id] = "pending";
  return m;
}

export interface PositionedStep {
  step: StepDef;
  x: number;
  y: number;
}

// The graph flows top→down: a layer is a horizontal band, so the DAG grows into
// the vertical scroll area and stays narrow enough to leave the inspector column
// real width. Spacing is in flow units: bands separated by LAYER_GAP_Y, siblings
// within a band separated by NODE_GAP_X.
const LAYER_GAP_Y = 150;
const NODE_GAP_X = 250;

/**
 * Longest-path layering, laid out top→down (`depth` → y, in-layer order → x).
 *
 * Within a layer, nodes are ordered by the barycenter — the mean x of the parents
 * already placed in an earlier layer — so a step sits under the node it depends on
 * and edges stay short and vertical instead of crossing. Roots and any layer whose
 * parents are not placed yet fall back to declaration order, which keeps the result
 * stable run to run. Each layer is centered on x, then the whole drawing is shifted
 * to non-negative coordinates.
 */
export function layoutSteps(): PositionedStep[] {
  const depth = new Map<string, number>();
  for (const id of topoOrder()) {
    const step = byId.get(id)!;
    const d = step.dependsOn.length === 0
      ? 0
      : 1 + Math.max(...step.dependsOn.map((dep) => depth.get(dep) ?? 0));
    depth.set(id, d);
  }

  const maxDepth = Math.max(0, ...DICTATION_STEPS.map((s) => depth.get(s.id) ?? 0));
  const xOf = new Map<string, number>();

  for (let layer = 0; layer <= maxDepth; layer++) {
    const ordered = DICTATION_STEPS
      .map((step, i) => ({ step, i }))
      .filter(({ step }) => (depth.get(step.id) ?? 0) === layer)
      .map(({ step, i }) => {
        const parents = step.dependsOn
          .map((dep) => xOf.get(dep))
          .filter((x): x is number => x !== undefined);
        const bary = parents.length
          ? parents.reduce((a, b) => a + b, 0) / parents.length
          : i * NODE_GAP_X;
        return { step, i, bary };
      })
      .sort((a, b) => a.bary - b.bary || a.i - b.i);

    ordered.forEach(({ step }, idx) => {
      xOf.set(step.id, (idx - (ordered.length - 1) / 2) * NODE_GAP_X);
    });
  }

  const placed = Array.from(xOf.values());
  const minX = placed.length ? Math.min(...placed) : 0;
  return DICTATION_STEPS.map((step) => ({
    step,
    x: (xOf.get(step.id) ?? 0) - minX,
    y: (depth.get(step.id) ?? 0) * LAYER_GAP_Y,
  }));
}

export interface Edge {
  id: string;
  source: string;
  target: string;
}

/** Directed edges derived from every step's `dependsOn`. */
export function buildEdges(): Edge[] {
  const edges: Edge[] = [];
  for (const s of DICTATION_STEPS) {
    for (const d of s.dependsOn) edges.push({ id: `${d}->${s.id}`, source: d, target: s.id });
  }
  return edges;
}

/** The steps the agent should advance + execute, given the current derivation. */
export function readyStepIds(view: StatusMap): string[] {
  return topoOrder().filter((id) => view[id] === "ready");
}

const STATUS_LABEL: Record<StepStatus, string> = {
  pending: "Pending",
  ready: "Ready",
  running: "Running",
  completed: "Done",
  failed: "Failed",
  blocked: "Blocked",
  skipped: "Skipped",
};

export function statusLabel(s: StepStatus): string {
  return STATUS_LABEL[s];
}

/**
 * Build the driver prompt handed to the Agent Dock. It names the run, tells the
 * agent to load/execute the DAG loop, and lists the currently-ready steps with
 * their actions + params so the first turn has everything it needs.
 */
export function buildPrompt(opts: {
  datasetName: string;
  datasetUuid: string;
  runId: string;
  view: StatusMap;
}): string {
  const { datasetName, datasetUuid, runId, view } = opts;
  const ready = readyStepIds(view);
  const skipped = topoOrder().filter((id) => view[id] === "skipped");
  const done = topoOrder().filter((id) => view[id] === "completed");

  const lines: string[] = [];
  lines.push(`Drive the dictation workflow for dataset "${datasetName}" (uuid ${datasetUuid}).`);
  lines.push("");
  lines.push(`Definition: ${DICTATION_DEFINITION.yamlPath} (name: ${DICTATION_DEFINITION.name}, version: ${DICTATION_DEFINITION.version}).`);
  lines.push(`Run id: ${runId}`);
  lines.push("");
  lines.push("Steps already known complete on disk (do NOT re-run unless I ask): " + (done.length ? done.join(", ") : "none") + ".");
  lines.push("Steps intentionally skipped: " + (skipped.length ? skipped.join(", ") : "none") + ".");
  lines.push("");
  lines.push("Loop until the run is completed or blocked:");
  lines.push("1. workflow_status(run_id) to sync derived statuses (safe to call after a restart).");
  lines.push("2. workflow_next(run_id) for the ready steps.");
  lines.push("3. For each ready step: workflow_advance(run_id, step), call the MCP tool with the same name as its action using the resolved inputs/params, then workflow_record(run_id, step, event=completed|failed, detail).");
  lines.push("4. Use workflow_intervene (retry/skip/reset) to recover, and report progress as you go.");
  lines.push("");
  if (ready.length > 0) {
    lines.push("Start with these ready steps now:");
    for (const id of ready) {
      const s = stepById(id)!;
      const params = s.params ? ` — params: ${JSON.stringify(s.params)}` : "";
      lines.push(`  - ${id}: ${s.action} (${s.description})${params}`);
    }
  } else {
    lines.push("No steps are currently ready — inspect workflow_status and intervene to unblock.");
  }
  return lines.join("\n");
}
