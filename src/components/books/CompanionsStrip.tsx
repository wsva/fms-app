"use client";

// ---------------------------------------------------------------------------
// A book's companions: the other datasets that share its family key.
//
// This strip is the one place the `<type>_<family key>` relation is *written* —
// every other surface only reads it. One chip per companion type: an existing
// member shows its name and opens the page that owns it; a missing one offers to
// create it, which mints `<type>_<the book's key>` and so joins the family with
// nothing stored anywhere.
//
// A book whose id predates the grammar has no key to share, so the strip stays
// hidden rather than offering companions that could never be linked back to it.
// Re-keying an existing book is a migration, not a button (docs/design/
// dataset-info.md): the uuid keys progress, XP and sync state, so editing it in
// place would orphan all three.
// ---------------------------------------------------------------------------

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { BookMarked, Copy, Headphones, Mic, Plus } from "lucide-react";

import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import { btnSm, btnSmSecondary } from "@/lib/datasets/types";
import { familyMember, type DatasetTypeSlug } from "@/lib/datasets/family";
import { useDatasetFamily } from "@/hooks/useDatasetFamily";
import { openDatasetInTab } from "@/lib/nav/open-in-tab";
import type { TabId } from "@/lib/shell/tabs";

/** One companion slot: what to call it, which page owns it, and how to make one.
 *  The `book` type is absent because the book is the anchor, not its own
 *  companion. Order is the order the chips render in. */
interface CompanionDef {
  slug: DatasetTypeSlug;
  label: string;
  icon: typeof Headphones;
  tab: TabId;
  /** The page's "open this dataset" UI action, run after the tab is opened. */
  action: string;
  /** The create command; every one of them takes `{ name, family }`. */
  command: string;
}

const COMPANIONS: readonly CompanionDef[] = [
  {
    slug: "dictation",
    label: "Dictation",
    icon: Headphones,
    tab: "dictation",
    action: "dictation.open-dataset",
    command: "dataset_create",
  },
  {
    slug: "card",
    label: "Cards",
    icon: Copy,
    tab: "cards",
    action: "cards.select-dataset",
    command: "card_dataset_create",
  },
  {
    slug: "read_aloud",
    label: "Read Aloud",
    icon: Mic,
    tab: "read-aloud",
    action: "read-aloud.open-dataset",
    command: "read_aloud_create",
  },
  {
    slug: "wiki",
    label: "Wiki",
    icon: BookMarked,
    tab: "wiki",
    action: "wiki.open-dataset",
    command: "wiki_dataset_create",
  },
];

interface CompanionsStripProps {
  /** The open book's dataset uuid. */
  bookUuid: string;
  /** The book's name, used to name a companion it does not have yet. */
  bookName: string;
  /** The shell's `openTab`, so a chip can open the page that owns a member. */
  onNavigate: (tab: TabId) => void;
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function CompanionsStrip({ bookUuid, bookName, onNavigate }: CompanionsStripProps) {
  const { report, refresh } = useDatasetFamily(bookUuid);
  const [busy, setBusy] = useState<DatasetTypeSlug | null>(null);
  const [error, setError] = useState<string | null>(null);

  // No key, no family: a legacy book id cannot mint a sibling that points back
  // at it, so there is nothing to offer.
  const key = report?.family;
  if (!key) return null;

  const open = async (def: CompanionDef, uuid: string) => {
    setError(null);
    try {
      await openDatasetInTab(onNavigate, def.tab, def.action, uuid);
    } catch (err) {
      // The tab did open; only the hand-off failed, so say which dataset was
      // meant and let the user pick it from the page's own list.
      const msg = messageOf(err);
      logError(`open ${def.slug} companion '${uuid}' failed: ${msg}`, "family");
      setError(`Could not open the ${def.label} dataset: ${msg}`);
    }
  };

  const create = async (def: CompanionDef) => {
    if (!isTauri() || busy) return;
    setBusy(def.slug);
    setError(null);
    try {
      await invoke(def.command, { name: `${bookName} (${def.label})`, family: key });
      // The create commands do not emit this themselves — only their MCP twins
      // do — and every page's refresh listens for it.
      await emit("dataset-list-changed", {});
      refresh();
    } catch (err) {
      const msg = messageOf(err);
      logError(`create ${def.slug} companion in family '${key}' failed: ${msg}`, "family");
      setError(msg);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 border-b border-border-default bg-bg-card overflow-x-auto shrink-0">
      <span className="text-xs text-text-tertiary shrink-0">Companions</span>
      {COMPANIONS.map((def) => {
        const member = familyMember(report, def.slug);
        const Icon = def.icon;
        if (member) {
          return (
            <button
              key={def.slug}
              type="button"
              className={`${btnSmSecondary} flex items-center gap-1.5 shrink-0`}
              title={`Open the ${def.label} dataset of this book`}
              onClick={() => void open(def, member.uuid)}
            >
              <Icon size={13} className="shrink-0 opacity-70" />
              <span className="truncate max-w-[180px]">{member.name}</span>
            </button>
          );
        }
        return (
          <button
            key={def.slug}
            type="button"
            className={`${btnSm} flex items-center gap-1.5 shrink-0 bg-transparent border border-dashed border-border-light text-text-tertiary hover:bg-bg-hover hover:text-text-primary disabled:opacity-50`}
            title={`Create the ${def.label} dataset of this book — it joins the same family`}
            disabled={busy !== null}
            onClick={() => void create(def)}
          >
            <Icon size={13} className="shrink-0 opacity-60" />
            <span>{busy === def.slug ? "Creating…" : def.label}</span>
            <Plus size={12} className="shrink-0 opacity-60" />
          </button>
        );
      })}
      {error && (
        <span className="text-xs text-error-text truncate shrink min-w-0" title={error}>
          {error}
        </span>
      )}
    </div>
  );
}
