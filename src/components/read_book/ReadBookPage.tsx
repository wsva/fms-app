"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, Library, Settings2, Zap } from "lucide-react";
import type { BookMeta } from "@/lib/read/types";
import { isTauri } from "@/lib/tauri";
import ReadingView from "./ReadingView";
import BookManager from "./BookManager";
import BookAdvanced from "./BookAdvanced";

type View = "read" | "manage" | "advanced";

export default function ReadBookPage() {
  const [books, setBooks] = useState<BookMeta[]>([]);
  const [view, setView] = useState<View>("read");

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

  const tabClass = (active: boolean) =>
    `flex items-center gap-2 px-3 py-1.5 rounded-md text-sm font-medium cursor-pointer transition-colors ${
      active
        ? "bg-accent-bg text-white"
        : "text-text-secondary border border-border-default hover:bg-bg-hover"
    }`;

  return (
    <main className="flex-1 flex flex-col h-full min-h-0">
      {/* Header */}
      <div className="p-4 border-b border-border-default flex items-center gap-4 shrink-0">
        <h1 className="text-lg font-semibold flex items-center gap-2 text-text-primary">
          <BookOpen size={20} /> Read a Book
        </h1>
        <div className="ml-auto flex items-center gap-2">
          <button className={tabClass(view === "read")} onClick={() => setView("read")}>
            <Library size={16} /> Read
          </button>
          <button className={tabClass(view === "manage")} onClick={() => setView("manage")}>
            <Settings2 size={16} /> Manage
          </button>
          <button className={tabClass(view === "advanced")} onClick={() => setView("advanced")}>
            <Zap size={16} /> Advanced
          </button>
        </div>
      </div>

      {/* Content — both views stay mounted to preserve state */}
      <div className="flex-1 min-h-0 w-full flex flex-col">
        <div style={{ display: view === "read" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-hidden">
          <ReadingView books={books} />
        </div>
        <div style={{ display: view === "manage" ? "block" : "none" }} className="flex-1 min-h-0 overflow-y-auto p-4">
          <BookManager books={books} onBooksChanged={loadBooks} />
        </div>
        <div style={{ display: view === "advanced" ? "flex" : "none" }} className="flex-1 min-h-0 overflow-y-auto p-4">
          <BookAdvanced books={books} />
        </div>
      </div>
    </main>
  );
}
