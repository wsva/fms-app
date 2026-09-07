"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";
import { isTauri } from "@/lib/tauri";
import {
  type DatasetSummary,
  type DatasetDetail as DatasetDetailType,
  type DatasetProgressEvt,
  datasetStatusLabel,
  datasetStatusBadgeClasses,
  btnSmPrimary,
} from "@/lib/datasets/types";
import DatasetDetailPanel from "@/components/datasets/DatasetDetailPanel";
import ModelPickerModal from "@/components/datasets/ModelPickerModal";

const btnBase = "px-4 py-2 rounded-md font-medium cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed";

export default function DatasetsPage() {
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [expandedUuid, setExpandedUuid] = useState<string | null>(null);
  const [detail, setDetail] = useState<DatasetDetailType | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [progress, setProgress] = useState<DatasetProgressEvt | null>(null);

  // ---- Listen for dataset generation progress ----

  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<DatasetProgressEvt>("dataset-progress", (event) => {
      setProgress(event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

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

  useEffect(() => { fetchDatasets(); }, [fetchDatasets]);

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

  async function refreshDetail() {
    if (!expandedUuid) return;
    try {
      const res = await invoke<DatasetDetailType>("dataset_get", { uuid: expandedUuid });
      setDetail(res);
    } catch (e) {
      console.error("Failed to refresh detail:", e);
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

  async function handleOpenDatasetsDir() {
    if (!isTauri()) return;
    try {
      const settings = await invoke<{ datasets_dir: string }>("settings_get");
      await openPath(settings.datasets_dir);
    } catch (e) {
      console.error("Failed to open datasets directory:", e);
    }
  }

  // ---- Render ----

  return (
    <main className="flex-1 p-8 overflow-y-auto">
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-[1.8em] font-bold">Datasets</h1>
        <div className="flex items-center gap-2">
          <button className="p-2 rounded-md cursor-pointer transition-colors hover:bg-bg-hover text-text-secondary" onClick={handleOpenDatasetsDir} title="Open datasets directory">
            <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /><line x1="12" y1="11" x2="12" y2="17" /><line x1="9" y1="14" x2="15" y2="14" /></svg>
          </button>
          <button className="p-2 rounded-md cursor-pointer transition-colors hover:bg-bg-hover text-text-secondary" onClick={fetchDatasets} disabled={loading} title="Refresh">
            <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path d="M16 16h5v5" /></svg>
          </button>
          <button className={btnSmPrimary} onClick={handleImport} disabled={importing}>
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
          <p className="text-sm text-text-tertiary mt-1">Click &quot;Import Dataset&quot; to add a dataset directory containing audio files.</p>
        </div>
      ) : (
        <div className="flex flex-col gap-3">
          {datasets.map((ds) => (
            <div key={ds.info.uuid || ds.info.name} className="border border-border-default rounded-lg overflow-hidden">
              <div
                className="flex items-center justify-between p-4 cursor-pointer hover:bg-bg-hover transition-colors"
                onClick={() => toggleExpand(ds.info.uuid)}
              >
                <div className="flex items-center gap-3">
                  <span className="font-medium">{ds.info.name}</span>
                  <span className={datasetStatusBadgeClasses(ds.status)}>{datasetStatusLabel(ds.status)}</span>
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
                    disabled={generating}
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
                <DatasetDetailPanel
                  detail={detail}
                  datasetPath={ds.path}
                  progress={progress}
                  generating={generating}
                  setGenerating={setGenerating}
                  setProgress={setProgress}
                  onRefreshDetail={refreshDetail}
                  onRefreshList={fetchDatasets}
                />
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
          ))}
        </div>
      )}
    </main>
  );
}
