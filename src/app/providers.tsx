"use client";

import { CardEditorProvider } from "@/components/cards/CardEditorContext";
import { usePageZoom } from "@/hooks/usePageZoom";

export default function Providers({ children }: { children: React.ReactNode }) {
  // Global Ctrl/Cmd +/-/0 page zoom. Registered here so it wraps every route;
  // `children` keeps its identity across zoom changes, so the tree below does
  // not re-render.
  usePageZoom();

  return <CardEditorProvider>{children}</CardEditorProvider>;
}
