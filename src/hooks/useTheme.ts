"use client";

import { useCallback, useEffect, useState } from "react";
import {
  applyTheme,
  getStoredTheme,
  THEME_CHANGED_EVENT,
  type ThemeId,
} from "@/lib/theme";

/**
 * The applied theme, kept live across every surface that reads or writes it.
 *
 * The initial value is deliberately `light` rather than the stored one: the
 * static export is prerendered as `light`, so a first client render that already
 * knew the real theme would disagree with the server HTML. The mount effect
 * converges a frame later — the same deferral `isMobileApp()` uses.
 *
 * The listener is what makes this safe to call from two places at once (the
 * settings swatches and the title bar's View menu): pages are never unmounted in
 * this shell, so a copy that only read localStorage on mount would go stale the
 * moment the other one changed the theme.
 */
export function useTheme() {
  const [theme, setTheme] = useState<ThemeId>("light");

  useEffect(() => {
    const sync = (value: ThemeId) => setTheme(value);
    sync(getStoredTheme());
    const onChanged = (e: Event) => {
      const detail = (e as CustomEvent<ThemeId>).detail;
      if (detail) sync(detail);
    };
    window.addEventListener(THEME_CHANGED_EVENT, onChanged);
    return () => window.removeEventListener(THEME_CHANGED_EVENT, onChanged);
  }, []);

  const selectTheme = useCallback((next: ThemeId) => {
    applyTheme(next);
    setTheme(next);
  }, []);

  return { theme, setTheme: selectTheme };
}
