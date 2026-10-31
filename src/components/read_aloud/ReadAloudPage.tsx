"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import {
  Plus,
  Trash2,
  FilePlus,
  FileText,
  Wand2,
  Save,
  X,
  Square,
  History,
  Play,
  Columns,
  ChevronLeft,
  RefreshCw,
  Loader2,
  FolderOpen,
  Space,
  Type,
  AudioLines,
  Check,
  AlignLeft,
  LayoutGrid,
  Mic,
} from "lucide-react";
import { emit } from "@tauri-apps/api/event";
import MdEditor from "@/components/wiki/MdEditor";
import NormalizeActions from "./NormalizeActions";
import SourceAlignWorkspace from "./source_align/SourceAlignWorkspace";
import {
  CollapsibleSidebar,
  useCollapsibleSidebar,
} from "@/components/layout/CollapsibleSidebar";
import type {
  ReadAloudMeta,
  ReadAttempt,
  ReadStandardAudio,
  ReadText,
} from "@/lib/read_aloud/types";
import { scoreBadgeClasses } from "@/lib/read_aloud/types";
import { useIsActiveTab, usePageRefresh } from "@/lib/refresh/page-refresh";
import { clearDocIfOwned, setUiContext } from "@/lib/agent/ui-context";
import { DatasetCard } from "@/components/ui/dataset";
import { useAgentActions, reqStr } from "@/lib/agent/ui-actions";
import { useDatasetFamily } from "@/hooks/useDatasetFamily";
import { openDatasetInTab } from "@/lib/nav/open-in-tab";
import type { TabId } from "@/lib/shell/tabs";
import { rerecognizeAttempt } from "@/lib/read_aloud/rerecognize";
import { groupByText, hasUsableClip, listStandardAudio } from "@/lib/read_aloud/standardAudio";
import type { RaFontId, RaSpaceId } from "@/lib/read_aloud/fonts";
import {
  RA_FONT_OPTIONS,
  RA_SPACE_OPTIONS,
  getStoredRaFont,
  getStoredRaSpace,
  raFontOption,
  raSpaceOption,
  setStoredRaFont,
  setStoredRaSpace,
} from "@/lib/read_aloud/fonts";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import {
  Toolbar,
  Breadcrumb,
  ToolbarButton,
  ToolbarOverflow,
  ToolbarSelect,
  ToolbarMenuItem,
  RecordButton,
} from "@/components/ui/toolbar";
import { LocationsManager } from "@/components/ui/LocationsManager";
import PendingWritebackBanner from "@/components/ui/PendingWritebackBanner";
import PracticePanel, { type RecorderCtl } from "./PracticePanel";
import AttemptsPanel from "./AttemptsPanel";
import DatasetDialog from "./DatasetDialog";
import StandardAudioSection from "./StandardAudioSection";
import LiveTranscript from "./LiveTranscript";
import ConfirmDialog, { type ConfirmRequest } from "@/components/books/ConfirmDialog";

const LOG_MODULE = "read-aloud";

/** Recorder state mirrored from PracticePanel: the Record button in the toolbar
 *  reads it, and so does the live transcript dock under the practice pane. */
type RecorderMirror = {
  ready: boolean;
  recording: boolean;
  busy: boolean;
  liveText: string;
  liveBusy: boolean;
  liveSeconds: number;
};

const NO_RECORDER: RecorderMirror = {
  ready: false,
  recording: false,
  busy: false,
  liveText: "",
  liveBusy: false,
  liveSeconds: 0,
};

function getUUID(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

function nowIso(): string {
  return new Date().toISOString();
}

/** Format an ISO timestamp as `yyyy-mm-dd hh24:mi:ss` (local time) — the label
 *  used for an attempt's breadcrumb crumb. */
function formatStamp(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** Collapse whitespace and elide a long reference so one line of it fits. */
function preview(text: string, max = 140): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max).trimEnd()}…` : flat;
}

/** The cards of the text page, in the order they are stacked. The ids are the
 *  scroll anchors the sections sidebar jumps to. */
const MANAGE_SECTIONS = [
  { id: "manage-details", label: "Details", icon: FileText },
  { id: "manage-text", label: "Text to read", icon: AlignLeft },
  { id: "manage-scoring", label: "Scoring reference", icon: Wand2 },
  { id: "manage-audio", label: "Standard audio", icon: AudioLines },
];

/** One card of the text page: a header and a body. Every card is always open —
 *  the sections sidebar scrolls to these, and a collapsed card would make its
 *  own entry in that list a dead click. */
function ManageGroup({
  id,
  title,
  children,
}: {
  id?: string;
  title: string;
  children: ReactNode;
}) {
  return (
    <section
      id={id}
      className="scroll-mt-3 rounded-lg border border-border-default bg-bg-card"
    >
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border-default">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">
          {title}
        </h3>
      </div>
      <div className="flex flex-col gap-3 p-3">{children}</div>
    </section>
  );
}

export default function ReadAloudPage({ onNavigate }: { onNavigate: (tab: TabId) => void }) {
  const [datasets, setDatasets] = useState<ReadAloudMeta[]>([]);
  const [selectedDataset, setSelectedDataset] = useState<string>("");
  const [texts, setTexts] = useState<ReadText[]>([]);
  const [selectedText, setSelectedText] = useState<string | null>(null);

  // `isMobileApp()` is false during prerender (no navigator) and only turns true
  // inside the webview, so resolve it in an effect to avoid a hydration mismatch.
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // ── The text page (edit / view mode, mirroring the wiki page) ──────
  // One page holds everything about a text that is not the reading itself:
  // its title and note, the text, the LLM-normalized reference scoring reads,
  // and the standard audio pronunciation is measured against. `editing` swaps
  // the content column for that page and the sub-toolbar for Save / Close;
  // `editText` is the text being edited, or null for a brand-new one.
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState<ReadText | null>(null);
  const [title, setTitle] = useState("");
  const [note, setNote] = useState("");
  const [content, setContent] = useState("");
  const [normalized, setNormalized] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Briefly true after a save, so the header can confirm it: the page no longer
  // closes on Save, so without a word the button would look inert.
  const [savedFlash, setSavedFlash] = useState(false);
  // Bumped after a save so the standard-audio section re-reads its clips: a
  // content edit turns every clip stale, and that badge lives in the section.
  const [clipsVersion, setClipsVersion] = useState(0);
  useEffect(() => {
    if (!savedFlash) return;
    const t = window.setTimeout(() => setSavedFlash(false), 2000);
    return () => window.clearTimeout(t);
  }, [savedFlash]);

  // ── The text page's sections sidebar ──────────────────────────────
  // A table of contents for the four cards, not a filter: they are one draft,
  // and the audio card's "paused until saved" note only reads beside the fields
  // it is about. The hook opens the desktop column by default and always starts
  // a phone with the drawer closed; the dragged width persists per page.
  const manageSidebar = useCollapsibleSidebar({
    storageKey: "read-aloud-manage-width",
    defaultWidth: 200,
    minWidth: 150,
  });
  const manageScrollRef = useRef<HTMLDivElement | null>(null);
  const [activeSection, setActiveSection] = useState<string>(MANAGE_SECTIONS[0].id);
  // Blank space after the last card, sized so that card can sit at the top of
  // the column too. Only the last card needs it: every other one already has
  // the rest of the page below it to scroll against.
  const [tailSpace, setTailSpace] = useState(0);
  // Set when a click starts a smooth scroll: while it runs the scroll handler
  // must not fight it, or the highlight flickers through every card passed.
  const scrollToUntil = useRef(0);
  // Locations overlay (shared manager) — opened from the datasets-view overflow
  // menu, replacing the content area the same way the Cards page does.
  const [showLocations, setShowLocations] = useState(false);
  // The dataset's standard-audio page: aligning every text to a folder of
  // recordings. An overlay on the level rather than a deeper one — it belongs
  // to the dataset, not to a text, and it is reached from the texts view's menu.
  const [advancedView, setAdvancedView] = useState(false);
  // Bumped by the toolbar Refresh so an open Locations overlay re-reads its own
  // directory list instead of keeping the one it mounted with.
  const [locRefresh, setLocRefresh] = useState(0);

  // Which texts of the open dataset have a usable standard clip, from one
  // dataset-wide listing. Kept beside the texts rather than on `ReadText` so the
  // sync payload is untouched.
  const [clipsByText, setClipsByText] = useState<Map<string, ReadStandardAudio[]>>(
    () => new Map()
  );
  // The open text's own listing — the call that cuts a lazily-staged clip's
  // audio, since the backend materialises on a *text* query and deliberately not
  // on the dataset-wide one (badging a list is not worth decoding a book). Kept
  // beside that map rather than merged into it, so a slow dataset-wide answer
  // can never overwrite a fresh per-text one and take the player away.
  const [openTextClips, setOpenTextClips] = useState<{
    text: string;
    clips: ReadStandardAudio[];
  } | null>(null);
  // Bumped when the open text's clips must be re-read for a reason other than
  // switching texts: a refresh, or an apply from the source-align page.
  const [textClipsKey, setTextClipsKey] = useState(0);

  // The create dialog. Renaming and re-describing live on the dataset card's
  // own back face in the list, so there is no second edit surface to keep in
  // step with it (§ docs/design/cards.md).
  const [datasetDialog, setDatasetDialog] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  // The attempt history lives on its own page, opened from the practice view's
  // toolbar ("Attempts").
  const [attemptsView, setAttemptsView] = useState(false);
  // The attempt opened in the detail panel is lifted here so the breadcrumb can
  // show it as a level. `selectedAttempt` is reported up by AttemptsPanel and is
  // used only to label the crumb.
  const [selectedAttemptUuid, setSelectedAttemptUuid] = useState<string | null>(null);
  const [selectedAttempt, setSelectedAttempt] = useState<ReadAttempt | null>(null);
  const handleSelectAttempt = useCallback((a: ReadAttempt | null) => setSelectedAttempt(a), []);
  // Bumped after the toolbar deletes the open attempt so AttemptsPanel reloads.
  const [attemptsRefreshKey, setAttemptsRefreshKey] = useState(0);
  // True while the toolbar's "Assess again" re-transcribes the open attempt.
  const [rerecognizingAttempt, setRerecognizingAttempt] = useState(false);
  // Toggled by the toolbar's "Compare" button: the attempt detail then shows the
  // text and the recognized transcript side by side, sentence by sentence.
  const [compareView, setCompareView] = useState(false);
  // Which writing the text to read is set in. Device-local view state, like the
  // theme, so it is read straight from storage on first render and written back
  // when the toolbar changes it - see src/lib/read_aloud/fonts.ts.
  const [raFont, setRaFont] = useState<RaFontId>(() => getStoredRaFont());
  const chooseRaFont = useCallback((id: RaFontId) => {
    setRaFont(id);
    setStoredRaFont(id);
  }, []);
  // How far apart the words are - the same kind of device-local view state.
  const [raSpace, setRaSpace] = useState<RaSpaceId>(() => getStoredRaSpace());
  const chooseRaSpace = useCallback((id: RaSpaceId) => {
    setRaSpace(id);
    setStoredRaSpace(id);
  }, []);

  // Playback of the open attempt's recording, driven by the toolbar Play/Stop
  // button. The element is kept in a ref so playback can be stopped — on toggle,
  // when navigating away from the attempt, and on unmount — instead of running
  // fire-and-forget.
  const attemptAudioRef = useRef<HTMLAudioElement | null>(null);
  const [playingAttempt, setPlayingAttempt] = useState(false);

  const stopAttemptAudio = useCallback(() => {
    const el = attemptAudioRef.current;
    if (el) {
      el.onended = null;
      el.pause();
      el.currentTime = 0;
      attemptAudioRef.current = null;
    }
    setPlayingAttempt(false);
  }, []);

  const toggleAttemptPlay = useCallback(
    (url?: string | null) => {
      // Already playing → stop (toggle off), regardless of which attempt.
      if (attemptAudioRef.current) {
        stopAttemptAudio();
        return;
      }
      if (!url) return;
      const el = new Audio(convertFileSrc(url));
      attemptAudioRef.current = el;
      el.onended = () => {
        attemptAudioRef.current = null;
        setPlayingAttempt(false);
      };
      setPlayingAttempt(true);
      el.play().catch(() => {
        attemptAudioRef.current = null;
        setPlayingAttempt(false);
      });
    },
    [stopAttemptAudio]
  );

  // Stop playback when the open attempt changes or is cleared, and on unmount.
  useEffect(() => {
    return () => stopAttemptAudio();
  }, [selectedAttemptUuid, stopAttemptAudio]);

  // Compare is per-attempt; leave it whenever a different attempt is opened (or
  // the detail is closed) so it never leaks across attempts.
  useEffect(() => {
    setCompareView(false);
  }, [selectedAttemptUuid]);

  // The Record button and the live transcript dock live in this page frame, but
  // the recorder + submit logic stay in PracticePanel; it publishes its control
  // here (and clears it on unmount).
  const recToggleRef = useRef<() => void>(() => {});
  const [rec, setRec] = useState<RecorderMirror>(NO_RECORDER);
  const handleRecorder = useCallback((ctl: RecorderCtl | null) => {
    if (!ctl) {
      recToggleRef.current = () => {};
      setRec(NO_RECORDER);
      return;
    }
    recToggleRef.current = ctl.toggle;
    setRec({
      ready: true,
      recording: ctl.recording,
      busy: ctl.busy,
      liveText: ctl.liveText,
      liveBusy: ctl.liveBusy,
      liveSeconds: ctl.liveSeconds,
    });
  }, []);

  const loadDatasets = useCallback(async (preferUuid?: string) => {
    if (!isTauri()) return;
    try {
      const list = await invoke<ReadAloudMeta[]>("read_aloud_list");
      setDatasets(list);
      setSelectedDataset((prev) => {
        const target = preferUuid ?? prev;
        if (target && list.some((d) => d.uuid === target)) return target;
        // No explicit target: fall back to the dataset list view (wiki-style),
        // letting the user pick a dataset rather than auto-opening the first.
        return "";
      });
    } catch (e) {
      logError(`load datasets failed: ${String(e)}`, LOG_MODULE);
    }
  }, []);

  const loadTexts = useCallback(async (datasetUuid: string) => {
    if (!isTauri() || !datasetUuid) {
      setTexts([]);
      return;
    }
    try {
      const list = await invoke<ReadText[]>("read_aloud_list_texts", { datasetUuid });
      setTexts(list);
      // Keep a still-valid selection, otherwise show the text list (no auto-open).
      setSelectedText((prev) => (prev && list.some((t) => t.uuid === prev) ? prev : null));
    } catch (e) {
      logError(`load texts failed: ${String(e)}`, LOG_MODULE);
    }
  }, []);

  // The dataset-wide clip listing, grouped by text: one call badges every row
  // in the texts list, and the text page refreshes it when a clip changes.
  const loadStandardClips = useCallback(async (datasetUuid: string) => {
    if (!isTauri() || !datasetUuid) {
      setClipsByText(new Map());
      return;
    }
    try {
      setClipsByText(groupByText(await listStandardAudio(datasetUuid)));
    } catch (e) {
      // A badge is a hint, not the page: never surface this as an error.
      logError(`load standard audio failed: ${String(e)}`, LOG_MODULE);
    }
  }, []);

  useEffect(() => {
    loadDatasets();
  }, [loadDatasets]);

  useEffect(() => {
    loadTexts(selectedDataset);
    void loadStandardClips(selectedDataset);
  }, [selectedDataset, loadTexts, loadStandardClips]);

  // Re-read everything the current level shows. Driven by the shared page
  // refresh — there is no toolbar button any more — and never by the editor
  // draft, which lives in local form state.
  const handleRefresh = useCallback(() => {
    void loadDatasets();
    if (selectedDataset) {
      void loadTexts(selectedDataset);
      void loadStandardClips(selectedDataset);
    }
    setLocRefresh((n) => n + 1);
  }, [loadDatasets, loadStandardClips, loadTexts, selectedDataset]);

  // Inside a text — the practice pane, the history, one open attempt —
  // everything on screen is the same subject, so one plain Refresh re-reads all
  // of it: the dataset's texts (the open one and its best score), the standard
  // clips behind the audio controls, and the attempt history.
  const handleRefreshText = useCallback(() => {
    if (selectedDataset) {
      void loadTexts(selectedDataset);
      void loadStandardClips(selectedDataset);
    }
    setTextClipsKey((k) => k + 1);
    setAttemptsRefreshKey((k) => k + 1);
  }, [loadStandardClips, loadTexts, selectedDataset]);

  // The attempts page is a per-text concern; close it whenever the selected
  // text changes so it never leaks across texts.
  useEffect(() => {
    setAttemptsView(false);
    setSelectedAttemptUuid(null);
  }, [selectedText]);

  // Leaving the attempts page clears any open attempt detail.
  useEffect(() => {
    if (!attemptsView) {
      setSelectedAttemptUuid(null);
      setSelectedAttempt(null);
    }
  }, [attemptsView]);

  const activeText = texts.find((t) => t.uuid === selectedText) ?? null;
  const activeDataset = datasets.find((d) => d.uuid === selectedDataset) ?? null;

  // What the practice pane and the attempts page read: the open text's own
  // listing once it is loaded (it is the one with a cut wav), the dataset-wide
  // badge data until then.
  const activeTextClips =
    (openTextClips && openTextClips.text === activeText?.uuid ? openTextClips.clips : undefined) ??
    (activeText ? clipsByText.get(activeText.uuid) : undefined);

  // Opening a text is what gives a span clip its wav, so this effect is a
  // materialisation step as much as a read — see `listStandardAudio`'s
  // `textUuid` parameter. A `null` here always means "not loaded for *this*
  // text", which is why the text uuid is stored with the clips.
  useEffect(() => {
    const textUuid = activeText?.uuid;
    if (!isTauri() || !selectedDataset || !textUuid) {
      setOpenTextClips(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const clips = await listStandardAudio(selectedDataset, textUuid);
        if (!cancelled) setOpenTextClips({ text: textUuid, clips });
      } catch (e) {
        // Never fatal: the badge data still renders the row.
        if (!cancelled) logError(`load standard audio failed: ${String(e)}`, LOG_MODULE);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedDataset, activeText?.uuid, textClipsKey]);

  // Wiki-style view modes derived from the selection: no dataset → the dataset
  // list, a dataset but no text → its text list, a text → the practice pane.
  // The standard-audio page is the one mode that is not derived — it is opened
  // and closed explicitly — so it wins over the selection while it is open.
  const viewMode: "datasets" | "texts" | "practice" | "advanced" =
    advancedView && activeDataset
      ? "advanced"
      : !activeDataset
        ? "datasets"
        : activeText
          ? "practice"
          : "texts";

  // The text being practised, published for the agent's "where am I" and for the
  // card capture's source line. Only while this tab is on screen: pages stay
  // mounted, so an un-gated report would keep naming this text from another page.
  const readAloudActive = useIsActiveTab("read-aloud");
  const textDoc =
    viewMode === "practice" && selectedDataset && activeText
      ? `read_aloud:${selectedDataset}:${activeText.uuid}`
      : null;
  useEffect(() => {
    if (readAloudActive && textDoc) setUiContext({ active_doc: textDoc });
    else clearDocIfOwned("read_aloud");
  }, [readAloudActive, textDoc]);

  // One refresh means different things per level, so the view picks the one that
  // re-reads what is actually on screen: the linked directories, the dataset
  // list, the open dataset's texts, or — once a text is open — the whole of that
  // text, which the practice, attempts and attempt levels all show.
  const refreshActiveText = viewMode === "practice" && !showLocations;
  const refreshCurrentView = refreshActiveText ? handleRefreshText : handleRefresh;

  // Draft differs from the stored text (or, for a new one, from empty fields).
  const dirty =
    title !== (editText?.title ?? "") ||
    note !== (editText?.note ?? "") ||
    content !== (editText?.content ?? "") ||
    normalized !== (editText?.normalized_content ?? "");

  // Keep the current level current without a toolbar button: the shared hook
  // re-reads on activation, on app resume and on `dataset-list-changed`, and
  // refuses while the editor holds unsaved changes.
  usePageRefresh("read-aloud", {
    refresh: () => refreshCurrentView(),
    canRefresh: () => !dirty,
    events: ["dataset-list-changed"],
  });

  // What the scorer actually reads: the normalized reference when one is set,
  // otherwise the text itself. Shown on the page so the two boxes above it are
  // never a guess about which one wins.
  const scoringText = normalized.trim() || content.trim();

  // Choosing another dataset (or deleting the open one) leaves the standard-audio
  // page: its sources and proposals belong to the dataset it was opened on.
  useEffect(() => {
    setAdvancedView(false);
  }, [selectedDataset]);

  const goDatasets = () => {
    setAdvancedView(false);
    setSelectedDataset("");
    setSelectedText(null);
  };
  const goTexts = () => {
    setAdvancedView(false);
    setSelectedText(null);
  };

  // The family of the open dataset, read so the trail can show which book it
  // belongs to. `report.anchor` is the book member's uuid, and `display_name`
  // is that book's name when there is one — so a legacy dataset (no key, no
  // family) yields no anchor and simply adds no crumb.
  const { report: family } = useDatasetFamily(selectedDataset);
  const bookUuid = family?.anchor && family.anchor !== selectedDataset ? family.anchor : null;

  // What an agent — and the book page's companions strip — can drive here.
  // The lookup refuses a uuid this page has not loaded, in the same wording the
  // dictation and cards actions use, so a caller that arrives before the dataset
  // list does can retry instead of guessing.
  useAgentActions("read-aloud", [
    {
      id: "read-aloud.open-dataset",
      label: "Open a read-aloud dataset and show its texts.",
      kind: "read",
      params_schema: {
        dataset_uuid: {
          type: "string",
          description: "The read-aloud dataset uuid, from read_aloud_list or dataset_family.",
          required: true,
        },
      },
      run: (params) => {
        const uuid = reqStr("read-aloud.open-dataset", params, "dataset_uuid");
        const found = datasets.find((d) => d.uuid === uuid);
        if (!found) {
          throw new Error(
            `no read-aloud dataset with uuid '${uuid}' is loaded on this page. Loaded: ${
              datasets.map((d) => `${d.name} (${d.uuid})`).join(", ") ||
              "(none yet — the list may still be loading; retry once)"
            }. read_aloud_list has the full set.`
          );
        }
        setAdvancedView(false);
        setSelectedDataset(uuid);
        setSelectedText(null);
        setShowLocations(false);
        return { opened: uuid, name: found.name };
      },
    },
  ]);

  // ── Editor lifecycle ──────────────────────────────────────────────
  const startEdit = (t: ReadText | null) => {
    setEditText(t);
    setTitle(t?.title ?? "");
    setNote(t?.note ?? "");
    setContent(t?.content ?? "");
    setNormalized(t?.normalized_content ?? "");
    setMenuOpen(false);
    setError(null);
    setSavedFlash(false);
    setActiveSection(MANAGE_SECTIONS[0].id);
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setEditText(null);
    setError(null);
    setSavedFlash(false);
  };

  const saveText = async () => {
    if (!isTauri() || !selectedDataset) return;
    if (!content.trim()) {
      setError("The text content must not be empty.");
      return;
    }
    setSaving(true);
    setError(null);
    const now = nowIso();
    const trimmedContent = content.trim();
    const contentChanged = editText != null && trimmedContent !== editText.content.trim();
    // Mirrors the backend staleness rule: editing the content invalidates a
    // normalization that was derived from the old one.
    const normalizedValue = contentChanged ? null : normalized.trim() || null;
    const payload: ReadText = {
      uuid: editText?.uuid ?? getUUID(),
      dataset_uuid: selectedDataset,
      order_num: editText?.order_num ?? texts.length + 1,
      title: title.trim(),
      note: note.trim(),
      content: trimmedContent,
      normalized_content: normalizedValue,
      best_score: editText?.best_score ?? null,
      attempt_count: editText?.attempt_count ?? 0,
      created_at: editText?.created_at ?? now,
      updated_at: now,
    };
    try {
      await invoke("read_aloud_save_text", { datasetUuid: selectedDataset, text: payload });
      // Stay on the page rather than returning to the practice view: a text that
      // was just created has no other way to get its standard audio (clips
      // attach to an existing text), and an edited one has probably just turned
      // its clips stale — the section below is where both get fixed.
      //
      // The backend keeps the stored normalization when only the box was emptied
      // with the text untouched, so mirror that rather than showing a blank the
      // save did not actually write.
      const normalizedAfter = contentChanged
        ? ""
        : (normalizedValue ?? editText?.normalized_content ?? "");
      const saved: ReadText = { ...payload, normalized_content: normalizedAfter || null };
      setEditText(saved);
      setTitle(saved.title);
      setNote(saved.note);
      setContent(saved.content);
      setNormalized(normalizedAfter);
      setSelectedText(saved.uuid);
      setSavedFlash(true);
      await loadTexts(selectedDataset);
      setClipsVersion((n) => n + 1);
      void loadStandardClips(selectedDataset);
    } catch (e) {
      const msg = String(e);
      logError(`save text failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  const deleteText = (t: ReadText) => {
    setConfirm({
      title: "Delete text",
      // The cascade takes the text's standard audio and every attempt with it,
      // so the confirmation says so rather than only "recordings".
      message: `Delete "${t.title || "Untitled"}" with its standard audio and every attempt?`,
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_text", { datasetUuid: selectedDataset, uuid: t.uuid });
          if (selectedText === t.uuid) setSelectedText(null);
          await loadTexts(selectedDataset);
        } catch (e) {
          logError(`delete text failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  // ── Section navigation ────────────────────────────────────────────
  const goToSection = (id: string) => {
    setActiveSection(id);
    // A phone's drawer covers the page it navigates; the desktop column stays
    // open so the list can be used twice in a row.
    manageSidebar.closeOnSelect();
    scrollToUntil.current = Date.now() + 600;
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  // Which card's header last passed the top of the scroll column. The column
  // scrolls inside the page rather than the window, so this compares rects with
  // its own top, not with 0.
  const onManageScroll = () => {
    if (Date.now() < scrollToUntil.current) return;
    const root = manageScrollRef.current;
    if (!root) return;
    const top = root.getBoundingClientRect().top;
    let current = MANAGE_SECTIONS[0].id;
    for (const s of MANAGE_SECTIONS) {
      const el = document.getElementById(s.id);
      if (el && el.getBoundingClientRect().top - top <= 24) current = s.id;
    }
    // The last card may be too short to ever reach the top; at the bottom of
    // the column it is the one being read.
    if (root.scrollTop + root.clientHeight >= root.scrollHeight - 8) {
      current = MANAGE_SECTIONS[MANAGE_SECTIONS.length - 1].id;
    }
    setActiveSection(current);
  };

  // Measure the room the last card needs to reach the top of the column: its
  // own height is the part of the viewport it cannot cover, so that is exactly
  // how much blank space it takes. Recomputed when the column is resized or the
  // audio card grows and shrinks with its clips.
  useEffect(() => {
    if (!editing) {
      setTailSpace(0);
      return;
    }
    const root = manageScrollRef.current;
    const last = document.getElementById(MANAGE_SECTIONS[MANAGE_SECTIONS.length - 1].id);
    if (!root || !last) return;
    const measure = () => {
      // The column's own bottom padding already contributes scroll range.
      const padding = 16;
      setTailSpace(Math.max(0, Math.round(root.clientHeight - last.offsetHeight - padding)));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    observer.observe(last);
    return () => observer.disconnect();
  }, [editing]);

  // Re-transcribe the open attempt with the current model. The same work as the
  // attempts list's per-row button, reached from the attempt page's toolbar so
  // the reader does not have to navigate back to the list to redo one take.
  // `rerecognizeAttempt` decides which model and reports each stage to the live
  // assessment log; see the note there.
  const assessSelectedAgain = async () => {
    if (!isTauri() || !selectedAttempt || !selectedDataset) return;
    const a = selectedAttempt;
    setRerecognizingAttempt(true);
    setError(null);
    try {
      await rerecognizeAttempt(selectedDataset, a);
      // Transcript, score and assessment all changed: reload the list (which
      // re-reports the open attempt upward, so the detail panel and the
      // breadcrumb both see the new numbers) and the text's best score.
      setAttemptsRefreshKey((k) => k + 1);
      await loadTexts(selectedDataset);
    } catch (e) {
      const msg = String(e);
      logError(`assess again failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setRerecognizingAttempt(false);
    }
  };

  // Delete the attempt currently open in the detail panel. The button lives in
  // the toolbar; AttemptsPanel reloads via the bumped `attemptsRefreshKey`.
  const deleteSelectedAttempt = () => {
    if (!selectedAttempt || !selectedDataset) return;
    const a = selectedAttempt;
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid: selectedDataset, uuid: a.uuid });
          setSelectedAttemptUuid(null);
          setAttemptsRefreshKey((k) => k + 1);
          await loadTexts(selectedDataset);
        } catch (e) {
          logError(`delete attempt failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  const deleteDataset = (meta: ReadAloudMeta) => {
    setConfirm({
      title: "Delete dataset",
      message: `Delete the read-aloud dataset "${meta.name}" and everything inside it?\n\nThe folder is moved to trash, not permanently deleted.`,
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete", { uuid: meta.uuid });
          if (selectedDataset === meta.uuid) setSelectedDataset("");
          await loadDatasets();
        } catch (e) {
          logError(`delete dataset failed: ${String(e)}`, LOG_MODULE);
        }
      },
    });
  };

  const fieldCls =
    "w-full p-2 rounded-md bg-bg-muted border border-border-default text-text-primary text-sm focus:outline-none focus:border-accent-bg";
  const labelCls = "text-xs font-medium text-text-secondary mb-1 block";

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0 bg-bg-body">
      {/* Thin-client reminder: recorded takes and text edits wait in the
          writeback queue until a sync round flushes them to the hub. */}
      <PendingWritebackBanner mobile={mobile} />
      {/* Breadcrumb navigation — Read Aloud › dataset › text, mirroring the wiki
          page's edge-to-edge trail. Crumbs are inert while editing so a stray
          click can't discard the draft. */}
      <Breadcrumb>
        {editing ? (
          <span className="shrink-0 text-accent">Read Aloud</span>
        ) : showLocations ? (
          <Breadcrumb.Link onClick={() => setShowLocations(false)}>Read Aloud</Breadcrumb.Link>
        ) : viewMode === "datasets" ? (
          <Breadcrumb.Current grow={false}>Read Aloud</Breadcrumb.Current>
        ) : (
          <Breadcrumb.Link onClick={goDatasets}>Read Aloud</Breadcrumb.Link>
        )}
        {showLocations && (
          <>
            <Breadcrumb.Sep />
            <Breadcrumb.Current grow={false}>Locations</Breadcrumb.Current>
          </>
        )}
        {bookUuid && (
          <>
            <Breadcrumb.Sep />
            {/* The book this read-aloud dataset belongs to: one family, one key.
                Clicking crosses to the Books page and opens it there. */}
            <Breadcrumb.Link
              truncate
              title={family?.display_name ?? "Book"}
              onClick={() => void openDatasetInTab(onNavigate, "books", "books.open-book", bookUuid)}
            >
              {family?.display_name ?? "Book"}
            </Breadcrumb.Link>
          </>
        )}
        {activeDataset && (
          <>
            <Breadcrumb.Sep />
            {editing ? (
              <span className="truncate min-w-0 max-w-[240px] text-accent" title={activeDataset.name}>
                {activeDataset.name}
              </span>
            ) : (
              <Breadcrumb.Link
                variant={viewMode === "texts" ? "current" : "link"}
                truncate={viewMode !== "texts"}
                title={activeDataset.name}
                onClick={goTexts}
              >
                {activeDataset.name}
              </Breadcrumb.Link>
            )}
          </>
        )}
        {!editing && viewMode === "advanced" && (
          <>
            <Breadcrumb.Sep />
            <Breadcrumb.Current grow={false}>Standard audios</Breadcrumb.Current>
          </>
        )}
        {editing ? (
          <>
            <Breadcrumb.Sep />
            <span className="text-text-primary font-medium truncate min-w-0 max-w-[320px]">
              {editText ? editText.title || "Untitled" : "New text"}
            </span>
          </>
        ) : (
          viewMode === "practice" &&
          activeText && (
            <>
              <Breadcrumb.Sep />
              {attemptsView ? (
                <Breadcrumb.Link
                  truncate
                  title={activeText.title || "Untitled"}
                  onClick={() => setAttemptsView(false)}
                >
                  {activeText.title || "Untitled"}
                </Breadcrumb.Link>
              ) : (
                <Breadcrumb.Current title={activeText.title || "Untitled"}>
                  {activeText.title || "Untitled"}
                </Breadcrumb.Current>
              )}
              {attemptsView && (
                <>
                  <Breadcrumb.Sep />
                  {selectedAttempt ? (
                    <Breadcrumb.Link onClick={() => setSelectedAttemptUuid(null)}>
                      Attempts
                    </Breadcrumb.Link>
                  ) : (
                    <span className="text-text-primary font-medium shrink-0">Attempts</span>
                  )}
                  {selectedAttempt && (
                    <>
                      <Breadcrumb.Sep />
                      <span className="text-text-primary font-medium shrink-0 tabular-nums">
                        {formatStamp(selectedAttempt.created_at)}
                      </span>
                    </>
                  )}
                </>
              )}
            </>
          )
        )}
      </Breadcrumb>

      {/* Sub-toolbar: contextual actions for the current view — the same stacked
          bar treatment the wiki page uses. While an inline editor is open it hosts
          the Save / Close controls instead. */}
      <Toolbar mobile={mobile}>
        {/* On the text page the bar's first control is the one that opens its
            table of contents, beside what it opens and out of the way of
            Save / Close at the other end. */}
        {editing && (
          <ToolbarButton
            icon={LayoutGrid}
            label="Sections"
            variant="toggleSoft"
            active={manageSidebar.visible}
            onClick={manageSidebar.toggle}
            title={manageSidebar.visible ? "Hide sections" : "Show sections"}
          />
        )}
        {!editing && viewMode === "advanced" && (
          <ToolbarButton
            icon={ChevronLeft}
            label="Texts"
            title="Back to the texts list"
            onClick={goTexts}
          />
        )}
        <div className="flex-1 min-w-0" />

        {editing ? (
          <>
            <ToolbarButton
              variant="toggle"
              active={dirty}
              icon={Save}
              label={saving ? "Saving…" : "Save"}
              title="Save"
              disabled={saving || !dirty}
              onClick={() => void saveText()}
            />
            <ToolbarButton
              icon={X}
              label="Close"
              title="Close the editor"
              onClick={cancelEdit}
            />
          </>
        ) : (
          <>
            {viewMode === "datasets" && !mobile && (
              <ToolbarButton
                icon={Plus}
                label="New Dataset"
                title="New dataset"
                onClick={() => setDatasetDialog(true)}
              />
            )}

            {/* The dataset's own name and description are edited on the card's
                back face in the list, so this view only adds texts. */}
            {viewMode === "texts" && !mobile && (
              <ToolbarButton
                icon={FilePlus}
                label="New Text"
                title="Add text"
                onClick={() => startEdit(null)}
              />
            )}

            {viewMode === "practice" && activeText && !attemptsView && (
              <>
                <RecordButton
                  recording={rec.recording}
                  busy={rec.busy}
                  disabled={!rec.ready || rec.busy}
                  onClick={() => recToggleRef.current()}
                />
                <ToolbarButton
                  icon={History}
                  label="Attempts"
                  title="Show attempts"
                  onClick={() => setAttemptsView(true)}
                />
                {/* Which writing the text is set in. A view setting, so it shows
                    its value in the bar rather than hiding in the overflow. */}
                <ToolbarSelect icon={Type} label="Writing" value={raFontOption(raFont).label}>
                  {RA_FONT_OPTIONS.map((o) => (
                    <ToolbarMenuItem
                      key={o.id}
                      checked={o.id === raFont}
                      onClick={() => chooseRaFont(o.id)}
                    >
                      <span className="flex flex-col items-start leading-tight">
                        {o.label}
                        <span className="text-[11px] text-text-tertiary">{o.hint}</span>
                      </span>
                    </ToolbarMenuItem>
                  ))}
                </ToolbarSelect>
                {/* How wide the gap between the words is. It matters most in the
                    cursive models, where a space is the only thing left marking
                    where one word ends and the next begins. */}
                <ToolbarSelect icon={Space} label="Spacing" value={raSpaceOption(raSpace).label}>
                  {RA_SPACE_OPTIONS.map((o) => (
                    <ToolbarMenuItem
                      key={o.id}
                      checked={o.id === raSpace}
                      onClick={() => chooseRaSpace(o.id)}
                    >
                      <span className="flex flex-col items-start leading-tight">
                        {o.label}
                        <span className="text-[11px] text-text-tertiary">{o.hint}</span>
                      </span>
                    </ToolbarMenuItem>
                  ))}
                </ToolbarSelect>
                {/* Everything about the text that is not the reading itself:
                    its title and note, the text, its normalized scoring
                    reference, and the standard audio pronunciation is measured
                    against. A text with no clip cannot be scored for
                    pronunciation at all, so this is also where that gets fixed. */}
                <ToolbarButton
                  icon={AudioLines}
                  label="Manage"
                  title="Manage this text: text, normalized reference and standard audio"
                  onClick={() => startEdit(activeText)}
                />
              </>
            )}

            {/* Attempt detail — its Play / Assess again / Delete actions
                live here rather than inside the panel, mirroring how Record
                lives in the toolbar. */}
            {viewMode === "practice" && activeText && attemptsView && selectedAttempt && (
              <>
                <ToolbarButton
                  variant="toggle"
                  active={compareView}
                  icon={Columns}
                  label="Compare"
                  title="Compare the text and the transcript sentence by sentence"
                  onClick={() => setCompareView((v) => !v)}
                />
                {selectedAttempt.audio_url && (
                  <ToolbarButton
                    icon={rerecognizingAttempt ? Loader2 : RefreshCw}
                    iconProps={rerecognizingAttempt ? { className: "animate-spin" } : undefined}
                    label="Assess again"
                    title="Assess again (re-transcribe this recording with the current model and score it again)"
                    disabled={rerecognizingAttempt}
                    onClick={() => void assessSelectedAgain()}
                  />
                )}
                {selectedAttempt.audio_url && (
                  <ToolbarButton
                    icon={playingAttempt ? Square : Play}
                    label={playingAttempt ? "Stop" : "Play"}
                    title={playingAttempt ? "Stop playback" : "Play recording"}
                    onClick={() => toggleAttemptPlay(selectedAttempt.audio_url)}
                  />
                )}
                <ToolbarButton
                  variant="danger"
                  icon={Trash2}
                  label="Delete"
                  title="Delete attempt"
                  onClick={deleteSelectedAttempt}
                />
              </>
            )}
          </>
        )}

        {/* Overflow menu — the three-dot home for occasional actions: Locations
            and Delete (desktop) plus, on a phone, the Add/New actions, so the
            visible stacked row keeps only the contextual actions. Rendered only
            where the menu has entries — the Refresh item that used to keep it
            alive at every level is gone — and hidden while inline-editing a text
            (Save/Close own the bar there). */}
        {!editing &&
          (viewMode === "datasets" ||
            viewMode === "texts" ||
            (viewMode === "practice" && !!activeText && !attemptsView)) && (
          <ToolbarOverflow open={menuOpen} onOpenChange={setMenuOpen}>
            {viewMode === "datasets" && (
              <>
                <ToolbarMenuItem icon={FolderOpen} onClick={() => setShowLocations(true)}>
                  Locations
                </ToolbarMenuItem>
                {mobile && (
                  <ToolbarMenuItem icon={Plus} onClick={() => setDatasetDialog(true)}>
                    New Dataset
                  </ToolbarMenuItem>
                )}
              </>
            )}
            {viewMode === "texts" && (
              <>
                {mobile && (
                  <ToolbarMenuItem icon={FilePlus} onClick={() => startEdit(null)}>
                    New Text
                  </ToolbarMenuItem>
                )}
                {/* The dataset's bulk tools are occasional, and they act on the
                    whole dataset rather than on the row under the cursor, so
                    they live behind the dots and open a page of their own. The
                    label is the job, not the page — what it does is cut one clip
                    per text out of an audiobook, and the page it opens says so. */}
                <ToolbarMenuItem icon={AudioLines} onClick={() => setAdvancedView(true)}>
                  Init Audios
                </ToolbarMenuItem>
                <ToolbarMenuItem
                  icon={Trash2}
                  danger
                  disabled={!activeDataset}
                  onClick={() => {
                    if (activeDataset) deleteDataset(activeDataset);
                  }}
                >
                  Delete dataset
                </ToolbarMenuItem>
              </>
            )}
            {viewMode === "practice" && activeText && !attemptsView && (
              <ToolbarMenuItem icon={Trash2} danger onClick={() => deleteText(activeText)}>
                Delete text
              </ToolbarMenuItem>
            )}
          </ToolbarOverflow>
        )}
      </Toolbar>

      {/* Content — while editing this is the inline text form; otherwise the
          practice pane (own scroll), the standard-audio page, or the dataset/text
          lists. The Locations overlay replaces all of them while open (Cards-page
          pattern). */}
      {showLocations ? (
        <div className="flex-1 min-h-0 overflow-hidden flex flex-col">
          {error && (
            <div className="mx-4 mt-4 p-3 bg-error-bg text-error-text rounded-md text-sm">{error}</div>
          )}
          <div className="flex-1 min-h-0">
            <LocationsManager
              datasetType="read_aloud"
              description="Read-aloud datasets are stored across dataset directories. Add linked directories to store datasets on external drives or other locations."
              onError={setError}
              onChanged={() => void loadDatasets()}
              countFor={(path) => datasets.filter((ds) => ds.path.startsWith(path)).length}
              refreshToken={locRefresh}
            />
          </div>
        </div>
      ) : editing ? (
        // The text page with its table of contents. The sidebar scrolls to a
        // card rather than filtering the page: the four cards are one draft,
        // and the audio card's "paused until saved" note only reads beside the
        // fields it refers to.
        <div className="relative flex flex-1 min-h-0 overflow-hidden">
          <CollapsibleSidebar sidebar={manageSidebar}>
            <nav className="flex flex-col gap-1 p-3 overflow-y-auto min-h-0">
              <div className="text-xs font-semibold text-text-tertiary uppercase tracking-wide px-2 pt-1 pb-2 mb-1 border-b border-border-light">
                Sections
              </div>
              {MANAGE_SECTIONS.map((s) => {
                const Icon = s.icon;
                return (
                  <button
                    type="button"
                    key={s.id}
                    className={`flex items-center gap-2 px-2 py-1.5 rounded-md text-sm text-left cursor-pointer transition-colors ${
                      activeSection === s.id
                        ? "bg-bg-hover text-text-primary font-medium"
                        : "text-text-secondary hover:bg-bg-hover hover:text-text-primary"
                    }`}
                    onClick={() => goToSection(s.id)}
                  >
                    <Icon size={15} className="shrink-0" />
                    <span className="truncate">{s.label}</span>
                  </button>
                );
              })}
            </nav>
          </CollapsibleSidebar>

          <div
            ref={manageScrollRef}
            onScroll={onManageScroll}
            className="flex-1 min-h-0 min-w-0 overflow-y-auto overflow-x-hidden p-4 flex flex-col gap-4"
          >
            {/* Page header: which text this is, and whether the draft is stored.
                Save no longer closes the page, so the header has to say that it
                worked — and say the opposite while the draft runs ahead of it. */}
            <div className="flex flex-wrap items-center gap-2 min-w-0">
              <h2 className="text-base font-semibold text-text-primary truncate min-w-0">
                {editText ? title.trim() || "Untitled" : "New text"}
              </h2>
              {/* Dirty first: typing within the two-second window must not keep
                  claiming the text is saved. */}
              {dirty ? (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary">
                  {editText ? "unsaved changes" : "not saved yet"}
                </span>
              ) : savedFlash ? (
                <span className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-full bg-success-bg text-text-primary">
                  <Check size={10} /> Saved
                </span>
              ) : null}
            </div>

            {error && (
              <div className="p-2 rounded-md bg-error-bg text-error-text text-xs break-words">
                {error}
              </div>
            )}

            <ManageGroup id="manage-details" title="Details">
              <div>
                <label className={labelCls}>Title</label>
                <input
                  className={fieldCls}
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  placeholder="e.g. Der Erlkönig — first stanza"
                />
              </div>
              <div>
                <label className={labelCls}>Note (source, location, …)</label>
                <input
                  className={fieldCls}
                  value={note}
                  onChange={(e) => setNote(e.target.value)}
                  placeholder="e.g. Goethe, from a textbook p.42"
                />
              </div>
            </ManageGroup>

            <ManageGroup id="manage-text" title="Text to read">
              <MdEditor
                value={content}
                onChange={setContent}
                spellCheck={false}
                shortcutsOpen={false}
                textareaBgClass="bg-bg-muted"
              />
            </ManageGroup>

            <ManageGroup id="manage-scoring" title="Scoring reference">
              <p className="text-[11px] text-text-tertiary">
                <span className="text-text-secondary">Normalized text</span> — numbers and
                abbreviations spelled out so they match a transcription. Leave it empty to score
                against the text to read. Editing the text clears it, because it described the
                old wording; emptying this box alone does not, since the stored reference is kept
                while the text is unchanged. Generating standard audio reads the same reference,
                so the two always agree.
              </p>
              {/* Two ways to fill the box below: the app's own prompt through a local
                  model, or that same prompt on the clipboard for any other LLM.
                  Both only fill the draft — the gate measures, Save stores. */}
              <NormalizeActions
                content={content}
                baseline={
                  editText ? editText.normalized_content?.trim() || editText.content.trim() : ""
                }
                datasetUuid={selectedDataset}
                textUuid={editText?.uuid ?? null}
                candidate={normalized}
                onCandidate={setNormalized}
                disabled={saving}
                refreshKey={clipsVersion}
              />
              <MdEditor
                value={normalized}
                onChange={setNormalized}
                spellCheck={false}
                shortcutsOpen={false}
                textareaBgClass="bg-bg-muted"
              />
              {/* Which of the two boxes the scorer will actually read. */}
              <div className="rounded-md bg-bg-muted px-2.5 py-2">
                <div className="text-[11px] text-text-secondary">
                  <span className="font-medium">Scoring will read</span>{" "}
                  <span className="text-text-tertiary">
                    ({normalized.trim() ? "the normalized reference" : "the text to read"})
                  </span>
                </div>
                <div className="text-[11px] text-text-primary mt-0.5 break-words">
                  {scoringText ? (
                    preview(scoringText)
                  ) : (
                    <span className="text-text-tertiary">
                      nothing yet — the text to read is empty
                    </span>
                  )}
                </div>
              </div>
            </ManageGroup>

            {/* The clips belong to the *saved* text: a clip made from an unsaved
                draft would be stale the moment Save landed. A new text has no row
                to hang them on yet. Both cases keep the card visible and say why
                rather than letting a section vanish — the page never changes
                shape under the cursor. */}
            <div id="manage-audio" className="scroll-mt-3">
              {editText && selectedDataset ? (
                <StandardAudioSection
                  datasetUuid={selectedDataset}
                  textUuid={editText.uuid}
                  disabled={dirty}
                  refreshKey={clipsVersion}
                  onChanged={() => void loadStandardClips(selectedDataset)}
                  // The language picker on this card is also how a dataset that
                  // has no language gets one — and without one, no per-language
                  // voice preference can apply at all.
                  onLanguageChange={async (language) => {
                    const meta = datasets.find((d) => d.uuid === selectedDataset);
                    if (!meta) throw new Error("the dataset is not loaded");
                    await invoke("read_aloud_update", {
                      uuid: meta.uuid,
                      name: meta.name,
                      description: meta.description,
                      language,
                    });
                    await loadDatasets(meta.uuid);
                  }}
                />
              ) : (
                <ManageGroup title="Standard audio">
                  <p className="text-[11px] text-text-secondary">
                    Save the text first — a clip is recorded against the text as it is stored, so
                    there is nothing to attach one to yet.
                  </p>
                </ManageGroup>
              )}
            </div>

            {/* Tail room, so the last card can be scrolled to the top rather
                than stopping at the bottom of the column. Invisible until you
                scroll into it. */}
            {tailSpace > 0 && <div aria-hidden style={{ height: tailSpace }} className="shrink-0" />}
          </div>
        </div>
      ) : attemptsView && activeDataset && activeText ? (
        // The page-level error is where the toolbar's Assess again reports a
        // failure; the panel owns its own row-level errors.
        <>
          {error && (
            <div className="mx-4 mt-4 p-3 bg-error-bg text-error-text rounded-md text-sm break-words">
              {error}
            </div>
          )}
          <AttemptsPanel
            datasetUuid={activeDataset.uuid}
            text={activeText}
            onChanged={() => loadTexts(selectedDataset)}
            selectedUuid={selectedAttemptUuid}
            onSelectUuid={setSelectedAttemptUuid}
            onSelectAttempt={handleSelectAttempt}
            refreshKey={attemptsRefreshKey}
            compare={compareView}
            standardAudioAvailable={hasUsableClip(activeTextClips)}
          />
        </>
      ) : viewMode === "practice" && activeDataset && activeText ? (
        <PracticePanel
          datasetUuid={activeDataset.uuid}
          text={activeText}
          standardAudio={activeTextClips}
          fontCss={raFontOption(raFont).css}
          spaceCss={raSpaceOption(raSpace).css}
          onScored={() => loadTexts(selectedDataset)}
          onRecorder={handleRecorder}
        />
      ) : viewMode === "advanced" ? (
        // A page of its own, and a workspace rather than a fold: the pipeline
        // runs across the top and the three zones below it scroll separately.
        <div className="flex-1 min-h-0 min-w-0 flex flex-col">
          {selectedDataset && (
            <SourceAlignWorkspace
              datasetUuid={selectedDataset}
              onApplied={() => {
                void loadStandardClips(selectedDataset);
                void loadTexts(selectedDataset);
                // The apply may have given the open text a span clip, which only
                // gets its wav when this text's own listing is read.
                setTextClipsKey((k) => k + 1);
              }}
            />
          )}
        </div>
      ) : viewMode === "texts" ? (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-1">
          {texts.length === 0 ? (
            <p className="text-text-secondary">
              No texts yet. Add one with the New Text button.
            </p>
          ) : (
            texts.map((t) => (
              <div
                key={t.uuid}
                className="flex items-center gap-2 px-3 py-2 rounded-md text-left transition-colors hover:bg-bg-hover cursor-pointer"
                onClick={() => setSelectedText(t.uuid)}
              >
                <FileText size={16} className="shrink-0 text-text-tertiary" />
                <div className="flex flex-col min-w-0 flex-1">
                  <span className="text-sm text-text-primary truncate flex items-center gap-1">
                    {t.title || "Untitled"}
                    {t.normalized_content && (
                      <Wand2
                        size={12}
                        className="shrink-0 text-accent-bg"
                        aria-label="Normalized reference set (used for scoring)"
                      />
                    )}
                    {hasUsableClip(clipsByText.get(t.uuid)) && (
                      <AudioLines
                        size={12}
                        className="shrink-0 text-text-tertiary"
                        aria-label="Standard audio set (pronunciation can be scored)"
                      />
                    )}
                  </span>
                  {t.note && (
                    <span className="text-[11px] text-text-tertiary truncate">{t.note}</span>
                  )}
                </div>
                <span className={scoreBadgeClasses(t.best_score)} title="Best score">
                  {t.best_score != null ? `${Math.round(t.best_score)}%` : "—"}
                </span>
                <span className="text-[10px] text-text-tertiary tabular-nums shrink-0" title="Attempts">
                  ×{t.attempt_count}
                </span>
              </div>
            ))
          )}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto min-h-0 min-w-0 p-4 flex flex-col gap-2">
          {datasets.length === 0 ? (
            <p className="text-text-secondary">
              No read-aloud datasets yet. Create one with the New Dataset button.
            </p>
          ) : (
            datasets.map((d) => (
              <DatasetCard
                key={d.uuid}
                variant="row"
                icon={Mic}
                name={d.name}
                description={d.description}
                language={d.language || undefined}
                title={d.path}
                onOpen={() => {
                  setSelectedDataset(d.uuid);
                  setSelectedText(null);
                }}
                onSave={async (patch) => {
                  await invoke("dataset_update", { uuid: d.uuid, ...patch });
                  await emit("dataset-list-changed", {});
                  await loadDatasets();
                }}
              />
            ))
          )}
        </div>
      )}

      {/* Live recognition — docked under the practice pane while a take is being
          recorded. The transcript arrives as the reader speaks, so a silent
          microphone or a model in the wrong language shows up while there is
          still time to start over. It steps aside as soon as the take ends
          (`busy`) and the result takes over. */}
      {viewMode === "practice" && !attemptsView && activeText && rec.recording && !rec.busy && (
        <LiveTranscript
          text={rec.liveText}
          busy={rec.liveBusy}
          seconds={rec.liveSeconds}
        />
      )}

      {datasetDialog && (
        <DatasetDialog
          onClose={() => setDatasetDialog(false)}
          onSaved={(uuid) => loadDatasets(uuid)}
        />
      )}

      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </main>
  );
}
