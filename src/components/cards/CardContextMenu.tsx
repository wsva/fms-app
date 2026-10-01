"use client";

import { useEffect, useRef, useState } from "react";
import { Plus, Search, X } from "lucide-react";
import { useCardEditor } from "./CardEditorContext";
import { invoke } from "@tauri-apps/api/core";
import type { Card } from "@/lib/types";

/**
 * Global text-selection context menu for cards.
 *
 * Ported from the website's CardContextMenu: whenever the user selects text
 * anywhere in the app (including inside <input>/<textarea>), a small floating
 * menu appears offering to create a card from the selection.
 *
 * The menu now includes:
 * - An editable input showing the selected text
 * - A search button to find existing cards with matching questions
 * - Search results displayed as clickable links to jump to existing cards
 * - An "Add to Card" button to create a new card
 *
 * Mounted once in the app shell. Renders nothing until a selection is made, so
 * the initial server/client trees match (no hydration mismatch).
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

export default function CardContextMenu() {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const [selectedText, setSelectedText] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<CardSearchResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [showSearchPanel, setShowSearchPanel] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const { openCardEditor } = useCardEditor();

  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      // Ignore interactions originating inside the menu itself.
      if (menuRef.current?.contains(e.target as Node)) return;

      const sel = window.getSelection();
      const selText = sel?.toString().trim();
      if (selText && sel?.rangeCount) {
        const rect = sel.getRangeAt(0).getBoundingClientRect();
        if (rect.width || rect.height) {
          setSelectedText(selText);
          setSearchQuery(selText);
          setSearchResults([]);
          setShowSearchPanel(false);
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
            setSearchQuery(text);
            setSearchResults([]);
            setShowSearchPanel(false);
            setAnchor({ x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom });
            return;
          }
        }
      }

      // No usable selection — dismiss.
      setAnchor(null);
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
  }, []);

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
    // Open the card editor with the selected card
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
      result.dataset_name
    );
  }

  async function handleAddCard() {
    const text = searchQuery || selectedText;
    setAnchor(null);
    if (!text) return;

    try {
      // Get the first available dataset
      const datasets = await invoke<any[]>("card_dataset_list");
      if (datasets.length === 0) {
        alert("No card datasets found. Please create one first.");
        return;
      }

      const datasetUuid = datasets[0].info.uuid;

      // Open card editor with pre-filled question
      openCardEditor(
        datasetUuid,
        {
          uuid: "",
          question: text,
          answer: "",
          suggestion: "",
          note: "",
          familiarity: 0,
          question_hash: null,
          source_card_uuid: null,
          source_dataset_uuid: null,
          deleted_at: null,
          created_at: "",
          updated_at: "",
        },
        (savedCard) => {
          console.log("Card saved from context menu:", savedCard);
        }
      );
    } catch (err) {
      console.error("Failed to open card editor:", err);
      alert(`Failed to open card editor: ${err}`);
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

  if (!anchor) return null;

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
