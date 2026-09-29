"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, ChevronRight, ExternalLink } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { type ModelVersionInfo, type DownloadProgress, type ModelStatus } from "@/lib/models/types";
import ModelCard from "./components/ModelCard";
import ModelDetailsDialog from "./components/ModelDetailsDialog";

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

  // ---- Event listeners ----

  useEffect(() => {
    if (!isTauri()) return;
    const unlistenProgress = listen<DownloadProgress>("model-download-progress", (event) => {
      setDownloadProgress(event.payload);
    });
    const unlistenChanged = listen("model-status-changed", () => {
      fetchStatus();
    });
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
    };
  }, [fetchStatus]);

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
      <main className="flex-1 p-6 overflow-y-auto min-h-0">
        <h1 className="text-xl font-bold mb-4">Models</h1>

        {/* ─── STT Models Section ─── */}
        <section className="mb-6">
          <div className="flex items-center gap-2 mb-1">
            <h2 className="text-sm font-semibold uppercase tracking-wide text-text-secondary">Speech-to-Text</h2>
          </div>
          <p className="text-xs text-text-tertiary mb-3">
            Powered by models from <a href="https://github.com/cjpais/Handy" target="_blank" rel="noopener noreferrer" className="text-accent hover:underline inline-flex items-center gap-0.5">Handy <ExternalLink size={10} /></a>.
            Supports Parakeet, Whisper, Moonshine, and more via transcribe-rs and transcribe-cpp.
          </p>

          {/* Downloaded STT models - compact list */}
          <div className="flex flex-col gap-2">
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
              <p className="text-xs text-text-tertiary italic py-2">No STT models downloaded yet.</p>
            )}
          </div>

          {/* Available STT models (collapsible) */}
          {availableModels.length > 0 && (
            <div className="mt-3">
              <button
                className="flex items-center gap-1 text-xs font-medium text-text-tertiary hover:text-text-secondary cursor-pointer transition-colors mb-2"
                onClick={() => setShowAvailable(!showAvailable)}
              >
                {showAvailable ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                <span>Available Models</span>
                <span className="text-[10px]">({availableModels.length})</span>
              </button>

              {showAvailable && (
                <div className="flex flex-col gap-2">
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
      </main>

      {detailsModel && (
        <ModelDetailsDialog model={detailsModel} onClose={() => setDetailsModel(null)} />
      )}
    </>
  );
}
