"use client";

import { answerHostConfirm, useAgentControl } from "@/lib/agent/control";
import { btnGhost } from "./styles";

/** One host the backend wants approved before dialling it, rendered wherever
 *  the user might be: the dock (mounted on every screen) and the Agent page's
 *  Hosting tab. Both read the same store, so answering in one clears it in the
 *  other — and the agent that was refused the connect is unblocked either way. */
export default function AgentConfirmCard({ compact = false }: { compact?: boolean }) {
  const { confirm, note } = useAgentControl();
  if (!confirm) return null;
  const busy = note?.kind === "busy";

  return (
    <div
      className={`rounded-xl border border-accent-bg/40 bg-accent-bg/10 space-y-2 ${
        compact ? "p-3" : "p-4"
      }`}
    >
      <p className="text-sm font-medium text-text-primary">
        Host <strong>{confirm.display_name}</strong>?
      </p>
      <p className="text-xs text-text-secondary break-words">
        {confirm.reason}: <code>{confirm.endpoint}</code>
      </p>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className={btnGhost}
          onClick={() => void answerHostConfirm(true)}
          disabled={busy}
        >
          Approve and connect
        </button>
        <button
          type="button"
          className={btnGhost}
          onClick={() => void answerHostConfirm(false)}
          disabled={busy}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
