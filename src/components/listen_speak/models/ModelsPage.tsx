"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, ExternalLink } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import { type CardModelInfo, type ModelVersionInfo, type DownloadProgress, type ModelStatus, statusBadgeClass } from "@/lib/models/types";
import ModelCard from "./components/ModelCard";
import ModelDetailsDialog from "./components/ModelDetailsDialog";
import { PageHeader } from "@/components/ui/toolbar";
import { reqStr, useAgentActions } from "@/lib/agent/ui-actions";

// HF-catalog model info, mirroring the Rust `TextSplitModelInfo` / `PhonemeModelInfo`
// shapes. `loaded` is absent for the phoneme model: it has no resident session to
// report, it loads on the next scoring pass and is cached from there.
interface CatalogModelInfo {
  id: string;
  name: string;
  description: string;
  downloaded: boolean;
  loaded?: boolean;
  size_mb: number;
}

/** Engine chip text: which catalog family a card belongs to. */
type CatalogEngine = "textsplit" | "phoneme";

/**
 * Feed an HF-catalog entry to the same card the STT models use. The catalog calls the
 * title `name` and has no engine field, so the two are mapped here rather than the card
 * growing a second markup branch per catalog.
 */
function asCardModel(m: CatalogModelInfo, engineType: CatalogEngine): CardModelInfo {
  return {
    id: m.id,
    label: m.name,
    description: m.description,
    downloaded: m.downloaded,
    engine_type: engineType,
    size_mb: m.size_mb,
  };
}

/**
 * Catalog models are loaded by the app on first use, so of the STT card's lifecycle
 * only three states mean anything: a download in flight, the weights resident in memory
 * (the card's "active" green, labelled `Loaded` through the badge override), and present
 * on disk doing nothing.
 */
function catalogCardStatus(m: CatalogModelInfo, isDownloading: boolean): ModelStatus {
  if (isDownloading) return "Downloading";
  if (m.loaded) return "Running";
  if (m.downloaded) return "Downloaded";
  return "NotDownloaded";
}

// Download sources for HuggingFace-catalog models. The key is sent to
// `model_download_hf`, which maps it to a backend `HubProvider`.
const HF_PROVIDERS = [
  { key: "huggingface", label: "Hugging Face" },
  { key: "hf-mirror", label: "HF Mirror" },
  { key: "modelscope", label: "ModelScope" },
] as const;

/** Hub picker shown on an undownloaded catalog card. */
function ProviderSelect({ value, onChange }: { value: string; onChange: (next: string) => void }) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="px-2 py-1 rounded-lg bg-bg-muted border border-border-default text-xs text-text-primary hover:bg-bg-hover transition-colors cursor-pointer"
      title="Download source hub"
    >
      {HF_PROVIDERS.map((p) => (
        <option key={p.key} value={p.key}>{p.label}</option>
      ))}
    </select>
  );
}

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
  const [textSplitModels, setTextSplitModels] = useState<CatalogModelInfo[]>([]);
  const [tsDownloadingId, setTsDownloadingId] = useState<string | null>(null);
  const [tsConfirmDelete, setTsConfirmDelete] = useState<string | null>(null);
  // Pronunciation (phoneme) model: the read-aloud scorer's acoustic model. Same
  // catalog mechanics, different directory tree (`{model_root}/phoneme/`).
  const [phonemeModels, setPhonemeModels] = useState<CatalogModelInfo[]>([]);
  const [phonemeDir, setPhonemeDir] = useState<string | null>(null);
  const [phDownloadingId, setPhDownloadingId] = useState<string | null>(null);
  const [phConfirmDelete, setPhConfirmDelete] = useState<string | null>(null);
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
      const res = await invoke<{ models: CatalogModelInfo[] }>("text_split_status");
      setTextSplitModels(res.models);
    } catch (e) {
      logError(`text_split_status failed: ${String(e)}`, "models");
    }
  }, []);

  // ---- Pronunciation (phoneme) model status ----

  const fetchPhoneme = useCallback(async () => {
    if (!isTauri() || isMobileApp()) return;
    try {
      const res = await invoke<{ installed_dir: string | null; models: CatalogModelInfo[] }>(
        "read_aloud_phoneme_model_status"
      );
      setPhonemeModels(res.models);
      setPhonemeDir(res.installed_dir ?? null);
    } catch (e) {
      logError(`read_aloud_phoneme_model_status failed: ${String(e)}`, "models");
    }
  }, []);

  useEffect(() => {
    if (isMobileApp()) return;
    setDesktop(true);
    fetchTextSplit();
    fetchPhoneme();
  }, [fetchTextSplit, fetchPhoneme]);

  // ---- Event listeners ----

  useEffect(() => {
    if (!isTauri()) return;
    const unlistenProgress = listen<DownloadProgress>("model-download-progress", (event) => {
      setDownloadProgress(event.payload);
    });
    const unlistenChanged = listen("model-status-changed", () => {
      fetchStatus();
      if (!isMobileApp()) {
        fetchTextSplit();
        fetchPhoneme();
      }
    });
    return () => {
      unlistenProgress.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
    };
  }, [fetchStatus, fetchTextSplit, fetchPhoneme]);

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

  // Cancelling is safe by construction: `hf_downloaded_on_disk` requires every listed
  // file, so a half-fetched model still reads as not downloaded and can be started again.
  async function handleCancelTextSplit(id: string) {
    if (!isTauri()) return;
    try { await invoke("model_cancel_download", { version: id }); }
    catch (e) { logError(`Text-split download cancel '${id}' failed: ${String(e)}`, "models"); }
  }

  // ---- Pronunciation (phoneme) download / delete ----

  async function handleDownloadPhoneme(id: string, provider: string) {
    if (!isTauri()) return;
    setPhDownloadingId(id);
    setDownloadProgress(null);
    try {
      await invoke("model_download_hf", { id, provider });
    } catch (e) {
      const detail = String(e);
      logError(`Phoneme model download '${id}' failed: ${detail}`, "models");
      await message(`Could not download the model:\n${detail}`, { title: "Models", kind: "error" });
    } finally {
      setPhDownloadingId(null);
      await fetchPhoneme();
    }
  }

  async function handleDeletePhoneme(id: string) {
    if (!isTauri()) return;
    if (phConfirmDelete !== id) { setPhConfirmDelete(id); return; }
    setPhConfirmDelete(null);
    try {
      await invoke("read_aloud_phoneme_model_delete", { id });
      await fetchPhoneme();
    } catch (e) {
      const detail = String(e);
      logError(`Phoneme model delete '${id}' failed: ${detail}`, "models");
      await message(`Could not delete the model:\n${detail}`, { title: "Models", kind: "error" });
    }
  }

  async function handleCancelPhoneme(id: string) {
    if (!isTauri()) return;
    try { await invoke("model_cancel_download", { version: id }); }
    catch (e) { logError(`Phoneme download cancel '${id}' failed: ${String(e)}`, "models"); }
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
  // One download at a time is a backend rule — `model_download` and `model_download_hf`
  // both refuse while anything is in flight — so a card greys out for any card's run,
  // not just its own section's.
  const anyDownloadInProgress = isDownloadingAny || tsDownloadingId !== null || phDownloadingId !== null;
  const downloadedModels = models.filter((m) => m.downloaded);
  const availableModels = models.filter((m) => !m.downloaded);

  // ── Agent-drivable actions ──
  // Loading and defaulting an STT model are the two lifecycle moves an agent
  // would drive (downloads stay in the UI: they are long, cancellable, and the
  // progress panel is the only honest view of them). Both surface their own
  // result rather than the dialogs the click handlers show, so a failure comes
  // back to the caller instead of a modal nobody is watching.
  useAgentActions("models", [
    {
      id: "models.load",
      label:
        "Select and start a downloaded STT model so transcription uses it. Fails if the model is not downloaded yet.",
      kind: "mutate",
      params_schema: {
        model_id: {
          type: "string",
          description: "The model id, from model_get_status / the downloaded list.",
          required: true,
        },
      },
      run: async (params) => {
        const id = reqStr("models.load", params, "model_id");
        const found = models.find((m) => m.id === id);
        if (!found) {
          throw new Error(
            `no model with id '${id}'. Downloaded: ${
              downloadedModels.map((m) => `${m.label} (${m.id})`).join(", ") || "(none)"
            }. model_get_status lists the full catalog.`
          );
        }
        if (!found.downloaded) {
          throw new Error(
            `model '${id}' (${found.label}) is not downloaded yet; download it first (model_download).`
          );
        }
        // handleStart swallows its errors into console.error, so read the status
        // back afterwards and report what actually happened.
        await handleStart(id);
        const res = await invoke<{ active_version: string | null; active_status: ModelStatus }>(
          "model_get_status"
        );
        return {
          model_id: id,
          label: found.label,
          active_version: res.active_version,
          active_status: res.active_status,
        };
      },
    },
    {
      id: "models.set-default",
      label:
        "Mark a downloaded STT model as the app's default, loaded automatically when transcription is needed and nothing is resident.",
      kind: "mutate",
      params_schema: {
        model_id: {
          type: "string",
          description: "The model id to make the default.",
          required: true,
        },
      },
      run: async (params) => {
        const id = reqStr("models.set-default", params, "model_id");
        const found = models.find((m) => m.id === id);
        if (!found) {
          throw new Error(
            `no model with id '${id}'. Downloaded: ${
              downloadedModels.map((m) => `${m.label} (${m.id})`).join(", ") || "(none)"
            }.`
          );
        }
        if (!found.downloaded) {
          throw new Error(
            `model '${id}' (${found.label}) is not downloaded yet; download it before setting it as default.`
          );
        }
        // Not handleSetDefault: that shows a dialog on failure, which an agent
        // cannot read. Invoke directly so the rejection propagates as the result.
        await invoke("model_set_default", { version: id });
        setDefaultVersion(id);
        return { default_version: id, label: found.label };
      },
    },
  ]);

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
      <main className="flex-1 flex flex-col h-full min-h-0">
        <PageHeader title="Models" />
        <div className="flex-1 overflow-y-auto p-6 min-h-0">
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
                isDownloadInProgress={anyDownloadInProgress}
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
                      isDownloadInProgress={anyDownloadInProgress}
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
                const provider = providerByModel[model.id] ?? "huggingface";
                // The catalog's `id` is the HuggingFace repo id, so the repo page is one hop
                // away — the link an STT card keeps inside its details dialog.
                const repoUrl = `https://huggingface.co/${model.id}`;
                return (
                  <ModelCard
                    key={model.id}
                    model={asCardModel(model, "textsplit")}
                    status={catalogCardStatus(model, isDownloading)}
                    isDownloading={isDownloading}
                    isDownloadInProgress={anyDownloadInProgress}
                    downloadProgress={downloadProgress}
                    modelLoading={false}
                    confirmDelete={tsConfirmDelete === model.id}
                    autoLoads
                    externalUrl={repoUrl}
                    badge={model.loaded ? <span className={statusBadgeClass("Running")}>Loaded</span> : undefined}
                    providerSelector={!model.downloaded && !isDownloading ? (
                      <ProviderSelect
                        value={provider}
                        onChange={(next) =>
                          setProviderByModel((prev) => ({ ...prev, [model.id]: next }))
                        }
                      />
                    ) : undefined}
                    onDownload={() => handleDownloadTextSplit(model.id, provider)}
                    onCancel={() => handleCancelTextSplit(model.id)}
                    onLoad={() => {}}
                    onStop={() => {}}
                    onDelete={() => handleDeleteTextSplit(model.id)}
                    onCancelDelete={() => setTsConfirmDelete(null)}
                  />
                );
              })}
            </div>
          </section>
        )}

        {/* ─── Pronunciation (phoneme) Model Section — desktop only ─── */}
        {desktop && phonemeModels.length > 0 && (
          <section className="mb-6">
            <div className="flex items-center gap-2 mb-1">
              <h2 className="text-sm font-semibold uppercase tracking-wide text-text-secondary">Pronunciation</h2>
            </div>
            <p className="text-xs text-text-tertiary mb-3">
              Phone-level acoustic model used by <span className="font-medium">Read Aloud</span> to score how the words
              sounded, not just whether they were recognised. One IPA inventory covers every language, so the same model
              serves all of your read-aloud texts. It loads on the first scored take — no Start/Stop needed.
              {phonemeDir && (
                <> Installed at <span className="font-mono break-all">{phonemeDir}</span>.</>
              )}
            </p>

            <div className="flex flex-col gap-2">
              {phonemeModels.map((model) => {
                const isDownloading = phDownloadingId === model.id;
                const provider = providerByModel[model.id] ?? "huggingface";
                return (
                  <ModelCard
                    key={model.id}
                    model={asCardModel(model, "phoneme")}
                    status={catalogCardStatus(model, isDownloading)}
                    isDownloading={isDownloading}
                    isDownloadInProgress={anyDownloadInProgress}
                    downloadProgress={downloadProgress}
                    modelLoading={false}
                    confirmDelete={phConfirmDelete === model.id}
                    autoLoads
                    externalUrl={`https://huggingface.co/${model.id}`}
                    badge={phonemeDir && model.downloaded
                      ? <span className={statusBadgeClass("Running")}>In use</span>
                      : undefined}
                    providerSelector={!model.downloaded && !isDownloading ? (
                      <ProviderSelect
                        value={provider}
                        onChange={(next) =>
                          setProviderByModel((prev) => ({ ...prev, [model.id]: next }))
                        }
                      />
                    ) : undefined}
                    onDownload={() => handleDownloadPhoneme(model.id, provider)}
                    onCancel={() => handleCancelPhoneme(model.id)}
                    onLoad={() => {}}
                    onStop={() => {}}
                    onDelete={() => handleDeletePhoneme(model.id)}
                    onCancelDelete={() => setPhConfirmDelete(null)}
                  />
                );
              })}
            </div>
          </section>
        )}
        </div>
      </main>

      {detailsModel && (
        <ModelDetailsDialog model={detailsModel} onClose={() => setDetailsModel(null)} />
      )}
    </>
  );
}
