/**
 * useBookData — encapsulates all shared navigation state plus the "Books"
 * data loading, state management and business logic for the books page.
 *
 * Mirrors `useDictationData`: the hook owns everything, `BooksPage` renders
 * the breadcrumb + toolbar chrome off it, and `ReadingView` becomes a purely
 * presentational drill-down (books list -> chapter list -> reading pane).
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useImmer } from "use-immer";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
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
import type { CardDatasetSummary } from "@/lib/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { useRecorder } from "@/components/books/useRecorder";
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
  const [drawerRecognized, setDrawerRecognized] = useState("");
  const [drawerBgColor, setDrawerBgColor] = useState<string | null>(null);
  const [drawerAudio, setDrawerAudio] = useState<{ rel: string; url: string } | null>(null);

  const flatChapters = useMemo(() => flattenChapters(chaptersFlat), [chaptersFlat]);
  const paragraphs = useMemo(() => groupIntoParagraphs(data), [data]);

  // ── Understanding layer ────────────────────────────────────────
  // Split-mode toggle, the focused paragraph (its break-row uuid), the mobile
  // drawer open flag, the loaded artifacts/glossary, and the persisted split.
  const [understanding, setUnderstanding] = useState(false);
  const [focusParagraph, setFocusParagraphState] = useState<string | null>(null);
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

  // Load artifacts when a chapter opens and drop the previous paragraph focus.
  useEffect(() => {
    setFocusParagraphState(null);
    loadArtifacts();
    loadJobs();
  }, [chapterUUID, loadArtifacts, loadJobs]);

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
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [bookUUID, loadArtifacts, loadJobs]);

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

  // ── Recording ──────────────────────────────────────────────────
  const handleRecordResult = useCallback(
    async ({ wavBase64, text }: { wavBase64: string; text: string }) => {
      if (!bookUUID || !isTauri()) return;
      try {
        const res = await invoke<AudioWriteResult>("book_write_audio", {
          bookUuid: bookUUID,
          name: `${drawerUUID}.wav`,
          wavBase64,
        });
        setDrawerAudio({ rel: res.rel_path, url: res.abs_path });
        setDrawerRecognized(text);
        setDrawerContent((c) => (c ? c : text));
      } catch (e) {
        setError(`Failed to save recording: ${e}`);
      }
    },
    [bookUUID, drawerUUID]
  );

  const recorder = useRecorder(handleRecordResult, (msg) => setError(msg));

  // ── Drawer helpers ─────────────────────────────────────────────
  const openEditDrawer = (sentence: SentenceClient) => {
    setDrawer({ mode: "edit", sentence });
    setDrawerUUID(sentence.uuid);
    setDrawerContent(sentence.content ?? "");
    setDrawerRecognized(sentence.recognized ?? "");
    setDrawerBgColor(sentence.bg_color ?? null);
    setDrawerAudio(
      sentence.audio_path && sentence.audio_url
        ? { rel: sentence.audio_path, url: sentence.audio_url }
        : null
    );
  };

  const openInsertDrawer = (insertBeforeUUID: string | null) => {
    setDrawer({ mode: "add", insertBeforeUUID });
    setDrawerUUID(getUUID());
    setDrawerContent("");
    setDrawerRecognized("");
    setDrawerBgColor(null);
    setDrawerAudio(null);
  };

  const openAddDrawer = (para: Paragraph) => openInsertDrawer(para.breakSentence?.uuid ?? null);

  const closeDrawer = async () => {
    // Discard orphan audio recorded for a new sentence that was never saved.
    if (drawer?.mode === "add" && drawerAudio && isTauri()) {
      try {
        await invoke("book_delete_audio", { bookUuid: bookUUID, relPath: drawerAudio.rel });
      } catch {
        /* ignore */
      }
    }
    setDrawer(null);
    setDrawerContent("");
    setDrawerRecognized("");
    setDrawerBgColor(null);
    setDrawerAudio(null);
  };

  const clearDrawer = async () => {
    if (drawer?.mode !== "add") return;
    // Discard orphan audio before resetting.
    if (drawerAudio && isTauri()) {
      try {
        await invoke("book_delete_audio", { bookUuid: bookUUID, relPath: drawerAudio.rel });
      } catch {
        /* ignore */
      }
    }
    setDrawerUUID(getUUID());
    setDrawerContent("");
    setDrawerRecognized("");
    setDrawerBgColor(null);
    setDrawerAudio(null);
  };

  const discardDrawer = async () => {
    if (drawer?.mode === "edit") {
      // Revert to original sentence values
      const s = drawer.sentence;
      setDrawerContent(s.content ?? "");
      setDrawerRecognized(s.recognized ?? "");
      setDrawerBgColor(s.bg_color ?? null);
      setDrawerAudio(
        s.audio_path && s.audio_url
          ? { rel: s.audio_path, url: s.audio_url }
          : null
      );
    } else if (drawer?.mode === "add") {
      await clearDrawer();
    }
  };

  const playDrawerAudio = () => {
    if (drawerAudio?.url) new Audio(convertFileSrc(drawerAudio.url)).play();
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
        audio_path: drawerAudio?.rel ?? null,
        audio_url: drawerAudio?.url ?? null,
        recognized: drawerRecognized || null,
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
      setDrawerRecognized("");
      setDrawerBgColor(null);
      setDrawerAudio(null);
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Save edit ──────────────────────────────────────────────────
  const handleSaveEdit = async () => {
    if (drawer?.mode !== "edit" || !isTauri()) return;
    setSaving(true);
    setError("");
    const sentence = drawer.sentence;
    try {
      const updated: SentenceClient = {
        ...sentence,
        content: drawerContent,
        recognized: drawerRecognized || null,
        audio_path: drawerAudio?.rel ?? null,
        audio_url: drawerAudio?.url ?? null,
        bg_color: drawerBgColor,
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
      setDrawer(null);
      setDrawerContent("");
      setDrawerRecognized("");
      setDrawerBgColor(null);
      setDrawerAudio(null);
    } catch (e) {
      setError(`Save failed: ${e}`);
    }
    setSaving(false);
  };

  // ── Delete single sentence (from drawer) ───────────────────────
  const handleDelete = () => {
    if (drawer?.mode !== "edit") return;
    setConfirmDeleteSentence(true);
  };

  const doDeleteSentence = async () => {
    if (drawer?.mode !== "edit" || !isTauri()) return;
    setSaving(true);
    setError("");
    const sentence = drawer.sentence;
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

  // ── Import sentences from text (each line = sentence, empty line = paragraph break) ─
  const handleImport = async (para: Paragraph, text: string) => {
    if (!text.trim() || !chapterUUID || !isTauri()) return;
    setSaving(true);
    setError("");

    const lines = text.split("\n");
    const now = nowIso();
    const toSave: SentenceClient[] = [];

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i].trim();
      if (line === "") {
        // Skip leading empty lines
        if (toSave.length === 0) continue;
        // Skip trailing empty lines
        const remaining = lines.slice(i + 1).some((l) => l.trim() !== "");
        if (!remaining) break;
        // Collapse consecutive empty lines into a single paragraph break
        if (toSave[toSave.length - 1].sentence_type === "paragraph_break") continue;
        toSave.push({
          uuid: getUUID(),
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
        });
      } else {
        toSave.push({
          uuid: getUUID(),
          chapter_uuid: chapterUUID,
          user_id: "",
          order_num: 0,
          content: line,
          sentence_type: "text",
          audio_path: null,
          audio_url: null,
          recognized: null,
          bg_color: null,
          created_at: now,
          updated_at: now,
          modified: false,
          hasLocalAudio: false,
        });
      }
    }

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

  const setFocusParagraph = useCallback((uuid: string | null) => {
    setFocusParagraphState(uuid);
  }, []);

  // Tapping a paragraph focuses it and, on a phone, slides the panel in.
  const selectParagraph = useCallback(
    (uuid: string | null) => {
      setFocusParagraphState(uuid);
      if (mobile && uuid) setMobilePanelOpen(true);
    },
    [mobile]
  );

  const toggleUnderstanding = useCallback(() => {
    setUnderstanding((prev) => {
      const next = !prev;
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
  const handleGenerate = useCallback(
    async (scopeType: ArtifactScopeType, scopeUuid: string, kind: ArtifactKind) => {
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
    async (scopeType: ArtifactScopeType, scopeUuid: string, kind: ArtifactKind) => {
      if (!isTauri() || !bookUUID) return;
      setError("");
      try {
        await invoke("book_request_artifact", {
          bookUuid: bookUUID,
          scopeType,
          scopeUuid,
          kind,
          model: understandingModel.trim() ? understandingModel.trim() : undefined,
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
        new Audio(convertFileSrc(res.abs_path)).play();
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
      } catch (e) {
        setError(`Extract terms failed: ${e}`);
      }
      setGenerating(false);
    },
    [bookUUID, loadTerms, understandingModel]
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
    drawerRecognized,
    drawerBgColor,
    setDrawerBgColor,
    drawerAudio,
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
    playDrawerAudio,
    handleSaveAdd,
    handleSaveEdit,
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
    selectParagraph,
    mobilePanelOpen,
    setMobilePanelOpen,
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
