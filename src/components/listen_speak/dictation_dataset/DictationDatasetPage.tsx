"use client";

/**
 * DictationDatasetPage — a lightweight maintenance view over a dataset's
 * database. After picking a dataset (same breadcrumb pattern as the dictation
 * page) it lists every media row with its title, source and note, allowing the
 * user to edit and persist those fields, or remove a media entirely.
 */

import { useState, useEffect, useCallback } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { RefreshCw, Trash2, Save } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { isAudio } from "@/lib/listen/utils";
import type { ListenMedia } from "@/lib/types";
import {
  type DatasetSummary,
  btnSmPrimary,
  btnSmDanger,
  btnSmSecondary,
} from "@/lib/datasets/types";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";

type Edit = { title: string; note: string; source: string };

export default function DictationDatasetPage() {
  const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
  const [selectedDatasetUuid, setSelectedDatasetUuid] = useState<string>("");
  const [mediaList, setMediaList] = useState<ListenMedia[]>([]);
  const [edits, setEdits] = useState<Record<string, Edit>>({});
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);

  const selectedDataset = datasets.find((d) => d.info.uuid === selectedDatasetUuid);

  // ── Load datasets ──
  const loadDatasets = useCallback(() => {
    if (!isTauri()) return;
    setLoading(true);
    invoke<DatasetSummary[]>("dataset_list")
      .then((res) => setDatasets(res.filter((d) => d.status === "ready")))
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => { loadDatasets(); }, [loadDatasets]);

  // ── Load media list when the dataset changes ──
  const loadMedia = useCallback((uuid: string) => {
    if (!uuid) { setMediaList([]); return; }
    setLoading(true);
    invoke<ListenMedia[]>("listen_list_media", { datasetUuid: uuid })
      .then((res) => { setMediaList(res); setEdits({}); })
      .catch((e) => { console.error(e); setMediaList([]); setEdits({}); })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => { loadMedia(selectedDatasetUuid); }, [selectedDatasetUuid, loadMedia]);

  // ── Editing helpers ──
  const dirtyCount = Object.keys(edits).length;
  const isDirty = (m: ListenMedia) => {
    const e = edits[m.uuid];
    return !!e && (e.title !== m.title || e.note !== m.note || e.source !== m.source);
  };
  const fieldOf = (m: ListenMedia, key: "title" | "note" | "source") => edits[m.uuid]?.[key] ?? m[key];

  const setField = (m: ListenMedia, key: "title" | "note" | "source", value: string) => {
    setEdits((prev) => {
      const base = prev[m.uuid] ?? { title: m.title, note: m.note, source: m.source };
      const next = { ...base, [key]: value };
      // Drop the entry entirely when it matches the stored value again.
      if (next.title === m.title && next.note === m.note && next.source === m.source) {
        const { [m.uuid]: _removed, ...rest } = prev;
        void _removed;
        return rest;
      }
      return { ...prev, [m.uuid]: next };
    });
  };

  // Build a playable asset URL for a media source within the selected dataset.
  const getMediaSrc = (source: string) => {
    if (!selectedDataset || !source || !isTauri()) return "";
    return convertFileSrc(`${selectedDataset.path}/media/${source}`);
  };

  // ── Persistence ──
  const saveMedia = useCallback(async (m: ListenMedia, edit: Edit) => {
    await invoke("listen_save_media", {
      datasetUuid: selectedDatasetUuid,
      media: { ...m, title: edit.title, source: edit.source, note: edit.note, updated_at: new Date().toISOString() },
    });
  }, [selectedDatasetUuid]);

  const handleSaveRow = useCallback(async (m: ListenMedia) => {
    const edit = edits[m.uuid];
    if (!edit) return;
    setSaving(true);
    try {
      await saveMedia(m, edit);
      setMediaList((prev) => prev.map((x) => (x.uuid === m.uuid ? { ...x, title: edit.title, source: edit.source, note: edit.note } : x)));
      setEdits((prev) => { const { [m.uuid]: _r, ...rest } = prev; void _r; return rest; });
    } catch (e) {
      setConfirmReq({ title: "Save failed", message: String(e) });
    } finally { setSaving(false); }
  }, [edits, saveMedia]);

  const handleSaveAll = useCallback(async () => {
    if (dirtyCount === 0) return;
    setSaving(true);
    try {
      const entries = Object.entries(edits);
      await Promise.all(entries.map(([uuid, edit]) => {
        const m = mediaList.find((x) => x.uuid === uuid);
        return m ? saveMedia(m, edit) : Promise.resolve();
      }));
      setMediaList((prev) => prev.map((x) => (edits[x.uuid] ? { ...x, ...edits[x.uuid] } : x)));
      setEdits({});
    } catch (e) {
      setConfirmReq({ title: "Save failed", message: String(e) });
    } finally { setSaving(false); }
  }, [dirtyCount, edits, mediaList, saveMedia]);

  const handleDelete = useCallback(async (m: ListenMedia) => {
    setSaving(true);
    try {
      await invoke("listen_delete_media", { datasetUuid: selectedDatasetUuid, mediaUuid: m.uuid });
      setMediaList((prev) => prev.filter((x) => x.uuid !== m.uuid));
      setEdits((prev) => { const { [m.uuid]: _r, ...rest } = prev; void _r; return rest; });
    } catch (e) {
      setConfirmReq({ title: "Remove failed", message: String(e) });
    } finally { setSaving(false); }
  }, [selectedDatasetUuid]);

  return (
    <div className="flex flex-col flex-1 min-h-0 p-4 overflow-hidden">
      {/* Breadcrumb navigation */}
      <nav className="flex flex-row items-center gap-2 mb-4 text-sm select-none shrink-0">
        <button
          className={`cursor-pointer hover:underline ${selectedDatasetUuid ? "text-accent" : "text-text-primary font-medium"}`}
          onClick={() => setSelectedDatasetUuid("")}
        >
          Datasets
        </button>
        {selectedDataset && (
          <>
            <span className="text-text-tertiary">&rsaquo;</span>
            <span className="text-text-primary font-medium truncate max-w-[320px]">{selectedDataset.info.name}</span>
          </>
        )}
        {loading && <span className="text-text-tertiary text-xs">Loading…</span>}
      </nav>

      {/* Datasets view (initial) */}
      {!selectedDatasetUuid && (
        <div className="flex flex-col gap-2 flex-1 min-h-0 overflow-y-auto">
          {datasets.length === 0 ? (
            <p className="text-text-secondary">No datasets available. Generate a database for a dataset on the Datasets &rsaquo; Location page first.</p>
          ) : datasets.map((ds) => (
            <button
              key={ds.path}
              className="text-left p-4 border border-border-default rounded-lg hover:bg-bg-hover cursor-pointer transition-colors"
              onClick={() => setSelectedDatasetUuid(ds.info.uuid)}
            >
              <div className="font-medium text-text-primary">{ds.info.name}</div>
              <div className="text-xs text-text-tertiary truncate" title={ds.path}>{ds.path}</div>
            </button>
          ))}
        </div>
      )}

      {/* Media maintenance view */}
      {selectedDatasetUuid && (
        <div className="flex flex-col gap-3 flex-1 min-h-0">
          {/* Toolbar */}
          <div className="flex flex-row items-center gap-2 shrink-0">
            <span className="text-sm text-text-secondary">{mediaList.length} media</span>
            <div className="ml-auto flex flex-row items-center gap-2">
              {dirtyCount > 0 && (
                <button className={btnSmPrimary} disabled={saving} onClick={handleSaveAll}>
                  <span className="inline-flex items-center gap-1"><Save size={14} /> Save all ({dirtyCount})</span>
                </button>
              )}
              <button className={btnSmSecondary} disabled={loading} onClick={() => loadMedia(selectedDatasetUuid)} title="Reload media list">
                <span className="inline-flex items-center gap-1"><RefreshCw size={14} /> Reload</span>
              </button>
            </div>
          </div>

          {/* Table header */}
          <div className="grid grid-cols-[1fr_1fr_1fr_auto] gap-3 px-3 text-xs font-medium text-text-tertiary select-none shrink-0">
            <span>Title</span>
            <span>Source</span>
            <span>Note</span>
            <span className="w-24 text-right">Actions</span>
          </div>

          {/* Rows */}
          <div className="flex flex-col gap-2 flex-1 min-h-0 overflow-y-auto pb-4">
            {mediaList.length === 0 && !loading ? (
              <p className="text-text-secondary">No media in this dataset.</p>
            ) : mediaList.map((m) => {
              const dirty = isDirty(m);
              const source = fieldOf(m, "source");
              const src = getMediaSrc(source);
              return (
                <div
                  key={m.uuid}
                  className={`flex flex-col gap-2 px-3 py-2 border rounded-lg transition-colors ${dirty ? "border-accent bg-accent-bg/10" : "border-border-default"}`}
                >
                  <div className="grid grid-cols-[1fr_1fr_1fr_auto] gap-3 items-center">
                  <input
                    className="w-full px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                    value={fieldOf(m, "title")}
                    placeholder="(untitled)"
                    disabled={saving}
                    onChange={(e) => setField(m, "title", e.target.value)}
                  />
                  <input
                    className="w-full px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                    value={source}
                    placeholder="(source)"
                    disabled={saving}
                    onChange={(e) => setField(m, "source", e.target.value)}
                  />
                  <input
                    className="w-full px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                    value={fieldOf(m, "note")}
                    placeholder="—"
                    disabled={saving}
                    onChange={(e) => setField(m, "note", e.target.value)}
                  />
                  <div className="flex flex-row items-center justify-end gap-1 w-24">
                    <button
                      className={btnSmPrimary}
                      disabled={!dirty || saving}
                      title="Save changes"
                      onClick={() => handleSaveRow(m)}
                    >
                      <Save size={14} />
                    </button>
                    <button
                      className={btnSmDanger}
                      disabled={saving}
                      title="Remove media and all related data"
                      onClick={() => setConfirmReq({
                        title: "Remove media",
                        message: `Remove "${m.title || m.source || "this media"}" and all related data and files? This cannot be undone.`,
                        confirmLabel: "Remove",
                        onConfirm: () => handleDelete(m),
                      })}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                  </div>
                  {src && (isAudio(source)
                    ? <audio controls preload="none" src={src} className="w-full" />
                    : <video controls preload="none" src={src} className="w-full max-h-64" />)}
                </div>
              );
            })}
          </div>
        </div>
      )}

      <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
    </div>
  );
}
