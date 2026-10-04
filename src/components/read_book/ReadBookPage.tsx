"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, Library, Settings2, Zap, PanelLeftClose, PanelLeftOpen, Menu } from "lucide-react";
import type { BookMeta } from "@/lib/read/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import ReadingView from "./ReadingView";
import BookManager from "./BookManager";
import BookAdvanced from "./BookAdvanced";

type View = "read" | "manage" | "advanced";

export default function ReadBookPage() {
  const [books, setBooks] = useState<BookMeta[]>([]);
  const [view, setView] = useState<View>("read");
  const [sidebarVisible, setSidebarVisible] = useState(true);
  // Deferred so SSR/prerender HTML matches the first client paint (see
  // Sidebar.tsx / WikiPage.tsx). On mobile the TOC is a slide-in drawer that
  // starts closed.
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

  useEffect(() => {
    const m = isMobileApp();
    setMobile(m);
    if (m) setSidebarVisible(false);
  }, []);

  const tabClass = (active: boolean) =>
    `flex items-center gap-2 ${mobile ? "p-2" : "px-3 py-1.5"} rounded-md text-sm font-medium cursor-pointer transition-colors ${
      active
        ? "bg-accent-bg text-white"
        : "text-text-secondary border border-border-default hover:bg-bg-hover"
    }`;

  return (
    <main className="flex-1 flex flex-col h-full min-h-0 min-w-0">
      {/* Header */}
      <div className={`p-4 border-b border-border-default flex items-center ${mobile ? "gap-2" : "gap-4"} shrink-0`}>
        <h1 className="text-lg font-semibold flex items-center gap-2 text-text-primary">
          <BookOpen size={20} />{!mobile && " Read a Book"}
        </h1>
        {mobile ? (
          view === "read" && (
            <button
              className="p-1.5 rounded-md text-text-secondary hover:bg-bg-hover cursor-pointer"
              title="Open book / chapter list"
              aria-label="Open book / chapter list"
              onClick={() => setSidebarVisible((v) => !v)}
            >
              <Menu size={18} />
            </button>
          )
        ) : (
          <button
            className="p-1.5 rounded-md text-text-secondary hover:bg-bg-hover cursor-pointer"
            title={sidebarVisible ? "Hide sidebar" : "Show sidebar"}
            onClick={() => setSidebarVisible((v) => !v)}
          >
            {sidebarVisible ? <PanelLeftClose size={18} /> : <PanelLeftOpen size={18} />}
          </button>
        )}
        <div className="ml-auto flex items-center gap-2">
          <button className={tabClass(view === "read")} title="Read" aria-label="Read" onClick={() => setView("read")}>
            <Library size={16} />{!mobile && " Read"}
          </button>
          <button className={tabClass(view === "manage")} title="Manage" aria-label="Manage" onClick={() => setView("manage")}>
            <Settings2 size={16} />{!mobile && " Manage"}
          </button>
          <button className={tabClass(view === "advanced")} title="Advanced" aria-label="Advanced" onClick={() => setView("advanced")}>
            <Zap size={16} />{!mobile && " Advanced"}
          </button>
        </div>
      </div>

      {/* Content — both views stay mounted to preserve state */}
      <div className="flex-1 min-h-0 w-full flex flex-col min-w-0">
        <div style={{ display: view === "read" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-hidden min-w-0">
          <ReadingView
            books={books}
            sidebarVisible={sidebarVisible}
            onCloseSidebar={() => setSidebarVisible(false)}
          />
        </div>
        <div style={{ display: view === "manage" ? "block" : "none" }} className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 min-w-0">
          <BookManager books={books} onBooksChanged={loadBooks} />
        </div>
        <div style={{ display: view === "advanced" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 min-w-0">
          <BookAdvanced books={books} />
        </div>
      </div>
    </main>
  );
}
