"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, ChevronRight } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type ModelVersionInfo, type DownloadProgress, type ModelStatus } from "@/lib/models/types";
import ModelCard from "@/components/models/ModelCard";
import ModelDetailsDialog from "@/components/models/ModelDetailsDialog";
import TestSection from "@/components/models/TestSection";

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function ModelsPage() {
  const [models, setModels] = useState<ModelVersionInfo[]>([]);
  const [selectedVersion, setSelectedVersion] = useState<string>("");
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadingVersion, setDownloadingVersion] = useState<string | null>(null);
  const [activeVersion, setActiveVersion] = useState<string | null>(null);
  const [activeStatus, setActiveStatus] = useState<ModelStatus>("Stopped");
  const [modelLoading, setModelLoading] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [detailsModel, setDetailsModel] = useState<ModelVersionInfo | null>(null);
  const [showAvailable, setShowAvailable] = useState(false);

  // ---- Event listeners ----

  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen<DownloadProgress>("model-download-progress", (event) => {
      setDownloadProgress(event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // ---- Fetch status ----

  const fetchStatus = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const res = await invoke<{
        models: ModelVersionInfo[];
        selected_version: string;
        active_version: string | null;
        active_status: ModelStatus;
        download_progress: DownloadProgress | null;
      }>("model_get_status");
      setModels(res.models);
      setSelectedVersion(res.selected_version);
      setActiveVersion(res.active_version);
      setActiveStatus(res.active_status);
      setDownloadProgress(res.download_progress ?? null);
    } catch (e) {
      console.error("Failed to get model status:", e);
    }
  }, []);

  useEffect(() => { fetchStatus(); }, [fetchStatus]);

  // ---- Handlers ----

  async function handleDownload(modelId: string) {
    if (!isTauri()) return;
    setDownloading(true);
    setDownloadingVersion(modelId);
    try {
      await invoke("model_download", { version: modelId });
      await fetchStatus();
    } catch (e) {
      console.error("Download failed:", e);
      await fetchStatus();
    } finally {
      setDownloading(false);
      setDownloadingVersion(null);
    }
  }

  async function handleCancel(modelId: string) {
    if (!isTauri()) return;
    try { await invoke("model_cancel_download", { version: modelId }); }
    catch (e) { console.error("Cancel failed:", e); }
  }

  async function handleDelete(modelId: string) {
    if (!isTauri()) return;
    if (confirmDeleteId !== modelId) { setConfirmDeleteId(modelId); return; }
    setConfirmDeleteId(null);
    try { await invoke("model_delete", { version: modelId }); await fetchStatus(); }
    catch (e) { console.error("Delete failed:", e); }
  }

  async function handleStart(modelId: string) {
    if (!isTauri()) return;
    try { await invoke("model_select_version", { version: modelId }); setSelectedVersion(modelId); }
    catch (e) { console.error("Failed to select version:", e); return; }
    setModelLoading(true);
    try { await invoke("model_start"); await fetchStatus(); }
    catch (e) { console.error("Start failed:", e); await fetchStatus(); }
    finally { setModelLoading(false); }
  }

  async function handleStop() {
    if (!isTauri()) return;
    setModelLoading(true);
    try { await invoke("model_stop"); await fetchStatus(); }
    catch (e) { console.error("Stop failed:", e); await fetchStatus(); }
    finally { setModelLoading(false); }
  }

  // ---- Derived state ----

  const isModelRunning = activeStatus === "Running";
  const isDownloadingAny = downloading || activeStatus === "Downloading";
  const downloadedModels = models.filter((m) => m.downloaded);
  const availableModels = models.filter((m) => !m.downloaded);

  function getModelStatus(modelId: string): ModelStatus {
    if (activeVersion === modelId && isModelRunning) return "Running";
    if (activeVersion === modelId) return "Stopped";
    if (downloadingVersion === modelId && isDownloadingAny) return "Downloading";
    const model = models.find((m) => m.id === modelId);
    if (model?.downloaded) return "Downloaded";
    return "NotDownloaded";
  }

  // ---- Render ----

  return (
    <>
      <main className="flex-1 p-8 overflow-y-auto">
        <h1 className="text-[1.8em] font-bold mb-6">Models</h1>

        <section className="mb-8">
          <h2 className="text-[1.3em] font-semibold mb-2">Models</h2>
          <p className="text-text-secondary text-sm mb-4">
            Download, manage, and start speech recognition models.
          </p>

          {/* Downloaded models */}
          <div className="flex flex-col gap-3">
            {downloadedModels.map((model) => (
              <ModelCard
                key={model.id}
                model={model}
                status={getModelStatus(model.id)}
                isDownloading={downloadingVersion === model.id && isDownloadingAny}
                isDownloadInProgress={isDownloadingAny}
                downloadProgress={downloadProgress}
                modelLoading={modelLoading}
                confirmDelete={confirmDeleteId === model.id}
                onDownload={() => {}}
                onCancel={() => handleCancel(model.id)}
                onLoad={() => handleStart(model.id)}
                onStop={handleStop}
                onDelete={() => handleDelete(model.id)}
                onCancelDelete={() => setConfirmDeleteId(null)}
                onShowDetails={() => setDetailsModel(model)}
              />
            ))}
            {downloadedModels.length === 0 && !isDownloadingAny && (
              <p className="text-sm text-text-tertiary italic">No models downloaded yet.</p>
            )}
          </div>

          {/* Available models (collapsible) */}
          {availableModels.length > 0 && (
            <div className="mt-6">
              <button
                className="flex items-center gap-1.5 text-sm font-medium text-text-secondary hover:text-text-primary cursor-pointer transition-colors mb-3"
                onClick={() => setShowAvailable(!showAvailable)}
              >
                {showAvailable ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
                <span>Available Models</span>
                <span className="text-xs text-text-tertiary">({availableModels.length})</span>
              </button>

              {showAvailable && (
                <div className="flex flex-col gap-3">
                  {availableModels.map((model) => (
                    <ModelCard
                      key={model.id}
                      model={model}
                      status={getModelStatus(model.id)}
                      isDownloading={downloadingVersion === model.id && isDownloadingAny}
                      isDownloadInProgress={isDownloadingAny}
                      downloadProgress={downloadProgress}
                      modelLoading={modelLoading}
                      confirmDelete={false}
                      onDownload={() => handleDownload(model.id)}
                      onCancel={() => handleCancel(model.id)}
                      onLoad={() => {}}
                      onStop={() => {}}
                      onDelete={() => {}}
                      onCancelDelete={() => {}}
                      onShowDetails={() => setDetailsModel(model)}
                    />
                  ))}
                </div>
              )}
            </div>
          )}
        </section>

        <TestSection isModelRunning={isModelRunning} />
      </main>

      {detailsModel && (
        <ModelDetailsDialog model={detailsModel} onClose={() => setDetailsModel(null)} />
      )}
    </>
  );
}
