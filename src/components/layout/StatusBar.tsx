"use client";

import { useEffect, useState, type ReactNode } from "react";
import {
  Play,
  Square,
  Mic,
  Scissors,
  Cpu,
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

/** A hub model id is `owner/name` (`segment-any-text/sat-3l-sm`). The owner is
 *  the publisher, not the model, and every model a catalogue ships shares it —
 *  so the row shows the name alone and keeps the full id in the line's title.
 *  A plain `stt/<version>` id has no owner and comes back untouched. */
function modelName(id: string): string {
  const slash = id.lastIndexOf("/");
  return slash === -1 ? id : id.slice(slash + 1);
}

/**
 * The one verb a family offers right now, or null when there is none.
 *
 * A verb is offered only in a state it can honour: STT can be loaded once its
 * files are on disk (and a failed load can be retried) and stopped while it is
 * resident; text-split can only be unloaded, because it loads itself on first
 * use. A button that is offered but cannot succeed is worse than no button — it
 * costs a click to learn nothing.
 */
function actionLabel(m: LoadedModel): string | null {
  if (m.type === "stt") {
    if (m.state === "loaded") return "Stop";
    if (m.state === "available" || m.state === "error") return "Load";
    return null;
  }
  if (m.type === "textsplit") return m.state === "loaded" ? "Unload" : null;
  return null;
}

/** The bar shows how many models are in memory; the menu names each family, its
 *  state and the model behind it, so the two never say the same thing twice. */
function ModelsItem({
  models,
  onAction,
  onNavigate,
}: {
  models: LoadedModel[];
  onAction: (type: string) => void;
  onNavigate?: StatusBarNavigate;
}) {
  const loaded = models.filter((m) => m.state === "loaded").length;
  const anyError = models.some((m) => m.state === "error");
  const anyLoading = models.some((m) => m.state === "loading");
  const anyAvailable = models.some((m) => m.state === "available");

  // One dot for the whole group, worst state first: a failure must not be
  // hidden behind a healthy sibling, and "in flight" outranks "settled".
  const state: LoadedModelState = anyError
    ? "error"
    : anyLoading
      ? "loading"
      : loaded > 0
        ? "loaded"
        : anyAvailable
          ? "available"
          : "none";
  // The families are separate resources (STT and text-split live in different
  // binaries on Android), so a glance asks "how many are in memory?", not
  // "which one is on" — hence one chip with a count.
  const detail = models
    .map((m) => `${m.fullName}: ${m.stateText.toLowerCase()}`)
    .join(" · ");

  return (
    <StatusItem
      label={`Models — ${loaded} of ${models.length} loaded`}
      title={detail}
      align="start"
      active={loaded > 0}
      icon={Cpu}
      content={
        <>
          <StateDot state={state} />
          <span>Models</span>
          <span className="shrink-0 opacity-70">({loaded})</span>
        </>
      }
    >
      {/* One row per family, and each fact on it exactly once: the name says
          what it is, one muted line says where it stands (dot + word + the
          model in use), and the one verb it offers sits at the end of the row.
          The state used to be written three times — dot, word and a caption —
          and the action floated between families under a repeated name, which
          read as the next family's subtitle.

          The body is a fixed width because its widest child is a model id:
          without one the panel would resize itself every time an id changed
          length, and the ids are the only thing here long enough to drag it. */}
      <div className="w-[268px]">
        <StatusMenuHeading>Models</StatusMenuHeading>
        {models.map((m) => {
          const action = actionLabel(m);
          const FamilyIcon = typeIcon(m.type);
          return (
            <div key={m.type} className="flex items-start gap-2 px-3 py-1.5">
              <FamilyIcon size={14} className="mt-[3px] shrink-0 opacity-70" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-text-primary">{m.fullName}</p>
                <p className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px]">
                  <StateDot state={m.state} />
                  <span className={`shrink-0 ${m.state === "error" ? "text-red-500" : "text-text-tertiary"}`}>
                    {m.stateText}
                  </span>
                  {m.id && (
                    <span className="truncate text-text-tertiary" title={m.id}>
                      · {modelName(m.id)}
                    </span>
                  )}
                </p>
              </div>
              {action && (
                <button
                  type="button"
                  onClick={() => void onAction(m.type)}
                  className="shrink-0 cursor-pointer rounded border border-border-default px-1.5 py-0.5 text-[11px] text-text-secondary hover:bg-bg-hover"
                >
                  {action}
                </button>
              )}
            </div>
          );
        })}
      </div>
      <StatusMenuSep />
      <StatusMenuRow icon={ExternalLink} text="Manage models…" onClick={() => onNavigate?.("models")} />
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
  // Only a hub serves: a follower keeps the service stopped (the role is the
  // switch), so the bar must not advertise a hub that this machine is not — it
  // used to read "Hub :35711" on a follower too, because the word followed the
  // listener rather than the role.
  const isHubRole = role === "hub";

  return (
    <StatusItem
      label={
        !isHubRole
          ? "This workspace is a follower: it syncs from its hub and serves nothing"
          : running
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
          <span>
            {isHubRole ? (running ? `Hub :${status?.port}` : "Hub off") : "Follower"}
          </span>
        </>
      }
    >
      <StatusMenuHeading>{isHubRole ? "This device is the hub" : "This device is a follower"}</StatusMenuHeading>
      <StatusMenuBlock>
        <p className="text-text-primary">
          {isHubRole
            ? running
              ? "Serving phones and linked devices"
              : "Sync service stopped"
            : "Pulls datasets from its hub and pushes edits back. Nothing is served from this machine."}
        </p>
        {running && (
          <div className="mt-0.5 flex flex-col gap-0.5 text-[11px] text-text-tertiary">
            {status?.local_url && <p className="select-all truncate">{status.local_url}</p>}
            {status?.lan_url && <p className="select-all truncate">{status.lan_url}</p>}
            <p>Serving: {flags.join(", ") || "nothing"}</p>
          </div>
        )}
      </StatusMenuBlock>
      <StatusMenuSep />
      {/* A follower is offered no start: promoting the workspace on the Devices &
          Hub page is what turns serving on. A service left running from before the
          demotion stays stoppable. */}
      {isHubRole ? (
        <StatusMenuRow
          icon={running ? Square : Play}
          text={running ? "Stop sync service" : "Start sync service"}
          disabled={busy}
          onClick={() => onToggle(!running)}
        />
      ) : running ? (
        <StatusMenuRow
          icon={Square}
          text="Stop sync service"
          disabled={busy}
          onClick={() => onToggle(false)}
        />
      ) : null}
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
        {models.length > 0 && (
          <ModelsItem models={models} onAction={runAction} onNavigate={onNavigate} />
        )}
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
