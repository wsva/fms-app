"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, message } from "@tauri-apps/plugin-dialog";
import {
  ChevronRight, ChevronDown, Folder, FileText, RefreshCw, Link, Plus, X,
  FolderInput, FilePlus, FolderPlus, Trash2, BookMarked,
} from "lucide-react";
import { logError } from "@/lib/logger";
import { isMobileApp } from "@/lib/platform";
import {
  WikiEntry, WikiDatasetSummary, WikiFileEntry, Selection, WikiSource, selectionKey,
} from "@/lib/wiki/types";

interface WikiSidebarProps {
  wikiDir: string;
  selection: Selection | null;
  onFileSelect: (sel: Selection) => void;
  /** Hub read-only mode: the tree shows the paired hub's wiki datasets. */
  hubMode: boolean;
  /** Locally stored wiki datasets (downloaded or hub-owned). Empty in hub mode. */
  datasets: WikiDatasetSummary[];
  /** Whether this device is the hub (desktop, non-follower): gates create/convert. */
  canMutateRoots: boolean;
  /** Called after dataset tree mutations so the page can refresh its dataset list. */
  onDatasetsChanged: () => void;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// On the thin client, wiki commands are proxied to the PC; an unreachable PC is
// the normal offline state, so don't pop a dialog — the error is already logged.
function isPcUnreachable(err: unknown): boolean {
  return isMobileApp() && String(err).includes("Cannot reach PC");
}

// Backend sanitize_rel rejects these; mirror the guard in the UI so a prompt
// can't produce a path the journal would refuse.
function sanitizeName(raw: string): string {
  return raw.replace(/[\\/:*?"<>|]/g, "").replace(/\.\./g, "").trim();
}

function joinRel(dirRel: string, name: string): string {
  return dirRel ? `${dirRel}/${name}` : name;
}

function parentOf(rel: string): string {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

// One inline name-prompt row (browser prompt() is unavailable in webviews and
// the dialog plugin has no prompt variant, so a tiny controlled row is used).
interface PromptState {
  title: string;
  submit: (name: string) => Promise<void>;
}

export default function WikiSidebar({
  wikiDir, selection, onFileSelect, hubMode, datasets, canMutateRoots, onDatasetsChanged,
}: WikiSidebarProps) {
  const [entries, setEntries] = useState<WikiEntry[]>([]);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [dirContents, setDirContents] = useState<Record<string, WikiEntry[]>>({});
  const [loading, setLoading] = useState(false);
  const [mobile, setMobile] = useState(false);
  const [showAddDialog, setShowAddDialog] = useState(false);
  const [newDirName, setNewDirName] = useState("");
  const [newDirPath, setNewDirPath] = useState("");

  // Wiki dataset trees: expanded dirs keyed by `${uuid}:${rel}` and their children.
  const [expandedDsDirs, setExpandedDsDirs] = useState<Set<string>>(new Set());
  const [dsRootOpen, setDsRootOpen] = useState<Set<string>>(new Set());
  const [dsDirContents, setDsDirContents] = useState<Record<string, WikiFileEntry[]>>({});
  const [hubDsDirContents, setHubDsDirContents] = useState<Record<string, WikiFileEntry[]>>({});
  const [prompt, setPrompt] = useState<PromptState | null>(null);
  const [promptValue, setPromptValue] = useState("");
  const [promptBusy, setPromptBusy] = useState(false);

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
    const isSelected = selection?.source.kind === "legacy" && selection.source.path === entry.path;
    const children = dirContents[entry.path];
    return (
      <div key={entry.path}>
        <div
          className={`group flex items-center gap-1 px-2 py-1 cursor-pointer rounded-md text-sm transition-colors ${
            isSelected
              ? "bg-accent/15 text-accent"
              : "hover:bg-bg-hover text-text-primary"
          }`}
          style={{ paddingLeft: `${depth * 16 + 8}px` }}
          onClick={() => {
            if (entry.is_dir) {
              toggleDir(entry.path);
            } else {
              // In hub mode this is a read-only browse of the hub's own roots;
              // the page relays reads through the path-proxied commands.
              onFileSelect({ source: { kind: "legacy", path: entry.path }, label: entry.name });
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
          {!mobile && !hubMode && entry.is_linked && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                handleRemoveLinkedDir(entry.path);
              }}
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-text-primary transition-colors opacity-0 group-hover:opacity-100"
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

  // ---------------------------------------------------------------------------
  // Wiki dataset trees (local, or hub read-only)
  // ---------------------------------------------------------------------------

  const dsKey = (uuid: string, rel: string) => `${uuid}:${rel}`;

  const loadDsDir = async (uuid: string, rel: string) => {
    if (!isTauri()) return;
    try {
      const result = hubMode
        ? await invoke<WikiFileEntry[]>("wiki_hub_list_dir", { uuid, rel })
        : await invoke<WikiFileEntry[]>("wiki_dataset_list_dir", { uuid, rel });
      const key = dsKey(uuid, rel);
      if (hubMode) setHubDsDirContents((prev) => ({ ...prev, [key]: result }));
      else setDsDirContents((prev) => ({ ...prev, [key]: result }));
    } catch (err) {
      const errorMsg = `Failed to load wiki dataset directory: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      await message(errorMsg, { title: "Error", kind: "error" });
    }
  };

  const refreshDsDir = async (uuid: string, rel: string) => {
    await loadDsDir(uuid, rel);
  };

  const toggleDsDir = async (uuid: string, rel: string) => {
    const key = dsKey(uuid, rel);
    const newExpanded = new Set(expandedDsDirs);
    if (newExpanded.has(key)) {
      newExpanded.delete(key);
    } else {
      newExpanded.add(key);
      const cache = hubMode ? hubDsDirContents : dsDirContents;
      if (!cache[key]) await loadDsDir(uuid, rel);
    }
    setExpandedDsDirs(newExpanded);
  };

  const toggleDsRoot = async (uuid: string) => {
    const newOpen = new Set(dsRootOpen);
    if (newOpen.has(uuid)) {
      newOpen.delete(uuid);
    } else {
      newOpen.add(uuid);
      const key = dsKey(uuid, "");
      const cache = hubMode ? hubDsDirContents : dsDirContents;
      if (!cache[key]) await loadDsDir(uuid, "");
    }
    setDsRootOpen(newOpen);
  };

  const openPrompt = (title: string, submit: (name: string) => Promise<void>) => {
    setPromptValue("");
    setPrompt({ title, submit });
  };

  const submitPrompt = async () => {
    if (!prompt || promptBusy) return;
    const name = sanitizeName(promptValue);
    if (!name) return;
    setPromptBusy(true);
    try {
      await prompt.submit(name);
      setPrompt(null);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
    } finally {
      setPromptBusy(false);
    }
  };

  const handleCreateFile = (uuid: string, dirRel: string) => {
    openPrompt("New file (name without .md will get the extension)", async (name) => {
      const rel = joinRel(dirRel, name.endsWith(".md") ? name : `${name}.md`);
      await invoke("wiki_dataset_create_file", { uuid, rel });
      await refreshDsDir(uuid, dirRel);
      onDatasetsChanged();
      onFileSelect({ source: { kind: "dataset", uuid, rel }, label: rel.split("/").pop() || rel });
    });
  };

  const handleCreateDir = (uuid: string, dirRel: string) => {
    openPrompt("New folder", async (name) => {
      const rel = joinRel(dirRel, name);
      await invoke("wiki_dataset_create_dir", { uuid, rel });
      // Empty dirs aren't listed until a file lands inside (git-style); just
      // re-read the parent so any implicitly-created container shows up.
      await refreshDsDir(uuid, dirRel);
    });
  };

  const handleDeleteFile = (uuid: string, rel: string) => {
    void (async () => {
      const confirmed = await ask(`Delete this file?\n${rel}\n\nIt is moved to trash, not permanently deleted.`, { title: "Delete File", kind: "warning" });
      if (!confirmed) return;
      try {
        await invoke("wiki_dataset_delete_file", { uuid, rel });
        await refreshDsDir(uuid, parentOf(rel));
        if (selection?.source.kind === "dataset" && selection.source.uuid === uuid && selection.source.rel === rel) {
          onFileSelect({ source: { kind: "dataset", uuid, rel: "" }, label: "" });
        }
      } catch (err) {
        await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
      }
    })();
  };

  const handleDeleteDataset = (ds: WikiDatasetSummary) => {
    void (async () => {
      const confirmed = await ask(`Delete wiki dataset "${ds.name}"?\n\nThe folder is moved to trash, not permanently deleted.`, { title: "Delete Wiki Dataset", kind: "warning" });
      if (!confirmed) return;
      try {
        await invoke("wiki_dataset_delete", { uuid: ds.uuid });
        onDatasetsChanged();
      } catch (err) {
        await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
      }
    })();
  };

  const handleCreateDataset = () => {
    openPrompt("New wiki dataset name", async (name) => {
      await invoke<WikiDatasetSummary>("wiki_dataset_create", { name });
      onDatasetsChanged();
    });
  };

  const handleImportDir = async () => {
    if (!isTauri()) return;
    try {
      const path = await invoke<string>("settings_pick_folder", { field: "wiki_dir" });
      const name = path.split(/[\\/]/).filter(Boolean).pop() || "wiki";
      await invoke<WikiDatasetSummary>("wiki_dataset_import_dir", { name, path });
      onDatasetsChanged();
    } catch (err) {
      // A cancelled folder picker isn't an error.
      if (String(err).toLowerCase().includes("cancel")) return;
      const errorMsg = `Failed to convert folder: ${err instanceof Error ? err.message : String(err)}`;
      logError(errorMsg, "wiki");
      await message(errorMsg, { title: "Error", kind: "error" });
    }
  };

  const renderDsFileEntry = (uuid: string, entry: WikiFileEntry, depth: number) => {
    const key = dsKey(uuid, entry.rel_path);
    const isExpanded = expandedDsDirs.has(key);
    const children = (hubMode ? hubDsDirContents : dsDirContents)[key];
    const src: WikiSource = hubMode
      ? { kind: "hub-dataset", uuid, rel: entry.rel_path, name: "" }
      : { kind: "dataset", uuid, rel: entry.rel_path };
    const isSelected = selection && selectionKey(selection.source) === selectionKey(src);
    const canEdit = !hubMode;

    return (
      <div key={entry.rel_path}>
        <div
          className={`group flex items-center gap-1 px-2 py-1 cursor-pointer rounded-md text-sm transition-colors ${
            isSelected ? "bg-accent/15 text-accent" : "hover:bg-bg-hover text-text-primary"
          }`}
          style={{ paddingLeft: `${depth * 14 + 8}px` }}
          onClick={() => {
            if (entry.is_dir) {
              toggleDsDir(uuid, entry.rel_path);
            } else {
              onFileSelect({ source: src, label: entry.name });
            }
          }}
        >
          {entry.is_dir ? (
            <>
              {isExpanded ? <ChevronDown size={14} className="shrink-0 text-text-tertiary" /> : <ChevronRight size={14} className="shrink-0 text-text-tertiary" />}
              <Folder size={14} className="shrink-0 text-accent" />
            </>
          ) : (
            <>
              <span className="w-[14px] shrink-0" />
              <FileText size={14} className="shrink-0 text-text-tertiary" />
            </>
          )}
          <span className="truncate flex-1">{entry.name}</span>
          {canEdit && (
            <span className="hidden group-hover:flex items-center gap-0.5">
              {entry.is_dir && (
                <>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleCreateFile(uuid, entry.rel_path); }}
                    className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-text-primary"
                    title="New file in this folder"
                  >
                    <FilePlus size={12} />
                  </button>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleCreateDir(uuid, entry.rel_path); }}
                    className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-text-primary"
                    title="New folder inside"
                  >
                    <FolderPlus size={12} />
                  </button>
                </>
              )}
              {!entry.is_dir && (
                <button
                  onClick={(e) => { e.stopPropagation(); handleDeleteFile(uuid, entry.rel_path); }}
                  className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-error-text"
                  title="Delete file (moves to trash)"
                >
                  <Trash2 size={12} />
                </button>
              )}
            </span>
          )}
        </div>
        {entry.is_dir && isExpanded && children && (
          <div>{children.map((child) => renderDsFileEntry(uuid, child, depth + 1))}</div>
        )}
      </div>
    );
  };

  const renderDataset = (ds: WikiDatasetSummary) => {
    const open = dsRootOpen.has(ds.uuid);
    const rootEntries = (hubMode ? hubDsDirContents : dsDirContents)[dsKey(ds.uuid, "")];
    const src = selection?.source;
    const isSelected = !!src && src.kind !== "legacy" && src.uuid === ds.uuid && src.rel === "";

    return (
      <div key={ds.uuid}>
        <div
          className={`group flex items-center gap-1 px-2 py-1 cursor-pointer rounded-md text-sm font-medium transition-colors ${
            isSelected ? "bg-accent/15 text-accent" : "hover:bg-bg-hover text-text-primary"
          }`}
          onClick={() => toggleDsRoot(ds.uuid)}
        >
          {open ? <ChevronDown size={14} className="shrink-0 text-text-tertiary" /> : <ChevronRight size={14} className="shrink-0 text-text-tertiary" />}
          <BookMarked size={14} className="shrink-0 text-purple-500" />
          <span className="truncate flex-1">{ds.name}</span>
          <span className="text-[10px] text-text-tertiary shrink-0">{ds.file_count}</span>
          {!hubMode && canMutateRoots && (
            <button
              onClick={(e) => { e.stopPropagation(); handleDeleteDataset(ds); }}
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary hover:text-error-text transition-colors opacity-0 group-hover:opacity-100"
              title="Delete wiki dataset (moves to trash)"
            >
              <Trash2 size={12} />
            </button>
          )}
        </div>
        {open && rootEntries && (
          <div>
            {rootEntries.length === 0 && (
              <div className="text-xs text-text-tertiary py-1" style={{ paddingLeft: 24 }}>
                Empty — use the + actions to add files.
              </div>
            )}
            {rootEntries.map((child) => renderDsFileEntry(ds.uuid, child, 1))}
          </div>
        )}
      </div>
    );
  };

  const handleRefresh = async () => {
    // Clear cached directory contents so expanded dirs get refreshed too
    setDirContents({});
    setExpandedDirs(new Set());
    setDsDirContents({});
    setHubDsDirContents({});
    setExpandedDsDirs(new Set());
    // Re-fetch any open dataset roots.
    for (const uuid of dsRootOpen) {
      if (uuid) void loadDsDir(uuid, "");
    }
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

  // The dataset list shown in the tree: local datasets, or the hub's in hub mode.
  const treeDatasets = datasets;

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center justify-between px-3 py-2 border-b border-border-default">
        <h3 className="text-sm font-semibold text-text-primary">{hubMode ? "Hub Wiki" : "Wiki"}</h3>
        <div className="flex items-center gap-1">
          {!mobile && !hubMode && canMutateRoots && (
            <>
              <button
                onClick={handleCreateDataset}
                className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                title="New wiki dataset"
              >
                <Plus size={14} />
              </button>
              <button
                onClick={handleImportDir}
                className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                title="Convert an existing markdown folder into a wiki dataset (in place)"
              >
                <FolderInput size={14} />
              </button>
              <button
                onClick={() => setShowAddDialog(true)}
                className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-hover transition-colors"
                title="Link external directory (legacy wiki)"
              >
                <Link size={14} />
              </button>
            </>
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
      {prompt && (
        <div className="px-3 py-2 border-b border-border-default bg-bg-body space-y-2">
          <div className="text-xs font-semibold text-text-primary">{prompt.title}</div>
          <input
            type="text"
            autoFocus
            value={promptValue}
            onChange={(e) => setPromptValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void submitPrompt();
              else if (e.key === "Escape") setPrompt(null);
            }}
            className="w-full px-2 py-1 text-sm bg-bg-input border border-border-default rounded-md outline-none focus:border-accent"
          />
          <div className="flex gap-2">
            <button
              onClick={() => void submitPrompt()}
              disabled={!sanitizeName(promptValue) || promptBusy}
              className="flex-1 px-2 py-1 text-xs bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              {promptBusy ? "…" : "OK"}
            </button>
            <button
              onClick={() => setPrompt(null)}
              className="flex-1 px-2 py-1 text-xs bg-bg-input border border-border-default rounded-md hover:bg-bg-hover transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
      <div className="flex-1 overflow-y-auto min-h-0 p-2">
        {/* Wiki datasets section (local, or hub tree while in hub mode) */}
        {!hubMode && (
          <div className="mb-2">
            <div className="text-[10px] uppercase tracking-wide text-text-tertiary font-semibold px-2 py-1">
              Wiki Datasets (local)
            </div>
            {treeDatasets.length === 0 ? (
              <div className="text-xs text-text-tertiary px-2 py-1">
                None yet — create one or convert a folder.
              </div>
            ) : (
              treeDatasets.map(renderDataset)
            )}
          </div>
        )}
        {hubMode && (
          <div className="mb-2">
            <div className="text-[10px] uppercase tracking-wide text-text-tertiary font-semibold px-2 py-1">
              Hub Wiki Datasets
            </div>
            {treeDatasets.length === 0 ? (
              <div className="text-xs text-text-tertiary px-2 py-1">The hub has no wiki datasets.</div>
            ) : (
              treeDatasets.map(renderDataset)
            )}
          </div>
        )}
        {/* Legacy wiki roots (hidden in hub mode where they're read through REST) */}
        {!hubMode && (
          <div>
            <div className="text-[10px] uppercase tracking-wide text-text-tertiary font-semibold px-2 py-1">
              Wiki Folders
            </div>
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
        )}
        {hubMode && (
          <div>
            <div className="text-[10px] uppercase tracking-wide text-text-tertiary font-semibold px-2 py-1">
              Hub Legacy Wiki
            </div>
            {/* Legacy roots are absolute hub paths, browsable only through the
                mobile path-proxy commands (wiki_list_dirs / wiki_list_dir /
                wiki_read_file relay to the PC there). On a desktop follower
                these commands read the *local* disk, so skip the section. */}
            {!mobile ? (
              <div className="text-xs text-text-tertiary px-2 py-1">
                Legacy wiki browse is available in the mobile hub view.
              </div>
            ) : loading ? (
              <div className="text-sm text-text-tertiary text-center py-4">Loading...</div>
            ) : entries.length === 0 ? (
              <div className="text-xs text-text-tertiary text-center py-4">The hub exposes no legacy wiki.</div>
            ) : (
              entries.map((entry) => renderEntry(entry))
            )}
          </div>
        )}
      </div>
    </div>
  );
}
