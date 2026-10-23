"use client";

import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { Trash2, ArrowDown, Play, FileText, Radio } from "lucide-react";
import { PageHeader, Toolbar, ToolbarButton } from "@/components/ui/toolbar";

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

// Background tint + neutral text for the inline log-entry level badges, matching
// the filter chips: the hue lives in the background, the label text stays the
// same `text-text-secondary` used across the toolbar.
function levelBadgeColor(level: string): string {
  switch (level) {
    case "DEBUG":
      return "bg-gray-600/30 text-text-secondary";
    case "INFO":
      return "bg-blue-600/30 text-text-secondary";
    case "WARN":
      return "bg-yellow-600/30 text-text-secondary";
    case "ERROR":
      return "bg-red-600/30 text-text-secondary";
    default:
      return "bg-gray-600/30 text-text-secondary";
  }
}

// Background-only tints for the level FILTER chips. Text always uses the same
// neutral toolbar color (`text-text-secondary`) so the background hue alone
// distinguishes the levels.
function levelChipBg(level: string): string {
  switch (level) {
    case "DEBUG":
      return "bg-gray-600/30";
    case "INFO":
      return "bg-blue-600/30";
    case "WARN":
      return "bg-yellow-600/30";
    case "ERROR":
      return "bg-red-600/30";
    default:
      return "bg-gray-600/30";
  }
}

// ── Component ───────────────────────────────────────────────────────────────

export default function LogPage() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [source, setSource] = useState<"memory" | "file">("memory");
  const [logFilePath, setLogFilePath] = useState("");
  const [activeFilters, setActiveFilters] = useState<Set<LevelFilter>>(
    new Set(["INFO", "WARN", "ERROR"])
  );
  // Smart auto-scroll: stick to the newest entry only while the view is already
  // pinned at the bottom. Scrolling up pauses it; returning to the bottom (or
  // "Jump to latest") resumes. Replaces the old explicit Auto-scroll toggle.
  const [atBottom, setAtBottom] = useState(true);
  const atBottomRef = useRef(true);
  const scrollRef = useRef<HTMLDivElement>(null);
  // The live listener must not mix streamed entries into a static file snapshot.
  const sourceRef = useRef<"memory" | "file">("memory");
  sourceRef.current = source;

  // Listen for real-time log events
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen<LogEntry>("app-log", (evt) => {
      if (sourceRef.current !== "memory") return;
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

  // Within a small threshold counts as "at the bottom", so sub-pixel rounding or
  // a not-yet-laid-out line doesn't break the stickiness.
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight <= 40;
    if (near !== atBottomRef.current) {
      atBottomRef.current = near;
      setAtBottom(near);
    }
  }, []);

  const jumpToLatest = useCallback(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    atBottomRef.current = true;
    setAtBottom(true);
  }, []);

  // Re-pin on a source switch (memory <-> file) so the newest lines show and
  // auto-scroll resumes. Declared before the follow effect so that when both
  // `source` and `logs` change in one commit, the reset lands first.
  useEffect(() => {
    atBottomRef.current = true;
    setAtBottom(true);
  }, [source]);

  // Follow new entries only while already pinned at the bottom.
  useEffect(() => {
    if (atBottomRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs]);

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

  // Swap between the in-memory ring buffer (last 1000 entries, live) and the
  // persisted log file (full history, survives restarts and clears).
  const handleToggleSource = useCallback(() => {
    if (!isTauri()) return;
    if (source === "memory") {
      Promise.all([
        invoke<string>("log_get_file_path"),
        invoke<LogEntry[]>("log_read_file_history", { limit: 5000 }),
      ])
        .then(([path, entries]) => {
          setLogFilePath(path);
          setLogs(entries);
          setSource("file");
        })
        .catch(() => {});
    } else {
      invoke<LogEntry[]>("log_get_history")
        .then((entries) => {
          setLogs(entries);
          setSource("memory");
        })
        .catch(() => {});
    }
  }, [source]);

  // Filtered logs
  const filteredLogs = logs.filter((entry) => activeFilters.has(entry.level as LevelFilter));

  return (
    <div className="flex flex-1 min-w-0 flex-col h-full bg-bg-base">
      <PageHeader title="Logs" />
      {/* Toolbar — page-level actions. Wraps on narrow screens. */}
      <Toolbar className="flex-wrap">
        <div className="flex-1 min-w-0" />
        {/* Memory / file source toggle */}
        <ToolbarButton
          variant="toggle"
          active={source === "file"}
          icon={source === "memory" ? FileText : Radio}
          label={source === "memory" ? "Memory" : "File"}
          onClick={handleToggleSource}
          title={
            source === "memory"
              ? "Read the persisted log file (full history, survives restarts)"
              : "Back to the live in-memory buffer (last 1000 entries)"
          }
        />
        {/* Clear button */}
        <ToolbarButton
          icon={Trash2}
          label="Clear"
          onClick={handleClear}
          title="Clear logs"
        />
      </Toolbar>

      {/* Sub-toolbar — log-level filters. On their own row so the action
          toolbar stays compact and the chips can wrap freely on a phone. */}
      <Toolbar className="flex-wrap">
          {ALL_LEVELS.map((level) => (
            <button
              key={level}
              onClick={() => toggleFilter(level)}
              className={`px-2 py-0.5 rounded text-xs font-mono font-medium transition-colors ${
                activeFilters.has(level)
                  ? `${levelChipBg(level)} text-text-secondary`
                  : "bg-gray-700/20 text-gray-500 line-through"
              }`}
            >
              {level}
            </button>
          ))}
      </Toolbar>

      {/* Log count bar */}
      <div className="flex flex-wrap items-center justify-between gap-x-3 px-4 py-1 text-xs text-text-tertiary border-b border-border-default/50 shrink-0">
        <span className="shrink-0">
          {filteredLogs.length} entries{filteredLogs.length !== logs.length && ` (${logs.length} total)`}
        </span>
        {source === "file" ? (
          <span className="truncate max-w-full sm:max-w-[50%] min-w-0 font-mono" title={logFilePath}>
            file: {logFilePath}
          </span>
        ) : atBottom ? (
          <span className="flex items-center gap-1">
            <Play width={10} height={10} className="fill-current" /> Live
          </span>
        ) : (
          <button
            onClick={jumpToLatest}
            className="flex items-center gap-1 text-accent hover:underline"
            title="Scroll to the newest entry and resume auto-follow"
          >
            <ArrowDown width={12} height={12} /> Jump to latest
          </button>
        )}
      </div>

      {/* Log output */}
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto overflow-x-hidden p-2 font-mono text-xs min-h-0 min-w-0"
      >
        {filteredLogs.length === 0 ? (
          <div className="flex items-center justify-center h-full text-text-tertiary">
            {logs.length === 0 ? (source === "file" ? "Log file is empty" : "No logs yet") : "No entries match the current filters"}
          </div>
        ) : (
          filteredLogs.map((entry, i) => (
            <div
              key={`${entry.timestamp}-${i}`}
              className="flex flex-col sm:flex-row sm:items-baseline gap-y-0.5 sm:gap-x-2 py-0.5 px-1 hover:bg-white/5 rounded"
            >
              {/* Line 1 (narrow) / leading group (wide): timestamp + level tag.
                  The level tag sits right after the time string, and on narrow
                  screens the message wraps to a second line so the long
                  timestamp doesn't squeeze it. */}
              <div className="flex items-center gap-2 shrink-0">
                <span className="text-text-tertiary select-none">{entry.timestamp}</span>
                <span
                  className={`shrink-0 w-12 text-center rounded text-[10px] font-bold ${levelBadgeColor(entry.level)}`}
                >
                  {entry.level}
                </span>
              </div>
              <span className={`${levelColor(entry.level)} min-w-0 flex-1 break-all`}>{entry.message}</span>
              <span className="text-text-tertiary/50 shrink-0 sm:ml-auto pl-2 hidden xl:inline">
                {entry.module}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
