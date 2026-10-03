"use client";

/**
 * DictationPage — uses useDictationData for all state/logic,
 * this file only handles rendering.
 */

import { useState, useEffect, useMemo, useSyncExternalStore } from "react";

import { ProgressCircle, Select, ListBox, Label, Button, Tooltip } from "@heroui/react";
import { RefreshCw, Trash2, Database, List, Target, CheckCircle, FolderPlus, Folder, Link2, Pencil, Save, HelpCircle } from "lucide-react";
import CueEditor from "./components/CueEditor";
import WaveformCanvas from "./components/WaveformCanvas";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import type { Cue } from "@/lib/types";
import { useDictationData } from "@/hooks/useDictationData";
import { isAudio } from "@/lib/listen/utils";
import { datasetStatusLabel, datasetStatusBadgeClasses, type DatasetStatus } from "@/lib/datasets/types";
import { subscribe, getVoiceError, clearVoiceError } from "@/lib/voice-input";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logInfo, logError } from "@/lib/logger";

const getUUID = () => crypto.randomUUID().replaceAll("-", "");

export default function DictationPage() {
    const d = useDictationData();
    const [adminMode, setAdminMode] = useState(false);
    const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);
    // Media-list view/edit mode toggle (view mode hides players, edit mode shows source/note fields).
    const [mediaEditMode, setMediaEditMode] = useState(false);
    // Keyboard-shortcut help tip: shown on hover, and also on click (controlled).
    const [helpOpen, setHelpOpen] = useState(false);
    const voiceError = useSyncExternalStore(subscribe, getVoiceError, getVoiceError);
    // Guards against double-submits while a favorite clip is being cut.
    const [addingFavorite, setAddingFavorite] = useState(false);

    // Mobile thin client: surface how many local edits await upload.
    const [mobile, setMobile] = useState(false);
    const [pendingUpload, setPendingUpload] = useState(0);
    useEffect(() => {
        setMobile(isMobileApp());
    }, []);
    useEffect(() => {
        if (!isTauri() || !mobile) return;
        let alive = true;
        const refresh = () =>
            invoke<number>("writeback_pending_count")
                .then((n) => alive && setPendingUpload(n))
                .catch(() => {});
        refresh();
        const timer = setInterval(refresh, 5000);
        return () => {
            alive = false;
            clearInterval(timer);
        };
    }, [mobile]);

    // Group ready datasets by their root location for the datasets view.
    type DatasetItem = (typeof d.datasets)[number];
    const datasetsByLocation = useMemo(() => {
        const map = new Map<string, DatasetItem[]>();
        for (const ds of d.datasets) {
            const key = ds.location || "";
            const arr = map.get(key);
            if (arr) arr.push(ds);
            else map.set(key, [ds]);
        }
        return map;
    }, [d.datasets]);
    // Datasets whose location is missing from the dir list (e.g. list fetch failed).
    const orphanLocations = useMemo(
        () => Array.from(datasetsByLocation.keys()).filter((k) => k && !d.locations.some((l) => l.path === k)),
        [datasetsByLocation, d.locations]
    );
    const readyCount = useMemo(() => d.datasets.filter((ds) => ds.status === "ready").length, [d.datasets]);

    // Dataset card with a Ready / Not Ready mark; not-ready datasets cannot be
    // selected until their database is generated on the Datasets page.
    const renderDatasetCard = (ds: DatasetItem) => {
        const ready = ds.status === "ready";
        const status = ds.status as DatasetStatus;
        return (
            <button
                key={ds.path}
                className={`text-left p-4 border border-border-default rounded-lg transition-colors ${ready ? "hover:bg-bg-hover cursor-pointer" : "opacity-70 cursor-not-allowed"}`}
                disabled={!ready}
                onClick={() => ready && d.setSelectedDatasetUuid(ds.info.uuid)}
                title={ready ? undefined : "Not ready — generate the database in Datasets > Studio first"}
            >
                <div className="flex items-center gap-2">
                    <span className="font-medium text-text-primary">{ds.info.name}</span>
                    <span className={datasetStatusBadgeClasses(status)}>{datasetStatusLabel(status)}</span>
                </div>
                <div className="text-xs text-text-tertiary truncate" title={ds.path}>{ds.path}</div>
            </button>
        );
    };

    const selectedMedia = d.mediaList.find((m) => m.uuid === d.stateMediaUUID);
    // Media no longer has an editable title; use the source filename as its label.
    const selectedMediaLabel = selectedMedia?.source || d.stateMedia.source || "...";
    // The Favorites dataset is where cue clips are cut INTO; adding a cue from it
    // back to itself is meaningless, so the button is disabled while viewing it.
    const inFavoritesDataset = !!d.selectedDataset?.info.is_favorites;

    useEffect(() => {
        if (voiceError) {
            setConfirmReq({ title: "Voice input error", message: voiceError });
            clearVoiceError();
        }
    }, [voiceError]);

    // Cut the cue's audio into a WAV clip and add it to the Favorites dataset.
    const handleAddToFavorites = async (cue: Cue) => {
        if (!isTauri() || addingFavorite) return;
        if (!d.selectedDatasetUuid || !d.stateMediaUUID) {
            setConfirmReq({ title: "Add to favorites", message: "Select a dataset and media first." });
            return;
        }
        setAddingFavorite(true);
        try {
            const res = await invoke<{ status?: string; duration_ms?: number }>("dictation_add_cue_to_favorites", {
                datasetUuid: d.selectedDatasetUuid,
                mediaUuid: d.stateMediaUUID,
                cueUuid: cue.uuid,
            });
            // Refresh the favorited-cue set so the Star fills in immediately.
            d.reloadFavorites();
            if (res?.status === "duplicate") {
                logInfo(`Cue ${cue.uuid} already in favorites; skipped duplicate`, "dictation");
                setConfirmReq({ title: "Already in Favorites", message: "This cue is already in your Favorites dataset." });
                return;
            }
            const secs = Math.round((res?.duration_ms ?? 0) / 100) / 10;
            logInfo(`Added cue ${cue.uuid} to favorites (${res?.duration_ms ?? 0}ms clip)`, "dictation");
            d.loadDatasets();
            setConfirmReq({ title: "Added to favorites", message: `Clip (${secs}s) added to your Favorites dataset.` });
        } catch (e) {
            const msg = String(e);
            logError(`Failed to add cue to favorites: ${msg}`, "dictation");
            setConfirmReq({ title: "Add to favorites failed", message: msg });
        } finally {
            setAddingFavorite(false);
        }
    };

    // ── Arrow key navigation for cues ──
    useEffect(() => {
        const handleKeyDown = (e: KeyboardEvent) => {
            // Only handle Ctrl/Cmd + arrow keys
            if (!e.ctrlKey && !e.metaKey) return;
            if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
            
            // Don't interfere with contenteditable (rich text editing)
            // Allow in inputs and textareas since Ctrl+Arrow is intentional
            const activeEl = document.activeElement;
            if (activeEl && activeEl.getAttribute("contenteditable") === "true") {
                return;
            }
            
            // Must have cues with media selected
            if (d.stateCues.length === 0 || !d.stateMediaUUID) return;
            
            e.preventDefault();
            
            // Both modes render all cues; navigate by focused-cue uuid and scroll into view.
            const currentIndex = d.stateFocusedCueUUID ? d.stateCues.findIndex(c => c.uuid === d.stateFocusedCueUUID) : -1;
            let newIndex = currentIndex;

            if (e.key === "ArrowUp") {
                newIndex = currentIndex > 0 ? currentIndex - 1 : d.stateCues.length - 1;
            } else if (e.key === "ArrowDown") {
                newIndex = currentIndex < d.stateCues.length - 1 ? currentIndex + 1 : 0;
            }

            if (newIndex >= 0 && newIndex < d.stateCues.length) {
                const newCue = d.stateCues[newIndex];
                d.setStateFocusedCueUUID(newCue.uuid);
                // Focus the input field of the new cue
                setTimeout(() => {
                    const inputEl = document.getElementById(`d-s-i-${newCue.uuid}`) as HTMLInputElement | HTMLTextAreaElement;
                    if (inputEl) inputEl.focus();
                }, 0);
                // Scroll the cue into view
                const cueElements = document.querySelectorAll('[data-cue-index]');
                const targetElement = cueElements[newIndex] as HTMLElement;
                if (targetElement) {
                    targetElement.scrollIntoView({ behavior: "smooth", block: "nearest" });
                }
            }
        };
        
        window.addEventListener("keydown", handleKeyDown);
        return () => window.removeEventListener("keydown", handleKeyDown);
    }, [d.stateCues, d.stateFocusedCueUUID, d.stateMediaUUID]);

    // ── Media playback shortcut ──
    // Ctrl/Cmd + S toggles playback of the current media.
    // Ctrl/Cmd + D clears the dictation input (handled in CueEditor).
    // (Voice input is Ctrl/Cmd + C, handled globally in lib/voice-input; prev/next
    // cue is Ctrl/Cmd + ↑/↓, handled by the navigation effect above.)
    useEffect(() => {
        const handleMediaKeys = (e: KeyboardEvent) => {
            if (!e.ctrlKey && !e.metaKey) return;
            const key = e.key.toLowerCase();
            if (key !== "s") return;
            const media = d.videoRef.current;
            if (!d.hasMedia || !media) return;
            e.preventDefault();
            if (media.paused) void media.play(); else media.pause();
        };
        window.addEventListener("keydown", handleMediaKeys);
        return () => window.removeEventListener("keydown", handleMediaKeys);
    }, [d.videoRef, d.hasMedia]);

    return (
        <div className="flex flex-col flex-1 min-h-0 p-4 overflow-hidden">
            {mobile && pendingUpload > 0 && (
                <div className="mb-3 px-3 py-1.5 rounded-md text-xs font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300 w-fit">
                    {pendingUpload} change(s) pending upload
                </div>
            )}
            {/* Toolbar — sections are mutually exclusive: Location on the datasets view,
                Dataset on the media-list view, Dictation on the cue view. */}
            <div className="@container flex flex-row items-center gap-3 w-full px-3 py-2 mb-4 rounded-lg bg-bg-card border border-border-light">
                {/* ── Location section (datasets view) ── */}
                {!d.selectedDatasetUuid && (
                    <div className="flex items-center gap-1">
                        <Button variant="ghost" size="sm" aria-label="Refresh locations" isDisabled={d.stateLoading} onPress={d.loadLocations}>
                            <RefreshCw size={16} /> Refresh
                        </Button>
                        <Button variant="ghost" size="sm" aria-label="Add location" isDisabled={d.stateLoading} onPress={d.handleAddLocation}>
                            <FolderPlus size={16} /> Add Location
                        </Button>
                    </div>
                )}

                {/* ── Dataset section (media list view) ── */}
                {d.selectedDatasetUuid && !d.stateMediaUUID && (
                    <div className="flex items-center gap-1">
                        <Button variant="ghost" size="sm" aria-label="Reload database" isDisabled={!d.selectedDatasetUuid || d.stateLoading} onPress={d.handleReload}>
                            <Database size={16} /> Reload
                        </Button>
                        <Button size="sm" variant={mediaEditMode ? "primary" : "ghost"} aria-label="Toggle edit mode" isDisabled={!d.selectedDatasetUuid} onPress={() => setMediaEditMode((v) => !v)}>
                            <Pencil size={16} /> {mediaEditMode ? "View" : "Edit"}
                        </Button>
                    </div>
                )}

                {/* ── Dictation section (cue view) ── */}
                {!!d.stateMediaUUID && (
                    <div className="flex items-center gap-1">
                        <Button size="sm" variant={adminMode ? "primary" : "ghost"} aria-label="Toggle detailed mode" isDisabled={d.stateCues.length === 0} onPress={() => setAdminMode(!adminMode)}>
                            <List size={16} /> Detailed
                        </Button>
                        <Button size="sm" variant={d.stateDictMode === "large" ? "primary" : "ghost"} aria-label="Toggle large mode" isDisabled={d.stateCues.length === 0} onPress={() => {
                            if (d.stateDictMode === "large") { d.setStateDictMode("full"); }
                            else {
                                // Jump focus to the first incomplete cue when entering large mode.
                                const cueList = d.stateCues.filter((cue) => !d.stateDictSuccessSet.has(cue.uuid));
                                if (cueList.length > 0) d.setStateFocusedCueUUID(cueList[0].uuid);
                                d.setStateDictMode("large");
                            }
                        }}>
                            <Target size={16} /> Large
                        </Button>
                        <Button size="sm" variant={d.stateDictStatus === "complete" ? "primary" : "ghost"} aria-label="Mark complete" isDisabled={d.stateCues.length === 0} onPress={d.handleDictStatusToggle}>
                            <CheckCircle size={16} /> Complete
                        </Button>
                        <Tooltip isOpen={helpOpen} onOpenChange={setHelpOpen}>
                            <Tooltip.Trigger>
                                <Button size="sm" variant="ghost" aria-label="Keyboard shortcuts" onPress={() => setHelpOpen(true)}>
                                    <HelpCircle size={16} /> Help
                                </Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content>
                                <div className="flex flex-col gap-0.5">
                                    <span>Play Audio: Ctrl+s or double space at the end</span>
                                    <span>Clear Input: Ctrl+d</span>
                                    <span>Voice Input: Ctrl+c</span>
                                    <span>Go to Previous/Next: Ctrl+⬆/⬇</span>
                                    <span>Show Content/Reference: Ctrl+⬅/➡</span>
                                </div>
                            </Tooltip.Content>
                        </Tooltip>
                    </div>
                )}
            </div>

            {/* Breadcrumb navigation */}
            <nav className="flex flex-row items-center gap-2 mb-4 text-sm select-none min-w-0">
                <button
                    className={`shrink-0 cursor-pointer hover:underline ${d.selectedDatasetUuid ? "text-accent" : "text-text-primary font-medium"}`}
                    onClick={() => d.setSelectedDatasetUuid("")}
                >
                    Datasets
                </button>
                {d.selectedDataset && (
                    <>
                        <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                        <button
                            className={`cursor-pointer hover:underline truncate min-w-0 max-w-[240px] ${d.stateMediaUUID ? "text-accent" : "text-text-primary font-medium"}`}
                            onClick={() => { d.setStateMediaUUID(""); d.handleReload(); }}
                            title={d.selectedDataset.info.name}
                        >
                            {d.selectedDataset.info.name}
                        </button>
                    </>
                )}
                {d.selectedDataset && d.stateMediaUUID && (
                    <>
                        <span className="shrink-0 text-text-tertiary">&rsaquo;</span>
                        {d.stateSubtitle ? (
                            <span
                                className="text-text-primary font-medium truncate min-w-0 max-w-[320px]"
                                title={`${selectedMediaLabel} — ${d.stateSubtitle.name || d.stateSubtitle.uuid}`}
                            >
                                {selectedMediaLabel} <span className="text-text-tertiary font-normal">&rsaquo;</span> {d.stateSubtitle.name || d.stateSubtitle.uuid}
                            </span>
                        ) : (
                            <span
                                className="text-text-primary font-medium truncate min-w-0 max-w-[320px]"
                                title={selectedMediaLabel}
                            >
                                {selectedMediaLabel}
                            </span>
                        )}
                    </>
                )}
                {d.stateLoading && <ProgressCircle size="sm" aria-label="Loading" />}
            </nav>

            {/* Datasets view (initial) — grouped by location */}
            {!d.selectedDatasetUuid && (
                <div className="flex flex-col gap-4 flex-1 min-h-0 overflow-y-auto">
                    {/* Location summary */}
                    <div className="flex items-center justify-between gap-2">
                        <span className="text-xs text-text-tertiary">
                            {d.locations.length} location{d.locations.length !== 1 ? "s" : ""} · {readyCount} ready / {d.datasets.length} dataset{d.datasets.length !== 1 ? "s" : ""}
                        </span>
                    </div>

                    {d.locationError && (
                        <div className="p-3 bg-error-bg text-error-text rounded-md text-sm">{d.locationError}</div>
                    )}

                    {d.locations.length === 0 && d.datasets.length === 0 ? (
                        <p className="text-text-secondary">No datasets available. Use the &quot;Add dataset location&quot; button in the toolbar to link a directory containing datasets, or generate a database in Datasets &gt; Studio first.</p>
                    ) : (
                        <>
                            {d.locations.map((loc) => {
                                const items = datasetsByLocation.get(loc.path) ?? [];
                                return (
                                    <section key={loc.path} className="flex flex-col gap-2">
                                        {/* Location header */}
                                        <div className="flex items-center gap-2 px-1">
                                            {loc.is_linked ? (
                                                <Link2 size={14} className="text-accent shrink-0" />
                                            ) : (
                                                <Folder size={14} className="text-text-tertiary shrink-0" />
                                            )}
                                            <span className="text-sm font-semibold text-text-primary shrink-0">{loc.name}</span>
                                            <span className="text-xs text-text-tertiary truncate flex-1" title={loc.path}>{loc.path}</span>
                                            <span className="text-xs text-text-tertiary shrink-0">{items.length}</span>
                                            {loc.is_linked && (
                                                <Button
                                                    variant="ghost"
                                                    size="sm"
                                                    aria-label="Delete location"
                                                    className="text-text-tertiary hover:text-error-text"
                                                    onPress={() => setConfirmReq({
                                                        title: "Delete location",
                                                        message: `Delete "${loc.name}" from dataset locations? Its datasets will no longer be listed here, but files on disk are untouched.`,
                                                        confirmLabel: "Delete",
                                                        onConfirm: () => d.handleRemoveLocation(loc.path),
                                                    })}
                                                >
                                                    <Trash2 size={14} /> Delete Location
                                                </Button>
                                            )}
                                        </div>
                                        {/* Datasets under this location */}
                                        {items.length === 0 ? (
                                            <p className="text-xs text-text-tertiary px-1">No datasets in this location.</p>
                                        ) : items.map(renderDatasetCard)}
                                    </section>
                                );
                            })}

                            {/* Datasets from locations missing in the dir list (safety net) */}
                            {orphanLocations.map((locPath) => (
                                <section key={locPath} className="flex flex-col gap-2">
                                    <div className="flex items-center gap-2 px-1">
                                        <Folder size={14} className="text-text-tertiary shrink-0" />
                                        <span className="text-xs text-text-tertiary truncate flex-1" title={locPath}>{locPath}</span>
                                    </div>
                                    {(datasetsByLocation.get(locPath) ?? []).map(renderDatasetCard)}
                                </section>
                            ))}
                        </>
                    )}
                </div>
            )}

            {/* Media list view */}
            {d.selectedDatasetUuid && !d.stateMediaUUID && (
                <div className="flex flex-col gap-3 flex-1 min-h-0 overflow-y-auto">
                    {/* Edit-mode header: media count + Save all */}
                    {mediaEditMode && d.mediaList.length > 0 && (
                        <div className="flex items-center gap-2">
                            <span className="text-sm text-text-secondary">{d.mediaList.length} media</span>
                            {d.mediaDirtyCount > 0 && (
                                <Button className="ml-auto" size="sm" variant="primary" isDisabled={d.stateSaving} onPress={d.handleSaveAllMedia}>
                                    <Save size={14} /> Save all ({d.mediaDirtyCount})
                                </Button>
                            )}
                        </div>
                    )}
                    {d.mediaList.length === 0 && !d.stateLoading ? (
                        <p className="text-text-secondary">No media in this dataset.</p>
                    ) : d.mediaList.map((m) => {
                        const isCompleted = d.completedMediaUuids.has(m.uuid);
                        const dirty = mediaEditMode && d.isMediaDirty(m);
                        const source = mediaEditMode ? d.mediaFieldOf(m, "source") : m.source;
                        const src = mediaEditMode ? d.getMediaSrc(source) : "";
                        return (
                            <div
                                key={m.uuid}
                                className={`p-4 border rounded-lg flex flex-col gap-2 ${dirty ? "border-accent bg-accent-bg/10" : isCompleted ? "bg-success-bg/50 border-accent" : "border-border-default"}`}
                            >
                                <span className="text-left font-medium text-text-primary">
                                    {m.source}
                                </span>

                                {/* Subtitle links — click to jump into that subtitle's dictation */}
                                {(d.mediaSubtitles[m.uuid]?.length ?? 0) > 0 && (
                                    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                                        <span className="text-xs text-text-tertiary">Subtitles:</span>
                                        {d.mediaSubtitles[m.uuid].map((s) => (
                                            <button
                                                key={s.uuid}
                                                className="inline-flex items-center gap-1 text-xs hover:underline cursor-pointer"
                                                title={`Open dictation for "${s.name || s.uuid}"`}
                                                onClick={() => d.selectMediaSubtitle(m.uuid, s.uuid)}
                                            >
                                                <span className="text-accent">{s.name || s.uuid}</span>
                                                <span className="text-text-tertiary font-mono">(uuid: {s.uuid})</span>
                                            </button>
                                        ))}
                                    </div>
                                )}

                                {mediaEditMode && (
                                    <>
                                        {/* Source / note edit fields + row actions */}
                                        <div className="grid grid-cols-[1fr_1fr_auto] gap-3 items-center">
                                            <label className="flex items-center gap-2">
                                                <span className="shrink-0 text-xs text-text-tertiary w-12">Source</span>
                                                <input
                                                    className="w-full px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                                                    value={source}
                                                    placeholder="(source)"
                                                    disabled={d.stateSaving}
                                                    onChange={(e) => d.setMediaField(m, "source", e.target.value)}
                                                />
                                            </label>
                                            <label className="flex items-center gap-2">
                                                <span className="shrink-0 text-xs text-text-tertiary w-12">Note</span>
                                                <input
                                                    className="w-full px-2 py-1 text-sm rounded-md bg-bg-body border border-border-light text-text-primary focus:border-accent outline-none"
                                                    value={d.mediaFieldOf(m, "note")}
                                                    placeholder="—"
                                                    disabled={d.stateSaving}
                                                    onChange={(e) => d.setMediaField(m, "note", e.target.value)}
                                                />
                                            </label>
                                            <div className="flex flex-row items-center justify-end gap-1">
                                                <Tooltip>
                                                    <Tooltip.Trigger>
                                                        <Button isIconOnly size="sm" variant="ghost" aria-label="Remove media" className="text-error-text" isDisabled={d.stateSaving} onPress={() => setConfirmReq({
                                                            title: "Remove media",
                                                            message: `Remove "${m.source || "this media"}" and all related data and files? This cannot be undone.`,
                                                            confirmLabel: "Remove",
                                                            onConfirm: () => d.handleDeleteMediaRow(m),
                                                        })}>
                                                            <Trash2 size={14} />
                                                        </Button>
                                                    </Tooltip.Trigger>
                                                    <Tooltip.Content>Remove media and all related data</Tooltip.Content>
                                                </Tooltip>
                                            </div>
                                        </div>
                                        {src && (isAudio(source)
                                            ? <audio controls preload="none" src={src} className="w-full" />
                                            : <video controls preload="none" src={src} className="w-full max-h-64" />)}
                                    </>
                                )}
                            </div>
                        );
                    })}
                </div>
            )}

            {/* Main content area */}
            {d.stateMediaUUID && (
                <div className="flex flex-col gap-3 flex-1 min-h-0 overflow-hidden">
                    {/* Player */}
                    <div className="flex flex-col gap-1 shrink-0 w-full sticky top-0 z-10 bg-bg-body">
                        {d.hasMedia && (
                            d.audioMode ? (
                                <audio ref={d.videoRef as React.RefObject<HTMLAudioElement>} className="w-full" controls src={d.audioSrc} />
                            ) : (
                                <div className="rounded-xl overflow-hidden shadow-lg bg-black">
                                    <video ref={d.videoRef} className="w-full" controls src={d.audioSrc} />
                                </div>
                            )
                        )}

                        {/* Waveform */}
                        {d.stateWaveformPeaks && (
                            <div className="bg-bg-muted border-t border-border-light p-1 shadow-lg rounded-lg">
                                <WaveformCanvas
                                    peaks={d.stateWaveformPeaks}
                                    videoRef={d.videoRef}
                                    selection={(() => {
                                        const cue = d.stateFocusedCueUUID !== null ? d.stateCues.find(c => c.uuid === d.stateFocusedCueUUID) : undefined;
                                        return cue ? { start: cue.start_ms / 1000, end: cue.end_ms / 1000 } : undefined;
                                    })()}
                                />
                            </div>
                        )}
                    </div>

                    {/* Dictation */}
                    <div className="flex flex-col w-full gap-3 flex-1 min-h-0 overflow-hidden">
                            {/* ── Fixed header ── */}
                            <div className="shrink-0 flex flex-col gap-3">
                                {d.stateSubtitleList.length > 1 && (
                                    <Select value={d.stateSubtitle?.uuid ?? null} onChange={(v) => d.setStateSubtitle(d.stateSubtitleList.find((s) => s.uuid === String(v ?? "")))}>
                                        <Label>Select subtitle</Label>
                                        <Select.Trigger><Select.Value /><Select.Indicator /></Select.Trigger>
                                        <Select.Popover>
                                            <ListBox>
                                                {d.stateSubtitleList.map((v) => (
                                                    <ListBox.Item id={v.uuid} key={v.uuid} textValue={v.name || v.uuid}>{v.name || v.uuid}</ListBox.Item>
                                                ))}
                                            </ListBox>
                                        </Select.Popover>
                                    </Select>
                                )}

                                {d.stateCues.length > 0 && (
                                    <div className="flex items-center px-1">
                                        <span className="flex-1 text-sm text-foreground-500">{d.stateDictSuccessSet.size} / {d.stateCues.length} ✓</span>
                                    </div>
                                )}
                            </div>

                            {/* ── Scrollable cue cards ── */}
                            <div className="flex-1 min-h-0 overflow-y-auto flex flex-col gap-3 pb-48">
                                {d.stateNeedSave && (
                                    <div className="flex flex-row items-end justify-end fixed bottom-10 end-10 p-4 z-10 gap-2">
                                        <Button size="lg" variant="danger" onPress={() => {
                                            d.updateStateCues((draft) => { draft.forEach((cue) => { cue.content = cue.content_original ?? cue.content; cue.modified = false; cue.deleted = false; }); });
                                            d.setStateNeedSave(false);
                                        }}>Discard</Button>
                                        <Button size="lg" isDisabled={d.stateSaving} variant="danger" onPress={d.handleSaveSubtitle}>Save</Button>
                                    </div>
                                )}

                                {d.stateCues.map((cue, i) => (
                                    <div key={i} data-cue-index={i} className={`rounded-xl border-2 py-1.5 px-2 transition-colors ${d.stateFocusedCueUUID === cue.uuid ? "border-accent" : "border-border-light"} ${cue.deleted ? "bg-error-bg" : d.stateFocusedCueUUID === cue.uuid ? "bg-accent-bg/40" : cue.modified ? "bg-accent-bg/20" : "bg-bg-body"}`}>
                                        <CueEditor
                                            cue={cue}
                                            media={d.videoRef.current}
                                            allowEdit={true}
                                            mode={
                                                d.stateEditingCue === cue.uuid ? "dictation_edit"
                                                : d.stateDictMode === "large" && d.stateFocusedCueUUID === cue.uuid ? "dictation_large"
                                                : "dictation"
                                            }
                                            isDisabled={d.stateSaving}
                                            adminMode={adminMode}
                                            onUpdate={(updated) => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === updated.uuid); if (idx !== -1) { draft[idx] = { ...updated, content_original: draft[idx].content_original }; if (updated.modified) d.setStateNeedSave(true); } })}
                                            onExpandStart={() => d.handleExpandStart(cue)}
                                            onExpandEnd={() => d.handleExpandEnd(cue)}
                                            onDelete={() => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === cue.uuid); if (idx !== -1) draft[idx].deleted = true; let n = 1; draft.forEach((item) => { if (!item.deleted) { item.order_num = n++; item.modified = true; } }); d.setStateNeedSave(true); })}
                                            onMergeNext={() => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === cue.uuid); if (idx >= 0 && idx < draft.length - 1) { draft[idx].content += " " + draft[idx + 1].content; draft[idx].end_ms = draft[idx + 1].end_ms; draft[idx + 1].deleted = true; let n = 1; draft.forEach((item) => { if (!item.deleted) { item.order_num = n++; item.modified = true; } }); } d.setStateNeedSave(true); })}
                                            onInsert={(pos: number) => d.updateStateCues((draft) => { const newItem: Cue = { uuid: getUUID(), subtitle_uuid: d.stateSubtitle!.uuid, order_num: 0, start_ms: 0, end_ms: 0, content: "", reference: null }; if (pos < 1) draft.unshift(newItem); else if (pos > draft.length) draft.push(newItem); else draft.splice(pos - 1, 0, newItem); let n = 1; draft.forEach((item) => { if (!item.deleted) { item.order_num = n++; item.modified = true; } }); d.setStateNeedSave(true); })}
                                            onEdit={() => d.setStateEditingCue(cue.uuid)}
                                            onDone={() => d.setStateEditingCue(null)}
                                            initialSuccess={d.stateDictSuccessSet.has(cue.uuid)}
                                            onSuccess={d.handleDictSuccess}
                                            onFocusInput={() => d.setStateFocusedCueUUID(cue.uuid)}
                                            onAddToFavorites={() => handleAddToFavorites(cue)}
                                            favoritesDisabled={inFavoritesDataset}
                                            isFavorited={d.favoriteCueUuids.has(cue.uuid)}
                                        />
                                    </div>
                                ))}
                            </div>
                    </div>
                </div>
            )}

            <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
        </div>
    );
}
