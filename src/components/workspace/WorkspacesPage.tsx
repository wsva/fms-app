"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openPath } from "@tauri-apps/plugin-opener";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { Button, Input, TextField, Label } from "@heroui/react";
import {
  Plus,
  Trash2,
  Edit2,
  Check,
  X,
  FolderOpen,
  FolderSearch,
  MonitorCheck,
} from "lucide-react";
import ConfirmDialog, { type ConfirmRequest } from "@/components/books/ConfirmDialog";
import { PageHeader, Toolbar, ToolbarButton } from "@/components/ui/toolbar";
import { usePageRefresh } from "@/lib/refresh/page-refresh";

// ── Types ───────────────────────────────────────────────────────────────────

interface Workspace {
  uuid: string;
  name: string;
  user_id: string;
  avatar: string;
  created_at: string;
  last_accessed: string;
  auto_login: boolean;
}

interface AuthUser {
  name: string;
  email: string;
}

/** How a workspace's user_id relates to the current user. */
type Ownership = "same" | "other" | "unclaimed";

function isUnclaimed(userId: string): boolean {
  return !userId || userId === "local";
}

function ownershipOf(ws: Workspace, currentUser: string): Ownership {
  if (isUnclaimed(ws.user_id)) return "unclaimed";
  if (currentUser && ws.user_id === currentUser) return "same";
  if (currentUser) return "other";
  // No known current user: cannot compare, treat as neutral.
  return "unclaimed";
}

/** Card + badge styles per ownership state. */
const ownershipStyles: Record<
  Ownership,
  { card: string; badge: string; badgeLabel: string }
> = {
  same: {
    card: "border-success-text/50 bg-success-bg",
    badge: "bg-success-text/20 text-success-text",
    badgeLabel: "Same user",
  },
  other: {
    card: "border-error-text/50 bg-error-bg",
    badge: "bg-error-text/20 text-error-text",
    badgeLabel: "Other user",
  },
  unclaimed: {
    card: "border-border-light bg-bg-card",
    badge: "bg-mid-gray/20 text-text-tertiary",
    badgeLabel: "Unclaimed",
  },
};

function formatRelativeTime(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return "Just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function formatDate(dateStr: string): string {
  const d = new Date(dateStr);
  return Number.isNaN(d.getTime()) ? "—" : d.toLocaleDateString();
}

/** A labelled key/value line for the detailed mobile workspace card — the
 *  owner user_id (an email once claimed) and the timestamps get their own
 *  labeled row rather than being crammed into a single truncated caption. */
function LabeledField({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-2 min-w-0">
      <span className="shrink-0 w-24 text-[11px] uppercase tracking-wide text-text-tertiary">
        {label}
      </span>
      <span
        className={`min-w-0 text-xs text-text-secondary truncate ${mono ? "font-mono" : ""}`}
        title={value}
      >
        {value}
      </span>
    </div>
  );
}

// ── Component ───────────────────────────────────────────────────────────────

export default function WorkspacesPage() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [currentUuid, setCurrentUuid] = useState<string | null>(null);
  const [currentUser, setCurrentUser] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [mobile, setMobile] = useState(false);

  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [newName, setNewName] = useState("");
  const [editingUuid, setEditingUuid] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);

  const fetchAll = useCallback(async () => {
    if (!isTauri()) return;
    setRefreshing(true);
    try {
      setError(null);
      const [list, current] = await Promise.all([
        invoke<Workspace[]>("workspace_list"),
        invoke<Workspace | null>("workspace_get_current"),
      ]);
      setWorkspaces(list);
      setCurrentUuid(current?.uuid ?? null);

      // Reference user for coloring: current workspace owner, falling back
      // to the logged-in user's email.
      let user = current && !isUnclaimed(current.user_id) ? current.user_id : "";
      if (!user) {
        try {
          const authUser = await invoke<AuthUser | null>("auth_get_user");
          user = authUser?.email ?? "";
        } catch {
          /* not logged in */
        }
      }
      setCurrentUser(user);
    } catch (e) {
      setError(`Failed to load workspaces: ${e}`);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    fetchAll();
  }, [fetchAll]);

  // Narrow-screen variant: compact icon-only actions live next to the title.
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Refresh when the workspace changes (switch from this page or elsewhere),
  // and whenever this page comes back on screen — the shared hook covers tab
  // activation and app resume, so there is no toolbar button for it any more.
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen("workspace-selected", () => {
      fetchAll();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchAll]);

  usePageRefresh("workspaces", {
    refresh: fetchAll,
    canRefresh: () => !refreshing,
  });

  async function handleSwitch(ws: Workspace) {
    if (!isTauri() || ws.uuid === currentUuid || refreshing) return;
    try {
      setError(null);
      await invoke<Workspace>("workspace_select", { uuid: ws.uuid });
      // The "workspace-selected" event triggers fetchAll.
    } catch (e) {
      setError(`Failed to switch workspace: ${e}`);
    }
  }

  async function handleCreate() {
    if (!isTauri() || !newName.trim()) return;
    try {
      setError(null);
      await invoke<Workspace>("workspace_create", { name: newName.trim() });
      setNewName("");
      setShowCreateDialog(false);
      await fetchAll();
    } catch (e) {
      setError(`Failed to create workspace: ${e}`);
    }
  }

  async function handleDelete(ws: Workspace) {
    if (!isTauri()) return;
    try {
      setError(null);
      await invoke("workspace_delete", { uuid: ws.uuid });
      setWorkspaces((prev) => prev.filter((w) => w.uuid !== ws.uuid));
    } catch (e) {
      setConfirmReq({
        title: "Cannot delete workspace",
        message: String(e),
        confirmLabel: "OK",
        onConfirm: () => {},
      });
    }
  }

  async function handleRename(uuid: string) {
    if (!isTauri() || !editName.trim()) return;
    try {
      setError(null);
      await invoke("workspace_rename", { uuid, name: editName.trim() });
      setWorkspaces((prev) =>
        prev.map((w) => (w.uuid === uuid ? { ...w, name: editName.trim() } : w))
      );
      setEditingUuid(null);
      setEditName("");
    } catch (e) {
      setError(`Failed to rename workspace: ${e}`);
    }
  }

  async function handleToggleAutoLogin(ws: Workspace) {
    if (!isTauri()) return;
    try {
      setError(null);
      await invoke("workspace_set_auto_login", {
        uuid: ws.uuid,
        autoLogin: !ws.auto_login,
      });
      setWorkspaces((prev) =>
        prev.map((w) =>
          w.uuid === ws.uuid ? { ...w, auto_login: !w.auto_login } : w
        )
      );
    } catch (e) {
      setError(`Failed to update startup setting: ${e}`);
    }
  }

  // Open a workspace's folder in the OS file manager. Desktop only: on Android
  // that directory is app-private, so there is nothing for a file browser to
  // show. `openPath` (not `revealItemInDir`) is what lands the browser *inside*
  // the folder instead of sitting on its parent with the folder selected. The
  // path comes from the backend — the frontend never builds it from the uuid,
  // so the layout stays defined in one place (workspace.rs).
  async function handleOpenFolder(ws: Workspace) {
    if (!isTauri()) return;
    try {
      setError(null);
      const dir = await invoke<string>("workspace_get_dir", { uuid: ws.uuid });
      await openPath(dir);
    } catch (e) {
      setError(`Failed to open the workspace folder: ${e}`);
    }
  }

  const description = (
    <p className="text-sm text-text-secondary mt-1">
      Each workspace has its own datasets, wiki, books and user account.
      Colors show how a workspace&apos;s owner relates to the current user
      {currentUser ? ` (${currentUser})` : ""}.
    </p>
  );

  const createButton = (
    <ToolbarButton
      icon={Plus}
      label="New Workspace"
      title="Create a new workspace"
      onClick={() => setShowCreateDialog(true)}
    />
  );

  return (
    <main className="flex-1 flex flex-col h-full min-w-0 min-h-0">
      <PageHeader title="Workspaces" />
      <Toolbar>
        <div className="flex-1 min-w-0" />
        {createButton}
      </Toolbar>

      <div className="flex-1 overflow-y-auto overflow-x-hidden p-8 min-w-0 min-h-0">
      <section className="mb-8">
        {description}

        {/* Legend */}
        <div className="flex items-center gap-3 mb-4 text-xs text-text-secondary">
          <span className={`px-2 py-0.5 rounded-full ${ownershipStyles.same.badge}`}>
            Same user
          </span>
          <span className={`px-2 py-0.5 rounded-full ${ownershipStyles.other.badge}`}>
            Other user
          </span>
          <span className={`px-2 py-0.5 rounded-full ${ownershipStyles.unclaimed.badge}`}>
            Unclaimed / default
          </span>
        </div>

        {/* Error banner */}
        {error && (
          <div className="mb-4 px-4 py-2 rounded-lg border border-error-text/50 bg-error-bg text-error-text text-sm">
            {error}
          </div>
        )}

        {/* Workspace list */}
        {loading ? (
          <div className="text-center py-8 text-text-secondary">Loading...</div>
        ) : workspaces.length === 0 ? (
          <div className="text-center py-8 text-text-secondary">No workspaces found</div>
        ) : (
          <div className="flex flex-col gap-3">
            {workspaces.map((ws) => {
              const ownership = ownershipOf(ws, currentUser);
              const styles = ownershipStyles[ownership];
              const isCurrent = ws.uuid === currentUuid;
              const owner = isUnclaimed(ws.user_id) ? "Not claimed" : ws.user_id;
              const isEditing = editingUuid === ws.uuid;

              // Only a handful of workspaces ever exist, so the mobile card is
              // given more height and detail: larger avatar, labeled owner and
              // timestamps, and labeled text buttons across the full width.
              if (mobile) {
                return (
                  <div
                    key={ws.uuid}
                    className={`flex flex-col gap-3 p-4 rounded-xl border transition-colors ${styles.card} ${
                      isCurrent ? "ring-2 ring-accent" : ""
                    }`}
                  >
                    {/* Identity row: avatar + name/badges */}
                    <div className="flex items-center gap-3 min-w-0">
                      <div className="w-12 h-12 rounded-full bg-accent-bg text-white flex items-center justify-center text-xl font-bold shrink-0">
                        {ws.avatar || ws.name.charAt(0).toUpperCase()}
                      </div>
                      <div className="flex-1 min-w-0">
                        {isEditing ? (
                          <div className="flex items-center gap-1">
                            <input
                              className="flex-1 min-w-0 px-2 py-1 text-sm border border-border-light rounded bg-bg-input text-text-primary"
                              value={editName}
                              onChange={(e) => setEditName(e.target.value)}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") handleRename(ws.uuid);
                                if (e.key === "Escape") setEditingUuid(null);
                              }}
                              autoFocus
                            />
                            <Button isIconOnly size="sm" variant="ghost" onPress={() => handleRename(ws.uuid)}>
                              <Check size={16} />
                            </Button>
                            <Button isIconOnly size="sm" variant="ghost" onPress={() => setEditingUuid(null)}>
                              <X size={16} />
                            </Button>
                          </div>
                        ) : (
                          <div className="flex items-center gap-2 flex-wrap">
                            <span className="font-medium text-text-primary truncate">{ws.name}</span>
                            {isCurrent && (
                              <span className="px-2 py-0.5 rounded-full bg-accent-bg text-white text-[11px] font-medium">
                                Current
                              </span>
                            )}
                            <span className={`px-2 py-0.5 rounded-full text-[11px] font-medium ${styles.badge}`}>
                              {styles.badgeLabel}
                            </span>
                          </div>
                        )}
                      </div>
                    </div>

                    {/* Detail rows */}
                    <div className="flex flex-col gap-1.5">
                      <LabeledField label="Owner" value={owner} />
                      <LabeledField label="Last used" value={formatRelativeTime(ws.last_accessed)} />
                      <LabeledField label="Created" value={formatDate(ws.created_at)} />
                      <LabeledField label="Workspace ID" value={ws.uuid} mono />
                    </div>

                    {/* Actions: labeled text buttons across the card width. */}
                    <div className="flex flex-wrap items-center gap-2 pt-2 border-t border-border-light">
                      {!isCurrent && !isEditing && (
                        <Button size="sm" variant="ghost" isDisabled={refreshing} onPress={() => handleSwitch(ws)}>
                          <FolderOpen size={14} /> Load
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        onPress={() => handleToggleAutoLogin(ws)}
                        className={ws.auto_login ? "text-accent" : "text-text-tertiary"}
                      >
                        <MonitorCheck size={14} /> Open at startup: {ws.auto_login ? "On" : "Off"}
                      </Button>
                      {!isEditing && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onPress={() => {
                            setEditingUuid(ws.uuid);
                            setEditName(ws.name);
                          }}
                        >
                          <Edit2 size={14} /> Rename
                        </Button>
                      )}
                      <Button
                        size="sm"
                        variant="ghost"
                        isDisabled={isCurrent}
                        onPress={() =>
                          setConfirmReq({
                            title: "Delete workspace",
                            message: `Delete workspace "${ws.name}"? This will move all data to trash.`,
                            confirmLabel: "Delete",
                            onConfirm: () => handleDelete(ws),
                          })
                        }
                      >
                        <Trash2 size={14} className="text-error-text" /> Delete
                      </Button>
                    </div>
                  </div>
                );
              }

              return (
                <div
                  key={ws.uuid}
                  className={`flex items-center gap-4 p-4 rounded-xl border transition-colors ${styles.card} ${
                    isCurrent ? "ring-2 ring-accent" : ""
                  }`}
                >
                  {/* Avatar */}
                  <div className="w-11 h-11 rounded-full bg-accent-bg text-white flex items-center justify-center text-lg font-bold shrink-0">
                    {ws.avatar || ws.name.charAt(0).toUpperCase()}
                  </div>

                  {/* Info */}
                  <div className="flex-1 min-w-0">
                    {editingUuid === ws.uuid ? (
                      <div className="flex items-center gap-1">
                        <input
                          className="flex-1 px-2 py-1 text-sm border border-border-light rounded bg-bg-input text-text-primary"
                          value={editName}
                          onChange={(e) => setEditName(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") handleRename(ws.uuid);
                            if (e.key === "Escape") setEditingUuid(null);
                          }}
                          autoFocus
                        />
                        <Button
                          size="sm"
                          variant="ghost"
                          onPress={() => handleRename(ws.uuid)}
                        >
                          <Check size={14} /> Save
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onPress={() => setEditingUuid(null)}
                        >
                          <X size={14} /> Cancel
                        </Button>
                      </div>
                    ) : (
                      <>
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="font-medium text-text-primary truncate">
                            {ws.name}
                          </span>
                          {isCurrent && (
                            <span className="px-1.5 py-0.5 rounded-full bg-accent-bg text-white text-[10px] font-medium">
                              Current
                            </span>
                          )}
                          <span
                            className={`px-1.5 py-0.5 rounded-full text-[10px] font-medium ${styles.badge}`}
                          >
                            {styles.badgeLabel}
                          </span>
                          {ws.auto_login && (
                            <span
                              className="flex items-center gap-0.5 px-1.5 py-0.5 rounded-full bg-mid-gray/20 text-text-tertiary text-[10px]"
                              title="Opens at startup"
                            >
                              <MonitorCheck size={10} /> Startup
                            </span>
                          )}
                        </div>
                        <div className="text-xs text-text-tertiary truncate mt-0.5">
                          {isUnclaimed(ws.user_id) ? "Not claimed" : ws.user_id}
                          {" · last used "}
                          {formatRelativeTime(ws.last_accessed)}
                        </div>
                      </>
                    )}
                  </div>

                  {/* Actions — labeled text buttons on desktop for the same
                      actions the phone card spells out. */}
                  <div className="flex items-center gap-1 shrink-0">
                    {!isCurrent && editingUuid !== ws.uuid && (
                      <Button
                        size="sm"
                        variant="ghost"
                        isDisabled={refreshing}
                        onPress={() => handleSwitch(ws)}
                      >
                        <FolderOpen size={14} /> Load
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      onPress={() => handleToggleAutoLogin(ws)}
                      className={ws.auto_login ? "text-accent" : "text-text-tertiary"}
                    >
                      <MonitorCheck size={14} /> Startup: {ws.auto_login ? "On" : "Off"}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onPress={() => handleOpenFolder(ws)}
                    >
                      <FolderSearch size={14} /> Open folder
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onPress={() => {
                        setEditingUuid(ws.uuid);
                        setEditName(ws.name);
                      }}
                    >
                      <Edit2 size={14} /> Rename
                    </Button>
                    <span title={isCurrent ? "Cannot delete the active workspace" : "Delete workspace"}>
                      <Button
                        size="sm"
                        variant="ghost"
                        isDisabled={isCurrent}
                        onPress={() =>
                          setConfirmReq({
                            title: "Delete workspace",
                            message: `Delete workspace "${ws.name}"? This will move all data to trash.`,
                            confirmLabel: "Delete",
                            onConfirm: () => handleDelete(ws),
                          })
                        }
                      >
                        <Trash2 size={14} className="text-error-text" /> Delete
                      </Button>
                    </span>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>

      {/* Create dialog */}
      {showCreateDialog && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50">
          <div className="bg-bg-card border border-border-default rounded-xl shadow-xl w-full max-w-sm mx-4 p-6">
            <h3 className="text-lg font-bold text-text-primary mb-4">
              Create New Workspace
            </h3>
            <TextField className="w-full">
              <Label>Workspace Name</Label>
              <Input
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleCreate();
                  if (e.key === "Escape") setShowCreateDialog(false);
                }}
                autoFocus
              />
            </TextField>
            <div className="flex justify-end gap-2 mt-4">
              <Button variant="ghost" size="sm" onPress={() => setShowCreateDialog(false)}>
                Cancel
              </Button>
              <Button
                variant="primary"
                size="sm"
                isDisabled={!newName.trim()}
                onPress={handleCreate}
              >
                Create
              </Button>
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
      </div>
    </main>
  );
}
