// ---------------------------------------------------------------------------
// Card capture: a text selection turned into a Tasks todo, not an editor.
//
// "Add to Card" used to open the card editor on the spot, which meant the
// reader had to write an answer before they could go back to what they were
// reading. The capture keeps the raw material instead — the quote, the
// paragraph it sits in, and where it came from — and hands the writing to
// later, when the learner opens the todo.
//
// Everything here is derived from what is already on screen at the moment of
// the selection: the tab, the `active_doc` key (see `lib/agent/ui-context`),
// and the DOM the range points into. Nothing is re-resolved later, because
// "later" may be another device, another window, or after the document changed.
//
// Size discipline: todos sync to every device through the change log, so the
// snapshot is capped rather than complete. The paragraph is context for reading
// a quote out of its sentence, not a copy of the file.
// ---------------------------------------------------------------------------

import { invoke } from "@tauri-apps/api/core";

import { getUiContext } from "@/lib/agent/ui-context";
import type { OpenCardAction } from "@/lib/agent/types";
import { getActiveTabId } from "@/lib/refresh/page-refresh";
import { cardSubjectLabel } from "@/lib/shell/tab-subject";
import { TAB_DEFS } from "@/components/layout/Sidebar";
import type { TabId } from "@/lib/shell/tabs";
import type { CardDatasetSummary } from "@/lib/types";

/** Caps, in characters. */
const MAX_QUESTION = 2000;
const MAX_CONTEXT = 2000;
const MAX_DETAIL = 160;
/** Detail width kept for the "— «quote»" half, leaving room for the tail. */
const MAX_DETAIL_BODY = 120;

/** Where the last capture went, so the next one does not have to ask. */
const DECK_KEY = "cards.capture-deck";

/** What the selection's surroundings looked like, read at mouseup time. */
export type CaptureContext = {
  /** Nearest block ancestor's text (or the field's value for an input). */
  block: string;
  /** Nearest heading above the selection, when the page has one. */
  heading?: string;
};

export type CaptureSource = {
  /** The tab the selection was made in — `"wiki"`, `"books"`, `"dictation"`… */
  kind: string;
  /** The `active_doc` value at capture time, when there was one. */
  doc: string | null;
  /** One readable line: `Wiki › datadog · § Setup`. Never a filesystem path. */
  display: string;
};

export type Capture = {
  question: string;
  /** The containing paragraph; "" when it is the question itself. */
  context: string;
  source: CaptureSource;
  datasetUuid: string;
  capturedAt: string;
};

/** Collapse whitespace and cap. Long text is cut with an ellipsis, not silently. */
function clip(text: string, max: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  return `${flat.slice(0, Math.max(1, max - 1)).trimEnd()}…`;
}

function tabLabel(tab: string | null): string {
  if (!tab) return "App";
  return TAB_DEFS[tab as TabId]?.label ?? "App";
}

/**
 * The document's own name, when `active_doc` carries one.
 *
 * Only a wiki `ref` is a path with a human-readable last segment. A book
 * chapter and a read-aloud text are addressed by uuid, so they contribute
 * nothing here and lean on the heading instead of showing a uuid to a reader.
 */
function docLabel(doc: string | null): string | null {
  if (!doc) return null;
  const [, , ref] = splitDoc(doc);
  if (!ref || !ref.includes("/")) return null;
  const base = ref.split("/").pop() ?? "";
  const stem = base.replace(/\.[a-z0-9]+$/i, "");
  return stem || null;
}

/** `kind:uuid:ref`, with the ref kept whole even when it contains ':'. */
function splitDoc(doc: string): [string, string, string] {
  const first = doc.indexOf(":");
  const last = doc.lastIndexOf(":");
  if (first < 0) return [doc, "", ""];
  if (first === last) return [doc.slice(0, first), doc.slice(first + 1), ""];
  return [doc.slice(0, first), doc.slice(first + 1, last), doc.slice(last + 1)];
}

/** The one-line "where": tab, then document, then heading. */
export function sourceDisplay(
  tab: string | null,
  doc: string | null,
  heading?: string
): string {
  const parts = [tabLabel(tab)];
  const label = docLabel(doc);
  if (label) parts.push(label);
  const trail = parts.join(" › ");
  const head = heading ? clip(heading, 60) : "";
  return head ? `${trail} · § ${head}` : trail;
}

/** Snapshot one selection. Anything longer than the caps is clipped, not refused. */
export function buildCapture(
  selection: string,
  ctx: CaptureContext,
  datasetUuid: string
): Capture {
  const question = clip(selection, MAX_QUESTION);
  const block = clip(ctx.block, MAX_CONTEXT);
  const doc = getUiContext().active_doc ?? null;
  const tab = getActiveTabId();
  return {
    question,
    // A one-sentence paragraph keeps no extra context worth carrying twice.
    context: block && block !== question ? block : "",
    source: { kind: tab ?? "other", doc, display: sourceDisplay(tab, doc, ctx.heading) },
    datasetUuid,
    capturedAt: new Date().toISOString(),
  };
}

/** The todo's title: what the card will be about, not "Add card". */
export function captureTitle(capture: Capture): string {
  return cardSubjectLabel(capture.question, 60) || "Card from selection";
}

/**
 * The todo's one-line detail: where it came from and what was selected.
 *
 * It is what the Tasks page and the status-bar menu show, so it has to survive
 * a `truncate` and still name a place — the quote alone would be unreadable a
 * week later, and the source alone would say nothing about the card.
 */
export function captureDetail(capture: Capture, similar: number): string {
  const quote = clip(capture.question, MAX_DETAIL_BODY);
  const body = `${capture.source.display} — «${quote}»`;
  const tail = similar > 0 ? ` · ${similar} similar` : "";
  return clip(`${body}${tail}`, MAX_DETAIL);
}

/**
 * The payload the Tasks tab replays on tap.
 *
 * `todo_uuid` is absent on purpose: `todo_add` mints the uuid, a todo cannot be
 * rewritten afterwards, and the replay already knows which todo it was opened
 * from — `replayTaskTarget` hands it in. `completes` is what makes that id
 * meaningful: saving the card closes the capture instead of the tap.
 */
export function captureTarget(capture: Capture, datasetName?: string): OpenCardAction {
  return {
    type: "open-card",
    dataset_uuid: capture.datasetUuid,
    dataset_name: datasetName,
    prefill: {
      question: capture.question,
      answer: "",
      suggestion: "",
      note: "",
    },
    context: capture.context,
    source: { display: capture.source.display },
    completes: "on_save",
  };
}

/** The deck the last capture went to, if it still exists. */
function rememberedDeck(datasets: CardDatasetSummary[]): string | null {
  if (typeof window === "undefined") return null;
  const uuid = window.localStorage.getItem(DECK_KEY);
  if (!uuid) return null;
  return datasets.some((d) => d.info.uuid === uuid) ? uuid : null;
}

function rememberDeck(uuid: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(DECK_KEY, uuid);
  } catch {
    // A full or blocked storage costs the memory, not the capture.
  }
}

/**
 * Which deck a capture lands in.
 *
 * No prompt: interrupting a reader to ask "which deck?" is the interruption this
 * whole path exists to avoid. The order encodes what the reader most likely
 * meant — the deck they last captured into, else the deck they are looking at,
 * else the only deck there is. The toast names the answer, and the editor's
 * picker can move the card afterwards.
 */
export async function pickCaptureDeck(): Promise<CardDatasetSummary | null> {
  let datasets: CardDatasetSummary[] = [];
  try {
    datasets = await invoke<CardDatasetSummary[]>("card_dataset_list");
  } catch {
    return null;
  }
  if (datasets.length === 0) return null;

  const remembered = rememberedDeck(datasets);
  // `active_dataset_uuid` is shared with Dictation, so it only means "the deck
  // on screen" while the Cards page is the one reporting it.
  const onCards = getActiveTabId() === "cards";
  const viewed = onCards ? getUiContext().active_dataset_uuid ?? null : null;

  const chosen =
    remembered ??
    (viewed && datasets.some((d) => d.info.uuid === viewed) ? viewed : null) ??
    datasets[0].info.uuid;

  rememberDeck(chosen);
  return datasets.find((d) => d.info.uuid === chosen) ?? datasets[0];
}
