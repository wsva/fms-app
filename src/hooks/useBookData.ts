/**
 * useBookData — encapsulates all shared navigation state plus the "Books"
 * data loading, state management and business logic for the books page.
 *
 * Mirrors `useDictationData`: the hook owns everything, `BooksPage` renders
 * the breadcrumb + toolbar chrome off it, and `ReadingView` becomes a purely
 * presentational drill-down (books list -> chapter list -> reading pane).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useImmer } from "use-immer";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  BookMeta,
  BookChapter,
  BookSentence,
  AudioWriteResult,
  SentenceClient,
  Paragraph,
  DrawerState,
  ArtifactKind,
  ArtifactScopeType,
  ArtifactJob,
  BookArtifactView,
  BookTerm,
  QuizQuestion,
  QuizResultResponse,
} from "@/lib/read/types";
import { flattenChapters, groupIntoParagraphs, toDbSentence } from "@/lib/read/types";
import { groupArtifactsByScope } from "@/lib/read/artifacts";
import {
  DEFAULT_UNDERSTANDING_TAB,
  UNDERSTANDING_TABS,
  isUnderstandingTab,
  type UnderstandingTab,
} from "@/components/books/understanding_tabs";
import { parseLineRows, matchRows } from "@/components/books/bookLines";
import type { CardDatasetSummary } from "@/lib/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { useRecorder } from "@/components/books/useRecorder";
import { playUrl } from "@/hooks/useMediaPlay";
import { getUUID, nowIso } from "@/components/books/utils";

export type BookView = "read" | "manage" | "advanced";

/** How often an open agent request is reread from the ledger. The dock's open
 *  list uses 8 s; this panel is one step removed from the work (it reports the
 *  state, it does not work the queue), so it can afford to be a beat slower. */
const OPEN_JOBS_POLL_MS = 12_000;

export function useBookData() {
  // ── Shared navigation ──────────────────────────────────────────
  const [books, setBooks] = useState<BookMeta[]>([]);
  const [view, setView] = useState<BookView>("read");
  const [bookUUID, setBookUUID] = useState("");
  const [chapterUUID, setChapterUUID] = useState("");
  const [mobile, setMobile] = useState(false);

  const loadBooks = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const data = await invoke<BookMeta[]>("book_list");
      setBooks(data);
    } catch (e) {
      console.error("Failed to load books:", e);
    }
  }, []);

  useEffect(() => {
    loadBooks();
  }, [loadBooks]);

  // Deferred platform flag (SSR-safe): keeps the first client paint consistent
  // with the prerendered desktop HTML.
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // ── Read domain state ──────────────────────────────────────────
  const [chaptersFlat, setChaptersFlat] = useState<BookChapter[]>([]);
  const [data, updateData] = useImmer<SentenceClient[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [needSave, setNeedSave] = useState(false);
  const [viewMode, setViewMode] = useState<"line" | "inline">("line");
  const [deleteTarget, setDeleteTarget] = useState<Paragraph | null>(null);
  const [confirmDeleteSentence, setConfirmDeleteSentence] = useState(false);
  const [error, setError] = useState("");

  // drawer
  const [drawer, setDrawer] = useState<DrawerState>(null);
  const [drawerUUID, setDrawerUUID] = useState("");
  const [drawerContent, setDrawerContent] = useState("");
  const [drawerBgColor, setDrawerBgColor] = useState<string | null>(null);

  // Latest-value mirrors for callbacks that must not be re-created per render
  // (the recorder's result sink fires from an async transcription path).
  const drawerRef = useRef(drawer);
  drawerRef.current = drawer;
  const drawerContentRef = useRef(drawerContent);
  drawerContentRef.current = drawerContent;
  const dataRef = useRef(data);
  dataRef.current = data;
  // The write body, mirrored for the sink before its definition site below.
  const saveSentenceInnerRef = useRef<(
    sentence: SentenceClient,
    newContent: string,
    bgColor: string | null
  ) => Promise<void>>(async () => {});

  // The sentence the reading pane renders as contentEditable on desktop —
  // inline editing is what retired the drawer from the PC edit surface.
  const [editingSentence, setEditingSentence] = useState<string | null>(null);

  const flatChapters = useMemo(() => flattenChapters(chaptersFlat), [chaptersFlat]);
  const paragraphs = useMemo(() => groupIntoParagraphs(data), [data]);

  // ── Understanding layer ────────────────────────────────────────
  // Split-mode toggle, the focused paragraph (its break-row uuid), the mobile
  // drawer open flag, the loaded artifacts/glossary, and the persisted split.
  // The active tab and the scope (paragraph vs chapter) live here too: the
  // understanding sub-toolbar and the panel body are separate components and
  // must read one state, so it cannot sit in either of them.
  // Understanding opens by default on desktop (the left-right split); the
  // preference persists under its own key so the e-reader style full-width
  // pane stays one click away. `mobile` is false during SSR/first paint, so
  // phones (and the prerendered HTML) start plain.
  const [understanding, setUnderstanding] = useState<boolean>(() => {
    if (typeof window === "undefined") return false;
    return window.localStorage.getItem("book-understanding-default") !== "0";
  });
  const [focusParagraph, setFocusParagraphState] = useState<string | null>(null);
  // The focused sentence, if any: it deepens the paragraph scope down to a
  // sentence for the panel. Click/keyboard-driven only — never the scroll-spy
  // — so generated sentence artifacts don't shift under the reader.
  const [focusSentence, setFocusSentenceState] = useState<string | null>(null);
  // Which level the tabs bind to while chapter mode is off. focusSentence and
  // focusParagraph are positions that stay warm across level switches, so
  // returning to a level (Alt+S, the toolbar segment, leaving chapter mode)
  // lands back on the last sentence/paragraph, not on a cleared slate.
  const [sentenceLevel, setSentenceLevel] = useState(false);
  // Kept in step with every sentence-focus write (applySentenceFocus is the
  // only other road), so the recorder's sink can read it without a re-bind.
  const focusSentenceRef = useRef<string | null>(null);
  const setFocusSentenceBoth = useCallback((uuid: string | null) => {
    focusSentenceRef.current = uuid;
    setFocusSentenceState(uuid);
  }, []);
  const [understandingTab, setUnderstandingTabState] = useState<UnderstandingTab>(() => {
    if (typeof window === "undefined") return DEFAULT_UNDERSTANDING_TAB;
    const stored = window.localStorage.getItem("book-understanding-tab");
    return isUnderstandingTab(stored) ? stored : DEFAULT_UNDERSTANDING_TAB;
  });
  const setUnderstandingTab = useCallback((tab: UnderstandingTab) => {
    setUnderstandingTabState(tab);
    if (typeof window !== "undefined") window.localStorage.setItem("book-understanding-tab", tab);
  }, []);
  // Alt+D walks the tab strip forward (digest -> explain -> ... -> glossary),
  // the same order the toolbar renders, persisting the pick like a click.
  const cycleTab = useCallback(() => {
    const i = UNDERSTANDING_TABS.findIndex((t) => t.id === understandingTab);
    const next = UNDERSTANDING_TABS[(i + 1) % UNDERSTANDING_TABS.length];
    setUnderstandingTab(next ? next.id : DEFAULT_UNDERSTANDING_TAB);
  }, [understandingTab, setUnderstandingTab]);
  // Chapter scope: when on, every content tab binds to the whole chapter
  // instead of the focused paragraph. Persisted like the other panel prefs.
  const [chapterMode, setChapterModeState] = useState<boolean>(() => {
    if (typeof window === "undefined") return false;
    return window.localStorage.getItem("book-understanding-chapter-mode") === "1";
  });
  const setChapterMode = useCallback((on: boolean) => {
    setChapterModeState(on);
    // Chapter mode overrides the scope but keeps the sentence/paragraph
    // positions, so leaving it returns exactly where the reader left off.
    if (typeof window !== "undefined")
      window.localStorage.setItem("book-understanding-chapter-mode", on ? "1" : "0");
  }, []);
  const [mobilePanelOpen, setMobilePanelOpen] = useState(false);
  const [artifacts, setArtifacts] = useState<BookArtifactView[]>([]);
  // Outstanding agent-generation requests for this book (the mailbox the "Ask
  // agent" button writes to). Empty unless a delegation is in flight.
  const [jobs, setJobs] = useState<ArtifactJob[]>([]);
  const [terms, setTerms] = useState<BookTerm[]>([]);
  const [generating, setGenerating] = useState(false);
  // LLM model used for generation. Empty means "fall back to the app default
  // configured on the Settings page" (the command resolves it server-side).
  // Persisted in localStorage under the shared `llm_selected_model` key, the
  // same one the Chat and OCR pages use.
  const [understandingModel, setUnderstandingModelState] = useState<string>(() => {
    if (typeof window === "undefined") return "";
    return window.localStorage.getItem("llm_selected_model") || "";
  });
  const setUnderstandingModel = useCallback((model: string) => {
    setUnderstandingModelState(model);
    if (typeof window === "undefined") return;
    if (model) window.localStorage.setItem("llm_selected_model", model);
    else window.localStorage.removeItem("llm_selected_model");
  }, []);
  const [splitRatio, setSplitRatioState] = useState<number>(() => {
    if (typeof window === "undefined") return 58;
    const raw = window.localStorage.getItem("book-understanding-split");
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n) && n >= 20 && n <= 80) return n;
    // No stored preference: give the panel more room on narrower windows.
    return window.innerWidth < 1400 ? 50 : 58;
  });

  const artifactsByScope = useMemo(() => groupArtifactsByScope(artifacts), [artifacts]);
  const chapterArtifacts = useMemo(
    () => artifacts.filter((a) => a.scope_uuid === chapterUUID),
    [artifacts, chapterUUID]
  );
  // The scope every content tab binds to: the whole chapter in chapter mode,
  // the focused sentence while the panel sits on the sentence level, the
  // focused paragraph otherwise. `uuid` is "" while nothing is focused — the
  // panel shows its hint until the learner clicks a sentence or paragraph.
  const scope = useMemo<{ type: ArtifactScopeType; uuid: string }>(
    () =>
      chapterMode
        ? { type: "chapter", uuid: chapterUUID }
        : sentenceLevel && focusSentence
          ? { type: "sentence", uuid: focusSentence }
          : { type: "paragraph", uuid: focusParagraph ?? "" },
    [chapterMode, chapterUUID, sentenceLevel, focusSentence, focusParagraph]
  );

  const loadArtifacts = useCallback(async () => {
    if (!bookUUID || !chapterUUID || !isTauri()) {
      setArtifacts([]);
      return;
    }
    try {
      const rows = await invoke<BookArtifactView[]>("book_list_artifacts", {
        bookUuid: bookUUID,
        chapterUuid: chapterUUID,
      });
      setArtifacts(rows);
    } catch (e) {
      console.error("Failed to load artifacts:", e);
    }
  }, [bookUUID, chapterUUID]);

  // Outstanding agent jobs are book-scoped in the DB; read them for the open book.
  const loadJobs = useCallback(async () => {
    if (!bookUUID || !isTauri()) {
      setJobs([]);
      return;
    }
    try {
      setJobs(await invoke<ArtifactJob[]>("book_list_artifact_jobs", { bookUuid: bookUUID }));
    } catch (e) {
      console.error("Failed to load artifact jobs:", e);
    }
  }, [bookUUID]);

  // A claim, a lapsed lease or a question are ledger facts, not artifact facts,
  // so none of them fires `book-artifact-changed`. The panel tells the learner
  // who holds the request and how long is left, which is only true if the row
  // is reread while it is open. Delivery itself still arrives through the
  // event, exactly once — this poll only keeps the state line honest.
  const hasOpenJobs = jobs.length > 0;
  useEffect(() => {
    if (!hasOpenJobs) return;
    const id = setInterval(() => void loadJobs(), OPEN_JOBS_POLL_MS);
    return () => clearInterval(id);
  }, [hasOpenJobs, loadJobs]);

  const loadTerms = useCallback(async () => {
    if (!bookUUID || !isTauri()) {
      setTerms([]);
      return;
    }
    try {
      setTerms(await invoke<BookTerm[]>("book_list_terms", { bookUuid: bookUUID }));
    } catch (e) {
      console.error("Failed to load glossary:", e);
    }
  }, [bookUUID]);

  // Load artifacts when a chapter opens and drop the previous focus.
  useEffect(() => {
    setFocusParagraphState(null);
    setFocusSentenceBoth(null);
    setSentenceLevel(false);
    setEditingSentence(null);
    loadArtifacts();
    loadJobs();
  }, [chapterUUID, loadArtifacts, loadJobs, setFocusSentenceBoth]);

  // Load the glossary when a book opens.
  useEffect(() => {
    loadTerms();
  }, [bookUUID, loadTerms]);

  // Editing/adding/deleting sentences can flip artifacts stale (their source
  // hash no longer matches). While the panel is open, re-read them so the stale
  // badges stay honest. Idempotent; never writes `data`, so it cannot loop.
  useEffect(() => {
    if (understanding) loadArtifacts();
  }, [data, understanding, loadArtifacts]);

  // An attached agent delivers results (and clears the matching job) through
  // book_save_artifact, which emits `book-artifact-changed`. Reload both so a
  // delegated explanation shows up in the panel with no user action.
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<{ book_uuid?: string }>("book-artifact-changed", (e) => {
      if (e.payload?.book_uuid && e.payload.book_uuid !== bookUUID) return;
      loadArtifacts();
      loadJobs();
      // A delivered `terms` artifact filled the glossary rows beside it.
      loadTerms();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [bookUUID, loadArtifacts, loadJobs, loadTerms]);

  // ── Load chapters when book changes ────────────────────────────
  useEffect(() => {
    setChapterUUID("");
    updateData((d) => {
      d.length = 0;
    });
    setNeedSave(false);
    if (!bookUUID || !isTauri()) {
      setChaptersFlat([]);
      return;
    }
    invoke<BookChapter[]>("book_list_chapters", { bookUuid: bookUUID })
      .then(setChaptersFlat)
      .catch(() => setChaptersFlat([]));
  }, [bookUUID, updateData]);

  // ── Load sentences when chapter changes ────────────────────────
  useEffect(() => {
    if (!chapterUUID || !isTauri()) return;
    const load = async () => {
      setLoading(true);
      try {
        const rows = await invoke<BookSentence[]>("book_list_sentences", {
          bookUuid: bookUUID,
          chapterUuid: chapterUUID,
        });
        updateData((d) => {
          d.length = 0;
          rows.forEach((s) => d.push({ ...s, modified: false, hasLocalAudio: false }));
        });
      } catch (e) {
        setError(String(e));
      }
      setLoading(false);
      setNeedSave(false);
    };
    load();
  }, [chapterUUID, bookUUID, updateData]);

  // ── Voice input ────────────────────────────────────────────────
  // A take is an input method, not an artifact: nothing is written to the book's
  // media dir and the transcript is not stored on the sentence. What the model
  // heard is appended to the content field, where the reader edits it before
  // Save — the same way the chat composer folds a transcript into its input.
  const handleRecordResult = useCallback(
    // Only the transcript is of interest now — the WAV is dropped, not stored.
    ({ text }: { text: string }) => {
      if (!text.trim()) {
        setError("No speech recognized — speak a little closer and try again.");
        return;
      }
      // With the drawer open (the mobile road) the transcript still lands in
      // its content field; on desktop it appends into the focused sentence,
      // the same way the chat composer folds a transcript into its input.
      if (drawerRef.current) {
        const cur = drawerContentRef.current;
        setDrawerContent(cur.trim() ? `${cur.trimEnd()} ${text.trim()}` : text.trim());
        return;
      }
      const uuid = focusSentenceRef.current;
      if (!uuid) {
        setError("Select a sentence to dictate into.");
        return;
      }
      const s = dataRef.current.find((x) => x.uuid === uuid);
      if (!s) return;
      void saveSentenceInnerRef.current(s, `${s.content.trimEnd()} ${text.trim()}`.trim(), s.bg_color);
    },
    // saveSentenceInner/`data` are read through refs (defined below) so this
    // sink stays identity-stable for useRecorder.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );

  const recorder = useRecorder(handleRecordResult, (msg) => setError(msg));

  // ── Drawer helpers ─────────────────────────────────────────────
  const openEditDrawer = (sentence: SentenceClient) => {
    setDrawer({ mode: "edit", sentence });
    setDrawerUUID(sentence.uuid);
    setDrawerContent(sentence.content ?? "");
    setDrawerBgColor(sentence.bg_color ?? null);
  };

  const openInsertDrawer = (insertBeforeUUID: string | null) => {
    setDrawer({ mode: "add", insertBeforeUUID });
    setDrawerUUID(getUUID());
    setDrawerContent("");
    setDrawerBgColor(null);
  };

  const openAddDrawer = (para: Paragraph) => openInsertDrawer(para.breakSentence?.uuid ?? null);

  const closeDrawer = () => {
    setDrawer(null);
    setDrawerContent("");
    setDrawerBgColor(null);
  };

  const clearDrawer = () => {
    if (drawer?.mode !== "add") return;
    setDrawerUUID(getUUID());
    setDrawerContent("");
    setDrawerBgColor(null);
  };

  const discardDrawer = () => {
    if (drawer?.mode === "edit") {
      // Revert to original sentence values
      const s = drawer.sentence;
      setDrawerContent(s.content ?? "");
      setDrawerBgColor(s.bg_color ?? null);
    } else if (drawer?.mode === "add") {
      clearDrawer();
    }
  };

  // ── Renumber helper (immer freeze-safe) ────────────────────────
  const renumber = (d: SentenceClient[], markModified: boolean) => {
    for (let i = 0; i < d.length; i++) {
      const s = d[i];
      if (s.order_num !== i + 1) d[i] = { ...s, order_num: i + 1, modified: markModified };
    }
  };

  // ── Save add ───────────────────────────────────────────────────
  const handleSaveAdd = async () => {
    if (!drawerContent.trim()) {
      setError("Content is empty");
      return;
    }
    if (drawer?.mode !== "add" || !isTauri()) return;
    setSaving(true);
    setError("");
    try {
      const insertIndex =
        drawer.insertBeforeUUID === null
          ? data.length
          : data.findIndex((s) => s.uuid === drawer.insertBeforeUUID);

      const now = nowIso();
      const newSentence: SentenceClient = {
        uuid: drawerUUID,
        chapter_uuid: chapterUUID,
        user_id: "",
        order_num: insertIndex + 1,
        content: drawerContent,
        sentence_type: "text",
        // A sentence the drawer creates has no audio: audio belongs to the
        // paragraph (uploaded or synthesized), not to a single sentence.
        audio_path: null,
        audio_url: null,
        // Voice input is not stored as a transcript any more, so a new sentence
        // has none; the column stays for rows written before.
        recognized: null,
        bg_color: drawerBgColor,
        created_at: now,
        updated_at: now,
        modified: false,
        hasLocalAudio: false,
      };

      await invoke("book_save_sentence", { bookUuid: bookUUID, sentence: toDbSentence(newSentence) });

      // Award XP for typing a sentence.
      if (isTauri()) {
        invoke("xp_award_reading_sentence", { sentenceId: newSentence.uuid }).catch(() => {});
      }

      // Persist reordering of existing sentences shifted by the insert.
      if (insertIndex < data.length) {
        const shifted = data
          .slice(insertIndex)
          .map((s, i) => toDbSentence({ ...s, order_num: insertIndex + 2 + i }));
        await invoke("book_save_sentences", { bookUuid: bookUUID, sentences: shifted });
      }

      updateData((d) => {
        d.splice(insertIndex, 0, newSentence);
        for (let i = 0; i < d.length; i++) {
          const s = d[i];
          d[i] = { ...s, order_num: i + 1, modified: false };
        }
      });

      setDrawer(null);
      setDrawerContent("");
      setDrawerBgColor(null);
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Save edit ──────────────────────────────────────────────────
  // The write body behind every sentence edit: the drawer's Save button and
  // the desktop inline commit both run it, so XP, ordering and the stale
  // artifact refresh stay on one road. Whatever audio and transcript the
  // sentence already carried ride the spread untouched.
  const saveSentenceInner = async (
    sentence: SentenceClient,
    newContent: string,
    bgColor: string | null
  ) => {
    if (!isTauri()) return;
    setSaving(true);
    setError("");
    try {
      const updated: SentenceClient = {
        ...sentence,
        content: newContent,
        bg_color: bgColor,
        updated_at: nowIso(),
        hasLocalAudio: false,
        modified: sentence.modified,
      };
      await invoke("book_save_sentence", { bookUuid: bookUUID, sentence: toDbSentence(updated) });

      // Award XP for editing a sentence (first time only, backend deduplicates).
      if (isTauri()) {
        invoke("xp_award_reading_sentence", { sentenceId: updated.uuid }).catch(() => {});
      }

      updateData((d) => {
        const idx = d.findIndex((s) => s.uuid === sentence.uuid);
        if (idx !== -1) d[idx] = updated;
      });
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };
  saveSentenceInnerRef.current = saveSentenceInner;

  const handleSaveEdit = async () => {
    if (drawer?.mode !== "edit" || !isTauri()) return;
    await saveSentenceInner(drawer.sentence, drawerContent, drawerBgColor);
    setDrawer(null);
    setDrawerContent("");
    setDrawerBgColor(null);
  };

  // The inline commit from the reading pane's contentEditable. The drawer path
  // closes over its own fields, so the write body takes them explicitly.
  const handleCommitSentenceEdit = useCallback(
    async (uuid: string, content: string, bgColor?: string | null) => {
      const s = data.find((x) => x.uuid === uuid);
      if (!s) return;
      if (!content.trim()) {
        setError("Content is empty");
        return;
      }
      await saveSentenceInner(s, content, bgColor === undefined ? s.bg_color : bgColor);
    },
    [data, saveSentenceInner]
  );

  // ── Delete single sentence (from drawer) ───────────────────────
  const handleDelete = () => {
    if (drawer?.mode !== "edit" && !focusSentence) return;
    setConfirmDeleteSentence(true);
  };

  const doDeleteSentence = async () => {
    const sentence =
      drawer?.mode === "edit"
        ? drawer.sentence
        : data.find((s) => s.uuid === focusSentence) ?? null;
    if (!sentence || !isTauri()) return;
    setSaving(true);
    setError("");
    try {
      await invoke("book_delete_sentence", { bookUuid: bookUUID, uuid: sentence.uuid });
      updateData((d) => {
        const idx = d.findIndex((s) => s.uuid === sentence.uuid);
        if (idx !== -1) {
          d.splice(idx, 1);
          renumber(d, true);
        }
      });
      setNeedSave(true);
      setDrawer(null);
      if (focusSentence === sentence.uuid) applySentenceFocus(null);
    } catch (e) {
      setError(`Delete failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Save order ─────────────────────────────────────────────────
  const handleSaveOrder = async () => {
    if (!isTauri()) return;
    setSaving(true);
    setError("");
    try {
      const modified = data.filter((s) => s.modified).map(toDbSentence);
      await invoke("book_save_sentences", { bookUuid: bookUUID, sentences: modified });
      updateData((d) => {
        d.forEach((s) => {
          s.modified = false;
        });
      });
      setNeedSave(false);
    } catch (e) {
      setError(`Save order failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Insert paragraph break ─────────────────────────────────────
  const insertParagraph = async (insertBeforeUUID: string | null) => {
    if (!chapterUUID || !isTauri()) return;
    setSaving(true);
    setError("");
    const insertIndex =
      insertBeforeUUID === null ? data.length : data.findIndex((s) => s.uuid === insertBeforeUUID);
    const now = nowIso();
    const breakSentence: SentenceClient = {
      uuid: getUUID(),
      chapter_uuid: chapterUUID,
      user_id: "",
      order_num: insertIndex + 1,
      content: "",
      sentence_type: "paragraph_break",
      audio_path: null,
      audio_url: null,
      recognized: null,
      bg_color: null,
      created_at: now,
      updated_at: now,
      modified: false,
      hasLocalAudio: false,
    };
    try {
      await invoke("book_save_sentence", {
        bookUuid: bookUUID,
        sentence: toDbSentence(breakSentence),
      });
      updateData((d) => {
        d.splice(insertIndex, 0, breakSentence);
        renumber(d, true);
      });
      setNeedSave(true);
      setDrawer(null);
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  const handleNewParagraph = () => insertParagraph(null);

  // ── Paragraph reflow (the panel's Edit tab) ──────────────────────
  // Rewrite one paragraph's text: lines re-segment its rows, a line equal to
  // an existing sentence keeps that row (uuid, audio, color, sentence-scoped
  // artifacts), unmatched rows are deleted through book_delete_sentence (which
  // cascades their artifacts), and an empty line splits the paragraph in two —
  // the old break row moves to the split so the first half keeps its anchors,
  // a fresh break terminates the second. Rows are written immediately (the
  // Import road), so no "Save Order" flush is needed.
  const handleParagraphReflow = async (paraIndex: number, text: string) => {
    if (!chapterUUID || !isTauri()) return;
    const para = paragraphs[paraIndex];
    if (!para) return;
    const desired = parseLineRows(text.split("\n"));
    const { rows: matched, removed } = matchRows(
      desired,
      [
        ...para.sentences.map((s) => ({ uuid: s.uuid, content: s.content, isBreak: false })),
        ...(para.breakSentence
          ? [{ uuid: para.breakSentence.uuid, content: "", isBreak: true }]
          : []),
      ]
    );
    if (removed.length > 0 && !reflowAllowDelete.current) {
      setError("Removing sentences also removes their generated artifacts — confirm to proceed.");
      return;
    }
    reflowAllowDelete.current = false;
    setSaving(true);
    setError("");
    const now = nowIso();
    const oldEnd = para.breakSentence
      ? data.findIndex((s) => s.uuid === para.breakSentence!.uuid)
      : data.length - 1;
    if (oldEnd < 0) {
      setSaving(false);
      return;
    }
    // The paragraph's first row follows the previous break (the last
    // paragraph_break before this one), or starts the chapter.
    const prevBreak =
      data.slice(0, oldEnd).map((s) => s.sentence_type).lastIndexOf("paragraph_break");
    const oldStart = prevBreak + 1;
    // Build the replacement rows; the old break (if the chapter has one) is
    // reused as the terminating break, so a split never orphans its audio.
    const rows: SentenceClient[] = [];
    let oldBreakUsed = false;
    for (let i = 0; i < matched.length; i++) {
      const m = matched[i];
      const isTermBreak = i === matched.length - 1 && m.isBreak;
      const uuid =
        isTermBreak && para.breakSentence && !oldBreakUsed
          ? para.breakSentence.uuid
          : m.uuid;
      if (uuid === para.breakSentence?.uuid) oldBreakUsed = true;
      const old = m.reused ? data.find((s) => s.uuid === uuid) ?? null : null;
      if (m.isBreak) {
        rows.push(
          old
            ? { ...old, order_num: 0, updated_at: nowIso(), modified: false }
            : {
                uuid,
                chapter_uuid: chapterUUID,
                user_id: "",
                order_num: 0,
                content: "",
                sentence_type: "paragraph_break",
                audio_path: null,
                audio_url: null,
                recognized: null,
                bg_color: null,
                created_at: now,
                updated_at: now,
                modified: false,
                hasLocalAudio: false,
              }
        );
      } else {
        rows.push({
          uuid,
          chapter_uuid: chapterUUID,
          user_id: "",
          order_num: 0,
          content: m.text,
          sentence_type: "text",
          audio_path: old?.audio_path ?? null,
          audio_url: old?.audio_url ?? null,
          recognized: old?.recognized ?? null,
          bg_color: old?.bg_color ?? null,
          created_at: old?.created_at ?? now,
          updated_at: old && old.content.trim() === m.text ? old.updated_at : now,
          modified: false,
          hasLocalAudio: false,
        });
      }
    }
    try {
      for (const uuid of removed) {
        await invoke("book_delete_sentence", { bookUuid: bookUUID, uuid });
      }
      // Sequential order_num at write time, mirroring handleImport.
      const ordered = rows.map((s, i) => ({ ...s, order_num: oldStart + 1 + i }));
      await invoke("book_save_sentences", {
        bookUuid: bookUUID,
        sentences: ordered.map(toDbSentence),
      });
      const tailStart = oldStart + ordered.length;
      if (tailStart < data.length) {
        const shifted = data
          .slice(oldEnd + 1)
          .map((s, j) => toDbSentence({ ...s, order_num: tailStart + 1 + j }));
        await invoke("book_save_sentences", { bookUuid: bookUUID, sentences: shifted });
      }
      updateData((d) => {
        d.splice(oldStart, oldEnd - oldStart + 1, ...ordered);
        for (let i = 0; i < d.length; i++) {
          d[i] = { ...d[i], order_num: i + 1, modified: false };
        }
      });
      // Sentence focus belonged to the rewritten text; drop it when its row
      // is gone or the paragraph was split in two.
      const fs = focusSentenceRef.current;
      if (fs && !ordered.some((s) => s.uuid === fs)) applySentenceFocus(null);
      else if (ordered.some((s) => s.sentence_type === "paragraph_break") && para.breakSentence) {
        setFocusParagraph(para.breakSentence.uuid);
      }
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  // One-pass confirmation for a reflow that deletes sentences (their artifacts
  // cascade away with them); the Edit tab arms it after removedArtifactCount
  // has explained what is at stake.
  const reflowAllowDelete = useRef(false);
  const allowReflowDelete = useCallback(() => {
    reflowAllowDelete.current = true;
  }, []);

  // How many generated artifacts hang off sentences a reflow is about to
  // remove — the Edit tab's one-line confirm reads this before arming.
  const removedArtifactCount = useCallback(
    (removedUuids: string[]) => {
      if (removedUuids.length === 0) return 0;
      let n = 0;
      for (const u of removedUuids) n += artifactsByScope.get(u)?.length ?? 0;
      return n;
    },
    [artifactsByScope]
  );

  // ── Import sentences from text (each line = sentence, empty line = paragraph break) ─
  const handleImport = async (para: Paragraph, text: string) => {
    if (!text.trim() || !chapterUUID || !isTauri()) return;
    setSaving(true);
    setError("");

    const now = nowIso();
    // The shared line grammar (one line = sentence, empty line = break);
    // every imported row is new, so reflow's match-by-text keeps nothing.
    const toSave: SentenceClient[] = parseLineRows(text.split("\n")).map((d) => ({
      uuid: getUUID(),
      chapter_uuid: chapterUUID,
      user_id: "",
      order_num: 0,
      content: d.text,
      sentence_type: d.isBreak ? "paragraph_break" : "text",
      audio_path: null,
      audio_url: null,
      recognized: null,
      bg_color: null,
      created_at: now,
      updated_at: now,
      modified: false,
      hasLocalAudio: false,
    }));

    if (toSave.length === 0) {
      setSaving(false);
      return;
    }

    // Find insertion point: after the last sentence of the target paragraph
    const insertIndex =
      para.sentences.length > 0
        ? data.findIndex((s) => s.uuid === para.sentences[para.sentences.length - 1].uuid) + 1
        : para.breakSentence
          ? data.findIndex((s) => s.uuid === para.breakSentence!.uuid)
          : data.length;

    try {
      // Assign sequential 1-based order_num at DB-write time (mirrors
      // handleSaveAdd) so imported rows persist in the correct position even
      // if the chapter is reloaded before a "Save Order" flush.
      const ordered: SentenceClient[] = toSave.map((s, i) => ({
        ...s,
        order_num: insertIndex + 1 + i,
      }));
      await invoke("book_save_sentences", {
        bookUuid: bookUUID,
        sentences: ordered.map(toDbSentence),
      });

      // Persist reordering of existing sentences shifted down by the insert.
      if (insertIndex < data.length) {
        const shifted = data
          .slice(insertIndex)
          .map((s, j) =>
            toDbSentence({ ...s, order_num: insertIndex + 1 + ordered.length + j }),
          );
        await invoke("book_save_sentences", { bookUuid: bookUUID, sentences: shifted });
      }

      updateData((d) => {
        d.splice(insertIndex, 0, ...ordered);
        for (let i = 0; i < d.length; i++) {
          const s = d[i];
          d[i] = { ...s, order_num: i + 1, modified: false };
        }
      });
    } catch (e) {
      setError(`Import failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Paragraph audio upload ─────────────────────────────────────
  const handleParagraphAudio = async (para: Paragraph) => {
    if (!chapterUUID || !isTauri()) return;
    const picked = await open({
      multiple: false,
      filters: [{ name: "Audio", extensions: ["mp3", "wav", "m4a", "ogg", "flac", "webm"] }],
    });
    if (!picked || Array.isArray(picked)) return;
    const sourcePath = picked as string;

    setSaving(true);
    setError("");
    try {
      let breakSentence = para.breakSentence;
      if (!breakSentence) {
        const now = nowIso();
        breakSentence = {
          uuid: getUUID(),
          chapter_uuid: chapterUUID,
          user_id: "",
          order_num: data.length + 1,
          content: "",
          sentence_type: "paragraph_break",
          audio_path: null,
          audio_url: null,
          recognized: null,
          bg_color: null,
          created_at: now,
          updated_at: now,
          modified: false,
          hasLocalAudio: false,
        };
        await invoke("book_save_sentence", {
          bookUuid: bookUUID,
          sentence: toDbSentence(breakSentence),
        });
        const created = breakSentence;
        updateData((d) => {
          d.push(created);
        });
      }

      const res = await invoke<AudioWriteResult>("book_import_audio", {
        bookUuid: bookUUID,
        name: `${breakSentence.uuid}.wav`,
        sourcePath,
      });

      const updated: SentenceClient = {
        ...breakSentence,
        audio_path: res.rel_path,
        audio_url: res.abs_path,
        updated_at: nowIso(),
      };
      await invoke("book_save_sentence", { bookUuid: bookUUID, sentence: toDbSentence(updated) });
      updateData((d) => {
        const idx = d.findIndex((s) => s.uuid === updated.uuid);
        if (idx !== -1) d[idx] = updated;
      });
    } catch (e) {
      setError(`Upload failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Delete paragraph ───────────────────────────────────────────
  const handleDeleteParagraph = (para: Paragraph) => {
    const toDelete = [...para.sentences, ...(para.breakSentence ? [para.breakSentence] : [])];
    if (toDelete.length === 0) return;
    setDeleteTarget(para);
  };

  const handleDeleteAll = async () => {
    if (!deleteTarget || !isTauri()) return;
    const para = deleteTarget;
    setDeleteTarget(null);
    const toDelete = [...para.sentences, ...(para.breakSentence ? [para.breakSentence] : [])];
    setSaving(true);
    setError("");
    try {
      for (const s of toDelete) {
        await invoke("book_delete_sentence", { bookUuid: bookUUID, uuid: s.uuid });
      }
      const deleted = new Set(toDelete.map((s) => s.uuid));
      updateData((d) => {
        for (let i = d.length - 1; i >= 0; i--) {
          if (deleted.has(d[i].uuid)) d.splice(i, 1);
        }
        renumber(d, true);
      });
      setNeedSave(true);
    } catch (e) {
      setError(`Delete failed: ${e}`);
    }
    setSaving(false);
  };

  const handleDeleteBreakOnly = async () => {
    const breakSentence = deleteTarget?.breakSentence;
    if (!breakSentence || !isTauri()) return;
    setDeleteTarget(null);
    setSaving(true);
    setError("");
    try {
      await invoke("book_delete_sentence", { bookUuid: bookUUID, uuid: breakSentence.uuid });
      updateData((d) => {
        const idx = d.findIndex((s) => s.uuid === breakSentence.uuid);
        if (idx !== -1) {
          d.splice(idx, 1);
          renumber(d, true);
        }
      });
      setNeedSave(true);
    } catch (e) {
      setError(`Delete failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Toggle chapter completed ───────────────────────────────────
  const handleToggleChapterStatus = async () => {
    const chapter = chaptersFlat.find((c) => c.uuid === chapterUUID);
    if (!chapter || !isTauri()) return;
    setSaving(true);
    const newStatus = chapter.status === "completed" ? null : "completed";
    const updated: BookChapter = { ...chapter, status: newStatus, updated_at: nowIso() };
    try {
      await invoke("book_save_chapter", { bookUuid: bookUUID, chapter: updated });
      setChaptersFlat((prev) => prev.map((c) => (c.uuid === chapterUUID ? updated : c)));
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Drawer derived props ───────────────────────────────────────
  const editSentence = drawer?.mode === "edit" ? drawer.sentence : null;
  const editIdx = editSentence ? data.findIndex((x) => x.uuid === editSentence.uuid) : -1;
  const nextUUID = editIdx !== -1 && editIdx + 1 < data.length ? data[editIdx + 1].uuid : null;

  // ── Breadcrumb derived ─────────────────────────────────────────
  const selectedBook = books.find((b) => b.uuid === bookUUID) ?? null;
  const currentChapter = chaptersFlat.find((c) => c.uuid === chapterUUID);

  // Reload the current view from disk without disturbing navigation: the book
  // list always, the open book's chapters, and — only when no reorder is
  // pending — the open chapter's sentences (so a refresh never discards unsaved
  // "Save Order" edits). Driven by the shared page refresh, never by a button.
  //
  // It is also *silent*: `loading` is the one thing that would make this refresh
  // destructive. ReadingView swaps the paragraph list for a one-line "Loading…"
  // while it is set, the reading pane's content collapses with it, and the
  // browser clamps the pane's scroll offset to 0 — so a learner who switched to
  // another tab and back would lose the line they were reading, every time. The
  // spinner therefore belongs to a pane with nothing on it yet and to nothing
  // else.
  const paneIsEmpty = data.length === 0;
  const refresh = useCallback(async () => {
    await loadBooks();
    if (!isTauri()) return;
    if (bookUUID) {
      try {
        const ch = await invoke<BookChapter[]>("book_list_chapters", { bookUuid: bookUUID });
        setChaptersFlat(ch);
      } catch {
        /* ignore — keep the chapters currently shown */
      }
    }
    if (bookUUID && chapterUUID && !needSave) {
      if (paneIsEmpty) setLoading(true);
      try {
        const rows = await invoke<BookSentence[]>("book_list_sentences", {
          bookUuid: bookUUID,
          chapterUuid: chapterUUID,
        });
        updateData((d) => {
          d.length = 0;
          rows.forEach((s) => d.push({ ...s, modified: false, hasLocalAudio: false }));
        });
      } catch (e) {
        setError(String(e));
      }
      if (paneIsEmpty) setLoading(false);
    }
  }, [loadBooks, bookUUID, chapterUUID, needSave, updateData, paneIsEmpty]);

  // ── Understanding: mode + focus ────────────────────────────────
  const setSplitRatio = useCallback((ratio: number) => {
    const clamped = Math.max(20, Math.min(80, Math.round(ratio)));
    setSplitRatioState(clamped);
    try {
      window.localStorage.setItem("book-understanding-split", String(clamped));
    } catch {
      /* ignore */
    }
  }, []);

  const applySentenceFocus = useCallback(
    (uuid: string | null) => {
      setFocusSentenceBoth(uuid);
      setSentenceLevel(uuid !== null);
      setEditingSentence(uuid);
      // Focusing a sentence is the sentence level: clicking (or walking to)
      // one while chapter mode is on drops back to it — the click is the
      // selection. Clearing focus leaves the level alone.
      if (uuid) setChapterMode(false);
    },
    [setFocusSentenceBoth, setChapterMode]
  );

  const setFocusParagraph = useCallback((uuid: string | null) => {
    // Sentence focus belongs to a paragraph; moving to another one drops it
    // (re-clicking the same paragraph keeps the selected sentence). Read the
    // current value from state rather than computing inside the updater.
    if (focusParagraph !== uuid) {
      setEditingSentence(null);
      setFocusParagraphState(uuid);
      setFocusSentenceBoth(null);
      setSentenceLevel(false);
    }
  }, [focusParagraph, setFocusSentenceBoth]);

  // The scroll spy's own road: it keeps the paragraph position warm while
  // the reader scrolls, but it must never touch the level or the remembered
  // sentence — those follow clicks and keys only. Identity-stable (state
  // updater, no closure deps) so the spy's observer does not re-bind on every
  // focus change and re-fire its initial intersection.
  const spyFocusParagraph = useCallback((uuid: string | null) => {
    setFocusParagraphState((prev) => (prev === uuid ? prev : uuid));
  }, []);

  // Tapping a paragraph focuses it and, on a phone, slides the panel in.
  const selectParagraph = useCallback(
    (uuid: string | null) => {
      setFocusParagraph(uuid);
      if (mobile && uuid) setMobilePanelOpen(true);
    },
    [mobile, setFocusParagraph]
  );

  // Land the tabs on the sentence level, then the paragraph level. Each one
  // leaves the other's position intact, so walking back with Alt+S or a
  // segment click returns to the last sentence/paragraph the reader had.
  const selectSentenceLevel = useCallback(() => {
    setChapterMode(false);
    setSentenceLevel(true);
    setEditingSentence(null);
  }, [setChapterMode]);
  const selectParagraphLevel = useCallback(() => {
    setChapterMode(false);
    setSentenceLevel(false);
    setEditingSentence(null);
  }, [setChapterMode]);

  // Alt+S walks the toolbar's scope segment forward: sentence -> paragraph ->
  // chapter -> sentence. Each level lands on its remembered position; only
  // the sentence step with nothing remembered falls back (like the keyboard
  // walk) to the focused paragraph's first sentence.
  const cycleScope = useCallback(() => {
    const current: "sentence" | "paragraph" | "chapter" = chapterMode
      ? "chapter"
      : sentenceLevel && focusSentence
        ? "sentence"
        : "paragraph";
    if (current === "sentence") {
      selectParagraphLevel();
      return;
    }
    if (current === "paragraph") {
      setChapterMode(true);
      return;
    }
    if (!focusSentence) {
      const first =
        paragraphs.find((p) => p.breakSentence?.uuid === focusParagraph)
          ?.sentences[0]?.uuid ?? null;
      if (first) setFocusSentenceBoth(first);
    }
    selectSentenceLevel();
  }, [
    chapterMode,
    sentenceLevel,
    focusSentence,
    focusParagraph,
    paragraphs,
    setChapterMode,
    setFocusSentenceBoth,
    selectParagraphLevel,
    selectSentenceLevel,
  ]);

  // Selecting a sentence deepens the panel's scope to it. On desktop inline
  // editing replaces the drawer; on a phone the drawer stays the edit surface
  // (with the panel following via the paragraph focus).
  const selectSentence = useCallback(
    (s: SentenceClient) => {
      if (mobile) {
        applySentenceFocus(null);
        openEditDrawer(s);
        return;
      }
      const owner = paragraphs.find((p) => p.sentences.some((x) => x.uuid === s.uuid));
      if (owner) setFocusParagraph(owner.breakSentence?.uuid ?? null);
      applySentenceFocus(s.uuid);
    },
    [mobile, paragraphs, setFocusParagraph, applySentenceFocus]
  );

  // The paragraph a sentence belongs to ("¶ N · sentence M" in the panel).
  const paraIndexOf = useCallback(
    (uuid: string | null): number | null => {
      if (!uuid) return null;
      const i = paragraphs.findIndex((p) => p.sentences.some((s) => s.uuid === uuid));
      return i === -1 ? null : i;
    },
    [paragraphs]
  );

  const toggleUnderstanding = useCallback(() => {
    setUnderstanding((prev) => {
      const next = !prev;
      try {
        window.localStorage.setItem("book-understanding-default", next ? "1" : "0");
      } catch {
        /* ignore */
      }
      if (next) {
        // Focus the first paragraph that has an anchor so the panel has a scope.
        setFocusParagraphState((cur) => {
          if (cur) return cur;
          const first = paragraphs.find((p) => p.breakSentence);
          return first?.breakSentence?.uuid ?? null;
        });
        if (mobile) setMobilePanelOpen(true);
      } else if (mobile) {
        setMobilePanelOpen(false);
      }
      return next;
    });
  }, [paragraphs, mobile]);

  // ── Understanding: generation + actions ────────────────────────
  // `instructions` is free-form guidance the backend appends to the prompt (the
  // Translate tab names its target language there); omit it for plain generate.
  const handleGenerate = useCallback(
    async (scopeType: ArtifactScopeType, scopeUuid: string, kind: ArtifactKind, instructions?: string) => {
      if (!isTauri() || !bookUUID) return;
      setGenerating(true);
      setError("");
      try {
        await invoke("book_generate_artifact", {
          bookUuid: bookUUID,
          scopeType,
          scopeUuid,
          kind,
          model: understandingModel.trim() ? understandingModel.trim() : undefined,
          instructions: instructions?.trim() ? instructions.trim() : undefined,
        });
        await loadArtifacts();
      } catch (e) {
        setError(`Generate failed: ${e}`);
      }
      setGenerating(false);
    },
    [bookUUID, loadArtifacts, understandingModel]
  );

  // Delegate generation to an attached third-party agent instead of the built-in
  // LLM: write a job to the mailbox. The agent picks it up over MCP and delivers
  // through book_save_artifact, which clears the job and fires the reload event.
  const handleRequestAgent = useCallback(
    async (scopeType: ArtifactScopeType, scopeUuid: string, kind: ArtifactKind, instructions?: string) => {
      if (!isTauri() || !bookUUID) return;
      setError("");
      try {
        await invoke("book_request_artifact", {
          bookUuid: bookUUID,
          scopeType,
          scopeUuid,
          kind,
          model: understandingModel.trim() ? understandingModel.trim() : undefined,
          instructions: instructions?.trim() ? instructions.trim() : undefined,
        });
        await loadJobs();
      } catch (e) {
        setError(`Ask agent failed: ${e}`);
      }
    },
    [bookUUID, loadJobs, understandingModel]
  );

  const handleCancelJob = useCallback(
    async (uuid: string) => {
      if (!isTauri() || !bookUUID) return;
      setError("");
      try {
        await invoke("book_cancel_artifact_job", { bookUuid: bookUUID, uuid });
        await loadJobs();
      } catch (e) {
        setError(`Cancel request failed: ${e}`);
      }
    },
    [bookUUID, loadJobs]
  );

  const handleParagraphListen = useCallback(
    async (paragraphUuid: string) => {
      if (!isTauri() || !bookUUID || !paragraphUuid) return;
      setSaving(true);
      setError("");
      try {
        const res = await invoke<AudioWriteResult>("book_paragraph_listen", {
          bookUuid: bookUUID,
          paragraphUuid,
        });
        updateData((d) => {
          const idx = d.findIndex((s) => s.uuid === paragraphUuid);
          if (idx !== -1)
            d[idx] = {
              ...d[idx],
              audio_path: res.rel_path,
              audio_url: res.abs_path,
              updated_at: nowIso(),
            };
        });
        // Through the shared one-at-a-time player, so a synthesised listen does
        // not talk over the paragraph clip that may already be sounding.
        playUrl(res.abs_path);
      } catch (e) {
        setError(`Listen failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID, updateData]
  );

  const handleSaveNote = useCallback(
    async (scopeType: ArtifactScopeType, scopeUuid: string, content: string) => {
      if (!isTauri() || !bookUUID || !content.trim()) return;
      setSaving(true);
      setError("");
      const now = nowIso();
      try {
        await invoke("book_save_artifact", {
          bookUuid: bookUUID,
          artifact: {
            uuid: "",
            book_uuid: bookUUID,
            scope_type: scopeType,
            scope_uuid: scopeUuid,
            kind: "note",
            content,
            source: "user",
            model: null,
            parent_uuid: null,
            content_hash: "",
            created_at: now,
            updated_at: now,
          },
        });
        await loadArtifacts();
      } catch (e) {
        setError(`Save note failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID, loadArtifacts]
  );

  const handleDeleteArtifact = useCallback(
    async (uuid: string) => {
      if (!isTauri() || !bookUUID) return;
      setSaving(true);
      setError("");
      try {
        await invoke("book_delete_artifact", { bookUuid: bookUUID, uuid });
        await loadArtifacts();
      } catch (e) {
        setError(`Delete failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID, loadArtifacts]
  );

  const handleAssessNote = useCallback(
    async (noteUuid: string) => {
      if (!isTauri() || !bookUUID) return;
      setGenerating(true);
      setError("");
      try {
        await invoke("book_assess_note", {
          bookUuid: bookUUID,
          noteUuid,
          model: understandingModel.trim() ? understandingModel.trim() : undefined,
        });
        await loadArtifacts();
      } catch (e) {
        setError(`Assess failed: ${e}`);
      }
      setGenerating(false);
    },
    [bookUUID, loadArtifacts, understandingModel]
  );

  const handleExtractTerms = useCallback(
    async (scopeType: ArtifactScopeType, scopeUuid: string) => {
      if (!isTauri() || !bookUUID) return;
      setGenerating(true);
      setError("");
      try {
        await invoke("book_extract_terms", {
          bookUuid: bookUUID,
          scopeType,
          scopeUuid,
          model: understandingModel.trim() ? understandingModel.trim() : undefined,
        });
        await loadTerms();
        // The extraction also recorded a `terms` batch artifact for this scope;
        // the glossary's provenance line reads off it.
        await loadArtifacts();
      } catch (e) {
        setError(`Extract terms failed: ${e}`);
      }
      setGenerating(false);
    },
    [bookUUID, loadTerms, loadArtifacts, understandingModel]
  );

  const handleSaveTerm = useCallback(
    async (term: BookTerm) => {
      if (!isTauri() || !bookUUID) return;
      setSaving(true);
      setError("");
      try {
        await invoke("book_save_term", { bookUuid: bookUUID, term });
        await loadTerms();
      } catch (e) {
        setError(`Save term failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID, loadTerms]
  );

  const handleDeleteTerm = useCallback(
    async (uuid: string) => {
      if (!isTauri() || !bookUUID) return;
      setSaving(true);
      setError("");
      try {
        await invoke("book_delete_term", { bookUuid: bookUUID, uuid });
        await loadTerms();
      } catch (e) {
        setError(`Delete term failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID, loadTerms]
  );

  const handleQuizFinish = useCallback(
    async (
      artifactUuid: string,
      total: number,
      correct: number
    ): Promise<QuizResultResponse | null> => {
      if (!isTauri() || !bookUUID) return null;
      setError("");
      try {
        return await invoke<QuizResultResponse>("book_record_quiz_result", {
          bookUuid: bookUUID,
          artifactUuid,
          total,
          correct,
        });
      } catch (e) {
        setError(`Quiz result failed: ${e}`);
        return null;
      }
    },
    [bookUUID]
  );

  // Card datasets for the "promote to card" picker.
  const listCardDatasets = useCallback(async (): Promise<CardDatasetSummary[]> => {
    if (!isTauri()) return [];
    try {
      return await invoke<CardDatasetSummary[]>("card_dataset_list");
    } catch {
      return [];
    }
  }, []);

  const handlePromoteToCard = useCallback(
    async (datasetUuid: string, question: QuizQuestion) => {
      if (!isTauri() || !datasetUuid) return;
      setSaving(true);
      setError("");
      const now = nowIso();
      const answer =
        question.type === "mcq"
          ? `${question.choices[question.correct] ?? ""}\n\n${question.explanation}`.trim()
          : `${question.model_answer}\n\n${question.explanation}`.trim();
      try {
        await invoke("card_save", {
          datasetUuid,
          card: {
            uuid: "",
            question: question.stem,
            suggestion: "",
            answer,
            note: "",
            familiarity: 0,
            question_hash: null,
            source_card_uuid: null,
            source_dataset_uuid: bookUUID || null,
            deleted_at: null,
            created_at: now,
            updated_at: now,
          },
        });
      } catch (e) {
        setError(`Card save failed: ${e}`);
      }
      setSaving(false);
    },
    [bookUUID]
  );

  return {
    // navigation
    books,
    loadBooks,
    refresh,
    view,
    setView,
    bookUUID,
    setBookUUID,
    chapterUUID,
    setChapterUUID,
    mobile,
    // chapters / sentences
    chaptersFlat,
    flatChapters,
    data,
    paragraphs,
    loading,
    saving,
    needSave,
    viewMode,
    setViewMode,
    error,
    setError,
    // dialogs
    deleteTarget,
    setDeleteTarget,
    confirmDeleteSentence,
    setConfirmDeleteSentence,
    // drawer
    drawer,
    drawerUUID,
    drawerContent,
    setDrawerContent,
    drawerBgColor,
    setDrawerBgColor,
    recorder,
    // derived
    selectedBook,
    currentChapter,
    editSentence,
    nextUUID,
    // handlers
    openEditDrawer,
    openInsertDrawer,
    openAddDrawer,
    closeDrawer,
    clearDrawer,
    discardDrawer,
    handleSaveAdd,
    handleSaveEdit,
    handleParagraphReflow,
    handleDelete,
    doDeleteSentence,
    handleSaveOrder,
    insertParagraph,
    handleNewParagraph,
    handleImport,
    handleParagraphAudio,
    handleDeleteParagraph,
    handleDeleteAll,
    handleDeleteBreakOnly,
    handleToggleChapterStatus,
    // understanding layer
    understanding,
    setUnderstanding,
    toggleUnderstanding,
    focusParagraph,
    setFocusParagraph,
    spyFocusParagraph,
    selectParagraph,
    focusSentence,
    selectSentence,
    cycleScope,
    selectSentenceLevel,
    selectParagraphLevel,
    editingSentence,
    setEditingSentence,
    handleCommitSentenceEdit,
    allowReflowDelete,
    removedArtifactCount,
    understandingTab,
    setUnderstandingTab,
    cycleTab,
    chapterMode,
    setChapterMode,
    scope,
    mobilePanelOpen,
    setMobilePanelOpen,
    paraIndexOf,
    splitRatio,
    setSplitRatio,
    artifacts,
    artifactsByScope,
    chapterArtifacts,
    jobs,
    terms,
    generating,
    understandingModel,
    setUnderstandingModel,
    handleGenerate,
    handleRequestAgent,
    handleCancelJob,
    handleParagraphListen,
    handleSaveNote,
    handleDeleteArtifact,
    handleAssessNote,
    handleExtractTerms,
    handleSaveTerm,
    handleDeleteTerm,
    handleQuizFinish,
    listCardDatasets,
    handlePromoteToCard,
  };
}
