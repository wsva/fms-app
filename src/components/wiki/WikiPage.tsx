"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import {
  RefreshCw, BookOpen, ArrowLeft, ArrowRight, Cloud, Download, Pencil, Eye,
} from "lucide-react";
import WikiSidebar from "./WikiSidebar";
import WikiSearch from "./WikiSearch";
import MdEditor from "./MdEditor";
import MarkdownViewer from "./markdown/markdown";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import {
  useCollapsibleSidebar,
  CollapsibleSidebar,
  SidebarToggleButton,
} from "@/components/layout/CollapsibleSidebar";
import {
  WikiDatasetSummary, HubWikiList, Selection, WikiSource,
} from "@/lib/wiki/types";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface AppSettings {
  wiki_dir: string;
  [key: string]: unknown;
}

// Mirrors the backend `WikiEntry`; used on mobile to resolve the PC wiki root
// (the non-linked entry) as the base for in-content link / deep-link resolution.
interface WikiDirEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_linked: boolean;
  modified: string | null;
}

// Strip a dataset-root-relative wiki link to a clean rel path.
function normalizeWikiRel(relativePath: string): string {
  return relativePath.replace(/^\.?\//, "").replace(/\\/g, "/");
}

export default function WikiPage() {
  const [wikiDir, setWikiDir] = useState<string>("");
  const [selection, setSelection] = useState<Selection | null>(null);
  const [fileContent, setFileContent] = useState<string>("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isIndexing, setIsIndexing] = useState(false);

  // Hub read-only browse + local wiki datasets.
  const [hubMode, setHubMode] = useState(false);
  const [localDatasets, setLocalDatasets] = useState<WikiDatasetSummary[]>([]);
  const [hubDatasets, setHubDatasets] = useState<WikiDatasetSummary[]>([]);
  const [isHub, setIsHub] = useState(false);

  // Dual-mode content: rendered HTML view vs. the ported markdown editor.
  // Editing is only ever offered for a locally stored wiki dataset file.
  const [editing, setEditing] = useState(false);
  const [editContent, setEditContent] = useState("");
  const [saving, setSaving] = useState(false);

  const sidebar = useCollapsibleSidebar({
    storageKey: "wiki-sidebar-width",
    defaultWidth: 240,
    minWidth: 160,
    maxWidth: 420,
  });
  const mobile = sidebar.mobile;

  // Navigation history (now over typed selections, not just paths).
  const [history, setHistory] = useState<Selection[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const historyIndexRef = useRef(-1);
  const isHistoryNavRef = useRef(false);

  useEffect(() => {
    historyIndexRef.current = historyIndex;
  }, [historyIndex]);

  // Resolve the wiki directory used as the base for link / deep-link handling.
  useEffect(() => {
    if (!isTauri()) return;
    const loadWikiDir = async () => {
      try {
        if (isMobileApp()) {
          const dirs = await invoke<WikiDirEntry[]>("wiki_list_dirs");
          const root = dirs.find((d) => !d.is_linked);
          setWikiDir(root ? root.path : "");
        } else {
          const settings = await invoke<AppSettings>("settings_get");
          setWikiDir(settings.wiki_dir || "");
        }
      } catch (err) {
        logError(`Failed to load wiki directory: ${err instanceof Error ? err.message : String(err)}`, "wiki");
      }
    };
    loadWikiDir();
  }, []);

  // Load locally stored wiki datasets (downloaded or hub-owned). Cross-platform.
  const loadLocalDatasets = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setLocalDatasets(await invoke<WikiDatasetSummary[]>("wiki_dataset_list"));
    } catch (err) {
      logError(`Failed to load wiki datasets: ${err instanceof Error ? err.message : String(err)}`, "wiki");
    }
  }, []);

  useEffect(() => {
    loadLocalDatasets();
  }, [loadLocalDatasets]);

  // Detect the hub role so the UI can hide pointless hub-browse of self.
  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ role: string }>("sync_status")
      .then((s) => setIsHub(s.role === "hub"))
      .catch(() => setIsHub(false));
  }, []);

  // Read one file for a given source, dispatching to the right backend command.
  const readSource = useCallback(async (source: WikiSource): Promise<string> => {
    switch (source.kind) {
      case "legacy":
        return await invoke<string>("wiki_read_file", { path: source.path });
      case "dataset":
        return await invoke<string>("wiki_dataset_read_file", { uuid: source.uuid, rel: source.rel });
      case "hub-dataset":
        return await invoke<string>("wiki_hub_read_file", { uuid: source.uuid, rel: source.rel });
    }
  }, []);

  // Load a selection's content, leaving view mode, and push navigation history.
  const loadSelection = useCallback(async (sel: Selection, pushHistory: boolean) => {
    if (!isTauri()) return;
    if (sel.source.kind !== "legacy" && sel.source.rel === "") {
      // A dataset root placeholder (used to clear a deleted file) — nothing to read.
      setSelection(null);
      setFileContent("");
      setEditing(false);
      return;
    }
    setIsLoading(true);
    setError(null);
    setEditing(false);
    try {
      const content = await readSource(sel.source);
      setFileContent(content);
      setSelection(sel);
      if (pushHistory && !isHistoryNavRef.current) {
        setHistory((prev) => {
          const currentIndex = historyIndexRef.current;
          const newHistory = prev.slice(0, currentIndex + 1);
          newHistory.push(sel);
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
      setSelection(sel);
    } finally {
      setIsLoading(false);
    }
  }, [readSource]);

  const handleFileSelect = useCallback((sel: Selection) => {
    sidebar.closeOnSelect();
    loadSelection(sel, true);
  }, [sidebar, loadSelection]);

  const restoreFromHistory = useCallback((sel: Selection) => {
    isHistoryNavRef.current = true;
    loadSelection(sel, false);
    isHistoryNavRef.current = false;
  }, [loadSelection]);

  const goBack = useCallback(() => {
    if (historyIndex <= 0) return;
    const sel = history[historyIndex - 1];
    setHistoryIndex(historyIndex - 1);
    restoreFromHistory(sel);
  }, [history, historyIndex, restoreFromHistory]);

  const goForward = useCallback(() => {
    if (historyIndex >= history.length - 1) return;
    const sel = history[historyIndex + 1];
    setHistoryIndex(historyIndex + 1);
    restoreFromHistory(sel);
  }, [history, historyIndex, restoreFromHistory]);

  // Keyboard shortcuts: Alt+Left = back, Alt+Right = forward. Suppressed while
  // the editor is mounted so its own history/selection keys win.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (editing) return;
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
  }, [goBack, goForward, editing]);

  // Listen for wiki deep link navigation (fms-app://wiki/path/to/file.md).
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<string>("wiki-navigate", (event) => {
      const relativePath = event.payload;
      if (!relativePath || !wikiDir) return;
      loadSelection({ source: { kind: "legacy", path: wikiDir + "/" + relativePath }, label: relativePath }, true);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [wikiDir, loadSelection]);

  // Handle wiki link clicks within markdown content. Resolve against the
  // selected dataset dir (or the hub root in Hub mode), not only wiki_dir.
  const handleWikiLink = useCallback((relativePath: string) => {
    const rel = normalizeWikiRel(relativePath);
    const cur = selection?.source;
    if (cur && cur.kind === "dataset") {
      loadSelection({ source: { kind: "dataset", uuid: cur.uuid, rel }, label: rel.split("/").pop() || rel }, true);
      return;
    }
    if (cur && cur.kind === "hub-dataset") {
      loadSelection({ source: { kind: "hub-dataset", uuid: cur.uuid, rel, name: cur.name }, label: rel.split("/").pop() || rel }, true);
      return;
    }
    if (wikiDir) {
      loadSelection({ source: { kind: "legacy", path: wikiDir + "/" + rel }, label: rel.split("/").pop() || rel }, true);
    }
  }, [selection, wikiDir, loadSelection]);

  // Whether the current selection is an editable local dataset file.
  const canEdit =
    !hubMode && selection?.source.kind === "dataset" && selection.source.rel !== "";

  const startEdit = () => {
    setEditContent(fileContent);
    setEditing(true);
  };

  const handleSave = async () => {
    if (!selection || selection.source.kind !== "dataset") return;
    setSaving(true);
    try {
      await invoke("wiki_dataset_write_file", {
        uuid: selection.source.uuid,
        rel: selection.source.rel,
        content: editContent,
      });
      setFileContent(editContent);
      setEditing(false);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Save failed", kind: "error" });
    } finally {
      setSaving(false);
    }
  };

  // Download a hub dataset to this device (requirement 2), then flip the root
  // to local so Edit becomes available.
  const handleDownload = useCallback(async (uuid: string, rel: string) => {
    if (!isTauri()) return;
    setIsLoading(true);
    try {
      await invoke("dataset_sync_snapshot", { uuid });
      await loadLocalDatasets();
      setHubMode(false);
      setSelection(null);
      // Re-open the same file from the now-local dataset copy.
      const ds = (await invoke<WikiDatasetSummary[]>("wiki_dataset_list")).find((d) => d.uuid === uuid);
      if (ds) {
        loadSelection({ source: { kind: "dataset", uuid, rel }, label: rel.split("/").pop() || ds.name }, true);
      }
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Download failed", kind: "error" });
    } finally {
      setIsLoading(false);
    }
  }, [loadLocalDatasets, loadSelection]);

  // Enter / leave the read-only hub browse view.
  const toggleHubMode = useCallback(async () => {
    if (hubMode) {
      setHubMode(false);
      return;
    }
    if (!isTauri()) return;
    setIsLoading(true);
    try {
      const list = await invoke<HubWikiList>("wiki_hub_list");
      setHubDatasets(list.datasets || []);
      setSelection(null);
      setFileContent("");
      setHubMode(true);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Cannot reach hub", kind: "error" });
    } finally {
      setIsLoading(false);
    }
  }, [hubMode]);

  const handleIndexWiki = async () => {
    if (!isTauri()) return;
    setIsIndexing(true);
    try {
      // When a local dataset file is open, index just that dataset; otherwise
      // refresh the legacy wiki index (unchanged behavior).
      const sel = selection?.source;
      if (sel && sel.kind === "dataset") {
        const count = await invoke<number>("wiki_dataset_index", { uuid: sel.uuid });
        console.log(`Indexed ${count} dataset files`);
      } else {
        const count = await invoke<number>("wiki_index");
        console.log(`Indexed ${count} files`);
      }
    } catch (err) {
      console.error("Failed to index wiki:", err);
    } finally {
      setIsIndexing(false);
    }
  };

  // Sidebar dataset tree source: hub datasets while browsing, else local.
  const treeDatasets = hubMode ? hubDatasets : localDatasets;

  // Search mode follows the current selection's scope.
  const searchMode: { kind: "legacy" } | { kind: "dataset"; uuid: string } | { kind: "hub"; uuid: string } = (() => {
    const src = selection?.source;
    if (hubMode && src && src.kind === "hub-dataset") return { kind: "hub", uuid: src.uuid };
    if (!hubMode && src && src.kind === "dataset") return { kind: "dataset", uuid: src.uuid };
    return { kind: "legacy" };
  })();

  const handleSearchResult = useCallback((source: WikiSource) => {
    const label = source.kind === "legacy"
      ? source.path.split(/[\\/]/).pop() || source.path
      : source.rel.split("/").pop() || source.rel;
    handleFileSelect({ source, label });
  }, [handleFileSelect]);

  const headerLabel = selection
    ? (selection.label || (selection.source.kind === "legacy"
        ? selection.source.path.split(/[\\/]/).pop()
        : selection.source.rel.split("/").pop()))
    : "";

  // Narrow once so JSX callbacks see a concrete hub-dataset source.
  const hubSrc = selection && selection.source.kind === "hub-dataset" ? selection.source : null;

  return (
    <div className="flex h-full w-full bg-bg-body min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Top bar with sidebar toggle, navigation, search and controls */}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-border-default bg-bg-card">
          <SidebarToggleButton sidebar={sidebar} title="Show/hide wiki files" />
          <div className="flex items-center gap-1">
            <button
              onClick={goBack}
              disabled={historyIndex <= 0 || editing}
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
              title="Back (Alt+Left)"
            >
              <ArrowLeft size={16} />
            </button>
            <button
              onClick={goForward}
              disabled={historyIndex >= history.length - 1 || editing}
              className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
              title="Forward (Alt+Right)"
            >
              <ArrowRight size={16} />
            </button>
          </div>
          <div className="flex-1 max-w-md">
            <WikiSearch mode={searchMode} onResultClick={handleSearchResult} />
          </div>

          {/* Hub browse toggle — read-only view of the paired hub's wiki. Hidden
              for the hub itself (it already sees everything locally). */}
          {!isHub && (
            <button
              onClick={toggleHubMode}
              className={`flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md transition-colors ${
                hubMode
                  ? "bg-accent text-white hover:bg-accent/90"
                  : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
              }`}
              title={hubMode ? "Leave hub browse" : "Browse the hub's wiki (read-only)"}
            >
              <Cloud size={14} />
              <span>{hubMode ? "Local" : "Hub"}</span>
            </button>
          )}

          {/* Indexing is a local operation; hidden in hub read-only mode. */}
          {!mobile && !hubMode && (
            <button
              onClick={handleIndexWiki}
              disabled={isIndexing}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors disabled:opacity-50"
              title="Re-index wiki files for search"
            >
              <RefreshCw size={14} className={isIndexing ? "animate-spin" : ""} />
              <span>{isIndexing ? "Indexing..." : "Index"}</span>
            </button>
          )}
        </div>

        {/* Hub read-only banner */}
        {hubMode && (
          <div className="px-4 py-1.5 text-xs bg-accent/10 text-accent border-b border-border-default flex items-center gap-2">
            <Cloud size={13} />
            <span>Hub view — read-only. Download a dataset to edit it locally.</span>
          </div>
        )}

        <div className="relative flex flex-1 min-h-0 overflow-hidden">
          <CollapsibleSidebar sidebar={sidebar}>
            <WikiSidebar
              wikiDir={wikiDir}
              selection={selection}
              onFileSelect={handleFileSelect}
              hubMode={hubMode}
              datasets={treeDatasets}
              canMutateRoots={!mobile && !hubMode}
              onDatasetsChanged={loadLocalDatasets}
            />
          </CollapsibleSidebar>

          {/* Content area */}
          <div className="flex-1 overflow-y-auto min-h-0">
            {isLoading ? (
              <div className="flex items-center justify-center h-full">
                <div className="text-text-tertiary">Loading...</div>
              </div>
            ) : error ? (
              <div className="flex items-center justify-center h-full p-6">
                <div className="max-w-lg w-full text-center">
                  <div className="text-error-text bg-error-bg px-4 py-2 rounded-lg inline-block mb-4">
                    Error: {error}
                  </div>
                  {selection?.source.kind === "hub-dataset" && (
                    <div>
                      <p className="text-sm text-text-secondary mb-3">
                        This file lives on the hub. Download the dataset to read it locally.
                      </p>
                      <button
                        onClick={() => hubSrc && handleDownload(hubSrc.uuid, hubSrc.rel)}
                        className="inline-flex items-center gap-1.5 px-4 py-2 text-sm bg-accent text-white rounded-md hover:bg-accent/90"
                      >
                        <Download size={14} /> Download dataset
                      </button>
                    </div>
                  )}
                </div>
              </div>
            ) : selection && selection.source.kind !== "legacy" && selection.source.rel === "" ? null
              : selection ? (
              <div className="h-full flex flex-col min-h-0">
                {/* Content header: breadcrumb + View/Edit / Download */}
                <div className="flex items-center gap-2 px-6 py-2 border-b border-border-default bg-bg-card shrink-0">
                  <div className="text-xs text-text-tertiary truncate flex-1">{headerLabel}</div>

                  {hubSrc && (
                    <button
                      onClick={() => handleDownload(hubSrc.uuid, hubSrc.rel)}
                      disabled={isLoading}
                      className="flex items-center gap-1.5 px-3 py-1 text-sm bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50"
                      title="Download this dataset to edit locally"
                    >
                      <Download size={14} /> Download
                    </button>
                  )}

                  {canEdit && !editing && (
                    <button
                      onClick={startEdit}
                      className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                      title="Edit this file"
                    >
                      <Pencil size={14} /> Edit
                    </button>
                  )}

                  {canEdit && editing && (
                    <button
                      onClick={() => setEditing(false)}
                      className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                      title="Return to view"
                    >
                      <Eye size={14} /> View
                    </button>
                  )}
                </div>

                {/* Body: editor (edit mode) or rendered markdown (view mode) */}
                <div className="flex-1 overflow-y-auto min-h-0">
                  {editing ? (
                    <div className="px-4 py-3">
                      <MdEditor
                        value={editContent}
                        onChange={setEditContent}
                        onSave={handleSave}
                        saving={saving}
                        dirty={editContent !== fileContent}
                        label={headerLabel}
                      />
                    </div>
                  ) : (
                    <div className="px-6 pt-6 pb-[50vh]">
                      <MarkdownViewer content={fileContent} withTOC={true} onWikiLink={handleWikiLink} />
                    </div>
                  )}
                </div>
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
    </div>
  );
}
