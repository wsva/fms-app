"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, ExternalLink, Download, Trash2, Loader2 } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import { type ModelVersionInfo, type DownloadProgress, type ModelStatus } from "@/lib/models/types";
import ModelCard from "./components/ModelCard";
import ModelDetailsDialog from "./components/ModelDetailsDialog";

// Text-split (SaT) model info, mirroring the Rust `TextSplitModelInfo` shape.
interface TextSplitModelInfo {
  id: string;
  name: string;
  description: string;
  downloaded: boolean;
  loaded: boolean;
  size_mb: number;
}

// Download sources for HuggingFace-catalog models (Text Split). The key is
// sent to `model_download_hf`, which maps it to a backend `HubProvider`.
const HF_PROVIDERS = [
  { key: "huggingface", label: "Hugging Face" },
  { key: "hf-mirror", label: "HF Mirror" },
  { key: "modelscope", label: "ModelScope" },
] as const;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function ModelsPage() {
  const [models, setModels] = useState<ModelVersionInfo[]>([]);
  const [selectedVersion, setSelectedVersion] = useState<string>("");
  const [defaultVersion, setDefaultVersion] = useState<string>("");
  const [settingDefaultId, setSettingDefaultId] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadingVersion, setDownloadingVersion] = useState<string | null>(null);
  const [activeVersion, setActiveVersion] = useState<string | null>(null);
  const [activeStatus, setActiveStatus] = useState<ModelStatus>("Stopped");
  const [modelLoading, setModelLoading] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [detailsModel, setDetailsModel] = useState<ModelVersionInfo | null>(null);
  const [showAvailable, setShowAvailable] = useState(false);
  // Text-split (SaT) models are managed here too, but only on desktop (the
  // `textsplit` cargo feature is off on the Android thin client). `desktop` is
  // resolved in an effect rather than at render to avoid a hydration mismatch.
  const [desktop, setDesktop] = useState(false);
  const [textSplitModels, setTextSplitModels] = useState<TextSplitModelInfo[]>([]);
  const [tsDownloadingId, setTsDownloadingId] = useState<string | null>(null);
  const [tsConfirmDelete, setTsConfirmDelete] = useState<string | null>(null);
  // Per-card download-hub selection (defaults to Hugging Face).
  const [providerByModel, setProviderByModel] = useState<Record<string, string>>({});

  // On Android nothing is downloaded yet, so the collapsed "Available Models"
  // toggle would hide the whole list. Expand it by default there.
  // isMobileApp() must run in an effect (not render) to avoid SSR hydration mismatch.
  useEffect(() => {
    if (isMobileApp()) setShowAvailable(true);
  }, []);

  // ---- Fetch status ----

  const fetchStatus = useCallback(async () => {
    // The `model_*` commands are available on desktop and (via the `stt`
    // cargo feature) on the Android build, where we verify ONNX Runtime support.
    if (!isTauri()) return;
    try {
      const res = await invoke<{
        models: ModelVersionInfo[];
        selected_version: string;
        default_version: string;
        active_version: string | null;
        active_status: ModelStatus;
        download_progress: DownloadProgress | null;
      }>("model_get_status");
      setModels(res.models);
      setSelectedVersion(res.selected_version);
      setDefaultVersion(res.default_version ?? "");
      setActiveVersion(res.active_version);
      setActiveStatus(res.active_status);
      setDownloadProgress(res.download_progress ?? null);
    } catch (e) {
      console.error("Failed to get model status:", e);
    }
  }, []);

  // ---- Text-split (SaT) model status (desktop only) ----

  const fetchTextSplit = useCallback(async () => {
    // The `text_split_status` command only exists in the desktop binary.
    if (!isTauri() || isMobileApp()) return;
    try {
      const res = await invoke<{ models: TextSplitModelInfo[] }>("text_split_status");
      setTextSplitModels(res.models);
    } catch (e) {
      logError(`text_split_status failed: ${String(e)}`, "models");
    }
  }, []);

  useEffect(() => {
    if (isMobileApp()) return;
    setDesktop(true);
    fetchTextSplit();
  }, [fetchTextSplit]);

  // ---- Event listeners ----

  useEffect(() => {
    if (!isTauri()) return;
    const unlistenProgress = listen<DownloadProgress>("model-download-progress", (event) => {
      setDownloadProgress(event.payload);
    });
    const unlistenChanged = listen("model-status-changed", () => {
      fetchStatus();
      if (!isMobileApp()) fetchTextSplit();
    });
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
    };
  }, [fetchStatus, fetchTextSplit]);

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

  // ---- Text-split (SaT) download / delete ----

  async function handleDownloadTextSplit(id: string, provider: string) {
    if (!isTauri()) return;
    setTsDownloadingId(id);
    setDownloadProgress(null);
    try {
      await invoke("model_download_hf", { id, provider });
    } catch (e) {
      const detail = String(e);
      logError(`Text-split model download '${id}' failed: ${detail}`, "models");
      await message(`Could not download the model:\n${detail}`, { title: "Models", kind: "error" });
    } finally {
      setTsDownloadingId(null);
      await fetchTextSplit();
    }
  }

  // Two-click confirm, mirroring the STT delete flow.
  async function handleDeleteTextSplit(id: string) {
    if (!isTauri()) return;
    if (tsConfirmDelete !== id) { setTsConfirmDelete(id); return; }
    setTsConfirmDelete(null);
    try {
      await invoke("text_split_delete", { id });
      await fetchTextSplit();
    } catch (e) {
      const detail = String(e);
      logError(`Text-split model delete '${id}' failed: ${detail}`, "models");
      await message(`Could not delete the model:\n${detail}`, { title: "Models", kind: "error" });
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

  // Mark a downloaded model as the user's preference — the one the app loads
  // automatically whenever it needs transcription and nothing is loaded yet.
  async function handleSetDefault(modelId: string) {
    if (!isTauri()) return;
    setSettingDefaultId(modelId);
    try {
      await invoke("model_set_default", { version: modelId });
      setDefaultVersion(modelId);
    } catch (e) {
      const detail = String(e);
      logError(`Failed to set default model '${modelId}': ${detail}`, "models");
      await message(`Could not set this model as default:\n${detail}`, {
        title: "Set as default",
        kind: "error",
      });
    } finally {
      setSettingDefaultId(null);
    }
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
            Use <span className="font-medium">Set as default</span> to pick the model loaded automatically when none is running.
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
                isDefault={defaultVersion === model.id}
                onSetDefault={() => handleSetDefault(model.id)}
                defaultSaving={settingDefaultId === model.id}
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

        {/* ─── Text Split (SaT) Models Section — desktop only ─── */}
        {desktop && textSplitModels.length > 0 && (
          <section className="mb-6">
            <div className="flex items-center gap-2 mb-1">
              <h2 className="text-sm font-semibold uppercase tracking-wide text-text-secondary">Text Split</h2>
            </div>
            <p className="text-xs text-text-tertiary mb-3">
              Subword sentence-segmentation models (SaT) used by the <span className="font-medium">Split Text</span> tool and
              by the <span className="font-medium">Split Book</span> stage (which runs SaT 3L Small).
              They load automatically on first use — no Start/Stop needed.
            </p>

            <div className="flex flex-col gap-2">
              {textSplitModels.map((model) => {
                const isDownloading = tsDownloadingId === model.id;
                const pct =
                  isDownloading && downloadProgress && downloadProgress.overall_total_bytes > 0
                    ? Math.min(100, Math.round((downloadProgress.overall_bytes_downloaded / downloadProgress.overall_total_bytes) * 100))
                    : null;
                return (
                  <div key={model.id} className="rounded-xl border border-border-default bg-bg-card p-3">
                    <div className="flex items-center gap-3">
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-medium text-text-primary truncate" title={model.name}>{model.name}</p>
                        <p className="text-xs text-text-tertiary truncate" title={model.description}>{model.description}</p>
                      </div>

                      {model.loaded && (
                        <span className="px-2 py-0.5 rounded-full text-xs font-medium bg-green-500/15 text-green-500 shrink-0">Loaded</span>
                      )}
                      <span className="text-xs text-text-tertiary shrink-0">{model.size_mb} MB</span>

                      {model.downloaded ? (
                        <button
                          className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-medium border transition-colors ${
                            tsConfirmDelete === model.id
                              ? "bg-error-text text-white border-error-text"
                              : "bg-bg-muted text-text-secondary border-border-default hover:bg-error-bg hover:text-error-text"
                          }`}
                          onClick={() => handleDeleteTextSplit(model.id)}
                        >
                          <Trash2 size={14} /> {tsConfirmDelete === model.id ? "Confirm" : "Delete"}
                        </button>
                      ) : (
                        <>
                          <select
                            value={providerByModel[model.id] ?? "huggingface"}
                            onChange={(e) =>
                              setProviderByModel((prev) => ({ ...prev, [model.id]: e.target.value }))
                            }
                            disabled={isDownloading}
                            className="px-2 py-1 rounded-lg bg-bg-muted border border-border-default text-xs text-text-primary hover:bg-bg-hover transition-colors cursor-pointer"
                            title="Download source hub"
                          >
                            {HF_PROVIDERS.map((p) => (
                              <option key={p.key} value={p.key}>{p.label}</option>
                            ))}
                          </select>
                          <button
                            className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-medium bg-accent-bg text-white hover:bg-accent-bg-hover disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                            onClick={() => handleDownloadTextSplit(model.id, providerByModel[model.id] ?? "huggingface")}
                            disabled={isDownloading || downloading}
                          >
                            {isDownloading ? <Loader2 size={14} className="animate-spin" /> : <Download size={14} />}
                            {isDownloading ? "Downloading…" : "Download"}
                          </button>
                        </>
                      )}
                    </div>

                    {isDownloading && pct !== null && (
                      <div className="mt-2">
                        <div className="flex items-center justify-between text-xs text-text-tertiary mb-1">
                          <span>Downloading…</span>
                          <span>{pct}%</span>
                        </div>
                        <div className="h-1.5 rounded-full bg-bg-muted overflow-hidden">
                          <div className="h-full bg-accent transition-[width] duration-200" style={{ width: `${pct}%` }} />
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </section>
        )}
      </main>

      {detailsModel && (
        <ModelDetailsDialog model={detailsModel} onClose={() => setDetailsModel(null)} />
      )}
    </>
  );
}
