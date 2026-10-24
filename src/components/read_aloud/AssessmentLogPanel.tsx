"use client";

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Check, ChevronDown, ChevronUp, Copy, Loader2, Terminal, X } from "lucide-react";
import {
  clearAssessmentLog,
  getAssessmentLog,
  subscribe,
  type AssessmentLogLevel,
} from "@/lib/read_aloud/assessmentLog";

const LEVEL_COLOR: Record<AssessmentLogLevel, string> = {
  DEBUG: "text-text-tertiary",
  INFO: "text-text-secondary",
  WARN: "text-warning-text",
  ERROR: "text-error-text",
};

/**
 * Bottom-docked live log of the running assessment.
 *
 * Re-assessment is a long, otherwise silent wait (STT, then fluency and
 * pronunciation against the standard audio), so this window tails the runner's
 * own steps interleaved with the backend's `app-log` lines. It appears with the
 * first run and stays as a record of that run until the next one replaces it;
 * collapse it to a one-line header or close it outright.
 */
export default function AssessmentLogPanel() {
  const log = useSyncExternalStore(subscribe, getAssessmentLog, getAssessmentLog);
  const [open, setOpen] = useState(true);
  const [copied, setCopied] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  // Follow new lines only while the reader is already at the bottom — scrolling
  // up to read an earlier step pauses the tail, scrolling back resumes it.
  const pinnedRef = useRef(true);

  // A new run re-opens the window: watching it is the whole point.
  useEffect(() => {
    if (log.running) {
      setOpen(true);
      pinnedRef.current = true;
    }
  }, [log.running]);

  useEffect(() => {
    const el = bodyRef.current;
    if (el && open && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [log.entries, open]);

  const handleScroll = useCallback(() => {
    const el = bodyRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 24;
  }, []);

  const handleCopy = useCallback(() => {
    const text = log.entries
      .map((e) => `${e.time} ${e.level} ${e.message}`)
      .join("\n");
    navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      })
      .catch(() => {
        /* clipboard unavailable */
      });
  }, [log.entries]);

  if (log.entries.length === 0) return null;

  const seconds = (log.elapsedMs / 1000).toFixed(1);
  const status = log.running
    ? `Running… ${seconds}s`
    : log.ok
      ? `Finished in ${seconds}s`
      : `Failed after ${seconds}s`;
  const statusClass = log.running
    ? "text-text-secondary"
    : log.ok
      ? "text-success-text"
      : "text-error-text";

  return (
    <div className="shrink-0 min-w-0 border-t border-border-default bg-bg-card">
      <div className="flex items-center gap-2 px-3 py-1.5 min-w-0">
        <Terminal size={14} className="text-text-tertiary shrink-0" />
        <span className="text-xs font-medium text-text-primary shrink-0">Assessment log</span>
        <span className={`flex items-center gap-1 text-[11px] tabular-nums min-w-0 truncate ${statusClass}`}>
          {log.running && <Loader2 size={11} className="animate-spin shrink-0" />}
          {status}
        </span>
        <div className="flex-1 min-w-0" />
        <button
          type="button"
          onClick={handleCopy}
          title="Copy the log"
          aria-label="Copy the log"
          className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          title={open ? "Collapse the log" : "Expand the log"}
          aria-label={open ? "Collapse the log" : "Expand the log"}
          aria-expanded={open}
          className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
        >
          {open ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
        </button>
        <button
          type="button"
          onClick={clearAssessmentLog}
          title="Close the log"
          aria-label="Close the log"
          className="p-1 rounded text-text-tertiary hover:text-text-primary hover:bg-bg-hover cursor-pointer shrink-0"
        >
          <X size={14} />
        </button>
      </div>

      {open && (
        <div
          ref={bodyRef}
          onScroll={handleScroll}
          role="log"
          aria-live="polite"
          className="max-h-[28vh] overflow-y-auto overflow-x-hidden px-3 pb-2 flex flex-col gap-0.5 font-mono text-[11px] leading-5"
        >
          {log.entries.map((e) => (
            <div key={e.id} className="flex gap-2 min-w-0">
              <span className="text-text-tertiary tabular-nums shrink-0">{e.time}</span>
              <span className={`shrink-0 w-11 ${LEVEL_COLOR[e.level]}`}>{e.level}</span>
              <span className="text-text-primary whitespace-pre-wrap break-words min-w-0">
                {e.message}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
