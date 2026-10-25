// ---------------------------------------------------------------------------
// Publish the workflow step-op vocabulary (from `lib/workflow/step-ops.ts`) to
// the backend, so `workflow_list_actions` and the agent manual can describe
// what an authored `workflow.yaml` step may name.
//
// The registry lives here, in TypeScript, because the Workflow page is what
// runs a step. Rust deliberately keeps no second copy: the page publishes the
// vocabulary, the tools read it back. That is why this is a one-way report and
// nothing ever asks the backend what the vocabulary is.
//
// Wire contract (mirrors `src-tauri/src/ui_state.rs`):
//   { actions: StepOpDoc[], commands: string[] }
// ---------------------------------------------------------------------------

import { invoke } from "@tauri-apps/api/core";
import { isAgentHost } from "./host";
import { KNOWN_COMMANDS, stepOpDocs, type StepOpDoc } from "../workflow/step-ops";

/** What the backend holds after a successful report. */
export interface WorkflowActionSnapshot {
  actions: StepOpDoc[];
  /** The flat set of Tauri commands a step's `action` may name. */
  commands: string[];
  /** RFC 3339 stamp the backend added when it accepted the report. */
  updated_at?: string;
}

/** Payload last accepted by the backend, so a remount is a no-op. */
let published: string | null = null;
/** The report in flight, if any — concurrent mounts share it. */
let inFlight: Promise<void> | null = null;

/** Report the step-op vocabulary once per webview session (cheap to repeat,
 *  but pointless). Never throws: the backend cache is a convenience for
 *  agents, never something the UI waits on. */
export function publishWorkflowActions(): Promise<void> {
  if (!isAgentHost()) return Promise.resolve();
  if (inFlight) return inFlight;

  const actions = stepOpDocs();
  const payload = JSON.stringify({ actions, commands: KNOWN_COMMANDS });
  if (published === payload) return Promise.resolve();

  inFlight = invoke("agent_report_workflow_actions", {
    actions,
    commands: KNOWN_COMMANDS,
  })
    .then(() => {
      published = payload;
    })
    .catch(() => {
      // A failed report only costs an agent some context; retry on next mount.
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}
