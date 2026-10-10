// ---------------------------------------------------------------------------
// Workflow step definitions + a small client-side mirror of the Rust engine.
//
// The authoritative engine lives in `src-tauri/src/workflow/core` and persists
// runs as `state.json`. Before a run exists, the Workflow page shows an honest
// all-`pending` graph and only derives what `engine::recompute` can from the DAG
// itself — `when`-guard skipping and dependency readiness — never faking step
// completion from the dataset's on-disk facts. Once a `workflow_*` Tauri command
// twin returns real run state, that replaces this local projection entirely; the
// graph and prompt code stay the same.
//
// The DAG itself is NOT hardcoded here: it is parsed from `workflow.yaml`-shaped
// text (`parseDefinition`), so the Workflow page's editor can drive any pipeline.
// Every layout/engine helper takes the parsed `steps` as a parameter. The one
// built-in pipeline ("Dataset Dictation") is no longer duplicated in this repo's
// frontend either — it ships in the Rust binary (`src-tauri/src/workflow/
// templates/dataset_dictation.yaml`, served via `workflow_builtin_templates`),
// so the page fetches it at runtime instead of importing a seed string.
// ---------------------------------------------------------------------------

import { load as loadYaml } from "js-yaml";

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

/** A parsed `workflow.yaml`: identity + the ordered step list. */
export interface WorkflowDefinition {
  name: string;
  version: number;
  steps: StepDef[];
}

// ---------------------------------------------------------------------------
// YAML → StepDef parsing (the editor's source of truth)
// ---------------------------------------------------------------------------

/** Raw shape of one YAML step (engine `workflow.yaml` schema + UI-only `title`). */
interface RawStep {
  id?: unknown;
  action?: unknown;
  title?: unknown;
  description?: unknown;
  depends_on?: unknown;
  dependsOn?: unknown;
  when?: unknown;
  params?: unknown;
}

/** `align_cues_transcript` → `Align cues transcript` (fallback node label). */
function humanize(id: string): string {
  const s = id.replace(/[_-]+/g, " ").trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : id;
}

/** Substring (engine `when` form) → the `DatasetFacts` key the client evaluates. */
const FACT_GUARDS: Array<[string, keyof DatasetFacts]> = [
  ["has_media", "hasMedia"],
  ["has_subtitles", "hasSubtitles"],
  ["has_database", "hasDatabase"],
  ["has_waveforms", "hasWaveforms"],
  ["has_book", "hasBook"],
  ["has_transcript", "hasTranscript"],
];

const FACT_KEYS = new Set<string>([
  "hasMedia",
  "hasSubtitles",
  "hasDatabase",
  "hasWaveforms",
  "hasBook",
  "hasTranscript",
]);

/**
 * Map an engine `when` guard onto a `DatasetFacts` key the client projection can
 * evaluate. The engine form is an output reference (`${steps.x.outputs.has_book}`);
 * we match the known disk-probe guards by substring, and also accept a bare
 * `DatasetFacts` key. Anything the client can't resolve becomes `undefined` (the
 * step is always active) — arbitrary output references are only resolvable from a
 * real run's `state.json`, which this client projection does not read.
 */
function normalizeWhen(when: unknown): keyof DatasetFacts | undefined {
  if (typeof when !== "string") return undefined;
  const trimmed = when.trim();
  if (FACT_KEYS.has(trimmed)) return trimmed as keyof DatasetFacts;
  const lower = trimmed.toLowerCase();
  for (const [needle, key] of FACT_GUARDS) {
    if (lower.includes(needle)) return key;
  }
  return undefined;
}

function asStringArray(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.filter((x): x is string => typeof x === "string");
}

/**
 * Parse a `workflow.yaml`-shaped document into a [`WorkflowDefinition`].
 * Throws an actionable `Error` on malformed YAML, a missing step `id`/`action`,
 * duplicate ids, or a `depends_on` referencing an unknown step — the editor
 * surfaces the message inline and keeps the last-good graph.
 */
export function parseDefinition(yamlText: string): WorkflowDefinition {
  let doc: unknown;
  try {
    doc = loadYaml(yamlText);
  } catch (e) {
    throw new Error(`Invalid YAML: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) {
    throw new Error("Expected a mapping with `name`, `version` and `steps`.");
  }
  const root = doc as { name?: unknown; version?: unknown; steps?: unknown };
  const rawSteps = Array.isArray(root.steps) ? (root.steps as RawStep[]) : [];
  if (rawSteps.length === 0) {
    throw new Error("`steps` is empty — add at least one step.");
  }

  const steps: StepDef[] = rawSteps.map((raw, i) => {
    const id = typeof raw.id === "string" ? raw.id.trim() : "";
    if (!id) throw new Error(`Step #${i + 1} is missing an \`id\`.`);
    const action = typeof raw.action === "string" ? raw.action.trim() : "";
    if (!action) throw new Error(`Step "${id}" is missing an \`action\`.`);
    const step: StepDef = {
      id,
      title:
        typeof raw.title === "string" && raw.title.trim() ? raw.title.trim() : humanize(id),
      action,
      description: typeof raw.description === "string" ? raw.description.trim() : "",
      dependsOn: asStringArray(raw.depends_on ?? raw.dependsOn),
    };
    const when = normalizeWhen(raw.when);
    if (when) step.when = when;
    if (raw.params && typeof raw.params === "object" && !Array.isArray(raw.params)) {
      step.params = raw.params as Record<string, unknown>;
    }
    return step;
  });

  const ids = new Set<string>();
  for (const s of steps) {
    if (ids.has(s.id)) throw new Error(`Duplicate step id "${s.id}".`);
    ids.add(s.id);
  }
  for (const s of steps) {
    for (const dep of s.dependsOn) {
      if (!ids.has(dep)) {
        throw new Error(`Step "${s.id}" depends on unknown step "${dep}".`);
      }
    }
  }

  return {
    name: typeof root.name === "string" && root.name.trim() ? root.name.trim() : "workflow",
    version: typeof root.version === "number" ? root.version : 1,
    steps,
  };
}

/** Look a step up by id within a step list. */
export function stepById(id: string, steps: StepDef[]): StepDef | undefined {
  return steps.find((s) => s.id === id);
}

/** Committed statuses the UI mutates directly; readiness is derived on top. */
export type StatusMap = Record<string, StepStatus>;

/** Kahn's algorithm over `dependsOn`; source order preserved on ties. */
export function topoOrder(steps: StepDef[]): string[] {
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
export function recompute(committed: StatusMap, facts: DatasetFacts, steps: StepDef[]): StatusMap {
  const byId = new Map(steps.map((s) => [s.id, s]));
  const out: StatusMap = {};
  for (const id of topoOrder(steps)) {
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

// (Removed `seedFromDataset`: the pre-run graph no longer fakes completed steps
// from on-disk facts. It starts all-`pending` via `initialStatuses`; real progress
// comes from the engine's run state once the `workflow_*` command twins return it.)

/** Empty status map — nothing run yet. */
export function initialStatuses(steps: StepDef[]): StatusMap {
  const m: StatusMap = {};
  for (const s of steps) m[s.id] = "pending";
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
export function layoutSteps(steps: StepDef[]): PositionedStep[] {
  const byId = new Map(steps.map((s) => [s.id, s]));
  const depth = new Map<string, number>();
  for (const id of topoOrder(steps)) {
    const step = byId.get(id)!;
    const d = step.dependsOn.length === 0
      ? 0
      : 1 + Math.max(...step.dependsOn.map((dep) => depth.get(dep) ?? 0));
    depth.set(id, d);
  }

  const maxDepth = Math.max(0, ...steps.map((s) => depth.get(s.id) ?? 0));
  const xOf = new Map<string, number>();

  for (let layer = 0; layer <= maxDepth; layer++) {
    const ordered = steps
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
  return steps.map((step) => ({
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
export function buildEdges(steps: StepDef[]): Edge[] {
  const edges: Edge[] = [];
  for (const s of steps) {
    for (const d of s.dependsOn) edges.push({ id: `${d}->${s.id}`, source: d, target: s.id });
  }
  return edges;
}

/** The steps the agent should advance + execute, given the current derivation. */
export function readyStepIds(view: StatusMap, steps: StepDef[]): string[] {
  return topoOrder(steps).filter((id) => view[id] === "ready");
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
  steps: StepDef[];
  definition: { name: string; version: number; yamlPath?: string };
}): string {
  const { datasetName, datasetUuid, runId, view, steps, definition } = opts;
  const ready = readyStepIds(view, steps);
  const order = topoOrder(steps);
  const skipped = order.filter((id) => view[id] === "skipped");
  const done = order.filter((id) => view[id] === "completed");

  const lines: string[] = [];
  lines.push(`Drive the "${definition.name}" workflow for dataset "${datasetName}" (uuid ${datasetUuid}).`);
  lines.push("");
  lines.push(`Definition: ${definition.yamlPath ?? definition.name} (name: ${definition.name}, version: ${definition.version}).`);
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
      const s = stepById(id, steps)!;
      const params = s.params ? ` — params: ${JSON.stringify(s.params)}` : "";
      lines.push(`  - ${id}: ${s.action} (${s.description})${params}`);
    }
  } else {
    lines.push("No steps are currently ready — inspect workflow_status and intervene to unblock.");
  }
  return lines.join("\n");
}
