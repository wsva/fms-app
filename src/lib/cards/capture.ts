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
//
// The card keeps what the todo could not: `sourceNote` writes a provenance line
// into the note — the text's name and uuid, then the dataset's name, kind and
// uuid — so an answer can still be written from the original text on a device
// that has neither.
// ---------------------------------------------------------------------------

import { invoke } from "@tauri-apps/api/core";

import { getUiContext } from "@/lib/agent/ui-context";
import type { OpenCardAction } from "@/lib/agent/types";
import { getActiveTabId } from "@/lib/refresh/page-refresh";
import { cardSubjectLabel } from "@/lib/shell/tab-subject";
import { TAB_DEFS } from "@/components/layout/Sidebar";
import type { TabId } from "@/lib/shell/tabs";
import type { CardDatasetSummary } from "@/lib/types";
import type { FamilyMember, FamilyReport } from "@/lib/datasets/family";

/** Caps, in characters. */
const MAX_QUESTION = 2000;
const MAX_CONTEXT = 2000;
const MAX_DETAIL = 160;
/** Detail width kept for the "— «quote»" half, leaving room for the tail. */
const MAX_DETAIL_BODY = 120;
/** The containing paragraph kept in a card's note. */
const MAX_NOTE_CONTEXT = 600;
/** Title/heading width kept in the note's first line. */
const MAX_NOTE_TITLE = 80;

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

/**
 * Where a selection came from, in the one line both of its readers can use:
 *
 *   From “Chapter 1 · The Boy Who Lived · part 1/11” (uuid=acb5…) of dataset
 *   “Harry Potter and the Philosopher's Stone” (kind=read_aloud, uuid=card…)
 *
 * The names are what the learner recognises; the ids are what an agent feeds
 * straight back into `book_list_sentences`, `wiki_dataset_read_file`, the
 * read-aloud tools or the cue tools to answer from the original context. Both
 * are read at capture time — a later lookup would describe another document.
 */
export type CaptureOrigin = {
  /** The dataset's uuid — the one thing needed to open it again. */
  uuid: string;
  /** Display name; "" when the dataset could not be read (moved, deleted). */
  name: string;
  /** Dataset type slug (`book`, `wiki`, `read_aloud`, `dictation`). */
  type: string;
  /** The item the selection sits in, when the page addresses one. */
  item: CaptureItem | null;
};

/** The text inside the dataset: a book chapter, wiki file, read-aloud text or media. */
export type CaptureItem = {
  /** The noun for an unnamed item: `chapter`, `text`, `file`, `media`, `item`. */
  noun: string;
  /** Its title; null when the page only knows it by id. */
  name: string | null;
  /** The parenthetical: `uuid=<id>`, `uuid=<id>, cue=<n>`, or `path=<rel>`. */
  where: string;
};

export type Capture = {
  question: string;
  /** The containing paragraph; "" when it is the question itself. */
  context: string;
  source: CaptureSource;
  /** Named and located origin; null on a page that is not inside a dataset. */
  origin: CaptureOrigin | null;
  /** What the card's note gets — see {@link sourceNote}. */
  note: string;
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
  return fileStem(ref);
}

/** A wiki `ref`'s last path segment, extension dropped. */
function fileStem(ref: string): string | null {
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
  datasetUuid: string,
  origin: CaptureOrigin | null = null
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
    origin,
    note: sourceNote(origin, ctx, question),
    datasetUuid,
    capturedAt: new Date().toISOString(),
  };
}

/**
 * The provenance line a captured card carries in its note, plus the paragraph
 * it came out of:
 *
 *   From “Chapter 1 · The Boy Who Lived · part 1/11” (uuid=acb5…) of dataset
 *   “Harry Potter and the Philosopher's Stone” (kind=read_aloud, uuid=card…)
 *   > …the sentence, as it stood when the card was taken…
 *
 * One line, two readers: the quotes name the text and the dataset the way the
 * reader knows them, and the parentheticals carry the ids an agent needs to open
 * the very same item — the item as `uuid=` (chapter, text, media with a `cue`)
 * or `path=` (a wiki file), the dataset as `kind=` + `uuid=`. Both halves are
 * frozen at capture time; a later lookup would describe another document.
 *
 * The quoted paragraph is deliberately a copy: the note has to answer "what did
 * this come from" on a device that never had the dataset, and after the
 * document has been edited or deleted.
 */
export function sourceNote(
  origin: CaptureOrigin | null,
  ctx: CaptureContext,
  question: string
): string {
  if (!origin) return "";
  const kind = origin.type || "unknown";
  const work = origin.name
    ? `dataset “${clip(origin.name, MAX_NOTE_TITLE)}” (kind=${kind}, uuid=${origin.uuid})`
    : `dataset (kind=${kind}, uuid=${origin.uuid})`;

  const item = origin.item;
  // A page that publishes the item but not its title still had a heading above
  // the selection; that is the closest thing to a name there is.
  const label = item?.name ?? (ctx.heading ? clip(ctx.heading, MAX_NOTE_TITLE) : null);
  const head = !item
    ? `From ${work}`
    : label
      ? `From “${label}” (${item.where}) of ${work}`
      : `From the ${item.noun} (${item.where}) of ${work}`;

  const lines = [head];
  const quote = clip(ctx.block, MAX_NOTE_CONTEXT);
  if (quote && quote !== clip(question, MAX_NOTE_CONTEXT)) lines.push(`> ${quote}`);
  return lines.join("\n");
}

/**
 * Where the selection came from, named and located.
 *
 * Two lookups, both at capture time: the dataset's own `info.json` (name + type)
 * and, when a document page publishes one, the title of the item inside it — a
 * book chapter and a read-aloud text are addressed by uuid, so their titles are
 * not on screen to read. A wiki `ref` *is* a path, so its stem is the name.
 *
 * Every failure is survivable: a moved, deleted or unreadable dataset costs the
 * friendly name, never the id, because the note exists to be *re-found*.
 */
export async function captureOrigin(): Promise<CaptureOrigin | null> {
  const uuid = currentDatasetUuid();
  if (!uuid) return null;

  const ctx = getUiContext();
  const [kind, , ref] = splitDoc(ctx.active_doc ?? "");
  let name = "";
  let type = "";
  try {
    const detail = await invoke<{ info: { name: string; type: string } }>("dataset_get", {
      uuid,
    });
    name = detail.info.name;
    type = detail.info.type;
  } catch {
    type = kind;
  }

  // The item first without its title: the id is what must survive a lookup that
  // fails, so it is built before the invoke that can throw.
  let item: CaptureItem | null = ref
    ? {
        noun: kind === "book" ? "chapter" : kind === "read_aloud" ? "text" : kind === "wiki" ? "file" : "item",
        name: kind === "wiki" ? fileStem(ref) : null,
        where: kind === "wiki" ? `path=${ref}` : `uuid=${ref}`,
      }
    : null;

  if (item && (kind === "book" || kind === "read_aloud")) {
    try {
      if (kind === "book") {
        const chapters = await invoke<{ uuid: string; title: string }[]>("book_list_chapters", {
          bookUuid: uuid,
        });
        item.name = chapters.find((c) => c.uuid === ref)?.title.trim() || null;
      } else {
        const texts = await invoke<{ uuid: string; title: string }[]>("read_aloud_list_texts", {
          datasetUuid: uuid,
        });
        item.name = texts.find((t) => t.uuid === ref)?.title.trim() || null;
      }
    } catch {
      // The heading the page showed stands in for the title.
    }
  }

  // Dictation has no `active_doc`: the media item and the cue are its locator.
  if (!item && ctx.active_media_uuid) {
    const cue = typeof ctx.cue_index === "number" ? `, cue=${ctx.cue_index}` : "";
    item = { noun: "media", name: null, where: `uuid=${ctx.active_media_uuid}${cue}` };
  }

  return { uuid, name, type, item };
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
      note: capture.note,
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
 * The dataset the reader is in — what a family question is asked about, and what
 * the note's `dataset` half names.
 *
 * Two sources, because no page publishes both keys: Dictation and Cards report
 * `active_dataset_uuid`; the document pages report `active_doc` (`<kind>:`
 * `<dataset>:<ref>`). `active_doc` wins whenever it is set: it is published by
 * the page that has a document on screen, and its second segment *is* that
 * document's dataset. `active_dataset_uuid` is shared with the card editor and
 * the Cards page, which cannot clear a deck they no longer have selected — a
 * leftover value there must never outvote the document the selection was taken
 * from.
 */
function currentDatasetUuid(): string | null {
  const ctx = getUiContext();
  const doc = ctx.active_doc;
  if (doc) {
    const uuid = splitDoc(doc)[1];
    if (uuid) return uuid;
  }
  return ctx.active_dataset_uuid ?? null;
}

/**
 * The card dataset of the family the reader is reading in, or `null` when there
 * is none — a selection in an unaffiliated dataset, or in an app surface with no
 * dataset at all.
 *
 * Unlike {@link pickCaptureDeck} this asks the backend about one family instead
 * of choosing among decks: "add to this work's cards" has exactly one right
 * answer, and guessing another deck would file the card somewhere the reader
 * cannot find it from the text they took it out of.
 */
export async function pickFamilyDeck(): Promise<FamilyMember | null> {
  const uuid = currentDatasetUuid();
  if (!uuid) return null;
  try {
    const report = await invoke<FamilyReport>("dataset_family", { uuidOrKey: uuid });
    return report.members.find((m) => m.dataset_type === "card") ?? null;
  } catch {
    return null;
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
