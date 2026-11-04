"use client";

/**
 * ArtifactGenerateButton — the one "get this artifact made" button a tab shows.
 *
 * Two roads exist for every generated artifact and they are peers, but they
 * are not the same action: calling the configured LLM runs here and now,
 * while asking an agent writes a mailbox request an attached agent claims
 * later. The button *is* the choice: one click opens the two roads and names
 * each one's consequence, so the learner decides with the decision in front
 * of them rather than defaulting into one road. On a phone the agent road
 * cannot exist (agents attach to the desktop MCP server, a job here would
 * never be picked up), so there is nothing to choose and the button runs the
 * direct call at once.
 *
 * The menu row says "AGENT" in the brand badge — the vocabulary the mailbox
 * and the MCP tool descriptions already use.
 */

import { useEffect, useRef, useState } from "react";
import { Loader2, Wand2 } from "lucide-react";
import AgentBadge from "@/components/agent/AgentBadge";

type Props = {
  /** What clicking produces: "Explain this paragraph", "Digest chapter",
   *  or "Regenerate" once an artifact exists. */
  label: string;
  disabled?: boolean;
  /** The busy flag shared by both roads (generation in flight). */
  busy?: boolean;
  /** False on a phone: no menu, one click runs the direct call. */
  showAgent: boolean;
  /** The model a direct call will use ("" = the app default from Settings). */
  modelName?: string;
  onCallLlm: () => void;
  onAskAgent: () => void;
};

export default function ArtifactGenerateButton({
  label,
  disabled,
  busy,
  showAgent,
  modelName,
  onCallLlm,
  onAskAgent,
}: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const icon = busy ? (
    <Loader2 size={13} className="animate-spin" />
  ) : (
    <Wand2 size={13} />
  );
  const fire = (fn: () => void) => {
    setOpen(false);
    fn();
  };

  // One road only: no choice to open, direct call.
  if (!showAgent) {
    return (
      <button
        className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
        disabled={disabled}
        onClick={onCallLlm}
      >
        {icon}
        {label}
      </button>
    );
  }

  return (
    <div className="relative shrink-0" ref={ref}>
      {/* The button itself opens the menu — the click is the choice. */}
      <button
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
        disabled={disabled}
        title="Choose how to generate: your LLM now, or an attached agent"
        onClick={() => setOpen((v) => !v)}
      >
        {icon}
        {label}
      </button>
      {open && (
        <div role="menu" className="absolute right-0 top-full mt-1 z-50 w-72 rounded-lg border border-border-default bg-bg-card shadow-2xl flex flex-col py-1">
          <button
            role="menuitem"
            className="flex flex-col items-start gap-0.5 px-3 py-2 text-left hover:bg-bg-hover cursor-pointer"
            onClick={() => fire(onCallLlm)}
          >
            <span className="flex items-center gap-1.5 text-xs font-medium text-text-primary">
              <Wand2 size={12} className="text-accent shrink-0" />
              Call LLM directly
            </span>
            <span className="text-[11px] text-text-tertiary">
              Runs now with {modelName?.trim() || "the app default (Settings)"}.
            </span>
          </button>
          <button
            role="menuitem"
            className="flex flex-col items-start gap-0.5 px-3 py-2 text-left hover:bg-bg-hover cursor-pointer border-t border-border-default"
            onClick={() => fire(onAskAgent)}
          >
            <span className="flex items-center gap-1.5 text-xs font-medium text-text-primary">
              <AgentBadge height={11} chip title={null} />
              Ask an agent
            </span>
            <span className="text-[11px] text-text-tertiary">
              Queued as a request; an attached agent claims and delivers it.
            </span>
          </button>
        </div>
      )}
    </div>
  );
}
