"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, message } from "@tauri-apps/plugin-dialog";
import {
  RefreshCw, ArrowLeft, ArrowRight, Cloud, Download, Pencil, Eye,
  Folder, FileText, BookMarked, FilePlus, FolderPlus, Plus, Trash2,
  Save, Maximize2, Minimize2, CircleHelp, Search,
} from "lucide-react";
import WikiSearch from "./WikiSearch";
import MdEditor from "./MdEditor";
import { insertAround, insertAtLineStart } from "./mdEditorInserts";
import MarkdownViewer from "./markdown/markdown";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";
import {
  WikiDatasetSummary, HubWikiList, Selection, WikiSource, WikiFileEntry,
  selectionKey,
} from "@/lib/wiki/types";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// Strip a dataset-root-relative wiki link to a clean rel path.
function normalizeWikiRel(relativePath: string): string {
  return relativePath.replace(/^\.?\//, "").replace(/\\/g, "/");
}

// Backend sanitize_rel rejects these; mirror the guard in the UI so a name
// prompt can't produce a path the backend would refuse.
function sanitizeName(raw: string): string {
  return raw.replace(/[\\/:*?"<>|]/g, "").replace(/\.\./g, "").trim();
}

function joinRel(dirRel: string, name: string): string {
  return dirRel ? `${dirRel}/${name}` : name;
}

interface PromptState {
  title: string;
  submit: (name: string) => Promise<void>;
}

// Symbol / character sets backing the wiki page's Tools sub-toolbar row. These
// used to live inside MdEditor; pulling them up here lets the row sit alongside
// the other sub-toolbars and call the shared textarea-insert helpers directly.
const char1 = ["#", "⬌", "■", "=", "≈", "➤", "🡆"];
const char1Tips = ["heading", "left-right arrow", "square", "equals", "approx.", "right arrow", "right arrow"];

const char2 = ["ä", "Ä", "ö", "Ö", "ü", "Ü", "ß", "é", "€"];

const char3 = [
  { label: "B", start: "**", end: "**", tip: "bold (Ctrl+B)" },
  { label: "„“", start: "„", end: "“", tip: "German double quotes" },
  { label: "‚‘", start: "‚", end: "‘", tip: "German single quotes" },
  { label: "`c`", start: "`", end: "`", tip: "inline code (Ctrl+`)" },
  { label: "C", start: "`````\n", end: "\n`````", tip: "code block" },
];

const char4 = [
  { label: "I", start: "*", end: "*", tip: "italic (Ctrl+I)" },
  { label: "~~", start: "~~", end: "~~", tip: "strikethrough" },
  { label: "🔗", start: "[", end: "](url)", tip: "link" },
  { label: "---", start: "\n---\n", end: "", tip: "horizontal rule" },
];

// One step in the Back/Forward trail. Files, directories and the top-level
// datasets list are all tracked so navigation buttons survive folder hops.
type NavEntry =
  | { kind: "file"; selection: Selection }
  | { kind: "directory"; uuid: string; rel: string }
  | { kind: "datasets" };

function sameNavEntry(a: NavEntry, b: NavEntry): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "datasets") return true;
  if (a.kind === "directory" && b.kind === "directory") {
    return a.uuid === b.uuid && a.rel === b.rel;
  }
  if (a.kind === "file" && b.kind === "file") {
    return selectionKey(a.selection.source) === selectionKey(b.selection.source);
  }
  return false;
}

export default function WikiPage() {
  const [selection, setSelection] = useState<Selection | null>(null);
  const [fileContent, setFileContent] = useState<string>("");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isIndexing, setIsIndexing] = useState(false);

  // Hub read-only browse + local wiki datasets.
  const [hubMode, setHubMode] = useState(false);
  const [localDatasets, setLocalDatasets] = useState<WikiDatasetSummary[]>([]);
  const [hubDatasets, setHubDatasets] = useState<WikiDatasetSummary[]>([]);
  const [isHub, setIsHub] = useState(false);

  // Breadcrumb-style dataset/directory browsing in the main content area,
  // mirroring the dictation page's Datasets > media > cue flow: datasets list
  // by default, click one to see its directories/files, click a file to open.
  const [datasetBrowse, setDatasetBrowse] = useState<{ uuid: string; rel: string } | null>(null);
  const [browseEntries, setBrowseEntries] = useState<WikiFileEntry[] | null>(null);
  const [browseLoading, setBrowseLoading] = useState(false);
  const [browseError, setBrowseError] = useState<string | null>(null);
  // Bumping this re-runs the directory fetch without changing `datasetBrowse`
  // (e.g. the toolbar's Refresh button on the same folder).
  const [browseReloadNonce, setBrowseReloadNonce] = useState(0);

  // Inline name prompt for the toolbar's New dataset/file/folder actions —
  // browser prompt() isn't available in webviews and the dialog plugin has no
  // prompt variant, so a tiny controlled row is used (same pattern as the sidebar).
  const [prompt, setPrompt] = useState<PromptState | null>(null);
  const [promptValue, setPromptValue] = useState("");
  const [promptBusy, setPromptBusy] = useState(false);

  // Search-row visibility. The sub-toolbar shows a compact search icon that
  // toggles a second row hosting the WikiSearch input and the Index button —
  // folding the old top navigation bar into the same vertical stack.
  const [searchOpen, setSearchOpen] = useState(false);

  // Dual-mode content: rendered HTML view vs. the ported markdown editor.
  // Editing is only ever offered for a locally stored wiki dataset file.
  const [editing, setEditing] = useState(false);
  const [editContent, setEditContent] = useState("");
  const [saving, setSaving] = useState(false);

  // Editor chrome toggles. MdEditor is fully controlled for these five flags;
  // the sub-toolbar below owns the buttons and this page owns the state so the
  // editor itself can stay a body-only component.
  const [mdToolsOpen, setMdToolsOpen] = useState(false);
  const [mdPreview, setMdPreview] = useState(false);
  const [mdFullscreen, setMdFullscreen] = useState(false);
  const [mdSpellCheck, setMdSpellCheck] = useState(false);
  const [mdShortcutsOpen, setMdShortcutsOpen] = useState(false);

  // WikiPage owns the ref so the Tools sub-toolbar can call the shared insert
  // helpers directly on the textarea, without routing through MdEditor.
  const mdTextareaRef = useRef<HTMLTextAreaElement | null>(null);

  const runInsert = (fn: (ta: HTMLTextAreaElement) => void) => {
    const ta = mdTextareaRef.current;
    if (ta) fn(ta);
  };

  // Every exit path funnels through `editing === false`, so clearing the five
  // toggles there keeps a subsequent Edit click starting from a clean state.
  useEffect(() => {
    if (!editing) {
      setMdToolsOpen(false);
      setMdPreview(false);
      setMdFullscreen(false);
      setMdSpellCheck(false);
      setMdShortcutsOpen(false);
    }
  }, [editing]);

  // `isMobileApp()` is false during prerender (no navigator) and only turns true
  // inside the webview, so resolve it in an effect to avoid a hydration mismatch.
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Navigation history over files, directories and the top-level datasets view.
  // Seeded with a datasets entry so Back from the first folder returns cleanly
  // and historyIndex starts at 0 (Back button disabled at the trail's head).
  const [history, setHistory] = useState<NavEntry[]>([{ kind: "datasets" }]);
  const [historyIndex, setHistoryIndex] = useState(0);
  const historyIndexRef = useRef(0);
  const isHistoryNavRef = useRef(false);

  useEffect(() => {
    historyIndexRef.current = historyIndex;
  }, [historyIndex]);

  // Append a step to the trail, truncating any forward branch and de-duplicating
  // against the current entry so breadcrumb re-clicks don't spam history.
  const pushNavHistory = useCallback((entry: NavEntry) => {
    setHistory((prev) => {
      const currentIndex = historyIndexRef.current;
      const last = prev[currentIndex];
      if (last && sameNavEntry(last, entry)) return prev;
      const newHistory = prev.slice(0, currentIndex + 1);
      newHistory.push(entry);
      const newIndex = newHistory.length - 1;
      setHistoryIndex(newIndex);
      historyIndexRef.current = newIndex;
      return newHistory;
    });
  }, []);

  // Load locally stored wiki datasets (downloaded or hub-owned). Cross-platform.
  const loadLocalDatasets = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setLocalDatasets(await invoke<WikiDatasetSummary[]>("wiki_dataset_list"));
    } catch (err) {
      logError(`Failed to load wiki datasets: ${err instanceof Error ? err.message : String(err)}`, "wiki");
    }
  }, []);

  useEffect(() => {
    loadLocalDatasets();
  }, [loadLocalDatasets]);

  // Detect the hub role so the UI can hide pointless hub-browse of self.
  useEffect(() => {
    if (!isTauri()) return;
    invoke<{ role: string }>("sync_status")
      .then((s) => setIsHub(s.role === "hub"))
      .catch(() => setIsHub(false));
  }, []);

  // Read one file for a given source, dispatching to the right backend command.
  const readSource = useCallback(async (source: WikiSource): Promise<string> => {
    switch (source.kind) {
      case "dataset":
        return await invoke<string>("wiki_dataset_read_file", { uuid: source.uuid, rel: source.rel });
      case "hub-dataset":
        return await invoke<string>("wiki_hub_read_file", { uuid: source.uuid, rel: source.rel });
    }
  }, []);

  // Load a selection's content, leaving view mode, and push navigation history.
  const loadSelection = useCallback(async (sel: Selection, pushHistory: boolean) => {
    if (!isTauri()) return;
    if (sel.source.rel === "") {
      // A dataset root placeholder (used to clear a deleted file) — nothing to read.
      setSelection(null);
      setFileContent("");
      setEditing(false);
      return;
    }
    setIsLoading(true);
    setError(null);
    setEditing(false);
    try {
      const content = await readSource(sel.source);
      setFileContent(content);
      setSelection(sel);
      if (pushHistory && !isHistoryNavRef.current) {
        pushNavHistory({ kind: "file", selection: sel });
      }
    } catch (err) {
      console.error("Failed to read file:", err);
      setError(err instanceof Error ? err.message : String(err));
      setFileContent("");
      setSelection(sel);
    } finally {
      setIsLoading(false);
    }
  }, [readSource, pushNavHistory]);

  const handleFileSelect = useCallback((sel: Selection) => {
    loadSelection(sel, true);
  }, [loadSelection]);

  const restoreFromHistory = useCallback((entry: NavEntry) => {
    isHistoryNavRef.current = true;
    if (entry.kind === "file") {
      loadSelection(entry.selection, false);
    } else {
      // Directory / datasets-list entries reset the content pane without
      // touching history (the trail is already positioned by the caller).
      if (entry.kind === "directory") {
        setDatasetBrowse({ uuid: entry.uuid, rel: entry.rel });
      } else {
        setDatasetBrowse(null);
      }
      setSelection(null);
      setFileContent("");
      setEditing(false);
      setError(null);
    }
    isHistoryNavRef.current = false;
  }, [loadSelection]);

  const goBack = useCallback(() => {
    if (historyIndex <= 0) return;
    const entry = history[historyIndex - 1];
    setHistoryIndex(historyIndex - 1);
    restoreFromHistory(entry);
  }, [history, historyIndex, restoreFromHistory]);

  const goForward = useCallback(() => {
    if (historyIndex >= history.length - 1) return;
    const entry = history[historyIndex + 1];
    setHistoryIndex(historyIndex + 1);
    restoreFromHistory(entry);
  }, [history, historyIndex, restoreFromHistory]);

  // Keyboard shortcuts: Alt+Left = back, Alt+Right = forward. Suppressed while
  // the editor is mounted so its own history/selection keys win.
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (editing) return;
      if (e.altKey && e.key === "ArrowLeft") {
        e.preventDefault();
        goBack();
      } else if (e.altKey && e.key === "ArrowRight") {
        e.preventDefault();
        goForward();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [goBack, goForward, editing]);

  // Handle wiki link clicks within markdown content. Resolve against the
  // selected dataset dir (or the hub root in Hub mode).
  const handleWikiLink = useCallback((relativePath: string) => {
    const rel = normalizeWikiRel(relativePath);
    const cur = selection?.source;
    if (cur && cur.kind === "dataset") {
      loadSelection({ source: { kind: "dataset", uuid: cur.uuid, rel }, label: rel.split("/").pop() || rel }, true);
      return;
    }
    if (cur && cur.kind === "hub-dataset") {
      loadSelection({ source: { kind: "hub-dataset", uuid: cur.uuid, rel, name: cur.name }, label: rel.split("/").pop() || rel }, true);
    }
  }, [selection, loadSelection]);

  // Whether the current selection is an editable local dataset file.
  const canEdit =
    !hubMode && selection?.source.kind === "dataset" && selection.source.rel !== "";

  const startEdit = () => {
    setEditContent(fileContent);
    setEditing(true);
  };

  const handleSave = async () => {
    if (!selection || selection.source.kind !== "dataset") return;
    setSaving(true);
    try {
      await invoke("wiki_dataset_write_file", {
        uuid: selection.source.uuid,
        rel: selection.source.rel,
        content: editContent,
      });
      setFileContent(editContent);
      setEditing(false);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Save failed", kind: "error" });
    } finally {
      setSaving(false);
    }
  };

  // Ctrl+S (Cmd+S on Mac) still triggers Save while editing, now that the
  // button lives on the sub-toolbar. Uses a ref so the effect only re-binds on
  // edit-mode transitions instead of every keystroke.
  const handleSaveRef = useRef(handleSave);
  handleSaveRef.current = handleSave;
  useEffect(() => {
    if (!editing) return;
    const onKey = (e: KeyboardEvent) => {
      const ctrl = navigator.userAgent.includes("Mac") ? e.metaKey : e.ctrlKey;
      if (ctrl && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void handleSaveRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [editing]);

  // Download a hub dataset to this device (requirement 2), then flip the root
  // to local so Edit becomes available.
  const handleDownload = useCallback(async (uuid: string, rel: string) => {
    if (!isTauri()) return;
    setIsLoading(true);
    try {
      await invoke("dataset_sync_snapshot", { uuid });
      await loadLocalDatasets();
      setHubMode(false);
      setSelection(null);
      // Re-open the same file from the now-local dataset copy.
      const ds = (await invoke<WikiDatasetSummary[]>("wiki_dataset_list")).find((d) => d.uuid === uuid);
      if (ds) {
        loadSelection({ source: { kind: "dataset", uuid, rel }, label: rel.split("/").pop() || ds.name }, true);
      }
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Download failed", kind: "error" });
    } finally {
      setIsLoading(false);
    }
  }, [loadLocalDatasets, loadSelection]);

  // Enter / leave the read-only hub browse view.
  const toggleHubMode = useCallback(async () => {
    if (hubMode) {
      setHubMode(false);
      setDatasetBrowse(null);
      return;
    }
    if (!isTauri()) return;
    setIsLoading(true);
    try {
      const list = await invoke<HubWikiList>("wiki_hub_list");
      setHubDatasets(list.datasets || []);
      setSelection(null);
      setFileContent("");
      setDatasetBrowse(null);
      setHubMode(true);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Cannot reach hub", kind: "error" });
    } finally {
      setIsLoading(false);
    }
  }, [hubMode]);

  // The local dataset whose FTS index to refresh: the open file's dataset, or
  // the one currently being browsed. Null when neither is a local dataset.
  const indexableDatasetUuid =
    selection?.source.kind === "dataset"
      ? selection.source.uuid
      : !hubMode ? datasetBrowse?.uuid ?? null : null;

  const handleIndexWiki = async () => {
    if (!isTauri() || !indexableDatasetUuid) return;
    setIsIndexing(true);
    try {
      const count = await invoke<number>("wiki_dataset_index", { uuid: indexableDatasetUuid });
      console.log(`Indexed ${count} dataset files`);
    } catch (err) {
      console.error("Failed to index wiki:", err);
    } finally {
      setIsIndexing(false);
    }
  };

  // Sidebar dataset tree source: hub datasets while browsing, else local.
  const treeDatasets = hubMode ? hubDatasets : localDatasets;

  // Search is scoped to a dataset: the open file's dataset, else the one being
  // browsed. Null (search disabled) when no dataset is in context.
  const searchMode: { kind: "dataset"; uuid: string } | { kind: "hub"; uuid: string } | null = (() => {
    const src = selection?.source;
    if (hubMode) {
      const uuid = src && src.kind === "hub-dataset" ? src.uuid : datasetBrowse?.uuid;
      return uuid ? { kind: "hub", uuid } : null;
    }
    const uuid = src && src.kind === "dataset" ? src.uuid : datasetBrowse?.uuid;
    return uuid ? { kind: "dataset", uuid } : null;
  })();

  const handleSearchResult = useCallback((source: WikiSource) => {
    const label = source.rel.split("/").pop() || source.rel;
    handleFileSelect({ source, label });
  }, [handleFileSelect]);

  const headerLabel = selection
    ? (selection.label || selection.source.rel.split("/").pop())
    : "";

  // Narrow once so JSX callbacks see a concrete hub-dataset source.
  const hubSrc = selection && selection.source.kind === "hub-dataset" ? selection.source : null;

  // ---- Breadcrumb view state -------------------------------------------------
  // "file" (viewing a file) wins over "browse" (listing a dataset directory),
  // which wins over "datasets" (the default landing view).
  const viewMode: "datasets" | "browse" | "file" = selection
    ? "file"
    : datasetBrowse
      ? "browse"
      : "datasets";

  // Which dataset the breadcrumb refers to — from the open file's source when
  // viewing a file, else the dataset being browsed.
  const activeDatasetUuid =
    selection
      ? selection.source.uuid
      : datasetBrowse?.uuid ?? null;
  const activeDatasetName = activeDatasetUuid
    ? treeDatasets.find((d) => d.uuid === activeDatasetUuid)?.name ?? activeDatasetUuid
    : "";

  // Directory path segments shown in the breadcrumb: taken from the open
  // file's parent dirs, or from the directory being browsed.
  const currentDirSegments = (() => {
    if (selection) {
      return selection.source.rel.split("/").filter(Boolean).slice(0, -1);
    }
    if (datasetBrowse) {
      return datasetBrowse.rel.split("/").filter(Boolean);
    }
    return [];
  })();

  const clearSelection = useCallback(() => {
    setSelection(null);
    setFileContent("");
    setEditing(false);
    setError(null);
  }, []);

  const goDatasets = useCallback(() => {
    setDatasetBrowse(null);
    clearSelection();
    if (!isHistoryNavRef.current) pushNavHistory({ kind: "datasets" });
  }, [clearSelection, pushNavHistory]);

  const goToDatasetDir = useCallback((uuid: string, rel: string) => {
    setDatasetBrowse({ uuid, rel });
    clearSelection();
    if (!isHistoryNavRef.current) pushNavHistory({ kind: "directory", uuid, rel });
  }, [clearSelection, pushNavHistory]);

  const openBrowseFile = useCallback((entry: WikiFileEntry) => {
    if (!datasetBrowse) return;
    const { uuid } = datasetBrowse;
    const source: WikiSource = hubMode
      ? { kind: "hub-dataset", uuid, rel: entry.rel_path, name: activeDatasetName }
      : { kind: "dataset", uuid, rel: entry.rel_path };
    handleFileSelect({ source, label: entry.name });
  }, [datasetBrowse, hubMode, activeDatasetName, handleFileSelect]);

  // Fetch the current directory listing whenever we (re)enter the browse view.
  useEffect(() => {
    if (viewMode !== "browse" || !datasetBrowse || !isTauri()) return;
    let cancelled = false;
    const { uuid, rel } = datasetBrowse;
    setBrowseLoading(true);
    setBrowseError(null);
    (async () => {
      try {
        const result = hubMode
          ? await invoke<WikiFileEntry[]>("wiki_hub_list_dir", { uuid, rel })
          : await invoke<WikiFileEntry[]>("wiki_dataset_list_dir", { uuid, rel });
        if (!cancelled) setBrowseEntries(result);
      } catch (err) {
        if (!cancelled) {
          const msg = err instanceof Error ? err.message : String(err);
          logError(`Failed to load wiki directory: ${msg}`, "wiki");
          setBrowseError(msg);
          setBrowseEntries(null);
        }
      } finally {
        if (!cancelled) setBrowseLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [viewMode, datasetBrowse, hubMode, browseReloadNonce]);

  const canMutateRoots = !mobile && !hubMode;

  // Shared sub-toolbar actions: shown under the breadcrumb for every dataset
  // view, not just while viewing a file's rendered markdown.
  const refreshBrowse = useCallback(() => setBrowseReloadNonce((n) => n + 1), []);

  const openPrompt = useCallback((title: string, submit: (name: string) => Promise<void>) => {
    setPromptValue("");
    setPrompt({ title, submit });
  }, []);

  const submitPrompt = useCallback(async () => {
    if (!prompt || promptBusy) return;
    const name = sanitizeName(promptValue);
    if (!name) return;
    setPromptBusy(true);
    try {
      await prompt.submit(name);
      setPrompt(null);
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
    } finally {
      setPromptBusy(false);
    }
  }, [prompt, promptBusy, promptValue]);

  const handleCreateDataset = useCallback(() => {
    openPrompt("New wiki dataset name", async (name) => {
      await invoke<WikiDatasetSummary>("wiki_dataset_create", { name });
      loadLocalDatasets();
    });
  }, [openPrompt, loadLocalDatasets]);

  const handleCreateFile = useCallback(() => {
    if (!datasetBrowse) return;
    const { uuid, rel } = datasetBrowse;
    openPrompt("New file (name without .md will get the extension)", async (raw) => {
      const fileName = raw.endsWith(".md") ? raw : `${raw}.md`;
      await invoke("wiki_dataset_create_file", { uuid, rel: joinRel(rel, fileName) });
      refreshBrowse();
      loadLocalDatasets();
    });
  }, [datasetBrowse, openPrompt, refreshBrowse, loadLocalDatasets]);

  const handleCreateDir = useCallback(() => {
    if (!datasetBrowse) return;
    const { uuid, rel } = datasetBrowse;
    openPrompt("New folder", async (name) => {
      await invoke("wiki_dataset_create_dir", { uuid, rel: joinRel(rel, name) });
      refreshBrowse();
    });
  }, [datasetBrowse, openPrompt, refreshBrowse]);

  // Download the whole dataset currently being browsed, so it becomes
  // locally readable/editable without first opening one of its files.
  const handleDownloadDataset = useCallback(async () => {
    if (!datasetBrowse) return;
    setIsLoading(true);
    try {
      await invoke("dataset_sync_snapshot", { uuid: datasetBrowse.uuid });
      await loadLocalDatasets();
      setHubMode(false);
      refreshBrowse();
    } catch (err) {
      await message(err instanceof Error ? err.message : String(err), { title: "Download failed", kind: "error" });
    } finally {
      setIsLoading(false);
    }
  }, [datasetBrowse, loadLocalDatasets, refreshBrowse]);

  // Delete the currently-open local dataset file (moves it to trash), then
  // navigate back to its parent directory.
  const handleDeleteCurrentFile = useCallback(() => {
    const src = selection?.source;
    if (!src || src.kind !== "dataset") return;
    void (async () => {
      const confirmed = await ask(`Delete this file?\n${src.rel}\n\nIt is moved to trash, not permanently deleted.`, { title: "Delete File", kind: "warning" });
      if (!confirmed) return;
      try {
        await invoke("wiki_dataset_delete_file", { uuid: src.uuid, rel: src.rel });
        const parent = src.rel.includes("/") ? src.rel.slice(0, src.rel.lastIndexOf("/")) : "";
        loadLocalDatasets();
        goToDatasetDir(src.uuid, parent);
      } catch (err) {
        await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
      }
    })();
  }, [selection, loadLocalDatasets, goToDatasetDir]);

  // Delete the wiki dataset currently being browsed (moves it to trash).
  const handleDeleteDataset = useCallback(() => {
    if (!datasetBrowse) return;
    const { uuid } = datasetBrowse;
    const name = activeDatasetName;
    void (async () => {
      const confirmed = await ask(`Delete wiki dataset "${name}"?\n\nThe folder is moved to trash, not permanently deleted.`, { title: "Delete Wiki Dataset", kind: "warning" });
      if (!confirmed) return;
      try {
        await invoke("wiki_dataset_delete", { uuid });
        loadLocalDatasets();
        goDatasets();
      } catch (err) {
        await message(err instanceof Error ? err.message : String(err), { title: "Error", kind: "error" });
      }
    })();
  }, [datasetBrowse, activeDatasetName, loadLocalDatasets, goDatasets]);

  const browseItemCount = browseEntries?.length ?? 0;
  const toolbarLabel =
    viewMode === "file" ? headerLabel
      : viewMode === "browse" ? `${browseItemCount} item${browseItemCount === 1 ? "" : "s"}`
        : `${treeDatasets.length} dataset${treeDatasets.length === 1 ? "" : "s"}`;

  const mdDirty = editContent !== fileContent;
  const inEditorFullscreen = editing && mdFullscreen;

  return (
    <div className="flex h-full w-full bg-bg-body min-w-0">
      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        {/* Hub read-only banner */}
        {hubMode && (
          <div className="px-4 py-1.5 text-xs bg-accent/10 text-accent border-b border-border-default flex items-center gap-2">
            <Cloud size={13} />
            <span>Hub view — read-only. Download a dataset to edit it locally.</span>
          </div>
        )}

        <div className="relative flex flex-1 min-h-0 overflow-hidden">
          {/* Content column: breadcrumb nav on top, scrollable view below */}
          <div className="flex-1 flex flex-col min-h-0 min-w-0">
            {/* Breadcrumb navigation — mirrors the dictation page's
                Wiki › dataset › directory › file trail. Hidden while the
                editor is in fullscreen to reclaim vertical space. */}
            {!inEditorFullscreen && (
            <nav className="shrink-0 flex items-center gap-2 px-4 py-2 text-sm select-none min-w-0 overflow-x-auto border-b border-border-default bg-bg-card">
              <button
                className={`shrink-0 cursor-pointer hover:underline ${viewMode === "datasets" ? "text-text-primary font-medium" : "text-accent"}`}
                onClick={goDatasets}
              >
                Wiki
              </button>
              {activeDatasetUuid && (
                <>
                  <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                  <button
                    className={`cursor-pointer hover:underline truncate min-w-0 max-w-[240px] ${viewMode === "file" || currentDirSegments.length > 0 ? "text-accent" : "text-text-primary font-medium"}`}
                    onClick={() => goToDatasetDir(activeDatasetUuid, "")}
                    title={activeDatasetName}
                  >
                    {activeDatasetName}
                  </button>
                </>
              )}
              {activeDatasetUuid && currentDirSegments.map((seg, i) => {
                const isLastDir = i === currentDirSegments.length - 1 && viewMode !== "file";
                const path = currentDirSegments.slice(0, i + 1).join("/");
                return (
                  <span key={path} className="flex items-center gap-2 min-w-0 shrink-0">
                    <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                    {isLastDir ? (
                      <span className="text-text-primary font-medium truncate max-w-[240px]" title={seg}>{seg}</span>
                    ) : (
                      <button
                        className="cursor-pointer hover:underline text-accent truncate max-w-[240px]"
                        onClick={() => goToDatasetDir(activeDatasetUuid, path)}
                        title={seg}
                      >
                        {seg}
                      </button>
                    )}
                  </span>
                );
              })}
              {viewMode === "file" && (
                <>
                  <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                  <span className="text-text-primary font-medium truncate min-w-0 max-w-[320px]" title={headerLabel}>
                    {headerLabel}
                  </span>
                </>
              )}
            </nav>
            )}

            {/* Shared sub-toolbar: always shown under the breadcrumb, whether
                viewing the dataset list, a dataset's directory, or a file.
                Left side carries the old top-bar controls (Back / Forward /
                Search toggle / Hub browse); right side keeps the contextual
                file/dataset actions. */}
            <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
              <div className="flex items-center gap-1 shrink-0">
                <button
                  onClick={goBack}
                  disabled={historyIndex <= 0 || editing}
                  className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
                  title="Back (Alt+Left)"
                >
                  <ArrowLeft size={16} />
                </button>
                <button
                  onClick={goForward}
                  disabled={historyIndex >= history.length - 1 || editing}
                  className="p-1.5 rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors disabled:opacity-30 disabled:cursor-default"
                  title="Forward (Alt+Right)"
                >
                  <ArrowRight size={16} />
                </button>
                <button
                  onClick={() => setSearchOpen((v) => !v)}
                  className={`p-1.5 rounded-md transition-colors ${
                    searchOpen
                      ? "bg-bg-hover text-text-primary"
                      : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                  }`}
                  title={searchOpen ? "Hide search" : "Show search"}
                >
                  <Search size={16} />
                </button>
              </div>

              {/* Hub browse toggle — read-only view of the paired hub's wiki.
                  Hidden for the hub itself (it already sees everything locally). */}
              {!isHub && (
                <button
                  onClick={toggleHubMode}
                  className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors shrink-0 ${
                    hubMode
                      ? "bg-accent text-white hover:bg-accent/90"
                      : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                  }`}
                  title={hubMode ? "Leave hub browse" : "Browse the hub's wiki (read-only)"}
                >
                  <Cloud size={14} />
                  <span>{hubMode ? "Local" : "Hub"}</span>
                </button>
              )}

              <div className="text-xs text-text-tertiary truncate flex-1 min-w-0">{toolbarLabel}</div>

              {viewMode === "file" && hubSrc && (
                <button
                  onClick={() => handleDownload(hubSrc.uuid, hubSrc.rel)}
                  disabled={isLoading}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50"
                  title="Download this dataset to edit locally"
                >
                  <Download size={14} /> Download
                </button>
              )}

              {viewMode === "file" && canEdit && !editing && (
                <button
                  onClick={startEdit}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                  title="Edit this file"
                >
                  <Pencil size={14} /> Edit
                </button>
              )}

              {viewMode === "file" && canEdit && !editing && (
                <button
                  onClick={handleDeleteCurrentFile}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-error-text hover:bg-bg-hover rounded-md transition-colors"
                  title="Delete this file (moves to trash)"
                >
                  <Trash2 size={14} /> Delete
                </button>
              )}

              {viewMode === "file" && canEdit && editing && (
                <>
                  <button
                    onClick={() => void handleSave()}
                    disabled={saving || !mdDirty}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors disabled:opacity-50 ${
                      mdDirty
                        ? "bg-accent text-white hover:bg-accent/90"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title="Save this file (Ctrl+S)"
                  >
                    <Save size={14} /> {saving ? "Saving..." : "Save"}
                  </button>
                  <button
                    onClick={() => setMdToolsOpen((v) => !v)}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors ${
                      mdToolsOpen
                        ? "bg-bg-hover text-text-primary"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title="Toggle symbols/format toolbar"
                  >
                    Tools
                  </button>
                  <button
                    onClick={() => setMdPreview((v) => !v)}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors ${
                      mdPreview
                        ? "bg-bg-hover text-text-primary"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title={mdPreview ? "Return to editor" : "Preview rendered markdown"}
                  >
                    {mdPreview ? "Editor" : "Preview"}
                  </button>
                  <button
                    onClick={() => setMdFullscreen((v) => !v)}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors ${
                      mdFullscreen
                        ? "bg-bg-hover text-text-primary"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title={mdFullscreen ? "Exit fullscreen" : "Fullscreen editor"}
                  >
                    {mdFullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
                    <span>{mdFullscreen ? "Minimize" : "Fullscreen"}</span>
                  </button>
                  <button
                    onClick={() => setMdSpellCheck((v) => !v)}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors ${
                      mdSpellCheck
                        ? "bg-bg-hover text-text-primary"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title={mdSpellCheck ? "Disable spell check" : "Enable spell check"}
                  >
                    ABC
                  </button>
                  <button
                    onClick={() => setMdShortcutsOpen((v) => !v)}
                    className={`flex items-center gap-1.5 px-3 py-1 text-sm rounded-md transition-colors ${
                      mdShortcutsOpen
                        ? "bg-bg-hover text-text-primary"
                        : "text-text-secondary hover:text-text-primary hover:bg-bg-hover"
                    }`}
                    title="Keyboard shortcuts"
                  >
                    <CircleHelp size={14} />
                  </button>
                  <button
                    onClick={() => setEditing(false)}
                    className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                    title="Return to view"
                  >
                    <Eye size={14} /> View
                  </button>
                </>
              )}

              {viewMode === "browse" && hubMode && (
                <button
                  onClick={handleDownloadDataset}
                  disabled={isLoading}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50"
                  title="Download this dataset to edit locally"
                >
                  <Download size={14} /> Download
                </button>
              )}

              {viewMode === "browse" && !hubMode && (
                <>
                  <button
                    onClick={handleCreateFile}
                    className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                    title="New file in this folder"
                  >
                    <FilePlus size={14} /> New File
                  </button>
                  <button
                    onClick={handleCreateDir}
                    className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                    title="New folder in this folder"
                  >
                    <FolderPlus size={14} /> New Folder
                  </button>
                </>
              )}

              {viewMode === "browse" && canMutateRoots && !hubMode && (
                <button
                  onClick={handleDeleteDataset}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-error-text hover:bg-bg-hover rounded-md transition-colors"
                  title="Delete this dataset (moves to trash)"
                >
                  <Trash2 size={14} /> Delete Dataset
                </button>
              )}

              {viewMode === "datasets" && canMutateRoots && !hubMode && (
                <button
                  onClick={handleCreateDataset}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors"
                  title="New wiki dataset"
                >
                  <Plus size={14} /> New Dataset
                </button>
              )}

              {viewMode !== "file" && (
                <button
                  onClick={viewMode === "browse" ? refreshBrowse : loadLocalDatasets}
                  disabled={viewMode === "browse" ? browseLoading : isLoading}
                  className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors disabled:opacity-50"
                  title="Refresh"
                >
                  <RefreshCw size={14} className={(viewMode === "browse" ? browseLoading : isLoading) ? "animate-spin" : ""} />
                  <span>Refresh</span>
                </button>
              )}
            </div>

            {/* Tools row: revealed by the sub-toolbar's Tools button while
                editing. Mirrors the search-row pattern — a compact strip
                hosting the Symbols / German / Format / Lists button groups
                that operate directly on MdEditor's textarea via the shared
                insert helpers, so this panel now sits alongside the other
                sub-toolbars instead of squeezing in above the textarea. */}
            {editing && mdToolsOpen && (
              <div className="shrink-0 flex flex-wrap gap-x-6 gap-y-3 px-4 py-2 border-b border-border-default bg-bg-card">
                <div>
                  <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Symbols</div>
                  <div className="flex flex-wrap gap-1">
                    {char1.map((v, i) => (
                      <button
                        key={`c1-${i}`}
                        onClick={() => runInsert((ta) => insertAround(ta, v, ""))}
                        className="px-2 py-1 text-base rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
                        title={char1Tips[i]}
                      >
                        {v}
                      </button>
                    ))}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">German</div>
                  <div className="flex flex-wrap gap-1">
                    {char2.map((v, i) => (
                      <button
                        key={`c2-${i}`}
                        onClick={() => runInsert((ta) => insertAround(ta, v, ""))}
                        className="px-2 py-1 text-base rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
                        title={v}
                      >
                        {v}
                      </button>
                    ))}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Format</div>
                  <div className="flex flex-wrap gap-1">
                    {[...char3, ...char4].map((v, i) => (
                      <button
                        key={`c34-${i}`}
                        onClick={() => runInsert((ta) => insertAround(ta, v.start, v.end))}
                        className="px-2 py-1 text-base rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
                        title={v.tip}
                      >
                        {v.label}
                      </button>
                    ))}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Lists</div>
                  <div className="flex flex-wrap gap-1">
                    <button
                      onClick={() => runInsert((ta) => insertAtLineStart(ta, "- "))}
                      className="px-2 py-1 text-sm font-mono rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
                      title="unordered list"
                    >
                      -
                    </button>
                    <button
                      onClick={() => runInsert((ta) => insertAtLineStart(ta, "1. "))}
                      className="px-2 py-1 text-sm font-mono rounded-md text-text-secondary hover:text-text-primary hover:bg-bg-hover transition-colors"
                      title="ordered list"
                    >
                      1.
                    </button>
                  </div>
                </div>
              </div>
            )}

            {/* Search row: revealed by the sub-toolbar's search icon. Hosts the
                WikiSearch input and the Index button (a local operation, so
                hidden on mobile and in hub read-only mode). */}
            {searchOpen && (
              <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-b border-border-default bg-bg-card">
                <div className="flex-1 min-w-0 max-w-xl">
                  <WikiSearch mode={searchMode} onResultClick={handleSearchResult} />
                </div>
                {!mobile && !hubMode && (
                  <button
                    onClick={handleIndexWiki}
                    disabled={isIndexing || !indexableDatasetUuid}
                    className="flex items-center gap-1.5 px-3 py-1 text-sm text-text-secondary hover:text-text-primary hover:bg-bg-hover rounded-md transition-colors disabled:opacity-50 shrink-0"
                    title="Re-index this dataset's wiki files for search"
                  >
                    <RefreshCw size={14} className={isIndexing ? "animate-spin" : ""} />
                    <span>{isIndexing ? "Indexing..." : "Index"}</span>
                  </button>
                )}
              </div>
            )}

            {/* Inline name prompt backing the New dataset/file/folder actions */}
            {prompt && (
              <div className="shrink-0 px-4 py-2 border-b border-border-default bg-bg-body space-y-2">
                <div className="text-xs font-semibold text-text-primary">{prompt.title}</div>
                <input
                  type="text"
                  autoFocus
                  value={promptValue}
                  onChange={(e) => setPromptValue(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void submitPrompt();
                    else if (e.key === "Escape") setPrompt(null);
                  }}
                  className="w-full px-2 py-1 text-sm bg-bg-input border border-border-default rounded-md outline-none focus:border-accent"
                />
                <div className="flex gap-2">
                  <button
                    onClick={() => void submitPrompt()}
                    disabled={!sanitizeName(promptValue) || promptBusy}
                    className="flex-1 px-2 py-1 text-xs bg-accent text-white rounded-md hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                  >
                    {promptBusy ? "…" : "OK"}
                  </button>
                  <button
                    onClick={() => setPrompt(null)}
                    className="flex-1 px-2 py-1 text-xs bg-bg-input border border-border-default rounded-md hover:bg-bg-hover transition-colors"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}

            {/* Scrollable content */}
            <div className="flex-1 overflow-y-auto min-h-0">
              {isLoading ? (
                <div className="flex items-center justify-center h-full">
                  <div className="text-text-tertiary">Loading...</div>
                </div>
              ) : error ? (
                <div className="flex items-center justify-center h-full p-6">
                  <div className="max-w-lg w-full text-center">
                    <div className="text-error-text bg-error-bg px-4 py-2 rounded-lg inline-block mb-4">
                      Error: {error}
                    </div>
                    {selection?.source.kind === "hub-dataset" && (
                      <div>
                        <p className="text-sm text-text-secondary mb-3">
                          This file lives on the hub. Download the dataset to read it locally.
                        </p>
                        <button
                          onClick={() => hubSrc && handleDownload(hubSrc.uuid, hubSrc.rel)}
                          className="inline-flex items-center gap-1.5 px-4 py-2 text-sm bg-accent text-white rounded-md hover:bg-accent/90"
                        >
                          <Download size={14} /> Download dataset
                        </button>
                      </div>
                    )}
                  </div>
                </div>
              ) : selection && selection.source.rel === "" ? null
                : selection ? (
                <div className="h-full flex flex-col min-h-0">
                  {/* Body: editor (edit mode) or rendered markdown (view mode) */}
                  <div className={inEditorFullscreen ? "flex-1 flex flex-col min-h-0" : "flex-1 overflow-y-auto min-h-0"}>
                    {editing ? (
                      <div className={inEditorFullscreen ? "flex-1 flex flex-col min-h-0 px-4 py-3" : "px-4 py-3"}>
                        <MdEditor
                          ref={mdTextareaRef}
                          value={editContent}
                          onChange={setEditContent}
                          preview={mdPreview}
                          fullscreen={mdFullscreen}
                          spellCheck={mdSpellCheck}
                          shortcutsOpen={mdShortcutsOpen}
                        />
                      </div>
                    ) : (
                      <div className="px-6 pt-6 pb-[50vh]">
                        <MarkdownViewer content={fileContent} withTOC={true} onWikiLink={handleWikiLink} />
                      </div>
                    )}
                  </div>
                </div>
              ) : datasetBrowse ? (
                <div className="p-4 flex flex-col gap-1">
                  {browseLoading ? (
                    <div className="text-text-tertiary">Loading...</div>
                  ) : browseError ? (
                    <div className="text-error-text bg-error-bg px-4 py-2 rounded-lg text-sm">{browseError}</div>
                  ) : !browseEntries || browseEntries.length === 0 ? (
                    <p className="text-text-secondary">This folder is empty.</p>
                  ) : (
                    browseEntries.map((entry) => (
                      <button
                        key={entry.rel_path}
                        className="flex items-center gap-2 px-3 py-2 rounded-md text-sm text-left text-text-primary transition-colors hover:bg-bg-hover cursor-pointer"
                        onClick={() => (entry.is_dir ? goToDatasetDir(datasetBrowse.uuid, entry.rel_path) : openBrowseFile(entry))}
                        title={entry.rel_path}
                      >
                        {entry.is_dir ? (
                          <Folder size={16} className="shrink-0 text-accent" />
                        ) : (
                          <FileText size={16} className="shrink-0 text-text-tertiary" />
                        )}
                        <span className="truncate flex-1">{entry.name}</span>
                      </button>
                    ))
                  )}
                </div>
              ) : (
                <div className="p-4 flex flex-col gap-2">
                  {treeDatasets.length === 0 ? (
                    <p className="text-text-secondary">
                      {hubMode
                        ? "The hub has no wiki datasets."
                        : "No wiki datasets yet. Create one with the New Dataset button, or download one from the hub."}
                    </p>
                  ) : (
                    treeDatasets.map((ds) => (
                      <button
                        key={ds.uuid}
                        className="text-left p-4 border border-border-default rounded-lg transition-colors hover:bg-bg-hover cursor-pointer"
                        onClick={() => goToDatasetDir(ds.uuid, "")}
                        title={ds.path}
                      >
                        <div className="flex items-center gap-2">
                          <BookMarked size={16} className="shrink-0 text-purple-500" />
                          <span className="font-medium text-text-primary truncate">{ds.name}</span>
                          <span className="ml-auto text-xs text-text-tertiary shrink-0">{ds.file_count}</span>
                        </div>
                        <div className="text-xs text-text-tertiary truncate mt-1" title={ds.path}>{ds.path}</div>
                      </button>
                    ))
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
