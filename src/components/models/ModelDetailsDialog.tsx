"use client";

import { type ModelVersionInfo } from "@/lib/models/types";

export default function ModelDetailsDialog({
  model,
  onClose,
}: {
  model: ModelVersionInfo;
  onClose: () => void;
}) {
  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50" onClick={onClose}>
      <div className="bg-bg-card rounded-xl shadow-2xl max-w-lg w-full mx-4 max-h-[80vh] overflow-y-auto" onClick={(e) => e.stopPropagation()}>
        <div className="p-6">
          <div className="flex items-start justify-between mb-4">
            <div>
              <h3 className="text-lg font-semibold">{model.label}</h3>
              <p className="text-sm text-text-secondary mt-1">{model.description}</p>
            </div>
            <button className="p-1 rounded hover:bg-bg-hover text-text-secondary cursor-pointer" onClick={onClose}>
              <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
            </button>
          </div>

          <div className="space-y-3 text-sm">
            <div className="flex justify-between">
              <span className="text-text-secondary">Engine</span>
              <span className="font-medium">{model.engine_type}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Size</span>
              <span className="font-medium">{model.size_mb} MB</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Translation</span>
              <span className="font-medium">{model.supports_translation ? "Supported" : "Not supported"}</span>
            </div>
            <div>
              <span className="text-text-secondary">Languages</span>
              <div className="flex flex-wrap gap-1 mt-1">
                {model.languages.map((lang) => (
                  <span key={lang} className="text-[10px] px-1.5 py-0.5 rounded bg-bg-muted text-text-tertiary">{lang}</span>
                ))}
              </div>
            </div>

            {/* Accuracy / Speed bars */}
            <div className="pt-2 border-t border-border-default">
              <div className="flex items-center gap-3 mb-2">
                <span className="text-text-secondary w-16">Accuracy</span>
                <div className="flex-1 h-2 bg-bg-progress rounded-full overflow-hidden">
                  <div className="h-full bg-accent-bg rounded-full" style={{ width: `${model.accuracy_score * 100}%` }} />
                </div>
                <span className="text-xs w-8 text-right">{Math.round(model.accuracy_score * 100)}%</span>
              </div>
              <div className="flex items-center gap-3">
                <span className="text-text-secondary w-16">Speed</span>
                <div className="flex-1 h-2 bg-bg-progress rounded-full overflow-hidden">
                  <div className="h-full bg-success-text rounded-full" style={{ width: `${model.speed_score * 100}%` }} />
                </div>
                <span className="text-xs w-8 text-right">{Math.round(model.speed_score * 100)}%</span>
              </div>
            </div>

            {/* URLs */}
            <div className="pt-2 border-t border-border-default space-y-2">
              <div>
                <span className="text-text-secondary text-xs">Download URL</span>
                <div className="flex items-center gap-2 mt-0.5">
                  <code className="text-[11px] text-text-tertiary truncate flex-1">{model.blob_url}</code>
                  <button
                    className="text-[10px] px-2 py-0.5 rounded bg-bg-muted hover:bg-bg-hover cursor-pointer"
                    onClick={() => navigator.clipboard.writeText(model.blob_url)}
                  >
                    Copy
                  </button>
                </div>
              </div>
              <div>
                <span className="text-text-secondary text-xs">HuggingFace</span>
                <div className="flex items-center gap-2 mt-0.5">
                  <code className="text-[11px] text-text-tertiary truncate flex-1">{model.hf_repo_url}</code>
                  <button
                    className="text-[10px] px-2 py-0.5 rounded bg-bg-muted hover:bg-bg-hover cursor-pointer"
                    onClick={() => window.open(model.hf_repo_url, "_blank")}
                  >
                    Open
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
