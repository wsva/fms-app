"use client";

import { useState } from "react";
import { ChevronDown, ChevronUp, X, Mic, Scissors, Play, Square } from "lucide-react";
import { useLoadedModels, type LoadedModel, type LoadedModelState } from "@/hooks/useLoadedModels";

/** Dot color per normalized model state — shared by the chips and the drawer. */
function dotClass(state: LoadedModelState): string {
  switch (state) {
    case "loaded":
      return "bg-green-500";
    case "loading":
      return "bg-blue-500 animate-pulse";
    case "error":
      return "bg-red-500";
    case "available":
      return "bg-gray-400";
    default:
      return "bg-gray-300";
  }
}

/** Per-family icon so a type is recognizable without reading its label. */
function TypeIcon({ type, className }: { type: string; className?: string }) {
  if (type === "stt") return <Mic className={className} />;
  if (type === "textsplit") return <Scissors className={className} />;
  return <Mic className={className} />;
}

export default function StatusBar() {
  const { models, active, idleCount, toggleStt } = useLoadedModels();
  const [open, setOpen] = useState(false);

  const nothingToShow = models.length === 0 || (active.length === 0 && idleCount === 0);

  return (
    <div className="flex flex-col shrink-0 border-t border-border-default bg-bg-card text-xs">
      {/* Expanded drawer — grows the bar upward over the content area. */}
      {open && (
        <div className="px-4 py-2 border-b border-border-default bg-bg-muted/40">
          <div className="flex items-center justify-between mb-2">
            <span className="font-medium text-text-primary">Models</span>
            <button
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary cursor-pointer"
              onClick={() => setOpen(false)}
              title="Collapse"
            >
              <X size={14} />
            </button>
          </div>
          <div className="flex flex-col gap-1">
            {models.map((m) => (
              <DrawerRow key={m.type} model={m} onToggleStt={toggleStt} />
            ))}
            {models.length === 0 && (
              <span className="text-text-tertiary">No model information available.</span>
            )}
          </div>
        </div>
      )}

      {/* Compact row — stable single-line height. */}
      <div className="flex items-center justify-between px-4 py-1.5">
        <div className="flex items-center gap-3 min-w-0 overflow-hidden">
          {nothingToShow ? (
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-gray-400 shrink-0" />
              <span className="text-text-secondary">No model loaded</span>
            </div>
          ) : (
            <>
              {active.map((m) => (
                <button
                  key={m.type}
                  className="flex items-center gap-1.5 min-w-0 cursor-pointer hover:opacity-80"
                  onClick={() => setOpen((v) => !v)}
                  title={`${m.fullName}: ${m.id ?? "none"} — ${m.stateText}`}
                >
                  <span className={`w-2 h-2 rounded-full shrink-0 ${dotClass(m.state)}`} />
                  <span className="text-text-tertiary shrink-0">{m.label}</span>
                  <span className="font-medium text-text-primary truncate max-w-[140px]">
                    {m.id ?? "—"}
                  </span>
                </button>
              ))}
              {idleCount > 0 && (
                <button
                  className="text-text-tertiary hover:text-text-secondary cursor-pointer shrink-0"
                  onClick={() => setOpen(true)}
                  title={`${idleCount} model${idleCount > 1 ? "s" : ""} downloaded but idle`}
                >
                  +{idleCount}
                </button>
              )}
            </>
          )}
        </div>

        <button
          className="flex items-center gap-1 p-0.5 rounded text-text-tertiary hover:bg-bg-hover hover:text-text-secondary cursor-pointer shrink-0"
          onClick={() => setOpen((v) => !v)}
          title={open ? "Collapse model details" : "Show model details"}
        >
          {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
        </button>
      </div>
    </div>
  );
}

function DrawerRow({ model, onToggleStt }: { model: LoadedModel; onToggleStt: () => void }) {
  return (
    <div className="flex items-center gap-2">
      <span className={`w-2 h-2 rounded-full shrink-0 ${dotClass(model.state)}`} />
      <TypeIcon type={model.type} className="w-3.5 h-3.5 text-text-tertiary shrink-0" />
      <span className="text-text-secondary w-24 shrink-0">{model.fullName}</span>
      <span className="font-medium text-text-primary truncate">{model.id ?? "—"}</span>
      <span className="text-text-tertiary truncate">{model.caption}</span>
      <span className="ml-auto shrink-0">
        {model.actionable ? (
          <button
            className="flex items-center gap-1 px-2 py-0.5 rounded border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer"
            onClick={onToggleStt}
          >
            {model.state === "loaded" ? <Square size={11} /> : <Play size={11} />}
            {model.state === "loaded" ? "Stop" : "Load"}
          </button>
        ) : (
          <span className="px-2 py-0.5 rounded bg-bg-muted text-text-tertiary">{model.stateText}</span>
        )}
      </span>
    </div>
  );
}
