// ---------------------------------------------------------------------------
// Whether this window is an agent host.
//
// The agent surface (the dock, the `agent_*` commands, the `/mcp` server) is
// desktop-only: Android runs the same webview as a thin sync client and has
// none of those commands registered, so an `invoke` there rejects. Every
// agent-facing call is gated on this.
//
// Safe to call from effects and handlers only: `isMobileApp()` falls back to
// `false` during prerendering, so using it while rendering would break
// hydration.
// ---------------------------------------------------------------------------

import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";

export function isAgentHost(): boolean {
  return isTauri() && !isMobileApp();
}
