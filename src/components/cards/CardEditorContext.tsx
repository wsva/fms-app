"use client";

// ---------------------------------------------------------------------------
// The card editor: one inline-editable card surface, served to two hosts.
//
// The overlay is the original host: `fixed inset-0`, its own header, floating
// above whichever tab the shell is showing, because "Add to Card" has to work
// from a wiki page or a dictation cue without navigating anywhere first. The
// second host is a page that spreads the provider's `session` into the modal
// (`inline`): it supplies the surface, the breadcrumb and the name, so the Cards
// page can put the writer exactly where the reader already is. `inline` decides
// host chrome only — the state machine, the draft, the guard and the fields are
// the same object in both.
//
// What this file is responsible for, in the order the code below is written:
//
//   1. Never lose work. `baselineRef` is the last value read from or written to
//      disk; `dirty` is anything differing from it (or a new card with content,
//      which is equally not on disk). Every exit — Escape, Close, Back, a tab
//      replace, F5 — goes through the same guard, and the guard's sheet is an
//      inline panel rather than a native dialog, because a phone must be able to
//      answer it. A debounced localStorage draft is the backstop for the case no
//      guard catches: the process being killed.
//   2. Save means save. It writes, re-reads nothing away, keeps the card open
//      and editable, and says "Saved" in the header — or, inline, where the
//      header would have been. Closing is the user's act.
//   3. One layout. Fields render through `CardFields` in reading order —
//      question, answer, suggestion, note — and clicking one edits that one.
//      `viewMode` no longer picks a layout; "edit" is just every field open.
//      The body is a leaf (`CardFields`) fed by a `CardFieldBinding`, so a card
//      shown inside a page rather than over one gets the same body; everything
//      in this file is the chrome around it. The strip under the toolbar holds
//      two views of that body — the fields, and the card's media — because
//      a row of handles, kinds and their own buttons wants a column, not 288px.
//   4. Identity. The card it shows is published to the tab strip's subject store
//      (so the chip names it), into the address bar (so a restored window reopens
//      it) and into the agent's UI context (so "this card" means something).
//   5. Standing, decoded. Familiarity is per-user review state — see Phase 5 of
//      the design note — so the row names the value and the popover rates it
//      through `card_test_submit`, never through a text write.
//
// Attach and draft interact by design, not accident: `card_media_add` writes
// bytes and a row immediately and durably, while the `{{handle}}` token that
// points at them lives in text that is only durable once saved. The library
// therefore reports a row as "attached, not placed" rather than letting the
// mismatch look like a lost attachment.
// ---------------------------------------------------------------------------

import { useState, useEffect, useMemo, useRef, useCallback } from "react";
import { createContext, useContext } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  Check, ChevronLeft, ChevronRight, CircleHelp, Eye, Film, Frown, GraduationCap, Laugh,
  List, Meh, Paperclip, Pencil, Smile, Trash2, Type, X,
  Image as ImageIcon, Music2, Video,
} from "lucide-react";
import type { Card, CardDatasetSummary, CardMedia, CardMediaField, CardReview } from "@/lib/types";
import { isMobileApp, isTauri } from "@/lib/platform";
import { toHTML } from "@/components/wiki/markdown/html";
import { cardKindOf, cardExtsFor, CARD_IMAGE_EXTS, CARD_AUDIO_EXTS, CARD_VIDEO_EXTS } from "@/lib/media/kind";
import { stripAnchors } from "@/lib/media/anchors";
import { copyText } from "@/lib/clipboard";
import { notify, notifyError } from "@/lib/feedback/toast";
import {
  canRefreshInstance,
  getActiveTabId,
  setOverlayGuard,
} from "@/lib/refresh/page-refresh";
import {
  cardSubjectLabel,
  clearTabSubject,
  readCardFromUrl,
  setTabSubject,
  type TabSubject,
} from "@/lib/shell/tab-subject";
import { setUiContext } from "@/lib/agent/ui-context";
import CardFields, {
  CARD_FIELD_LABEL,
  CARD_FIELD_ORDER,
  type CardFieldBinding,
  type CardFieldValues,
} from "./CardFields";
import MediaLibrary from "./MediaLibrary";
import { CollapsibleSidebar, useCollapsibleSidebar } from "@/components/layout/CollapsibleSidebar";
import {
  PageHeader,
  TabBar,
  Toolbar,
  ToolbarButton,
  ToolbarMenuItem,
  ToolbarSelect,
  type TabBarItem,
} from "@/components/ui/toolbar";

type FontScaleKey = "normal" | "large" | "x2";

/** The editor's two views. Tabs, not actions: the same card, read as fields or
 *  as files. */
type CardEditorPane = "edit" | "media";

// Font size options: each step renders the content bigger and bolder.
const FONT_SCALE_MAP: Record<FontScaleKey, { zoom: number; weight: number }> = {
  normal: { zoom: 1, weight: 400 },
  large: { zoom: 1.3, weight: 500 },
  x2: { zoom: 1.6, weight: 600 },
};

// The label names the multiplier it applies. "Large x2" on its own said nothing
// about what you would get, and the three steps are 100 / 130 / 160 percent.
const FONT_SCALE_LABELS: Record<FontScaleKey, string> = {
  normal: "Normal 100%",
  large: "Large 130%",
  x2: "Large x2 160%",
};

/** What the trigger shows. A toolbar is a row of words, and "Large x2 160%" in it
 *  reads as a sentence; the percentage is the value and the menu keeps the names. */
const FONT_SCALE_SHORT: Record<FontScaleKey, string> = {
  normal: "100%",
  large: "130%",
  x2: "160%",
};

/** Above this, a card is read rather than glanced at: the biggest step would put
 *  several screens of answer under the fold, so a long answer defaults to large. */
const LONG_ANSWER_CHARS = 600;

// Familiarity tint ramp (theme-aware --fam-* tokens, see globals.css), shared
// with the Cards list rows. Literal class names keep Tailwind's scanner happy.
const FAM_CLASSES = [
  "bg-fam-0",
  "bg-fam-1",
  "bg-fam-2",
  "bg-fam-3",
  "bg-fam-4",
  "bg-fam-5",
  "bg-fam-6",
];

/** The ramp as `CardsPage.familiarityColor` documents it, so the row tint and
 *  this badge are one vocabulary instead of two guesses. */
const FAMILIARY_RAMP = [
  { f: 0, name: "new" },
  { f: 1, name: "strange" },
  { f: 2, name: "known" },
  { f: 3, name: "familiar" },
  { f: 4, name: "skilled" },
  { f: 5, name: "practiced" },
  { f: 6, name: "easy" },
];

/** The five grades the Review page offers, copied rather than re-invented: same
 *  quality numbers, same words, same colours, so `f=` means one thing anywhere. */
const GRADES = [
  { q: 1, label: "Forgot", Icon: CircleHelp },
  { q: 2, label: "Hard", Icon: Frown },
  { q: 3, label: "Okay", Icon: Meh },
  { q: 4, label: "Good", Icon: Smile },
  { q: 5, label: "Easy", Icon: Laugh },
] as const;

function famName(f: number): string {
  return FAMILIARY_RAMP[Math.min(Math.max(f, 0), 6)]?.name ?? "new";
}

const valuesOf = (source: Partial<Card> | null): CardFieldValues => ({
  question: source?.question ?? "",
  answer: source?.answer ?? "",
  note: source?.note ?? "",
  suggestion: source?.suggestion ?? "",
});

/** Where a new field value goes. Bound per render from the state setters, so the
 *  media seam can write any field without four copies of the same switch. */
type FieldSetter = (field: CardMediaField, value: string) => void;

// A dashed uuid matching the Rust `Uuid::new_v4().to_string()` the card table
// mints, so a pre-minted id is indistinguishable from a backend-minted one.
function mintCardUuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/** The persisted shape of an unsaved card, and the wiki draft's twin. */
type CardDraft = CardFieldValues;

const draftKeyFor = (datasetUuid: string, cardUuid: string) =>
  `card-draft:${datasetUuid}:${cardUuid}`;

function readDraft(key: string): CardDraft | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<CardDraft>;
    return {
      question: parsed.question ?? "",
      answer: parsed.answer ?? "",
      note: parsed.note ?? "",
      suggestion: parsed.suggestion ?? "",
    };
  } catch {
    // Unreadable or unavailable (private mode): the card opens from disk, which
    // is the safe direction — a lost draft must not become a wrong card.
    return null;
  }
}

function writeDraft(key: string, values: CardDraft): void {
  try {
    localStorage.setItem(key, JSON.stringify(values));
  } catch {
    /* quota or private mode: the dirty guard still protects the session */
  }
}

function clearDraft(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* nothing to clean up if storage is unavailable */
  }
}

/** What the guard's sheet is waiting on: an exit that must be confirmed, or the
 *  delete of the card on screen. Both are inline because a phone has no dialog. */
type PendingExit =
  | { kind: "exit"; what: "close" | "step"; run: () => void }
  | { kind: "delete" };

/** The list the editor walks with prev/next.
 *
 *  Carries uuids rather than cards on purpose: a step re-reads the row from disk,
 *  so a card edited by a save, a review, or another device is never shown from a
 *  stale snapshot, and the walk survives the list page reloading underneath it. */
export type CardQueue = {
  /** The loaded list, in the order the page shows it. */
  uuids: string[];
  /** Where the card on screen sits in `uuids`. */
  index: number;
  /** Ask the list for the next page. A queue with no paging makes it a no-op. */
  loadMore: (dir: -1 | 1) => void;
};

/**
 * The guard's inline action sheet.
 *
 * Deliberately not `ask()`: this decision has to be answerable on a phone, where
 * a native dialog is at best an interruption and at worst unsupported, and it has
 * three options rather than a yes/no. It renders as a bottom sheet on a phone and
 * a centred panel on a desktop, over the card it is protecting.
 */
function ExitSheet({
  pending,
  primaryRef,
  onConfirm,
  onDiscard,
  onKeep,
}: {
  pending: PendingExit;
  /** The default action takes focus, so Enter does the productive thing. */
  primaryRef: React.RefObject<HTMLButtonElement | null>;
  onConfirm: () => Promise<void>;
  onDiscard: () => Promise<void>;
  onKeep: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const isDelete = pending.kind === "delete";

  const run = async (action: () => Promise<void> | void) => {
    setBusy(true);
    try {
      await action();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-[60] flex items-end justify-center bg-black/40 sm:items-center sm:px-6"
      // A tap outside the panel is "keep editing", not an answer: nothing here
      // should be dismissible by accident.
      onClick={onKeep}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={isDelete ? "Delete this card?" : "Unsaved changes"}
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-md rounded-t-2xl border border-border-default bg-bg-card p-4 shadow-xl sm:rounded-2xl"
      >
        <h2 className="text-base font-semibold text-text-primary">
          {isDelete ? "Delete this card?" : "Unsaved changes"}
        </h2>
        <p className="mt-1 text-sm text-text-secondary">
          {isDelete
            ? "It leaves the deck and its review history, and there is no Restore in the app."
            : "This card has edits that are not on disk."}
        </p>
        <div className="mt-4 flex flex-col gap-2">
          {isDelete ? (
            <>
              <button
                ref={primaryRef as React.RefObject<HTMLButtonElement>}
                type="button"
                disabled={busy}
                onClick={() => void run(onConfirm)}
                className="min-h-11 rounded-lg bg-error-text px-3 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50"
              >
                Delete card
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={onKeep}
                className="min-h-11 rounded-lg border border-border-default px-3 py-2 text-sm hover:bg-bg-hover"
              >
                Keep it
              </button>
            </>
          ) : (
            <>
              <button
                ref={primaryRef as React.RefObject<HTMLButtonElement>}
                type="button"
                disabled={busy}
                onClick={() => void run(onConfirm)}
                className="min-h-11 rounded-lg bg-accent px-3 py-2 text-sm font-medium text-white hover:opacity-90 disabled:opacity-50"
              >
                {pending.what === "close" ? "Save & close" : "Save & continue"}
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void run(onDiscard)}
                className="min-h-11 rounded-lg border border-border-default px-3 py-2 text-sm hover:bg-bg-hover"
              >
                Discard changes
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={onKeep}
                className="min-h-11 rounded-lg px-3 py-2 text-sm text-text-secondary hover:bg-bg-hover"
              >
                Keep editing
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export type CardEditModalProps = {
  card: Card | null;
  datasetUuid: string;
  datasets: CardDatasetSummary[];
  /** Writes the card and returns what the backend stored, or null on failure. */
  onSave: (datasetUuid: string, card: Partial<Card>) => Promise<Card | null>;
  onDelete?: () => void;
  onClose: () => void;
  initialViewMode?: boolean;
  /** The loaded list this card came from, so prev/next can be offered. */
  queue: CardQueue | null;
  /** Ask the provider for the neighbouring card. Routed through the guard here. */
  onStep: (dir: -1 | 1) => void;
  /**
   * Hosted by a page instead of floating over it. Inline drops the scrim, the
   * safe-area insets and the `PageHeader` — the host's breadcrumb already names
   * the card, and the host supplies the surface — and keeps everything else:
   * the writing row, the fields, the attachments and the exit guard.
   */
  inline?: boolean;
};

export function CardEditModal({
  card,
  datasetUuid,
  datasets,
  onSave,
  onDelete,
  onClose,
  initialViewMode,
  queue,
  onStep,
  inline = false,
}: CardEditModalProps) {
  const initial = useMemo(() => valuesOf(card), [card]);
  const [selectedDataset, setSelectedDataset] = useState(datasetUuid);
  const [question, setQuestion] = useState(initial.question);
  const [answer, setAnswer] = useState(initial.answer);
  const [note, setNote] = useState(initial.note);
  const [suggestion, setSuggestion] = useState(initial.suggestion);
  // A saved card opens as a page you read; a new one opens as a form you fill.
  // `editing` is per field, so these are the same mechanism, not two modes.
  const [editing, setEditing] = useState<Record<CardMediaField, boolean>>(() => {
    const all = initialViewMode === undefined ? !card?.uuid : !initialViewMode;
    return { question: all, answer: all, note: all, suggestion: all };
  });
  const [fontScale, setFontScale] = useState<FontScaleKey>("normal");
  // Deferred platform detection (SSR-safe): isMobileApp() must not run during render.
  const [mobile, setMobile] = useState(false);
  // False until this card exists on disk. Drives Delete, the rating offer, and
  // the "a new card with content is unsaved" half of the dirty rule.
  const [saved, setSaved] = useState(!!card?.uuid);
  const [familiarity, setFamiliarity] = useState(card?.familiarity ?? 0);

  // Card uuid, minted up front for a blank card so `media/<card-uuid>/` can be
  // named before the first save (SQLite FKs are not enforced here, so an
  // attachment can land ahead of its card row). `card_save` honors a non-empty
  // uuid, so this is the id the saved card keeps.
  const [cardUuid] = useState(() =>
    card?.uuid && card.uuid.length ? card.uuid : mintCardUuid()
  );
  // Which tab chip this overlay decorates. Read once, at mount: the editor is
  // drawn over whatever tab was focused when it opened, and a tab change under it
  // (the shell is still reachable behind the scrim) must not move the subject to a
  // tab the card is not on.
  const [subjectTab] = useState(() => getActiveTabId() ?? "cards");
  // Keyed on the dataset this session opened on, not the one the picker has
  // moved to: the id must survive a save into another dataset and a picker
  // change, or a draft would be orphaned by the very edit it protects. Frozen
  // with a lazy initialiser so a save that re-points `datasetUuid` cannot
  // re-key the draft mid-session — which would re-run the mount effect below
  // and reset the reader's chosen font size.
  const [openDatasetUuid] = useState(datasetUuid);
  const draftKey = draftKeyFor(openDatasetUuid, cardUuid);

  const [media, setMedia] = useState<CardMedia[]>([]);
  // Which view is showing. The media view replaces the narrow sidebar this file
  // used to open on demand: the same rows and the same actions, but with the
  // width a list of handles, kinds and their controls was always asking for.
  const [pane, setPane] = useState<CardEditorPane>("edit");
  // Which field an attach or a Place targets — follows the last focused field,
  // and is now *shown* rather than only implied. The media view can also set it
  // directly, since no field is focused while it is open.
  const [attachField, setAttachField] = useState<CardMediaField>("question");
  const [dragOver, setDragOver] = useState(false);
  const [mediaBanner, setMediaBanner] = useState("");
  const [mediaBusy, setMediaBusy] = useState(false);
  const [pending, setPending] = useState<PendingExit | null>(null);
  const [savedFlash, setSavedFlash] = useState(false);
  const [draftRestored, setDraftRestored] = useState(false);
  const [saving, setSaving] = useState(false);

  // The last value read from or written to disk. Every save and every discard
  // re-seeds it; `dirty` is measured against it and nothing else.
  const baselineRef = useRef<CardFieldValues>(initial);
  const setField: FieldSetter = useCallback((field, value) => {
    if (field === "question") setQuestion(value);
    else if (field === "answer") setAnswer(value);
    else if (field === "note") setNote(value);
    else setSuggestion(value);
  }, []);

  const values: CardFieldValues = { question, answer, note, suggestion };
  const dirtyField: Record<CardMediaField, boolean> = {
    question: question !== baselineRef.current.question,
    answer: answer !== baselineRef.current.answer,
    note: note !== baselineRef.current.note,
    suggestion: suggestion !== baselineRef.current.suggestion,
  };
  const dirty = useMemo(
    () =>
      CARD_FIELD_ORDER.some((f) => values[f] !== baselineRef.current[f]) ||
      (!saved && CARD_FIELD_ORDER.some((f) => values[f].trim().length > 0)),
    // `values` is rebuilt every render, so depend on the four fields directly.
    [question, answer, note, suggestion, saved]
  );

  const questionRef = useRef<HTMLTextAreaElement | HTMLInputElement | null>(null);
  const answerRef = useRef<HTMLTextAreaElement | HTMLInputElement | null>(null);
  const noteRef = useRef<HTMLTextAreaElement | HTMLInputElement | null>(null);
  const suggestionRef = useRef<HTMLTextAreaElement | HTMLInputElement | null>(null);
  const fieldRefs = useRef<Record<CardMediaField, React.RefObject<HTMLTextAreaElement | HTMLInputElement | null>>>({
    question: questionRef,
    answer: answerRef,
    note: noteRef,
    suggestion: suggestionRef,
  });
  const primaryRef = useRef<HTMLButtonElement | null>(null);

  // Volatile mirrors for the window-level key handler, which must not
  // re-subscribe on every keystroke.
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const attachFieldRef = useRef(attachField);
  attachFieldRef.current = attachField;

  // Absolute dataset dir of the currently selected dataset — the base `resolveSrc`
  // joins onto for asset-protocol URLs.
  const datasetPath =
    datasets.find((ds) => ds.info.uuid === selectedDataset)?.path || "";
  const selectedDatasetName =
    datasets.find((ds) => ds.info.uuid === selectedDataset)?.info.name || "";

  // TOC drawer, mirroring the wiki page: the toolbar's TOC button toggles a
  // left drawer hosting the answer's heading tree (drag-resizable split column
  // on desktop, slide-in drawer on mobile). Opens by default on a PC — the
  // hook itself forces it closed on mobile; width persists.
  const tocSidebar = useCollapsibleSidebar({
    storageKey: "card-toc-width",
    defaultWidth: 240,
    minWidth: 180,
    initialVisible: true,
  });

  const anyEditing = editing.question || editing.answer || editing.note || editing.suggestion;
  const setFieldEditing = useCallback((field: CardMediaField, on: boolean) => {
    setEditing((prev) => (prev[field] === on ? prev : { ...prev, [field]: on }));
  }, []);
  const setAllEditing = useCallback((on: boolean) => {
    setEditing({ question: on, answer: on, note: on, suggestion: on });
  }, []);

  // One mount effect for the two decisions that need the platform and the draft:
  // the phone has no "big by default", and a restored draft changes what the
  // answer's length — and so the default scale — really is.
  useEffect(() => {
    const m = isMobileApp();
    setMobile(m);

    const stored = readDraft(draftKey);
    const restored =
      !!stored && CARD_FIELD_ORDER.some((f) => stored[f] !== baselineRef.current[f]);
    if (stored && restored) {
      setQuestion(stored.question);
      setAnswer(stored.answer);
      setNote(stored.note);
      setSuggestion(stored.suggestion);
      setDraftRestored(true);
      // Everything is open, because the reader needs to see what the draft added.
      setEditing({ question: true, answer: true, note: true, suggestion: true });
    }
    if (!m) {
      const effective = restored ? stored.answer : baselineRef.current.answer;
      setFontScale(effective.length > LONG_ANSWER_CHARS ? "large" : "x2");
    }
  }, [draftKey]);

  // The draft safety net — the seam the wiki page proved works: while the text
  // runs ahead of disk, persist it debounced; the moment it matches again, remove
  // the key so a later open never restores a stale draft. Removal also happens on
  // save and on discard, and a killed process is the case this exists for.
  useEffect(() => {
    if (!dirty) {
      clearDraft(draftKey);
      return;
    }
    const snapshot: CardDraft = { question, answer, note, suggestion };
    const timer = setTimeout(() => writeDraft(draftKey, snapshot), 400);
    return () => clearTimeout(timer);
  }, [dirty, draftKey, question, answer, note, suggestion]);

  // The "Saved" chip is a flash, not a state: it confirms the write and then
  // leaves the header back to telling the truth about what is on screen.
  useEffect(() => {
    if (!savedFlash) return;
    const timer = setTimeout(() => setSavedFlash(false), 2_000);
    return () => clearTimeout(timer);
  }, [savedFlash]);

  // ---- the guard ----------------------------------------------------------

  // An exit is only free when there is nothing to lose. Otherwise the sheet
  // intercepts, and the pending action runs from its own button — so Escape,
  // Close, Back and a step all answer to one rule instead of four.
  const requestExit = useCallback((run: () => void, what: "close" | "step" = "close") => {
    if (!dirtyRef.current) {
      run();
      return;
    }
    setPending({ kind: "exit", what, run });
  }, []);

  // The overlay is a root, so the shell cannot see it: the tab strip and F5 ask
  // `canRefreshInstance`, which ANDs this in. A dirty editor therefore auto-pins
  // the tab it is drawn on instead of being orphaned by a replace, and refuses a
  // reload that would remount every page beneath it.
  useEffect(() => {
    setOverlayGuard(() => !dirtyRef.current);
    return () => setOverlayGuard(null);
  }, []);

  useEffect(() => {
    if (!pending) return;
    // The default is the safe, productive one: save what you typed and move on.
    primaryRef.current?.focus();
  }, [pending]);

  /** Keep the sheet's "keep editing" honest: back into the field you were in. */
  const resumeEditing = useCallback(() => {
    const field = attachFieldRef.current;
    setPending(null);
    setFieldEditing(field, true);
    requestAnimationFrame(() => fieldRefs.current[field].current?.focus());
  }, [setFieldEditing]);

  /** Backing out of the sheet. A delete question is answered by closing it
   *  where it stood — nothing about it wants a field opened. */
  const dismissSheet = useCallback(() => {
    if (pendingRef.current?.kind === "delete") {
      setPending(null);
      return;
    }
    resumeEditing();
  }, [resumeEditing]);

  const applyValues = useCallback(
    (next: CardFieldValues, asBaseline: boolean) => {
      setQuestion(next.question);
      setAnswer(next.answer);
      setNote(next.note);
      setSuggestion(next.suggestion);
      if (asBaseline) baselineRef.current = next;
    },
    []
  );

  /**
   * Throw away the unsaved text — from disk, not from the props this overlay
   * opened with. An attachment added mid-edit is already a durable row, so
   * reusing the original card object would present it as absent and invite the
   * user to delete it a second time.
   */
  const discardChanges = useCallback(async (): Promise<boolean> => {
    clearDraft(draftKey);
    setDraftRestored(false);
    if (!saved || !isTauri()) {
      applyValues(baselineRef.current, false);
      return true;
    }
    try {
      const disk = await invoke<Card>("card_get", {
        datasetUuid: selectedDataset,
        cardUuid,
      });
      applyValues(valuesOf(disk), true);
      setFamiliarity(disk.familiarity);
      await reloadMediaRef.current();
      return true;
    } catch (e) {
      notifyError(`Could not re-read the card from disk: ${e}`);
      return false;
    }
  }, [saved, selectedDataset, cardUuid, draftKey, applyValues]);

  // ---- save ---------------------------------------------------------------

  const doSave = useCallback(async (): Promise<Card | null> => {
    if (!selectedDataset) {
      notifyError("Pick the dataset this card belongs to first.");
      return null;
    }
    if (!question.trim()) {
      // The toolbar's Save is disabled in this case; the guard's sheet reaches
      // saving from a different button, so the rule is enforced here as well
      // rather than writing a card no list can name.
      notifyError("A card needs a question before it can be saved.");
      return null;
    }
    setSaving(true);
    try {
      const savedCard = await onSave(selectedDataset, {
        uuid: cardUuid,
        question,
        answer,
        note,
        suggestion,
        // The value the card came with. `card_save` uses it only to seed a new
        // row; an existing row's standing is per-user review state and the
        // backend deliberately does not overwrite it.
        familiarity,
      });
      if (!savedCard) return null;
      baselineRef.current = valuesOf(savedCard);
      setFamiliarity(savedCard.familiarity);
      setSaved(true);
      clearDraft(draftKey);
      setDraftRestored(false);
      setSavedFlash(true);
      return savedCard;
    } finally {
      setSaving(false);
    }
  }, [selectedDataset, onSave, cardUuid, question, answer, note, suggestion, familiarity, draftKey]);

  // ---- keyboard -----------------------------------------------------------

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (pendingRef.current) {
        // The sheet owns the keyboard while it is up. Escape backs out of it;
        // nothing else may reach the card, or Save/step would fire under it.
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          dismissSheet();
        }
        return;
      }
      if (e.key === "Escape") {
        // Field-first. `EditableField` swallows Escape on the field it is
        // editing; this is the second half of the rule, for the case where the
        // keystroke starts outside a field.
        const target = e.target as HTMLElement | null;
        const inField =
          target &&
          (target.tagName === "TEXTAREA" ||
            target.tagName === "INPUT" ||
            target.isContentEditable);
        if (inField) return;
        e.preventDefault();
        e.stopPropagation();
        requestExit(onClose);
        return;
      }
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void doSave();
        return;
      }
      if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
        e.preventDefault();
        requestExit(() => onStep(e.key === "ArrowLeft" ? -1 : 1), "step");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [doSave, onStep, onClose, requestExit, dismissSheet]);

  // ---- card media ----

  const reloadMedia = useCallback(async () => {
    if (!isTauri() || !cardUuid || !selectedDataset) return;
    try {
      const rows = await invoke<CardMedia[]>("card_media_list", { datasetUuid: selectedDataset, cardUuid });
      setMedia(rows);
    } catch (e) {
      console.error("card_media_list failed:", e);
    }
  }, [selectedDataset, cardUuid]);
  const reloadMediaRef = useRef(reloadMedia);
  reloadMediaRef.current = reloadMedia;

  // Insert `token` at the caret of `field`. Reads the live DOM value so a modal
  // file dialog opening cannot clobber text typed since this closure was built.
  const insertToken = useCallback((field: CardMediaField, token: string) => {
    const el = fieldRefs.current[field].current;
    if (!el) {
      // The field is not open (or has not mounted yet): placement is
      // block-level, so an appended token is a whole block at the end.
      if (field === "question") setQuestion((v) => v + token);
      else if (field === "answer") setAnswer((v) => v + token);
      else if (field === "note") setNote((v) => v + token);
      else setSuggestion((v) => v + token);
      return;
    }
    const value = el.value;
    const start = el.selectionStart ?? value.length;
    const end = el.selectionEnd ?? start;
    setField(field, value.slice(0, start) + token + value.slice(end));
    const pos = start + token.length;
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(pos, pos);
    });
  }, [setField]);

  const addMediaFromPath = useCallback(
    async (sourcePath: string, field: CardMediaField) => {
      if (!isTauri() || !selectedDataset || !cardUuid) return;
      if (!cardKindOf(sourcePath)) {
        setMediaBanner("Unsupported file — a card accepts only images, audio, or video in the portable set.");
        return;
      }
      setMediaBusy(true);
      try {
        const row = await invoke<CardMedia>("card_media_add", {
          datasetUuid: selectedDataset,
          cardUuid,
          sourcePath,
          field,
        });
        setMedia((prev) => [...prev, row]);
        // Opening the field it lands in is what makes the token visible: an
        // attachment you cannot see in the text reads as a lost file.
        setFieldEditing(field, true);
        insertToken(field, `{{${row.handle}}}`);
        setMediaBanner("");
      } catch (e) {
        setMediaBanner(`Attach failed: ${e}`);
      } finally {
        setMediaBusy(false);
      }
    },
    [selectedDataset, cardUuid, insertToken, setFieldEditing]
  );

  const attach = useCallback(
    async (kind: "image" | "audio" | "video", field?: CardMediaField) => {
      if (!isTauri()) return;
      try {
        const selected = await open({
          multiple: false,
          filters: [{ name: kind, extensions: [...cardExtsFor(kind)] }],
        });
        const path = Array.isArray(selected) ? selected[0] : selected;
        if (path) await addMediaFromPath(path, field ?? attachFieldRef.current);
      } catch (e) {
        setMediaBanner(`Attach failed: ${e}`);
      }
    },
    [addMediaFromPath]
  );

  const deleteMedia = useCallback(
    async (uuid: string) => {
      const target = media.find((m) => m.uuid === uuid);
      try {
        await invoke("card_media_delete", { datasetUuid: selectedDataset, mediaUuid: uuid });
        setMedia((prev) => prev.filter((m) => m.uuid !== uuid));
        if (target) {
          const token = `{{${target.handle}}}`;
          const strip = (v: string) => v.split(token).join("");
          const next: CardFieldValues = {
            question: strip(question),
            answer: strip(answer),
            note: strip(note),
            suggestion: strip(suggestion),
          };
          if (next[target.field] !== values[target.field]) setFieldEditing(target.field, true);
          applyValues(next, false);
        }
      } catch (e) {
        setMediaBanner(`Delete failed: ${e}`);
      }
    },
    [media, selectedDataset, question, answer, note, suggestion, values, applyValues, setFieldEditing]
  );

  // One picker for every kind the card accepts. The toolbar's Attach asks which
  // kind first because a filter narrows the dialog; a management view has no such
  // question — the file's own extension already answers it, and the backend
  // refuses anything outside the portable set either way.
  const uploadMedia = useCallback(
    async (field: CardMediaField) => {
      if (!isTauri()) return;
      try {
        const selected = await open({
          multiple: false,
          filters: [
            {
              name: "Media",
              extensions: [...CARD_IMAGE_EXTS, ...CARD_AUDIO_EXTS, ...CARD_VIDEO_EXTS],
            },
          ],
        });
        const path = Array.isArray(selected) ? selected[0] : selected;
        if (path) {
          // Keep the marker honest: uploading from a preview sets the target to
          // the field that preview sits in, so the next Place agrees with it.
          setAttachField(field);
          await addMediaFromPath(path, field);
        }
      } catch (e) {
        setMediaBanner(`Attach failed: ${e}`);
      }
    },
    [addMediaFromPath]
  );

  // The handle is the one thing a writer types by hand, so a copy button is the
  // paste half of attaching — and it has to say so when the clipboard refuses,
  // because that text is exactly what would have been pasted.
  const copyHandle = useCallback(async (handle: string) => {
    const text = `{{${handle}}}`;
    if (await copyText(text)) notify(`Copied ${text}`);
    else notifyError(`Could not reach the clipboard — type ${text} into a field instead.`);
  }, []);

  // Reorder within one field's unanchored tail strip by re-writing `sort` to a
  // spaced sequence after moving the row by `dir`. Order is a table column, not
  // text, so this stays durable without opening the field.
  const moveMedia = useCallback(
    async (uuid: string, dir: -1 | 1) => {
      const cur = media.find((m) => m.uuid === uuid);
      if (!cur) return;
      const fieldList = media.filter((m) => m.field === cur.field);
      const i = fieldList.findIndex((m) => m.uuid === uuid);
      const j = i + dir;
      if (j < 0 || j >= fieldList.length) return;
      const reordered = [...fieldList];
      const [moved] = reordered.splice(i, 1);
      reordered.splice(j, 0, moved);
      try {
        const updated = await Promise.all(
          reordered.map((m, k) =>
            invoke<CardMedia>("card_media_update", {
              datasetUuid: selectedDataset,
              media: { ...m, sort: k * 10 },
            })
          )
        );
        const byUuid = new Map(updated.map((r) => [r.uuid, r]));
        setMedia((prev) => prev.map((m) => byUuid.get(m.uuid) ?? m));
      } catch (e) {
        setMediaBanner(`Reorder failed: ${e}`);
      }
    },
    [media, selectedDataset]
  );

  // On mount (and when the card or dataset changes), reconcile the directory
  // with the table and load the rows. `missing` (a row whose bytes are on
  // another device) surfaces as a banner; nothing is deleted by this call.
  useEffect(() => {
    if (!isTauri() || !cardUuid || !selectedDataset) return;
    let cancelled = false;
    (async () => {
      try {
        const rec = await invoke<{ imported: CardMedia[]; missing: string[]; orphaned: string[] }>(
          "card_media_register",
          { datasetUuid: selectedDataset, cardUuid }
        );
        if (cancelled) return;
        if (rec.missing?.length) {
          setMediaBanner(
            `${rec.missing.length} attachment${rec.missing.length > 1 ? "s are" : " is"} not on this device yet (still syncing).`
          );
        }
        await reloadMedia();
      } catch (e) {
        console.error("card_media_register failed:", e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cardUuid, selectedDataset, reloadMedia]);

  // Native drag-drop of a file onto the window attaches it to the target field.
  // Enabling the webview's native drop means HTML5 drop won't fire, so there is
  // deliberately no separate dropzone (see the design note) — instead the field
  // the drop would land in is ringed for as long as the file is over the window,
  // so a drop is never a guess.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "drop") {
          const path = payload.paths[0];
          if (path) void addMediaFromPath(path, attachFieldRef.current);
          setDragOver(false);
        } else if (payload.type === "enter" || payload.type === "over") {
          setDragOver(true);
        } else if (payload.type === "leave") {
          setDragOver(false);
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* drag-drop unavailable on this platform */
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [addMediaFromPath]);

  // ---- identity: the tab chip, the address bar, the agent -----------------

  const subjectLabel = cardSubjectLabel(stripAnchors(question));
  useEffect(() => {
    const subject: TabSubject = {
      kind: "card",
      datasetUuid: selectedDataset,
      datasetName: selectedDatasetName || undefined,
      cardUuid,
      label: subjectLabel,
    };
    setTabSubject(subjectTab, subject);
    // Withdraw the card by value, so a cleanup that ran late (the modal unmounting
    // under a newer subject) cannot clear something it never published.
    return () => clearTabSubject(subjectTab, subject);
  }, [subjectTab, selectedDataset, selectedDatasetName, cardUuid, subjectLabel]);

  // The agent answers "which card is on screen" from the merged UI context. A
  // card that has never been saved has no id for anyone else to act on, so only
  // the dataset is reported until the first save.
  useEffect(() => {
    setUiContext({
      active_dataset_uuid: selectedDataset,
      active_card_uuid: saved ? cardUuid : null,
    });
    return () => setUiContext({ active_dataset_uuid: null, active_card_uuid: null });
  }, [selectedDataset, saved, cardUuid]);

  // ---- standing -----------------------------------------------------------

  // A rating is a review event, not a field write: it goes to
  // `card_test_submit`, which owns the SM-2 state for this user. Repaint from
  // what it returns rather than guessing the schedule advanced by one.
  const rate = useCallback(
    async (quality: number) => {
      if (!saved || !isTauri()) return;
      try {
        const result = await invoke<CardReview>("card_test_submit", {
          datasetUuid: selectedDataset,
          cardUuid,
          quality,
        });
        setFamiliarity(result.familiarity);
      } catch (e) {
        notifyError(`Rating failed: ${e}`);
      }
    },
    [saved, selectedDataset, cardUuid]
  );

  // ---- derived render inputs ---------------------------------------------

  // Build the TOC node from the same content the body renders, so the
  // `#sec-...` anchors stay in sync (wiki pattern). Null when the answer has
  // no headings — that gates both the toolbar button and the drawer column.
  // Offered while editing too: heading navigation of an answer you are writing
  // is exactly when it earns its keep.
  const tocNode = useMemo(() => (answer.trim() ? toHTML(answer).toc : null), [answer]);

  // A handle with no token in any field: durable, but invisible on the card.
  const unplacedHandles = useMemo(() => {
    const out = new Set<string>();
    for (const m of media) {
      const token = `{{${m.handle}}}`;
      if (!CARD_FIELD_ORDER.some((f) => values[f].includes(token))) out.add(m.handle);
    }
    return out;
  }, [media, question, answer, note, suggestion]);

  const missingHandles = useMemo(
    () => new Set(media.filter((m) => m.present === false).map((m) => m.handle)),
    [media]
  );

  const heading = stripAnchors(question).trim().slice(0, 60) || (saved ? "Card" : "New card");
  const canStep = !!queue;

  // The strip's items. The count answers "how many" before the view is open (a
  // zero is information), and the dot carries the one state the words cannot: a
  // row saved with the card whose `{{handle}}` no field holds.
  const paneItems: TabBarItem<CardEditorPane>[] = [
    { id: "edit", label: "Edit", icon: Pencil },
    {
      id: "media",
      label: "Media",
      icon: Film,
      count: media.length,
      dot: unplacedHandles.size > 0,
    },
  ];

  // The four fields as one leaf: it renders them, this host owns everything
  // they read and write. The overlay's toolbar, draft, save and exit guard stay
  // here — a page that shows a card inside its own breadcrumb level passes its
  // own binding and gets the same body without any of this chrome.
  const fieldBinding: CardFieldBinding = {
    values,
    editing,
    dirty: dirtyField,
    media,
    datasetPath,
    attachField,
    dragOver,
    textStyle: {
      zoom: FONT_SCALE_MAP[fontScale].zoom,
      fontWeight: FONT_SCALE_MAP[fontScale].weight,
    },
    mediaBusy,
    onFieldChange: (field, value) => {
      setField(field, value);
      setAttachField(field);
    },
    onStartEdit: (field) => {
      setAttachField(field);
      setFieldEditing(field, true);
      requestAnimationFrame(() => fieldRefs.current[field].current?.focus());
    },
    onEndEdit: (field) => {
      // Committing out of a field is not a save: the text stays in the draft
      // until the reader saves or the exit guard asks about it.
      setFieldEditing(field, false);
    },
    onAttach: (kind, field) => void attach(kind, field),
    onMediaUpload: (field) => void uploadMedia(field),
    onMediaRemove: (uuid) => void deleteMedia(uuid),
    onMediaCopy: (handle) => void copyHandle(handle),
    onRegisterElement: (field, el) => {
      fieldRefs.current[field].current = el;
    },
  };

  return (
    // Full-screen overlay with the unified page top-area pattern: a PageHeader
    // bar, a Toolbar action row directly beneath it, and a scrolling body. A
    // page that hosts the editor instead (`inline`) supplies the surface and
    // the name, so the row stands directly under its breadcrumb.
    <div
      className={
        inline
          ? "flex flex-1 min-h-0 flex-col bg-bg-body"
          : "fixed inset-0 z-50 flex flex-col bg-bg-body"
      }
      style={
        mobile && !inline
          ? {
              paddingTop: "env(safe-area-inset-top)",
              paddingBottom: "env(safe-area-inset-bottom)",
            }
          : undefined
      }
    >
      {/* A header row is a name plus status, never a control (see PageHeader):
          the card's question, whether it is on disk, and where it stands. The
          dataset picker and the rating live in the Toolbar below. Inline has no
          header at all: the page's breadcrumb is that row, and its last crumb
          already carries this question. */}
      {!inline && (
      <PageHeader
        title={heading}
        badge={
          <div className="flex items-center gap-1.5 shrink-0">
            {dirty ? (
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary">
                {saved ? "unsaved changes" : "not saved yet"}
              </span>
            ) : savedFlash ? (
              <span className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-full bg-success-bg text-text-primary">
                <Check size={10} /> Saved
              </span>
            ) : saved ? (
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-tertiary">
                saved
              </span>
            ) : (
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary">
                unsaved card
              </span>
            )}
            <span
              className={`text-[10px] px-1.5 py-0.5 rounded font-medium ${FAM_CLASSES[Math.min(familiarity, 6)]}`}
              title={`Familiarity ${familiarity} — ${famName(familiarity)}`}
            >
              {famName(familiarity)} · {familiarity}
            </span>
          </div>
        }
      />
      )}

      <Toolbar mobile={mobile}>
        {/* Inline has no header, so the row carries the one thing the header was
            there to say besides the name: whether this edit is on disk. */}
        {inline &&
          (dirty ? (
            <span className="shrink-0 rounded-full bg-bg-muted px-1.5 py-0.5 text-[10px] text-text-secondary">
              {saved ? "unsaved changes" : "not saved yet"}
            </span>
          ) : savedFlash ? (
            <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-success-bg px-1.5 py-0.5 text-[10px] text-text-primary">
              <Check size={10} /> Saved
            </span>
          ) : null)}
        {/* Dataset picker, in both modes: which deck a card belongs to is a
            property of the card, not of the mode you happen to be in. */}
        <select
          value={selectedDataset}
          onChange={(e) => setSelectedDataset(e.target.value)}
          aria-label="Dataset"
          title={selectedDatasetName ? `Dataset: ${selectedDatasetName}` : undefined}
          className="min-w-0 max-w-[12rem] shrink-0 cursor-pointer rounded-lg border border-border-default bg-bg-card px-2 py-1.5 text-sm text-text-primary hover:bg-mid-gray/20"
        >
          {!datasets.some((ds) => ds.info.uuid === selectedDataset) && (
            <option value={selectedDataset}>
              {selectedDatasetName || (saved ? "(unknown dataset)" : "(unsaved card)")}
            </option>
          )}
          {datasets.map((ds) => (
            <option key={ds.info.uuid} value={ds.info.uuid}>
              {ds.info.name} ({ds.card_count} cards)
            </option>
          ))}
        </select>

        {/* Prev/next only when there is a list to walk. Opened from a context
            menu or a review jump there is none, and a dead arrow lies. */}
        {canStep && (
          <>
            <ToolbarButton
              icon={ChevronLeft}
              title="Previous card (Alt+←)"
              ariaLabel="Previous card"
              collapseOnMobile={false}
              onClick={() => requestExit(() => onStep(-1), "step")}
            />
            <ToolbarButton
              icon={ChevronRight}
              title="Next card (Alt+→)"
              ariaLabel="Next card"
              collapseOnMobile={false}
              onClick={() => requestExit(() => onStep(1), "step")}
            />
          </>
        )}

        {/* Standing: the legend the ramp uses and the grades that change it. */}
        <ToolbarSelect
          icon={GraduationCap}
          label="Standing"
          value={`${famName(familiarity)} · ${familiarity}`}
          title="Familiarity scale — rate this card"
        >
          {FAMILIARY_RAMP.map((r) => (
            <div
              key={r.f}
              className="flex items-center gap-2 px-3 py-1 text-sm text-text-secondary"
            >
              <span className={`h-3 w-3 shrink-0 rounded-sm ${FAM_CLASSES[r.f]}`} />
              <span className="w-3 shrink-0 tabular-nums">{r.f}</span>
              <span>{r.name}</span>
              {r.f === Math.min(familiarity, 6) && (
                <Check size={14} className="ml-auto text-accent" />
              )}
            </div>
          ))}
          <div className="my-1 border-t border-border-default" />
          {saved ? (
            GRADES.map((g) => (
              <ToolbarMenuItem key={g.q} icon={g.Icon} onClick={() => void rate(g.q)}>
                {g.label} ({g.q})
              </ToolbarMenuItem>
            ))
          ) : (
            <div className="px-3 py-2 text-sm text-text-tertiary">
              Rate it after the first review
            </div>
          )}
        </ToolbarSelect>

        {/* The rest of what the hidden header used to say, kept together and next
            to the deck: how practised the card is (the Standing control), when it
            last changed, and whether what is on screen is on disk (the badge that
            opens the row). */}
        {inline && saved && card?.updated_at && (
          <span
            className="shrink-0 select-none text-[11px] tabular-nums text-text-tertiary"
            title="Last updated"
          >
            updated {card.updated_at.slice(0, 16).replace("T", " ")}
          </span>
        )}

        <div className="flex-1 min-w-0" />

        {/* Attach on the fields, Upload on the media view: one add-a-file
            control per view, each asking for what that view already knows (a
            kind here, a placement target there). */}
        {isTauri() && pane === "edit" && (
          <>
            <ToolbarSelect
              icon={Paperclip}
              label="Attach"
              value={mediaBusy ? "Attaching…" : "Attach"}
              title={`Attach to “${attachField}”`}
              disabled={mediaBusy}
            >
              <ToolbarMenuItem icon={ImageIcon} onClick={() => void attach("image")}>
                Image
              </ToolbarMenuItem>
              <ToolbarMenuItem icon={Music2} onClick={() => void attach("audio")}>
                Audio
              </ToolbarMenuItem>
              <ToolbarMenuItem icon={Video} onClick={() => void attach("video")}>
                Video
              </ToolbarMenuItem>
            </ToolbarSelect>
          </>
        )}
        {/* Two toolbar controls belong to a view, not to the card, so they are
            offered only on the view that answers them: the TOC toggle opens a
            column of the fields, and the whole-card mode switch opens fields. A
            control that changes nothing on screen is worse than an absent one. */}
        {pane === "edit" && !!tocNode && (
          <ToolbarButton
            icon={List}
            variant="toggleSoft"
            active={tocSidebar.visible}
            label="TOC"
            onClick={tocSidebar.toggle}
            title={tocSidebar.visible ? "Hide table of contents" : "Show table of contents"}
          />
        )}
        <ToolbarSelect icon={Type} label="Font size" value={FONT_SCALE_SHORT[fontScale]}>
          {(Object.keys(FONT_SCALE_MAP) as FontScaleKey[]).map((k) => (
            <ToolbarMenuItem key={k} checked={fontScale === k} onClick={() => setFontScale(k)}>
              {FONT_SCALE_LABELS[k]}
            </ToolbarMenuItem>
          ))}
        </ToolbarSelect>
        {/* Mode switch as an action button: in view mode it offers Edit, in
            edit mode it offers View — the label names what the click does, not
            the mode you are in. It now opens and closes every field at once;
            clicking a single field has always opened that one. Not offered when
            a page hosts the editor: there, every field is open and the page's own
            card, one step back, is the reading mode. */}
        {!inline && pane === "edit" && (
          <ToolbarButton
            icon={anyEditing ? Eye : Pencil}
            label={anyEditing ? "View" : "Edit"}
            title={anyEditing ? "Close every field" : "Open every field"}
            onClick={() => setAllEditing(!anyEditing)}
          />
        )}
        <ToolbarButton
          variant={dirty ? "solid" : "ghost"}
          icon={Check}
          label={saving ? "Saving…" : "Save"}
          disabled={!dirty || saving || !question.trim()}
          title={
            !question.trim()
              ? "A card needs a question before it can be saved"
              : "Save this card (Ctrl+S) — it stays open"
          }
          onClick={() => void doSave()}
        />
        {onDelete && (
          <>
            {/* A rule before the only destructive control on the surface: it is
                already next to Save, and it should not also be next to Close. */}
            <span aria-hidden className="mx-0.5 h-5 w-px shrink-0 bg-border-default" />
            <ToolbarButton
              variant="danger"
              icon={Trash2}
              label="Delete"
              title="Delete this card"
              onClick={() => setPending({ kind: "delete" })}
            />
          </>
        )}
        <ToolbarButton
          icon={X}
          label={inline ? "Done" : "Close"}
          title={
            inline
              ? "Back to the card — unsaved changes are confirmed first"
              : "Close the editor"
          }
          onClick={() => requestExit(onClose)}
        />
      </Toolbar>

      <TabBar
        label="Card editor sections"
        items={paneItems}
        value={pane}
        onChange={setPane}
        className="shrink-0 px-4 bg-bg-card"
      />

      {/* Notices sit between the bars and the body, not inside a view: a media
          failure is worth the same line whichever tab is open, and a banner that
          scrolled away with the fields would not be a banner. */}
      {(mediaBanner || draftRestored) && (
        <div className="shrink-0 space-y-2 border-b border-border-default px-4 py-2 sm:px-8">
          {draftRestored && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border-default bg-bg-surface px-3 py-1.5 text-sm text-warning-text">
              Restored unsaved draft
              <ToolbarButton
                variant="ghost"
                label="Revert to saved"
                onClick={() => void discardChanges()}
                title="Discard the draft and edit the saved card instead"
              />
            </div>
          )}
          {mediaBanner && (
            <div className="rounded-lg border border-border-default bg-bg-surface px-3 py-1.5 text-sm text-text-secondary">
              {mediaBanner}
            </div>
          )}
        </div>
      )}

      {/* Body row: the strip above decides which view; the TOC drawer occupies the
          left of the edit view (split column on desktop, slide-in on mobile) and
          the card fills the rest — anchored under the bars so nothing covers the
          chrome that opens it. */}
      <div className="relative flex flex-1 min-h-0 overflow-hidden">
        {pane === "edit" ? (
          <>
            {!!tocNode && (
              <CollapsibleSidebar sidebar={tocSidebar}>
                <div
                  className="wiki-toc flex-1 min-h-0 overflow-y-auto px-3 py-2"
                  onClick={(e) => {
                    // Mobile: picking a section closes the drawer so the anchor
                    // jump is visible; the desktop column stays put.
                    if ((e.target as HTMLElement).closest("a")) tocSidebar.closeOnSelect();
                  }}
                >
                  {tocNode}
                </div>
              </CollapsibleSidebar>
            )}
            <div className="flex-1 min-h-0 overflow-y-auto">
              {/* The font step travels inside the fields (see `CardFields`), not
                  on this wrapper: zooming here scaled the labels, the chips and
                  the attach controls along with the type, so the chrome grew into
                  a second headline at the largest step. The width is the pane's,
                  and all four panels share it, so none ends somewhere different
                  from the others. */}
              <div className="px-4 py-6 sm:px-8">
                <CardFields binding={fieldBinding} />
              </div>
            </div>
          </>
        ) : (
          <div className="flex-1 min-h-0 overflow-y-auto">
            <div className="px-4 py-6 sm:px-8">
              {/* The management view. No placement control here: a handle is text,
                  so the way to put one in a field is to copy it and paste it — which
                  is what the row's Copy button hands you. What the view does still
                  decide is where a new Upload lands, and that is the last field the
                  writer touched, so the line above says which one it is. */}
              {unplacedHandles.size > 0 ? (
                <p className="mb-4 text-xs text-warning-text">
                  {unplacedHandles.size} attached, not placed — copy the handle and paste it into a field
                </p>
              ) : (
                <p className="mb-4 text-xs text-text-tertiary">
                  Uploads land in the {CARD_FIELD_LABEL[attachField].toLowerCase()} field — copy
                  a handle to place one anywhere else.
                </p>
              )}
              <MediaLibrary
                media={media}
                datasetPath={datasetPath}
                missingHandles={missingHandles}
                unplacedHandles={unplacedHandles}
                canUpload={isTauri()}
                uploadTarget={CARD_FIELD_LABEL[attachField].toLowerCase()}
                onUpload={() => void uploadMedia(attachField)}
                onCopy={(handle) => void copyHandle(handle)}
                onMove={(uuid, dir) => void moveMedia(uuid, dir)}
                onDelete={(uuid) => void deleteMedia(uuid)}
              />
            </div>
          </div>
        )}
      </div>

      {pending && (
        <ExitSheet
          pending={pending}
          primaryRef={primaryRef}
          onConfirm={async () => {
            if (pending.kind === "delete") {
              setPending(null);
              onDelete?.();
              return;
            }
            const run = pending.run;
            const savedCard = await doSave();
            if (!savedCard) return; // already toasted; leave the sheet up to be answered
            setPending(null);
            run();
          }}
          onDiscard={async () => {
            if (pending.kind === "delete") return;
            const run = pending.run;
            if (await discardChanges()) {
              setPending(null);
              run();
            }
          }}
          onKeep={dismissSheet}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The provider — one editor for the whole app.
//
// It lives above the shell rather than inside `CardsPage` so "Add to Card" from a
// text selection anywhere (the wiki, a PDF, a dictation cue) opens the card
// immediately instead of navigating there first. The cost of that choice is that
// the editor is not itself a tab instance, which is what the overlay guard, the
// subject publication and the address reflection exist to pay for.
//
// Session identity is `sessionKey`, and the modal is keyed on it: a step to
// another card remounts the surface, which is the cheapest correct way to reset
// forty-odd pieces of field state, the media rows, and the TOC at once. A save
// does *not* change it, so the editor you are typing in survives its own write.
// ---------------------------------------------------------------------------

export type CardEditorState = {
  isOpen: boolean;
  card: Card | null;
  datasetUuid: string;
  /** The deck the session opened on. `datasetUuid` follows a save into another
   *  deck; this does not, because it is what the draft key is pinned to. */
  draftDatasetUuid: string;
  datasetName?: string;
  datasets: CardDatasetSummary[];
  onSave?: (card: Card) => void;
  onDelete?: () => void;
  initialViewMode?: boolean;
  queue: CardQueue | null;
  /** The page hosting this session draws the editor itself, in its content
   *  area, instead of the provider floating it over the shell. */
  inline: boolean;
};

/** Everything a page needs to host an open editor inline, plus its session key. */
export type CardEditSession = CardEditModalProps & { sessionKey: string };

export type CardEditorContextType = {
  isOpen: boolean;
  openCardEditor: (
    datasetUuid: string,
    card?: Card | null,
    onSave?: (card: Card) => void,
    onDelete?: () => void,
    datasetName?: string,
    initialViewMode?: boolean,
    queue?: CardQueue,
    /** `inline` hands the editor to the page instead of floating it. */
    opts?: { inline?: boolean }
  ) => void;
  closeCardEditor: () => void;
  /** Hand the editor the list it is walking, after every re-read of that list. */
  refreshCardQueue: (queue: CardQueue) => void;
  /** An open inline session for this page to render, or null. The provider never
   *  draws an inline session itself — that is what "inline" means. */
  session: CardEditSession | null;
};

const CardEditorContext = createContext<CardEditorContextType | null>(null);

export function useCardEditor() {
  const context = useContext(CardEditorContext);
  if (!context) {
    throw new Error("useCardEditor must be used within CardEditorProvider");
  }
  return context;
}

/** A queued step that is waiting for the next page of the list to arrive. */
type PendingStep = { dir: -1 | 1; fromKey: string };

type InternalState = CardEditorState & {
  sessionKey: string;
  /** False when the session was opened without a list, so a later `CardsPage`
   *  re-render cannot bolt prev/next onto a card the user jumped to. */
  queueable: boolean;
  pendingStep: PendingStep | null;
};

const CLOSED: InternalState = {
  isOpen: false,
  sessionKey: "",
  card: null,
  datasetUuid: "",
  draftDatasetUuid: "",
  datasets: [],
  queue: null,
  queueable: false,
  pendingStep: null,
  inline: false,
};

const queueKey = (q: CardQueue | null) => (q ? q.uuids.join(",") : "");

function sameQueue(a: CardQueue | null, b: CardQueue | null): boolean {
  if (!a || !b) return a === b;
  return a.index === b.index && queueKey(a) === queueKey(b);
}

export function CardEditorProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [state, setState] = useState<InternalState>(CLOSED);
  // Read by the stable callbacks below, so none of them has to be rebuilt (and
  // none of their consumers — `CardsPage`'s queue effect, the modal's props —
  // re-runs because a render happened).
  const stateRef = useRef(state);
  stateRef.current = state;

  async function loadDatasets(): Promise<CardDatasetSummary[]> {
    // Card datasets only exist inside the Tauri runtime. This provider also
    // wraps the public marketing site (`/`), so skip the invoke there instead of
    // logging a failed call on every web visit.
    if (!isTauri()) return [];
    try {
      const result = await invoke<CardDatasetSummary[]>("card_dataset_list");
      setState((prev) => ({ ...prev, datasets: result }));
      return result;
    } catch (e) {
      console.error("Failed to load datasets:", e);
      return [];
    }
  }

  /** Open the card the address names. Two uuids are enough — `card_get` needs no
   *  list, so a restored window shows its card before the dataset list lands. */
  async function openFromUrl(target: { datasetUuid: string; cardUuid: string }, datasets: CardDatasetSummary[]) {
    if (stateRef.current.isOpen) return;
    try {
      const card = await invoke<Card>("card_get", {
        datasetUuid: target.datasetUuid,
        cardUuid: target.cardUuid,
      });
      const datasetName =
        datasets.find((ds) => ds.info.uuid === target.datasetUuid)?.info.name;
      setState((prev) => ({
        ...prev,
        isOpen: true,
        sessionKey: card.uuid,
        card,
        datasetUuid: target.datasetUuid,
        draftDatasetUuid: target.datasetUuid,
        datasetName,
        onSave: undefined,
        onDelete: undefined,
        initialViewMode: undefined,
        queue: null,
        queueable: false,
        pendingStep: null,
      }));
    } catch (e) {
      // A card that is gone is not worth a dialog: the address is simply stale,
      // and the shell opens on the tab it was left on.
      console.warn("Could not restore the card named by the address:", e);
    }
  }

  // Boot: datasets first, then the address — which is the only way to reopen a
  // card without a route, and why the query is read exactly once.
  useEffect(() => {
    void loadDatasets().then((list) => {
      const target = readCardFromUrl();
      if (target) void openFromUrl(target, list);
    });
  }, []);

  // Back/forward. Only `replaceState` is written today, so this is a listener for
  // a history entry the shell does not yet create — kept because a card in the
  // address is a promise that navigating to it shows that card.
  useEffect(() => {
    const onPop = () => {
      if (!isTauri()) return;
      const tab = getActiveTabId();
      // Unsaved work on screen outranks the navigation: the overlay guard answers
      // for the editor and the page's own guard for the tab beneath it.
      if (tab && !canRefreshInstance(tab)) return;
      const target = readCardFromUrl();
      if (target) void openFromUrl(target, stateRef.current.datasets);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const openCardEditor = useCallback(
    (
      datasetUuid: string,
      card?: Card | null,
      onSave?: (card: Card) => void,
      onDelete?: () => void,
      datasetName?: string,
      initialViewMode?: boolean,
      queue?: CardQueue,
      opts?: { inline?: boolean }
    ) => {
      // Reload datasets to ensure we have the latest card counts
      void loadDatasets().then((list) => {
        setState((prev) => {
          const datasets = list.length ? list : prev.datasets;
          // Check if the dataset is already in the list
          const datasetExists = datasets.some((ds) => ds.info.uuid === datasetUuid);

          // If dataset doesn't exist but we have a name, add a placeholder
          const merged = !datasetExists && datasetName && datasetUuid
            ? [
                { info: { uuid: datasetUuid, name: datasetName }, card_count: 0, path: "", location: "" } as CardDatasetSummary,
                ...datasets,
              ]
            : datasets;

          return {
            ...prev,
            isOpen: true,
            sessionKey: card?.uuid && card.uuid.length ? card.uuid : `new-${Date.now()}`,
            card: card || null,
            datasetUuid,
            draftDatasetUuid: datasetUuid,
            datasetName,
            datasets: merged,
            onSave,
            onDelete,
            initialViewMode,
            queue: queue ?? null,
            queueable: !!queue,
            inline: !!opts?.inline,
            pendingStep: null,
          };
        });
      });
    },
    []
  );

  const closeCardEditor = useCallback(() => {
    setState((prev) => ({
      ...CLOSED,
      datasets: prev.datasets,
    }));
  }, []);

  /** The list re-read itself, so a step after a page change has somewhere to go.
   *  The index is re-derived from the card on screen rather than trusted from the
   *  list, because a save can move a card within an ordered page. */
  const refreshCardQueue = useCallback((queue: CardQueue) => {
    setState((prev) => {
      if (!prev.isOpen || !prev.queueable) return prev;
      const uuid = prev.card?.uuid;
      const at = uuid ? queue.uuids.indexOf(uuid) : -1;
      const next: CardQueue = { ...queue, index: at < 0 ? queue.index : at };
      if (sameQueue(prev.queue, next)) return prev;
      return { ...prev, queue: next };
    });
  }, []);

  const loadInto = useCallback(async (uuid: string, datasetUuid: string, queue: CardQueue | null) => {
    try {
      const card = await invoke<Card>("card_get", { datasetUuid, cardUuid: uuid });
      setState((prev) => ({
        ...prev,
        isOpen: true,
        sessionKey: card.uuid,
        card,
        datasetUuid,
        draftDatasetUuid: datasetUuid,
        initialViewMode: undefined,
        queue,
        pendingStep: null,
      }));
    } catch (e) {
      notifyError(`Could not open that card: ${e}`);
      setState((prev) => ({ ...prev, pendingStep: null }));
    }
  }, []);

  const stepBy = useCallback((dir: -1 | 1) => {
    const { queue, datasetUuid } = stateRef.current;
    if (!queue) return;
    const target = queue.index + dir;
    if (target < 0 || target >= queue.uuids.length) {
      // Past the loaded page: ask the list for the next one, then step onto its
      // end card when it arrives. This is what keeps a browse from stopping at
      // the 50-card boundary the page happens to fetch.
      setState((prev) =>
        prev.queue ? { ...prev, pendingStep: { dir, fromKey: queueKey(prev.queue) } } : prev
      );
      queue.loadMore(dir);
      return;
    }
    const nextQueue = { ...queue, index: target };
    setState((prev) => ({ ...prev, queue: nextQueue }));
    void loadInto(queue.uuids[target], datasetUuid, nextQueue);
  }, [loadInto]);

  // A step that ran off the end of the page resolves when the list comes back.
  // An unchanged list means the walk hit a real end (no further page, or a queue
  // that cannot page) — stop instead of holding a pending step open forever.
  useEffect(() => {
    const { pendingStep, queue, datasetUuid } = state;
    if (!pendingStep || !queue) return;
    if (queueKey(queue) === pendingStep.fromKey) {
      setState((prev) => ({ ...prev, pendingStep: null }));
      return;
    }
    const uuid =
      pendingStep.dir > 0 ? queue.uuids[0] : queue.uuids[queue.uuids.length - 1];
    if (!uuid) {
      setState((prev) => ({ ...prev, pendingStep: null }));
      return;
    }
    const index = queue.uuids.indexOf(uuid);
    const nextQueue = { ...queue, index };
    setState((prev) => ({ ...prev, pendingStep: null, queue: nextQueue }));
    void loadInto(uuid, datasetUuid, nextQueue);
  }, [state.pendingStep, state.queue, state.datasetUuid, loadInto]);

  /**
   * Write the card and keep the session open.
   *
   * The editor does not close on a save: a card is edited in passes (answer, then
   * note, then an attachment, then the answer again), and sending the user back to
   * the list for each one made every change cost a navigation. The caller's `onSave`
   * still runs, so the list re-reads, and the saved row comes back to the modal so
   * it can re-seed its baseline instead of trusting what it sent.
   */
  const handleSave = useCallback(
    async (datasetUuid: string, cardData: Partial<Card>): Promise<Card | null> => {
      if (!datasetUuid) return null;

      const fullCard: Card = {
        uuid: cardData.uuid || "",
        question: cardData.question || "",
        suggestion: cardData.suggestion || "",
        answer: cardData.answer || "",
        note: cardData.note || "",
        familiarity: cardData.familiarity || 0,
        question_hash: null,
        source_card_uuid: cardData.source_card_uuid || null,
        source_dataset_uuid: cardData.source_dataset_uuid || null,
        deleted_at: null,
        created_at: "",
        updated_at: "",
      };

      try {
        const savedCard = await invoke<Card>("card_save", {
          datasetUuid,
          card: fullCard,
        });
        setState((prev) => ({ ...prev, card: savedCard, datasetUuid }));
        stateRef.current.onSave?.(savedCard);
        // The picker shows counts, so a save into another deck has to re-read them.
        void loadDatasets();
        return savedCard;
      } catch (e) {
        console.error("Failed to save card:", e);
        // The text is still on screen and still in the draft — say so, because the
        // user's next action depends on whether anything was lost.
        notifyError(`Failed to save card: ${e}. Your edits are still here.`);
        return null;
      }
    },
    []
  );

  const handleDelete = useCallback(async () => {
    const { card, datasetUuid } = stateRef.current;
    if (!card?.uuid || !datasetUuid) return;

    try {
      await invoke("card_delete", {
        datasetUuid,
        cardUuid: card.uuid,
      });
      // The card is gone, so its draft is garbage — and leaving it behind would
      // resurrect a tombstone the next time this uuid is opened. Both keys: a
      // save into another deck moved `datasetUuid`, but never the draft's.
      clearDraft(draftKeyFor(stateRef.current.draftDatasetUuid, card.uuid));
      clearDraft(draftKeyFor(datasetUuid, card.uuid));
      stateRef.current.onDelete?.();
      closeCardEditor();
    } catch (e) {
      console.error("Failed to delete card:", e);
      notifyError(`Failed to delete card: ${e}`);
    }
  }, [closeCardEditor]);

  // What a page hosting an inline session renders. Built from the same four
  // pieces the overlay is built from, so the two hosts cannot drift: the page
  // is not a second editor, it is the same one with its chrome supplied by the
  // breadcrumb and the toolbars around it.
  const session = useMemo<CardEditSession | null>(
    () =>
      state.isOpen && state.inline
        ? {
            sessionKey: state.sessionKey,
            card: state.card,
            datasetUuid: state.datasetUuid,
            datasets: state.datasets,
            onSave: handleSave,
            onDelete: state.card?.uuid ? handleDelete : undefined,
            onClose: closeCardEditor,
            initialViewMode: state.initialViewMode,
            queue: state.queueable ? state.queue : null,
            onStep: stepBy,
            inline: true,
          }
        : null,
    [state, handleSave, handleDelete, closeCardEditor, stepBy]
  );

  const value = useMemo(
    () => ({
      isOpen: state.isOpen,
      openCardEditor,
      closeCardEditor,
      refreshCardQueue,
      session,
    }),
    [state.isOpen, openCardEditor, closeCardEditor, refreshCardQueue, session]
  );

  return (
    <CardEditorContext.Provider value={value}>
      {children}
      {/* An inline session is the page's to draw; the provider must not also
          float a copy over it. */}
      {state.isOpen && !state.inline && (
        <CardEditModal
          // Keyed on the session: a step to another card starts a clean surface,
          // a save does not.
          key={state.sessionKey}
          card={state.card}
          datasetUuid={state.datasetUuid}
          datasets={state.datasets}
          onSave={handleSave}
          onDelete={state.card?.uuid ? handleDelete : undefined}
          onClose={closeCardEditor}
          initialViewMode={state.initialViewMode}
          queue={state.queueable ? state.queue : null}
          onStep={stepBy}
        />
      )}
    </CardEditorContext.Provider>
  );
}
