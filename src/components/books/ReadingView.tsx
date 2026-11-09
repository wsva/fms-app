"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { BookOpen, Check, Library, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import type { useBookData } from "@/hooks/useBookData";
import { DatasetCard } from "@/components/ui/dataset";
import ParagraphList from "./ParagraphList";
import SentenceDrawer from "./SentenceDrawer";
import ConfirmDialog from "./ConfirmDialog";
import UnderstandingPanel from "./UnderstandingPanel";
import { useBookKeynav } from "./useBookKeynav";

type Props = {
  b: ReturnType<typeof useBookData>;
};

/**
 * Presentational drill-down for the "Books" mode. All state and handlers
 * live in `useBookData` (see BooksPage, which renders the shared breadcrumb +
 * toolbar). This component swaps between three levels:
 *   no book       -> books list
 *   book, no chap -> chapter list
 *   chapter       -> reading pane (paragraphs + sentence drawer)
 */
export default function ReadingView({ b }: Props) {
  const {
    bookUUID, chapterUUID, flatChapters, mobile,
    loading, saving, error, setError, viewMode, paragraphs,
    deleteTarget, setDeleteTarget, confirmDeleteSentence, setConfirmDeleteSentence,
    drawer, drawerContent, drawerBgColor, recorder, editSentence, nextUUID,
  } = b;

  const {
    understanding, splitRatio, setSplitRatio, focusParagraph, selectParagraph,
    focusSentence, selectSentence, editingSentence, cycleScope,
    cycleTab, spyFocusParagraph, scope,
    mobilePanelOpen, setMobilePanelOpen, chapterMode,
  } = b;

  // Reading-pane scroll container — the scroll-spy root and the split's left column.
  const leftRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  // A sentence focus reached by keyboard walking scrolls its line back into
  // view; a click needs no help, so the click road leaves the flag unset.
  const [keepSentenceInScope, setKeepSentenceInScope] = useState(false);

  // Scroll-spy: while understanding mode is on (desktop), the paragraph nearest
  // the top of the reading pane becomes the panel's focus. In chapter mode the
  // tabs bind to the whole chapter, so the spy keeps the focus warm but does
  // not drive the panel. It takes the spy's own road, which only moves the
  // paragraph position — clicks and keys own the level and the sentence —
  // and whose stable identity keeps the observer from re-binding (and
  // re-firing its initial intersection) on every focus change.
  useEffect(() => {
    if (!understanding || mobile || chapterMode) return;
    const root = leftRef.current;
    if (!root) return;
    const els = Array.from(root.querySelectorAll<HTMLElement>("[data-para-uuid]"));
    if (els.length === 0) return;
    const obs = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, c) => a.boundingClientRect.top - c.boundingClientRect.top);
        const uuid = visible[0]?.target.getAttribute("data-para-uuid");
        if (uuid) spyFocusParagraph(uuid);
      },
      { root, rootMargin: "-8% 0px -78% 0px", threshold: 0 }
    );
    els.forEach((el) => obs.observe(el));
    return () => obs.disconnect();
  }, [understanding, mobile, chapterMode, paragraphs, spyFocusParagraph]);

  // Keyboard walking: ↑/↓ (or j/k) step through sentences, p/n through
  // paragraphs, driven off the focused selection.
  const flatSentences = useMemo(
    () => paragraphs.flatMap((p) => p.sentences),
    [paragraphs]
  );
  useBookKeynav(!!chapterUUID && !mobile, paragraphs, focusSentence, focusParagraph, {
    goToSentence: (i) => {
      const s = flatSentences[i];
      if (!s) return;
      setKeepSentenceInScope(true);
      selectSentence(s);
    },
    goToParagraph: (i) => {
      const uuid = paragraphs[i]?.breakSentence?.uuid;
      if (uuid) selectParagraph(uuid);
    },
    cycleScope,
    cycleTab,
  });
  useEffect(() => {
    if (!keepSentenceInScope || !focusSentence) return;
    document
      .querySelector(`[data-sentence-uuid="${focusSentence}"]`)
      ?.scrollIntoView({ block: "nearest" });
    setKeepSentenceInScope(false);
  }, [keepSentenceInScope, focusSentence]);

  // Drag splitter: turn a pointer x into the left column's percentage width.
  const onSplitterDown = (e: React.PointerEvent) => {
    dragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onSplitterMove = (e: React.PointerEvent) => {
    if (!dragging.current) return;
    const row = leftRef.current?.parentElement;
    if (!row) return;
    const rect = row.getBoundingClientRect();
    if (rect.width <= 0) return;
    setSplitRatio(((e.clientX - rect.left) / rect.width) * 100);
  };
  const onSplitterUp = (e: React.PointerEvent) => {
    dragging.current = false;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
  };

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
              <DatasetCard
                key={book.uuid}
                variant="row"
                icon={BookOpen}
                name={book.name}
                description={book.description}
                language={book.language || undefined}
                onOpen={() => b.setBookUUID(book.uuid)}
                onSave={async (patch) => {
                  await invoke("dataset_update", { uuid: book.uuid, ...patch });
                  await emit("dataset-list-changed", {});
                  await b.loadBooks();
                }}
              />
            ))}
          </div>
        )}
      </div>
    );
  }

  // ── Level 2: chapter list ──────────────────────────────────────
  if (!chapterUUID) {
    const chapterList = (
      <>
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
      </>
    );

    return (
      <div className="flex-1 min-h-0 overflow-y-auto p-4 min-w-0">{chapterList}</div>
    );
  }

  // ── Level 3: reading pane ──────────────────────────────────
  // The reading content is shared by the plain pane and the split's left column.
  const readingContent = (
    <>
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
          understanding={understanding}
          focusParagraph={focusParagraph}
          // The ring marks the live scope; a remembered sentence at another
          // level stays unmarked (keyboard walking still resumes from it).
          focusSentence={scope.type === "sentence" ? focusSentence : null}
          editingSentence={editingSentence}
          onCommitEdit={b.handleCommitSentenceEdit}
          onFocusParagraph={selectParagraph}
          onSelectSentence={selectSentence}
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
    </>
  );

  // Overlays (dialogs + sentence drawer) render the same in every layout.
  const overlays = (
    <>
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
              <p className="text-xs text-text-tertiary">
                Deleting the break merges this paragraph with the next and removes
                the paragraph&rsquo;s audio and its generated understanding artifacts.
              </p>
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

      {/* Sentence drawer — retired on desktop, where the focused sentence is
          edited inline and the panel carries the label/copy/color commands.
          It keeps one busy flag: preparing (the model preload) used to ride
          inside `processing`; folded back in so Voice input still disables
          and reads "Recognizing…" while the recorder opens. */}
      {mobile && (
        <SentenceDrawer
          drawer={drawer}
          content={drawerContent}
          onContentChange={b.setDrawerContent}
          bgColor={drawerBgColor}
          onBgColorChange={b.setDrawerBgColor}
          bookUUID={bookUUID}
          terms={b.terms}
          saving={saving}
          recording={recorder.recording}
          processing={recorder.processing || recorder.preparing}
          onClose={b.closeDrawer}
          onClear={b.clearDrawer}
          onDiscard={b.discardDrawer}
          onVoiceInput={recorder.toggle}
          onSaveAdd={b.handleSaveAdd}
          onSaveEdit={b.handleSaveEdit}
          onDelete={b.handleDelete}
          onInsertBefore={() => editSentence && b.openInsertDrawer(editSentence.uuid)}
          onInsertAfter={() => b.openInsertDrawer(nextUUID)}
          onParagraphBefore={() => editSentence && b.insertParagraph(editSentence.uuid)}
          onParagraphAfter={() => b.insertParagraph(nextUUID)}
        />
      )}

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
    </>
  );

  return (
    <>
      {understanding && !mobile ? (
        /* Desktop: horizontal split — reading pane left, understanding panel right. */
        <div className="flex-1 min-w-0 min-h-0 flex flex-row overflow-hidden">
          <div
            ref={leftRef}
            className="min-w-0 overflow-y-auto p-4 pb-[50vh] flex flex-col gap-4"
            style={{ flexBasis: `${splitRatio}%`, flexGrow: 0, flexShrink: 0 }}
          >
            {readingContent}
          </div>
          <div
            className="relative w-3 shrink-0 cursor-col-resize hover:bg-accent/10 after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 after:-translate-x-1/2 after:bg-border-default hover:after:bg-accent/60"
            onPointerDown={onSplitterDown}
            onPointerMove={onSplitterMove}
            onPointerUp={onSplitterUp}
            onPointerCancel={onSplitterUp}
          />
          <div className="min-w-0 flex-1 overflow-hidden flex flex-col">
            <UnderstandingPanel b={b} />
          </div>
        </div>
      ) : (
        /* Plain full-width reading pane. */
        <div
          ref={leftRef}
          className="flex-1 min-w-0 min-h-0 flex flex-col gap-4 overflow-y-auto p-4 pb-[50vh]"
        >
          {readingContent}
        </div>
      )}

      {/* Mobile: the same panel slides in from the right instead of splitting. */}
      {understanding && mobile && (
        <>
          {mobilePanelOpen && (
            <div
              className="fixed inset-0 z-40 bg-black/40"
              onClick={() => setMobilePanelOpen(false)}
            />
          )}
          <div
            className={`fixed top-0 right-0 z-50 h-full w-[88%] max-w-sm bg-bg-body border-l border-border-default shadow-2xl flex flex-col transform transition-transform ${
              mobilePanelOpen ? "translate-x-0" : "translate-x-full"
            }`}
          >
            <div className="shrink-0 flex items-center justify-end px-2 py-1 border-b border-border-default">
              <button
                className="p-1.5 rounded text-text-secondary hover:bg-bg-hover cursor-pointer"
                onClick={() => setMobilePanelOpen(false)}
              >
                <X size={18} />
              </button>
            </div>
            <div className="flex-1 min-h-0">
              <UnderstandingPanel b={b} />
            </div>
          </div>
        </>
      )}

      {overlays}
    </>
  );
}
