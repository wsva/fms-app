"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw, BookOpen, ArrowLeft, ArrowRight } from "lucide-react";
import WikiSidebar from "./WikiSidebar";
import WikiSearch from "./WikiSearch";
import MarkdownViewer from "./markdown/markdown";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface AppSettings {
  wiki_dir: string;
  [key: string]: unknown;
}

const WIKI_SIDEBAR_WIDTH_KEY = "wiki-sidebar-width";
const DEFAULT_SIDEBAR_WIDTH = 224; // w-56 = 14rem = 224px
const MIN_SIDEBAR_WIDTH = 150;
const MAX_SIDEBAR_WIDTH = 400;

export default function WikiPage() {
  const [wikiDir, setWikiDir] = useState<string>("");
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [fileContent, setFileContent] = useState<string>("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isIndexing, setIsIndexing] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const handleSidebarDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = sidebarWidth;
      const onMove = (ev: MouseEvent) =>
        setSidebarWidth(Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, startWidth + ev.clientX - startX)));
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        localStorage.setItem(WIKI_SIDEBAR_WIDTH_KEY, String(sidebarWidth));
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [sidebarWidth]
  );

  // Navigation history
  const [history, setHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const historyIndexRef = useRef(-1);
  const isHistoryNavRef = useRef(false);

  // Keep ref in sync with state
  useEffect(() => {
    historyIndexRef.current = historyIndex;
  }, [historyIndex]);

  // Load settings to get wiki directory
  useEffect(() => {
    if (!isTauri()) return;

    const loadSettings = async () => {
      try {
        const settings = await invoke<AppSettings>("settings_get");
        setWikiDir(settings.wiki_dir || "");
      } catch (err) {
        console.error("Failed to load settings:", err);
      }
    };

    loadSettings();
  }, []);

  // Load file content - also manages navigation history
  const loadFileContent = useCallback(async (path: string) => {
    if (!isTauri()) return;
    setIsLoading(true);
    setError(null);
    try {
      const content = await invoke<string>("wiki_read_file", { path });
      setFileContent(content);
      setSelectedFile(path);

      // Update navigation history (skip if navigating via back/forward)
      if (!isHistoryNavRef.current) {
        setHistory(prev => {
          const currentIndex = historyIndexRef.current;
          // Truncate forward history and push new entry
          const newHistory = prev.slice(0, currentIndex + 1);
          newHistory.push(path);
          const newIndex = newHistory.length - 1;
          setHistoryIndex(newIndex);
          historyIndexRef.current = newIndex;
          return newHistory;
        });
      }
    } catch (err) {
      console.error("Failed to read file:", err);
      setError(err instanceof Error ? err.message : String(err));
      setFileContent("");
    } finally {
      setIsLoading(false);
    }
  }, []);

  const goBack = useCallback(() => {
    if (historyIndex <= 0) return;
    isHistoryNavRef.current = true;
    const path = history[historyIndex - 1];
    setHistoryIndex(historyIndex - 1);
    // Load file without pushing to history
    setIsLoading(true);
    setError(null);
    invoke<string>("wiki_read_file", { path })
      .then(content => {
        setFileContent(content);
        setSelectedFile(path);
      })
      .catch(err => {
        setError(err instanceof Error ? err.message : String(err));
        setFileContent("");
      })
      .finally(() => {
        setIsLoading(false);
        isHistoryNavRef.current = false;
      });
  }, [history, historyIndex]);

  const goForward = useCallback(() => {
    if (historyIndex >= history.length - 1) return;
    isHistoryNavRef.current = true;
    const path = history[historyIndex + 1];
    setHistoryIndex(historyIndex + 1);
    setIsLoading(true);
    setError(null);
    invoke<string>("wiki_read_file", { path })
      .then(content => {
        setFileContent(content);
        setSelectedFile(path);
      })
      .catch(err => {
        setError(err instanceof Error ? err.message : String(err));
        setFileContent("");
      })
      .finally(() => {
        setIsLoading(false);
        isHistoryNavRef.current = false;
      });
  }, [history, historyIndex]);

  // Keyboard shortcuts: Alt+Left = back, Alt+Right = forward
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.altKey && e.key === "ArrowLeft") {
        e.preventDefault();
        goBack();
      } else if (e.altKey && e.key === "ArrowRight") {
        e.preventDefault();
        goForward();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [goBack, goForward]);

  const handleFileSelect = (path: string) => {
    loadFileContent(path);
  };

  // Listen for wiki deep link navigation (fms-app://wiki/path/to/file.md)
  useEffect(() => {
    if (!isTauri()) return;

    const unlisten = listen<string>("wiki-navigate", (event) => {
      const relativePath = event.payload;
      if (!relativePath || !wikiDir) return;
      // Resolve relative path to absolute path
      const absolutePath = wikiDir + "/" + relativePath;
      loadFileContent(absolutePath);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [wikiDir, loadFileContent]);

  // Handle wiki link clicks within markdown content
  const handleWikiLink = useCallback((relativePath: string) => {
    if (!wikiDir) return;
    const absolutePath = wikiDir + "/" + relativePath;
    loadFileContent(absolutePath);
  }, [wikiDir, loadFileContent]);

  const handleIndexWiki = async () => {
    if (!isTauri()) return;
    setIsIndexing(true);
    try {
      const count = await invoke<number>("wiki_index");
      console.log(`Indexed ${count} files`);
    } catch (err) {
      console.error("Failed to index wiki:", err);
    } finally {
      setIsIndexing(false);
    }
  };

  const getFileName = (path: string) => {
    return path.split(/[\\/]/).pop() || path;
  };

  // Load saved sidebar width from localStorage
  useEffect(() => {
    const saved = localStorage.getItem(WIKI_SIDEBAR_WIDTH_KEY);
    if (saved) {
      const w = parseInt(saved, 10);
      if (w >= MIN_SIDEBAR_WIDTH && w <= MAX_SIDEBAR_WIDTH) setSidebarWidth(w);
    }
  }, []);

  return (
    <div className="flex h-full w-full bg-bg-body">
      {/* Sidebar - file tree */}
      <div
        className="relative shrink-0 min-h-0"
        style={{ width: sidebarWidth }}
      >
        <WikiSidebar
          wikiDir={wikiDir}
          selectedFile={selectedFile}
          onFileSelect={handleFileSelect}
        />
        {/* Resize handle */}
        <div
          className="absolute top-0 right-0 h-full w-3 cursor-col-resize flex items-center justify-center group z-10"
          onMouseDown={handleSidebarDrag}
        >
          <div className="w-0.5 h-12 rounded-full bg-border-default group-hover:bg-accent transition-colors" />
        </div>
      </div>

      {/* Main content area */}
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Top bar with navigation, search and controls */}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-border-default bg-bg-card">
          {/* Back/Forward navigation */}
          <div className="flex items-center gap-1">
            <button
              onClick={goBack}
              disabled={historyIndex <= 0}
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
              title="Back (Alt+Left)"
            >
              <ArrowLeft size={16} />
            </button>
            <button
              onClick={goForward}
              disabled={historyIndex >= history.length - 1}
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
              title="Forward (Alt+Right)"
            >
              <ArrowRight size={16} />
            </button>
          </div>
          <div className="flex-1 max-w-md">
            <WikiSearch wikiDir={wikiDir} onResultClick={handleFileSelect} />
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleIndexWiki}
              disabled={isIndexing}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors disabled:opacity-50"
              title="Re-index wiki files for search"
            >
              <RefreshCw size={14} className={isIndexing ? "animate-spin" : ""} />
              <span>{isIndexing ? "Indexing..." : "Index"}</span>
            </button>
          </div>
        </div>

        {/* Content area */}
        <div className="flex-1 overflow-y-auto min-h-0">
          {isLoading ? (
            <div className="flex items-center justify-center h-full">
              <div className="text-text-tertiary">Loading...</div>
            </div>
          ) : error ? (
            <div className="flex items-center justify-center h-full">
              <div className="text-error-text bg-error-bg px-4 py-2 rounded-lg">
                Error: {error}
              </div>
            </div>
          ) : selectedFile ? (
            <div className="px-6 pt-6 pb-[50vh]">
              {/* File path breadcrumb */}
              <div className="text-xs text-text-tertiary mb-4 truncate">
                {getFileName(selectedFile)}
              </div>
              <MarkdownViewer content={fileContent} withTOC={true} onWikiLink={handleWikiLink} />
            </div>
          ) : (
            <div className="flex flex-col items-center justify-center h-full text-text-tertiary">
              <BookOpen size={48} className="mb-4 opacity-30" />
              <p className="text-lg">Select a file to view</p>
              <p className="text-sm mt-2">
                Choose a markdown file from the sidebar or search for content
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
