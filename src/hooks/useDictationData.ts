/**
 * useDictationData — encapsulates all data loading, state management,
 * and business logic for the dictation page.
 */

import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useImmer } from "use-immer";
import { isAudio } from "@/lib/listen/utils";
import { isTauri } from "@/lib/tauri";
import type { Cue, ListenMedia, ListenSubtitle, ListenDictation } from "@/lib/types";
import type { WaveformData } from "@/components/listen_speak/dictation/components/WaveformCanvas";

const getUUID = () => crypto.randomUUID().replaceAll("-", "");

interface DatasetSummary {
    info: { uuid: string; name: string };
    path: string;
    status: string;
}

const newMedia = (): ListenMedia => ({
    uuid: getUUID(),
    title: "",
    source: "",
    note: "",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
});

export function useDictationData() {
    // Dataset / media selection
    const [datasets, setDatasets] = useState<DatasetSummary[]>([]);
    const [selectedDatasetUuid, setSelectedDatasetUuid] = useState<string>("");
    const [mediaList, setMediaList] = useState<ListenMedia[]>([]);
    const [stateMediaUUID, setStateMediaUUID] = useState<string>("");
    const [stateMedia, setStateMedia] = useState<ListenMedia>(newMedia());
    const [stateSaving, setStateSaving] = useState(false);
    const [stateLoading, setStateLoading] = useState(false);

    // Subtitles / cues
    const [stateSubtitleList, setStateSubtitleList] = useState<ListenSubtitle[]>([]);
    const [stateSubtitle, setStateSubtitle] = useState<ListenSubtitle | undefined>();
    const [stateCues, updateStateCues] = useImmer<Cue[]>([]);
    const [stateActiveCue, setStateActiveCue] = useState("");
    const [stateNeedSave, setStateNeedSave] = useState(false);
    const [stateEditingCue, setStateEditingCue] = useState<string | null>(null);

    // Dictation
    const [stateDictSuccessSet, setStateDictSuccessSet] = useState<Set<string>>(new Set());
    const [stateDictStatus, setStateDictStatus] = useState<"in_progress" | "complete">("in_progress");
    const [stateDictMode, setStateDictMode] = useState<"full" | "focus">("full");
    const [stateDictCue, setStateDictCue] = useState<Cue | undefined>();
    const dictSaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    // Player
    const videoRef = useRef<HTMLVideoElement>(null);
    const [stateActiveTab, setStateActiveTab] = useState("dictation");
    const [stateFocusedCueUUID, setStateFocusedCueUUID] = useState<string | null>(null);
    const [stateWaveformPeaks, setStateWaveformPeaks] = useState<WaveformData | null>(null);

    // ── Load datasets ──

    const loadDatasets = useCallback(() => {
        if (!isTauri()) return;
        setStateLoading(true);
        invoke<DatasetSummary[]>("dataset_list")
            .then((res) => setDatasets(res.filter((d) => d.status === "ready")))
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, []);

    useEffect(() => { loadDatasets(); }, [loadDatasets]);

    // ── Load media list when dataset changes ──

    useEffect(() => {
        if (!selectedDatasetUuid) { setMediaList([]); setStateMediaUUID(""); return; }
        setStateLoading(true);
        invoke<ListenMedia[]>("listen_list_media", { datasetUuid: selectedDatasetUuid })
            .then((res) => { setMediaList(res); setStateMediaUUID(""); })
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, [selectedDatasetUuid]);

    // ── Load media details ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateMediaUUID) { setStateMedia(newMedia()); setStateSubtitleList([]); setStateSubtitle(undefined); return; }
        setStateLoading(true);
        invoke<ListenMedia>("listen_get_media", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID })
            .then((res) => setStateMedia(res))
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, [selectedDatasetUuid, stateMediaUUID]);

    // ── Load subtitles ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateMediaUUID) { setStateSubtitleList([]); setStateSubtitle(undefined); return; }
        invoke<ListenSubtitle[]>("listen_get_subtitles", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID })
            .then((res) => { setStateSubtitleList(res); setStateSubtitle(res[0]); })
            .catch(console.error);
    }, [selectedDatasetUuid, stateMediaUUID]);

    // ── Load cues when subtitle changes ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateSubtitle?.uuid) { updateStateCues(() => []); return; }
        invoke<Cue[]>("listen_get_cues", { datasetUuid: selectedDatasetUuid, subtitleUuid: stateSubtitle.uuid })
            .then((res) => updateStateCues(() => res.map((item) => ({ ...item, content_original: item.content }))))
            .catch(console.error);
    }, [selectedDatasetUuid, stateSubtitle?.uuid]);

    // ── Load dictation progress ──

    useEffect(() => {
        if (dictSaveTimer.current) clearTimeout(dictSaveTimer.current);
        if (!selectedDatasetUuid || !stateMediaUUID || !stateSubtitle?.uuid) { setStateDictSuccessSet(new Set()); setStateDictStatus("in_progress"); return; }
        invoke<ListenDictation | null>("listen_get_dictation", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID, subtitleUuid: stateSubtitle.uuid })
            .then((res) => {
                if (res) {
                    const ids = res.completed ? res.completed.split(",").filter(Boolean) : [];
                    setStateDictSuccessSet(new Set(ids));
                    setStateDictStatus(res.status as "in_progress" | "complete");
                } else { setStateDictSuccessSet(new Set()); setStateDictStatus("in_progress"); }
            })
            .catch(console.error);
    }, [selectedDatasetUuid, stateMediaUUID, stateSubtitle?.uuid]);

    // ── Sync active cue with playback time ──

    useEffect(() => {
        const videoEl = videoRef.current;
        if (!videoEl) return;
        const onTimeUpdate = () => {
            const currentMs = videoEl.currentTime * 1000;
            const activeCue = stateCues.find((cue) => currentMs >= cue.start_ms && currentMs <= cue.end_ms);
            setStateActiveCue(activeCue ? activeCue.content : "");
            updateStateCues((draft) => { draft.forEach((cue) => { cue.active = currentMs >= cue.start_ms && currentMs <= cue.end_ms; }); });
        };
        videoEl.addEventListener("timeupdate", onTimeUpdate);
        return () => videoEl.removeEventListener("timeupdate", onTimeUpdate);
    }, [stateCues, updateStateCues]);

    // ── Load waveform when media changes ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateMedia.source) { setStateWaveformPeaks(null); return; }
        let cancelled = false;
        invoke<WaveformData | null>("listen_get_waveform", { datasetUuid: selectedDatasetUuid, source: stateMedia.source })
            .then((data) => { if (!cancelled) setStateWaveformPeaks(data ?? null); })
            .catch(() => { if (!cancelled) setStateWaveformPeaks(null); });
        return () => { cancelled = true; };
    }, [selectedDatasetUuid, stateMedia.source]);

    // ── Resolve audio src ──

    const selectedDataset = datasets.find((d) => d.info.uuid === selectedDatasetUuid);
    const audioFullPath = selectedDataset && stateMedia.source ? `${selectedDataset.path}/media/${stateMedia.source}` : "";
    const audioSrc = audioFullPath && isTauri() ? convertFileSrc(audioFullPath) : "";
    const audioMode = isAudio(stateMedia.source);
    const hasMedia = !!audioSrc;

    // ── Dictation handlers ──

    const scheduleDictSave = useCallback((successSet: Set<string>, status: string) => {
        if (!selectedDatasetUuid || !stateSubtitle?.uuid) return;
        if (dictSaveTimer.current) clearTimeout(dictSaveTimer.current);
        const dsUuid = selectedDatasetUuid;
        const mediaUUID = stateMediaUUID;
        const subtitleUUID = stateSubtitle.uuid;
        const completed = Array.from(successSet).join(",");
        dictSaveTimer.current = setTimeout(() => {
            invoke("listen_save_dictation", { datasetUuid: dsUuid, dictation: { media_uuid: mediaUUID, subtitle_uuid: subtitleUUID, status, completed } });
        }, 1000);
    }, [selectedDatasetUuid, stateMediaUUID, stateSubtitle?.uuid]);

    const handleDictSuccess = useCallback((uuid: string, success: boolean) => {
        const newSet = new Set(stateDictSuccessSet);
        if (success) newSet.add(uuid); else newSet.delete(uuid);
        setStateDictSuccessSet(newSet);
        scheduleDictSave(newSet, stateDictStatus);
    }, [stateDictSuccessSet, stateDictStatus, scheduleDictSave]);

    const handleDictStatusToggle = useCallback(async () => {
        if (!selectedDatasetUuid || !stateSubtitle?.uuid) return;
        if (dictSaveTimer.current) clearTimeout(dictSaveTimer.current);
        const newStatus = stateDictStatus === "complete" ? "in_progress" : "complete";
        setStateDictStatus(newStatus);
        const completed = Array.from(stateDictSuccessSet).join(",");
        await invoke("listen_save_dictation", { datasetUuid: selectedDatasetUuid, dictation: { media_uuid: stateMediaUUID, subtitle_uuid: stateSubtitle.uuid, status: newStatus, completed } });
    }, [selectedDatasetUuid, stateMediaUUID, stateSubtitle?.uuid, stateDictStatus, stateDictSuccessSet]);

    // ── Cue editing handlers ──

    const handleExpandStart = useCallback((cue: Cue) => {
        updateStateCues((draft) => {
            const index = draft.findIndex((c) => c.uuid === cue.uuid);
            if (index === 0) draft[index] = { ...draft[index], start_ms: 1, modified: true };
            else if (index > 0) draft[index] = { ...draft[index], start_ms: draft[index - 1].end_ms + 1, modified: true };
        });
        setStateNeedSave(true);
    }, [updateStateCues]);

    const handleExpandEnd = useCallback((cue: Cue) => {
        updateStateCues((draft) => {
            const index = draft.findIndex((c) => c.uuid === cue.uuid);
            if (index === draft.length - 1) draft[index] = { ...draft[index], end_ms: draft[index].start_ms + 3600000, modified: true };
            else if (index >= 0) draft[index] = { ...draft[index], end_ms: draft[index + 1].start_ms - 1, modified: true };
        });
        setStateNeedSave(true);
    }, [updateStateCues]);

    const handleSaveSubtitle = useCallback(async () => {
        if (!selectedDatasetUuid) return;
        setStateSaving(true);
        try {
            const tasks = stateCues.map(async (cue) => {
                if (cue.deleted) {
                    await invoke("listen_delete_cue", { datasetUuid: selectedDatasetUuid, cueUuid: cue.uuid });
                    return { type: "remove" as const, uuid: cue.uuid };
                } else if (cue.modified) {
                    const { active, content_original, modified, deleted, ...saveData } = cue;
                    await invoke("listen_save_cue", { datasetUuid: selectedDatasetUuid, cue: saveData });
                    return { type: "update" as const, cue: saveData };
                }
                return null;
            });
            const results = (await Promise.all(tasks)).filter(Boolean);
            updateStateCues((draft) => {
                for (const item of results) {
                    if (item?.type === "remove") { const idx = draft.findIndex((c) => c.uuid === item.uuid); if (idx !== -1) draft.splice(idx, 1); }
                    if (item?.type === "update") { const idx = draft.findIndex((c) => c.uuid === item.cue.uuid); if (idx !== -1) { draft[idx].content_original = item.cue.content; draft[idx].modified = false; } }
                }
            });
            setStateNeedSave(false);
        } finally { setStateSaving(false); }
    }, [selectedDatasetUuid, stateCues, updateStateCues]);

    const handleSaveMedia = useCallback(async () => {
        if (!selectedDatasetUuid) return;
        setStateSaving(true);
        try { await invoke("listen_save_media", { datasetUuid: selectedDatasetUuid, media: { ...stateMedia, updated_at: new Date().toISOString() } }); }
        catch (e) { console.error(e); }
        setStateSaving(false);
    }, [selectedDatasetUuid, stateMedia]);

    return {
        // Dataset / media
        datasets, selectedDatasetUuid, setSelectedDatasetUuid, mediaList, stateMediaUUID, setStateMediaUUID,
        stateMedia, setStateMedia, stateSaving, stateLoading, loadDatasets,
        // Subtitles / cues
        stateSubtitleList, setStateSubtitleList, stateSubtitle, setStateSubtitle,
        stateCues, updateStateCues, stateActiveCue, stateNeedSave, setStateNeedSave,
        stateEditingCue, setStateEditingCue,
        // Dictation
        stateDictSuccessSet, stateDictStatus, stateDictMode, setStateDictMode,
        stateDictCue, setStateDictCue,
        handleDictSuccess, handleDictStatusToggle,
        // Player
        videoRef, stateActiveTab, setStateActiveTab, stateFocusedCueUUID, setStateFocusedCueUUID,
        stateWaveformPeaks, audioSrc, audioMode, hasMedia,
        // Handlers
        handleExpandStart, handleExpandEnd, handleSaveSubtitle, handleSaveMedia,
    };
}
