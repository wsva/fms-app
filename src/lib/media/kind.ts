//! Frontend mirror of `src-tauri/src/datasets/media_kind.rs`.
//!
//! Two lists exist because of the FFI boundary — one per side of it — not by
//! design. Keep both in step; the Rust module is the source of truth (the
//! backend is the one that copies files and refuses uploads).
//!
//! Classification is extension-only on both sides: no probing, no magic bytes.
//! A `.mov` renamed to `.mp4` renders wrong here and there, symmetrically.
//!
//! Two sets:
//! - the broad viewer set ( [`kindOf` ] / [`isVideoPath` ]), used by surfaces
//!   that just need to pick an element (wiki preview, dictation media list);
//! - the card-portable set ( [`cardKindOf` ] / [`cardExtsFor` ]), narrower so
//!   Android WebView will always play what a PC author attached. Do not widen
//!   it back toward the viewer set without re-checking codecs.

/// What a file is, decided by its extension alone.
export type MediaKind = "image" | "audio" | "video" | "other";

// ---------------------------------------------------------------------------
// Broad (viewer) set — mirrors the Rust `IMAGE_EXTS`/`AUDIO_EXTS`/`VIDEO_EXTS`.
// ---------------------------------------------------------------------------

export const IMAGE_EXTS = [
  "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif",
] as const;
export const AUDIO_EXTS = [
  "mp3", "wav", "m4a", "m4b", "ogg", "opus", "flac", "aac",
] as const;
export const VIDEO_EXTS = ["mp4", "webm", "mov", "m4v"] as const;

// ---------------------------------------------------------------------------
// Card-portable set — mirrors `CARD_IMAGE_EXTS`/`CARD_AUDIO_EXTS`/`CARD_VIDEO_EXTS`.
// ---------------------------------------------------------------------------

export const CARD_IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp"] as const;
export const CARD_AUDIO_EXTS = ["mp3", "m4a", "wav", "ogg"] as const;
export const CARD_VIDEO_EXTS = ["mp4", "webm"] as const;

/** Lowercased extension of a file name or dataset-relative path ("" when none). */
export function extensionOf(name: string): string {
  const base = name.split(/[/\\]/).pop() ?? name;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** Classify against the broad set — for anything that only renders. */
export function kindOf(name: string): MediaKind {
  const ext = extensionOf(name);
  if (!ext) return "other";
  if ((IMAGE_EXTS as readonly string[]).includes(ext)) return "image";
  if ((AUDIO_EXTS as readonly string[]).includes(ext)) return "audio";
  if ((VIDEO_EXTS as readonly string[]).includes(ext)) return "video";
  return "other";
}

/** True when the wiki viewer should emit `<video>` rather than `<audio>`. */
export function isVideoPath(rel: string): boolean {
  return kindOf(rel) === "video";
}

/** Classify against the card-portable set; `null` means "a card must refuse this". */
export function cardKindOf(name: string): MediaKind | null {
  const ext = extensionOf(name);
  if ((CARD_IMAGE_EXTS as readonly string[]).includes(ext)) return "image";
  if ((CARD_AUDIO_EXTS as readonly string[]).includes(ext)) return "audio";
  if ((CARD_VIDEO_EXTS as readonly string[]).includes(ext)) return "video";
  return null;
}

/** The accepted extensions of one card medium, for a file-picker filter. */
export function cardExtsFor(kind: MediaKind): readonly string[] {
  switch (kind) {
    case "image": return CARD_IMAGE_EXTS;
    case "audio": return CARD_AUDIO_EXTS;
    case "video": return CARD_VIDEO_EXTS;
    default: return [];
  }
}
