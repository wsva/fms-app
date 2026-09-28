"use client";

import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Trash2, ArrowDown, Pause, Play } from "lucide-react";

// ── Types ───────────────────────────────────────────────────────────────────

interface LogEntry {
  timestamp: string;
  level: string;
  message: string;
  module: string;
}

type LevelFilter = "DEBUG" | "INFO" | "WARN" | "ERROR";

const ALL_LEVELS: LevelFilter[] = ["DEBUG", "INFO", "WARN", "ERROR"];

// ── Level colors ────────────────────────────────────────────────────────────

function levelColor(level: string): string {
  switch (level) {
    case "DEBUG":
      return "text-gray-400";
    case "INFO":
      return "text-text-primary";
    case "WARN":
      return "text-yellow-400";
    case "ERROR":
      return "text-red-400";
    default:
      return "text-text-primary";
  }
}

function levelBadgeColor(level: string): string {
  switch (level) {
    case "DEBUG":
      return "bg-gray-600/30 text-gray-400";
    case "INFO":
      return "bg-blue-600/30 text-blue-300";
    case "WARN":
      return "bg-yellow-600/30 text-yellow-400";
    case "ERROR":
      return "bg-red-600/30 text-red-400";
    default:
      return "bg-gray-600/30 text-gray-400";
  }
}

// ── Component ───────────────────────────────────────────────────────────────

export default function LogPage() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [activeFilters, setActiveFilters] = useState<Set<LevelFilter>>(
    new Set(["INFO", "WARN", "ERROR"])
  );
  const [autoScroll, setAutoScroll] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Listen for real-time log events
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen<LogEntry>("app-log", (evt) => {
      setLogs((prev) => {
        const next = [...prev, evt.payload];
        // Keep at most 1000 entries in frontend buffer
        if (next.length > 1000) next.splice(0, next.length - 1000);
        return next;
      });
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Fetch log history on mount
  useEffect(() => {
    if (!isTauri()) return;
    invoke<LogEntry[]>("log_get_history")
      .then((entries) => {
        if (entries.length > 0) {
          setLogs(entries);
        }
      })
      .catch(() => {});
  }, []);

  // Auto-scroll to bottom
  useEffect(() => {
    if (autoScroll && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs, autoScroll]);

  const toggleFilter = useCallback((level: LevelFilter) => {
    setActiveFilters((prev) => {
      const next = new Set(prev);
      if (next.has(level)) {
        next.delete(level);
      } else {
        next.add(level);
      }
      return next;
    });
  }, []);

  const handleClear = useCallback(() => {
    setLogs([]);
    if (isTauri()) {
      invoke("log_clear").catch(() => {});
    }
  }, []);

  // Filtered logs
  const filteredLogs = logs.filter((entry) => activeFilters.has(entry.level as LevelFilter));

  return (
    <div className="flex flex-col h-full bg-bg-base">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border-default shrink-0">
        <h2 className="text-lg font-semibold text-text-primary">Logs</h2>
        <div className="flex items-center gap-2">
          {/* Level filters */}
          {ALL_LEVELS.map((level) => (
            <button
              key={level}
              onClick={() => toggleFilter(level)}
              className={`px-2 py-0.5 rounded text-xs font-mono font-medium transition-colors ${
                activeFilters.has(level)
                  ? levelBadgeColor(level)
                  : "bg-gray-700/20 text-gray-500 line-through"
              }`}
            >
              {level}
            </button>
          ))}

          <div className="w-px h-4 bg-border-default mx-1" />

          {/* Auto-scroll toggle */}
          <button
            onClick={() => setAutoScroll(!autoScroll)}
            className={`p-1.5 rounded transition-colors ${
              autoScroll
                ? "text-accent hover:bg-accent/10"
                : "text-text-tertiary hover:bg-mid-gray/20"
            }`}
            title={autoScroll ? "Auto-scroll ON" : "Auto-scroll OFF"}
          >
            {autoScroll ? <ArrowDown width={16} height={16} /> : <Pause width={16} height={16} />}
          </button>

          {/* Clear button */}
          <button
            onClick={handleClear}
            className="p-1.5 rounded text-text-tertiary hover:text-red-400 hover:bg-red-400/10 transition-colors"
            title="Clear logs"
          >
            <Trash2 width={16} height={16} />
          </button>
        </div>
      </div>

      {/* Log count bar */}
      <div className="flex items-center justify-between px-4 py-1 text-xs text-text-tertiary border-b border-border-default/50 shrink-0">
        <span>
          {filteredLogs.length} entries{filteredLogs.length !== logs.length && ` (${logs.length} total)`}
        </span>
        {autoScroll && (
          <span className="flex items-center gap-1">
            <Play width={10} height={10} className="fill-current" /> Live
          </span>
        )}
      </div>

      {/* Log output */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto p-2 font-mono text-xs min-h-0">
        {filteredLogs.length === 0 ? (
          <div className="flex items-center justify-center h-full text-text-tertiary">
            {logs.length === 0 ? "No logs yet" : "No entries match the current filters"}
          </div>
        ) : (
          filteredLogs.map((entry, i) => (
            <div
              key={`${entry.timestamp}-${i}`}
              className="flex gap-2 py-0.5 px-1 hover:bg-white/5 rounded"
            >
              <span className="text-text-tertiary shrink-0 select-none">{entry.timestamp}</span>
              <span
                className={`shrink-0 w-12 text-center rounded text-[10px] font-bold ${levelBadgeColor(entry.level)}`}
              >
                {entry.level}
              </span>
              <span className={`${levelColor(entry.level)} break-all`}>{entry.message}</span>
              <span className="text-text-tertiary/50 shrink-0 ml-auto pl-2 hidden xl:inline">
                {entry.module}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
