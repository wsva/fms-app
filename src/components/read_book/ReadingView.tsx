"use client";

import { BookOpen, Check, Library } from "lucide-react";
import type { useBookData } from "@/hooks/useBookData";
import ParagraphList from "./ParagraphList";
import SentenceDrawer from "./SentenceDrawer";
import ConfirmDialog from "./ConfirmDialog";

type Props = { b: ReturnType<typeof useBookData> };

/**
 * Presentational drill-down for the "Read a Book" mode. All state and handlers
 * live in `useBookData` (see ReadBookPage, which renders the shared breadcrumb +
 * toolbar). This component swaps between three levels:
 *   no book       -> books list
 *   book, no chap -> chapter list
 *   chapter       -> reading pane (paragraphs + sentence drawer)
 */
export default function ReadingView({ b }: Props) {
  const {
    bookUUID, chapterUUID, flatChapters, mobile,
    loading, saving, error, setError, viewMode, paragraphs,
    drawer, drawerContent, drawerRecognized, drawerBgColor, drawerAudio, recorder,
    deleteTarget, setDeleteTarget, confirmDeleteSentence, setConfirmDeleteSentence,
    editSentence, nextUUID,
  } = b;

  // ── Level 1: books list ────────────────────────────────────────
  if (!bookUUID) {
    return (
      <div className="flex-1 min-h-0 overflow-y-auto p-4 min-w-0">
        <div className="flex items-center gap-2 mb-3">
          <Library size={16} className="text-text-tertiary" />
          <span className="text-xs font-semibold text-text-tertiary tracking-wider">Library</span>
          <span className="text-xs text-text-tertiary">{b.books.length}</span>
        </div>
        {b.books.length === 0 ? (
          <div className="text-center text-text-tertiary py-8 text-sm">
            No books yet. Use &ldquo;Manage&rdquo; to create one.
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {b.books.map((book) => (
              <button
                key={book.uuid}
                onClick={() => b.setBookUUID(book.uuid)}
                className="w-full flex items-center gap-3 px-4 py-3 rounded-lg border border-border-default bg-bg-card hover:bg-bg-hover text-left cursor-pointer transition-colors"
              >
                <BookOpen size={18} className="text-text-tertiary shrink-0" />
                <span className="text-base font-medium text-text-primary truncate">{book.name}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }

  // ── Level 2: chapter list ──────────────────────────────────────
  if (!chapterUUID) {
    return (
      <div className="flex-1 min-h-0 overflow-y-auto p-4 min-w-0">
        <div className="flex items-center gap-2 mb-3">
          <BookOpen size={16} className="text-text-tertiary" />
          <span className="text-xs font-semibold text-text-tertiary tracking-wider truncate">
            {b.selectedBook?.name}
          </span>
          <span className="text-xs text-text-tertiary">{flatChapters.length}</span>
        </div>
        {flatChapters.length === 0 ? (
          <div className="text-center text-text-tertiary py-8 text-sm">
            No chapters yet. Use &ldquo;Manage&rdquo; to add some.
          </div>
        ) : (
          <div className="flex flex-col gap-1">
            {flatChapters.map((c) => {
              const completed = c.status === "completed";
              return (
                <button
                  key={c.uuid}
                  onClick={() => b.setChapterUUID(c.uuid)}
                  className={`w-full flex items-center gap-2 px-3 py-2 rounded-lg text-left text-base cursor-pointer transition-colors ${
                    completed
                      ? "bg-green-500/15 hover:bg-green-500/25 text-text-primary"
                      : "bg-bg-card hover:bg-bg-hover text-text-primary"
                  }`}
                  style={{ paddingLeft: `${c.depth * 16 + 12}px` }}
                >
                  <span className="line-clamp-1 flex-1">{c.depth > 0 ? "└ " : ""}{c.title}</span>
                  {completed && <Check size={16} className="text-green-500 shrink-0" />}
                </button>
              );
            })}
          </div>
        )}
      </div>
    );
  }

  // ── Level 3: reading pane ──────────────────────────────────────
  return (
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

      {loading && <div className="flex justify-center my-8 text-text-tertiary">Loading…</div>}

      {/* Paragraphs */}
      {!loading && (
        <ParagraphList
          paragraphs={paragraphs}
          viewMode={viewMode}
          saving={saving}
          mobile={mobile}
          onEditSentence={b.openEditDrawer}
          onAddSentence={b.openAddDrawer}
          onDeleteParagraph={b.handleDeleteParagraph}
          onParagraphAudio={b.handleParagraphAudio}
          onImport={b.handleImport}
        />
      )}

      {/* New paragraph */}
      {!loading && (
        <div className="flex justify-center">
          <button
            className="px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover border border-border-default disabled:opacity-50 cursor-pointer"
            disabled={saving}
            onClick={b.handleNewParagraph}
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
                onClick={b.handleDeleteAll}
              >
                Delete all sentences
              </button>
              <button
                className="px-3 py-2 rounded-md text-sm text-red-500 border border-border-default hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
                disabled={saving || !deleteTarget.breakSentence}
                onClick={b.handleDeleteBreakOnly}
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
        onContentChange={b.setDrawerContent}
        bgColor={drawerBgColor}
        onBgColorChange={b.setDrawerBgColor}
        hasAudio={!!drawerAudio}
        bookUUID={bookUUID}
        saving={saving}
        recording={recorder.recording}
        processing={recorder.processing}
        onClose={b.closeDrawer}
        onClear={b.clearDrawer}
        onDiscard={b.discardDrawer}
        onPlay={b.playDrawerAudio}
        onToggleRecording={recorder.toggle}
        onSaveAdd={b.handleSaveAdd}
        onSaveEdit={b.handleSaveEdit}
        onDelete={b.handleDelete}
        onInsertBefore={() => editSentence && b.openInsertDrawer(editSentence.uuid)}
        onInsertAfter={() => b.openInsertDrawer(nextUUID)}
        onParagraphBefore={() => editSentence && b.insertParagraph(editSentence.uuid)}
        onParagraphAfter={() => b.insertParagraph(nextUUID)}
      />

      <ConfirmDialog
        request={
          confirmDeleteSentence
            ? {
                message: "Delete this sentence?",
                confirmLabel: "Delete",
                onConfirm: b.doDeleteSentence,
              }
            : null
        }
        onClose={() => setConfirmDeleteSentence(false)}
      />
    </div>
  );
}
