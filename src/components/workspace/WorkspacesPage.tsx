"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { Button, Input, TextField, Label } from "@heroui/react";
import {
  Plus,
  Trash2,
  Edit2,
  Check,
  X,
  RefreshCw,
  FolderOpen,
  MonitorCheck,
} from "lucide-react";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

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

  // Refresh when the workspace changes (switch from this page or elsewhere).
  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = listen("workspace-selected", () => {
      fetchAll();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchAll]);

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
      setError(`Failed to toggle auto-login: ${e}`);
    }
  }

  const description = (
    <p className="text-sm text-text-secondary mt-1">
      Each workspace has its own datasets, wiki, books and user account.
      Colors show how a workspace&apos;s owner relates to the current user
      {currentUser ? ` (${currentUser})` : ""}.
    </p>
  );

  const refreshButton = (
    <span title="Refresh workspace list">
      <Button
        isIconOnly
        variant="ghost"
        size="sm"
        isDisabled={refreshing}
        onPress={fetchAll}
      >
        <RefreshCw size={16} className={refreshing ? "animate-spin" : undefined} />
      </Button>
    </span>
  );

  const createButton = (
    <Button variant="primary" size="sm" onPress={() => setShowCreateDialog(true)}>
      <Plus size={16} /> New Workspace
    </Button>
  );

  const createIconButton = (
    <span title="New workspace">
      <Button
        isIconOnly
        variant="ghost"
        size="sm"
        onPress={() => setShowCreateDialog(true)}
      >
        <Plus size={16} />
      </Button>
    </span>
  );

  return (
    <main className="flex-1 p-8 overflow-y-auto">
      <section className="mb-8">
        {/* Header */}
        {mobile ? (
          <div className="mb-4">
            <div className="flex items-center gap-2">
              <h1 className="text-2xl font-bold text-text-primary">Workspaces</h1>
              <div className="flex items-center gap-1">
                {refreshButton}
                {createIconButton}
              </div>
            </div>
            {description}
          </div>
        ) : (
          <div className="flex items-center justify-between mb-4">
            <div>
              <h1 className="text-2xl font-bold text-text-primary">Workspaces</h1>
              {description}
            </div>
            <div className="flex items-center gap-2">
              {refreshButton}
              {createButton}
            </div>
          </div>
        )}

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
                          isIconOnly
                          size="sm"
                          variant="ghost"
                          onPress={() => handleRename(ws.uuid)}
                        >
                          <Check size={14} />
                        </Button>
                        <Button
                          isIconOnly
                          size="sm"
                          variant="ghost"
                          onPress={() => setEditingUuid(null)}
                        >
                          <X size={14} />
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
                              title="Auto-selected at app startup"
                            >
                              <MonitorCheck size={10} /> Auto
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

                  {/* Actions */}
                  <div className="flex items-center gap-1 shrink-0">
                    {!isCurrent && editingUuid !== ws.uuid && (
                      <span title="Load this workspace (switch to it)">
                        <Button
                          size="sm"
                          variant="ghost"
                          isDisabled={refreshing}
                          onPress={() => handleSwitch(ws)}
                        >
                          <FolderOpen size={14} /> Load
                        </Button>
                      </span>
                    )}
                    <span title={ws.auto_login ? "Disable auto-login" : "Enable auto-login"}>
                      <Button
                        isIconOnly
                        size="sm"
                        variant="ghost"
                        onPress={() => handleToggleAutoLogin(ws)}
                        className={ws.auto_login ? "text-accent" : "text-text-tertiary"}
                      >
                        <MonitorCheck size={14} />
                      </Button>
                    </span>
                    <span title="Rename">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="ghost"
                        onPress={() => {
                          setEditingUuid(ws.uuid);
                          setEditName(ws.name);
                        }}
                      >
                        <Edit2 size={14} />
                      </Button>
                    </span>
                    <span title={isCurrent ? "Cannot delete the active workspace" : "Delete"}>
                      <Button
                        isIconOnly
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
                        <Trash2 size={14} className="text-error-text" />
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
    </main>
  );
}
