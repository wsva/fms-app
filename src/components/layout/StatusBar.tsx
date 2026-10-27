"use client";

import { useEffect, useState, type ReactNode } from "react";
import {
  Play,
  Square,
  Eject,
  Mic,
  Scissors,
  ListTodo,
  Server,
  ExternalLink,
  Check,
  X,
  type LucideIcon,
} from "lucide-react";
import { useLoadedModels, type LoadedModel, type LoadedModelState } from "@/hooks/useLoadedModels";
import { useAgentTasks } from "@/hooks/useAgentTasks";
import { useSyncStatus, type SyncServiceStatus } from "@/hooks/useSyncStatus";
import type { AgentTask } from "@/lib/agent/tasks";
import AgentBadge from "@/components/agent/AgentBadge";
import {
  dispatchDockToggle,
  DOCK_STATE_DEFAULT,
  DOCK_STATE_EVENT,
  type DockStateDetail,
} from "@/lib/agent/dock-events";
import { StatusItem, StatusMenuBlock, StatusMenuHeading, StatusMenuRow, StatusMenuSep } from "./StatusItem";

/**
 * The status bar: a VS Code-style strip of buttons. Each item fills the bar's
 * height, and clicking one raises its menu *above* it — there is no drawer and
 * no shared expand state, so two items never fight over who is showing.
 *
 * The strip is desktop-only (`home/page.tsx` renders it when `!mobile`), which
 * is also what lets it own desktop-only commands: the sync service status and
 * the text-split model state have no Android twin.
 *
 * It spans the whole window width as the shell's last row, so the sidebar and
 * the agent dock stop at its top edge. Its height is the shell's var
 * (`--shell-statusbar-h` in globals.css) precisely because those surfaces read
 * it back.
 */

/** Which tab a menu's "open the page" row jumps to. */
export type StatusBarNavigate = (tab: string) => void;

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/** Dot color per normalized model state. */
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

function StateDot({ state, className = "" }: { state: LoadedModelState; className?: string }) {
  return <span className={`h-2 w-2 shrink-0 rounded-full ${dotClass(state)} ${className}`} />;
}

/** Per-family icon so a type is recognizable without reading its label. */
function typeIcon(type: string): LucideIcon {
  if (type === "textsplit") return Scissors;
  return Mic;
}

/**
 * The menu action for a model in its current state, or null when there is
 * nothing to offer. STT toggles load/stop; text-split can only be unloaded
 * (it auto-loads on demand), and only while it is resident.
 */
function actionFor(m: LoadedModel): { label: string; Icon: LucideIcon } | null {
  if (m.type === "stt") {
    return m.state === "loaded" ? { label: "Stop model", Icon: Square } : { label: "Load model", Icon: Play };
  }
  if (m.type === "textsplit") {
    return m.state === "loaded" ? { label: "Unload model", Icon: Eject } : null;
  }
  return null;
}

function ModelItem({
  model,
  onAction,
  onNavigate,
}: {
  model: LoadedModel;
  onAction: (type: string) => void;
  onNavigate?: StatusBarNavigate;
}) {
  const action = actionFor(model);
  return (
    <StatusItem
      label={`${model.fullName}: ${model.id ?? "none"} — ${model.stateText}`}
      title={`${model.fullName} — ${model.stateText}`}
      align="start"
      active={model.state === "loaded"}
      icon={typeIcon(model.type)}
      content={
        <>
          <StateDot state={model.state} />
          <span className="shrink-0 opacity-70">{model.label}</span>
          <span className="max-w-[160px] truncate">{model.id ?? "—"}</span>
        </>
      }
    >
      <StatusMenuHeading>{model.fullName}</StatusMenuHeading>
      <StatusMenuBlock>
        <div className="flex items-center gap-2">
          <StateDot state={model.state} />
          <span className="text-text-primary">{model.stateText}</span>
          <span className="min-w-0 truncate text-text-tertiary">{model.id ?? "—"}</span>
        </div>
        <p className="mt-0.5 text-[11px] text-text-tertiary">{model.caption}</p>
      </StatusMenuBlock>
      <StatusMenuSep />
      {action && (
        <StatusMenuRow
          icon={action.Icon}
          text={action.label}
          onClick={() => void onAction(model.type)}
        />
      )}
      <StatusMenuRow icon={ExternalLink} text="Models page…" onClick={() => onNavigate?.("models")} />
    </StatusItem>
  );
}

// ---------------------------------------------------------------------------
// Agent tasks
// ---------------------------------------------------------------------------

/** How many tasks the menu shows before pointing at the page for the rest. */
const MENU_TASKS = 6;

function TaskItem({
  tasks,
  busy,
  error,
  onComplete,
  onDismiss,
  onNavigate,
}: {
  tasks: AgentTask[];
  busy: string | null;
  error: string | null;
  onComplete: (task: AgentTask) => void;
  onDismiss: (task: AgentTask) => void;
  onNavigate?: StatusBarNavigate;
}) {
  const shown = tasks.slice(0, MENU_TASKS);
  return (
    <StatusItem
      label={`${tasks.length} open agent task${tasks.length === 1 ? "" : "s"}`}
      title="Agent tasks waiting on you"
      align="start"
      icon={ListTodo}
      content={<span>Tasks</span>}
      trailing={
        <span className="min-w-[15px] rounded-full bg-accent-bg px-1 text-center text-[10px] font-semibold leading-[15px] text-white">
          {tasks.length}
        </span>
      }
    >
      <StatusMenuHeading>Waiting on you</StatusMenuHeading>
      {shown.map((t) => (
        <StatusMenuBlock key={t.uuid}>
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0">
              <p className="truncate text-text-primary">{t.title}</p>
              {t.detail && <p className="text-[11px] text-text-tertiary">{t.detail}</p>}
            </div>
            <div className="flex shrink-0 gap-1">
              <button
                type="button"
                className="flex items-center gap-1 rounded border border-border-default px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-bg-hover disabled:opacity-50"
                disabled={busy === t.uuid}
                onClick={() => onComplete(t)}
                title="Jump to it and mark it done"
              >
                <Check size={12} />
                Done
              </button>
              <button
                type="button"
                className="flex items-center gap-1 rounded border border-border-default px-1.5 py-0.5 text-[11px] text-text-tertiary hover:bg-bg-hover disabled:opacity-50"
                disabled={busy === t.uuid}
                onClick={() => onDismiss(t)}
                title="Dismiss without doing it"
              >
                <X size={12} />
                Skip
              </button>
            </div>
          </div>
        </StatusMenuBlock>
      ))}
      <StatusMenuSep />
      {error && (
        <StatusMenuBlock>
          <p className="break-words text-red-500">{error}</p>
        </StatusMenuBlock>
      )}
      <StatusMenuRow
        icon={ExternalLink}
        text={tasks.length > shown.length ? `Show all ${tasks.length} tasks…` : "Open Tasks page…"}
        onClick={() => onNavigate?.("tasks")}
      />
    </StatusItem>
  );
}

// ---------------------------------------------------------------------------
// Sync service
// ---------------------------------------------------------------------------

function SyncItem({
  status,
  role,
  busy,
  error,
  onToggle,
  onNavigate,
}: {
  status: SyncServiceStatus | null;
  role: string;
  busy: boolean;
  error: string | null;
  onToggle: (on: boolean) => void;
  onNavigate?: StatusBarNavigate;
}) {
  const running = Boolean(status?.running);
  const flags = [
    status?.stt && "STT",
    status?.dataset && "datasets",
    status?.tts && "TTS",
  ].filter(Boolean) as string[];

  return (
    <StatusItem
      label={
        running
          ? `Sync service on port ${status?.port} (serving ${flags.join(", ") || "nothing"})`
          : "Sync service stopped"
      }
      title={status?.local_url ?? undefined}
      align="start"
      active={running}
      icon={Server}
      content={
        <>
          <span className={`h-2 w-2 shrink-0 rounded-full ${running ? "bg-green-500" : "bg-gray-400"}`} />
          <span>{running ? `Hub :${status?.port}` : "No hub"}</span>
        </>
      }
    >
      <StatusMenuHeading>{role === "hub" ? "This device is the hub" : "This device is a follower"}</StatusMenuHeading>
      <StatusMenuBlock>
        <p className="text-text-primary">{running ? "Serving phones and linked devices" : "Sync service stopped"}</p>
        {running && (
          <div className="mt-0.5 flex flex-col gap-0.5 text-[11px] text-text-tertiary">
            {status?.local_url && <p className="select-all truncate">{status.local_url}</p>}
            {status?.lan_url && <p className="select-all truncate">{status.lan_url}</p>}
            <p>Serving: {flags.join(", ") || "nothing"}</p>
          </div>
        )}
      </StatusMenuBlock>
      <StatusMenuSep />
      <StatusMenuRow
        icon={running ? Square : Play}
        text={running ? "Stop sync service" : "Start sync service"}
        disabled={busy}
        onClick={() => onToggle(!running)}
      />
      {error && (
        <StatusMenuBlock>
          <p className="break-words text-red-500">{error}</p>
        </StatusMenuBlock>
      )}
      <StatusMenuRow
        icon={ExternalLink}
        text="Devices & Hub page…"
        onClick={() => onNavigate?.("devices-hub")}
      />
    </StatusItem>
  );
}

// ---------------------------------------------------------------------------
// The bar
// ---------------------------------------------------------------------------

export default function StatusBar({ onNavigate }: { onNavigate?: StatusBarNavigate }) {
  const { models, runAction } = useLoadedModels();
  const { open: tasks, busy: taskBusy, error: taskError, complete, dismiss } = useAgentTasks(true);
  const { status, role, busy: syncBusy, error: syncError, setRunning } = useSyncStatus(true);

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

  const dockMark: ReactNode = dock.badge ? (
    <span className="min-w-[15px] rounded-full bg-yellow-500 px-1 text-center text-[10px] font-semibold leading-[15px] text-black">
      {dock.badge}
    </span>
  ) : dock.unread ? (
    <span className="h-2 w-2 rounded-full bg-yellow-500" />
  ) : null;

  return (
    <div className="flex h-[var(--shell-statusbar-h)] shrink-0 items-stretch border-t border-border-default bg-bg-card text-[12px]">
      {/* Left group — the things the app is currently *about*. No
          `overflow-hidden` anywhere on this path: it would clip a menu (and
          `overflow-x: hidden` silently turns the other axis into `auto`). */}
      <div className="flex min-w-0 items-stretch">
        {models.map((m) => (
          <ModelItem key={m.type} model={m} onAction={runAction} onNavigate={onNavigate} />
        ))}
        {tasks.length > 0 && (
          <TaskItem
            tasks={tasks}
            busy={taskBusy}
            error={taskError}
            onComplete={complete}
            onDismiss={dismiss}
            onNavigate={onNavigate}
          />
        )}
        <SyncItem
          status={status}
          role={role}
          busy={syncBusy}
          error={syncError}
          onToggle={setRunning}
          onNavigate={onNavigate}
        />
      </div>

      {/* Right group — the agent dock's permanent entry point: always visible in
          the bar, unlike the opt-in floating button. It toggles over the
          `agent-dock-toggle` seam and wears the same attention marker the dock
          would show, so it is a bare button rather than a menu. */}
      <div className="ml-auto flex shrink-0 items-stretch pl-1.5">
        <StatusItem
          label="Agent mailbox"
          title={dock.open ? "Close the agent mailbox" : "Open the agent mailbox (Ctrl+Space)"}
          align="end"
          active={dock.open}
          onClick={dispatchDockToggle}
          content={<AgentBadge height={11} chip title={null} />}
          trailing={
            // Count and dot are siblings of the badge, not overlays of it: an
            // absolute marker would be painted *under* the chip's black
            // background, hiding the very signal it exists to show.
            dockMark ? <span className="flex shrink-0 items-center">{dockMark}</span> : undefined
          }
        />
      </div>
    </div>
  );
}
