/**
 * In-browser TTS playback for arbitrary text (used by the Understanding panel's
 * Listen buttons). A single app-wide audio instance means two panels never
 * speak at once, and starting a new utterance notifies the previous owner so
 * its button flips back from "Stop" to "Listen". The voice is resolved
 * server-side by `tts_speak_text` (preferred voice per language), so callers
 * pass only a language tag.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import type { TtsPreviewResult, TtsVoice } from "./types";

/**
 * Turn Markdown into prose a synthesizer reads sensibly — headings, list
 * bullets, emphasis and table pipes are markup, not words. Fenced code becomes a
 * space so a code block is never read out character by character.
 */
export function stripMarkdown(md: string): string {
  return md
    .replace(/```[\s\S]*?```/g, " ") // fenced code blocks
    .replace(/`([^`]*)`/g, "$1") // inline code
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1") // images + links -> label text
    .replace(/^\s*[#>+-]+\s*/gm, "") // heading / quote / list markers
    .replace(/^\s*\d+\.\s*/gm, "") // ordered-list numbers
    .replace(/[*_~]{1,3}/g, "") // emphasis / strikethrough
    .replace(/\|/g, " ") // table pipes
    .replace(/\s+/g, " ")
    .trim();
}

let audio: HTMLAudioElement | null = null;
const stopListeners = new Set<() => void>();
// Session blob cache: key (language + stripped text) -> object URL. A repeat
// listen reuses the decoded audio with no IPC at all; the backend disk cache
// still serves the first listen after a restart. URLs live for the page's
// lifetime (bounded by the distinct texts heard in a study session).
const blobCache = new Map<string, string>();

/** Pause any playback and notify subscribed buttons. Cached URLs are kept. */
function teardown(): void {
  if (audio) {
    audio.pause();
    audio.onended = null;
    audio = null;
  }
  stopListeners.forEach((l) => l());
}

/** Stop the current utterance immediately. */
export const stopSpeech = teardown;

/**
 * Subscribe to "speech stopped" (this one ended, or another replaced it).
 * Returns an unsubscribe fn. Components use it to reset their playing state.
 */
export function onSpeechStop(cb: () => void): () => void {
  stopListeners.add(cb);
  return () => {
    stopListeners.delete(cb);
  };
}

/**
 * Play `text` (Markdown stripped) in `language`, synthesizing through the
 * backend only when it isn't already in the session cache. Rejects if the
 * backend has no voice configured, so callers can surface the error.
 */
export async function speakText(text: string, language?: string): Promise<void> {
  const plain = stripMarkdown(text);
  if (!plain || !isTauri()) return;
  const lang = (language || "").trim().toLowerCase();
  const key = `${lang}\n${plain}`;
  teardown(); // replace any prior utterance
  let url = blobCache.get(key);
  if (!url) {
    const res = await invoke<TtsPreviewResult>("tts_speak_text", {
      text: plain,
      language: lang ? lang : undefined,
    });
    const bytes = Uint8Array.from(atob(res.audio_base64), (c) => c.charCodeAt(0));
    url = URL.createObjectURL(new Blob([bytes], { type: "audio/mpeg" }));
    blobCache.set(key, url);
  }
  audio = new Audio(url);
  audio.onended = teardown;
  await audio.play();
}

// The distinct language subtags Edge TTS actually has voices for, fetched once
// and shared by every toolbar (a German book can then offer `de`, not a guess).
let languagesCache: Promise<string[]> | null = null;

export function listLanguages(): Promise<string[]> {
  if (!languagesCache) {
    languagesCache = invoke<TtsVoice[]>("edge_tts_list_voices")
      .then((voices) => {
        const set = new Set<string>();
        for (const v of voices) {
          const primary = (v.locale || "").split("-")[0].toLowerCase();
          if (primary) set.add(primary);
        }
        return [...set].sort();
      })
      .catch(() => {
        languagesCache = null; // never cache a failure
        return [] as string[];
      });
  }
  return languagesCache;
}
