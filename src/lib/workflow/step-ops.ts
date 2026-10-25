// ---------------------------------------------------------------------------
// In-app step operations registry for the Workflow page.
//
// Each dictation pipeline step maps to a typed Tauri command twin (the very
// command this page invokes and the MCP tool wraps), so clicking a graph node's
// Run button performs the identical core work an attached agent would drive over
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
  /** Selected dataset's own folder path (raw folder before init, or the
   *  initialized dataset's dir). Powers the `init_dataset` step. */
  path: string;
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
  // Initialize a raw media folder IN PLACE as a dictation dataset (mint a uuid,
  // write info.json, create an empty data.sqlite3) — the head of the built-in
  // pipeline. Unlike every other step this one has no dataset to act on yet: it
  // consumes the *selected folder's path* (raw, no uuid) instead, which is why
  // the gate keys off `path` rather than `needDataset`. Idempotent on the
  // backend, so re-running after init just returns the existing summary.
  init_dataset: {
    runLabel: "Initialize",
    formFields: [
      { name: "path", label: "Folder path", type: "text", placeholder: "C:/path/to/dataset_folder" },
    ],
    defaults: (ctx) => ({ path: ctx.path }),
    gate: (ctx) => (ctx.path ? undefined : "Select a raw media folder (no info.json yet) first."),
    run: (ctx) => ({
      command: "dataset_init_dir",
      args: { path: String(ctx.form.path ?? ctx.path) },
    }),
    outputs: (r) => {
      const d = r as { info?: { uuid?: string }; media_count?: number };
      return { dataset_uuid: d?.info?.uuid, media_count: d?.media_count };
    },
    emitListChanged: true,
    reloadAfter: true,
  },

  // Reconcile listen_media with the files under media/ in place — the cheap,
  // non-destructive alternative to a full dataset_generate_database rebuild.
  sync_media: {
    runLabel: "Sync",
    formFields: [],
    gate: needDataset,
    run: (ctx) => ({ command: "dataset_sync_media", args: { uuid: ctx.uuid } }),
    outputs: (r) => {
      const d = r as { added_count?: number; removed_count?: number };
      return { added_count: d?.added_count ?? 0, removed_count: d?.removed_count ?? 0 };
    },
    reloadAfter: true,
  },

  // Metadata of the selected dataset (the "Edit Dataset" step). Brand-new empty
  // datasets are still created here (kept until removed) or the raw-folder
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
    formFields: [
      { name: "threshold", label: "Match threshold (0-100)", type: "text", placeholder: "70" },
    ],
    defaults: () => ({ threshold: "70" }),
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasSubtitles")
            ? undefined
            : "No subtitles. Generate subtitles first.",
    run: (ctx) => {
      const parsed = Number(ctx.form.threshold);
      return {
        command: "dataset_sync_cue_times",
        // Blank or unparseable leaves the backend's own default in charge rather
        // than sending a null the command would read as "no threshold given".
        args: { uuid: ctx.uuid, threshold: Number.isFinite(parsed) ? parsed : undefined },
      };
    },
    reloadAfter: true,
  },

  split_book: {
    runLabel: "Split",
    // No form: the stage always runs the SaT 3L Small model, the same one the
    // Split Text page defaults to — so the only setup is having it downloaded.
    formFields: [],
    gate: (ctx) =>
      needDataset(ctx)
        ? needDataset(ctx)
        : !has(ctx, "hasDatabase")
          ? "No database. Build the database first."
          : has(ctx, "hasBook")
            ? undefined
            : "No book.txt in dataset.",
    run: (ctx) => ({ command: "dataset_parse_book", args: { datasetUuid: ctx.uuid } }),
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

  // The sync pipeline's probe, and the guard input its optional generate step is
  // evaluated against: one `dataset_audit` call answers "is there a media file with
  // no subtitle anywhere?". An absent `subtitles_missing` check means *not measured*
  // (the audit short-circuits when there is no data.sqlite3 to compare against), so
  // it reads false — the honest answer, since generating subtitles would then have
  // to build the database first, which is the build workflow's job.
  detect_subtitles: {
    runLabel: "Detect",
    formFields: [],
    gate: needDataset,
    run: (ctx) => ({ command: "dataset_audit", args: { uuid: ctx.uuid } }),
    outputs: (r) => {
      const d = r as { checks?: { id: string; count: number }[] };
      const missing = d?.checks?.find((c) => c.id === "subtitles_missing");
      return { needs_subtitles: (missing?.count ?? 0) > 0 };
    },
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

/** A safe, dataset-less context used only to enumerate command names statically.
 *  A Proxy rather than an object literal on purpose: every read answers `""`, so
 *  no builder can throw on a field it dereferences (`ctx.form.path`) whichever
 *  way the bundler treats this module. The static-export prerender proved it
 *  matters: the server bundle emitted this context with *no* properties at all
 *  (the client bundle kept them), and module-scope enumeration then crashed the
 *  build with `Cannot read properties of undefined (reading 'path')`. Only
 *  `.command` is read off the result, so empty argument values cost nothing. */
const ENUM_CTX: RunCtx = new Proxy({} as RunCtx, { get: () => "" });

/** Every Tauri command the registry can dispatch — powers the Add-step list. */
export const KNOWN_COMMANDS: string[] = Array.from(
  new Set(
    Object.values(STEP_OPS).flatMap((op) =>
      [op.run?.(ENUM_CTX)?.command, op.clear?.(ENUM_CTX)?.command].filter((c): c is string => !!c),
    ),
  ),
);

/** One step op as an agent-readable row: what the step is called, which command
 *  it dispatches, and which form fields a definition may carry. This is the
 *  shape {@link stepOpDocs} publishes for `workflow_list_actions`, so Rust can
 *  describe authored YAML without keeping a second copy of this registry. */
export interface StepOpDoc {
  id: string;
  run_label: string;
  /** The Tauri command `run` resolves to. Absent for an agent-only node. */
  run_command?: string;
  /** The command `clear` resolves to, when the step has a destructive counterpart. */
  clear_command?: string;
  /** Whether the page re-probes dataset facts after a run. */
  reload_after: boolean;
  fields: {
    name: string;
    label: string;
    type: FieldType;
    placeholder?: string;
    /** The closed value set, when the field is a `select`/`radios`. */
    options?: string[];
    inline?: boolean;
  }[];
}

/** Every step op as an agent-readable row, sorted by id so the published
 *  snapshot is stable across reloads. Commands are resolved through
 *  {@link ENUM_CTX}, i.e. statically — nothing here is dispatched. */
export function stepOpDocs(): StepOpDoc[] {
  const docs = Object.entries(STEP_OPS).map(([id, op]): StepOpDoc => {
    const doc: StepOpDoc = {
      id,
      run_label: op.runLabel ?? "Run",
      reload_after: !!op.reloadAfter,
      fields: op.formFields.map((f) => ({
        name: f.name,
        label: f.label,
        type: f.type,
        placeholder: f.placeholder,
        options: f.options?.length ? f.options.map((o) => o.value) : undefined,
        inline: f.inline,
      })),
    };
    if (op.run) doc.run_command = op.run(ENUM_CTX).command;
    if (op.clear) doc.clear_command = op.clear(ENUM_CTX).command;
    return doc;
  });
  return docs.sort((a, b) => a.id.localeCompare(b.id));
}

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
