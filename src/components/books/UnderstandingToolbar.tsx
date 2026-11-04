"use client";

/**
 * UnderstandingToolbar — the sub-toolbar row the Understanding mode adds under
 * the Books toolbar. It carries everything that used to sit inside the right
 * panel's chrome: the scope toggle (focused paragraph vs. the whole chapter),
 * the tab strip, and the LLM model picker (behind a popover, with a link to
 * the Settings page where the provider itself is configured).
 *
 * The tab and scope state lives in `useBookData` because the panel body reads
 * the same state; this component only renders the controls.
 */

import { useEffect, useRef, useState } from "react";
import { BookOpen, Cpu, Settings2, Wand2 } from "lucide-react";
import { ToolbarButton } from "@/components/ui/toolbar";
import UnderstandingModelPicker from "./UnderstandingModelPicker";
import { UNDERSTANDING_TABS, type UnderstandingTab } from "./understanding_tabs";
import { findArtifact } from "@/lib/read/artifacts";
import type { ArtifactKind } from "@/lib/read/types";
import type { useBookData } from "@/hooks/useBookData";
import type { TabId } from "@/lib/shell/tabs";

type Props = {
  b: ReturnType<typeof useBookData>;
  /** The shell's `openTab`, used by the popover's provider-settings link. */
  onNavigate: (tab: TabId) => void;
};

/** The primary generated artifact a tab's stale wand keys off (null: never stale). */
const TAB_STALE_KIND: Record<UnderstandingTab, ArtifactKind | null> = {
  digest: "digest",
  explain: "explanation",
  simplify: "simplification",
  translate: "translation",
  argument: "argument_map",
  quiz: "questions",
  notes: null,
  glossary: null,
};

export default function UnderstandingToolbar({ b, onNavigate }: Props) {
  const { understandingTab, setUnderstandingTab, chapterMode, setChapterMode, scope } = b;
  const [modelOpen, setModelOpen] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  // Click-outside closes the model popover (the picker is a native select, so
  // choosing a model also needs an explicit close).
  useEffect(() => {
    if (!modelOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!popoverRef.current?.contains(e.target as Node)) setModelOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [modelOpen]);

  // A tab shows the stale wand when its primary generated artifact is stale —
  // measured against the scope the tab actually binds to.
  const scopeArtifacts = scope.uuid ? b.artifactsByScope.get(scope.uuid) : undefined;
  const tabStale = (id: UnderstandingTab): boolean => {
    const kind = TAB_STALE_KIND[id];
    if (!kind) return false;
    return !!findArtifact(scopeArtifacts, kind)?.stale;
  };

  // The row itself must not scroll: `overflow-x-auto` would compute `overflow-y`
  // to `auto` as well and clip the model popover, which hangs below this row.
  // The tab strip is the scroller instead, so it shrinks on a narrow window and
  // the toggle and the model button keep their place.
  return (
    <div className="shrink-0 flex items-center gap-1 px-2 py-1 border-b border-border-default">
      {/* Scope toggle: understand the whole chapter instead of the focused
          paragraph. Applies to every content tab at once. */}
      <ToolbarButton
        variant="toggle"
        active={chapterMode}
        icon={BookOpen}
        label="Chapter"
        title="Understand the whole chapter instead of the focused paragraph"
        onClick={() => setChapterMode(!chapterMode)}
      />

      <div className="w-px h-4 bg-border-default shrink-0 mx-1" />

      {/* Tabs — the panel body below renders the active one. */}
      <div className="flex-1 min-w-0 flex items-center gap-1 overflow-x-auto">
        {UNDERSTANDING_TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setUnderstandingTab(t.id)}
            className={`shrink-0 px-2.5 py-1 rounded-md text-xs font-medium cursor-pointer transition-colors ${
              understandingTab === t.id
                ? "bg-accent-bg text-white"
                : "text-text-secondary hover:bg-bg-hover"
            }`}
          >
            {t.label}
            {tabStale(t.id) && <span className="ml-1 align-middle"><Wand2 size={11} className="inline text-amber-500" /></span>}
          </button>
        ))}
      </div>

      {/* Model picker behind a popover: quick model switch stays here, the
          provider configuration (base URL, keys) lives on the Settings page. */}
      <div className="relative shrink-0" ref={popoverRef}>
        <ToolbarButton
          variant="toggleSoft"
          active={modelOpen}
          icon={Cpu}
          label={b.understandingModel.trim() ? b.understandingModel.trim().split(/[/:]/)[0] : "Model"}
          title="LLM model used to generate these artifacts"
          onClick={() => setModelOpen((v) => !v)}
        />
        {modelOpen && (
          <div className="absolute right-0 top-full mt-1 z-50 w-72 rounded-lg border border-border-default bg-bg-card shadow-2xl flex flex-col">
            <div className="px-1 pt-1">
              <UnderstandingModelPicker
                value={b.understandingModel}
                onChange={(m) => {
                  b.setUnderstandingModel(m);
                  setModelOpen(false);
                }}
              />
            </div>
            <button
              className="flex items-center gap-1.5 px-3 py-2 border-t border-border-default text-xs text-text-secondary hover:bg-bg-hover cursor-pointer rounded-b-lg"
              onClick={() => {
                setModelOpen(false);
                onNavigate("settings");
              }}
            >
              <Settings2 size={12} />
              LLM provider settings…
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
