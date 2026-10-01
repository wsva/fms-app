"use client";

/**
 * DictationPage — uses useDictationData for all state/logic,
 * this file only handles rendering.
 */

import { useState, useEffect, useMemo, useSyncExternalStore } from "react";

import { ProgressCircle, Input, Select, Tabs, ListBox, Label, TextField, Separator, Button, Tooltip } from "@heroui/react";
import { RefreshCw, Trash2, Database, Shield, Target, CheckCircle, FolderPlus, Folder, Link2, X } from "lucide-react";
import CueEditor from "./components/CueEditor";
import SubtitleItem from "./components/Subtitle";
import WaveformCanvas from "./components/WaveformCanvas";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import type { Cue } from "@/lib/types";
import { useDictationData } from "@/hooks/useDictationData";
import { isAudio } from "@/lib/listen/utils";
import { datasetStatusLabel, datasetStatusBadgeClasses, type DatasetStatus } from "@/lib/datasets/types";
import { subscribe, getVoiceError, clearVoiceError } from "@/lib/voice-input";

const getUUID = () => crypto.randomUUID().replaceAll("-", "");

export default function DictationPage() {
    const d = useDictationData();
    const [adminMode, setAdminMode] = useState(false);
    const [confirmReq, setConfirmReq] = useState<ConfirmRequest | null>(null);
    const voiceError = useSyncExternalStore(subscribe, getVoiceError, getVoiceError);

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
    const selectedMediaTitle = selectedMedia?.title || d.stateMedia.title || "...";

    useEffect(() => {
        if (voiceError) {
            setConfirmReq({ title: "Voice input error", message: voiceError });
            clearVoiceError();
        }
    }, [voiceError]);

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
            
            // Must have cues and be on dictation tab with media selected
            if (d.stateCues.length === 0 || !d.stateMediaUUID || d.stateActiveTab !== "dictation") return;
            
            e.preventDefault();
            
            if (d.stateDictMode === "focus") {
                // Focus mode: navigate between cues
                const currentDictCue = d.stateDictCue;
                const currentIndex = currentDictCue ? d.stateCues.findIndex(c => c.uuid === currentDictCue.uuid) : -1;
                let newIndex = currentIndex;
                
                if (e.key === "ArrowUp") {
                    newIndex = currentIndex > 0 ? currentIndex - 1 : d.stateCues.length - 1;
                } else if (e.key === "ArrowDown") {
                    newIndex = currentIndex < d.stateCues.length - 1 ? currentIndex + 1 : 0;
                }
                
                if (newIndex >= 0 && newIndex < d.stateCues.length) {
                    const newCue = d.stateCues[newIndex];
                    d.setStateDictCue(newCue);
                    // Focus the input field of the new cue
                    setTimeout(() => {
                        const inputEl = document.getElementById(`d-s-i-${newCue.uuid}`) as HTMLInputElement | HTMLTextAreaElement;
                        if (inputEl) inputEl.focus();
                    }, 0);
                }
            } else {
                // Full mode: navigate and focus on cue
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
            }
        };
        
        window.addEventListener("keydown", handleKeyDown);
        return () => window.removeEventListener("keydown", handleKeyDown);
    }, [d.stateCues, d.stateDictCue, d.stateFocusedCueUUID, d.stateDictMode, d.stateMediaUUID, d.stateActiveTab]);

    return (
        <div className="flex flex-col flex-1 min-h-0 p-4 overflow-hidden">
            {/* Toolbar */}
            <div className="@container flex flex-row items-center gap-3 w-full px-3 py-2 mb-4 rounded-lg bg-bg-card border border-border-light">
                {/* ── Dataset section ── */}
                <div className="flex items-center gap-1">
                    <span className="select-none @max-lg:hidden text-xs font-medium text-text-tertiary mr-1">Dataset</span>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly variant="ghost" size="sm" aria-label="Refresh datasets" isDisabled={d.stateLoading} onPress={d.loadDatasets}>
                                <RefreshCw size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>Refresh datasets</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly variant="ghost" size="sm" aria-label="Add location" isDisabled={d.stateLoading} onPress={d.handleAddLocation}>
                                <FolderPlus size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>Add dataset location</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly variant="ghost" size="sm" aria-label="Reload database" isDisabled={!d.selectedDatasetUuid || d.stateLoading} onPress={d.handleReload}>
                                <Database size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>Reload dataset database</Tooltip.Content>
                    </Tooltip>
                </div>

                {/* split marker */}
                <div className="h-6 w-px bg-border-light" />

                {/* ── Media section ── */}
                <div className="flex items-center gap-1">
                    <span className="select-none @max-lg:hidden text-xs font-medium text-text-tertiary mr-1">Media</span>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly variant="ghost" size="sm" aria-label="Remove media" className="text-error-text" isDisabled={!d.stateMediaUUID || d.stateSaving || d.stateLoading} onPress={() => setConfirmReq({
                                title: "Remove media",
                                message: `Remove "${d.stateMedia.title || "this media"}" and all related data and files? This cannot be undone.`,
                                confirmLabel: "Remove",
                                onConfirm: d.handleDeleteMedia,
                            })}>
                                <Trash2 size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>Remove media and all related data</Tooltip.Content>
                    </Tooltip>
                </div>

                {/* split marker */}
                <div className="h-6 w-px bg-border-light" />

                {/* ── Dictation section ── */}
                <div className="flex items-center gap-1">
                    <span className="select-none @max-lg:hidden text-xs font-medium text-text-tertiary mr-1">Dictation</span>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly size="sm" variant={adminMode ? "primary" : "ghost"} aria-label="Toggle admin mode" isDisabled={d.stateCues.length === 0} onPress={() => setAdminMode(!adminMode)}>
                                <Shield size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>{adminMode ? "Normal mode" : "Admin mode"}</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly size="sm" variant={d.stateDictMode === "focus" ? "primary" : "ghost"} aria-label="Toggle focus mode" isDisabled={d.stateCues.length === 0} onPress={() => {
                                if (d.stateDictMode === "focus") { d.setStateDictMode("full"); }
                                else {
                                    const cueList = d.stateCues.filter((cue) => !d.stateDictSuccessSet.has(cue.uuid));
                                    d.setStateDictCue(cueList.length > 0 ? cueList[0] : undefined);
                                    d.setStateDictMode("focus");
                                }
                            }}>
                                <Target size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>{d.stateDictMode === "full" ? "Focus mode" : "Full view"}</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button isIconOnly size="sm" variant={d.stateDictStatus === "complete" ? "primary" : "ghost"} aria-label="Mark complete" isDisabled={d.stateCues.length === 0} onPress={d.handleDictStatusToggle}>
                                <CheckCircle size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>{d.stateDictStatus === "complete" ? "Complete" : "Mark complete"}</Tooltip.Content>
                    </Tooltip>
                </div>
            </div>

            {/* Breadcrumb navigation */}
            <nav className="flex flex-row items-center gap-2 mb-4 text-sm select-none">
                <button
                    className={`cursor-pointer hover:underline ${d.selectedDatasetUuid ? "text-accent" : "text-text-primary font-medium"}`}
                    onClick={() => d.setSelectedDatasetUuid("")}
                >
                    Datasets
                </button>
                {d.selectedDataset && (
                    <>
                        <span className="text-text-tertiary">&rsaquo;</span>
                        <button
                            className={`cursor-pointer hover:underline truncate max-w-[240px] ${d.stateMediaUUID ? "text-accent" : "text-text-primary font-medium"}`}
                            onClick={() => d.setStateMediaUUID("")}
                        >
                            {d.selectedDataset.info.name}
                        </button>
                    </>
                )}
                {d.selectedDataset && d.stateMediaUUID && (
                    <>
                        <span className="text-text-tertiary">&rsaquo;</span>
                        <span className="text-text-primary font-medium truncate max-w-[240px]">{selectedMediaTitle}</span>
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
                                                <Tooltip>
                                                    <Tooltip.Trigger>
                                                        <Button
                                                            isIconOnly
                                                            variant="ghost"
                                                            size="sm"
                                                            aria-label="Remove location"
                                                            className="text-text-tertiary hover:text-error-text"
                                                            onPress={() => setConfirmReq({
                                                                title: "Remove location",
                                                                message: `Remove "${loc.name}" from dataset locations? Its datasets will no longer be listed here, but files on disk are untouched.`,
                                                                confirmLabel: "Remove",
                                                                onConfirm: () => d.handleRemoveLocation(loc.path),
                                                            })}
                                                        >
                                                            <X size={14} />
                                                        </Button>
                                                    </Tooltip.Trigger>
                                                    <Tooltip.Content>Remove this location</Tooltip.Content>
                                                </Tooltip>
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
                    {d.mediaList.length === 0 && !d.stateLoading ? (
                        <p className="text-text-secondary">No media in this dataset.</p>
                    ) : d.mediaList.map((m) => {
                        const src = d.getMediaSrc(m.source);
                        const isCompleted = d.completedMediaUuids.has(m.uuid);
                        return (
                            <div key={m.uuid} className={`p-4 border rounded-lg flex flex-col gap-2 ${isCompleted ? "bg-success-bg/50 border-accent" : "border-border-default"}`}>
                                <button
                                    className="text-left font-medium text-accent hover:underline cursor-pointer"
                                    onClick={() => d.setStateMediaUUID(m.uuid)}
                                >
                                    {m.title || m.source}
                                </button>
                                {src && (isAudio(m.source)
                                    ? <audio controls preload="none" src={src} className="w-full" />
                                    : <video controls preload="none" src={src} className="w-full max-h-64" />)}
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

                        {/* Active cue */}
                        {d.stateCues.length > 0 && d.stateActiveTab !== "dictation" && (
                            <div className="flex flex-row items-center justify-center w-full py-3">
                                <div className="transition-all duration-300 text-xl font-semibold leading-snug">
                                    {d.stateActiveCue || "..."}
                                </div>
                            </div>
                        )}
                    </div>

                    {/* Tabs */}
                    <Tabs className="font-bold w-full flex-1 min-h-0 overflow-hidden" variant="secondary" selectedKey={d.stateActiveTab} onSelectionChange={(v) => d.setStateActiveTab(String(v))}>
                        <Tabs.ListContainer>
                            <Tabs.List aria-label="Media tabs" className="w-fit *:h-6 *:w-fit *:px-3 *:text-sm *:font-normal *:data-[selected=true]:font-bold">
                                <Tabs.Tab id="media">Media</Tabs.Tab>
                                <Tabs.Tab id="dictation">Dictation</Tabs.Tab>
                            </Tabs.List>
                        </Tabs.ListContainer>

                        <Tabs.Panel id="media" className="flex flex-col w-full gap-3">
                            {/* ── Media tab ── */}
                            <div>
                                <div className="flex flex-row items-center justify-start gap-2">
                                    <span className="flex-1 text-xl font-bold text-blue-500">Media</span>
                                </div>
                                <Separator className="my-4" />
                                <TextField className="w-full">
                                    <Label>Title</Label>
                                    <Input value={d.stateMedia.title} onChange={(e) => d.setStateMedia({ ...d.stateMedia, title: e.target.value })} />
                                </TextField>
                                <TextField className="w-full mt-2">
                                    <Label>Source</Label>
                                    <Input value={d.stateMedia.source} readOnly />
                                </TextField>
                                <TextField className="w-full mt-2">
                                    <Label>Note</Label>
                                    <Input value={d.stateMedia.note} onChange={(e) => d.setStateMedia({ ...d.stateMedia, note: e.target.value })} />
                                </TextField>
                                <div className="flex justify-end gap-2 pt-3 mt-1">
                                    <Button variant="primary" size="sm" isDisabled={d.stateSaving} onPress={d.handleSaveMedia}>Save</Button>
                                </div>
                            </div>

                            <div>
                                <div className="flex flex-row items-center justify-start gap-2">
                                    <span className="flex-1 text-xl font-bold text-blue-500">Subtitle</span>
                                </div>
                                <Separator className="my-4" />
                                {d.stateSubtitleList.map((v) => (
                                    <SubtitleItem key={v.uuid} item={v} datasetUuid={d.selectedDatasetUuid} isActive={d.stateSubtitle?.uuid === v.uuid} onSelect={() => d.setStateSubtitle(v)} />
                                ))}
                            </div>
                        </Tabs.Panel>

                        <Tabs.Panel id="dictation" className="flex flex-col w-full gap-3 flex-1 min-h-0">
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

                                {!!d.stateSubtitle && <span className="text-xs text-gray-300">UUID: {d.stateSubtitle.uuid}</span>}
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

                                {d.stateDictMode === "full" ? (
                                    d.stateCues.map((cue, i) => (
                                        <div key={i} data-cue-index={i} className={`rounded-xl border-2 py-1.5 px-2 transition-colors border-border-light ${cue.deleted ? "bg-error-bg" : cue.modified ? "bg-accent-bg/20" : "bg-bg-body"}`}>
                                            <CueEditor
                                                cue={cue}
                                                media={d.videoRef.current}
                                                allowEdit={true}
                                                mode={d.stateEditingCue !== cue.uuid ? "dictation" : "dictation_edit"}
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
                                            />
                                        </div>
                                    ))
                                ) : (
                                    <div className="flex flex-col items-center justify-center gap-1 w-full">
                                        {!d.stateDictCue ? (
                                            <div>cue not found</div>
                                        ) : (
                                            <CueEditor
                                                cue={d.stateDictCue}
                                                media={d.videoRef.current}
                                                allowEdit={true}
                                                mode="dictation_focus"
                                                isDisabled={d.stateSaving}
                                                adminMode={adminMode}
                                                onUpdate={(updated) => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === updated.uuid); if (idx !== -1) draft[idx] = updated; })}
                                                onExpandStart={() => d.handleExpandStart(d.stateDictCue!)}
                                                onExpandEnd={() => d.handleExpandEnd(d.stateDictCue!)}
                                                onDelete={() => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === d.stateDictCue!.uuid); if (idx !== -1) draft.splice(idx, 1); draft.forEach((item, i) => (item.order_num = i + 1)); })}
                                                onMergeNext={() => d.updateStateCues((draft) => { const idx = draft.findIndex((c) => c.uuid === d.stateDictCue!.uuid); if (idx >= 0 && idx < draft.length - 1) { draft[idx].content += " " + draft[idx + 1].content; draft[idx].end_ms = draft[idx + 1].end_ms; draft.splice(idx + 1, 1); draft.forEach((item, i) => (item.order_num = i + 1)); } })}
                                                onInsert={(pos: number) => d.updateStateCues((draft) => { const newItem: Cue = { uuid: getUUID(), subtitle_uuid: d.stateSubtitle!.uuid, order_num: 0, start_ms: 0, end_ms: 0, content: "", reference: null }; if (pos < 1) draft.unshift(newItem); else if (pos > draft.length) draft.push(newItem); else draft.splice(pos - 1, 0, newItem); draft.forEach((item, i) => (item.order_num = i + 1)); })}
                                                onEdit={() => d.setStateEditingCue(d.stateDictCue!.uuid)}
                                                onDone={() => d.setStateEditingCue(null)}
                                                initialSuccess={d.stateDictSuccessSet.has(d.stateDictCue.uuid)}
                                                onSuccess={d.handleDictSuccess}
                                                onFocusInput={() => d.setStateFocusedCueUUID(d.stateDictCue!.uuid)}
                                            />
                                        )}
                                        {!!d.stateDictCue && (
                                            <div className="flex flex-row items-center justify-center gap-1 w-full">
                                                <Button onPress={() => {
                                                    for (const cue of d.stateCues) {
                                                        if (cue.order_num > d.stateDictCue!.order_num && !d.stateDictSuccessSet.has(cue.uuid)) { d.setStateDictCue(cue); return; }
                                                    }
                                                    setConfirmReq({ message: "finished!" });
                                                }}>Next</Button>
                                            </div>
                                        )}
                                    </div>
                                )}
                            </div>
                        </Tabs.Panel>
                    </Tabs>
                </div>
            )}

            <ConfirmDialog request={confirmReq} onClose={() => setConfirmReq(null)} />
        </div>
    );
}
