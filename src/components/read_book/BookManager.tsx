"use client";

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ChevronDown,
  ChevronRight,
  Plus,
  Pencil,
  Trash2,
  Copy,
  Check,
  BookOpen,
} from "lucide-react";
import type { BookMeta, BookChapter } from "@/lib/read/types";
import { isTauri } from "@/lib/tauri";
import { getUUID, nowIso } from "./utils";
import ConfirmDialog, { type ConfirmRequest } from "./ConfirmDialog";

type ChapterNode = BookChapter & { children: ChapterNode[] };

function buildTree(flat: BookChapter[], parentUuid: string | null = null): ChapterNode[] {
  return flat
    .filter((c) => (c.parent_uuid ?? null) === parentUuid)
    .sort((a, b) => (a.order_num ?? 0) - (b.order_num ?? 0))
    .map((c) => ({ ...c, children: buildTree(flat, c.uuid) }));
}

const btnGhost =
  "px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover border border-border-default cursor-pointer disabled:opacity-50";
const btnPrimary =
  "px-3 py-1.5 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50";
const inputCls =
  "flex-1 px-3 py-1.5 rounded-md bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent";

type Props = {
  books: BookMeta[];
  onBooksChanged: () => void;
};

export default function BookManager({ books, onBooksChanged }: Props) {
  const [saving, setSaving] = useState(false);
  const [showAdd, setShowAdd] = useState(false);
  const [addTitle, setAddTitle] = useState("");
  const [editBookUUID, setEditBookUUID] = useState<string | null>(null);
  const [editBookTitle, setEditBookTitle] = useState("");

  const [bookUUID, setBookUUID] = useState("");
  const [flat, setFlat] = useState<BookChapter[]>([]);
  const [chaptersLoading, setChaptersLoading] = useState(false);
  const [editChapterUUID, setEditChapterUUID] = useState<string | null>(null);
  const [editChapterTitle, setEditChapterTitle] = useState("");
  const [addingUnder, setAddingUnder] = useState<string | null | undefined>(undefined);
  const [addChapterTitle, setAddChapterTitle] = useState("");
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);

  const tree = useMemo(() => buildTree(flat), [flat]);
  const selectedBook = books.find((b) => b.uuid === bookUUID);

  // Load chapters when a book is selected.
  useEffect(() => {
    if (!bookUUID || !isTauri()) return;
    const load = async () => {
      setChaptersLoading(true);
      try {
        const data = await invoke<BookChapter[]>("book_list_chapters", { bookUuid: bookUUID });
        setFlat(data);
      } catch {
        setFlat([]);
      }
      setChaptersLoading(false);
    };
    load();
  }, [bookUUID]);

  // ── Book handlers ──────────────────────────────────────────────
  const handleAddBook = async () => {
    if (!addTitle.trim() || !isTauri()) return;
    setSaving(true);
    try {
      await invoke<BookMeta>("book_create", { title: addTitle.trim() });
      setAddTitle("");
      setShowAdd(false);
      onBooksChanged();
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleRenameBook = async (item: BookMeta) => {
    if (!editBookTitle.trim() || !isTauri()) return;
    setSaving(true);
    try {
      await invoke("book_rename", { uuid: item.uuid, title: editBookTitle.trim() });
      setEditBookUUID(null);
      onBooksChanged();
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleDeleteBook = (uuid: string) => {
    setConfirmReq({
      message: "Delete this book and all its chapters?",
      confirmLabel: "Delete",
      onConfirm: () => doDeleteBook(uuid),
    });
  };

  const doDeleteBook = async (uuid: string) => {
    if (!isTauri()) return;
    setSaving(true);
    try {
      await invoke("book_delete", { uuid });
      if (bookUUID === uuid) {
        setBookUUID("");
        setFlat([]);
      }
      onBooksChanged();
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  // ── Chapter handlers ───────────────────────────────────────────
  const handleAddChapter = async (parentUuid: string | null) => {
    if (!addChapterTitle.trim() || !bookUUID || !isTauri()) return;
    const siblings = flat.filter((c) => (c.parent_uuid ?? null) === parentUuid);
    const now = nowIso();
    const chapter: BookChapter = {
      uuid: getUUID(),
      book_uuid: bookUUID,
      parent_uuid: parentUuid,
      order_num: siblings.length + 1,
      title: addChapterTitle.trim(),
      status: null,
      created_at: now,
      updated_at: now,
    };
    setSaving(true);
    try {
      await invoke("book_save_chapter", { bookUuid: bookUUID, chapter });
      setAddChapterTitle("");
      setAddingUnder(undefined);
      setFlat((prev) => [...prev, chapter]);
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleSaveEditChapter = async (item: BookChapter) => {
    if (!isTauri()) return;
    const updated: BookChapter = {
      ...item,
      title: editChapterTitle.trim() || item.title,
      updated_at: nowIso(),
    };
    setSaving(true);
    try {
      await invoke("book_save_chapter", { bookUuid: bookUUID, chapter: updated });
      setEditChapterUUID(null);
      setFlat((prev) => prev.map((c) => (c.uuid === item.uuid ? updated : c)));
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleToggleStatus = async (item: BookChapter) => {
    if (!isTauri()) return;
    const newStatus = item.status === "completed" ? null : "completed";
    const updated: BookChapter = { ...item, status: newStatus, updated_at: nowIso() };
    setSaving(true);
    try {
      await invoke("book_save_chapter", { bookUuid: bookUUID, chapter: updated });
      setFlat((prev) => prev.map((c) => (c.uuid === item.uuid ? updated : c)));
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleDeleteChapter = (item: BookChapter) => {
    const hasChildren = flat.some((c) => c.parent_uuid === item.uuid);
    if (hasChildren) {
      setConfirmReq({ message: "Remove child chapters first." });
      return;
    }
    setConfirmReq({
      message: `Delete "${item.title}"?`,
      confirmLabel: "Delete",
      onConfirm: () => doDeleteChapter(item),
    });
  };

  const doDeleteChapter = async (item: BookChapter) => {
    if (!isTauri()) return;
    setSaving(true);
    try {
      await invoke("book_delete_chapter", { bookUuid: bookUUID, uuid: item.uuid });
      setFlat((prev) => prev.filter((c) => c.uuid !== item.uuid));
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  // ── Chapter tree item ──────────────────────────────────────────
  const ChapterItem = ({ node, depth }: { node: ChapterNode; depth: number }) => {
    const [open, setOpen] = useState(true);
    const isEditing = editChapterUUID === node.uuid;
    const isAddingChild = addingUnder === node.uuid;
    const completed = node.status === "completed";

    return (
      <div>
        <div
          className={`flex flex-row items-center gap-1 rounded-lg px-2 py-1 group ${
            completed ? "bg-green-500/15" : "bg-bg-muted hover:bg-bg-hover"
          }`}
          style={{ marginLeft: depth * 20 }}
        >
          {isEditing ? (
            <div className="flex flex-row gap-2 flex-1 py-1">
              <input
                autoFocus
                className={inputCls}
                value={editChapterTitle}
                onChange={(e) => setEditChapterTitle(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleSaveEditChapter(node);
                  if (e.key === "Escape") setEditChapterUUID(null);
                }}
              />
              <button className={btnPrimary} disabled={saving} onClick={() => handleSaveEditChapter(node)}>
                Save
              </button>
              <button className={btnGhost} onClick={() => setEditChapterUUID(null)}>
                Cancel
              </button>
            </div>
          ) : (
            <>
              <button
                className="p-0.5 text-text-tertiary cursor-pointer shrink-0"
                onClick={() => setOpen((v) => !v)}
              >
                {node.children.length > 0 ? (
                  open ? <ChevronDown size={16} /> : <ChevronRight size={16} />
                ) : (
                  <span className="inline-block w-4" />
                )}
              </button>
              <div className="flex-1 min-w-0 font-medium text-text-primary truncate">
                {node.title}
              </div>
              <div className="flex flex-row gap-0.5 shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                  title={completed ? "Mark incomplete" : "Mark completed"}
                  className={`p-1 rounded cursor-pointer ${
                    completed ? "text-accent" : "hover:bg-bg-card text-text-secondary"
                  }`}
                  disabled={saving}
                  onClick={() => handleToggleStatus(node)}
                >
                  <Check size={15} />
                </button>
                <button
                  title="Copy UUID"
                  className="p-1 rounded hover:bg-bg-card text-text-secondary cursor-pointer"
                  onClick={() => navigator.clipboard.writeText(node.uuid)}
                >
                  <Copy size={15} />
                </button>
                <button
                  title="Add child chapter"
                  className="p-1 rounded hover:bg-bg-card text-text-secondary cursor-pointer"
                  onClick={() => {
                    setEditChapterUUID(null);
                    setAddingUnder((prev) => (prev === node.uuid ? undefined : node.uuid));
                    setAddChapterTitle("");
                  }}
                >
                  <Plus size={15} />
                </button>
                <button
                  title="Edit"
                  className="p-1 rounded hover:bg-bg-card text-text-secondary cursor-pointer"
                  onClick={() => {
                    setAddingUnder(undefined);
                    setEditChapterUUID(node.uuid);
                    setEditChapterTitle(node.title ?? "");
                  }}
                >
                  <Pencil size={15} />
                </button>
                <button
                  title="Delete"
                  className="p-1 rounded hover:bg-bg-card text-red-500 cursor-pointer"
                  disabled={saving}
                  onClick={() => handleDeleteChapter(node)}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            </>
          )}
        </div>

        {isAddingChild && (
          <div className="flex flex-row gap-2 py-1" style={{ marginLeft: (depth + 1) * 20 }}>
            <input
              autoFocus
              className={inputCls}
              placeholder="Child chapter title"
              value={addChapterTitle}
              onChange={(e) => setAddChapterTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleAddChapter(node.uuid);
                if (e.key === "Escape") setAddingUnder(undefined);
              }}
            />
            <button className={btnPrimary} disabled={saving} onClick={() => handleAddChapter(node.uuid)}>
              Add
            </button>
            <button className={btnGhost} onClick={() => setAddingUnder(undefined)}>
              Cancel
            </button>
          </div>
        )}

        {open &&
          node.children.map((child) => (
            <ChapterItem key={child.uuid} node={child} depth={depth + 1} />
          ))}
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-4">
      {/* Books header */}
      <div className="flex flex-row items-center justify-between">
        <h2 className="text-lg font-semibold text-text-primary flex items-center gap-2">
          <BookOpen size={20} /> Books
        </h2>
        <button
          className={btnPrimary}
          onClick={() => {
            setShowAdd((v) => !v);
            setAddTitle("");
          }}
        >
          {showAdd ? "Cancel" : "+ New Book"}
        </button>
      </div>

      {showAdd && (
        <div className="flex flex-row gap-2 p-3 rounded-lg bg-bg-muted border border-border-default">
          <input
            autoFocus
            className={inputCls}
            placeholder="Book title"
            value={addTitle}
            onChange={(e) => setAddTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleAddBook();
            }}
          />
          <button className={btnPrimary} disabled={saving || !addTitle.trim()} onClick={handleAddBook}>
            Add Book
          </button>
        </div>
      )}

      {/* Book list */}
      <div className="flex flex-col gap-2">
        {books.map((item) => (
          <div
            key={item.uuid}
            className={`flex flex-col gap-2 p-3 rounded-lg border cursor-pointer transition-colors ${
              bookUUID === item.uuid
                ? "bg-bg-hover border-accent"
                : "bg-bg-card border-border-default hover:bg-bg-muted"
            }`}
            onClick={() => {
              if (bookUUID === item.uuid) {
                setBookUUID("");
                setFlat([]);
              } else {
                setBookUUID(item.uuid);
                setEditChapterUUID(null);
                setAddingUnder(undefined);
              }
            }}
          >
            {editBookUUID === item.uuid ? (
              <div className="flex flex-row gap-2" onClick={(e) => e.stopPropagation()}>
                <input
                  autoFocus
                  className={inputCls}
                  value={editBookTitle}
                  onChange={(e) => setEditBookTitle(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleRenameBook(item);
                    if (e.key === "Escape") setEditBookUUID(null);
                  }}
                />
                <button className={btnPrimary} disabled={saving} onClick={() => handleRenameBook(item)}>
                  Save
                </button>
                <button className={btnGhost} onClick={() => setEditBookUUID(null)}>
                  Cancel
                </button>
              </div>
            ) : (
              <div className="flex flex-row items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="text-base font-semibold text-text-primary truncate">
                    {item.title}
                  </div>
                  <div className="text-xs text-text-tertiary font-mono truncate">{item.uuid}</div>
                </div>
                <div className="flex flex-row gap-1 shrink-0" onClick={(e) => e.stopPropagation()}>
                  <button
                    className={btnGhost}
                    onClick={() => {
                      setEditBookUUID(item.uuid);
                      setEditBookTitle(item.title || "");
                    }}
                  >
                    Edit
                  </button>
                  <button
                    className="px-3 py-1.5 rounded-md text-sm text-red-500 border border-border-default hover:bg-bg-hover cursor-pointer disabled:opacity-50"
                    disabled={saving}
                    onClick={() => handleDeleteBook(item.uuid)}
                  >
                    Delete
                  </button>
                </div>
              </div>
            )}
          </div>
        ))}
        {books.length === 0 && (
          <div className="text-center text-text-tertiary py-8">No books yet.</div>
        )}
      </div>

      {/* Chapters */}
      {selectedBook && (
        <div className="border-t border-border-default pt-4">
          <div className="flex flex-row items-center justify-between mb-3">
            <h3 className="text-base font-semibold text-text-primary">
              Chapters — <span className="font-normal text-text-secondary">{selectedBook.title}</span>
            </h3>
            <button
              className={btnPrimary}
              onClick={() => {
                setEditChapterUUID(null);
                setAddingUnder((prev) => (prev === null ? undefined : null));
                setAddChapterTitle("");
              }}
            >
              {addingUnder === null ? "Cancel" : "+ Root Chapter"}
            </button>
          </div>

          {chaptersLoading && (
            <div className="text-center text-text-tertiary py-4">Loading…</div>
          )}

          {!chaptersLoading && (
            <>
              {addingUnder === null && (
                <div className="flex flex-row gap-2 py-1 mb-2">
                  <input
                    autoFocus
                    className={inputCls}
                    placeholder="Chapter title"
                    value={addChapterTitle}
                    onChange={(e) => setAddChapterTitle(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") handleAddChapter(null);
                      if (e.key === "Escape") setAddingUnder(undefined);
                    }}
                  />
                  <button className={btnPrimary} disabled={saving} onClick={() => handleAddChapter(null)}>
                    Add
                  </button>
                  <button className={btnGhost} onClick={() => setAddingUnder(undefined)}>
                    Cancel
                  </button>
                </div>
              )}

              {flat.length === 0 && addingUnder !== null && (
                <div className="text-center text-text-tertiary py-4">No chapters yet.</div>
              )}

              <div className="flex flex-col gap-0.5">
                {tree.map((node) => (
                  <ChapterItem key={node.uuid} node={node} depth={0} />
                ))}
              </div>
            </>
          )}
        </div>
      )}

      <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
    </div>
  );
}
