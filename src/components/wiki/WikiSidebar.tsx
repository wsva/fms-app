"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { ChevronRight, ChevronDown, Folder, FileText, RefreshCw, Link, Plus, X } from "lucide-react";
import { logError } from "@/lib/logger";
import { isMobileApp } from "@/lib/platform";

interface WikiEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_linked: boolean;
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

// On the thin client, wiki commands are proxied to the PC; an unreachable PC is
// the normal offline state, so don't pop a dialog — the error is already logged.
function isPcUnreachable(err: unknown): boolean {
  return isMobileApp() && String(err).includes("Cannot reach PC");
}

export default function WikiSidebar({ wikiDir, selectedFile, onFileSelect }: WikiSidebarProps) {
  const [entries, setEntries] = useState<WikiEntry[]>([]);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [dirContents, setDirContents] = useState<Record<string, WikiEntry[]>>({});
  const [loading, setLoading] = useState(false);
  const [mobile, setMobile] = useState(false);
  const [showAddDialog, setShowAddDialog] = useState(false);
  const [newDirName, setNewDirName] = useState("");
  const [newDirPath, setNewDirPath] = useState("");

  const loadRootEntries = useCallback(async () => {
    if (!isTauri() || !wikiDir) return;
    setLoading(true);
    try {
      const result = await invoke<WikiEntry[]>("wiki_list_dirs");
      setEntries(result);
    } catch (err) {
      const errorMsg = `Failed to load wiki entries: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      if (!isPcUnreachable(err)) await message(errorMsg, { title: "Error", kind: "error" });
    } finally {
      setLoading(false);
    }
  }, [wikiDir]);

  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  useEffect(() => {
    loadRootEntries();
  }, [loadRootEntries]);

  const loadDirContents = async (dirPath: string) => {
    if (!isTauri()) return;
    try {
      const result = await invoke<WikiEntry[]>("wiki_list_dir", { path: dirPath });
      setDirContents((prev) => ({ ...prev, [dirPath]: result }));
    } catch (err) {
      const errorMsg = `Failed to load directory contents: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      if (!isPcUnreachable(err)) await message(errorMsg, { title: "Error", kind: "error" });
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
              {entry.is_linked ? (
                <Link size={14} className="shrink-0 text-blue-500" />
              ) : (
                <Folder size={14} className="shrink-0 text-accent" />
              )}
            </>
          ) : (
            <>
              <span className="w-[14px] shrink-0" />
              <FileText size={14} className="shrink-0 text-text-tertiary" />
            </>
          )}
          <span className="truncate flex-1">{entry.name}</span>
          {!mobile && entry.is_linked && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                handleRemoveLinkedDir(entry.path);
              }}
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-text-primary transition-colors"
              title="Remove linked directory"
            >
              <X size={12} />
            </button>
          )}
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

  const handlePickFolder = async () => {
    if (!isTauri()) return;
    try {
      const path = await invoke<string>("settings_pick_folder", { field: "wiki_dir" });
      setNewDirPath(path);
      // Use the last segment as default name
      const name = path.split(/[\\/]/).filter(Boolean).pop() || "linked";
      setNewDirName(name);
    } catch (err) {
      const errorMsg = `Failed to pick folder: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      await message(errorMsg, { title: "Error", kind: "error" });
    }
  };

  const handleAddLinkedDir = async () => {
    if (!isTauri() || !newDirName.trim() || !newDirPath.trim()) return;
    try {
      await invoke("wiki_add_dir", { name: newDirName.trim(), path: newDirPath.trim() });
      setShowAddDialog(false);
      setNewDirName("");
      setNewDirPath("");
      await handleRefresh();
    } catch (err) {
      const errorMsg = `Failed to add linked directory: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      await message(errorMsg, { title: "Error", kind: "error" });
    }
  };

  const handleRemoveLinkedDir = async (path: string) => {
    if (!isTauri()) return;
    const confirmed = await ask(`Remove linked directory?\n${path}\n\nThe directory itself will not be deleted.`, { title: "Remove Linked Directory", kind: "warning" });
    if (!confirmed) return;
    try {
      await invoke("wiki_remove_dir", { path });
      await handleRefresh();
    } catch (err) {
      const errorMsg = `Failed to remove linked directory: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      await message(errorMsg, { title: "Error", kind: "error" });
    }
  };

  return (
    <div className="flex flex-col h-full min-h-0 bg-bg-card border-r border-border-default">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border-default">
        <h3 className="text-sm font-semibold text-text-primary">Wiki</h3>
        <div className="flex items-center gap-1">
          {!mobile && (
            <button
              onClick={() => setShowAddDialog(true)}
              className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
              title="Link external directory"
            >
              <Plus size={14} />
            </button>
          )}
          <button
            onClick={handleRefresh}
            disabled={loading}
            className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-50"
            title="Refresh wiki directory"
          >
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
          </button>
        </div>
      </div>
      {showAddDialog && (
        <div className="px-3 py-2 border-b border-border-default bg-bg-body space-y-2">
          <div className="text-xs font-semibold text-text-primary">Link External Directory</div>
          <div>
            <input
              type="text"
              placeholder="Display name (letters, numbers, hyphens, underscores)"
              value={newDirName}
              onChange={(e) => {
                // Only allow URL-compatible characters: letters, numbers, hyphens, underscores
                const value = e.target.value.replace(/[^a-zA-Z0-9_-]/g, "");
                setNewDirName(value);
              }}
              className="w-full px-2 py-1 text-sm bg-bg-input border border-border-default rounded-md outline-none focus:border-accent"
            />
          </div>
          <button
            onClick={handlePickFolder}
            className="w-full px-2 py-2 text-sm bg-bg-input border border-border-default rounded-md hover:bg-bg-hover transition-colors text-left truncate"
          >
            {newDirPath || "Select directory..."}
          </button>
          <div className="flex gap-2">
            <button
              onClick={handleAddLinkedDir}
              disabled={!newDirName.trim() || !newDirPath.trim()}
              className="flex-1 px-2 py-1 text-xs bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              Add
            </button>
            <button
              onClick={() => {
                setShowAddDialog(false);
                setNewDirName("");
                setNewDirPath("");
              }}
              className="flex-1 px-2 py-1 text-xs bg-bg-input border border-border-default rounded-md hover:bg-bg-hover transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
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
