// ---------------------------------------------------------------------------
// Dock toggle seam
// ---------------------------------------------------------------------------
// The status bar owns a button; the dock owns the state. Instead of lifting the
// dock's presentation state into a store both import — which would re-render the
// shell on every mailbox poll and couple two distant components through data —
// they talk through two window events, the same fire-and-forget seam
// `usePageZoom` uses for `page-zoom-changed`.
//
// Order-independent by construction: the dock broadcasts its state on every
// change (including its mount), so whoever listens later still converges, and a
// dispatch with no listener is simply a no-op.
// ---------------------------------------------------------------------------

/** StatusBar → AgentDock: flip the dock open/closed. */
export const DOCK_TOGGLE_EVENT = "agent-dock-toggle";

/** AgentDock → StatusBar: the presentation state the button mirrors. */
export const DOCK_STATE_EVENT = "agent-dock-state";

export interface DockStateDetail {
  /** Whether the dock is currently open. */
  open: boolean;
  /** The attention count from `badgeOf`, or null when nothing needs flagging. */
  badge: string | null;
  /** Output arrived while the dock was closed. */
  unread: boolean;
}

/** The state a freshly mounted listener assumes before the dock reports:
 *  closed, nothing flagged — which is also what a fresh install looks like. */
export const DOCK_STATE_DEFAULT: DockStateDetail = {
  open: false,
  badge: null,
  unread: false,
};

export function dispatchDockToggle() {
  window.dispatchEvent(new CustomEvent(DOCK_TOGGLE_EVENT));
}

export function dispatchDockState(detail: DockStateDetail) {
  window.dispatchEvent(new CustomEvent(DOCK_STATE_EVENT, { detail }));
}
