"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail as DatasetDetailType,
  datasetStatusLabel,
  datasetStatusBadgeClasses,
  btnSmPrimary,
} from "@/lib/datasets/types";
import DatasetDetailPanel from "./components/DatasetDetailPanel";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

// Group datasets by non-empty uuid and return the groups that collide.
function findUuidConflicts(list: DatasetSummary[]): [string, DatasetSummary[]][] {
  const byUuid = new Map<string, DatasetSummary[]>();
  for (const ds of list) {
    const uuid = ds.info.uuid;
    if (!uuid) continue;
    const arr = byUuid.get(uuid);
    if (arr) arr.push(ds);
    else byUuid.set(uuid, [ds]);
  }
  return Array.from(byUuid.entries()).filter(([, arr]) => arr.length > 1);
}

export default function DatasetsPage() {
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [locations, setLocations] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [expandedUuid, setExpandedUuid] = useState<string | null>(null);
  const [detail, setDetail] = useState<DatasetDetailType | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmRemoveLocation, setConfirmRemoveLocation] = useState<string | null>(null);
  const [uuidConflict, setUuidConflict] = useState<ConfirmRequest | null>(null);

  // ---- Fetch dataset list ----

  const fetchDatasets = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const res = await invoke<DatasetSummary[]>("dataset_list");
      setDatasets(res);
    } catch (e) {
      console.error("Failed to list datasets:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchLocations = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<string[]>("dataset_list_locations");
      setLocations(res);
    } catch (e) {
      console.error("Failed to list dataset locations:", e);
    }
  }, []);

  useEffect(() => { fetchLocations(); fetchDatasets(); }, [fetchLocations, fetchDatasets]);

  // Warn the user when two or more datasets share the same uuid.
  useEffect(() => {
    const conflicts = findUuidConflicts(datasets);
    if (conflicts.length === 0) return;
    setUuidConflict({
      title: "Duplicate dataset UUIDs",
      danger: true,
      message: (
        <div className="flex flex-col gap-3">
          <p>Multiple datasets share the same UUID. Each dataset must have a unique UUID. The following conflicts were found:</p>
          {conflicts.map(([uuid, list]) => (
            <div key={uuid} className="flex flex-col gap-1">
              <span className="font-mono text-xs break-all">UUID: {uuid}</span>
              <ul className="flex flex-col gap-1 pl-4 list-disc">
                {list.map((ds) => (
                  <li key={ds.path} className="text-xs break-all">{ds.info.name} &mdash; {ds.path}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      ),
    });
  }, [datasets]);

  // ---- Import ----

  async function handleImport() {
    if (!isTauri()) return;
    setImportError(null);
    try {
      const sourceDir = await invoke<string>("settings_pick_folder", { field: "datasets_dir" });
      setImporting(true);
      await invoke("dataset_import", { sourceDir });
      await fetchDatasets();
    } catch (e) {
      setImportError(String(e));
    } finally {
      setImporting(false);
    }
  }

  // ---- Locations ----

  async function handleAddLocation() {
    if (!isTauri()) return;
    setImportError(null);
    try {
      const path = await invoke<string>("settings_pick_folder", { field: "dataset_location" });
      const updated = await invoke<string[]>("dataset_add_location", { path });
      setLocations(updated);
      await fetchDatasets();
    } catch (e) {
      // Ignore folder-picker cancellation.
      if (String(e).includes("No folder selected")) return;
      setImportError(String(e));
    }
  }

  async function handleRemoveLocation(path: string) {
    if (!isTauri()) return;
    try {
      const updated = await invoke<string[]>("dataset_remove_location", { path });
      setLocations(updated);
      setConfirmRemoveLocation(null);
      await fetchDatasets();
    } catch (e) {
      setImportError(String(e));
    }
  }

  async function handleOpenLocation(path: string) {
    if (!isTauri()) return;
    try {
      await openPath(path);
    } catch (e) {
      console.error("Failed to open location:", e);
    }
  }

  // ---- Expand / collapse ----

  async function toggleExpand(uuid: string) {
    if (expandedUuid === uuid) {
      setExpandedUuid(null);
      setDetail(null);
      return;
    }
    setExpandedUuid(uuid);
    setDetailLoading(true);
    try {
      const res = await invoke<DatasetDetailType>("dataset_get", { uuid });
      setDetail(res);
    } catch (e) {
      console.error("Failed to get dataset detail:", e);
      setDetail(null);
    } finally {
      setDetailLoading(false);
    }
  }

  async function handleDelete(uuid?: string) {
    if (!isTauri()) return;
    const targetUuid = uuid || detail?.info.uuid;
    if (!targetUuid) return;
    if (!confirmDelete) { setConfirmDelete(true); return; }
    try {
      await invoke("dataset_delete", { uuid: targetUuid });
      if (expandedUuid === targetUuid) { setExpandedUuid(null); setDetail(null); }
      setConfirmDelete(false);
      await fetchDatasets();
    } catch (e) {
      console.error("Failed to delete dataset:", e);
    }
  }

  // ---- Render ----

  function renderDatasetCard(ds: DatasetSummary) {
    return (
      <div key={ds.path} className="border border-border-default rounded-lg overflow-hidden">
        <div
          className="flex items-center justify-between p-4 cursor-pointer hover:bg-bg-hover transition-colors"
          onClick={() => toggleExpand(ds.info.uuid)}
        >
          <div className="flex flex-col gap-1 min-w-0">
            <div className="flex items-center gap-3">
              <span className="font-medium">{ds.info.name}</span>
              <span className={datasetStatusBadgeClasses(ds.status)}>{datasetStatusLabel(ds.status)}</span>
            </div>
            <span className="text-xs text-text-tertiary truncate" title={ds.location}>{ds.location}</span>
          </div>
          <div className="flex items-center gap-2">
            <button
              className="p-1 rounded cursor-pointer transition-colors text-text-tertiary hover:text-accent hover:bg-info-bg"
              onClick={async (e) => {
                e.stopPropagation();
                await fetchDatasets();
                if (expandedUuid === ds.info.uuid) {
                  setDetailLoading(true);
                  try { setDetail(await invoke<DatasetDetailType>("dataset_get", { uuid: ds.info.uuid })); }
                  catch { /* ignore */ }
                  finally { setDetailLoading(false); }
                }
              }}
              disabled={detailLoading}
              title="Refresh"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path d="M16 16h5v5" /></svg>
            </button>
            <button
              className={`p-1 rounded cursor-pointer transition-colors ${confirmDelete ? "bg-error-text text-white hover:bg-error-hover" : "text-error-text hover:bg-error-bg"}`}
              onClick={(e) => { e.stopPropagation(); handleDelete(ds.info.uuid); }}
              title={confirmDelete ? "Click again to confirm" : "Delete dataset"}
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>
            </button>
            {confirmDelete && (
              <button className="px-2 py-0.5 text-xs rounded cursor-pointer bg-transparent border border-border-light text-text-secondary hover:bg-bg-hover" onClick={(e) => { e.stopPropagation(); setConfirmDelete(false); }}>
                Cancel
              </button>
            )}
          </div>
        </div>

        {expandedUuid === ds.info.uuid && detail && (
          <DatasetDetailPanel detail={detail} datasetPath={ds.path} />
        )}
        {expandedUuid === ds.info.uuid && detailLoading && (
          <div className="px-4 pb-4 border-t border-border-default">
            <p className="text-text-secondary py-4">Loading details...</p>
          </div>
        )}
        {expandedUuid === ds.info.uuid && !detail && !detailLoading && (
          <div className="px-4 pb-4 border-t border-border-default">
            <p className="text-error-text text-sm py-4">Failed to load details.</p>
          </div>
        )}
      </div>
    );
  }

  return (
    <main className="flex-1 p-8 overflow-y-auto">
      {/* ---- Locations section ---- */}
      <section className="mb-8">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-[1.3em] font-bold">Locations</h2>
          <button className="p-2 rounded-md cursor-pointer transition-colors hover:bg-bg-hover text-text-secondary" onClick={handleAddLocation} title="Add datasets location">
            <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /><line x1="12" y1="11" x2="12" y2="17" /><line x1="9" y1="14" x2="15" y2="14" /></svg>
          </button>
        </div>
        {locations.length === 0 ? (
          <p className="text-sm text-text-tertiary">No locations configured. Click the add button to add a directory containing datasets.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {locations.map((loc) => (
              <div key={loc} className="flex items-center justify-between gap-2 p-3 border border-border-default rounded-lg">
                <div className="flex items-center gap-2 min-w-0">
                  <svg className="shrink-0 text-text-tertiary" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>
                  <span className="text-sm truncate" title={loc}>{loc}</span>
                </div>
                <div className="flex items-center gap-1 shrink-0">
                  <button className="p-1 rounded cursor-pointer transition-colors text-text-tertiary hover:text-accent hover:bg-info-bg" onClick={() => handleOpenLocation(loc)} title="Open location">
                    <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" /><polyline points="15 3 21 3 21 9" /><line x1="10" y1="14" x2="21" y2="3" /></svg>
                  </button>
                  {confirmRemoveLocation === loc ? (
                    <>
                      <button className="px-2 py-0.5 text-xs rounded cursor-pointer bg-error-text text-white hover:bg-error-hover" onClick={() => handleRemoveLocation(loc)}>
                        Remove
                      </button>
                      <button className="px-2 py-0.5 text-xs rounded cursor-pointer bg-transparent border border-border-light text-text-secondary hover:bg-bg-hover" onClick={() => setConfirmRemoveLocation(null)}>
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button className="p-1 rounded cursor-pointer transition-colors text-text-tertiary hover:text-error-text hover:bg-error-bg" onClick={() => setConfirmRemoveLocation(loc)} title="Remove location">
                      <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg>
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* ---- Datasets section ---- */}
      <section>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-[1.3em] font-bold">Datasets</h2>
          <div className="flex items-center gap-2">
            <button className="p-2 rounded-md cursor-pointer transition-colors hover:bg-bg-hover text-text-secondary" onClick={fetchDatasets} disabled={loading} title="Refresh">
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path d="M16 16h5v5" /></svg>
            </button>
            <button className={`${btnSmPrimary} bg-success-text text-white hover:opacity-90`} onClick={handleImport} disabled={importing}>
              {importing ? "Importing..." : "Import"}
            </button>
          </div>
        </div>

        {importError && <div className="p-3 bg-error-bg text-error-text rounded-md mb-4 text-sm">{importError}</div>}

        {loading ? (
          <p className="text-text-secondary">Loading datasets...</p>
        ) : datasets.length === 0 ? (
          <div className="text-center py-12">
            <p className="text-text-secondary">No datasets found.</p>
            <p className="text-sm text-text-tertiary mt-1">Add a location above, or click &quot;Import&quot; to copy a dataset directory containing audio files.</p>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {datasets.map(renderDatasetCard)}
          </div>
        )}
      </section>

      <ConfirmDialog request={uuidConflict} onClose={() => setUuidConflict(null)} />
    </main>
  );
}
