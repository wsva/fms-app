"use client";

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button, Input, TextField, Label } from "@heroui/react";
import { FolderOpen, Plus, Trash2, Edit2, Check, X } from "lucide-react";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface Workspace {
  uuid: string;
  name: string;
  user_id: string;
  avatar: string;
  created_at: string;
  last_accessed: string;
  auto_login: boolean;
}

export default function WorkspaceChooser({
  onSelect,
}: {
  onSelect: (workspace: Workspace) => void;
}) {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [loading, setLoading] = useState(true);
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  const [newName, setNewName] = useState("");
  const [editingUuid, setEditingUuid] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);

  const fetchWorkspaces = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const list = await invoke<Workspace[]>("workspace_list");
      setWorkspaces(list);
    } catch (e) {
      console.error("Failed to load workspaces:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchWorkspaces();
  }, [fetchWorkspaces]);

  async function handleSelect(ws: Workspace) {
    if (!isTauri()) return;
    try {
      const selected = await invoke<Workspace>("workspace_select", { uuid: ws.uuid });
      onSelect(selected);
    } catch (e) {
      console.error("Failed to select workspace:", e);
    }
  }

  async function handleCreate() {
    if (!isTauri() || !newName.trim()) return;
    try {
      const ws = await invoke<Workspace>("workspace_create", { name: newName.trim() });
      setWorkspaces((prev) => [...prev, ws]);
      setNewName("");
      setShowCreateDialog(false);
      // Auto-select the newly created workspace
      handleSelect(ws);
    } catch (e) {
      console.error("Failed to create workspace:", e);
    }
  }

  async function handleDelete(uuid: string) {
    if (!isTauri()) return;
    try {
      await invoke("workspace_delete", { uuid });
      setWorkspaces((prev) => prev.filter((w) => w.uuid !== uuid));
    } catch (e) {
      console.error("Failed to delete workspace:", e);
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
      await invoke("workspace_rename", { uuid, name: editName.trim() });
      setWorkspaces((prev) =>
        prev.map((w) => (w.uuid === uuid ? { ...w, name: editName.trim() } : w))
      );
      setEditingUuid(null);
      setEditName("");
    } catch (e) {
      console.error("Failed to rename workspace:", e);
    }
  }

  async function handleToggleAutoLogin(ws: Workspace) {
    if (!isTauri()) return;
    try {
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
      console.error("Failed to toggle auto-login:", e);
    }
  }

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

  return (
    <>
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
        <div className="bg-bg-card border border-border-default rounded-2xl shadow-2xl w-full max-w-md mx-4 overflow-hidden">
          {/* Header */}
          <div className="px-6 pt-6 pb-4">
            <h2 className="text-xl font-bold text-text-primary">Select Workspace</h2>
            <p className="text-sm text-text-secondary mt-1">
              Choose a workspace to continue
            </p>
          </div>

          {/* Workspace list */}
          <div className="px-4 pb-4 max-h-80 overflow-y-auto">
            {loading ? (
              <div className="text-center py-8 text-text-secondary">Loading...</div>
            ) : workspaces.length === 0 ? (
              <div className="text-center py-8 text-text-secondary">
                No workspaces found
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                {workspaces.map((ws) => (
                  <div
                    key={ws.uuid}
                    className="group flex items-center gap-3 p-3 rounded-xl border border-border-light hover:border-accent hover:bg-bg-hover transition-colors cursor-pointer"
                    onClick={() => handleSelect(ws)}
                  >
                    {/* Avatar */}
                    <div className="w-10 h-10 rounded-full bg-accent-bg text-white flex items-center justify-center text-lg font-bold shrink-0">
                      {ws.avatar || ws.name.charAt(0).toUpperCase()}
                    </div>

                    {/* Info */}
                    <div className="flex-1 min-w-0">
                      {editingUuid === ws.uuid ? (
                        <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
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
                          <div className="font-medium text-text-primary truncate">
                            {ws.name}
                          </div>
                          <div className="text-xs text-text-tertiary truncate">
                            {ws.user_id || "Not claimed"}
                            {ws.user_id && " · "}
                            {formatRelativeTime(ws.last_accessed)}
                          </div>
                        </>
                      )}
                    </div>

                    {/* Actions */}
                    <div
                      className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity"
                      onClick={(e) => e.stopPropagation()}
                    >
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
                      <Button
                        isIconOnly
                        size="sm"
                        variant="ghost"
                        onPress={() =>
                          setConfirmReq({
                            title: "Delete workspace",
                            message: `Delete workspace "${ws.name}"? This will move all data to trash.`,
                            confirmLabel: "Delete",
                            onConfirm: () => handleDelete(ws.uuid),
                          })
                        }
                      >
                        <Trash2 size={14} className="text-error-text" />
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Footer */}
          <div className="px-6 py-4 border-t border-border-light flex items-center justify-between">
            <Button
              variant="ghost"
              size="sm"
              onPress={() => setShowCreateDialog(true)}
            >
              <Plus size={16} /> New Workspace
            </Button>
            <span className="text-xs text-text-tertiary">
              {workspaces.length} workspace{workspaces.length !== 1 ? "s" : ""}
            </span>
          </div>
        </div>
      </div>

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
    </>
  );
}
