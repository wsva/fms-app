"use client";

// ---------------------------------------------------------------------------
// What each open tab is actually showing.
//
// A `TabInstance` knows its *kind* ("cards") and its chip label ("Cards"), which
// is enough while a tab names a page but not once it names a thing inside the
// page: two cards tabs are indistinguishable, the address bar says `/home`, and
// an agent asking "which card is on screen?" gets nothing. So the subject lives
// beside the strip rather than inside it — a module-scope store keyed on the tab,
// published by the page (or the card editor overlay) that owns the thing.
//
// Keyed by `TabId` rather than `instanceId` because every kind that reports a
// subject is a singleton today (`MULTI_INSTANCE` is empty in lib/shell/tabs.ts).
// When a kind becomes multi-instance its publishers must switch to the instance
// id, which is the same migration `usePageRefresh` faces (docs/design/tab-strip.md
// §5, §8.1) — the store accepts either as the key and does not care.
//
// The store has three consumers, none of which may import another: the tab strip
// chips (a name to show), the address bar (a name to restore from), and the agent
// context (a name to act on). Same reason `lib/agent/ui-actions.ts` is a module
// singleton and not a provider.
// ---------------------------------------------------------------------------

import { useSyncExternalStore } from "react";

/** The thing a tab is showing, if it has one worth naming. */
export type TabSubject =
  | { kind: "dataset"; datasetUuid: string; datasetName: string }
  | {
      kind: "card";
      datasetUuid: string;
      /** Optional because a card can be opened before its dataset list lands. */
      datasetName?: string;
      cardUuid: string;
      /** A short human label — the card's question, anchors stripped. */
      label?: string;
    };

const subjects = new Map<string, TabSubject>();
const subscribers = new Set<() => void>();

function emit(): void {
  subscribers.forEach((notify) => notify());
}

function same(a: TabSubject | undefined, b: TabSubject | null): boolean {
  if (!a || !b) return a === b;
  // By value, so a publisher that rebuilds its object every render does not
  // count as a change — `useSyncExternalStore` compares snapshots by identity.
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Publish (or with `null`, withdraw) what `tab` is showing. Idempotent by
 *  value: re-publishing the same subject notifies nobody. */
export function setTabSubject(tab: string, subject: TabSubject | null): void {
  if (same(subjects.get(tab) ?? undefined, subject)) return;
  if (subject === null) subjects.delete(tab);
  else subjects.set(tab, subject);
  emit();
}

/** Withdraw a subject only if it is still the one `publisher` published — a tab
 *  that unmounts late must not clear the subject its replacement just claimed. */
export function clearTabSubject(tab: string, subject: TabSubject | null): void {
  if (same(subjects.get(tab) ?? undefined, subject)) setTabSubject(tab, null);
}

function subscribe(notify: () => void): () => void {
  subscribers.add(notify);
  return () => {
    subscribers.delete(notify);
  };
}

/** The subject of one tab, re-rendering the caller when it changes. */
export function useTabSubject(tab: string | null): TabSubject | null {
  const getSnapshot = () => (tab ? subjects.get(tab) ?? null : null);
  return useSyncExternalStore(subscribe, getSnapshot, () => null);
}

/** Every tab that has a subject. Read imperatively by callers outside React. */
export function allTabSubjects(): Record<string, TabSubject> {
  return Object.fromEntries(subjects);
}

/** The chip's own label from the subject, dataset first: `der Lernende · FmS cards`.
 *  Falls back to null when there is nothing to say, so the caller keeps its
 *  page title. */
export function subjectLabel(subject: TabSubject): string | null {
  if (subject.kind === "card") {
    const head = subject.label?.trim() || "untitled card";
    return subject.datasetName ? `${head} · ${subject.datasetName}` : head;
  }
  return subject.datasetName || null;
}

/** Truncate a card's question into a label worth putting on a chip. */
export function cardSubjectLabel(question: string, max = 40): string {
  const flat = question.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  return `${flat.slice(0, max - 1).trimEnd()}…`;
}

// ---- the address bar -------------------------------------------------------
//
// The shell is display-toggled, not routed (docs/design/tab-strip.md §8, closing
// note): a card has no URL of its own unless the shell writes one. `replaceState`
// rather than `pushState` because the address should name what is on screen and
// must not become a back-step history for every keystroke of a browse. Two uuids
// are enough to reopen a card — `card_get` needs no list — which is what lets a
// restored window show a card before its dataset list has even loaded.

const PARAM_TAB = "tab";
const PARAM_DATASET = "dataset";
const PARAM_CARD = "card";

/** Is the document at the app shell route? The static export serves the
 *  marketing pages off the same bundle, and those must not grow a `?tab=`. */
function onAppShellRoute(): boolean {
  if (typeof window === "undefined") return false;
  const path = window.location.pathname.replace(/\/+$/, "");
  return path === "/home" || path.endsWith("/home");
}

/** Rewrite the address to name `tab` and, when it has one, its subject. */
export function applySubjectToUrl(tab: string | null, subject: TabSubject | null): void {
  if (!onAppShellRoute() || !tab) return;
  const url = new URL(window.location.href);
  const set = (key: string, value: string | null) => {
    if (value === null) url.searchParams.delete(key);
    else if (url.searchParams.get(key) !== value) url.searchParams.set(key, value);
  };
  set(PARAM_TAB, tab);
  set(PARAM_DATASET, subject ? subject.datasetUuid : null);
  set(PARAM_CARD, subject?.kind === "card" ? subject.cardUuid : null);
  if (url.pathname + url.search + url.hash === window.location.pathname + window.location.search + window.location.hash) return;
  window.history.replaceState(window.history.state, "", url.toString());
}

/** The `{dataset, card}` pair the address names, or null when it names neither. */
export function readCardFromUrl(): { datasetUuid: string; cardUuid: string } | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const datasetUuid = params.get(PARAM_DATASET);
  const cardUuid = params.get(PARAM_CARD);
  if (!datasetUuid || !cardUuid) return null;
  return { datasetUuid, cardUuid };
}
