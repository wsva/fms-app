// ---------------------------------------------------------------------------
// In-app step operations registry for the Workflow page.
//
// Each dictation pipeline step maps to a typed Tauri command twin (the very
// command Studio invokes and the MCP tool wraps), so clicking a graph node's
// Run button performs the identical core work a goose agent would drive over
// `/mcp`. The page pairs this with the `workflow_*` engine command twins
// (advance -> run -> record) so a run's `state.json` stays in sync.
//
// The registry is keyed by step id. A custom step whose `action` equals a known
// command falls back to the generic op (see `genericOp`); an action with no
// command twin is agent-only (no in-app Run).
// ---------------------------------------------------------------------------

import type { DatasetFacts } from "./steps";

/** One editable control in the inspector's parameter form. */
export type FieldType = "text" | "textarea" | "checkbox" | "select" | "radios" | "folder";

export interface FormField {
  name: string;
  label: string;
  type: FieldType;
  placeholder?: string;
  options?: { value: string; label: string; hint?: string }[];
  /** Render labels inline (compact) rather than stacked. */
  inline?: boolean;
}

/** Everything a step's `run`/`gate` needs, supplied by the page. */
export interface RunCtx {
  uuid: string;
  /** Selected dataset's info.json name / description (for prefill + update). */
  name: string;
  description: string;
  facts: DatasetFacts | null;
  /** Current parameter-form values for this step. */
  form: Record<string, string | boolean>;
}

/** A resolved Tauri command invocation. */
export interface OpResult {
  command: string;
  args: Record<string, unknown>;
}

export interface StepOp {
  /** Inspector Run button label (defaults to "Run"). */
  runLabel?: string;
  formFields: FormField[];
  /** Seed the form from the dataset context when the node is selected. */
  defaults?: (ctx: RunCtx) => Record<string, string | boolean>;
  /** Gate: a hint string blocks the Run button; undefined means runnable. */
  gate?: (ctx: RunCtx) => string | undefined;
  /** Build the command + args to run this step. Omit for a non-runnable node. */
  run?: (ctx: RunCtx) => OpResult;
  /** Optional destructive Clear counterpart. */
  clear?: (ctx: RunCtx) => OpResult;
  /** Map the command result to engine `outputs` recorded on completion. */
  outputs?: (result: unknown) => Record<string, unknown>;
  /** Emit `dataset-list-changed` after a successful run (creation/import). */
  emitListChanged?: boolean;
  /** Re-probe dataset facts (and refresh engine status) after a run. */
  reloadAfter?: boolean;
}

/** Convenience guards reused across gates. */
const needDataset = (ctx: RunCtx) => (ctx.uuid ? undefined : "Select or initialize a dataset first.");
const has = (ctx: RunCtx, key: keyof DatasetFacts) => !!ctx.facts?.[key];

export const STEP_OPS: Record<string, StepOp> = {
  // Metadata of the selected dataset (Studio's "Edit Dataset"). Brand-new empty
  // datasets are still created from Studio (kept until removed) or the raw-folder
  // "Initialize" flow in this page's header.
  create_dataset: {
    runLabel: "Save",
    formFields: [
      { name: "name", label: "Name", type: "text", placeholder: "Dataset name" },
      { name: "description", label: "Description (optional)", type: "text", placeholder: "Optional description" },
    ],
    defaults: (ctx) => ({ name: ctx.name, description: ctx.description }),
    gate: needDataset,
    run: (ctx) => ({
      command: "dataset_update",
      args: { uuid: ctx.uuid, name: String(ctx.form.name ?? ctx.name), description: String(ctx.form.description ?? "") },
    }),
    emitListChanged: true,
    reloadAfter: true,
  },

  import_media: {
    runLabel: "Import",
    formFields: [
      { name: "sourceDir", label: "Source directory", type: "folder", placeholder: "Folder of audio/video files" },
      { name: "link", label: "Symlink", type: "checkbox", inline: true },
    ],
    gate: needDataset,
    run: (ctx) => ({
      command: "dataset_import_media",
      args: { uuid: ctx.uuid, sourceDir: String(ctx.form.sourceDir ?? ""), link: !!ctx.form.link },
    }),
    outputs: (r) => ({ media_count: typeof r === "number" ? r : undefined }),
    emitListChanged: true,
    reloadAfter: true,
  },

  generate_subtitles: {
    runLabel: "Generate",
    formFields: [],
    gate: (ctx) => (needDataset(ctx) ? needDataset(ctx) : has(ctx, "hasMedia") ? undefined : "No media. Import media first."),
    run: (ctx) => ({ command: "dataset_generate_subtitles", args: { uuid: ctx.uuid } }),
    clear: (ctx) => ({ command: "dataset_delete_subtitles", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  generate_database: {
    runLabel: "Build",
    formFields: [],
    gate: (ctx) => (needDataset(ctx) ? needDataset(ctx) : has(ctx, "hasSubtitles") ? undefined : "No subtitles. Generate subtitles first."),
    run: (ctx) => ({ command: "dataset_generate_database", args: { uuid: ctx.uuid } }),
    clear: (ctx) => ({ command: "dataset_delete_database", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  generate_waveforms: {
    runLabel: "Generate",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasMedia")
            ? undefined
            : "No media. Import media first.",
    run: (ctx) => ({ command: "dataset_generate_waveform", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  write_subtitles_to_db: {
    runLabel: "Write",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasSubtitles")
            ? undefined
            : "No subtitles. Generate subtitles first.",
    run: (ctx) => ({ command: "dataset_write_subtitles_to_db", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  sync_cue_times: {
    runLabel: "Sync",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasSubtitles")
            ? undefined
            : "No subtitles. Generate subtitles first.",
    run: (ctx) => ({ command: "dataset_sync_cue_times", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  split_book: {
    runLabel: "Split",
    formFields: [
      {
        name: "mode",
        label: "Engine",
        type: "radios",
        options: [
          { value: "rust", label: "Built-in (Rust)", hint: "fast, no setup, simple heuristic" },
          { value: "python", label: "Python + NLTK", hint: "higher quality; needs Python & nltk" },
        ],
      },
    ],
    defaults: () => ({ mode: "rust" }),
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasBook")
            ? undefined
            : "No book.txt in dataset.",
    run: (ctx) => ({ command: "dataset_parse_book", args: { datasetUuid: ctx.uuid, mode: String(ctx.form.mode ?? "rust") } }),
    reloadAfter: true,
  },

  align_cues: {
    runLabel: "Align",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasBook")
            ? undefined
            : "No book.txt in dataset.",
    run: (ctx) => ({ command: "dataset_align_cues", args: { datasetUuid: ctx.uuid } }),
    reloadAfter: true,
  },

  align_cues_transcript: {
    runLabel: "Align",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : has(ctx, "hasDatabase")
          ? undefined
          : "No database. Build the database first.",
    run: (ctx) => ({ command: "dataset_align_cues_transcript", args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  },

  write_transcripts: {
    runLabel: "Write",
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : has(ctx, "hasDatabase")
          ? undefined
          : "No database. Build the database first.",
    run: (ctx) => ({ command: "dataset_write_transcripts", args: { datasetUuid: ctx.uuid } }),
    reloadAfter: true,
  },

  adjust_cue_times: {
    runLabel: "Adjust",
    formFields: [
      {
        name: "mode",
        label: "Mode",
        type: "radios",
        inline: true,
        options: [
          { value: "new", label: "New subtitle" },
          { value: "in_place", label: "In place" },
        ],
      },
      { name: "force", label: "Force re-adjust", type: "checkbox", inline: true },
      {
        name: "strategy",
        label: "Strategy",
        type: "select",
        inline: true,
        options: [
          { value: "noise_floor", label: "Noise Floor" },
          { value: "dual_bound", label: "Dual Bound" },
          { value: "peak_relative", label: "Peak Relative" },
          { value: "otsu", label: "Otsu (Auto)" },
        ],
      },
    ],
    defaults: () => ({ mode: "new", force: false, strategy: "noise_floor" }),
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasWaveforms")
            ? undefined
            : "No waveforms. Generate waveforms first.",
    run: (ctx) => ({
      command: "dataset_adjust_cue_time",
      args: {
        uuid: ctx.uuid,
        mode: String(ctx.form.mode ?? "new"),
        force: !!ctx.form.force,
        strategy: String(ctx.form.strategy ?? "noise_floor"),
      },
    }),
    reloadAfter: true,
  },

  // Reads the dataset to surface which optional references exist. Its recorded
  // outputs are what the engine resolves `when` guards against downstream
  // (`${steps.detect_reference.outputs.has_book}` / `…has_transcript`), so this
  // mapper is the linchpin of an engine-honest graph: without it the guarded
  // steps (split_book / align_cues / align_cues_transcript) would go *blocked*
  // on an unresolvable reference instead of *skipped* on a false guard.
  detect_reference: {
    runLabel: "Detect",
    formFields: [],
    gate: needDataset,
    run: (ctx) => ({ command: "dataset_get", args: { uuid: ctx.uuid } }),
    outputs: (r) => {
      const d = r as {
        has_book?: boolean;
        has_transcript?: boolean;
        media?: { has_transcript?: boolean }[];
      };
      return {
        has_book: !!d?.has_book,
        has_transcript: !!d?.has_transcript || (d?.media?.some((m) => m.has_transcript) ?? false),
      };
    },
  },

  validate: {
    runLabel: "Validate",
    formFields: [],
    gate: needDataset,
    run: (ctx) => ({ command: "dataset_get", args: { uuid: ctx.uuid } }),
    outputs: (r) => {
      const d = r as { media?: unknown[]; has_subtitles?: boolean; has_database?: boolean; has_waveforms?: boolean };
      return {
        media_count: d?.media?.length ?? 0,
        has_subtitles: !!d?.has_subtitles,
        has_database: !!d?.has_database,
        has_waveforms: !!d?.has_waveforms,
      };
    },
  },

  mark_ready: {
    runLabel: "Mark ready",
    formFields: [{ name: "description", label: "Final note", type: "textarea", placeholder: "Record the processed state" }],
    defaults: (ctx) => ({ description: ctx.description }),
    gate: needDataset,
    run: (ctx) => ({
      command: "dataset_update",
      args: { uuid: ctx.uuid, name: ctx.name, description: String(ctx.form.description ?? ctx.description) },
    }),
    emitListChanged: true,
    reloadAfter: true,
  },
};

/** A safe, dataset-less context used only to enumerate command names statically. */
const ENUM_CTX: RunCtx = { uuid: "", name: "", description: "", facts: null, form: {} };

/** Every Tauri command the registry can dispatch — powers the Add-step list. */
export const KNOWN_COMMANDS: string[] = Array.from(
  new Set(
    Object.values(STEP_OPS).flatMap((op) =>
      [op.run?.(ENUM_CTX)?.command, op.clear?.(ENUM_CTX)?.command].filter((c): c is string => !!c),
    ),
  ),
);

/** The set of step ids known to be runnable in-app. */
export function isRunnableStepId(id: string): boolean {
  return !!STEP_OPS[id]?.run;
}

/**
 * Resolve the op for a step: the id-specific op when present, otherwise a
 * generic op for a custom step whose `action` names a known command. Returns
 * `undefined` for an agent-only node (no in-app Run).
 */
export function getStepOp(id: string, action: string): StepOp | undefined {
  const specific = STEP_OPS[id];
  if (specific) return specific;
  if (KNOWN_COMMANDS.includes(action)) return genericOp(action);
  return undefined;
}

/** A custom step against a known command: run it with `{ uuid, ...params }`. */
function genericOp(command: string): StepOp {
  return {
    runLabel: "Run",
    formFields: [],
    gate: needDataset,
    run: (ctx) => ({ command, args: { uuid: ctx.uuid } }),
    reloadAfter: true,
  };
}
