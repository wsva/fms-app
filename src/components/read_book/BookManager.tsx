"use client";

import { useEffect, useMemo, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  BookOpen,
  FolderOpen,
  FolderPlus,
  MapPin,
  X,
  AlertTriangle,
  Eye,
  RefreshCw,
} from "lucide-react";
import type { BookMeta, BookChapter } from "@/lib/read/types";
import {
  chaptersToMarkdown,
  parseChaptersMarkdown,
  buildSyncNodes,
  type SyncNode,
  type SyncResult,
} from "@/lib/read/chapterMarkdown";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import HighlightedChapterInput from "./HighlightedChapterInput";
import ConfirmDialog, { type ConfirmRequest } from "./ConfirmDialog";

const btnGhost =
  "px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover border border-border-default cursor-pointer disabled:opacity-50";
const btnPrimary =
  "px-3 py-1.5 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50";
const inputCls =
  "flex-1 px-3 py-1.5 rounded-md bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent";

type Props = {
  books: BookMeta[];
  /** Shared selected book (owned by useBookData so the breadcrumb reflects it). */
  bookUUID: string;
  setBookUUID: (uuid: string) => void;
  onBooksChanged: () => void;
};

export default function BookManager({ books, bookUUID, setBookUUID, onBooksChanged }: Props) {
  const [saving, setSaving] = useState(false);
  const [showAdd, setShowAdd] = useState(false);
  const [addTitle, setAddTitle] = useState("");
  const [editBookUUID, setEditBookUUID] = useState<string | null>(null);
  const [editBookTitle, setEditBookTitle] = useState("");
  // Deferred platform flag (SSR-safe). On touch there is no hover, so row
  // actions that reveal on `group-hover` must stay visible on mobile. It also
  // hides the folder Browse... button: `open({ directory: true })` is desktop-only
  // (the dialog plugin returns FolderPickerNotImplemented), while the path field
  // below can still be filled by hand.
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Locations state
  const [dirs, setDirs] = useState<{ name: string; path: string; is_linked: boolean; is_read_only?: boolean }[]>([]);
  const [showAddDir, setShowAddDir] = useState(false);
  const [newDirName, setNewDirName] = useState("");
  const [newDirPath, setNewDirPath] = useState("");
  const [locationError, setLocationError] = useState<string | null>(null);

  const loadDirs = useCallback(async () => {
    try {
      const result = await invoke<{ name: string; path: string; is_linked: boolean; is_read_only?: boolean }[]>(
        "dataset_list_dirs",
        { datasetType: "book" }
      );
      setDirs(result);
    } catch {
      // Ignore
    }
  }, []);

  useEffect(() => {
    loadDirs();
  }, [loadDirs]);

  const handleAddLocation = async () => {
    if (!newDirName.trim() || !newDirPath.trim()) return;
    setLocationError(null);
    try {
      await invoke("dataset_add_dir", { name: newDirName.trim(), path: newDirPath.trim() });
      setShowAddDir(false);
      setNewDirName("");
      setNewDirPath("");
      loadDirs();
      onBooksChanged();
    } catch (e) {
      setLocationError(String(e));
    }
  };

  const handlePickLocation = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Select Book Directory" });
    if (typeof picked === "string") {
      setNewDirPath(picked);
      if (!newDirName.trim()) {
        setNewDirName(picked.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || picked);
      }
    }
  };

  const handleRemoveLocation = async (path: string) => {
    const confirmed = await ask(`Unlink this directory? Books inside will no longer be visible.`, {
      title: "Unlink Directory",
    });
    if (!confirmed) return;
    setLocationError(null);
    try {
      await invoke("dataset_remove_dir", { path });
      loadDirs();
      onBooksChanged();
    } catch (e) {
      setLocationError(String(e));
    }
  };

  const [flat, setFlat] = useState<BookChapter[]>([]);
  const [chaptersLoading, setChaptersLoading] = useState(false);
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);

  // Round-trip Markdown structure editor state. In the Manage view the editor is
  // shown directly for the selected book (no intermediate tree browser).
  const [mdText, setMdText] = useState("");
  const [mdErrors, setMdErrors] = useState<string[]>([]);
  const [mdPreview, setMdPreview] = useState<SyncResult | null>(null);
  const [mdBusy, setMdBusy] = useState(false);

  const selectedBook = books.find((b) => b.uuid === bookUUID);

  // Live per-line validation for the structure editor (red flags offending lines
  // as you type, before an explicit Preview/Apply).
  const mdErrorLines = useMemo(
    () => parseChaptersMarkdown(mdText).errorLines,
    [mdText]
  );

  // Load chapters when a book is selected and seed the Markdown editor with them
  // (the editor is the Manage view itself, so it opens directly on selection).
  useEffect(() => {
    if (!bookUUID || !isTauri()) return;
    const load = async () => {
      setChaptersLoading(true);
      try {
        const data = await invoke<BookChapter[]>("book_list_chapters", { bookUuid: bookUUID });
        setFlat(data);
        setMdText(chaptersToMarkdown(data));
      } catch {
        setFlat([]);
        setMdText("");
      }
      setMdErrors([]);
      setMdPreview(null);
      setChaptersLoading(false);
    };
    load();
  }, [bookUUID]);

  // ── Book handlers ──────────────────────────────────────────────
  const handleAddBook = async () => {
    if (!addTitle.trim() || !isTauri()) return;
    setSaving(true);
    try {
      await invoke<BookMeta>("book_create", { name: addTitle.trim() });
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
      await invoke("book_rename", { uuid: item.uuid, name: editBookTitle.trim() });
      setEditBookUUID(null);
      onBooksChanged();
    } catch (e) {
      console.error(e);
    }
    setSaving(false);
  };

  const handleDeleteBook = (uuid: string) => {
    setConfirmReq({
      message: "Delete this book and all its chapters?\n\nThe folder is moved to trash, not permanently deleted.",
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

  // ── Structure editor (round-trip Markdown) ─────────────────────
  const handleMdReload = () => {
    setMdText(chaptersToMarkdown(flat));
    setMdErrors([]);
    setMdPreview(null);
  };

  // Parse the editor text into backend nodes, surfacing any validation error.
  // Returns null (with `mdErrors` populated) when the text is not sendable.
  const buildFromMd = (): SyncNode[] | null => {
    const { nodes, errors } = parseChaptersMarkdown(mdText);
    if (errors.length) {
      setMdErrors(errors);
      setMdPreview(null);
      return null;
    }
    const built = buildSyncNodes(nodes);
    if (built.errors.length) {
      setMdErrors(built.errors);
      setMdPreview(null);
      return null;
    }
    setMdErrors([]);
    return built.syncNodes;
  };

  const handleMdPreview = async () => {
    if (!bookUUID || !isTauri()) return;
    const nodes = buildFromMd();
    if (!nodes) return;
    setMdBusy(true);
    try {
      const result = await invoke<SyncResult>("book_sync_chapters", {
        bookUuid: bookUUID,
        nodes,
        dryRun: true,
      });
      setMdPreview(result);
    } catch (e) {
      setMdErrors([String(e)]);
      setMdPreview(null);
    }
    setMdBusy(false);
  };

  const runApply = async (nodes: SyncNode[]) => {
    setMdBusy(true);
    try {
      const result = await invoke<SyncResult>("book_sync_chapters", {
        bookUuid: bookUUID,
        nodes,
        dryRun: false,
      });
      setFlat(result.chapters);
      setMdText(chaptersToMarkdown(result.chapters));
      setMdErrors([]);
      setMdPreview(null);
      onBooksChanged();
    } catch (e) {
      setMdErrors([String(e)]);
    }
    setMdBusy(false);
  };

  const handleMdApply = async () => {
    if (!bookUUID || !isTauri()) return;
    const nodes = buildFromMd();
    if (!nodes) return;
    setMdBusy(true);
    let preview: SyncResult;
    try {
      preview = await invoke<SyncResult>("book_sync_chapters", {
        bookUuid: bookUUID,
        nodes,
        dryRun: true,
      });
    } catch (e) {
      setMdErrors([String(e)]);
      setMdBusy(false);
      return;
    }
    setMdBusy(false);
    setMdPreview(preview);
    // Deletions that still carry sentences need an explicit, itemized OK.
    const risky = preview.summary.deletions.filter((d) => d.sentences > 0);
    if (risky.length) {
      const totalSentences = risky.reduce((s, d) => s + d.sentences, 0);
      setConfirmReq({
        title: "Chapters hold recorded content",
        message: (
          <div className="flex flex-col gap-2">
            <p>
              Applying deletes {risky.length} chapter(s) that still contain {totalSentences} sentence(s):
            </p>
            <ul className="list-disc pl-5 max-h-40 overflow-y-auto">
              {risky.map((d) => (
                <li key={d.uuid} className="text-xs">
                  {d.title} — {d.chapters} chapter{d.chapters !== 1 ? "s" : ""}, {d.sentences} sentence
                  {d.sentences !== 1 ? "s" : ""}
                </li>
              ))}
            </ul>
            <p className="text-xs text-text-tertiary">This cannot be undone here — recover from a backup if needed.</p>
          </div>
        ),
        confirmLabel: "Delete & Apply",
        onConfirm: () => runApply(nodes),
      });
      return;
    }
    await runApply(nodes);
  };

  return (
    <div className="flex flex-col gap-4 min-w-0 max-w-full h-full min-h-0 overflow-x-hidden">
      {/* Books level (no book selected): Locations + book CRUD overview. When a
          book is selected, Manage switches to editing that book's chapters. */}
      {!selectedBook && (
      <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-4 min-w-0">
      {/* Locations */}
      <section>
        <h2 className="text-lg font-semibold mb-3 flex items-center gap-2">
          <FolderOpen size={18} />
          Locations
        </h2>
        <p className="text-sm text-text-secondary mb-3">
          Books are stored across book directories. Add linked directories to store books on external drives or other locations.
        </p>

        {locationError && (
          <div className="p-3 bg-red-500/10 text-red-500 rounded-md text-sm mb-3">{locationError}</div>
        )}

        {/* Directory list */}
        <div className="space-y-2 mb-3">
          {dirs.map((dir) => {
            const booksInDir = books.filter((b) => b.path.startsWith(dir.path));
            return (
              <div
                key={dir.path}
                className="p-3 rounded-lg border border-border-default bg-bg-card"
              >
                <div className="flex items-center justify-between mb-1 gap-2">
                  <div className="flex items-center gap-2 flex-wrap min-w-0">
                    <MapPin size={14} className="text-text-tertiary" />
                    <span className="text-sm font-medium">{dir.name}</span>
                    {!dir.is_linked && !dir.is_read_only && (
                      <span className="text-xs px-1.5 py-0.5 rounded bg-accent-bg/10 text-accent">
                        default
                      </span>
                    )}
                    {dir.is_read_only && (
                      <span className="text-xs px-1.5 py-0.5 rounded border border-border-light bg-bg-body text-text-secondary">
                        app-managed
                      </span>
                    )}
                    <span className="text-xs text-text-tertiary">
                      {booksInDir.length} book{booksInDir.length !== 1 ? "s" : ""}
                    </span>
                  </div>
                  {dir.is_linked && (
                    <button
                      onClick={() => handleRemoveLocation(dir.path)}
                      className="p-1 rounded hover:bg-red-500/10 text-text-tertiary hover:text-red-500"
                      title="Remove linked directory"
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>
                <p className="text-xs text-text-tertiary font-mono break-all">{dir.path}</p>
              </div>
            );
          })}
        </div>

        {/* Add linked directory */}
        {showAddDir ? (
          <div className="p-3 rounded-lg border border-border-default bg-bg-card space-y-2">
            <div>
              <label className="block text-xs font-medium mb-1">Display Name</label>
              <input
                type="text"
                value={newDirName}
                onChange={(e) => setNewDirName(e.target.value)}
                placeholder="e.g. External SSD"
                className="w-full px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
              />
            </div>
            <div>
              <label className="block text-xs font-medium mb-1">Directory Path</label>
              <div className="flex gap-1">
                <input
                  type="text"
                  value={newDirPath}
                  onChange={(e) => setNewDirPath(e.target.value)}
                  placeholder="e.g. /mnt/external/books"
                  className="flex-1 px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
                />
                {!mobile && (
                  <button
                    onClick={handlePickLocation}
                    className="px-2 py-1 rounded border border-border-default hover:bg-bg-hover text-sm"
                    title="Browse..."
                  >
                    <FolderOpen size={14} />
                  </button>
                )}
              </div>
            </div>
            <div className="flex gap-2">
              <button
                onClick={handleAddLocation}
                className={btnPrimary}
              >
                Add
              </button>
              <button
                onClick={() => {
                  setShowAddDir(false);
                  setNewDirName("");
                  setNewDirPath("");
                }}
                className={btnGhost}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setShowAddDir(true)}
            className={btnGhost}
          >
            <FolderPlus size={14} className="inline mr-1" />
            Add Linked Directory
          </button>
        )}
      </section>

      {/* Books header */}
      <div className="flex flex-row items-center justify-between border-t border-border-default pt-4">
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
                setMdText("");
              } else {
                setBookUUID(item.uuid);
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
                    {item.name}
                  </div>
                  <div className="text-xs text-text-tertiary font-mono truncate">{item.uuid}</div>
                </div>
                <div className="flex flex-row gap-1 shrink-0" onClick={(e) => e.stopPropagation()}>
                  <button
                    className={btnGhost}
                    onClick={() => {
                      setEditBookUUID(item.uuid);
                      setEditBookTitle(item.name || "");
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
      </div>
      )}

      {/* Chapters (book level) — the Markdown structure editor fills the height. */}
      {selectedBook && (
        <div className="flex flex-col gap-4 min-h-0 flex-1 min-w-0">
          <h3 className="text-base font-semibold text-text-primary shrink-0">
            Chapters — <span className="font-normal text-text-secondary">{selectedBook.name}</span>
          </h3>

          {chaptersLoading ? (
            <div className="text-center text-text-tertiary py-4">Loading…</div>
          ) : (
            <div className="flex flex-col gap-3 p-3 rounded-lg border border-border-default bg-bg-card min-h-0 flex-1">
              <p className="text-xs text-text-secondary leading-relaxed shrink-0">
                One heading line per chapter, nested by <code className="px-1 rounded bg-bg-muted">#</code> depth. Keep a
                line&rsquo;s trailing <code className="px-1 rounded bg-bg-muted">{"{#uuid}"}</code> to preserve its sentences
                and completion; a line without one is a new chapter. Remove a line to delete that chapter (and its whole
                subtree). Preview before Apply to see exactly what changes.
              </p>

              <div className="flex gap-2 flex-wrap shrink-0 justify-end">
                <button className={btnGhost} disabled={mdBusy} onClick={handleMdPreview}>
                  <Eye size={14} className="inline mr-1" /> Preview
                </button>
                <button className={btnPrimary} disabled={mdBusy} onClick={handleMdApply}>
                  Apply
                </button>
                <button className={btnGhost} disabled={mdBusy} onClick={handleMdReload} title="Discard edits and reload from the current chapters">
                  <RefreshCw size={14} className="inline mr-1" /> Reload
                </button>
              </div>

              {mdErrors.length > 0 && (
                <div className="p-2 rounded-md bg-red-500/10 text-red-500 text-xs space-y-1 shrink-0">
                  {mdErrors.map((err, i) => (
                    <div key={i}>{err}</div>
                  ))}
                </div>
              )}

              {mdPreview && (
                <div className="p-2 rounded-md bg-bg-muted text-xs space-y-2 shrink-0 max-h-48 overflow-y-auto">
                  <div className="font-medium text-text-primary">Preview</div>
                  <div className="flex flex-wrap gap-x-3 gap-y-1 text-text-secondary">
                    <span>Created: {mdPreview.summary.created}</span>
                    <span>Renamed: {mdPreview.summary.updated}</span>
                    <span>Moved: {mdPreview.summary.moved}</span>
                    <span>Reordered: {mdPreview.summary.reordered}</span>
                    <span>Deleted: {mdPreview.summary.deleted}</span>
                  </div>
                  {mdPreview.summary.deletions.length > 0 && (
                    <div className="flex items-start gap-1.5 text-red-500">
                      <AlertTriangle size={14} className="mt-0.5 shrink-0" />
                      <div className="space-y-1">
                        <div>Removing {mdPreview.summary.deletions.length} chapter tree(s):</div>
                        <ul className="list-disc pl-4">
                          {mdPreview.summary.deletions.map((d) => (
                            <li key={d.uuid}>
                              {d.title} — {d.chapters} chapter{d.chapters !== 1 ? "s" : ""}, {d.sentences} sentence
                              {d.sentences !== 1 ? "s" : ""}
                              {d.sentences > 0 ? " (contains recorded content)" : ""}
                            </li>
                          ))}
                        </ul>
                      </div>
                    </div>
                  )}
                  {mdPreview.summary.created === 0 &&
                    mdPreview.summary.updated === 0 &&
                    mdPreview.summary.moved === 0 &&
                    mdPreview.summary.reordered === 0 &&
                    mdPreview.summary.deleted === 0 && (
                      <div className="text-text-tertiary">No changes.</div>
                    )}
                </div>
              )}

              <HighlightedChapterInput
                className="w-full flex-1 min-h-0 rounded-md bg-bg-input border border-border-default focus-within:border-accent"
                value={mdText}
                errorLines={mdErrorLines}
                disabled={mdBusy}
                onChange={(v) => {
                  setMdText(v);
                  setMdPreview(null);
                }}
              />
            </div>
          )}
        </div>
      )}

      <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
    </div>
  );
}
