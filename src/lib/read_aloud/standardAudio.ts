import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import type { AudioProbe, ReadStandardAudio } from "./types";

/**
 * Wrappers for the `read_aloud_standard_audio_*` commands — the reference clips
 * a text's pronunciation is measured against.
 *
 * Every call is guarded so the page can render in a plain browser (and during
 * `next build`) without a Tauri runtime: a read returns nothing, a write throws
 * a message the UI can show.
 */

function requireTauri(): void {
  if (!isTauri()) {
    throw new Error("Standard audio needs the desktop or Android app.");
  }
}

/**
 * The clips of one text, or of the whole dataset when `textUuid` is omitted
 * (the texts list uses that to badge rows that have a usable clip).
 */
export async function listStandardAudio(
  datasetUuid: string,
  textUuid?: string
): Promise<ReadStandardAudio[]> {
  if (!isTauri()) return [];
  return invoke<ReadStandardAudio[]>("read_aloud_standard_audio_list", {
    datasetUuid,
    textUuid: textUuid ?? null,
  });
}

/**
 * Decode a candidate file without storing it, so the crop track can draw it.
 *
 * Mobile caveat: a picker returns a `content://` URI, which is not a path, so
 * the bytes are read with plugin-fs and sent as base64 together with a name.
 * Exactly one of `sourcePath` / `sourceBase64` must be given.
 */
export async function probeAudioSource(source: {
  sourcePath?: string;
  sourceBase64?: string;
  fileName?: string;
}): Promise<AudioProbe> {
  requireTauri();
  return invoke<AudioProbe>("read_aloud_standard_audio_probe", {
    sourcePath: source.sourcePath ?? null,
    sourceBase64: source.sourceBase64 ?? null,
    fileName: source.fileName ?? null,
  });
}

/** Store a picked recording (optionally cut to `[start, end)` seconds). */
export async function importStandardAudio(params: {
  datasetUuid: string;
  textUuid: string;
  sourcePath?: string;
  sourceBase64?: string;
  fileName?: string;
  note?: string;
  start?: number;
  end?: number;
}): Promise<ReadStandardAudio> {
  requireTauri();
  return invoke<ReadStandardAudio>("read_aloud_standard_audio_import", {
    datasetUuid: params.datasetUuid,
    textUuid: params.textUuid,
    sourcePath: params.sourcePath ?? null,
    sourceBase64: params.sourceBase64 ?? null,
    fileName: params.fileName ?? null,
    note: params.note ?? null,
    start: params.start ?? null,
    end: params.end ?? null,
  });
}

/**
 * Synthesise the text with Edge TTS and store the result as a clip. `voice`
 * empty means "the voice configured on the TTS page".
 */
export async function generateStandardAudio(params: {
  datasetUuid: string;
  textUuid: string;
  voice?: string;
  note?: string;
}): Promise<ReadStandardAudio> {
  requireTauri();
  return invoke<ReadStandardAudio>("read_aloud_standard_audio_generate", {
    datasetUuid: params.datasetUuid,
    textUuid: params.textUuid,
    voice: params.voice ?? null,
    note: params.note ?? null,
  });
}

/** Make one clip the default the scorer reads. */
export async function setDefaultStandardAudio(
  datasetUuid: string,
  uuid: string
): Promise<void> {
  requireTauri();
  await invoke("read_aloud_standard_audio_set_default", { datasetUuid, uuid });
}

/** Delete one clip (row + wav); the earliest remaining clip is promoted. */
export async function deleteStandardAudio(
  datasetUuid: string,
  uuid: string
): Promise<void> {
  requireTauri();
  await invoke("read_aloud_standard_audio_delete", { datasetUuid, uuid });
}

/** Group clips by text, for the dataset-wide badge lookup. */
export function groupByText(
  clips: ReadStandardAudio[]
): Map<string, ReadStandardAudio[]> {
  const map = new Map<string, ReadStandardAudio[]>();
  for (const clip of clips) {
    const list = map.get(clip.text_uuid);
    if (list) list.push(clip);
    else map.set(clip.text_uuid, [clip]);
  }
  return map;
}

/** True when any of a text's clips can actually be scored against. */
export function hasUsableClip(clips: ReadStandardAudio[] | undefined): boolean {
  return Boolean(clips?.some((clip) => clip.usable));
}
