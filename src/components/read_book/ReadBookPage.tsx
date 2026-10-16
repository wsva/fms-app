"use client";

import { AlignJustify, AlignLeft, Settings2, Zap } from "lucide-react";
import { useBookData } from "@/hooks/useBookData";
import { tbToggle, tbToggleSoft, tbSolid } from "@/lib/ui/toolbar";
import ReadingView from "./ReadingView";
import BookManager from "./BookManager";
import BookAdvanced from "./BookAdvanced";

export default function ReadBookPage() {
  const b = useBookData();
  const mobile = b.mobile;

  // The last crumb in the trail grows to fill the row; earlier crumbs stay
  // packed left and capped so a long title never pushes them off-screen.
  const crumbCurrent = "text-text-primary font-medium truncate min-w-0 flex-1 text-left";
  const crumbLink = "cursor-pointer hover:underline truncate min-w-0 max-w-[240px] text-accent text-left shrink-0";

  return (
    <div className="flex h-full w-full bg-bg-body min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Breadcrumb navigation — wiki/dictation-style full-width bar sitting
            above the toolbar. Always shown (including mobile) so the trail stays
            reachable. The root crumb is always "Books"; it gains the selected book
            (and, in Read, the chapter) as you drill in — regardless of mode. */}
        <nav className="shrink-0 flex items-center justify-start gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
          {/* Root crumb is always "Books", in every mode. */}
          <button
            className={`shrink-0 cursor-pointer hover:underline text-left ${b.bookUUID ? "text-accent" : "text-text-primary font-medium"}`}
            onClick={() => {
              b.setBookUUID("");
              // Advanced is book-scoped; leaving the book returns to Read so the
              // toolbar and content stay coherent at the books level.
              if (b.view === "advanced") b.setView("read");
            }}
          >
            Books
          </button>
          {b.selectedBook && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              {b.view === "read" && b.chapterUUID ? (
                <button
                  className={crumbLink}
                  onClick={() => b.setChapterUUID("")}
                  title={b.selectedBook.name}
                >
                  {b.selectedBook.name}
                </button>
              ) : (
                <span className={crumbCurrent} title={b.selectedBook.name}>
                  {b.selectedBook.name}
                </span>
              )}
            </>
          )}
          {b.view === "read" && b.selectedBook && b.currentChapter && (
            <>
              <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
              <span className={crumbCurrent} title={b.currentChapter.title}>
                {b.currentChapter.title}
              </span>
            </>
          )}
        </nav>

        {/* Sub-toolbar — mode switch on the left, contextual read actions on the
            right. Wiki-style full-width bar under the breadcrumb. */}
        <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
          <div className="flex items-center gap-1 shrink-0">
            {/* Read is the base view (no toggle active). Manage and Advanced are
                toggles that return to Read when clicked while active. Manage keeps
                the current book but does a level-specific job, so its label differs:
                at the books level it is "Manage Books" (Locations + book
                add/edit/delete); on a book page it is "Manage Chapters" (edit that
                book's chapters). */}
            <button
              type="button"
              className={tbToggle(b.view === "manage")}
              title={b.bookUUID ? "Manage Chapters" : "Manage Books"}
              aria-label={b.bookUUID ? "Manage Chapters" : "Manage Books"}
              onClick={() => b.setView(b.view === "manage" ? "read" : "manage")}
            >
              <Settings2 size={16} />{!mobile && (b.bookUUID ? " Manage Chapters" : " Manage Books")}
            </button>
            {/* Advanced is book-scoped — shown only on a book page, not at the
                books-list level. */}
            {b.bookUUID && (
              <button
                type="button"
                className={tbToggle(b.view === "advanced")}
                title="Advanced"
                aria-label="Advanced"
                onClick={() => b.setView(b.view === "advanced" ? "read" : "advanced")}
              >
                <Zap size={16} />{!mobile && " Advanced"}
              </button>
            )}
          </div>

          <div className="flex-1 min-w-0" />

          {/* Contextual read actions — only when a chapter is open. */}
          {b.view === "read" && b.chapterUUID && !b.loading && (
            <div className="flex items-center gap-1 shrink-0">
              {b.needSave && (
                <button type="button" className={tbSolid} disabled={b.saving} onClick={b.handleSaveOrder} title="Save the reordered sentences">
                  Save Order
                </button>
              )}
              <button
                type="button"
                className={tbToggle(b.currentChapter?.status === "completed")}
                disabled={b.saving}
                onClick={b.handleToggleChapterStatus}
                title="Toggle chapter completed"
              >
                {b.currentChapter?.status === "completed" ? "Completed" : "Mark Completed"}
              </button>
              <button
                type="button"
                className={tbToggleSoft(b.viewMode === "inline")}
                onClick={() => b.setViewMode((m) => (m === "line" ? "inline" : "line"))}
                title={b.viewMode === "line" ? "Switch to paragraph view" : "Switch to line view"}
              >
                {b.viewMode === "line" ? <AlignJustify size={16} /> : <AlignLeft size={16} />}
                {!mobile && (b.viewMode === "line" ? " Lines" : " Inline")}
              </button>
            </div>
          )}
        </div>

        {/* Content — all three views stay mounted to preserve state. */}
        <div className="flex-1 flex flex-col min-h-0 overflow-hidden min-w-0">
          <div style={{ display: b.view === "read" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-hidden min-w-0">
            <ReadingView b={b} />
          </div>
          <div style={{ display: b.view === "manage" ? "block" : "none" }} className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 min-w-0">
            <BookManager books={b.books} bookUUID={b.bookUUID} setBookUUID={b.setBookUUID} onBooksChanged={b.loadBooks} />
          </div>
          <div style={{ display: b.view === "advanced" ? "block" : "none" }} className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 min-w-0">
            <BookAdvanced books={b.books} bookUUID={b.bookUUID} setBookUUID={b.setBookUUID} />
          </div>
        </div>
      </div>
    </div>
  );
}
