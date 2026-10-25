/**
 * Client-side helpers for the book "understanding layer".
 *
 * Prompt-free: the LLM work happens in the backend (`book_artifacts.rs`). This
 * module only groups artifacts by scope, parses/validates the quiz JSON the
 * `questions` artifact carries, grades an attempt, and maps each kind to a
 * label/icon for the UI.
 */

import {
  Lightbulb,
  TextQuote,
  ListTree,
  Network,
  HelpCircle,
  StickyNote,
  ClipboardCheck,
  BookMarked,
  type LucideIcon,
} from "lucide-react";
import type {
  ArtifactKind,
  BookArtifactView,
  QuizQuestion,
  QuizMcq,
} from "@/lib/read/types";

// ---------------------------------------------------------------------------
// Grouping
// ---------------------------------------------------------------------------

/** Index artifacts by the scope uuid they are attached to. */
export function groupArtifactsByScope(
  artifacts: BookArtifactView[]
): Map<string, BookArtifactView[]> {
  const map = new Map<string, BookArtifactView[]>();
  for (const a of artifacts) {
    const list = map.get(a.scope_uuid);
    if (list) list.push(a);
    else map.set(a.scope_uuid, [a]);
  }
  return map;
}

/** The single artifact of `kind` for a scope, if any (generated kinds are unique per scope). */
export function findArtifact(
  artifacts: BookArtifactView[] | undefined,
  kind: ArtifactKind
): BookArtifactView | undefined {
  return artifacts?.find((a) => a.kind === kind);
}

/** All notes for a scope, newest first. */
export function notesForScope(
  artifacts: BookArtifactView[] | undefined
): BookArtifactView[] {
  return (artifacts ?? [])
    .filter((a) => a.kind === "note")
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
}

/** The assessment artifact linked to a note, if one exists. */
export function assessmentForNote(
  artifacts: BookArtifactView[] | undefined,
  noteUuid: string
): BookArtifactView | undefined {
  return (artifacts ?? []).find(
    (a) => a.kind === "assessment" && a.parent_uuid === noteUuid
  );
}

// ---------------------------------------------------------------------------
// Quiz parsing + grading
// ---------------------------------------------------------------------------

/**
 * Parse and validate the `questions` artifact body into a quiz. Malformed
 * elements are dropped rather than throwing, so a partially-valid LLM response
 * still yields a usable quiz.
 */
export function parseQuiz(content: string): QuizQuestion[] {
  let raw: unknown;
  try {
    raw = JSON.parse(content);
  } catch {
    // Some models wrap the array in a code fence; recover the first [...] block.
    const match = content.match(/\[[\s\S]*\]/);
    if (!match) return [];
    try {
      raw = JSON.parse(match[0]);
    } catch {
      return [];
    }
  }
  if (!Array.isArray(raw)) return [];
  const out: QuizQuestion[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const q = item as Record<string, unknown>;
    const stem = typeof q.stem === "string" ? q.stem : "";
    if (!stem) continue;
    if (q.type === "mcq") {
      const choices = Array.isArray(q.choices)
        ? q.choices.filter((c): c is string => typeof c === "string")
        : [];
      const correct = typeof q.correct === "number" ? q.correct : -1;
      if (choices.length < 2 || correct < 0 || correct >= choices.length) continue;
      out.push({
        type: "mcq",
        stem,
        choices,
        correct,
        explanation: typeof q.explanation === "string" ? q.explanation : "",
      });
    } else if (q.type === "open") {
      out.push({
        type: "open",
        stem,
        model_answer: typeof q.model_answer === "string" ? q.model_answer : "",
        explanation: typeof q.explanation === "string" ? q.explanation : "",
      });
    }
  }
  return out;
}

/** An answer to one quiz question: the chosen index (mcq) or a self-grade (open). */
export type QuizAnswer =
  | { type: "mcq"; selected: number | null }
  | { type: "open"; graded: boolean | null };

/** A blank answer sheet for a quiz. */
export function emptyAnswers(questions: QuizQuestion[]): QuizAnswer[] {
  return questions.map((q) =>
    q.type === "mcq" ? { type: "mcq", selected: null } : { type: "open", graded: null }
  );
}

/** True when an MCQ answer matches the correct index. */
export function isMcqCorrect(q: QuizMcq, selected: number | null): boolean {
  return selected !== null && selected === q.correct;
}

/** Count correct answers across a finished quiz. Open questions count when self-graded correct. */
export function scoreQuiz(
  questions: QuizQuestion[],
  answers: QuizAnswer[]
): { total: number; correct: number } {
  let correct = 0;
  for (let i = 0; i < questions.length; i++) {
    const q = questions[i];
    const a = answers[i];
    if (!a) continue;
    if (q.type === "mcq" && a.type === "mcq" && isMcqCorrect(q, a.selected)) correct++;
    else if (q.type === "open" && a.type === "open" && a.graded === true) correct++;
  }
  return { total: questions.length, correct };
}

// ---------------------------------------------------------------------------
// Labels + icons
// ---------------------------------------------------------------------------

const KIND_META: Record<ArtifactKind, { label: string; icon: LucideIcon }> = {
  explanation: { label: "Explain", icon: Lightbulb },
  simplification: { label: "Simplify", icon: TextQuote },
  digest: { label: "Digest", icon: ListTree },
  argument_map: { label: "Argument", icon: Network },
  questions: { label: "Quiz", icon: HelpCircle },
  note: { label: "Note", icon: StickyNote },
  assessment: { label: "Assessment", icon: ClipboardCheck },
};

/** Human label for an artifact kind. */
export function kindLabel(kind: string): string {
  return KIND_META[kind as ArtifactKind]?.label ?? kind;
}

/** Lucide icon component for an artifact kind. */
export function kindIcon(kind: string): LucideIcon {
  return KIND_META[kind as ArtifactKind]?.icon ?? BookMarked;
}

/** Badge text for an artifact/term `source` value. */
export function sourceLabel(source: string): string {
  switch (source) {
    case "ollama":
      return "AI";
    case "agent":
      return "Agent";
    case "user":
      return "You";
    default:
      return source || "—";
  }
}
