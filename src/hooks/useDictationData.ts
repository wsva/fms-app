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
    info: { uuid: string; name: string; is_favorites?: boolean };
    path: string;
    /** Root location directory this dataset was found under. */
    location: string;
    status: string;
}

// Directory entry returned by `dataset_list_dirs` (mirrors DatasetDirEntry in Rust).
export interface DatasetDirEntry {
    name: string;
    path: string;
    is_linked: boolean;
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
    const [locations, setLocations] = useState<DatasetDirEntry[]>([]);
    const [locationError, setLocationError] = useState<string | null>(null);
    const [selectedDatasetUuid, setSelectedDatasetUuid] = useState<string>("");
    const [mediaList, setMediaList] = useState<ListenMedia[]>([]);
    // Per-media pending edits (source/note) for the media-list edit mode.
    const [mediaEdits, setMediaEdits] = useState<Record<string, { note: string; source: string }>>({});
    // Subtitles per media (media uuid -> subtitle list), loaded for the media-list cards.
    const [mediaSubtitles, setMediaSubtitles] = useState<Record<string, ListenSubtitle[]>>({});
    const [stateMediaUUID, setStateMediaUUID] = useState<string>("");
    const [stateMedia, setStateMedia] = useState<ListenMedia>(newMedia());
    const [stateSaving, setStateSaving] = useState(false);
    const [stateLoading, setStateLoading] = useState(false);
    // Bumped to force a reload of the selected dataset's database.
    const [reloadToken, setReloadToken] = useState(0);

    // Subtitles / cues
    const [stateSubtitleList, setStateSubtitleList] = useState<ListenSubtitle[]>([]);
    const [stateSubtitle, setStateSubtitle] = useState<ListenSubtitle | undefined>();
    // When set, the subtitle-loading effect selects this subtitle instead of the first
    // (used by media-card subtitle links to deep-link into a specific dictation).
    const pendingSubtitleUuid = useRef<string | null>(null);
    const [stateCues, updateStateCues] = useImmer<Cue[]>([]);
    const [stateActiveCue, setStateActiveCue] = useState("");
    const [stateNeedSave, setStateNeedSave] = useState(false);
    const [stateEditingCue, setStateEditingCue] = useState<string | null>(null);

    // Dictation
    const [stateDictSuccessSet, setStateDictSuccessSet] = useState<Set<string>>(new Set());
    const [stateDictStatus, setStateDictStatus] = useState<"in_progress" | "complete">("in_progress");
    const [stateDictMode, setStateDictMode] = useState<"full" | "focus">("full");
    const [stateDictCue, setStateDictCue] = useState<Cue | undefined>();
    const [completedMediaUuids, setCompletedMediaUuids] = useState<Set<string>>(new Set());
    // Cue UUIDs already present in the Favorites dataset (favorite clips reuse the
    // source cue's uuid). Used to mark/disable the "add to favorites" button.
    const [favoriteCueUuids, setFavoriteCueUuids] = useState<Set<string>>(new Set());
    const dictSaveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    // Player
    const videoRef = useRef<HTMLVideoElement>(null);
    const [stateFocusedCueUUID, setStateFocusedCueUUID] = useState<string | null>(null);
    const [stateWaveformPeaks, setStateWaveformPeaks] = useState<WaveformData | null>(null);

    // ── Load datasets ──

    const loadDatasets = useCallback(() => {
        if (!isTauri()) return;
        setStateLoading(true);
        // Keep not-ready datasets too so the page can display them with a
        // "Not Ready" mark; selection is blocked in the UI instead.
        invoke<DatasetSummary[]>("dataset_list")
            .then((res) => setDatasets(res))
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, []);

    useEffect(() => { loadDatasets(); }, [loadDatasets]);

    // ── Favorites (cue UUIDs already cut into the Favorites dataset) ──

    const loadFavoriteCues = useCallback(() => {
        if (!isTauri()) return;
        invoke<string[]>("dictation_list_favorite_cues")
            .then((res) => setFavoriteCueUuids(new Set(res)))
            .catch(() => setFavoriteCueUuids(new Set()));
    }, []);

    // Refresh when the viewed dataset/media changes (and on mount) so the Star
    // buttons reflect the current Favorites dataset contents.
    useEffect(() => { loadFavoriteCues(); }, [loadFavoriteCues, selectedDatasetUuid, stateMediaUUID, reloadToken]);

    // ── Dataset locations ──

    const loadLocations = useCallback(() => {
        if (!isTauri()) return;
        invoke<DatasetDirEntry[]>("dataset_list_dirs")
            .then((res) => setLocations(res))
            .catch((e) => setLocationError(`Failed to list dataset directories: ${e}`));
    }, []);

    useEffect(() => { loadLocations(); }, [loadLocations]);

    /// Pick a folder and add it as a linked dataset location.
    const handleAddLocation = useCallback(async () => {
        if (!isTauri()) return;
        setLocationError(null);
        try {
            const path = await invoke<string>("settings_pick_folder", { field: "dataset_location" });
            // Derive display name from the folder name (last path segment).
            const name = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
            await invoke("dataset_add_dir", { name, path });
            loadLocations();
            loadDatasets();
        } catch (e) {
            // Ignore folder-picker cancellation.
            if (String(e).includes("No folder selected")) return;
            setLocationError(String(e));
        }
    }, [loadLocations, loadDatasets]);

    /// Unlink a linked dataset location (files on disk are untouched).
    const handleRemoveLocation = useCallback(async (path: string) => {
        if (!isTauri()) return;
        setLocationError(null);
        try {
            await invoke("dataset_remove_dir", { path });
            loadLocations();
            loadDatasets();
        } catch (e) {
            setLocationError(String(e));
        }
    }, [loadLocations, loadDatasets]);

    // ── Load media list when dataset changes ──

    useEffect(() => {
        if (!selectedDatasetUuid) { setMediaList([]); setMediaEdits({}); setStateMediaUUID(""); setCompletedMediaUuids(new Set()); return; }
        setStateLoading(true);
        Promise.all([
            invoke<ListenMedia[]>("listen_list_media", { datasetUuid: selectedDatasetUuid }),
            invoke<string[]>("listen_get_dataset_dictation_status", { datasetUuid: selectedDatasetUuid }),
        ])
            .then(([mediaRes, completedRes]) => {
                setMediaList(mediaRes);
                setMediaEdits({});
                setCompletedMediaUuids(new Set(completedRes));
                // Keep the current selection if it still exists (e.g. on reload),
                // otherwise reset (e.g. on dataset switch or after deletion).
                setStateMediaUUID((prev) => (mediaRes.some((m) => m.uuid === prev) ? prev : ""));
            })
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, [selectedDatasetUuid, reloadToken]);

    // ── Load subtitles for each media in the list (for card links) ──

    useEffect(() => {
        if (!selectedDatasetUuid || mediaList.length === 0) { setMediaSubtitles({}); return; }
        let cancelled = false;
        Promise.all(
            mediaList.map((m) =>
                invoke<ListenSubtitle[]>("listen_get_subtitles", { datasetUuid: selectedDatasetUuid, mediaUuid: m.uuid })
                    .then((subs) => [m.uuid, subs] as [string, ListenSubtitle[]])
                    .catch((e) => { console.error(e); return [m.uuid, []] as [string, ListenSubtitle[]]; })
            )
        ).then((entries) => { if (!cancelled) setMediaSubtitles(Object.fromEntries(entries)); });
        return () => { cancelled = true; };
    }, [selectedDatasetUuid, mediaList, reloadToken]);

    // ── Load media details ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateMediaUUID) { setStateMedia(newMedia()); setStateSubtitleList([]); setStateSubtitle(undefined); return; }
        setStateLoading(true);
        invoke<ListenMedia>("listen_get_media", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID })
            .then((res) => setStateMedia(res))
            .catch(console.error)
            .finally(() => setStateLoading(false));
    }, [selectedDatasetUuid, stateMediaUUID, reloadToken]);

    // ── Load subtitles ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateMediaUUID) { setStateSubtitleList([]); setStateSubtitle(undefined); return; }
        invoke<ListenSubtitle[]>("listen_get_subtitles", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID })
            .then((res) => {
                setStateSubtitleList(res);
                const pending = pendingSubtitleUuid.current;
                pendingSubtitleUuid.current = null;
                const match = pending ? res.find((s) => s.uuid === pending) : undefined;
                setStateSubtitle(match ?? res[0]);
            })
            .catch(console.error);
    }, [selectedDatasetUuid, stateMediaUUID, reloadToken]);

    // ── Load cues when subtitle changes ──

    useEffect(() => {
        if (!selectedDatasetUuid || !stateSubtitle?.uuid) { updateStateCues(() => []); return; }
        invoke<Cue[]>("listen_get_cues", { datasetUuid: selectedDatasetUuid, subtitleUuid: stateSubtitle.uuid })
            .then((res) => updateStateCues(() => res.map((item) => ({ ...item, content_original: item.content }))))
            .catch(console.error);
    }, [selectedDatasetUuid, stateSubtitle?.uuid, reloadToken]);

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
    }, [selectedDatasetUuid, stateMedia.source, reloadToken]);

    // ── Resolve audio src ──

    // Guard against empty-uuid datasets (raw folders without info.json): an empty
    // selectedDatasetUuid means "nothing selected" and must not resolve to them.
    const selectedDataset = selectedDatasetUuid
        ? datasets.find((d) => d.info.uuid === selectedDatasetUuid)
        : undefined;
    const audioFullPath = selectedDataset && stateMedia.source ? `${selectedDataset.path}/media/${stateMedia.source}` : "";
    const audioSrc = audioFullPath && isTauri() ? convertFileSrc(audioFullPath) : "";
    const audioMode = isAudio(stateMedia.source);
    const hasMedia = !!audioSrc;

    /// Build a playable asset URL for any media source within the selected dataset.
    const getMediaSrc = useCallback((source: string) => {
        if (!selectedDataset || !source || !isTauri()) return "";
        return convertFileSrc(`${selectedDataset.path}/media/${source}`);
    }, [selectedDataset]);

    /// Select a media and jump straight into a specific subtitle's dictation view.
    const selectMediaSubtitle = useCallback((mediaUuid: string, subtitleUuid: string) => {
        pendingSubtitleUuid.current = subtitleUuid;
        setStateMediaUUID(mediaUuid);
    }, []);

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

        // Award XP for cue completion.
        if (success && isTauri() && selectedDatasetUuid && stateSubtitle?.uuid) {
            invoke("xp_award_dictation_cue", { cueId: uuid, datasetUuid: selectedDatasetUuid }).catch(() => {});
            // Check if all cues are now completed → subtitle bonus.
            if (newSet.size === stateCues.length && stateCues.length > 0) {
                invoke("xp_award_dictation_subtitle", { subtitleId: stateSubtitle.uuid, datasetUuid: selectedDatasetUuid }).catch(() => {});
            }
        }
    }, [stateDictSuccessSet, stateDictStatus, scheduleDictSave, selectedDatasetUuid, stateSubtitle?.uuid, stateCues.length]);

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

    // ── Reload / delete media ──

    /// Reload the database file of the selected dataset.
    const handleReload = useCallback(() => {
        if (!selectedDatasetUuid) return;
        setReloadToken((t) => t + 1);
    }, [selectedDatasetUuid]);

    /// Remove the selected media and all its related data and files.
    const handleDeleteMedia = useCallback(async () => {
        if (!selectedDatasetUuid || !stateMediaUUID) return;
        setStateSaving(true);
        try {
            await invoke("listen_delete_media", { datasetUuid: selectedDatasetUuid, mediaUuid: stateMediaUUID });
            setStateMediaUUID("");
            setReloadToken((t) => t + 1);
        } catch (e) {
            console.error(e);
        } finally { setStateSaving(false); }
    }, [selectedDatasetUuid, stateMediaUUID]);

    // ── Media list editing (edit mode on the media list view) ──

    /// Number of media rows with unsaved source/note edits.
    const mediaDirtyCount = Object.keys(mediaEdits).length;

    const isMediaDirty = useCallback((m: ListenMedia) => {
        const e = mediaEdits[m.uuid];
        return !!e && (e.note !== m.note || e.source !== m.source);
    }, [mediaEdits]);

    /// Current (possibly edited) value of a media field.
    const mediaFieldOf = useCallback(
        (m: ListenMedia, key: "note" | "source") => mediaEdits[m.uuid]?.[key] ?? m[key],
        [mediaEdits]
    );

    /// Stage an edit for a media row; drops the entry when it matches the stored value.
    const setMediaField = useCallback((m: ListenMedia, key: "note" | "source", value: string) => {
        setMediaEdits((prev) => {
            const base = prev[m.uuid] ?? { note: m.note, source: m.source };
            const next = { ...base, [key]: value };
            if (next.note === m.note && next.source === m.source) {
                const { [m.uuid]: _removed, ...rest } = prev;
                void _removed;
                return rest;
            }
            return { ...prev, [m.uuid]: next };
        });
    }, []);

    const persistMediaRow = useCallback(async (m: ListenMedia, edit: { note: string; source: string }) => {
        const newSource = edit.source.trim();
        // If the source (filename) changed, rename the physical files on disk first
        // (media/subtitle/waveform/transcript + DB source), then persist note & fields.
        if (newSource && newSource !== m.source) {
            await invoke("listen_rename_media", {
                datasetUuid: selectedDatasetUuid,
                mediaUuid: m.uuid,
                newSource,
            });
        }
        await invoke("listen_save_media", {
            datasetUuid: selectedDatasetUuid,
            media: { ...m, source: newSource || m.source, note: edit.note, updated_at: new Date().toISOString() },
        });
    }, [selectedDatasetUuid]);

    /// Save every pending media row edit at once.
    const handleSaveAllMedia = useCallback(async () => {
        const entries = Object.entries(mediaEdits);
        if (entries.length === 0) return;
        setStateSaving(true);
        try {
            await Promise.all(entries.map(([uuid, edit]) => {
                const m = mediaList.find((x) => x.uuid === uuid);
                return m ? persistMediaRow(m, edit) : Promise.resolve();
            }));
            setMediaList((prev) => prev.map((x) => (mediaEdits[x.uuid] ? { ...x, ...mediaEdits[x.uuid] } : x)));
            setMediaEdits({});
        } catch (e) { console.error(e); }
        finally { setStateSaving(false); }
    }, [mediaEdits, mediaList, persistMediaRow]);

    /// Remove a specific media row (and its related data/files) from the list.
    const handleDeleteMediaRow = useCallback(async (m: ListenMedia) => {
        setStateSaving(true);
        try {
            await invoke("listen_delete_media", { datasetUuid: selectedDatasetUuid, mediaUuid: m.uuid });
            setMediaList((prev) => prev.filter((x) => x.uuid !== m.uuid));
            setMediaEdits((prev) => { const { [m.uuid]: _r, ...rest } = prev; void _r; return rest; });
            setCompletedMediaUuids((prev) => { const next = new Set(prev); next.delete(m.uuid); return next; });
        } catch (e) { console.error(e); }
        finally { setStateSaving(false); }
    }, [selectedDatasetUuid]);

    return {
        // Dataset / media
        datasets, selectedDatasetUuid, setSelectedDatasetUuid, mediaList, stateMediaUUID, setStateMediaUUID,
        stateMedia, setStateMedia, stateSaving, stateLoading, loadDatasets,
        selectedDataset, getMediaSrc, mediaSubtitles, selectMediaSubtitle,
        // Dataset locations
        locations, locationError, loadLocations, handleAddLocation, handleRemoveLocation,
        // Subtitles / cues
        stateSubtitleList, setStateSubtitleList, stateSubtitle, setStateSubtitle,
        stateCues, updateStateCues, stateActiveCue, stateNeedSave, setStateNeedSave,
        stateEditingCue, setStateEditingCue,
        // Dictation
        stateDictSuccessSet, stateDictStatus, stateDictMode, setStateDictMode,
        stateDictCue, setStateDictCue, completedMediaUuids,
        handleDictSuccess, handleDictStatusToggle,
        // Favorites
        favoriteCueUuids, reloadFavorites: loadFavoriteCues,
        // Player
        videoRef, stateFocusedCueUUID, setStateFocusedCueUUID,
        stateWaveformPeaks, audioSrc, audioMode, hasMedia,
        // Handlers
        handleExpandStart, handleExpandEnd, handleSaveSubtitle, handleSaveMedia,
        handleReload, handleDeleteMedia,
        // Media list editing
        mediaEdits, mediaDirtyCount, isMediaDirty, mediaFieldOf, setMediaField,
        handleSaveAllMedia, handleDeleteMediaRow,
    };
}
