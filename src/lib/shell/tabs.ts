// ---------------------------------------------------------------------------
// The tab strip's state model: pure, framework-free, unit-testable.
//
// The shell (`app/(app)/home/page.tsx`) owns one `TabStripState` through a
// `useReducer`, renders the chips through `components/layout/TitleBarTabs.tsx`
// and the pages through `components/layout/PageHost.tsx`. Nothing here imports
// React or a component — the only cross-module reference is the `TabId` *type*
// from the sidebar, so a test can exercise the reducer without pulling in the
// icon tree.
//
// Two levels of identity (see docs/design/tab-strip.md §3):
//   TabId        the *kind* of page ("books", "wiki"); the sidebar / menu /
//                `app_navigate` vocabulary.
//   instanceId   one mounted instance. Equals the TabId for a singleton;
//                `${tabId}#${n}` for a multi-instance kind.
//
// Navigation policy (§4.4) is replace-by-default: browsing recycles the focused
// *unpinned, clean* tab; a pinned tab is protected. The "is it clean?" question
// is runtime state (the refresh registry's `canRefresh`), so the focus / replace
// / add decision lives in `planOpen`, which the shell calls with the dirtiness
// it read — the reducer itself stays pure and only executes the chosen verb.
// ---------------------------------------------------------------------------

import type { TabId } from "@/components/layout/Sidebar";

export type { TabId };

/** One open page instance. */
export interface TabInstance {
  /** Stable unique key. `tabId` for singletons, `${tabId}#${n}` otherwise. */
  instanceId: string;
  /** What kind of page this is. Drives icon/label and `app_navigate` matching. */
  tabId: TabId;
  /** Human label shown on the chip (from `TAB_DEFS`), set by the caller. */
  title: string;
  /** Protected from `replace` when true. Auto-set when the page is dirty. */
  pinned: boolean;
}

export interface TabStripState {
  /** Open instances, left-to-right chip order. */
  order: TabInstance[];
  /** The instanceId currently shown. Always a member of `order`. */
  focused: string;
  /** Per-tabId counter that mints the next `#n`. Never decremented. */
  seq: Record<string, number>;
}

/**
 * Kinds a user may open more than one of. Deliberately empty for now: making a
 * page multi-instance requires it to register no ui actions, be no directed-event
 * target, and key its refresh registration on `instanceId` rather than `TabId`
 * (docs/design/tab-strip.md §5, §8). `books` and `pdf-reader` are the first
 * candidates once that instance-keying lands.
 */
export const MULTI_INSTANCE: ReadonlySet<TabId> = new Set<TabId>([
  // "books",
  // "pdf-reader",
]);

export function isMultiInstance(tab: TabId): boolean {
  return MULTI_INSTANCE.has(tab);
}

/** Hard bound on `order.length`; `add`/`spawn` refuse past it (caller toasts). */
export const MAX_OPEN_TABS = 8;

/** The instanceId a fresh instance of `tab` with counter `n` would get. */
export function makeInstanceId(tab: TabId, n: number): string {
  return isMultiInstance(tab) ? `${tab}#${n}` : tab;
}

/** The strip the shell starts with: the landing tab, pinned so browsing never
 *  recycles the app's home page out from under a first-run user. */
export function initialTabStrip(): TabStripState {
  return {
    order: [
      { instanceId: "dictation", tabId: "dictation", title: "Dictation", pinned: true },
    ],
    focused: "dictation",
    seq: { dictation: 1 },
  };
}

export type TabAction =
  | { type: "focus"; instanceId: string }
  | { type: "add"; tabId: TabId; title: string }
  | { type: "replace"; instanceId: string; tabId: TabId; title: string }
  | { type: "spawn"; tabId: TabId; title: string }
  | { type: "togglePin"; instanceId: string }
  | { type: "close"; instanceId: string }
  | { type: "move"; instanceId: string; toIndex: number };

function nextSeq(state: TabStripState, tab: TabId): number {
  return (state.seq[tab] ?? 0) + 1;
}

function makeInstance(tab: TabId, title: string, n: number): TabInstance {
  return { instanceId: makeInstanceId(tab, n), tabId: tab, title, pinned: false };
}

export function tabStripReducer(state: TabStripState, action: TabAction): TabStripState {
  switch (action.type) {
    case "focus": {
      if (!state.order.some((i) => i.instanceId === action.instanceId)) return state;
      if (state.focused === action.instanceId) return state;
      return { ...state, focused: action.instanceId };
    }

    case "add": {
      if (state.order.length >= MAX_OPEN_TABS) return state;
      // A singleton is never added twice — `planOpen` focuses an existing one —
      // but guard anyway so a stray dispatch cannot duplicate the key.
      if (!isMultiInstance(action.tabId) && state.order.some((i) => i.tabId === action.tabId)) {
        return state;
      }
      const n = nextSeq(state, action.tabId);
      const inst = makeInstance(action.tabId, action.title, n);
      return {
        order: [...state.order, inst],
        focused: inst.instanceId,
        seq: { ...state.seq, [action.tabId]: n },
      };
    }

    case "spawn": {
      if (!isMultiInstance(action.tabId)) return state; // singletons cannot spawn
      if (state.order.length >= MAX_OPEN_TABS) return state;
      const n = nextSeq(state, action.tabId);
      const inst = makeInstance(action.tabId, action.title, n);
      return {
        order: [...state.order, inst],
        focused: inst.instanceId,
        seq: { ...state.seq, [action.tabId]: n },
      };
    }

    case "replace": {
      const idx = state.order.findIndex((i) => i.instanceId === action.instanceId);
      if (idx < 0) return state;
      const n = nextSeq(state, action.tabId);
      const inst = makeInstance(action.tabId, action.title, n);
      const order = state.order.slice();
      order[idx] = inst;
      return { order, focused: inst.instanceId, seq: { ...state.seq, [action.tabId]: n } };
    }

    case "togglePin": {
      let changed = false;
      const order = state.order.map((i) => {
        if (i.instanceId !== action.instanceId) return i;
        changed = true;
        return { ...i, pinned: !i.pinned };
      });
      return changed ? { ...state, order } : state;
    }

    case "close": {
      // Never close the last remaining tab: the shell must always show a page.
      if (state.order.length <= 1) return state;
      const idx = state.order.findIndex((i) => i.instanceId === action.instanceId);
      if (idx < 0) return state;
      const order = state.order.filter((i) => i.instanceId !== action.instanceId);
      let focused = state.focused;
      if (state.focused === action.instanceId) {
        // Focus the right neighbour, or the new last tab when we closed the end.
        focused = order[Math.min(idx, order.length - 1)].instanceId;
      }
      return { ...state, order, focused };
    }

    case "move": {
      const from = state.order.findIndex((i) => i.instanceId === action.instanceId);
      if (from < 0) return state;
      const to = Math.max(0, Math.min(action.toIndex, state.order.length - 1));
      if (from === to) return state;
      const order = state.order.slice();
      const [moved] = order.splice(from, 1);
      order.splice(to, 0, moved);
      return { ...state, order };
    }

    default:
      return state;
  }
}

/** What `openTab` decided to do, so the shell can dispatch one primitive. */
export type Placement =
  | { kind: "focus"; instanceId: string }
  | { kind: "replace"; instanceId: string }
  /** Append a new tab; `autoPin` is a dirty focused tab to protect first. */
  | { kind: "add"; autoPin?: string };

/**
 * The focus / replace / add decision of §4.4, computed purely.
 *
 * `focusedDirty` is read by the caller from the refresh registry (`canRefresh`
 * returning false means unsaved edits). When the focused tab is dirty it is never
 * replaced — it is auto-pinned so the user can see why, and the target is added.
 */
export function planOpen(
  state: TabStripState,
  tabId: TabId,
  focusedDirty: boolean
): Placement {
  // Already open → focus it (for a multi kind, its last-focused instance is the
  // one the shell tracks as `focused` when the kind is on screen; otherwise the
  // first match, which is the stable "the X tab" an agent means).
  const existing = state.order.find((i) => i.tabId === tabId);
  if (existing) return { kind: "focus", instanceId: existing.instanceId };

  const f = state.order.find((i) => i.instanceId === state.focused);
  // No focused tab (should not happen) → just add.
  if (!f) return { kind: "add" };
  // Focused tab is replaceable: unpinned and clean → recycle its slot.
  if (!f.pinned && !focusedDirty) return { kind: "replace", instanceId: f.instanceId };
  // Pinned → add beside it. Dirty → auto-pin it, then add.
  if (!f.pinned && focusedDirty) return { kind: "add", autoPin: f.instanceId };
  return { kind: "add" };
}
