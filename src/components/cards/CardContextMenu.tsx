"use client";

import { useEffect, useRef, useState } from "react";
import { Loader2, Plus, Search, Wand2, X } from "lucide-react";
import { useCardEditor } from "./CardEditorContext";
import { notify, notifyError } from "@/lib/feedback/toast";
import { invoke } from "@tauri-apps/api/core";
import type { Card } from "@/lib/types";
import { PROMPT_TEMPLATES } from "@/lib/llm/prompt-templates";
import { askAboutSelection } from "@/lib/llm/ask";
import { useContextMenuEnabled } from "@/lib/ui/context-menu";
import { isTauri } from "@/lib/platform";
import { replayTaskTarget } from "@/lib/agent/tasks";
import {
  buildCapture,
  captureDetail,
  captureTarget,
  captureTitle,
  pickCaptureDeck,
  type CaptureContext,
} from "@/lib/cards/capture";

/**
 * Global text-selection context menu for cards.
 *
 * Ported from the website's CardContextMenu: whenever the user selects text
 * anywhere in the app (including inside <input>/<textarea>), a small floating
 * menu appears offering to create a card from the selection.
 *
 * The menu now includes:
 * - An editable input showing the selected text
 * - Ask-the-model actions: the prompt templates run over the selection, so a
 *   question about a word or sentence is answered where the text is
 *   (the app deliberately has no chat page of its own)
 * - A search button to find existing cards with matching questions
 * - Search results displayed as clickable links to jump to existing cards
 * - An "Add to Card" button to create a new card
 *
 * Mounted once in the app shell. Renders nothing until a selection is made, so
 * the initial server/client trees match (no hydration mismatch).
 *
 * It ships switched off: the status bar's **Context menu** button turns it on,
 * and the choice is remembered (`lib/ui/context-menu`). While it is off this
 * component attaches no listeners at all — a disabled switch should cost no
 * `mouseup` handler on every click — and nothing renders.
 */

type MenuAnchor = { x: number; top: number; bottom: number };

type CardSearchResult = {
  dataset_uuid: string;
  dataset_name: string;
  card_uuid: string;
  question: string;
  answer: string;
  note: string;
  suggestion: string;
  location: string;
};

// Flip the menu below the selection when it would clip the top edge.
const TOP_MARGIN = 56;
// Rough half-width used to keep the menu inside the viewport.
const EDGE_MARGIN = 160;

/** What a capture keeps of the field text it was selected from, in characters. */
const MAX_BLOCK = 2000;

/** Block-level elements whose text reads as "the paragraph this sentence is in". */
const BLOCK_SELECTOR = "p, li, h1, h2, h3, h4, h5, h6, blockquote, pre, td";
const HEADING_SELECTOR = "h1, h2, h3, h4, h5, h6";

/** The nearest block-level ancestor of the selection start. */
function blockElementAt(node: Node | null): Element | null {
  let el: Element | null = node instanceof Element ? node : (node?.parentElement ?? null);
  while (el) {
    if (el.matches(BLOCK_SELECTOR)) return el;
    el = el.parentElement;
  }
  return null;
}

/**
 * The nearest heading above `block`, searched outwards.
 *
 * Siblings first, then the parent's siblings, and so on: the result is the
 * closest heading the reader could have scrolled past. A document-wide
 * `querySelectorAll` would be wrong here — every tab stays mounted, so the last
 * heading in the DOM often belongs to a page that is not on screen.
 */
function nearestHeading(block: Element): string | undefined {
  for (let node: Element | null = block; node; node = node.parentElement) {
    for (let sib = node.previousElementSibling; sib; sib = sib.previousElementSibling) {
      const heads = sib.matches(HEADING_SELECTOR)
        ? [sib]
        : Array.from(sib.querySelectorAll(HEADING_SELECTOR));
      for (let i = heads.length - 1; i >= 0; i -= 1) {
        const text = (heads[i].textContent ?? "").replace(/\s+/g, " ").trim();
        if (text) return text;
      }
    }
  }
  return undefined;
}

/** The surrounding block, read while the selection still exists. */
function readBlock(range: Range): CaptureContext {
  const block = blockElementAt(range.startContainer);
  if (!block) return { block: "" };
  return {
    block: (block.textContent ?? "").slice(0, MAX_BLOCK),
    heading: nearestHeading(block),
  };
}

export default function CardContextMenu() {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const [selectedText, setSelectedText] = useState("");
  // The paragraph and heading around the selection, snapshotted at mouseup:
  // clicking the menu collapses the selection, so this cannot wait for the
  // click on "Add as new card".
  const [captureCtx, setCaptureCtx] = useState<CaptureContext | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<CardSearchResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [showSearchPanel, setShowSearchPanel] = useState(false);
  // Ask-the-model state. One question at a time: starting another replaces the
  // answer, which is what a menu over a fresh selection means.
  const [asking, setAsking] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  const [askError, setAskError] = useState("");
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const { openCardEditor } = useCardEditor();
  const enabled = useContextMenuEnabled();

  useEffect(() => {
    if (!enabled) return;
    const onMouseUp = (e: MouseEvent) => {
      // Ignore interactions originating inside the menu itself.
      if (menuRef.current?.contains(e.target as Node)) return;

      const sel = window.getSelection();
      const selText = sel?.toString().trim();
      if (selText && sel?.rangeCount) {
        const range = sel.getRangeAt(0);
        const rect = range.getBoundingClientRect();
        if (rect.width || rect.height) {
          setSelectedText(selText);
          setCaptureCtx(readBlock(range));
          setSearchQuery(selText);
          setSearchResults([]);
          setShowSearchPanel(false);
          setAnswer("");
          setAskError("");
          setAnchor({ x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom });
          return;
        }
      }

      // window.getSelection() is empty inside <input>/<textarea>; read the
      // element's own selection instead.
      const active = document.activeElement as HTMLInputElement | HTMLTextAreaElement | null;
      if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) {
        const start = active.selectionStart ?? 0;
        const end = active.selectionEnd ?? 0;
        if (start < end) {
          const text = active.value.slice(start, end).trim();
          if (text) {
            const rect = active.getBoundingClientRect();
            setSelectedText(text);
            // A field has no paragraphs to point at: its whole value is the
            // context, and there is no heading to name.
            setCaptureCtx({ block: active.value.slice(0, MAX_BLOCK) });
            setSearchQuery(text);
            setSearchResults([]);
            setShowSearchPanel(false);
            setAnswer("");
            setAskError("");
            setAnchor({ x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom });
            return;
          }
        }
      }

      // No usable selection — dismiss.
      setAnchor(null);
      setCaptureCtx(null);
    };

    const onPointerDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setAnchor(null);
      }
    };

    document.addEventListener("mouseup", onMouseUp);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("mouseup", onMouseUp);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [enabled]);

  // Focus the input when the search panel opens
  useEffect(() => {
    if (showSearchPanel && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [showSearchPanel]);

  async function handleSearch() {
    if (!searchQuery.trim()) {
      setSearchResults([]);
      return;
    }

    setIsSearching(true);
    try {
      const results = await invoke<CardSearchResult[]>("card_search", {
        query: searchQuery,
        mode: "question",
      });
      setSearchResults(results);
    } catch (err) {
      console.error("Search failed:", err);
      setSearchResults([]);
    } finally {
      setIsSearching(false);
    }
  }

  function handleSelectCard(result: CardSearchResult) {
    setAnchor(null);
    // Open the card editor with the selected card, starting in edit mode
    openCardEditor(
      result.dataset_uuid,
      {
        uuid: result.card_uuid,
        question: result.question,
        answer: result.answer,
        suggestion: result.suggestion,
        note: result.note,
        familiarity: 0,
        question_hash: null,
        source_card_uuid: null,
        source_dataset_uuid: null,
        deleted_at: null,
        created_at: "",
        updated_at: "",
      },
      (savedCard) => {
        console.log("Card saved from context menu search:", savedCard);
      },
      undefined,
      result.dataset_name,
      false
    );
  }

  /**
   * "Add as new card" files a Tasks todo, it does not open the editor.
   *
   * The reader selected a sentence while reading; making them write an answer
   * before they can go back is the interruption this path exists to avoid. The
   * todo carries everything needed to write the card later — the quote, the
   * paragraph, and where it came from — and the toast says which deck took it.
   */
  async function handleAddCard() {
    const text = searchQuery || selectedText;
    const ctx = captureCtx;
    setAnchor(null);
    if (!text) return;
    setCaptureCtx(null);

    if (!isTauri()) {
      notifyError("Adding a card needs the desktop app.");
      return;
    }

    try {
      const deck = await pickCaptureDeck();
      if (!deck) {
        // The banner rather than `alert()`: this menu is reached from a text
        // selection anywhere, and a blocking dialog that cannot be styled or
        // read on a phone is the worst way to say "nothing to add to yet".
        notify("No card dataset yet — create one on the Cards page first.");
        return;
      }

      // How many cards already ask this. The count is a nicety in the detail
      // line, never a reason to lose the capture.
      let similar = 0;
      try {
        const results = await invoke<CardSearchResult[]>("card_search", {
          query: text,
          mode: "question",
        });
        similar = results.length;
      } catch {
        similar = 0;
      }

      const capture = buildCapture(text, ctx ?? { block: text }, deck.info.uuid);
      const target = captureTarget(capture, deck.info.name);

      const created = await invoke<{ uuid: string }>("todo_add", {
        title: captureTitle(capture),
        detail: captureDetail(capture, similar),
        target,
        priority: "normal",
      });

      notify(`Added to Tasks → ${deck.info.name}`, {
        action: {
          label: "Edit now",
          // The same replay the Tasks tab uses, with the same todo id — so
          // saving from here closes the todo too, and the two paths cannot
          // drift apart.
          run: () => replayTaskTarget(target, created.uuid),
        },
      });
    } catch (err) {
      console.error("Failed to capture the selection:", err);
      notifyError(`Failed to add the card to Tasks: ${err}`);
    }
  }

  async function handleAsk(templateId: string) {
    const text = (searchQuery || selectedText).trim();
    if (!text) return;
    setAsking(templateId);
    setAnswer("");
    setAskError("");
    try {
      setAnswer(await askAboutSelection(templateId, text));
    } catch (e) {
      setAskError(e instanceof Error ? e.message : String(e));
    } finally {
      setAsking(null);
    }
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleSearch();
    } else if (e.key === "Escape") {
      setShowSearchPanel(false);
      setSearchResults([]);
    }
  }

  if (!enabled || !anchor) return null;

  const flip = anchor.top < TOP_MARGIN;
  const style: React.CSSProperties = {
    position: "fixed",
    top: flip ? anchor.bottom : anchor.top,
    left: Math.max(8, Math.min(anchor.x, window.innerWidth - EDGE_MARGIN)),
    transform: flip ? "translate(-50%, 8px)" : "translate(-50%, calc(-100% - 8px))",
    zIndex: 9999,
  };

  return (
    <div
      ref={menuRef}
      style={style}
      className="bg-bg-card border border-border-default rounded-lg shadow-lg min-w-[300px] max-w-[400px]"
    >
      {/* Search Input Section */}
      <div className="p-3 border-b border-border-default">
        <input
          ref={inputRef}
          type="text"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Search cards..."
          className="w-full px-3 py-1.5 text-sm rounded border border-border-default bg-bg-surface text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-accent"
        />
      </div>

      {/* Search Results Section */}
      {showSearchPanel && (
        <div className="max-h-[200px] overflow-y-auto border-b border-border-default">
          {isSearching ? (
            <div className="px-3 py-2 text-sm text-text-tertiary text-center">
              Searching...
            </div>
          ) : searchResults.length > 0 ? (
            <div className="py-1">
              <div className="px-3 py-1 text-xs text-text-tertiary font-medium">
                Found {searchResults.length} card{searchResults.length !== 1 ? "s" : ""}
              </div>
              {searchResults.map((result) => (
                <button
                  key={result.card_uuid}
                  onClick={() => handleSelectCard(result)}
                  className="w-full text-left px-3 py-2 hover:bg-mid-gray/20 transition-colors cursor-pointer group"
                >
                  <div className="text-sm text-text-primary truncate group-hover:text-accent transition-colors">
                    {result.question}
                  </div>
                  <div className="text-xs text-text-tertiary truncate">
                    {result.dataset_name}
                  </div>
                </button>
              ))}
            </div>
          ) : searchQuery.trim() ? (
            <div className="px-3 py-2 text-sm text-text-tertiary text-center">
              No matching cards found
            </div>
          ) : null}
        </div>
      )}

      {/* Ask the model — one template over the selection. This is where a
          "what does this word mean / parse this sentence" question lives: no
          chat page, no re-pasting, the answer sits against the text. */}
      <div className="p-3 border-b border-border-default">
        <div className="flex items-center gap-1.5 mb-1.5 text-xs font-medium text-text-tertiary">
          <Wand2 size={12} className="shrink-0" />
          Ask the model
        </div>
        <div className="flex flex-wrap gap-1">
          {PROMPT_TEMPLATES.map((t) => (
            <button
              key={t.id}
              onClick={() => void handleAsk(t.id)}
              disabled={asking !== null || !(searchQuery || selectedText).trim()}
              className="px-2 py-1 text-xs rounded border border-border-default bg-bg-muted text-text-secondary hover:bg-bg-hover hover:text-text-primary transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {asking === t.id ? <Loader2 size={12} className="animate-spin" /> : t.name.en}
            </button>
          ))}
        </div>
        {askError && (
          <p className="mt-2 text-xs text-error-text">{askError}</p>
        )}
        {answer && (
          <div className="mt-2 max-h-[220px] overflow-y-auto px-2 py-1.5 rounded border border-border-default bg-bg-surface text-xs text-text-primary whitespace-pre-wrap select-text">
            {answer}
          </div>
        )}
      </div>

      {/* Action Buttons */}
      <div className="p-1">
        {!showSearchPanel ? (
          <button
            onClick={() => {
              setShowSearchPanel(true);
              handleSearch();
            }}
            disabled={isSearching || !searchQuery.trim()}
            className="w-full text-left px-3 py-1.5 text-sm text-text-primary hover:bg-mid-gray/20 transition-colors cursor-pointer flex items-center gap-2 rounded disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Search size={14} className="shrink-0" />
            Search existing cards
          </button>
        ) : (
          <button
            onClick={() => {
              setShowSearchPanel(false);
              setSearchResults([]);
            }}
            className="w-full text-left px-3 py-1.5 text-sm text-text-secondary hover:bg-mid-gray/20 transition-colors cursor-pointer flex items-center gap-2 rounded"
          >
            <X size={14} className="shrink-0" />
            Close search
          </button>
        )}
        <button
          onClick={handleAddCard}
          className="w-full text-left px-3 py-1.5 text-sm text-text-primary hover:bg-mid-gray/20 transition-colors cursor-pointer flex items-center gap-2 rounded"
        >
          <Plus size={14} className="shrink-0" />
          Add as new card
        </button>
      </div>
    </div>
  );
}
