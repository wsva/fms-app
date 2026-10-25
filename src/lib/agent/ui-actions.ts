// ---------------------------------------------------------------------------
// The UI action registry: everything the mounted app can do, published to the
// backend so an agent can drive it generically.
//
// Two MCP tools read it (`ui_list_actions`, `ui_invoke_action` — see
// `src-tauri/src/mcp/ui.rs`), so a page that registers an action adds to the
// agent's surface with no Rust change at all. That is the point: one tool pair
// instead of one hand-written tool per button.
//
// Pages here are display-toggled rather than unmounted, so registrations
// survive navigation and the registry means "everything the app can do right
// now" — a capability manifest, not "what is on screen". Actions are removed
// only when the page that registered them actually unmounts, because an action
// whose closure is gone cannot run.
//
// House precedents: `lib/workflow/step-ops.ts` (a registry of known commands
// keyed by id) and the `agent-prefill` event (a named intent the shell routes).
// ---------------------------------------------------------------------------

import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isAgentHost } from "./host";

/** What an action does to the world. `destructive` is refused by
 *  `ui_invoke_action` outright: destructive work goes through the data tools,
 *  which have a trash, so there is one undo path and not two. Declaring it is
 *  still worth doing — `ui_list_actions` shows it, so an agent learns the
 *  boundary instead of discovering it by being refused. */
export type UiActionKind = "read" | "mutate" | "destructive";

/** One declared parameter. Mirrors the check `mcp/ui.rs::check_params` makes
 *  before the event is ever emitted. */
export interface UiActionParam {
  type: "string" | "number" | "boolean";
  description: string;
  required?: boolean;
  /** A closed set of allowed values. */
  enum?: string[];
}

export interface UiAction {
  /** Stable id, `"<tab>.<verb>"` by convention, e.g. `"cards.start-review"`. */
  id: string;
  /** One human sentence: what clicking this does. */
  label: string;
  /** The `TabId` of the page that owns it (stamped by {@link useAgentActions}). */
  tab: string;
  kind: UiActionKind;
  /** Declared parameters; omit when the action takes none. */
  params_schema?: Record<string, UiActionParam>;
  /** The action itself. Return a value and it reaches the agent as `detail`. */
  run: (params: Record<string, unknown>) => unknown | Promise<unknown>;
}

/** What a page declares; `tab` comes from the hook so it is written once. */
export type UiActionDef = Omit<UiAction, "tab">;

const registry = new Map<string, UiAction>();

let timer: ReturnType<typeof setTimeout> | null = null;
const DEBOUNCE_MS = 200;

/** The manifest an action publishes: everything but the closure. */
function manifestOf(a: UiActionDef): Omit<UiActionDef, "run"> {
  return {
    id: a.id,
    label: a.label,
    kind: a.kind,
    params_schema: a.params_schema ?? {},
  };
}

/** Everything currently registered, for local diagnostics. */
export function listUiActions(): UiAction[] {
  return [...registry.values()];
}

/** Add actions to the registry and republish it. Returns the unregister half. */
export function registerUiActions(actions: UiAction[]): () => void {
  for (const a of actions) registry.set(a.id, a);
  schedulePublish();
  return () => {
    for (const a of actions) {
      // Identity check, not id check: React strict mode mounts twice, and the
      // first cleanup must not drop the second mount's registration.
      if (registry.get(a.id) === a) registry.delete(a.id);
    }
    schedulePublish();
  };
}

function schedulePublish(): void {
  if (!isAgentHost() || timer) return;
  timer = setTimeout(publish, DEBOUNCE_MS);
}

function publish(): void {
  timer = null;
  if (!isAgentHost()) return;
  // `run` is a function and cannot cross the IPC boundary — the backend only
  // ever sees the manifest. It replaces the registry wholesale, so this sends
  // every page's slice, not just the one that changed.
  const actions = [...registry.values()].map((a) => ({ ...manifestOf(a), tab: a.tab }));
  invoke("agent_report_ui_actions", { actions }).catch(() => {});
}

/**
 * Register `actions` for `tab` for as long as this component is mounted.
 *
 * The array is expected to be rebuilt on every render — the closures have to
 * capture fresh state — so registration is keyed on the *manifest* (ids,
 * labels, kinds, schemas) rather than on array identity. Re-registering on
 * every render would republish on every keystroke; and keying on the manifest
 * means adding a parameter to an action does republish, which is correct,
 * because that is what the agent reads.
 */
export function useAgentActions(tab: string, actions: UiActionDef[]): void {
  const latest = useRef(actions);
  latest.current = actions;

  const manifestKey = JSON.stringify(actions.map(manifestOf));

  useEffect(() => {
    // Stamp the tab and delegate `run` to whatever the latest render provided,
    // so a registered action never calls a stale closure.
    const stamped: UiAction[] = latest.current.map((def) => ({
      ...def,
      tab,
      run: (params) => {
        const live = latest.current.find((a) => a.id === def.id);
        if (!live) {
          throw new Error(
            `action '${def.id}' was unregistered while it was being invoked`
          );
        }
        return live.run(params);
      },
    }));
    return registerUiActions(stamped);
    // `manifestKey` stands in for `actions`: see the doc comment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manifestKey, tab]);
}

/**
 * Run a registered action and return whatever it returned.
 *
 * The error messages follow the house convention — name the valid set and the
 * next call — because they travel back to the agent verbatim as the invocation's
 * `detail`.
 */
export async function runUiAction(
  id: string,
  params: Record<string, unknown>
): Promise<unknown> {
  const action = registry.get(id);
  if (!action) {
    const known = [...registry.keys()].sort();
    throw new Error(
      known.length === 0
        ? `no UI actions are registered — the page that owns '${id}' is not mounted`
        : `unknown ui action '${id}'. Registered actions: ${known.join(", ")}`
    );
  }
  return await action.run(params ?? {});
}

/** Coerce an action's return value into something Tauri can serialise.
 *
 *  The result travels back over `agent_report_action_result`, which is
 *  JSON-encoded: an action that returns a function, a `Map` or a circular
 *  structure would fail the report and cost the agent a full 10s timeout for an
 *  action that actually succeeded. Degrading to a string keeps the answer
 *  arriving. */
export function jsonSafeDetail(value: unknown): unknown {
  if (value === null || value === undefined) return null;
  const t = typeof value;
  if (t === "string" || t === "number" || t === "boolean") return value;
  try {
    JSON.stringify(value);
    return value;
  } catch {
    return String(value);
  }
}

// ---------------------------------------------------------------------------
// Parameter helpers
//
// Actions receive an untyped bag, so every page would otherwise re-write the
// same three coercions and its own error wording. These throw the house-style
// message, which reaches the agent as the failure reason.
// ---------------------------------------------------------------------------

function describe(actionId: string, name: string): string {
  return `action '${actionId}' parameter '${name}'`;
}

/** A required string parameter. Empty and whitespace-only are both rejected:
 *  an agent passing `""` for a uuid is a mistake, not a filter. */
export function reqStr(
  actionId: string,
  params: Record<string, unknown>,
  name: string
): string {
  const v = params[name];
  if (typeof v !== "string" || v.trim() === "") {
    throw new Error(`${describe(actionId, name)} must be a non-empty string, got ${JSON.stringify(v ?? null)}`);
  }
  return v.trim();
}

/** An optional string parameter; `undefined` when absent or empty. */
export function optStr(
  actionId: string,
  params: Record<string, unknown>,
  name: string
): string | undefined {
  const v = params[name];
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "string") {
    throw new Error(`${describe(actionId, name)} must be a string, got ${JSON.stringify(v)}`);
  }
  return v.trim() === "" ? undefined : v.trim();
}

/** A required number parameter. A numeric string is accepted: agents quote
 *  numbers often enough that refusing one costs a round trip for nothing. */
export function reqNum(
  actionId: string,
  params: Record<string, unknown>,
  name: string
): number {
  const v = params[name];
  const n = typeof v === "number" ? v : typeof v === "string" ? Number(v) : Number.NaN;
  if (!Number.isFinite(n)) {
    throw new Error(`${describe(actionId, name)} must be a number, got ${JSON.stringify(v ?? null)}`);
  }
  return n;
}

/** A value that must be one of `allowed`; the error names the set. */
export function oneOf<T extends string>(
  actionId: string,
  params: Record<string, unknown>,
  name: string,
  allowed: readonly T[],
  fallback: T
): T {
  const v = params[name];
  if (v === undefined || v === null || v === "") return fallback;
  if (typeof v === "string" && (allowed as readonly string[]).includes(v)) return v as T;
  throw new Error(
    `${describe(actionId, name)} must be one of ${allowed.join(" | ")}, got ${JSON.stringify(v)}`
  );
}
