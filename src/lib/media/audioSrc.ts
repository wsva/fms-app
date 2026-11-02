import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import { isMobileApp } from "@/lib/platform";

/**
 * Resolve an absolute audio file path to something usable as an `<audio>` /
 * `new Audio()` `src` — the one place that decides *how* a stored recording
 * reaches the media pipeline, so every read-aloud player behaves the same.
 *
 * Desktop returns the Tauri asset-protocol URL: WebView2 / WKWebView honour
 * Range requests on it, so long files stream and seek correctly.
 *
 * Android returns a **blob URL** built from the file's bytes. The system media
 * framework stalls at roughly 30 seconds on a long WAV served over the asset
 * protocol — it buffers only the first chunk of an uncompressed PCM stream and
 * cannot seek forward through the rest — while the *same* bytes, handed to the
 * webview as a blob, are decoded in-process and play to the end. This is why
 * the read-aloud pages (whose audio is all 16-bit PCM WAV: the standard clips
 * and the attempt recordings) cut off on a phone even though dictation's
 * original MP3/M4A files play through. It mirrors the mic-test recorder's
 * Android playback path, which already builds a WAV blob for exactly this
 * reason.
 *
 * The blob must be released when the caller is done with it: pass the URL the
 * helper returned back to {@link releaseAudioSrc}. On desktop that is a no-op
 * (the value is an asset URL, not a blob). A mobile read failure falls back to
 * the asset URL rather than losing playback entirely — at worst the behaviour
 * is the current 30-second cutoff, never nothing.
 */
export async function acquireAudioSrc(absPath: string | null | undefined): Promise<string> {
  if (!absPath) return "";
  if (!isMobileApp()) return convertFileSrc(absPath);
  try {
    const bytes = await readFile(absPath);
    return URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }));
  } catch {
    return convertFileSrc(absPath);
  }
}

/** Revoke a blob URL produced by {@link acquireAudioSrc}; ignores asset URLs. */
export function releaseAudioSrc(src: string | null | undefined): void {
  if (src && src.startsWith("blob:")) URL.revokeObjectURL(src);
}
