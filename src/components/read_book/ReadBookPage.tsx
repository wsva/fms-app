"use client";

import { useState } from "react";
import { AlignJustify, AlignLeft, Settings2, Zap, FolderOpen, Brain } from "lucide-react";
import { useBookData } from "@/hooks/useBookData";
import { usePageRefresh } from "@/lib/refresh/page-refresh";
import { Toolbar, Breadcrumb, ToolbarButton, ToolbarOverflow, ToolbarMenuItem } from "@/components/ui/toolbar";
import { LocationsManager } from "@/components/ui/LocationsManager";
import PendingWritebackBanner from "@/components/ui/PendingWritebackBanner";
import ReadingView from "./ReadingView";
import BookManager from "./BookManager";
import BookAdvanced from "./BookAdvanced";

export default function ReadBookPage() {
  const b = useBookData();
  const mobile = b.mobile;

  // Locations overlay (shared manager) — opened from the toolbar overflow menu at
  // the books level, replacing the content area the same way the Cards page does.
  const [showLocations, setShowLocations] = useState(false);
  const [locError, setLocError] = useState<string | null>(null);
  // Bumped by the shared page refresh so an open Locations overlay re-reads its
  // own directory list instead of keeping the one it mounted with.
  const [locRefresh, setLocRefresh] = useState(0);

  // On a chapter page you are reading a chapter, so the book-level mode toggles
  // (Manage Chapters / Advanced) do not apply and are hidden; only the contextual
  // read actions show on the toolbar right.
  const onChapterPage = b.view === "read" && !!b.chapterUUID;

  // The root "Books" crumb is plain text only while you are on the books library
  // in Read mode (nothing to go back to). Everywhere else — a book is open, or
  // you are in Manage/Advanced — it is a link back to the library.
  const booksIsCurrent = !b.bookUUID && b.view === "read" && !showLocations;

  // Keep the books/chapters/sentences current without a toolbar button: the
  // shared hook re-reads on activation, on app resume and on
  // `dataset-list-changed`, and refuses while a sentence reorder is unsaved.
  usePageRefresh("read-book", {
    refresh: () => {
      b.refresh();
      setLocRefresh((n) => n + 1);
    },
    canRefresh: () => !b.needSave,
    events: ["dataset-list-changed"],
  });

  return (
    <div className="flex h-full w-full bg-bg-body min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Thin-client reminder: chapter/sentence/word edits wait in the
            writeback queue until a sync round flushes them to the hub. */}
        <PendingWritebackBanner mobile={mobile} />
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
                setShowLocations(false);
              }}
            >
              Books
            </Breadcrumb.Link>
          )}
          {showLocations && (
            <>
              <Breadcrumb.Sep />
              <Breadcrumb.Current grow={false}>Locations</Breadcrumb.Current>
            </>
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
            {!mobile && (
              <ToolbarButton
                variant="toggle"
                active={b.view === "manage"}
                icon={Settings2}
                label={b.bookUUID ? "Manage Chapters" : "Manage Books"}
                onClick={() => b.setView(b.view === "manage" ? "read" : "manage")}
              />
            )}
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
              {/* Understanding split mode: on desktop it splits the reading pane
                  (content left, panel right); on a phone it opens the same panel
                  as a slide-in drawer. Clicking while active returns to plain
                  reading. */}
              <ToolbarButton
                variant="toggle"
                active={b.understanding}
                icon={Brain}
                label="Understanding"
                title="Toggle understanding mode"
                onClick={b.toggleUnderstanding}
              />
            </div>
          )}

          {/* Locations management (books level) — opens the shared manager as a
              full-content overlay, matching the Cards page pattern. On a phone the
              overflow also swallows the Manage toggle, so the visible stacked row
              keeps just the read actions. Rendered only where it has an entry: the
              Refresh item that used to keep it alive on a chapter page is gone. */}
          {(!b.bookUUID || (mobile && !onChapterPage)) && (
            <ToolbarOverflow>
              {!b.bookUUID && (
                <ToolbarMenuItem icon={FolderOpen} onClick={() => setShowLocations(true)}>Locations</ToolbarMenuItem>
              )}
              {mobile && !onChapterPage && (
                <ToolbarMenuItem
                  icon={Settings2}
                  checked={b.view === "manage"}
                  onClick={() => b.setView(b.view === "manage" ? "read" : "manage")}
                >
                  {b.bookUUID ? "Manage Chapters" : "Manage Books"}
                </ToolbarMenuItem>
              )}
            </ToolbarOverflow>
          )}
        </Toolbar>

        {/* Locations overlay — replaces the three views while open, the same
            full-content overlay the Cards page uses. */}
        {showLocations && (
          <div className="flex-1 min-h-0 overflow-hidden flex flex-col">
            {locError && (
              <div className="mx-3 mt-3 p-3 bg-error-bg text-error-text rounded-md text-sm">{locError}</div>
            )}
            <div className="flex-1 min-h-0">
              <LocationsManager
                datasetType="book"
                description="Books are stored across book directories. Add linked directories to store books on external drives or other locations."
                onError={setLocError}
                onChanged={b.loadBooks}
                countFor={(path) => b.books.filter((bk) => bk.path.startsWith(path)).length}
                refreshToken={locRefresh}
              />
            </div>
          </div>
        )}

        {/* Content — all three views stay mounted to preserve state. */}
        <div className={`${showLocations ? "hidden" : ""} flex-1 flex flex-col min-h-0 overflow-hidden min-w-0`}>
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
