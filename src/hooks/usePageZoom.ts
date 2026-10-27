"use client";

import { useCallback, useEffect, useState } from "react";

/**
 * Global page zoom (Ctrl / Cmd + `+`, `-`, `0`).
 *
 * Implemented with the non-standard `zoom` property on `<html>` rather than the
 * WebView's own zoom control, because Tauri's `zoom_hotkeys_enabled` is a
 * WebView2-only feature (unsupported on Android/iOS and off by default). CSS
 * zoom is supported by every engine this app ships on (WebView2, WKWebView,
 * WebKitGTK, Android WebView) and scales the whole page uniformly.
 *
 * One caveat shapes the layout: `zoom` multiplies viewport units by the zoom
 * factor without changing the window, so a box sized with `100vh` overshoots
 * the viewport at any level above 1 and the window grows a scrollbar of its
 * own — which scrolls the whole shell, chrome included, instead of the page
 * body. Full-height layout therefore uses percentages (`h-full`, plus the
 * html/body rule in globals.css), which resolve against a viewport already
 * corrected for the zoom factor. Do not reintroduce `h-screen`.
 *
 * The value lives in localStorage under `pageZoom` and is applied before first
 * paint by the head script in app/layout.tsx, which must keep the same key and
 * clamping rules as this module.
 */

const ZOOM_KEY = "pageZoom";
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2;
const ZOOM_STEP = 0.1;
const ZOOM_DEFAULT = 1;

/** Fired on `window` whenever the applied factor changes, carrying the new
 *  level in `detail`. CSS zoom does not fire `resize`, so anything whose pixel
 *  geometry was derived from the viewport (the agent dock's floating button)
 *  must re-derive itself on this event or it silently drifts off-screen. */
export const ZOOM_CHANGED_EVENT = "page-zoom-changed";

/**
 * The factor `<html>` is currently zoomed by. `window.innerWidth/Height` are
 * *not* scaled by CSS `zoom` — they stay in window CSS px — so a fixed element
 * placed at `left: x` renders at `x * zoom`. Code that clamps a position to
 * the visible area must therefore work in layout units: `innerWidth / zoom`.
 */
export function pageZoomFactor(): number {
  if (typeof window === "undefined") return 1;
  const z = parseFloat(document.documentElement.style.zoom || "1");
  return Number.isFinite(z) && z > 0 ? z : 1;
}

function clampZoom(value: number): number {
  if (!Number.isFinite(value)) return ZOOM_DEFAULT;
  // Round to the step so repeated zooming does not drift on float noise.
  const stepped = Math.round(value * 100) / 100;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, stepped));
}

function readStoredZoom(): number {
  if (typeof window === "undefined") return ZOOM_DEFAULT;
  return clampZoom(parseFloat(localStorage.getItem(ZOOM_KEY) ?? ""));
}

export function usePageZoom() {
  const [zoom, setZoom] = useState(readStoredZoom);

  // Push the level onto <html>. Runs on mount too, so the hook converges on
  // whatever the pre-paint script left there. The event lets zoom-derived
  // geometry (see `pageZoomFactor`) re-clamp; dispatching it redundantly on
  // mount is harmless — listeners re-derive the same in-view position.
  useEffect(() => {
    document.documentElement.style.zoom = zoom === ZOOM_DEFAULT ? "" : String(zoom);
    localStorage.setItem(ZOOM_KEY, String(zoom));
    window.dispatchEvent(new CustomEvent(ZOOM_CHANGED_EVENT, { detail: zoom }));
  }, [zoom]);

  const zoomIn = useCallback(() => setZoom((z) => clampZoom(z + ZOOM_STEP)), []);
  const zoomOut = useCallback(() => setZoom((z) => clampZoom(z - ZOOM_STEP)), []);
  const zoomReset = useCallback(() => setZoom(ZOOM_DEFAULT), []);

  // ---- Global shortcuts: Ctrl/Cmd + plus, minus, zero ----
  // Matched on `key`, so the combos are exactly what they look like: the
  // character the user sees. Numpad +, - and 0 produce the same `key` as their
  // main-row counterparts, and a layout where plus lives elsewhere still works,
  // because the browser reports the character that combo produces. `=` is
  // accepted alongside `+` since zooming in is normally reached without Shift.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      if ("=+".includes(e.key)) {
        e.preventDefault();
        zoomIn();
      } else if ("-_".includes(e.key)) {
        // `_` is Ctrl+Shift+- , the natural "zoom out hard" gesture.
        e.preventDefault();
        zoomOut();
      } else if (e.key === "0") {
        e.preventDefault();
        zoomReset();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [zoomIn, zoomOut, zoomReset]);

  return { zoom, zoomIn, zoomOut, zoomReset };
}
