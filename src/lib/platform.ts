/**
 * Platform detection helpers for the mobile (Android) thin client.
 *
 * These are all SSR-safe: during server-side rendering `navigator`/`window`
 * are undefined so every helper returns the desktop default, keeping the first
 * client paint consistent with the server HTML. Components that branch on the
 * result should only do so after mount (see the `mounted` guard used elsewhere).
 */

/** True when running inside the Tauri runtime (desktop or mobile). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * True on the Android thin-client build. Drives nav gating and the mobile UI
 * variants (datasets sync page, PC discovery, pending-upload badges).
 */
export function isMobileApp(): boolean {
  if (typeof navigator === "undefined") return false;
  return /android/i.test(navigator.userAgent);
}
