"use client";

import { useModelStatus } from "@/hooks/useModelStatus";

export default function StatusBar() {
  const { activeModel, isRunning, toggle } = useModelStatus();

  return (
    <div className="flex items-center justify-between px-4 py-1.5 border-t border-border-default bg-bg-card text-xs shrink-0">
      <div className="flex items-center gap-2">
        <span
          className={`w-2 h-2 rounded-full ${
            isRunning ? "bg-green-500" : "bg-gray-400"
          }`}
        />
        <span className="text-text-secondary">
          {activeModel ? (
            <>
              Model: <span className="font-medium text-text-primary">{activeModel}</span>
            </>
          ) : (
            "No model loaded"
          )}
        </span>
      </div>
      <button
        className="px-2 py-0.5 rounded text-xs cursor-pointer transition-colors bg-bg-hover hover:bg-bg-muted text-text-secondary"
        onClick={toggle}
      >
        {activeModel ? "Stop" : "Start"}
      </button>
    </div>
  );
}
