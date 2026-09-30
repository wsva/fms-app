"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronRight, ChevronDown, Folder, FileText, RefreshCw } from "lucide-react";

interface WikiEntry {
  name: string;
  path: string;
  is_dir: boolean;
  modified: string | null;
}

interface WikiSidebarProps {
  wikiDir: string;
  selectedFile: string | null;
  onFileSelect: (path: string) => void;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export default function WikiSidebar({ wikiDir, selectedFile, onFileSelect }: WikiSidebarProps) {
  const [entries, setEntries] = useState<WikiEntry[]>([]);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [dirContents, setDirContents] = useState<Record<string, WikiEntry[]>>({});
  const [loading, setLoading] = useState(false);

  const loadRootEntries = useCallback(async () => {
    if (!isTauri() || !wikiDir) return;
    setLoading(true);
    try {
      const result = await invoke<WikiEntry[]>("wiki_list_dirs");
      setEntries(result);
    } catch (err) {
      console.error("Failed to load wiki entries:", err);
    } finally {
      setLoading(false);
    }
  }, [wikiDir]);

  useEffect(() => {
    loadRootEntries();
  }, [loadRootEntries]);

  const loadDirContents = async (dirPath: string) => {
    if (!isTauri()) return;
    try {
      const result = await invoke<WikiEntry[]>("wiki_list_dir", { path: dirPath });
      setDirContents((prev) => ({ ...prev, [dirPath]: result }));
    } catch (err) {
      console.error("Failed to load directory contents:", err);
    }
  };

  const toggleDir = async (path: string) => {
    const newExpanded = new Set(expandedDirs);
    if (newExpanded.has(path)) {
      newExpanded.delete(path);
    } else {
      newExpanded.add(path);
      // Load contents if not already loaded
      if (!dirContents[path]) {
        await loadDirContents(path);
      }
    }
    setExpandedDirs(newExpanded);
  };

  const renderEntry = (entry: WikiEntry, depth: number = 0) => {
    const isExpanded = expandedDirs.has(entry.path);
    const isSelected = selectedFile === entry.path;
    const children = dirContents[entry.path];

    return (
      <div key={entry.path}>
        <div
          className={`flex items-center gap-1 px-2 py-1 cursor-pointer rounded-md text-sm transition-colors ${
            isSelected
              ? "bg-accent/15 text-accent"
              : "hover:bg-bg-hover text-text-primary"
          }`}
          style={{ paddingLeft: `${depth * 16 + 8}px` }}
          onClick={() => {
            if (entry.is_dir) {
              toggleDir(entry.path);
            } else {
              onFileSelect(entry.path);
            }
          }}
        >
          {entry.is_dir ? (
            <>
              {isExpanded ? (
                <ChevronDown size={14} className="shrink-0 text-text-tertiary" />
              ) : (
                <ChevronRight size={14} className="shrink-0 text-text-tertiary" />
              )}
              <Folder size={14} className="shrink-0 text-accent" />
            </>
          ) : (
            <>
              <span className="w-[14px] shrink-0" />
              <FileText size={14} className="shrink-0 text-text-tertiary" />
            </>
          )}
          <span className="truncate">{entry.name}</span>
        </div>
        {entry.is_dir && isExpanded && children && (
          <div>
            {children.map((child) => renderEntry(child, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  const handleRefresh = async () => {
    // Clear cached directory contents so expanded dirs get refreshed too
    setDirContents({});
    setExpandedDirs(new Set());
    await loadRootEntries();
  };

  return (
    <div className="flex flex-col h-full min-h-0 bg-bg-card border-r border-border-default">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border-default">
        <h3 className="text-sm font-semibold text-text-primary">Wiki</h3>
        <button
          onClick={handleRefresh}
          disabled={loading}
          className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-50"
          title="Refresh wiki directory"
        >
          <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
        </button>
      </div>
      <div className="flex-1 overflow-y-auto min-h-0 p-2">
        {loading ? (
          <div className="text-sm text-text-tertiary text-center py-4">Loading...</div>
        ) : entries.length === 0 ? (
          <div className="text-sm text-text-tertiary text-center py-4">
            No files found.
            <br />
            <span className="text-xs">Add markdown files to your wiki directory.</span>
          </div>
        ) : (
          entries.map((entry) => renderEntry(entry))
        )}
      </div>
    </div>
  );
}
