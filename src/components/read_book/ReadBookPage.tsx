"use client";

import { AlignJustify, AlignLeft, Settings2, Zap } from "lucide-react";
import { useBookData } from "@/hooks/useBookData";
import { Toolbar, Breadcrumb, ToolbarButton } from "@/components/ui/toolbar";
import ReadingView from "./ReadingView";
import BookManager from "./BookManager";
import BookAdvanced from "./BookAdvanced";

export default function ReadBookPage() {
  const b = useBookData();
  const mobile = b.mobile;

  // On a chapter page you are reading a chapter, so the book-level mode toggles
  // (Manage Chapters / Advanced) do not apply and are hidden; only the contextual
  // read actions show on the toolbar right.
  const onChapterPage = b.view === "read" && !!b.chapterUUID;

  // The root "Books" crumb is plain text only while you are on the books library
  // in Read mode (nothing to go back to). Everywhere else — a book is open, or
  // you are in Manage/Advanced — it is a link back to the library.
  const booksIsCurrent = !b.bookUUID && b.view === "read";

  return (
    <div className="flex h-full w-full bg-bg-body min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Breadcrumb navigation — wiki/dictation-style full-width bar sitting
            above the toolbar. Always shown (including mobile) so the trail stays
            reachable. The root crumb is always "Books"; it gains the selected book
            (and, in Read, the chapter) as you drill in — regardless of mode. */}
        <Breadcrumb>
          {/* Root crumb is always "Books" — plain text on the books page (Read),
              a link back to the library whenever a book is open or you are in
              Manage/Advanced mode. */}
          {booksIsCurrent ? (
            <Breadcrumb.Current grow={false}>Books</Breadcrumb.Current>
          ) : (
            <Breadcrumb.Link
              onClick={() => {
                b.setBookUUID("");
                // Return to the Read library from any mode so the link is
                // meaningful when clicked from Manage/Advanced.
                b.setView("read");
              }}
            >
              Books
            </Breadcrumb.Link>
          )}
          {b.selectedBook && (
            <>
              <Breadcrumb.Sep />
              {b.view === "read" && b.chapterUUID ? (
                <Breadcrumb.Link
                  truncate
                  onClick={() => b.setChapterUUID("")}
                  title={b.selectedBook.name}
                >
                  {b.selectedBook.name}
                </Breadcrumb.Link>
              ) : (
                <Breadcrumb.Current title={b.selectedBook.name}>
                  {b.selectedBook.name}
                </Breadcrumb.Current>
              )}
            </>
          )}
          {b.view === "read" && b.selectedBook && b.currentChapter && (
            <>
              <Breadcrumb.Sep />
              <Breadcrumb.Current title={b.currentChapter.title}>
                {b.currentChapter.title}
              </Breadcrumb.Current>
            </>
          )}
        </Breadcrumb>

        {/* Sub-toolbar — mode switch on the left, contextual read actions on the
            right. Wiki-style full-width bar under the breadcrumb. */}
        <Toolbar mobile={mobile}>
          {/* All toolbar buttons are right-aligned (pushed to the end). The
              flexible spacer sits on the left so whichever group is visible hugs
              the right edge; the mode group and the contextual read actions are
              mutually exclusive (mode hides on a chapter page, the actions show
              only there). */}
          <div className="flex-1 min-w-0" />

          {!onChapterPage && (
          <div className="flex items-center gap-1 shrink-0">
            {/* Read is the base view (no toggle active). Manage and Advanced are
                toggles that return to Read when clicked while active. Manage keeps
                the current book but does a level-specific job, so its label differs:
                at the books level it is "Manage Books" (Locations + book
                add/edit/delete); on a book page it is "Manage Chapters" (edit that
                book's chapters). */}
            <ToolbarButton
              variant="toggle"
              active={b.view === "manage"}
              icon={Settings2}
              label={b.bookUUID ? "Manage Chapters" : "Manage Books"}
              onClick={() => b.setView(b.view === "manage" ? "read" : "manage")}
            />
            {/* Advanced is book-scoped — shown only on a book page, not at the
                books-list level. */}
            {b.bookUUID && (
              <ToolbarButton
                variant="toggle"
                active={b.view === "advanced"}
                icon={Zap}
                label="Advanced"
                onClick={() => b.setView(b.view === "advanced" ? "read" : "advanced")}
              />
            )}
          </div>
          )}

          {/* Contextual read actions — only when a chapter is open. */}
          {b.view === "read" && b.chapterUUID && !b.loading && (
            <div className="flex items-center gap-1 shrink-0">
              {b.needSave && (
                <ToolbarButton
                  variant="toggle"
                  active
                  label="Save Order"
                  title="Save the reordered sentences"
                  disabled={b.saving}
                  onClick={b.handleSaveOrder}
                />
              )}
              <ToolbarButton
                variant="toggle"
                active={b.currentChapter?.status === "completed"}
                label={b.currentChapter?.status === "completed" ? "Completed" : "Mark Completed"}
                title="Toggle chapter completed"
                disabled={b.saving}
                onClick={b.handleToggleChapterStatus}
              />
              <ToolbarButton
                variant="toggleSoft"
                active={b.viewMode === "inline"}
                icon={b.viewMode === "line" ? AlignJustify : AlignLeft}
                label={b.viewMode === "line" ? "Lines" : "Inline"}
                title={b.viewMode === "line" ? "Switch to paragraph view" : "Switch to line view"}
                onClick={() => b.setViewMode((m) => (m === "line" ? "inline" : "line"))}
              />
            </div>
          )}
        </Toolbar>

        {/* Content — all three views stay mounted to preserve state. */}
        <div className="flex-1 flex flex-col min-h-0 overflow-hidden min-w-0">
          <div style={{ display: b.view === "read" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-hidden min-w-0">
            <ReadingView b={b} />
          </div>
          <div style={{ display: b.view === "manage" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-hidden p-4 min-w-0 flex flex-col">
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
