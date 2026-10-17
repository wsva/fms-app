"use client";

import { createContext, useContext } from "react";

/**
 * Lets a `ToolbarMenuItem` close its owning `ToolbarOverflow` on click without
 * the caller wiring state. Provided by `ToolbarOverflow`; null when a menu item
 * is rendered standalone.
 */
export const OverflowContext = createContext<(() => void) | null>(null);

export function useOverflowClose(): (() => void) | null {
  return useContext(OverflowContext);
}
