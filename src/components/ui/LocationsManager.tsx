"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, FolderPlus, MapPin, Trash2 } from "lucide-react";
import { isMobileApp } from "@/lib/platform";

/** Dataset types that support linked-directory locations (mirrors the backend DatasetType). */
export type LocationDatasetType = "card" | "dictation" | "book" | "read_aloud" | "wiki";

interface DirEntry {
  name: string;
  path: string;
  is_linked: boolean;
  is_read_only?: boolean;
}

/**
 * Shared dataset-location manager — the Cards "Locations" view, generalised to any
 * dataset type. Lists the default + linked + app-managed (read-only) directories,
 * lets the user add a linked directory, and delete (unlink) a linked one.
 *
 * "Delete Location" only unlinks: the directory is removed from the app's list and
 * its datasets stop showing here, but the files on disk are untouched (this is all
 * `dataset_remove_dir` does). The default and app-managed roots are inherent and
 * cannot be deleted, so the button is offered for linked directories only.
 */
export function LocationsManager({
  datasetType,
  description,
  onError,
  onChanged,
  countFor,
}: {
  datasetType: LocationDatasetType;
  /** Page-specific blurb; falls back to a generic one. */
  description?: string;
  /** Surface a failure (toast / error banner) on the owning page. */
  onError: (msg: string) => void;
  /** Called after a successful add/delete so the page can reload its dataset list. */
  onChanged?: () => void;
  /** Optional per-directory dataset count (path → number), shown beside the name. */
  countFor?: (path: string) => number;
}) {
  const [dirs, setDirs] = useState<DirEntry[]>([]);
  const [showAddDir, setShowAddDir] = useState(false);
  const [newDirName, setNewDirName] = useState("");
  const [newDirPath, setNewDirPath] = useState("");
  // Deferred platform flag (SSR-safe). Folder picking is desktop-only — the dialog
  // plugin answers `open({ directory: true })` with FolderPickerNotImplemented on
  // Android/iOS — so the Browse affordance is hidden there and the path stays typed
  // by hand (e.g. a directory filled by the PC sync flow).
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  const loadDirs = useCallback(async () => {
    try {
      const result = await invoke<DirEntry[]>("dataset_list_dirs", { datasetType });
      setDirs(result);
    } catch {
      // Ignore — leave whatever is currently shown.
    }
  }, [datasetType]);

  useEffect(() => {
    loadDirs();
  }, [loadDirs]);

  return (
    <div className="p-3 overflow-y-auto h-full space-y-6 sm:p-6">
      <section>
        <h2 className="text-lg font-semibold mb-3 flex items-center gap-2">
          <FolderOpen size={18} />
          Locations
        </h2>
        <p className="text-sm text-text-secondary mb-3">
          {description ??
            "Datasets are stored across dataset directories. Add linked directories to store datasets on external drives or other locations."}
        </p>

        {/* Directory list */}
        <div className="space-y-2 mb-3">
          {dirs.map((dir) => {
            const count = countFor?.(dir.path);
            return (
              <div
                key={dir.path}
                className="p-3 rounded-lg border border-border-default bg-bg-card"
              >
                <div className="flex items-center justify-between mb-1 gap-2">
                  <div className="flex items-center gap-2 min-w-0">
                    <MapPin size={14} className="text-text-tertiary shrink-0" />
                    <span className="text-sm font-medium truncate">{dir.name}</span>
                    {!dir.is_linked && !dir.is_read_only && (
                      <span className="text-xs px-1.5 py-0.5 rounded bg-accent-blue/10 text-accent-blue shrink-0">
                        default
                      </span>
                    )}
                    {dir.is_read_only && (
                      <span className="text-xs px-1.5 py-0.5 rounded border border-border-light bg-bg-body text-text-secondary shrink-0">
                        app-managed
                      </span>
                    )}
                    {count !== undefined && (
                      <span className="text-xs text-text-tertiary shrink-0">
                        {count} dataset{count !== 1 ? "s" : ""}
                      </span>
                    )}
                  </div>
                  {dir.is_linked && (
                    <button
                      onClick={async () => {
                        const confirmed = await ask(
                          `Delete location "${dir.name}"? Its datasets will no longer be listed here, but files on disk are untouched.`,
                          { title: "Delete Location" }
                        );
                        if (!confirmed) return;
                        try {
                          await invoke("dataset_remove_dir", { path: dir.path });
                          loadDirs();
                          onChanged?.();
                        } catch (e) {
                          onError(`Failed to delete location: ${e}`);
                        }
                      }}
                      className="flex items-center gap-1 px-2 py-1 rounded text-xs shrink-0 text-text-tertiary hover:bg-red-500/10 hover:text-red-500"
                      title="Delete location (unlink; files on disk are kept)"
                    >
                      <Trash2 size={14} /> Delete Location
                    </button>
                  )}
                </div>
                <p className="text-xs text-text-tertiary font-mono break-all">{dir.path}</p>
              </div>
            );
          })}
        </div>

        {/* Add linked directory */}
        {showAddDir ? (
          <div className="p-3 rounded-lg border border-border-default bg-bg-card space-y-2">
            <div>
              <label className="block text-xs font-medium mb-1">Display Name</label>
              <input
                type="text"
                value={newDirName}
                onChange={(e) => setNewDirName(e.target.value)}
                placeholder="e.g. External SSD"
                className="w-full px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
              />
            </div>
            <div>
              <label className="block text-xs font-medium mb-1">Directory Path</label>
              <div className="flex gap-1">
                <input
                  type="text"
                  value={newDirPath}
                  onChange={(e) => setNewDirPath(e.target.value)}
                  placeholder="e.g. /mnt/external/fms-datasets"
                  className="flex-1 px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
                />
                {!mobile && (
                  <button
                    onClick={async () => {
                      const picked = await open({ directory: true, multiple: false, title: "Select Directory" });
                      if (typeof picked === "string") {
                        setNewDirPath(picked);
                        if (!newDirName.trim()) {
                          setNewDirName(picked.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || picked);
                        }
                      }
                    }}
                    className="px-2 py-1 rounded border border-border-default hover:bg-mid-gray/20 text-sm"
                    title="Browse..."
                  >
                    <FolderOpen size={14} />
                  </button>
                )}
              </div>
            </div>
            <div className="flex gap-2">
              <button
                onClick={async () => {
                  if (!newDirName.trim() || !newDirPath.trim()) return;
                  try {
                    await invoke("dataset_add_dir", { name: newDirName.trim(), path: newDirPath.trim() });
                    setShowAddDir(false);
                    setNewDirName("");
                    setNewDirPath("");
                    loadDirs();
                    onChanged?.();
                  } catch (e) {
                    onError(`Failed to add directory: ${e}`);
                  }
                }}
                className="px-3 py-1 rounded bg-accent-blue text-sm hover:opacity-90"
              >
                Add
              </button>
              <button
                onClick={() => {
                  setShowAddDir(false);
                  setNewDirName("");
                  setNewDirPath("");
                }}
                className="px-3 py-1 rounded border border-border-default text-sm"
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setShowAddDir(true)}
            className="px-3 py-1.5 rounded border border-border-default text-sm hover:bg-mid-gray/20"
          >
            <FolderPlus size={14} className="inline mr-1" />
            Add Linked Directory
          </button>
        )}
      </section>
    </div>
  );
}

export default LocationsManager;
