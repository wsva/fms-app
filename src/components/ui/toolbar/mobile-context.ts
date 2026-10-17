"use client";

import { createContext, useContext } from "react";

/**
 * Mobile flag shared down the toolbar tree so `ToolbarButton` can collapse its
 * label without every page prop-drilling `mobile`. `<Toolbar>` provides it; a
 * button rendered outside a `<Toolbar>` gets `undefined` and falls back to the
 * `useIsMobile()` hook.
 */
export const MobileContext = createContext<boolean | undefined>(undefined);

export function useToolbarMobile(): boolean | undefined {
  return useContext(MobileContext);
}
