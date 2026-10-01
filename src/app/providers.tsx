"use client";

import { CardEditorProvider } from "@/components/cards/CardEditorContext";

export default function Providers({ children }: { children: React.ReactNode }) {
  return <CardEditorProvider>{children}</CardEditorProvider>;
}
