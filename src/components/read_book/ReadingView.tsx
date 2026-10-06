"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useImmer } from "use-immer";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Library, AlignLeft, AlignJustify, BookOpen, RotateCcw } from "lucide-react";
import type {
  BookMeta,
  BookChapter,
  BookSentence,
  AudioWriteResult,
  SentenceClient,
  Paragraph,
  DrawerState,
} from "@/lib/read/types";
import { flattenChapters, groupIntoParagraphs, toDbSentence } from "@/lib/read/types";
import { isTauri } from "@/lib/tauri";
import { CollapsibleSidebar, type SidebarController } from "@/components/layout/CollapsibleSidebar";
import ParagraphList from "./ParagraphList";
import SentenceDrawer from "./SentenceDrawer";
import { useRecorder } from "./useRecorder";
import { getUUID, nowIso } from "./utils";
import ConfirmDialog from "./ConfirmDialog";

type Props = { books: BookMeta[]; sidebar: SidebarController };

export default function ReadingView({ books, sidebar }: Props) {
  // selectors
  const [chaptersFlat, setChaptersFlat] = useState<BookChapter[]>([]);
  const [bookUUID, setBookUUID] = useState("");
  const [chapterUUID, setChapterUUID] = useState("");
  const mobile = sidebar.mobile;

  // data
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
  const currentChapter = chaptersFlat.find((c) => c.uuid === chapterUUID);

  const bookBtnClass = (active: boolean) =>
    `w-full text-left px-2 py-1.5 rounded-lg text-base font-medium transition-colors flex items-center gap-2 cursor-pointer ${
      active ? "bg-accent-bg text-white" : "hover:bg-bg-hover text-text-primary"
    }`;
  const chapterBtnClass = (active: boolean, completed: boolean) =>
    `w-full text-left px-2 py-1 rounded text-base transition-colors cursor-pointer ${
      active
        ? "bg-accent-bg text-white font-semibold"
        : completed
        ? "bg-green-500/15 hover:bg-green-500/25 text-text-secondary"
        : "hover:bg-bg-hover text-text-secondary"
    }`;

  // Book / chapter list. Rendered inside the persistent desktop column or the
  // mobile slide-in drawer. Selecting a chapter dismisses the drawer on mobile
  // so the reading pane gets the full width.
  const tocContent = (
    <div className="flex-1 min-w-0 overflow-y-auto p-3 flex flex-col gap-0.5">
      <div className="flex flex-row items-center justify-start px-1 mb-2 gap-2">
        <Library size={16} className="text-text-tertiary" />
        <span className="text-xs font-semibold text-text-tertiary tracking-wider">Library</span>
        {loading && <span className="text-xs text-text-tertiary">loading…</span>}
      </div>
      {books.map((book) => (
        <div key={book.uuid}>
          <button
            className={bookBtnClass(bookUUID === book.uuid)}
            onClick={() => setBookUUID(bookUUID === book.uuid ? "" : book.uuid)}
          >
            <BookOpen size={16} /> {book.title}
          </button>
          {bookUUID === book.uuid && flatChapters.length > 0 && (
            <div className="ml-2 mt-0.5 mb-1 flex flex-col gap-0.5 border-l-2 border-border-default pl-2">
              {flatChapters.map((c) => (
                <button
                  key={c.uuid}
                  className={chapterBtnClass(chapterUUID === c.uuid, c.status === "completed")}
                  onClick={() => {
                    setChapterUUID(c.uuid);
                    sidebar.closeOnSelect();
                  }}
                  style={{ paddingLeft: `${c.depth * 16 + 8}px` }}
                >
                  <span className="line-clamp-1">{c.depth > 0 ? "└ " : ""}{c.title}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
      {books.length === 0 && (
        <div className="text-center text-text-tertiary py-4 text-sm">
          No books yet. Use "Manage" to create one.
        </div>
      )}
    </div>
  );

  return (
    <div className="relative w-full h-full min-h-0 flex overflow-hidden min-w-0">
      {/* TOC — shared sidebar (split column on desktop, slide-in drawer on mobile) */}
      <CollapsibleSidebar sidebar={sidebar}>{tocContent}</CollapsibleSidebar>

      {/* Main content */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col gap-4 overflow-y-auto p-4 pb-[50vh]">
      {/* Error banner */}
      {error && (
        <div className="px-3 py-2 rounded-lg bg-red-500/15 text-red-500 text-sm flex items-center justify-between">
          <span>{error}</span>
          <button className="cursor-pointer" onClick={() => setError("")}>
            ✕
          </button>
        </div>
      )}

      {/* Toolbar */}
      {chapterUUID && !loading && (
        <div className="flex flex-row items-center justify-end gap-2">
          {needSave && (
            <button
              className="px-3 py-1.5 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
              disabled={saving}
              onClick={handleSaveOrder}
            >
              Save Order
            </button>
          )}
          <button
            className={`px-3 py-1.5 rounded-md text-sm border disabled:opacity-50 cursor-pointer ${
              currentChapter?.status === "completed"
                ? "bg-accent-bg text-white border-transparent"
                : "border-border-default text-text-secondary hover:bg-bg-hover"
            }`}
            disabled={saving}
            onClick={handleToggleChapterStatus}
          >
            {currentChapter?.status === "completed" ? "✓ Completed" : "Mark Completed"}
          </button>
          <button
            title={viewMode === "line" ? "Switch to paragraph view" : "Switch to line view"}
            className="p-1.5 rounded-md border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer"
            onClick={() => setViewMode((m) => (m === "line" ? "inline" : "line"))}
          >
            {viewMode === "line" ? <AlignJustify size={18} /> : <AlignLeft size={18} />}
          </button>
        </div>
      )}

      {loading && <div className="flex justify-center my-8 text-text-tertiary">Loading…</div>}

      {/* Paragraphs */}
      {chapterUUID && !loading && (
        <ParagraphList
          paragraphs={paragraphs}
          viewMode={viewMode}
          saving={saving}
          mobile={mobile}
          onEditSentence={openEditDrawer}
          onAddSentence={openAddDrawer}
          onDeleteParagraph={handleDeleteParagraph}
          onParagraphAudio={handleParagraphAudio}
          onImport={handleImport}
        />
      )}

      {/* New paragraph */}
      {chapterUUID && !loading && (
        <div className="flex justify-center">
          <button
            className="px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover border border-border-default disabled:opacity-50 cursor-pointer"
            disabled={saving}
            onClick={handleNewParagraph}
          >
            + New Paragraph
          </button>
        </div>
      )}

      {/* Delete paragraph dialog */}
      {deleteTarget && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
          onClick={() => setDeleteTarget(null)}
        >
          <div
            className="bg-bg-card rounded-2xl shadow-2xl p-6 flex flex-col gap-4 w-72 border border-border-default"
            onClick={(e) => e.stopPropagation()}
          >
            <p className="font-semibold text-base text-text-primary">Delete paragraph?</p>
            <p className="text-sm text-text-secondary">
              {deleteTarget.sentences.length} sentence(s)
            </p>
            <div className="flex flex-col gap-2">
              <button
                className="px-3 py-2 rounded-md text-sm bg-red-500 text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
                disabled={saving}
                onClick={handleDeleteAll}
              >
                Delete all sentences
              </button>
              <button
                className="px-3 py-2 rounded-md text-sm text-red-500 border border-border-default hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
                disabled={saving || !deleteTarget.breakSentence}
                onClick={handleDeleteBreakOnly}
              >
                Delete paragraph break only
              </button>
              <button
                className="px-3 py-2 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
                onClick={() => setDeleteTarget(null)}
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Sentence drawer */}
      <SentenceDrawer
        drawer={drawer}
        recognized={drawerRecognized}
        content={drawerContent}
        onContentChange={setDrawerContent}
        bgColor={drawerBgColor}
        onBgColorChange={setDrawerBgColor}
        hasAudio={!!drawerAudio}
        bookUUID={bookUUID}
        saving={saving}
        recording={recorder.recording}
        processing={recorder.processing}
        onClose={closeDrawer}
        onClear={clearDrawer}
        onDiscard={discardDrawer}
        onPlay={playDrawerAudio}
        onToggleRecording={recorder.toggle}
        onSaveAdd={handleSaveAdd}
        onSaveEdit={handleSaveEdit}
        onDelete={handleDelete}
        onInsertBefore={() => editSentence && openInsertDrawer(editSentence.uuid)}
        onInsertAfter={() => openInsertDrawer(nextUUID)}
        onParagraphBefore={() => editSentence && insertParagraph(editSentence.uuid)}
        onParagraphAfter={() => insertParagraph(nextUUID)}
      />

      <ConfirmDialog
        request={
          confirmDeleteSentence
            ? {
                message: "Delete this sentence?",
                confirmLabel: "Delete",
                onConfirm: doDeleteSentence,
              }
            : null
        }
        onClose={() => setConfirmDeleteSentence(false)}
      />
      </div>
    </div>
  );
}
