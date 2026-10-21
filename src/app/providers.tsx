"use client";

import { CardEditorProvider } from "@/components/cards/CardEditorContext";
import { usePageZoom } from "@/hooks/usePageZoom";
import { useMdFont } from "@/lib/md-font";

export default function Providers({ children }: { children: React.ReactNode }) {
  // Global Ctrl/Cmd +/-/0 page zoom. Registered here so it wraps every route;
  // `children` keeps its identity across zoom changes, so the tree below does
  // not re-render.
  usePageZoom();

  // Reading font for markdown prose (wiki + cards), chosen in Settings →
  // Fonts. Applied here so a stored pick takes effect without the user ever
  // opening Settings; the Settings section drives the same state.
  useMdFont();

  return <CardEditorProvider>{children}</CardEditorProvider>;
}
