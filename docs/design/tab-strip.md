# In-App Tab Strip (Option A)

A browser-style tab strip inside the existing single-page shell, so several pages
can be open at once — without introducing Next.js routing and without losing the
state preservation the app depends on.

This document is grounded in the current shell
(`src/app/(app)/home/page.tsx`), the navigation table
(`src/components/layout/Sidebar.tsx`), and the three module-scope registries that
assume "one page per tab": `src/lib/refresh/page-refresh.tsx`,
`src/lib/agent/ui-actions.ts`, and `src/lib/agent/ui-context.ts`.

---

## 1. Goal and non-goals

**Goal.** Let the user keep more than one page mounted *and visible-adjacent*: a
row of tabs in the title bar (§4.2), each tab a live page instance, clicking a
tab swaps which instance is shown. Open instances are display-toggled, not
unmounted, so switching between them preserves state (playback position,
half-typed answers, scroll, in-progress edits) — the same property the shell
already relies on. Browsing **replaces the current unpinned tab by default**; a
**pin ("stick") button** on each chip protects a tab the user wants to keep, and
any tab with unsaved edits is auto-protected (§4.4). Only a replaced or closed tab
unmounts, and never one that is dirty.

**Non-goals.**
- No Next.js routes. `src/app/listen_speak/*` stay dead redirect stubs; the app
  remains one shell route (`/home`) plus the marketing site (`(site)`).
- No split-view / side-by-side panes in phase 1. One instance is *focused*
  (visible); the rest stay mounted but hidden.
- No multi-instance for pages that own agent events or ui actions (see §5). The
  freedom to open two of the same page applies to read-only-ish content pages
  first.
- No change to the MCP tool *contracts* in phase 1. `app_navigate`,
  `ui_invoke_action`, `ui_list_actions`, `app_get_ui_state` keep their signatures;
  only the shell's interpretation of `navigate` changes (§7).

---

## 2. Current architecture (what we are changing)

The shell holds one piece of navigation state:

```tsx
const [activeTab, setActiveTab] = useState<TabId>("dictation");
```

and renders every page as a permanently-mounted sibling, toggled by inline
`display`:

```tsx
<div style={{ display: activeTab === "books" ? "flex" : "none" }} className="flex-1 min-h-0">
  <BooksPage />
</div>
```

`activeTab` fans out to four consumers, all of which the tab strip must continue
to feed:

1. **`Sidebar` / `BottomNav` / `TitleBar` / `StatusBar` / `AgentDock`** — take
   `activeTab` and an `onNavigate`/`onTabChange` callback typed on `TabId`.
2. **`setUiContext({ active_tab: activeTab })`** — the shell's slice of the
   agent-visible UI context (`src/lib/agent/ui-context.ts`).
3. **`useActiveTabSource(activeTab)`** — tells the refresh module which tab is on
   screen (`src/lib/refresh/page-refresh.tsx`, module-scope `currentTab`).
4. **The `agent-action` listener** — `case "navigate": setActiveTab(action.tab)`.

Three module-scope registries key on the **tab id** and today assume at most one
live page per id:

| Registry | File | Key | Assumption |
|---|---|---|---|
| Page refresh | `page-refresh.tsx` | `Map<string, Registration>` keyed on tab | "at most one registration per tab" (comment, line 61) |
| UI actions | `ui-actions.ts` | `Map<string, UiAction>` keyed on action id, stamped with `tab` | ids unique across the app |
| UI context | `ui-context.ts` | one merged singleton patch | pages own disjoint key slices |

The tab strip breaks the *first* assumption (two `books` instances both call
`usePageRefresh("books", …)`) and would break the *second* if a page that
registers actions were multi-instanced. §6 and §8 handle both.

---

## 3. Instance key model

Today a page instance is identified by its `TabId`. Under the strip, identity
splits into two levels:

- **`TabId`** (`"books"`, `"wiki"`, …) — the *kind* of page. Unchanged; still the
  vocabulary the sidebar, the status bar, and `app_navigate` speak.
- **`instanceId`** — a unique key for one mounted instance. For singleton pages
  it equals the `TabId`. For multi-instance pages it is `` `${tabId}#${n}` ``
  (e.g. `wiki#1`, `wiki#2`), where `n` is a monotonically increasing counter the
  shell owns.

```ts
// src/lib/shell/tabs.ts (new)
export type TabId = /* re-exported from Sidebar.tsx, unchanged */;

export interface TabInstance {
  /** Stable unique key. `tabId` for singletons, `${tabId}#${n}` otherwise. */
  instanceId: string;
  /** What kind of page this is. Drives icon/label and `app_navigate` matching. */
  tabId: TabId;
  /** Human label shown on the strip; may carry a disambiguator ("Wiki (2)"). */
  title: string;
  /** Protected from `replace` when true. Auto-set when the page is dirty (§4.4). */
  pinned: boolean;
}

export interface TabStripState {
  /** Open instances, left-to-right strip order. */
  order: TabInstance[];
  /** The instanceId currently shown. Exactly one; always a member of `order`. */
  focused: string;
  /** Per-tabId counter that mints the next `#n`. Never decremented. */
  seq: Record<string, number>;
}
```

**Focus invariant.** `focused` is always an `instanceId` present in `order`.
Everything that today reads `activeTab` reads *the focused instance's `tabId`*
instead — so `setUiContext({ active_tab })`, `useActiveTabSource`, and the
sidebar highlight all keep working with a `TabId`, and only the strip internals
deal with `instanceId`.

**Mount invariant.** Every instance in `order` is rendered (display-toggled). A
page unmounts only when its tab is **closed** or **replaced** (§4.4) — both remove
it from `order` and trigger its registry cleanups (§8). A replace is refused for a
pinned or dirty tab, so unmounting never discards unsaved work.

**Default state.** On first load the strip is `[{ instanceId: "dictation",
tabId: "dictation", title: "Dictation", pinned: true }]`, `focused: "dictation"`.
The landing tab starts **pinned** so browsing never recycles the app's home page
out from under a first-run user; every tab opened afterwards starts unpinned.

---

## 4. Component structure

New files, all under the existing layout/shell area:

```
src/lib/shell/tabs.ts                  # TabInstance / TabStripState types + the pure reducer + openTab thunk
src/components/layout/TitleBarTabs.tsx # the chip region rendered INSIDE TitleBar's middle (no separate bar)
src/components/layout/PageHost.tsx     # maps an instanceId to the right <Page> element
```

`TitleBar.tsx` is also modified to host `TitleBarTabs` and to pass the tab state
through (§4.2).

### 4.1 `tabs.ts` — pure state logic

A reducer, not a hook, so the shell stays the single owner and the logic is
testable without React:

```ts
export function tabStripReducer(state: TabStripState, action: TabAction): TabStripState;

type TabAction =
  | { type: "focus"; instanceId: string }
  | { type: "add"; tabId: TabId }                         // append a new instance, focus it
  | { type: "replace"; instanceId: string; tabId: TabId } // swap a slot; caller guarantees it is clean
  | { type: "spawn"; tabId: TabId }                       // multi-instance only: add with a fresh #n
  | { type: "togglePin"; instanceId: string }
  | { type: "close"; instanceId: string }
  | { type: "move"; instanceId: string; toIndex: number };
```

- **`openTab(tabId)` is a shell thunk, not a reducer action** — the "focus,
  replace, or add?" decision of §4.4 lives here, because it consults runtime state
  the *pure* reducer must not know: whether the focused page is dirty (`canRefresh`
  from §8.1). It dispatches a primitive:
  - kind already open → `focus` its instance;
  - else focused instance replaceable (unpinned && `canRefresh() !== false`) →
    `replace`;
  - else → `togglePin` the focused instance if it was dirty (auto-pin), then `add`.
- **`add`** — append a `TabInstance` (singleton: `instanceId = tabId`; multi: fresh
  `#n` from `seq`), unpinned, and focus it. Refuses (no-op; caller toasts) past
  `MAX_OPEN_TABS`.
- **`replace`** — unmount the `instanceId` slot and mount `tabId` in its place,
  keeping strip position (minting a fresh `#n` for a multi kind). The caller
  guarantees the slot is clean and unpinned; the reducer does not re-check.
- **`spawn`** — `add` restricted to multi-instance kinds; a plain nav click never
  dispatches it.
- **`togglePin`** — flip `pinned`; a pinned tab is exempt from `replace`.
- **`close`** — remove from `order`, focus the right neighbour (or left if last);
  never close the last remaining tab.
- **`move`** — reorder `order` for drag-to-arrange (phase 3, optional).

### 4.2 Tabs live in the TitleBar, not a separate bar

There is **no second chrome row**. The chips render inside the existing title bar
(`src/components/layout/TitleBar.tsx`), in the flexible middle region that is today
an empty drag handle:

```tsx
{/* TitleBar.tsx, today */}
<div className="min-w-0 flex-1" />   // pure drag handle
```

That `flex-1` slot becomes the tab region:

```tsx
<div className="min-w-0 flex-1 flex items-stretch" data-tauri-drag-region="false">
  <TitleBarTabs /* chips + "+", overflow-x-auto */ />
  <div className="flex-1 min-w-[48px]" />   {/* leftover stays the drag handle */}
</div>
```

**Placement — menus first, tabs after (the recommended layout).** The bar keeps
its current order: identity (logo + "FmS"), then the menus (File, View, Cross
Device, Tools, Help), then the tabs in the middle region, then the window
controls. Menus stay always-visible — the `NAV_GROUPS` comment in `TitleBar.tsx`
(lines 472–481) says their whole purpose is "reaching any page from anywhere",
so hiding them behind a toggle would defeat it. This is the Chrome / VS Code
arrangement and needs no mode state.

**Rejected variant — icon + title toggles menu view vs tabs view.** Making the
identity group a switch that shows *either* the menus *or* the tabs saves
horizontal space but (a) hides the menus, a primary navigation path, (b) adds a
mode the user must track, and (c) means tabs and menus are never visible together.
Only worth it if the menu row and the tab row genuinely cannot fit — which the
overflow rule below already handles. A lighter use of the same affordance: keep
the menus put and make the **logo / "FmS" a home button** (click →
`openTab("dictation")`, or spawn a fresh scratch tab), so the identity group earns
a function without hiding anything.

**Chip contents.** Compact, to fit `--shell-titlebar-h` at the bar's 12px scale:
page icon (from the shared `TAB_DEFS` lookup, Phase 0 step 1), a short `title`, a
**pin ("stick") toggle** (lucide `Pin`, filled when `pinned`, outlined when not),
and a **close "×"** (hidden when only one tab remains). The focused chip reads as
active (brighter background); a tab auto-pinned because it was dirty (§4.4) shows
the filled pin so the user sees why it is protected. A trailing **"+"** opens the
multi-instance picker (`spawn`).

**Interaction.** Chip body `onClick` → `focus`; middle-click → `close`;
Ctrl/Cmd-click on a menu page row or a "+" entry → `spawn` for multi-instance
kinds; pin toggle → `togglePin`. `role="tablist"` on the region, `role="tab"` +
`aria-selected` per chip, `aria-pressed` on the pin — mirroring the shared
`TabBar` accessibility shape.

**Drag region.** The bar root is `data-tauri-drag-region="deep"`; the tab region
opts out (`"false"`) exactly like the menu items do, so chips stay clickable and
the *leftover* space after the last chip (the `min-w-[48px]` tail above) remains a
drag handle — double-click there still maximizes.

**Overflow.** The region is `overflow-x-auto`: with more tabs than fit, they scroll
horizontally within the middle region; the menus, identity and window controls
never scroll away. `MAX_OPEN_TABS` (§4.4) bounds how far this can go.

**Mobile.** `TitleBar` is desktop-only (the shell renders it only when `!mobile`;
`decorations:false` is meaningless on Android/iOS), so the tabs are desktop-only
for free and mobile keeps `BottomNav` driving focus through the same reducer — no
separate hiding logic.

**Prop change.** `TitleBar` today takes `activeTab: TabId` and
`onNavigate(tab: string)`. It now also needs the strip state and verb callbacks:
`tabs: TabInstance[]`, `focused: string`, and
`onFocus / onOpenTab / onClose / onTogglePin / onSpawn` (thin wrappers over the
reducer and the `openTab` thunk). `activeTab` / `onNavigate` can stay for the menu
rows' `checked` state and navigation, both derived from `focused`.

### 4.3 `PageHost.tsx` — instanceId → element

Replaces the wall of 21 `display`-toggled `<div>`s in the shell with a map. For
each instance in `order`, render its page, wrapped in the same display toggle,
keyed on `instanceId`:

```tsx
{state.order.map((inst) => (
  <div
    key={inst.instanceId}
    style={{ display: inst.instanceId === state.focused ? "flex" : "none" }}
    className="flex-1 min-h-0"
  >
    <PageFor instance={inst} focused={inst.instanceId === state.focused} />
  </div>
))}
```

`PageFor` is a `switch (inst.tabId)` returning the page component. It threads two
props every page now needs:

- **`focused: boolean`** — replaces the ad-hoc `active` prop that `DictationPage`
  and `DeviceChatPage` already take, and generalises it to every page. Pages gate
  polling, ui_state reporting, and event handling on it (§6).
- **`instanceId: string`** — the registry key for the refresh and (future)
  action registries (§8). Singleton pages can ignore it and keep passing their
  `TabId`; multi-instance pages must use it.

**Scroll memory (§1's promise, made true).** The `display` toggle keeps a page's
React state but not its scroll offsets: a pane with no layout box has no
scrolling box, so a hidden Books page came back at the top of the chapter every
time. Each instance is therefore wrapped in a `TabPane` that hands its node to
`hooks/usePreservedScroll.ts`, which records the offset of every scroller inside
the pane through one capture-phase `scroll` listener and restores them when the
pane becomes the focused one (in a layout effect, then again on the next frame
after a re-read settles). Only a pane that *remounts* starts at the top, which is
correct — a replaced or closed tab has no place to return to.

The complementary half is the refresh side: a visibility-triggered re-read must
not blank the pane (`usePageRefresh`'s module doc says so), because a spinner in
place of the content collapses the scroller and discards the offset the hook is
meant to restore.

### 4.4 Navigation resolution: focus / open / spawn / replace (pin-protected)

Default policy is **replace**: browsing recycles the current *unpinned* tab. A
**pin ("stick") button** on each chip (§4.2) protects a tab from being replaced.
This is the browser / VS Code preview-tab model made the default, with an explicit
pin toggle instead of double-click-to-pin.

Four verbs:
- **focus** — target an instance already in `order`; set it `focused`. No mount
  change, no state lost.
- **open** — the default verb for every nav source. Ensure one instance of the
  target kind exists, then focus it. If the kind is already open, `open` is
  `focus`. If not, it *places* a new instance per the placement rule below.
- **spawn** — multi-instance kinds only: always **add** a fresh instance
  (`books#2`), never replace. Explicit intent ("+", Ctrl/Cmd-click,
  `app_open_tab`).
- **togglePin** — flip `TabInstance.pinned`.

**Placement rule** — what `open` does when the target kind is *not* yet open. Let
`f` be the focused instance:
1. `f` **replaceable** (`!f.pinned` and `f`'s page is clean) → **replace** `f`:
   unmount it, mount the target in its slot, keep strip position. Count unchanged.
2. otherwise (`f` pinned, or dirty) → **add** a new tab beside `f` and focus it.

**Dirty guard = auto-pin (mandatory).** "Clean" is never guessed: the shell
consults the focused instance's `canRefresh()` from the refresh registry (§8.1) —
the same signal that already means "a re-read would discard unsaved edits". If it
returns `false`, the tab is **auto-pinned** (so the user sees *why* it was
protected) and the target is **added**, never replaced. Dictation answers, a wiki
draft, a pending book reorder therefore can never be silently unmounted. `replace`
is only ever dispatched for a tab the guard has cleared.

**Consequence to accept.** Because replace is the default, a browsed-away-from tab
that was unpinned and clean **is unmounted** — switching back to that kind is a
fresh mount, not a warm one. This is the deliberate trade the pin button exists
for: pin what you want kept warm, let the rest recycle. It is a real change from
today's always-mounted shell (§11).

**Pinned tabs** accumulate and are never replaced; the single unpinned "scratch"
tab is what browsing recycles. Pin state lives on `TabInstance.pinned`; persisting
the pinned set (and the open strip) to `localStorage` is phase 3.

**Bound.** Pinned tabs can still pile up, so `order.length` is capped
(`MAX_OPEN_TABS`, e.g. 8). On hitting the cap, `add`/`spawn` refuse and the shell
toasts ("Close or unpin a tab first") rather than evicting — eviction that
unmounts a pinned or dirty tab is exactly the loss the pin button promises
against.

**Decision table** (source × target-already-open):

| Source | Target kind already open | Target not open |
|---|---|---|
| Sidebar / TitleBar / StatusBar / BottomNav click | `focus` | replace focused if replaceable, else `add` (auto-pin if dirty) |
| Ctrl/Cmd-click or middle-click (multi kind) | `focus` | `spawn` (always add) |
| "+" picker (multi kinds) | — | `spawn` |
| Agent `app_navigate` | `focus` | same as a click: replace-if-replaceable, else `add` |
| Agent `app_open_tab` (phase 3) | — | `spawn` |
| Directed event (`open-review`, `start-dictation`, `wiki-navigate`) | `focus` | replace-if-replaceable, else `add` |
| Pin button on a chip | — | `togglePin` |

**Mobile.** The strip is hidden, so mobile is replace-only in effect: one instance
per kind, `open` swaps the visible page (today's behaviour). No pin button, no
`spawn`, no growth.

---

## 5. Multi-instance-safe vs singleton

The classification is **not** a preference — it falls directly out of which
registries a page touches. A page is *singleton* if any of these is true:

- It registers **ui actions** (`useAgentActions(<tab>, …)`) — two instances would
  write the same action ids into the `Map<string, UiAction>` and last-mount would
  win, silently. (`ui-actions.ts` line 80: `registry.set(a.id, a)`.)
- It is the **target of a directed agent event** (`agent-open-review`,
  `agent-start-dictation`, `wiki-navigate`) — a broadcast `emit` reaches *every*
  mounted listener, so two instances would both react.
- It writes **instance-specific keys into the shared `ui_context` singleton**
  (`active_dataset_uuid`, `active_media_uuid`, `cue_index`, `selection`, `scroll`)
  — two instances would fight over one merged patch (§6).

Verified against the codebase:

| Page | `useAgentActions` | Directed event target | Writes ui_context keys | Class |
|---|---|---|---|---|
| `DictationPage` | ✅ `"dictation"` | ✅ `start-dictation` | ✅ dataset/media/cue | **Singleton** |
| `CardsPage` | ✅ `"cards"` | ✅ `open-review` | (selection) | **Singleton** |
| `WorkflowPage` | ✅ `"workflow"` | — | — | **Singleton** |
| `TtsPage` (edge-tts) | ✅ `"edge-tts"` | — | — | **Singleton** |
| `ModelsPage` | ✅ `"models"` | — | — | **Singleton** |
| `DeviceChatPage` (chat) | — | ✅ `agent-prefill`-style routing | — | **Singleton** |
| `LLMChatPage` | — | (session) | — | **Singleton** (holds a live session) |
| `WikiPage` | — | ✅ `wiki-navigate` (deep link) | — | **Singleton in phase 1** → multi in phase 3 (§9) |
| `BooksPage` | — | — | — | **Multi-instance-safe** |
| `PdfReaderPage` | — | — | — | **Multi-instance-safe** |
| `OcrPage`, `SplitTextPage`, `SimpleWordsPage`, `LogPage`, `TasksPage`, `DatasetsSyncPage`, `DevicesHubPage`, `WorkspacesPage`, `SettingsPage`, `AgentPage` | — | — | — | **Multi-instance-safe** (but low value; keep singleton by default to limit surface) |

**Phase-1 multi-instance set: `books`, `pdf-reader`.** These register no ui
actions, are no directed-event target, and write no ui_context keys. `wiki` is the
natural third candidate and the user asked for it, but it *is* a deep-link target
(`wiki-navigate` → `setActiveTab("wiki")`) — so making it multi-instance requires
resolving "which wiki instance does a deep link open". That is a real design step,
deferred to phase 3 (§9) rather than bundled into the strip's first landing.

Everything else stays singleton: `open` focuses the existing instance, so you can
still only have one Dictation, one Cards. The visible change for these 19 kinds is
the strip itself, the replace-on-browse default, and the pin button — plus, for the
two multi-instance kinds, the ability to open a second Books or PDF.

The classification lives in one exported table so the reducer, the "+" picker,
and `app_navigate` all read the same source of truth:

```ts
// src/lib/shell/tabs.ts
export const MULTI_INSTANCE: ReadonlySet<TabId> = new Set<TabId>([
  "books",
  "pdf-reader",
  // "wiki", // phase 3 — needs deep-link instance resolution (§9)
]);
export function isMultiInstance(tab: TabId): boolean {
  return MULTI_INSTANCE.has(tab);
}
```

---

## 6. Focused-instance ui_state reporting

`ui-context.ts` is a module singleton: one merged patch, flushed to
`agent_report_ui_state`. Today only Dictation writes instance keys into it, and
only one Dictation exists, so there is no conflict. The strip must preserve that
"one writer per key slice" property.

**Rule: an instance reports ui_context only while it is focused.** Two parts:

1. **The shell's `active_tab`.** Report the *focused instance's `tabId`*, not the
   raw focus:
   ```tsx
   const focusedInst = state.order.find((i) => i.instanceId === state.focused)!;
   useEffect(() => {
     setUiContext({ active_tab: focusedInst.tabId });
   }, [focusedInst.tabId]);
   ```
   `active_tab` stays a `TabId`, so `ui_state.rs` `KNOWN_KEYS` and every agent
   that reads it are unaffected.

2. **Page-owned keys.** `DictationPage`, `CardsPage`, etc. already gate on their
   `active`/`focused` prop for polling; extend the same gate to their
   `useReportUiContext` / `setUiContext` calls: **only merge keys while
   `focused` is true, and clear them (`null`) on losing focus.** Because
   `setUiContext` treats `null` as "clear" (line 57: `delete current[k]`), a page
   that loses focus releases its keys instead of leaving a stale
   `active_media_uuid` behind for the next page to inherit.

   Since all instance-key writers are singletons (§5), at most one instance of
   each ever exists, and the focus gate reduces to today's behaviour for them.
   The gate matters the moment a multi-instance page *starts* writing keys — which
   is why the rule is stated now even though books/pdf write nothing.

**Optional observability (phase 3):** add an `open_tabs` key to `KNOWN_KEYS` in
`src-tauri/src/ui_state.rs` (and to the `UiContextPatch` interface), carrying
`[{ tab, instance, title, focused }]`. The shell reports it whenever `order` or
`focused` changes. This is purely additive — an agent that ignores it is
unaffected — and it is what lets an agent reason about "open a wiki beside this"
before we build `app_open_tab`.

---

## 7. `app_navigate` becomes focus-or-open

**No change to the MCP tool.** `app_navigate` in `src-tauri/src/mcp/system.rs`
(line 492) still validates the tab string against the `TabId` list and emits:

```rust
self.app.emit("agent-action", json!({ "type": "navigate", "tab": tab }))
```

The change is entirely in the shell's `agent-action` listener. Today:

```tsx
case "navigate":
  if ((action.tab as string) === "agent") break;
  setActiveTab(action.tab);
  break;
```

becomes a call into the shell's `openTab` thunk (§4.1):

```tsx
case "navigate": {
  const tab = action.tab as TabId;
  if ((tab as string) === "agent") break;      // keep the existing guard
  openTab(tab);                                 // focus / replace / add — §4.4
  break;
}
```

For a **singleton** tab, `open` finds the existing instance and focuses it —
observationally identical to `setActiveTab`. For a **multi-instance** tab, the
agent's first `app_navigate("books")` opens/focuses one Books; a second call
focuses the *same* one (it does not spawn a second instance) — `open` for an
agent-driven navigate resolves to "the tab of this kind", i.e. focus the existing
one if any, else create the first. Agents therefore never accidentally fan out
into N Books tabs.

The distinction "open a *new* instance beside" is a separate verb reserved for
phase 3's `app_open_tab` (§9); `app_navigate` deliberately keeps its
one-of-each-kind meaning so every existing prompt and recipe stays correct.

Placement obeys the same rule as a user click (§4.4): if the target kind is not
open, `openTab` **replaces the focused tab when it is unpinned and clean**, else
adds — and a dirty focused tab is auto-pinned first, so an agent navigating away
never discards the user's half-typed dictation.

**Focus-stealing softening (optional, phase 3).** When the user is mid-dictation
(`focused.tabId === "dictation"` and it has unsaved edits), an agent-driven
`open` of another kind could open the tab *unfocused*, so the agent's page is
ready without yanking the user off their answer. This mirrors the existing
philosophy in the shell comment that already refuses to navigate to `agent`.
Deferred because it needs a "would this interrupt unsaved work" signal, which is
`usePageRefresh`'s `canRefresh` — wiring that into navigate is its own step.

**Other directed actions** are unchanged and remain singleton-safe:
- `open-review` → `openTab("cards")` then re-emit `agent-open-review`. Cards is a
  singleton, so exactly one listener.
- `start-dictation` → open `dictation` (singleton) then `runUiAction(...)`.
- `invoke-action` → routes by action id through `ui_invoke_action`; unaffected by
  the strip because the action registry is keyed on ids, not visibility, and all
  action-owning pages are singletons.

---

## 8. Registry implications (the real work)

### 8.1 Refresh registry — must become instance-keyed for multi-instance pages

`page-refresh.tsx` keys `registry` on the tab id and its own comment says "at
most one registration per tab". `BooksPage` calls `usePageRefresh("books", …)` and
`WikiPage` calls `usePageRefresh("wiki", …)`. Two Books instances would collide.

**Change:** the registry key becomes the **instanceId**, and `currentTab`
(module scope) becomes **`currentInstance`**. `usePageRefresh` already takes a
`tab: string` first arg — pass `instanceId` there instead. Concretely:

- `useActiveTabSource(activeTab)` → `useActiveInstanceSource(focusedInstanceId)`.
- `usePageRefresh(tab, opts)` → `usePageRefresh(instanceId, opts)`; internally
  `useIsActiveTab` → `useIsActiveInstance` comparing `currentInstance ===
  instanceId`.
- `refreshActivePage(reason)` resolves `registry.get(currentInstance)` — refreshes
  the focused instance, exactly as F5 / `shell.refresh` does today.

Singleton pages pass their `TabId` as the instanceId (they are equal), so their
call sites are textually unchanged. Multi-instance pages pass the `instanceId`
prop from `PageFor`. This is the one place the strip forces an edit inside a page
component, and it is a one-line key change per multi-instance page.

The identity-check cleanup already in `usePageRefresh` (line 164:
`if (registry.get(tab) === registration) registry.delete(tab)`) generalises
correctly: closing an instance unmounts it, its cleanup runs, and only its own
registration is removed.

### 8.2 UI-action registry — untouched in phase 1

Because every action-registering page is a singleton (§5), there is exactly one
instance writing each action id, and `ui-actions.ts` needs no change. The registry
already survives navigation (its header comment: pages are display-toggled, not
unmounted), which is precisely the property the strip preserves.

If a multi-instance page ever needs actions (phase 3+), the upgrade path is
instance-suffixed ids (`wiki.open@2`) plus a "focused instance" default resolver
in `ui_invoke_action` so a bare `wiki.open` still hits the visible one. **Do not
build this in phase 1** — it is speculative until a real page needs it.

### 8.3 ui-context singleton — handled by the focus gate (§6), no structural change

---

## 9. MCP contract evolution (phase 3, additive)

Only if agents need to drive the multi-instance dimension. All additive; the
manual (`src-tauri/src/mcp/manual.rs`) picks new tools up from the router
automatically, and `mcp/policy.rs` must classify each (these are `UI_CONTROL`).

- **`app_open_tab { tab, focus? }`** — open a *new* instance beside the current
  one (only valid for `MULTI_INSTANCE` kinds; for singletons it degrades to
  `app_navigate`). `focus: false` opens without stealing focus.
- **`app_close_tab { tab?, instance? }`** — close an instance; refuse to close the
  last one. Undoable-by-reopen, so `RECOVERABLE` in policy, not `destructive`.
- **`app_list_tabs`** — read `open_tabs` from ui_state (or a direct getter) so an
  agent can see what is open before acting. Pairs with the `open_tabs` ui_context
  key in §6.

`ui_list_actions` already stamps each action with its `tab`, so an agent can tell
which kind owns an action; if instance-suffixed ids ever land (§8.2), the `tab`
stamp keeps them grouped.

---

## 10. Phased implementation plan

Each phase is independently shippable and leaves the app in a working state.

### Phase 0 — Groundwork (no behaviour change)
1. Export `TAB_DEFS: Record<TabId, TabDef>` (icon + label lookup) from
   `Sidebar.tsx` so the strip and sidebar share one table.
2. Create `src/lib/shell/tabs.ts`: types (§3, incl. `pinned`), `MULTI_INSTANCE`
   set (§5), the pure `tabStripReducer` and the `openTab` thunk (§4.1). Unit-test
   the reducer (focus/add/replace/spawn/togglePin/close, `MAX_OPEN_TABS`) and the
   thunk's placement branch against a stubbed `canRefresh` (clean → replace, dirty
   → auto-pin + add, pinned → add).
3. Generalise the `active` prop to `focused` + `instanceId` on `DictationPage` and
   `DeviceChatPage` (the two that already take `active`), without changing
   behaviour.

### Phase 1 — The strip, replace-default + pin, singletons only
4. Add `TitleBarTabs.tsx` and host it in `TitleBar`'s middle `flex-1` region (no
   separate bar); extend `TitleBar`'s props from `activeTab`/`onNavigate` to the
   tab state + `openTab`/focus/close/pin/spawn callbacks (§4.2). Add
   `PageHost.tsx` (§4), rendering one instance per open tab.
5. In the shell, replace `activeTab`/`setActiveTab` with the reducer state and the
   `openTab` thunk; wire every nav source (`Sidebar`, `TitleBar`, `StatusBar`,
   `AgentDock`) through `openTab`. Derive `focusedInst.tabId` for `setUiContext`
   and the sidebar highlight. Enforce `MAX_OPEN_TABS` (refuse + toast, §4.4).
6. Wire the dirty guard: `openTab` reads the focused instance's `canRefresh()`
   (§8.1) before choosing replace vs add, and auto-pins a dirty tab.
7. Convert `agent-action` `navigate` to `openTab` (§7).
8. Make the refresh registry instance-keyed (§8.1); apply the focused-instance
   ui_context gate (§6).
9. **Ship check:** with `MULTI_INSTANCE` empty, browsing replaces the unpinned tab
   and pinning protects it; a dirty Dictation auto-pins instead of being replaced.
   Verify `npx tsc --noEmit`, and manually: navigate every tab, pin/unpin, F5
   refresh, agent `app_navigate`, `start-dictation`, `open-review`.

### Phase 2 — First multi-instance pages
10. Add `books`, `pdf-reader` to `MULTI_INSTANCE` (§5).
11. Thread `instanceId` from `PageFor` into these pages' `usePageRefresh` calls so
    two instances register separately (§8.1).
12. Add the "+" picker to `TabStrip` listing multi-instance kinds.
13. **Ship check:** open two Books, switch between them, confirm each keeps its own
    scroll/selection and F5 refreshes only the focused one; confirm
    `app_navigate("books")` focuses the existing Books rather than spawning a
    third.

### Phase 3 — Deep-link page + agent multi-tab tools (optional)
14. Resolve `wiki-navigate` deep links to an instance: add a `path` hint so a deep
    link opens/focuses the wiki showing that path, then admit `wiki` to
    `MULTI_INSTANCE` (§5).
15. Add `open_tabs` to `KNOWN_KEYS` (ui_state.rs) + `UiContextPatch`; shell reports
    it on `order`/`focused` change (§6).
16. Add `app_open_tab` / `app_close_tab` / `app_list_tabs` MCP tools (§9), classify
    them in `mcp/policy.rs`, confirm the manual lists them.
17. Optional: persist the open strip + pinned set to `localStorage`;
    drag-to-reorder (`move`); unfocused-open when the user has unsaved dictation
    edits (§7). Revisit `MAX_OPEN_TABS`.

---

## 11. Risks and open questions

- **Replace-by-default unmounts pages — the headline behaviour change.** Unlike
  today's always-mounted shell, browsing away from an unpinned clean tab unmounts
  it, so switching back is a cold mount. The dirty guard (§4.4) protects unsaved
  work and pinning protects anything the user wants warm, but every singleton's
  mount effect now re-runs on re-entry (e.g. `DictationPage` reloads its dataset
  list). Confirm each singleton mounts idempotently and fast enough — this is the
  main regression surface of the replace default. If a page proves too costly to
  remount, pin it by default (as the landing `dictation` tab already is).
- **Refresh key migration is the highest-risk edit.** Every multi-instance page
  must switch its `usePageRefresh` key from `TabId` to `instanceId`; miss one and
  two instances silently share a registration (last-mount wins, first instance
  stops refreshing on activation). Mitigation: the reducer + a lint-style test
  asserting each `MULTI_INSTANCE` page passes an `instanceId`-derived key. The
  same registry now also backs the dirty guard (`canRefresh`), so a wrong key
  would let a dirty tab be replaced — double reason to test it.
- **Memory.** Each extra instance is a fully-mounted page (a second Books holds a
  second media list, a second PDF holds a second document). Cap `order.length`
  (e.g. 8) and surface the cap in the "+" picker. Singleton-by-default already
  bounds the common case.
- **Mobile.** The tabs live in `TitleBar`, which the shell renders only when
  `!mobile`, so they are desktop-only for free; `BottomNav` keeps driving focus
  through the same reducer. Note `books` *is* in `MOBILE_VISIBLE_TABS` (`Sidebar.tsx`) while
  `pdf-reader` is not — so on a phone, tapping Books still routes through `open`,
  which for a multi-instance kind must resolve to "focus the one Books, else open
  the first", never "spawn a second". Because the "+" picker is desktop-only,
  mobile can only ever hold one instance per kind; multi-instance fan-out is out
  of scope there.
- **Does `focused` belong in the URL?** Not in phase 1–2. If deep-linkable focus is
  ever wanted, `history.replaceState` on focus change is additive and does not
  require real routes.
- **Strict-mode double mount.** The refresh registry already guards with an
  identity check (line 164); confirm the reducer's `open` is idempotent under a
  double-invoked effect so a dev-mode remount does not mint `books#2`.
