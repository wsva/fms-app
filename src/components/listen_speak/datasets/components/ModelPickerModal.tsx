"use client";

import { btnSmSecondary } from "@/lib/datasets/types";

interface ModelOption {
  id: string;
  label: string;
  description: string;
}

export default function ModelPickerModal({
  models,
  loading,
  onSelect,
  onClose,
}: {
  models: ModelOption[];
  loading: boolean;
  onSelect: (modelId: string) => void;
  onClose: () => void;
}) {
  return (
    <div
      className="fixed inset-0 bg-black/50 flex items-center justify-center z-[1000]"
      onClick={onClose}
    >
      <div
        className="bg-bg-card rounded-xl p-6 max-w-[420px] w-[90%] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-[1.1em] font-semibold mb-1">Select Model</h3>
        <p className="text-sm text-text-secondary mb-4">
          Choose a downloaded model to use for subtitle generation.
        </p>
        {loading ? (
          <p className="text-text-secondary">Loading model...</p>
        ) : (
          <div className="flex flex-col gap-2 mb-4">
            {models.map((model) => (
              <button
                key={model.id}
                className="flex flex-col items-start p-3 border border-border-default rounded-lg bg-bg-muted cursor-pointer text-left transition-colors hover:border-accent hover:bg-info-bg"
                onClick={() => onSelect(model.id)}
              >
                <span className="font-medium text-text-primary">{model.label}</span>
                <span className="text-xs text-text-secondary">{model.description}</span>
              </button>
            ))}
          </div>
        )}
        <button
          className={`${btnSmSecondary} w-full`}
          onClick={onClose}
          disabled={loading}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
