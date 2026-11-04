"use client";

/**
 * Understanding tabs — the shared vocabulary of the understanding layer's UI.
 *
 * The sub-toolbar ([UnderstandingToolbar](./UnderstandingToolbar.tsx)) renders
 * one button per tab and the panel body
 * ([UnderstandingPanel](./UnderstandingPanel.tsx)) renders the active one, so
 * the tab id and its order live in one place both read. Order follows the
 * learner's arc: orient (Digest) → drill in (Explain, Simplify, Translate,
 * Argument) → test (Quiz) → produce (Notes, Glossary).
 */

export type UnderstandingTab =
  | "digest"
  | "explain"
  | "simplify"
  | "translate"
  | "argument"
  | "quiz"
  | "notes"
  | "glossary";

export const UNDERSTANDING_TABS: { id: UnderstandingTab; label: string }[] = [
  { id: "digest", label: "Digest" },
  { id: "explain", label: "Explain" },
  { id: "simplify", label: "Simplify" },
  { id: "translate", label: "Translate" },
  { id: "argument", label: "Argument" },
  { id: "quiz", label: "Quiz" },
  { id: "notes", label: "Notes" },
  { id: "glossary", label: "Glossary" },
];

export const DEFAULT_UNDERSTANDING_TAB: UnderstandingTab = "digest";

/** localStorage guard: a stored id is honored only if it still names a tab. */
export function isUnderstandingTab(v: string | null): v is UnderstandingTab {
  return !!v && UNDERSTANDING_TABS.some((t) => t.id === v);
}
