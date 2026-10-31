"use client";

import { useState, useEffect, useCallback, useRef, useReducer } from "react";
import { listen, emit } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { X } from "lucide-react";
import { isMobileApp } from "@/lib/platform";
import Sidebar, { TAB_DEFS, type TabId } from "@/components/layout/Sidebar";
import TitleBar from "@/components/layout/TitleBar";
import StatusBar from "@/components/layout/StatusBar";
import BottomNav from "@/components/layout/BottomNav";
import PageHost from "@/components/layout/PageHost";
import WorkspaceChooser from "@/components/workspace/WorkspaceChooser";
import CardContextMenu from "@/components/cards/CardContextMenu";
import { useCardEditor } from "@/components/cards/CardEditorContext";
import AgentDock from "@/components/agent/AgentDock";
import type { AgentAction } from "@/lib/agent/types";
import type { Card } from "@/lib/types";
import { setUiContext } from "@/lib/agent/ui-context";
import { jsonSafeDetail, runUiAction, useAgentActions } from "@/lib/agent/ui-actions";
import { dismissToast, notify, notifyError, useToast } from "@/lib/feedback/toast";
import { applySubjectToUrl, useTabSubject } from "@/lib/shell/tab-subject";
import {
  refreshActivePage,
  useActiveTabSource,
  canRefreshInstance,
} from "@/lib/refresh/page-refresh";
import {
  initialTabStrip,
  tabStripReducer,
  planOpen,
  MAX_OPEN_TABS,
} from "@/lib/shell/tabs";

interface Workspace {
  uuid: string;
  name: string;
  user_id: string;
  avatar: string;
  created_at: string;
  last_accessed: string;
  auto_login: boolean;
}

// A pairing request emitted by the backend (pairing.rs) when an unknown device
// on an untrusted network asks to pair.
interface PairingRequest {
  request_id: string;
  device_id: string;
  name: string;
  fingerprint: string;
  // Identity the device declares; the PC binds writeback to it on approval.
  user_id?: string;
}

export default function Home() {
  // The tab strip's state (docs/design/tab-strip.md): a reducer over the open
  // page instances plus which one is focused. `activeTab` is derived from the
  // focused instance so every existing consumer — ui-context, the refresh
  // source, the sidebar, the title bar, the bottom nav, the agent dock — keeps
  // reading a plain `TabId` and none of them learn about instances.
  const [tabs, dispatchTabs] = useReducer(tabStripReducer, undefined, initialTabStrip);
  const focusedInst = tabs.order.find((i) => i.instanceId === tabs.focused) ?? tabs.order[0];
  const activeTab: TabId = focusedInst.tabId;
  // Latest committed state, read by `openTab` so it can stay a stable callback:
  // the agent listener and deep-link effects depend on it and must not
  // re-subscribe on every tab change.
  const tabsRef = useRef(tabs);
  // No sidebar width here: the rail is an in-flow flex item, so it reserves its
  // own space and the pages simply take what is left.
  const [showWorkspaceChooser, setShowWorkspaceChooser] = useState(false);
  const [currentWorkspace, setCurrentWorkspace] = useState<Workspace | null>(null);
  const [mobile, setMobile] = useState(false);
  // Android paints the prerendered desktop shell while the JS bundle is still
  // loading, which reads as "wrong app" for a second or more on a phone. The
  // splash is therefore rendered from the very first (prerendered) paint and
  // only dropped once the client has committed the platform-correct layout.
  // `booted` starts false on server and client alike, so hydration matches.
  const [booted, setBooted] = useState(false);
  // Ephemeral notification banner. It lives in the store rather than here so the
  // places that need it are not limited to the ones that render inside the shell:
  // a card editor failure happens in a root overlay above this component, and
  // `alert()` was the only way it used to be able to say anything.
  const toast = useToast();
  const { openCardEditor } = useCardEditor();

  useEffect(() => {
    setMobile(isMobileApp());
    setBooted(true);
  }, []);

  useEffect(() => {
    tabsRef.current = tabs;
  }, [tabs]);

  const titleFor = useCallback((tabId: TabId) => TAB_DEFS[tabId]?.label ?? tabId, []);

  // Focus-or-open: the single seam every navigation source uses (sidebar, menus,
  // status bar, bottom nav, agent, deep links). Policy is replace-by-default —
  // `planOpen` recycles the focused tab when it is unpinned and clean, otherwise
  // adds beside it. A dirty focused tab is auto-pinned first (its `canRefresh`
  // returns false), so unsaved edits are never unmounted (§4.4).
  const openTab = useCallback(
    (tabId: TabId) => {
      const state = tabsRef.current;
      const focusedDirty = !canRefreshInstance(state.focused);
      const placement = planOpen(state, tabId, focusedDirty);
      switch (placement.kind) {
        case "focus":
          dispatchTabs({ type: "focus", instanceId: placement.instanceId });
          return;
        case "replace":
          dispatchTabs({
            type: "replace",
            instanceId: placement.instanceId,
            tabId,
            title: titleFor(tabId),
          });
          return;
        case "add": {
          if (state.order.length >= MAX_OPEN_TABS) {
            notify("Too many tabs open — close one first.");
            return;
          }
          if (placement.autoPin) {
            dispatchTabs({ type: "togglePin", instanceId: placement.autoPin });
          }
          dispatchTabs({ type: "add", tabId, title: titleFor(tabId) });
          return;
        }
      }
    },
    [titleFor]
  );

  // Check workspace state on mount
  useEffect(() => {
    async function checkWorkspace() {
      try {
        const ws = await invoke<Workspace | null>("workspace_get_current");
        if (ws) {
          setCurrentWorkspace(ws);
        } else {
          // No workspace selected, show chooser
          setShowWorkspaceChooser(true);
        }
      } catch (e) {
        console.error("Failed to get current workspace:", e);
        setShowWorkspaceChooser(true);
      }
    }
    checkWorkspace();

    // Listen for workspace chooser event from backend
    const unlistenChooser = listen("workspace-show-chooser", () => {
      setShowWorkspaceChooser(true);
    });

    // Keep the shell in sync when the workspace is switched (e.g. from the
    // Workspaces page) so the current-workspace state stays fresh.
    const unlistenSelected = listen<Workspace>("workspace-selected", (event) => {
      setCurrentWorkspace(event.payload);
    });

    return () => {
      unlistenChooser.then((fn) => fn());
      unlistenSelected.then((fn) => fn());
    };
  }, []);

  // Listen for wiki deep link navigation (fms-app://wiki/path/to/file.md)
  useEffect(() => {
    const unlisten = listen<string>("wiki-navigate", () => {
      openTab("wiki");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [openTab]);

  // The shell's slice of the agent-visible UI context: which tab is showing.
  // The pages own the rest (Dictation reports the dataset/media/cue) and merge
  // into the same backend cache — see `lib/agent/ui-context`.
  useEffect(() => {
    setUiContext({ active_tab: activeTab });
  }, [activeTab]);

  // Agent app-control bridge: an attached agent drives the UI through the `app_*`
  // MCP tools, which emit `agent-action`. Perform the shell-level action here
  // (navigation + toast) and re-emit page-specific intents for the target page.
  useEffect(() => {
    const unlisten = listen<AgentAction>("agent-action", (event) => {
      const action = event.payload;
      switch (action.type) {
        case "navigate":
          // `agent` is a settings page, not a view an agent should drive, and
          // honouring it would yank the user off whatever they were reading.
          // Ignore it so a stale prompt cannot blank the content area.
          if ((action.tab as string) === "agent") break;
          openTab(action.tab as TabId);
          break;
        case "open-review":
          openTab("cards");
          void emit("agent-open-review", { dataset_uuid: action.dataset_uuid ?? null });
          break;
        case "start-dictation": {
          openTab("dictation");
          // Drive the page through the action registry instead of re-emitting an
          // event: `agent-start-dictation` had no listener, so the dataset and
          // media an agent named were silently dropped on the floor.
          const datasetUuid = action.dataset_uuid ?? null;
          const mediaUuid = action.media_uuid ?? null;
          void (async () => {
            try {
              if (datasetUuid) {
                await runUiAction("dictation.open-dataset", { dataset_uuid: datasetUuid });
              }
              if (mediaUuid) {
                await runUiAction("dictation.open-media", {
                  dataset_uuid: datasetUuid,
                  media_uuid: mediaUuid,
                });
              }
            } catch (e) {
              // Nothing to report back to: `app_start_dictation` is
              // fire-and-forget by design. The agent can confirm the effect
              // with app_get_ui_state, which is what its description says.
              console.error("agent start-dictation failed:", e);
            }
          })();
          break;
        }
        case "notify":
          notify(action.message);
          break;
        case "open-card": {
          // The card editor is a root overlay, so this needs no navigation: the
          // page that asked for the card stays mounted underneath, which is the
          // whole reason a capture can be filed without leaving the document.
          const datasetUuid = action.dataset_uuid;
          if (!datasetUuid) {
            notifyError("open-card needs a dataset_uuid.");
            break;
          }
          // Saving the card completes the todo that opened it. `todo_complete`
          // is idempotent, so an editor saved three times costs three no-ops.
          const todoUuid = action.todo_uuid ?? null;
          const onSave = todoUuid
            ? () => {
                void invoke("todo_complete", { uuid: todoUuid }).catch((e: unknown) => {
                  console.error("todo_complete after card save failed:", e);
                });
              }
            : undefined;
          void (async () => {
            try {
              let card: Card | null = null;
              if (action.card_uuid) {
                card = await invoke<Card>("card_get", {
                  datasetUuid,
                  cardUuid: action.card_uuid,
                });
              } else if (action.prefill) {
                const prefill = action.prefill;
                card = {
                  uuid: "",
                  question: prefill.question ?? "",
                  answer: prefill.answer ?? "",
                  suggestion: prefill.suggestion ?? "",
                  note: prefill.note ?? "",
                  familiarity: 0,
                  question_hash: null,
                  source_card_uuid: null,
                  source_dataset_uuid: null,
                  deleted_at: null,
                  created_at: "",
                  updated_at: "",
                };
              }
              // `false` opens every field: whoever replayed this came to write.
              openCardEditor(datasetUuid, card, onSave, undefined, action.dataset_name, false);
            } catch (e) {
              notifyError(`Could not open the card: ${e}`);
            }
          })();
          break;
        }
        case "invoke-action": {
          // The response half of `ui_invoke_action`. The tool is parked on
          // `correlation_id` waiting for this, so every path — success, thrown
          // error, IPC failure — has to report something or the agent waits out
          // the full timeout for no reason.
          const correlationId = action.correlation_id;
          void (async () => {
            let ok = false;
            let detail: unknown = null;
            try {
              detail = (await runUiAction(action.id, action.params ?? {})) ?? null;
              ok = true;
            } catch (e) {
              detail = e instanceof Error ? e.message : String(e);
            }
            try {
              await invoke("agent_report_action_result", {
                correlationId,
                ok,
                detail: jsonSafeDetail(detail),
              });
            } catch (e) {
              console.error("agent_report_action_result failed:", e);
            }
          })();
          break;
        }
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [openTab, openCardEditor]);

  // Pairing: ask the PC owner about each request, one at a time. This dialog is
  // the only way a device is ever approved — no bearer secret can replace the
  // click. Only the PC shows dialogs.
  useEffect(() => {
    if (isMobileApp()) return;
    const queue: PairingRequest[] = [];
    let running = false;
    const drain = async () => {
      if (running) return;
      running = true;
      while (queue.length > 0) {
        const req = queue.shift()!;
        const asWho = req.user_id && req.user_id !== "local"
          ? `\n\nIt will sync progress as: ${req.user_id}`
          : "";
        const approve = await ask(
          `FmS on ${req.name} wants to pair.${asWho}\n\nConfirm the code ${req.fingerprint} matches the one shown on the device, then allow or deny.`,
          { title: "Pairing request", kind: "warning", okLabel: "Allow", cancelLabel: "Deny" },
        );
        try {
          await invoke("pairing_respond", { requestId: req.request_id, approve });
        } catch (e) {
          console.error("pairing_respond failed:", e);
        }
      }
      running = false;
    };
    const unlisten = listen<PairingRequest>("pairing-request", (event) => {
      queue.push(event.payload);
      void drain();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Tell the refresh module which tab is on screen, so a page knows when it
  // becomes visible. See lib/refresh/page-refresh.tsx for why this is not a
  // provider: the pages are plain siblings, and one boolean is not worth
  // re-indenting the shell.
  useActiveTabSource(activeTab);

  // The address names what is on screen: the focused tab and, when the page (or
  // the card editor above it) publishes one, its subject — `?tab=cards&dataset=…
  // &card=…`. Nothing else reads it but the editor's own restore on boot and its
  // `popstate` listener, so the static export needs no routes for it.
  const subject = useTabSubject(activeTab);
  useEffect(() => {
    applySubjectToUrl(activeTab, subject);
  }, [activeTab, subject]);

  // F5 / Ctrl+R / Cmd+R re-read the page on screen. `preventDefault` is not
  // politeness: a WebView-level reload remounts every page and destroys state
  // the app deliberately keeps alive — including dictation answers, which are
  // never persisted. The page's own guard decides whether a re-read is safe.
  // The title bar's File menu calls this same function, so a click and a
  // keystroke cannot diverge on what "refresh" means or what it reports back.
  const refreshActivePageWithToast = useCallback(() => {
    void refreshActivePage("manual").then((outcome) => {
      if (outcome.status === "blocked") {
        notify("Not refreshed: this page has unsaved edits.");
      } else if (outcome.status === "none") {
        notify("Nothing to refresh on this page.");
      }
    });
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.repeat) return;
      const isRefreshKey =
        e.key === "F5" ||
        ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "r");
      if (!isRefreshKey) return;
      e.preventDefault();
      refreshActivePageWithToast();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [refreshActivePageWithToast]);

  // The same path the keyboard takes, for an agent: refresh is otherwise
  // UI-only, and every feature is supposed to be reachable through MCP.
  useAgentActions("shell", [
    {
      id: "shell.refresh",
      label: "Re-read the page on screen from disk",
      kind: "read",
      run: () => refreshActivePage("manual"),
    },
  ]);

  function handleWorkspaceSelect(ws: Workspace) {
    setCurrentWorkspace(ws);
    setShowWorkspaceChooser(false);
  }

  // The window's own bar. The desktop window is created with `decorations: false`,
  // so this row is the only way to minimize, maximize or move the window — which
  // is why it is drawn in *both* branches below: an undecorated window behind the
  // workspace chooser would otherwise be a frame the user cannot drag or leave.
  // Mobile keeps the system bar it was given.
  const titleBar = mobile ? null : (
    <TitleBar
      onNavigate={(tab) => openTab(tab as TabId)}
      activeTab={activeTab}
      onRefresh={refreshActivePageWithToast}
      onWorkspaceChooser={() => setShowWorkspaceChooser(true)}
      currentWorkspace={currentWorkspace?.uuid ?? null}
      tabs={tabs.order}
      focusedId={tabs.focused}
      onFocusTab={(instanceId) => dispatchTabs({ type: "focus", instanceId })}
      onCloseTab={(instanceId) => dispatchTabs({ type: "close", instanceId })}
      onTogglePin={(instanceId) => dispatchTabs({ type: "togglePin", instanceId })}
      onSpawnTab={(tab) =>
        dispatchTabs({ type: "spawn", tabId: tab, title: titleFor(tab) })
      }
    />
  );

  // Show workspace chooser if needed
  if (showWorkspaceChooser) {
    // The chooser is a `fixed inset-0` scrim of its own; the bar out-stacks it
    // (see the z order in TitleBar) instead of being dimmed under it, the way a
    // native title bar is not.
    return (
      <div className="flex flex-col h-full">
        {titleBar}
        <WorkspaceChooser onSelect={handleWorkspaceSelect} />
      </div>
    );
  }

  return (
    <div
      // `h-full` rather than `h-screen` — see the html/body rule in globals.css:
      // a viewport unit is multiplied by the global page zoom while a percentage
      // resolves against a viewport already divided by it, so 100vh overshoots
      // the window at any zoom above 1 and the window itself starts scrolling.
      // The shell is a column: the title bar on top, the sidebar + pages row
      // under it, the status bar under that. Both bars are siblings of the row
      // rather than children of the pages, so each spans the whole window width
      // — which is also why the sidebar can be an in-flow flex item instead of a
      // `fixed` panel: it stops at the row's edges, below the title bar and above
      // the status bar.
      className="flex flex-col h-full"
      // Edge-to-edge WebView draws under the status bar; push the shell below it.
      style={mobile ? { paddingTop: "env(safe-area-inset-top)" } : undefined}
    >
      {titleBar}

      <div className="flex flex-1 min-h-0">
        {!mobile && (
          <Sidebar activeTab={activeTab} onTabChange={openTab} />
        )}

        <div className="flex-1 flex flex-col overflow-hidden min-h-0">
          <PageHost order={tabs.order} focusedId={tabs.focused} onNavigate={openTab} />
        </div>
      </div>

      {/* A menu row's "…page…" lands on the same tabs the sidebar lists, so the
          strip navigates through the shell it lives in, not a route. */}
      {!mobile && <StatusBar onNavigate={(tab) => openTab(tab as TabId)} />}

      {mobile && <BottomNav activeTab={activeTab} onTabChange={openTab} />}

      {/* Global text-selection → "Add to Card" menu */}
      <CardContextMenu />

      {/* Always-accessible mailbox (desktop only) — overlays the content area
          on the right, toggled by the floating Bot button or Ctrl+Space. It
          renders the ledger's three lanes (you owe / your agent owes / inbox)
          over the same rows an attached agent claims from, so it holds no
          session: `onNavigate` is only how a row reaches what it is about.
          `booted` is required as well as `!mobile`: the platform is unknown on
          the prerendered first paint, so without it Android would mount the
          dock for one commit and its effects would call desktop-only commands. */}
      {booted && !mobile && <AgentDock onNavigate={(tab) => openTab(tab as TabId)} />}

      {/* The app's one notice banner, fed by `lib/feedback/toast`: the shell
          renders it, anyone can post to it — including the card editor, which is
          a root overlay above this component and used to have only `alert()`. */}
      {toast && (
        <div
          role="status"
          className={`fixed bottom-6 left-1/2 -translate-x-1/2 z-[9998] flex max-w-[80%] items-start gap-2 rounded-lg border px-4 py-2.5 text-sm shadow-lg break-words ${
            toast.tone === "error"
              ? "border-error-text/50 bg-error-bg text-error-text"
              : "border-border-default bg-bg-card text-text-primary"
          }`}
        >
          <span>{toast.message}</span>
          {toast.action && (
            <button
              type="button"
              onClick={() => {
                const run = toast.action?.run;
                dismissToast();
                run?.();
              }}
              className="shrink-0 rounded border border-border-default px-2 py-0.5 text-xs font-medium text-text-primary hover:bg-bg-hover"
            >
              {toast.action.label}
            </button>
          )}
          <button
            type="button"
            aria-label="Dismiss"
            onClick={dismissToast}
            className="shrink-0 text-text-secondary hover:text-text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      )}

      {/* Boot splash — visible on Android only, see .app-boot-splash in globals.css.
          No `flex` class: display is owned by that rule so the pre-paint
          data-mobile tag can decide it. /logo.png is a copy of
          src-tauri/icons/icon.png (the launcher artwork); it lives in public/ so
          the WebView can reach it, and is rounded to echo the adaptive icon. */}
      {!booted && (
        <div className="app-boot-splash fixed inset-0 z-[9999] flex-col items-center justify-center bg-bg-body">
          <img src="/logo.png" alt="" className="h-36 w-36 select-none rounded-2xl" draggable={false} />
        </div>
      )}
    </div>
  );
}

