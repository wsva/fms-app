"use client";

import { useEffect, useState, type ReactNode } from "react";
import {
  Play,
  Square,
  Mic,
  Scissors,
  Cpu,
  MousePointerClick,
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
import { tapCompletes } from "@/lib/agent/tasks";
import AgentBadge from "@/components/agent/AgentBadge";
import { reqBool, useAgentActions } from "@/lib/agent/ui-actions";
import {
  getContextMenuEnabled,
  setContextMenuEnabled,
  useContextMenuEnabled,
} from "@/lib/ui/context-menu";
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

/** The bar's state language: the leading icon's colour, not a dot beside it.
 *  Green = in memory, blue pulsing = in flight, red = failed, muted = present
 *  on disk but idle (or nothing there at all — the menu's word tells those two
 *  apart, and only one of them has a verb).
 *
 *  A tinted glyph says the same thing a dot did while costing no width, which
 *  matters in a bar that is read at a glance and never has room to spare. */
function stateIconClass(state: LoadedModelState): string {
  switch (state) {
    case "loaded":
      return "text-green-500";
    case "loading":
      return "text-blue-500 animate-pulse";
    case "error":
      return "text-red-500";
    default:
      return "text-text-tertiary";
  }
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

/** The bar says whether anything is in memory (the icon's colour); the menu says
 *  which model each family is on, one row per family. Neither repeats the other. */
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

  // One colour for the whole group, worst state first: a failure must not be
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
  // binaries on Android), so the chip answers "is anything in memory?" with its
  // icon's colour rather than a count: how many, and which, is what opening it
  // is for.
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
      iconClassName={stateIconClass(state)}
      content={<span>Models</span>}
    >
      {/* One row per family, and each fact on it exactly once: the name says
          what it is, one muted line says where it stands (word + the model in
          use), and the one verb it offers sits at the end of the row. The state
          used to be written three times and the action floated between families
          under a repeated name, which read as the next family's subtitle; the
          family icon now carries the state as colour, the same way the bar's own
          icon does.

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
              <FamilyIcon size={14} className={`mt-[3px] shrink-0 ${stateIconClass(m.state)}`} />
              <div className="min-w-0 flex-1">
                <p className="truncate text-text-primary">{m.fullName}</p>
                <p className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px]">
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
  const count = tasks.length;
  // The visible text is the label itself (`content` defaults to it), with the
  // count written into it rather than into a pill beside it: the pill stated
  // the same number a second time. Tinted only while something is actually
  // owed, because the chip is always in the bar now — the tint is what
  // separates "0" from "nothing to see here".
  return (
    <StatusItem
      label={`Tasks - ${count}`}
      title={
        count === 0
          ? "No agent tasks waiting on you"
          : `${count} agent task${count === 1 ? "" : "s"} waiting on you`
      }
      align="start"
      active={count > 0}
      icon={ListTodo}
    >
      <StatusMenuHeading>Waiting on you</StatusMenuHeading>
      {shown.length === 0 && (
        <StatusMenuBlock>Nothing waiting on you.</StatusMenuBlock>
      )}
      {shown.map((t) => {
        // A capture is opened, not cleared: it names a card that has not been
        // written yet, so the button says so and saving that card is what
        // finishes it (`tapCompletes`).
        const opensOnly = !tapCompletes(t.target);
        return (
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
                  title={opensOnly ? "Jump to it; it closes when you save" : "Jump to it and mark it done"}
                >
                  {opensOnly ? <ExternalLink size={12} /> : <Check size={12} />}
                  {opensOnly ? "Open" : "Done"}
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
        );
      })}
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
  // Same language as the Models chip: the icon's colour is the state, so the
  // word beside it can stay a name ("Hub", "Follower") instead of repeating
  // "on"/"off" or spending width on the port — that lives in the menu, next to
  // the URLs where the number is actually useful.
  const iconClass = running ? "text-green-500" : "text-text-tertiary";

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
      iconClassName={iconClass}
      content={
        <span>{isHubRole ? (running ? "Hub" : "Hub off") : "Follower"}</span>
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
// The text-selection context menu
// ---------------------------------------------------------------------------

/**
 * A switch rather than a status: the panel that appears when text is selected
 * anywhere in the app (`CardContextMenu`) is off until the user asks for it, so
 * this item says what it will do, not what the app is doing — which is why it
 * sits in the right group with the mailbox, not in the left one with the state
 * items. The reason the default is off (and the Android exception) lives in
 * `lib/ui/context-menu`.
 *
 * The state is carried by the icon's colour — green while on, muted while off —
 * rather than by a word after the label: a status bar is read at a glance, and
 * the word cost more width than the fact was worth. The word is not lost, it
 * moved into the tooltip and the accessible name, and `active` still sets
 * `aria-pressed` — colour alone would be invisible to a screen reader.
 */
function ContextMenuItem() {
  const enabled = useContextMenuEnabled();
  return (
    <StatusItem
      label={`Context menu — ${enabled ? "on" : "off"}`}
      title={
        enabled
          ? "The context menu is on: selecting text offers a menu. Click to stop offering it."
          : "The context menu is off: selecting text does nothing. Click to offer a menu — add a card, or ask the model about it."
      }
      align="end"
      active={enabled}
      icon={MousePointerClick}
      iconClassName={enabled ? "text-green-500" : "text-text-tertiary"}
      onClick={() => setContextMenuEnabled(!enabled)}
      content={<span>Context menu</span>}
    />
  );
}

// ---------------------------------------------------------------------------
// The bar
// ---------------------------------------------------------------------------

export default function StatusBar({ onNavigate }: { onNavigate?: StatusBarNavigate }) {
  const { models, runAction } = useLoadedModels();
  const { open: tasks, busy: taskBusy, error: taskError, complete, dismiss } = useAgentTasks(true);
  const { status, role, busy: syncBusy, error: syncError, setRunning } = useSyncStatus(true);

  // The context-menu switch is shell chrome, so an agent reaches it under the
  // shell's tab like the refresh action `home/page.tsx` registers. The parameter
  // is optional: with no `on`, the action flips the switch, which is what the
  // button does.
  useAgentActions("shell", [
    {
      id: "shell.toggle-context-menu",
      label: "Turn the text-selection context menu on or off (it ships off)",
      kind: "mutate",
      params_schema: {
        on: { type: "boolean", description: "true turns it on, false turns it off; omit to flip it" },
      },
      run: (params) => {
        const on =
          params.on === undefined
            ? !getContextMenuEnabled()
            : reqBool("shell.toggle-context-menu", params, "on");
        setContextMenuEnabled(on);
        return { context_menu: on ? "on" : "off" };
      },
    },
  ]);

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
        {/* Always in the bar, count and all: the chip is how the bar says
            whether anything is owed, and a chip that vanished at zero left the
            learner unable to tell "no tasks" from "no such surface". */}
        <TaskItem
          tasks={tasks}
          busy={taskBusy}
          error={taskError}
          onComplete={complete}
          onDismiss={dismiss}
          onNavigate={onNavigate}
        />
        <SyncItem
          status={status}
          role={role}
          busy={syncBusy}
          error={syncError}
          onToggle={setRunning}
          onNavigate={onNavigate}
        />
      </div>

      {/* Right group — the switches and the agent dock's permanent entry point.
          The mailbox is always visible in the bar, unlike the opt-in floating
          button: it toggles over the `agent-dock-toggle` seam and wears the
          same attention marker the dock would show, so it is a bare button
          rather than a menu. */}
      <div className="ml-auto flex shrink-0 items-stretch pl-1.5">
        <ContextMenuItem />
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
