// ---------------------------------------------------------------------------
// Verification verdicts — the projection `workflow_verify` returns.
//
// One join, in one place. A step's `verify:` binding, the raw `dataset_audit`
// findings and the run's statuses are combined in Rust (`workflow::core::verify`
// + `workflow::verify`); this module only *renders* that result. Which check
// speaks for which step is a property of the definition, so nothing in here may
// name a step id, a check id or a repair sentence — that is exactly the
// duplication the binding was introduced to remove.
// ---------------------------------------------------------------------------

import type { AuditCheck, DatasetAudit } from "@/lib/datasets/types";

/** Mirrors the verdict string `workflow::core::verify` emits. */
export type StepVerdict = "verified" | "drifted" | "unknown";

/** One bound check that reported offenders. */
export interface VerdictFinding {
  check: string;
  count: number;
}

/** Per-step entry of the `verdicts` map. */
export interface StepVerdictEntry {
  /** The step's status in the run at probe time. */
  status: string;
  verdict: StepVerdict;
  /** Bound checks whose findings mean "not yet". */
  blocks?: VerdictFinding[];
  /** Bound checks whose findings contradict a `completed` claim. */
  proves?: VerdictFinding[];
  /** The ids the definition binds, whether or not they reported anything. */
  bound?: { blocks?: string[]; proves?: string[] };
  /** Bound ids this probe could not evaluate (another domain's vocabulary). */
  undecidable?: string[];
  /** Why the verdict is `unknown` when the step declares no binding at all. */
  reason?: string;
}

/** The subset of the `workflow_verify` payload the page consumes. */
export interface VerifyReport {
  run_id?: string;
  run_status?: string;
  verdicts?: Record<string, StepVerdictEntry>;
  verification?: {
    apply?: boolean;
    changed?: boolean;
    drifted?: string[];
    gated?: string[];
    unbound?: string[];
    undecidable?: string[];
    applied?: { step: string; from: string; to: string }[];
  };
  verify_hint?: string;
}

export const VERDICT_LABEL: Record<StepVerdict, string> = {
  verified: "evidence agrees",
  drifted: "evidence disagrees",
  unknown: "no evidence bound",
};

/** `id → the report row that describes it`, for labels and advice. */
export function checksById(audit: DatasetAudit | null): Map<string, AuditCheck> {
  return new Map((audit?.checks ?? []).map((c) => [c.id, c]));
}

const findings = (e: StepVerdictEntry, side: "blocks" | "proves"): VerdictFinding[] =>
  e[side] ?? [];

/** "Waveform older than its audio (3)" — the report's own words, never ours. */
export function findingLabel(f: VerdictFinding, byId: Map<string, AuditCheck>): string {
  const check = byId.get(f.check);
  return `${check?.label ?? f.check} (${f.count})`;
}

/** The advice sentence for one finding, from the report. */
export function findingAdvice(f: VerdictFinding, byId: Map<string, AuditCheck>): string {
  return byId.get(f.check)?.advice ?? "";
}

/**
 * A step's `blocks` findings as one gate sentence: why attempting it now would be
 * premature. `undefined` when the evidence says nothing is missing — the point of
 * the binding is that this sentence is assembled from the report, not retyped.
 */
export function gateReason(
  stepId: string,
  report: VerifyReport | null,
  byId: Map<string, AuditCheck>,
): string | undefined {
  const entry = report?.verdicts?.[stepId];
  if (!entry) return undefined;
  const blocked = findings(entry, "blocks");
  if (!blocked.length) return undefined;
  const parts = blocked.map((f) => {
    const label = findingLabel(f, byId);
    const advice = findingAdvice(f, byId);
    return advice ? `${label} — ${advice}` : label;
  });
  return `Evidence says not yet: ${parts.join(" ")}`;
}

/**
 * A step's `proves` findings as one drift sentence — what disagrees with the
 * progress the run claims. Rendered beside the node's own status, never instead
 * of it: a verdict is evidence about the world, not progress.
 */
export function driftReason(
  stepId: string,
  report: VerifyReport | null,
  byId: Map<string, AuditCheck>,
): string | undefined {
  const entry = report?.verdicts?.[stepId];
  if (!entry || entry.verdict !== "drifted") return undefined;
  const names = findings(entry, "proves").map((f) => findingLabel(f, byId));
  if (!names.length) return undefined;
  return `The evidence contradicts this step: ${names.join(", ")}.`;
}

/** Tooltip text for a node badge: verdict + binding + what could not be judged. */
export function verdictTitle(
  stepId: string,
  report: VerifyReport | null,
  byId: Map<string, AuditCheck>,
): string {
  const entry = report?.verdicts?.[stepId];
  if (!entry) return "Not verified yet — press Scan status.";
  if (entry.reason) return entry.reason;
  const bits: string[] = [`verdict: ${VERDICT_LABEL[entry.verdict]}`];
  const bound = entry.bound;
  bits.push(`proves: ${bound?.proves?.length ? bound.proves.join(", ") : "—"}`);
  bits.push(`blocks: ${bound?.blocks?.length ? bound.blocks.join(", ") : "—"}`);
  const findingsAll = [...findings(entry, "blocks"), ...findings(entry, "proves")];
  if (findingsAll.length) {
    bits.push(findingsAll.map((f) => findingLabel(f, byId)).join(", "));
  }
  if (entry.undecidable?.length) {
    bits.push(`not evaluated by this probe: ${entry.undecidable.join(", ")}`);
  }
  return bits.join(" · ");
}

/**
 * Steps the evidence has something to say about, for the panel's per-step view:
 * a drifted step, or a step whose binding reported findings. The flat check list
 * below it stays the subject-side detail; this is the pipeline-side reading of the
 * same probe, and both come from one call.
 */
export interface StepEvidenceRow {
  stepId: string;
  entry: StepVerdictEntry;
  findings: VerdictFinding[];
}

export function stepsWithEvidence(report: VerifyReport | null): StepEvidenceRow[] {
  const rows: StepEvidenceRow[] = [];
  for (const [stepId, entry] of Object.entries(report?.verdicts ?? {})) {
    const found = [...findings(entry, "proves"), ...findings(entry, "blocks")];
    if (entry.verdict === "drifted" || found.length) {
      rows.push({ stepId, entry, findings: found });
    }
  }
  return rows;
}

/** How many completed steps the evidence contradicts — the header's "drift". */
export function driftedCompleted(report: VerifyReport | null): string[] {
  return report?.verification?.drifted ?? [];
}
