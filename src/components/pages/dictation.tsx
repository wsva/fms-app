"use client";

/**
 * DictationPage — uses useDictationData for all state/logic,
 * this file only handles rendering.
 */

import { ProgressCircle, Input, Select, Tabs, ListBox, Label, TextField, Separator, Button } from "@heroui/react";
import { MdCheckCircle } from "react-icons/md";
import CueEditor from "./listen/CueEditor";
import SubtitleItem from "./listen/Subtitle";
import WaveformCanvas from "./listen/WaveformCanvas";
import type { Cue } from "@/lib/types";
import { useDictationData } from "@/hooks/useDictationData";

const getUUID = () => crypto.randomUUID().replaceAll("-", "");

export default function DictationPage() {
    const d = useDictationData();

    return (
        <div className="flex flex-col flex-1 min-h-0 p-4 overflow-hidden">
            {/* Dataset + Media selection bar */}
            <div className="flex flex-row items-center justify-start gap-4 mb-4">
                <select
                    className="px-3 py-2 border border-border-light rounded-lg bg-bg-card text-sm min-w-[200px]"
                    value={d.selectedDatasetUuid}
                    onChange={(e) => {
                        if (e.target.value === "__refresh__") { d.loadDatasets(); return; }
                        d.setSelectedDatasetUuid(e.target.value);
                    }}
                >
                    <option value="">Select a dataset...</option>
                    <option value="__refresh__">↻ Refresh datasets</option>
                    {d.datasets.map((ds) => (
                        <option key={ds.info.uuid} value={ds.info.uuid}>{ds.info.name}</option>
                    ))}
                </select>

                {d.selectedDatasetUuid && (
                    <select
                        className="px-3 py-2 border border-border-light rounded-lg bg-bg-card text-sm min-w-[200px]"
                        value={d.stateMediaUUID}
                        onChange={(e) => d.setStateMediaUUID(e.target.value)}
                        disabled={d.stateLoading}
                    >
                        <option value="">Select media...</option>
                        {d.mediaList.map((m) => (
                            <option key={m.uuid} value={m.uuid}>{m.title}</option>
                        ))}
                    </select>
                )}

                {d.stateLoading && <ProgressCircle size="sm" aria-label="Loading" />}
            </div>

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
                                    <div className="flex items-center justify-between px-1">
                                        <span className="flex-1 text-sm text-foreground-500">{d.stateDictSuccessSet.size} / {d.stateCues.length} ✓</span>
                                        <div className="flex flex-row items-center justify-center gap-2">
                                            <Button size="sm" variant="secondary" onPress={() => {
                                                if (d.stateDictMode === "focus") { d.setStateDictMode("full"); }
                                                else {
                                                    const cueList = d.stateCues.filter((cue) => !d.stateDictSuccessSet.has(cue.uuid));
                                                    d.setStateDictCue(cueList.length > 0 ? cueList[0] : undefined);
                                                    d.setStateDictMode("focus");
                                                }
                                            }}>
                                                {d.stateDictMode === "full" ? "Focus Mode" : "Full View"}
                                            </Button>
                                            <Button size="sm" variant={d.stateDictStatus === "complete" ? "primary" : "secondary"} onPress={d.handleDictStatusToggle}>
                                                {d.stateDictStatus === "complete" && <MdCheckCircle size={16} />}
                                                {d.stateDictStatus === "complete" ? "Complete" : "Mark Complete"}
                                            </Button>
                                        </div>
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
                                        <div key={i} className={`rounded-xl border-2 py-1.5 px-2 transition-colors ${cue.active ? "border-success-text" : "border-border-light"} ${cue.deleted ? "bg-error-bg" : cue.modified ? "bg-accent-bg/20" : "bg-bg-body"}`}>
                                            <CueEditor
                                                cue={cue}
                                                media={d.videoRef.current}
                                                allowEdit={true}
                                                mode={d.stateEditingCue !== cue.uuid ? "dictation" : "dictation_edit"}
                                                isDisabled={d.stateSaving}
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
                                                    alert("finished!");
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

            {/* Empty state */}
            {!d.stateMediaUUID && !d.stateLoading && (
                <div className="text-xl text-text-secondary mt-8">
                    <p>1. Select a dataset with a generated database</p>
                    <p>2. Select a media file from the dataset</p>
                    <p>3. Edit subtitles, practice dictation, manage transcripts and notes</p>
                </div>
            )}
        </div>
    );
}
