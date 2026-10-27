"use client";

import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp, X, Mic, Scissors, Play, Square, Eject } from "lucide-react";
import { useLoadedModels, type LoadedModel, type LoadedModelState } from "@/hooks/useLoadedModels";
import AgentBadge from "@/components/agent/AgentBadge";
import {
  dispatchDockToggle,
  DOCK_STATE_DEFAULT,
  DOCK_STATE_EVENT,
  type DockStateDetail,
} from "@/lib/agent/dock-events";

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

/**
 * The drawer action for a model in its current state, or null when there is
 * nothing to offer. STT toggles load/stop; text-split can only be unloaded
 * (it auto-loads on demand), and only while it is resident.
 */
function actionFor(m: LoadedModel): { label: string; Icon: typeof Play } | null {
  if (m.type === "stt") {
    return m.state === "loaded" ? { label: "Stop", Icon: Square } : { label: "Load", Icon: Play };
  }
  if (m.type === "textsplit") {
    return m.state === "loaded" ? { label: "Unload", Icon: Eject } : null;
  }
  return null;
}

export default function StatusBar() {
  const { models, active, idleCount, runAction } = useLoadedModels();
  const [open, setOpen] = useState(false);
  // Mirror of the agent dock's presentation state, broadcast by the dock itself
  // (see `lib/agent/dock-events`). Assumes the fresh-install shape until the
  // dock reports otherwise.
  const [dock, setDock] = useState<DockStateDetail>(DOCK_STATE_DEFAULT);

  useEffect(() => {
    const onState = (e: Event) => {
      const detail = (e as CustomEvent<DockStateDetail>).detail;
      if (detail) setDock(detail);
    };
    window.addEventListener(DOCK_STATE_EVENT, onState);
    return () => window.removeEventListener(DOCK_STATE_EVENT, onState);
  }, []);

  const nothingToShow = models.length === 0 || (active.length === 0 && idleCount === 0);

  return (
    <div className="flex flex-col shrink-0 border-t border-border-default bg-bg-card text-sm">
      {/* Expanded drawer — grows the bar upward over the content area. */}
      {open && (
        <div className="px-2.5 py-1.5 border-b border-border-default bg-bg-muted/40">
          <div className="flex items-center justify-between mb-1">
            <span className="font-medium text-text-primary">Models</span>
            <button
              className="p-0.5 rounded hover:bg-bg-hover text-text-tertiary cursor-pointer"
              onClick={() => setOpen(false)}
              title="Collapse"
            >
              <X size={16} />
            </button>
          </div>
          <div className="flex flex-col gap-1">
            {models.map((m) => (
              <DrawerRow key={m.type} model={m} onAction={runAction} />
            ))}
            {models.length === 0 && (
              <span className="text-text-tertiary">No model information available.</span>
            )}
          </div>
        </div>
      )}

      {/* Compact row — stable single-line height. */}
      <div className="flex items-center justify-between px-2.5 py-0.5">
        <div className="flex items-center gap-2.5 min-w-0 overflow-hidden">
          {nothingToShow ? (
            <div className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-full bg-gray-400 shrink-0" />
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
                  <span className={`w-2.5 h-2.5 rounded-full shrink-0 ${dotClass(m.state)}`} />
                  <span className="text-text-tertiary shrink-0">{m.label}</span>
                  <span className="font-medium text-text-primary truncate max-w-[180px]">
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

        <div className="flex items-center gap-1 shrink-0">
          {/* The agent dock's permanent entry point: always visible in the bar,
              unlike the opt-in floating button. Toggles over the
              `agent-dock-toggle` seam, and wears the same attention marker the
              dock would show — count first, plain dot for unread output. */}
          <button
            className={`relative flex items-center gap-1 p-0.5 rounded cursor-pointer shrink-0 hover:bg-bg-hover ${
              dock.open ? "text-text-primary" : "text-text-tertiary hover:text-text-secondary"
            }`}
            onClick={dispatchDockToggle}
            title={dock.open ? "Close the agent mailbox" : "Open the agent mailbox (Ctrl+Space)"}
            aria-label="Toggle the agent mailbox"
            aria-pressed={dock.open}
          >
            <AgentBadge height={12} chip title={null} />
            {/* Count and dot are siblings of the badge, not overlays of it: an
                absolute marker would be painted *under* the chip's black
                background (a positioned element loses to a later in-flow one),
                hiding the very signal it exists to show. */}
            {dock.badge ? (
              <span className="min-w-[16px] h-[16px] px-1 rounded-full bg-yellow-500 text-[10px] font-semibold leading-[16px] text-black text-center shrink-0">
                {dock.badge}
              </span>
            ) : dock.unread ? (
              <span className="w-2.5 h-2.5 rounded-full bg-yellow-500 shrink-0" />
            ) : null}
          </button>

          <button
            className="flex items-center gap-1 p-0.5 rounded text-text-tertiary hover:bg-bg-hover hover:text-text-secondary cursor-pointer shrink-0"
            onClick={() => setOpen((v) => !v)}
            title={open ? "Collapse model details" : "Show model details"}
          >
            {open ? <ChevronDown size={16} /> : <ChevronUp size={16} />}
          </button>
        </div>
      </div>
    </div>
  );
}

function DrawerRow({ model, onAction }: { model: LoadedModel; onAction: (type: string) => void }) {
  const action = actionFor(model);
  return (
    <div className="flex items-center gap-1.5">
      <span className={`w-2.5 h-2.5 rounded-full shrink-0 ${dotClass(model.state)}`} />
      <TypeIcon type={model.type} className="w-4 h-4 text-text-tertiary shrink-0" />
      <span className="text-text-secondary w-28 shrink-0">{model.fullName}</span>
      <span className="font-medium text-text-primary truncate">{model.id ?? "—"}</span>
      <span className="text-text-tertiary truncate">{model.caption}</span>
      <span className="ml-auto shrink-0">
        {action ? (
          <button
            className="flex items-center gap-1 px-1.5 py-0.5 rounded border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer"
            onClick={() => onAction(model.type)}
          >
            <action.Icon size={14} />
            {action.label}
          </button>
        ) : (
          <span className="px-1.5 py-0.5 rounded bg-bg-muted text-text-tertiary">{model.stateText}</span>
        )}
      </span>
    </div>
  );
}
