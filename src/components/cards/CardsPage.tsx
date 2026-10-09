"use client";

import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  Plus,
  RefreshCw,
  Search,
  ChevronLeft,
  ChevronRight,
  FlipHorizontal,
  CircleHelp,
  Frown,
  Meh,
  Smile,
  Laugh,
  SkipForward,
  Check,
  X,
  Tag,
  ExternalLink,
  Settings,
  RefreshCcw,
  Trash2,
  FolderOpen,
  FolderPlus,
  MapPin,
} from "lucide-react";
import type {
  CardDatasetSummary,
  Card,
  CardReview,
  CardTag,
  CardTestStats,
} from "@/lib/types";
import { CARD_BASE_URL, CARD_LINKS, openCardUrl } from "@/lib/cards";
import { isMobileApp } from "@/lib/platform";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import { useCardEditor } from "./CardEditorContext";

type TabId = "cards" | "search" | "review" | "tags" | "online" | "advanced";

export default function CardsPage() {
  // Dataset state
  const [datasets, setDatasets] = useState<CardDatasetSummary[]>([]);
  const [selectedDatasetUuid, setSelectedDatasetUuid] = useState<string>("");
  const [loading, setLoading] = useState(true);

  // Card list state
  const [cards, setCards] = useState<Card[]>([]);
  const [totalCards, setTotalCards] = useState(0);
  const [page, setPage] = useState(0);
  const [keyword, setKeyword] = useState("");
  const [filter, setFilter] = useState<"all" | "normal" | "easy" | "incomplete">("all");
  const pageSize = 50;

  // Active tab
  const [activeTab, setActiveTab] = useState<TabId>("cards");

  // Use global card editor
  const { openCardEditor } = useCardEditor();

  // Review state
  const [reviewCard, setReviewCard] = useState<Card | null>(null);
  const [reviewFlipped, setReviewFlipped] = useState(false);
  const [reviewResult, setReviewResult] = useState<CardReview | null>(null);
  const [reviewStats, setReviewStats] = useState<CardTestStats | null>(null);

  // Tags
  const [tags, setTags] = useState<CardTag[]>([]);

  // Global search state
  const [globalSearchQuery, setGlobalSearchQuery] = useState("");
  const [globalSearchResults, setGlobalSearchResults] = useState<
    { dataset_uuid: string; dataset_name: string; card_uuid: string; question: string; answer: string; note: string; location: string }[]
  >([]);
  const [searching, setSearching] = useState(false);
  const [searchMode, setSearchMode] = useState<"question" | "fulltext">("fulltext");
  const [rebuildingIndex, setRebuildingIndex] = useState(false);

  // Online links
  const [error, setError] = useState<string | null>(null);

  // Load datasets on mount
  const loadDatasets = useCallback(async () => {
    try {
      setLoading(true);
      const result = await invoke<CardDatasetSummary[]>("card_dataset_list");
      setDatasets(result);
      if (result.length > 0 && !selectedDatasetUuid) {
        setSelectedDatasetUuid(result[0].info.uuid);
      }
    } catch (e) {
      setError(`Failed to load datasets: ${e}`);
    } finally {
      setLoading(false);
    }
  }, [selectedDatasetUuid]);

  useEffect(() => {
    loadDatasets();
  }, [loadDatasets]);

  // Load cards when dataset changes
  const loadCards = useCallback(async () => {
    if (!selectedDatasetUuid) return;
    try {
      const [cardList, total] = await invoke<[Card[], number]>("card_list", {
        datasetUuid: selectedDatasetUuid,
        filter: filter === "all" ? null : filter,
        keyword: keyword || null,
        limit: pageSize,
        offset: page * pageSize,
      });
      setCards(cardList);
      setTotalCards(total);
    } catch (e) {
      setError(`Failed to load cards: ${e}`);
    }
  }, [selectedDatasetUuid, filter, keyword, page]);

  useEffect(() => {
    if (selectedDatasetUuid) {
      loadCards();
    }
  }, [loadCards, selectedDatasetUuid]);

  // Load tags when dataset changes
  const loadTags = useCallback(async () => {
    if (!selectedDatasetUuid) return;
    try {
      const result = await invoke<CardTag[]>("card_tag_list", {
        datasetUuid: selectedDatasetUuid,
      });
      setTags(result);
    } catch (e) {
      // Tags may not exist yet
    }
  }, [selectedDatasetUuid]);

  useEffect(() => {
    if (selectedDatasetUuid && activeTab === "tags") {
      loadTags();
    }
  }, [selectedDatasetUuid, activeTab, loadTags]);

  // Queue counts for the Review tab. Refreshed with every draw so the numbers
  // describe the same pool card_test_get just picked from.
  const loadReviewStats = useCallback(async () => {
    if (!selectedDatasetUuid) return;
    try {
      setReviewStats(await invoke<CardTestStats>("card_test_stats", {
        datasetUuid: selectedDatasetUuid,
      }));
    } catch {
      // Informational only, so a failure just hides the bar.
      setReviewStats(null);
    }
  }, [selectedDatasetUuid]);

  // Get next review card
  const loadNextReview = useCallback(async () => {
    if (!selectedDatasetUuid) return;
    try {
      const result = await invoke<[Card, CardReview] | null>("card_test_get", {
        datasetUuid: selectedDatasetUuid,
      });
      if (result) {
        setReviewCard(result[0]);
      } else {
        setReviewCard(null);
      }
      setReviewFlipped(false);
      setReviewResult(null);
    } catch (e) {
      setError(`Failed to load review card: ${e}`);
    } finally {
      void loadReviewStats();
    }
  }, [selectedDatasetUuid, loadReviewStats]);

  useEffect(() => {
    if (selectedDatasetUuid && activeTab === "review") {
      loadNextReview();
    }
  }, [selectedDatasetUuid, activeTab, loadNextReview]);

  // Agent control: `app_open_review` puts this page into review/quiz mode,
  // optionally preselecting a card dataset. The review-load effect above picks
  // up the tab/dataset change and draws the first due card.
  useEffect(() => {
    const unlisten = listen<{ dataset_uuid: string | null }>("agent-open-review", (event) => {
      const uuid = event.payload?.dataset_uuid;
      if (uuid) setSelectedDatasetUuid(uuid);
      setActiveTab("review");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Submit review
  async function submitReview(quality: number) {
    if (!reviewCard || !selectedDatasetUuid) return;
    try {
      const result = await invoke<CardReview>("card_test_submit", {
        datasetUuid: selectedDatasetUuid,
        cardUuid: reviewCard.uuid,
        quality,
      });
      setReviewResult(result);
      // Load next card after a short delay
      setTimeout(() => loadNextReview(), 800);
    } catch (e) {
      setError(`Failed to submit review: ${e}`);
    }
  }

  // Create dataset
  async function createDataset() {
    const name = prompt("Dataset name:");
    if (!name) return;
    try {
      const result = await invoke<CardDatasetSummary>("card_dataset_create", {
        name,
        description: null,
        location: null,
      });
      setDatasets((prev) => [...prev, result]);
      setSelectedDatasetUuid(result.info.uuid);
    } catch (e) {
      setError(`Failed to create dataset: ${e}`);
    }
  }

  // Open online card link
  async function handleOpenOnline(path: string) {
    setError(null);
    try {
      await openCardUrl(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  // Global FTS search across all datasets
  async function handleGlobalSearch() {
    if (!globalSearchQuery.trim()) {
      setGlobalSearchResults([]);
      return;
    }
    setSearching(true);
    try {
      const results = await invoke<
        { dataset_uuid: string; dataset_name: string; card_uuid: string; question: string; answer: string; note: string; location: string }[]
      >("card_search", { query: globalSearchQuery, mode: searchMode });
      setGlobalSearchResults(results);
    } catch (e) {
      setError(`Search failed: ${e}`);
    } finally {
      setSearching(false);
    }
  }

  // Rebuild FTS index
  async function handleRebuildIndex() {
    setRebuildingIndex(true);
    try {
      const count = await invoke<number>("card_fts_rebuild", { location: null });
      alert(`Search index rebuilt! ${count} cards indexed.`);
    } catch (e) {
      setError(`Failed to rebuild index: ${e}`);
    } finally {
      setRebuildingIndex(false);
    }
  }

  const selectedDataset = datasets.find((d) => d.info.uuid === selectedDatasetUuid);
  const totalPages = Math.ceil(totalCards / pageSize);

  // Familiarity color
  function familiarityColor(f: number) {
    const colors = [
      "bg-slate-200",    // 0 - new
      "bg-red-200",      // 1 - strange
      "bg-orange-200",   // 2 - known
      "bg-amber-200",    // 3 - familiar
      "bg-cyan-200",     // 4 - skilled
      "bg-green-200",    // 5 - practiced
      "bg-lime-200",     // 6 - easy
    ];
    return colors[Math.min(f, 6)] || colors[0];
  }

  if (loading) {
    return (
      <main className="flex-1 flex items-center justify-center">
        <p className="text-text-secondary">Loading card datasets...</p>
      </main>
    );
  }

  return (
    <main className="flex-1 flex flex-col min-h-0 min-w-0">
      {/* Header */}
      <div className="px-2 pt-3 pb-2 border-b border-border-default sm:px-6 sm:pt-4">
        <div className="flex flex-wrap items-center justify-between gap-2 sm:gap-4">
          <h1 className="text-[1.5em] font-bold">Cards</h1>
          <div className="flex items-center gap-2">
            <select
              value={selectedDatasetUuid}
              onChange={(e) => {
                setSelectedDatasetUuid(e.target.value);
                setPage(0);
              }}
              className="max-w-[50vw] truncate px-3 py-1.5 rounded-lg border border-border-default bg-bg-card text-sm"
            >
              {datasets.map((ds) => (
                <option key={ds.info.uuid} value={ds.info.uuid}>
                  {ds.info.name} ({ds.card_count})
                </option>
              ))}
            </select>
            <button
              onClick={createDataset}
              className="p-1.5 rounded-lg border border-border-default hover:bg-mid-gray/20"
              title="Create new dataset"
            >
              <Plus size={16} />
            </button>
            <button
              onClick={loadDatasets}
              className="p-1.5 rounded-lg border border-border-default hover:bg-mid-gray/20"
              title="Refresh datasets"
            >
              <RefreshCw size={16} />
            </button>
          </div>
        </div>

        {/* Dataset info */}
        {selectedDataset && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 mt-1 text-xs text-text-secondary">
            <span>{selectedDataset.info.sharing.visibility}</span>
            <span>{selectedDataset.card_count} cards</span>
          </div>
        )}

        {/* Tabs */}
        <div className="flex gap-0.5 mt-3 overflow-x-auto sm:gap-1">
          {(["cards", "search", "review", "tags", "online", "advanced"] as TabId[]).map((tab) => (
            <button
              key={tab}
              onClick={() => setActiveTab(tab)}
              className={`shrink-0 whitespace-nowrap px-2.5 py-1 rounded-t-lg text-xs transition-colors sm:px-4 sm:py-1.5 sm:text-sm ${
                activeTab === tab
                  ? "bg-bg-card text-text-primary border-b-2 border-accent-blue"
                  : "text-text-secondary hover:text-text-primary"
              }`}
            >
              {tab === "cards" && "Cards"}
              {tab === "search" && "Search"}
              {tab === "review" && "Review"}
              {tab === "tags" && "Tags"}
              {tab === "online" && "Online"}
              {tab === "advanced" && "Advanced"}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="mx-2 mt-2 p-2 rounded bg-red-500/10 text-red-500 text-sm sm:mx-6">
          {error}
          <button onClick={() => setError(null)} className="ml-2 underline">
            dismiss
          </button>
        </div>
      )}

      {/* Tab content */}
      <div className="flex-1 min-h-0 overflow-hidden">
        {/* Cards tab */}
        {activeTab === "cards" && (
          <div className="flex flex-col h-full">
            {/* Toolbar */}
            <div className="px-2 py-2 flex flex-wrap items-center gap-2 border-b border-border-default sm:px-6">
              <div className="relative flex-1 max-w-xs">
                <Search
                  size={14}
                  className="absolute left-2 top-1/2 -translate-y-1/2 text-text-tertiary"
                />
                <input
                  type="text"
                  placeholder="Search cards..."
                  value={keyword}
                  onChange={(e) => {
                    setKeyword(e.target.value);
                    setPage(0);
                  }}
                  className="w-full pl-7 pr-2 py-1 rounded border border-border-default bg-bg-card text-sm"
                />
              </div>
              <select
                value={filter}
                onChange={(e) => {
                  setFilter(e.target.value as typeof filter);
                  setPage(0);
                }}
                className="px-2 py-1 rounded border border-border-default bg-bg-card text-sm"
              >
                <option value="all">All</option>
                <option value="normal">Normal (familiarity &lt; 6)</option>
                <option value="easy">Easy (familiarity = 6)</option>
                <option value="incomplete">Incomplete</option>
              </select>
              <button
                onClick={() => {
                  openCardEditor(selectedDatasetUuid, null, () => loadCards());
                }}
                className="ml-auto px-3 py-1 rounded border border-border-default text-sm hover:bg-mid-gray/20"
              >
                <Plus size={14} className="inline mr-1" />
                Add Card
              </button>
            </div>

            {/* Card list */}
            <div className="flex-1 overflow-y-auto px-2 py-2 sm:px-6">
              {cards.length === 0 ? (
                <p className="text-text-secondary text-center py-8">
                  No cards found. Click "Add Card" to create one.
                </p>
              ) : (
                <div className="space-y-1">
                  {cards.map((card) => (
                    <div
                      key={card.uuid}
                      className={`w-full flex items-center gap-2 p-2 rounded-lg hover:bg-mid-gray/10 cursor-pointer sm:gap-3 sm:p-3 ${familiarityColor(card.familiarity)}`}
                      onClick={() => {
                        const selectedDataset = datasets.find((ds) => ds.info.uuid === selectedDatasetUuid);
                        openCardEditor(selectedDatasetUuid, card, () => loadCards(), undefined, selectedDataset?.info.name);
                      }}
                    >
                      <div className="flex-1 min-w-0">
                        <p className="text-lg font-medium truncate sm:text-2xl">
                          {card.question || "(empty question)"}
                        </p>
                      </div>
                      <span className="text-xs text-text-tertiary shrink-0">
                        f={card.familiarity}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* Pagination */}
            {totalPages > 1 && (
              <div className="px-2 py-2 flex items-center justify-between border-t border-border-default sm:px-6">
                <span className="text-xs text-text-secondary">
                  {totalCards} cards total
                </span>
                <div className="flex items-center gap-2">
                  <button
                    onClick={() => setPage((p) => Math.max(0, p - 1))}
                    disabled={page === 0}
                    className="p-1 rounded hover:bg-mid-gray/20 disabled:opacity-30"
                  >
                    <ChevronLeft size={16} />
                  </button>
                  <span className="text-xs">
                    {page + 1} / {totalPages}
                  </span>
                  <button
                    onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
                    disabled={page >= totalPages - 1}
                    className="p-1 rounded hover:bg-mid-gray/20 disabled:opacity-30"
                  >
                    <ChevronRight size={16} />
                  </button>
                </div>
              </div>
            )}
          </div>
        )}

        {/* Search tab - FTS5 global search */}
        {activeTab === "search" && (
          <div className="flex flex-col h-full">
            {/* Search bar */}
            <div className="px-2 py-3 flex flex-wrap items-center gap-2 border-b border-border-default sm:px-6">
              <div className="relative flex-1 max-w-md">
                <Search
                  size={16}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary"
                />
                <input
                  type="text"
                  placeholder="Search across all datasets... (FTS5)"
                  value={globalSearchQuery}
                  onChange={(e) => setGlobalSearchQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleGlobalSearch();
                  }}
                  className="w-full pl-9 pr-3 py-2 rounded-lg border border-border-default bg-bg-card text-sm"
                />
              </div>
              <select
                value={searchMode}
                onChange={(e) => setSearchMode(e.target.value as "question" | "fulltext")}
                className="px-3 py-2 rounded-lg border border-border-default bg-bg-card text-sm"
                title="Search mode"
              >
                <option value="fulltext">Full text</option>
                <option value="question">Questions only</option>
              </select>
              <button
                onClick={handleGlobalSearch}
                disabled={searching}
                className="px-4 py-2 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20 disabled:opacity-50"
              >
                {searching ? "Searching..." : "Search"}
              </button>
              <button
                onClick={handleRebuildIndex}
                disabled={rebuildingIndex}
                className="px-3 py-2 rounded-lg border border-border-default text-xs hover:bg-mid-gray/20 disabled:opacity-50 disabled:cursor-not-allowed"
                title={rebuildingIndex ? "Rebuilding index..." : "Rebuild search index"}
              >
                <RefreshCw size={14} className={rebuildingIndex ? "animate-spin" : ""} />
              </button>
            </div>

            {/* Search results */}
            <div className="flex-1 overflow-y-auto px-2 py-3 sm:px-6">
              {globalSearchResults.length === 0 ? (
                <div className="text-center py-12">
                  <Search size={32} className="mx-auto text-text-tertiary mb-3" />
                  <p className="text-text-secondary">
                    {globalSearchQuery ? "No results found." : "Enter a search term to find cards across all datasets."}
                  </p>
                  <p className="text-xs text-text-tertiary mt-2">
                    Supports FTS5 syntax: "word1 word2" (AND), "word1 OR word2" (OR)
                  </p>
                  <button
                    onClick={handleRebuildIndex}
                    disabled={rebuildingIndex}
                    className="mt-4 px-4 py-2 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20 disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    {rebuildingIndex ? "Rebuilding..." : "Rebuild Search Index"}
                  </button>
                </div>
              ) : (
                <div className="space-y-2">
                  <p className="text-xs text-text-tertiary mb-2">
                    {globalSearchResults.length} result{globalSearchResults.length !== 1 ? "s" : ""}
                  </p>
                  {globalSearchResults.map((result) => (
                    <div
                      key={`${result.dataset_uuid}-${result.card_uuid}`}
                      className="p-3 rounded-lg border border-border-default bg-bg-card hover:bg-mid-gray/10 cursor-pointer"
                      onClick={() => {
                        // Switch to the dataset and open the card editor
                        setSelectedDatasetUuid(result.dataset_uuid);
                        setActiveTab("cards");
                        // Load the card and open editor
                        invoke<Card>("card_get", {
                          datasetUuid: result.dataset_uuid,
                          cardUuid: result.card_uuid,
                        })
                          .then((card) => {
                            openCardEditor(result.dataset_uuid, card, () => {
                              // Refresh search after edit
                              handleGlobalSearch();
                            }, undefined, result.dataset_name);
                          })
                          .catch((e) => setError(`Failed to load card: ${e}`));
                      }}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="flex-1 min-w-0">
                          <p className="text-lg font-medium truncate">
                            {result.question || "(empty question)"}
                          </p>
                          {result.answer && (
                            <p className="text-sm text-text-secondary truncate mt-1">
                              {result.answer}
                            </p>
                          )}
                        </div>
                      </div>
                      <div className="flex items-center gap-3 mt-2 text-xs text-text-tertiary">
                        <span className="font-medium text-text-secondary">
                          {result.dataset_name}
                        </span>
                        <span className="font-mono text-[10px]">
                          {result.card_uuid.slice(0, 8)}…
                        </span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}

        {/* Review tab. This panel is the only scroller: the card grows with its content
            instead of scrolling internally, so a long answer uses the full tab height.
            Content is top-aligned on purpose: centring it would move the toolbar up and
            down every time the answer is revealed or a longer card is drawn. */}
        {activeTab === "review" && (
          <div className="flex h-full overflow-y-auto p-3 sm:p-6">
            {reviewCard ? (
              <div className="w-full">
                {/* Desktop: one toolbar row, the grade scale dead-centre, the card-level
                    actions pinned to the far end. The empty flank keeps the scale centred on the
                    row rather than centred in whatever the actions leave over. Ratings are
                    always on screen, so a card can be graded from its question and nothing
                    shifts when the answer appears.
                    Below `sm` the cells stack full width and `order` puts the actions on the
                    first line, flush right, with the grade scale centred under them — card
                    level controls above the grading decision, which is the order a thumb
                    reaches for them on a phone. `items-start` keeps both groups on their top
                    edge, so a wrapped scale cannot push the actions up and down.
                    Sticky, so the grade buttons stay reachable while a long answer scrolls
                    (the same `sticky top-0 z-10 bg-bg-body` idiom the dictation toolbar
                    uses). The negative margins pull the bar up over the panel's own padding
                    and its padding puts that space back, so the opaque background covers the
                    gutters too and no text peeks around the edges. */}
                <div className="sticky top-0 z-10 -mx-3 -mt-3 grid grid-cols-1 items-start gap-2 bg-bg-body px-3 pb-4 pt-3 sm:-mx-6 sm:-mt-6 sm:grid-cols-[1fr_auto_1fr] sm:gap-4 sm:px-6 sm:pt-6">
                  {/* Left flank exists only to balance the 3-column grid on desktop. Hidden
                      below `sm`, where it would take up a stacked line. */}
                  <div className="hidden sm:order-1 sm:block" />

                  <div className="order-2 flex flex-wrap justify-center gap-2">
                    {/* Icon + label on desktop, icon only below `sm` (see `IconLabel`),
                        so the scale still fits a phone. The tint and left-to-right order
                        carry the progression; the face on top names it. */}
                    {[
                      { q: 1, label: "Forgot", color: "bg-red-500", Icon: CircleHelp },
                      { q: 2, label: "Hard", color: "bg-orange-500", Icon: Frown },
                      { q: 3, label: "Okay", color: "bg-amber-500", Icon: Meh },
                      { q: 4, label: "Good", color: "bg-cyan-500", Icon: Smile },
                      { q: 5, label: "Easy", color: "bg-green-500", Icon: Laugh },
                    ].map((btn) => (
                      <button
                        key={btn.q}
                        onClick={() => submitReview(btn.q)}
                        title={btn.label}
                        aria-label={btn.label}
                        className={`flex items-center gap-1.5 px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default ${btn.color} text-sm hover:opacity-90`}
                      >
                        <btn.Icon size={14} />
                        <IconLabel>{btn.label}</IconLabel>
                      </button>
                    ))}
                  </div>

                  <div className="order-1 flex items-center justify-end gap-2 sm:order-3">
                    <button
                      onClick={() => setReviewFlipped(!reviewFlipped)}
                      title={reviewFlipped ? "Show question" : "Show answer"}
                      aria-label={reviewFlipped ? "Show question" : "Show answer"}
                      className="flex items-center gap-1.5 px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20"
                    >
                      <FlipHorizontal size={14} />
                      <IconLabel>{reviewFlipped ? "Question" : "Answer"}</IconLabel>
                    </button>
                    <button
                      onClick={loadNextReview}
                      title="Skip this card"
                      aria-label="Skip this card"
                      className="flex items-center gap-1.5 px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20"
                    >
                      <SkipForward size={14} />
                      <IconLabel>Skip</IconLabel>
                    </button>
                  </div>
                </div>

                {/* Card display. Deliberately not clickable: dragging to select part of
                    a question or answer must not flip the card over, so the side switch
                    lives on the buttons above. No `max-h`/`overflow-y-auto` here — the
                    box grows and the panel above scrolls. */}
                <div className="bg-bg-card rounded-xl border border-border-default p-4 min-h-[220px] flex flex-col sm:p-8 sm:min-h-[300px]">
                  {/* Long answers read as a block of prose, so the revealed side is
                      left-aligned; the prompt-only side stays centered. */}
                  <div className={`break-words my-auto w-full ${reviewFlipped ? "text-left" : "text-center"}`}>
                    {!reviewFlipped ? (
                      /* The prompt is the whole point of this side, so it is sized as a
                          display heading. Parent `break-words` keeps a long German compound
                          wrapping instead of overflowing the card. */
                      <p className="text-4xl font-bold sm:text-5xl">
                        {reviewCard.question}
                      </p>
                    ) : (
                      <>
                        {/* Repeated above the answer for context only, so it stays small —
                            at display size it would compete with what you are reading. */}
                        <p className="text-lg text-text-secondary mb-2">
                          {reviewCard.question}
                        </p>
                        <div className="border-t border-border-default my-4" />
                        {/* Answers are written in markdown (the card editor says so), so
                            render them through the same viewer the wiki uses. */}
                        <div className="text-xl">
                          <MarkdownViewer content={reviewCard.answer} />
                        </div>
                        {reviewCard.note && (
                          <div className="text-sm mt-4">
                            <MarkdownViewer content={reviewCard.note} />
                          </div>
                        )}
                      </>
                    )}
                  </div>
                </div>

                {/* Review result feedback */}
                {reviewResult && (
                  <div className="mt-4 p-3 rounded-lg bg-green-500/10 text-green-600 text-sm text-center">
                    <Check size={16} className="inline mr-1" />
                    Reviewed! Next review in {reviewResult.interval_days} day(s).
                    Familiarity: {reviewResult.familiarity}
                  </div>
                )}

                <ReviewQueueBar stats={reviewStats} />
              </div>
            ) : (
              <div className="text-center">
                <ReviewQueueBar stats={reviewStats} />
                <p className="text-text-secondary text-lg mb-4">
                  No cards to review!
                </p>
                <p className="text-text-tertiary text-sm">
                  {reviewStats && reviewStats.incomplete > 0
                    ? `${reviewStats.incomplete} ${reviewStats.incomplete === 1 ? "card has" : "cards have"} no question or answer yet — fill them in on the Cards tab.`
                    : "Nothing is due, and no new cards are waiting."}
                </p>
                <button
                  onClick={loadNextReview}
                  className="mt-4 flex items-center gap-1.5 mx-auto px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default text-sm hover:bg-mid-gray/20"
                >
                  <RefreshCw size={14} />
                  Check again
                </button>
              </div>
            )}
          </div>
        )}

        {/* Tags tab */}
        {activeTab === "tags" && (
          <div className="p-3 overflow-y-auto h-full sm:p-6">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-lg font-semibold">Tags</h2>
              <button
                onClick={async () => {
                  const name = prompt("Tag name:");
                  if (!name || !selectedDatasetUuid) return;
                  try {
                    await invoke("card_tag_save", {
                      datasetUuid: selectedDatasetUuid,
                      tag: { uuid: "", name, color: null, deleted_at: null, created_at: "", updated_at: "" },
                    });
                    loadTags();
                  } catch (e) {
                    setError(`Failed to create tag: ${e}`);
                  }
                }}
                className="px-3 py-1 rounded border border-border-default text-sm hover:bg-mid-gray/20"
              >
                <Plus size={14} className="inline mr-1" />
                Add Tag
              </button>
            </div>
            {tags.length === 0 ? (
              <p className="text-text-secondary">No tags yet.</p>
            ) : (
              <div className="space-y-2">
                {tags.map((tag) => (
                  <div
                    key={tag.uuid}
                    className="flex items-center gap-2 p-2 rounded border border-border-default"
                  >
                    <Tag size={14} className="text-text-tertiary" />
                    <span className="flex-1">{tag.name}</span>
                    <button
                      onClick={async () => {
                        const confirmed = await ask(`Delete tag "${tag.name}"?`, { title: "Delete Tag" });
                        if (!confirmed) return;
                        try {
                          await invoke("card_tag_delete", {
                            datasetUuid: selectedDatasetUuid,
                            tagUuid: tag.uuid,
                          });
                          loadTags();
                        } catch (e) {
                          setError(`Failed to delete tag: ${e}`);
                        }
                      }}
                      className="p-1 rounded hover:bg-red-500/10 text-text-tertiary hover:text-red-500"
                    >
                      <X size={14} />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Online tab */}
        {activeTab === "online" && (
          <div className="p-3 overflow-y-auto h-full sm:p-6">
            <h2 className="text-lg font-semibold mb-2">Online Card System</h2>
            <p className="text-text-secondary text-sm mb-4">
              Access your cards online at{" "}
              <span className="font-medium">{CARD_BASE_URL}</span>.
            </p>
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
              {CARD_LINKS.map((link) => (
                <button
                  key={link.key}
                  onClick={() => handleOpenOnline(link.path)}
                  className="text-left p-4 rounded-xl border border-border-default bg-bg-card hover:bg-mid-gray/20 transition-colors cursor-pointer flex flex-col gap-1"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-semibold text-text-primary">
                      {link.label}
                    </span>
                    <ExternalLink
                      size={16}
                      className="text-text-tertiary shrink-0"
                    />
                  </div>
                  <span className="text-sm text-text-secondary">
                    {link.description}
                  </span>
                </button>
              ))}
            </div>
          </div>
        )}

        {/* Advanced tab */}
        {activeTab === "advanced" && (
          <AdvancedTab
            dataset={selectedDataset}
            selectedDatasetUuid={selectedDatasetUuid}
            allDatasets={datasets}
            onDeleteDataset={async (uuid: string) => {
              try {
                await invoke("card_dataset_delete", { uuid });
                setDatasets((prev) => prev.filter((d) => d.info.uuid !== uuid));
                if (uuid === selectedDatasetUuid) {
                  setSelectedDatasetUuid("");
                }
              } catch (e) {
                setError(`Failed to delete dataset: ${e}`);
              }
            }}
            onRefresh={loadDatasets}
            onError={setError}
          />
        )}
      </div>
    </main>
  );
}

// ── Advanced Tab ────────────────────────────────────────────────────────────

function AdvancedTab({
  dataset,
  selectedDatasetUuid,
  allDatasets,
  onDeleteDataset,
  onRefresh,
  onError,
}: {
  dataset: CardDatasetSummary | undefined;
  selectedDatasetUuid: string;
  allDatasets: CardDatasetSummary[];
  onDeleteDataset: (uuid: string) => Promise<void>;
  onRefresh: () => void;
  onError: (msg: string) => void;
}) {
  // Locations state
  const [dirs, setDirs] = useState<{ name: string; path: string; is_linked: boolean }[]>([]);
  const [showAddDir, setShowAddDir] = useState(false);
  const [newDirName, setNewDirName] = useState("");
  const [newDirPath, setNewDirPath] = useState("");
  // Deferred platform flag (SSR-safe). Folder picking is desktop-only — the dialog
  // plugin answers `open({ directory: true })` with FolderPickerNotImplemented on
  // Android/iOS — so the Browse affordance is hidden there and the path stays typed
  // by hand (e.g. a directory filled by the PC sync flow).
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Load dataset directories (locations)
  const loadDirs = useCallback(async () => {
    try {
      const result = await invoke<{ name: string; path: string; is_linked: boolean }[]>("dataset_list_dirs", { datasetType: "card" });
      setDirs(result);
    } catch {
      // Ignore
    }
  }, []);

  useEffect(() => {
    loadDirs();
  }, [loadDirs]);

  return (
    <div className="p-3 overflow-y-auto h-full space-y-6 sm:p-6">
      {/* Locations - moved to top */}
      <section>
        <h2 className="text-lg font-semibold mb-3 flex items-center gap-2">
          <FolderOpen size={18} />
          Locations
        </h2>
        <p className="text-sm text-text-secondary mb-3">
          Card datasets are stored across dataset directories. Add linked directories to store datasets on external drives or other locations.
        </p>

        {/* Directory list */}
        <div className="space-y-2 mb-3">
          {dirs.map((dir) => {
            const datasetsInDir = allDatasets.filter((d) => d.location === dir.path);
            return (
              <div
                key={dir.path}
                className="p-3 rounded-lg border border-border-default bg-bg-card"
              >
                <div className="flex items-center justify-between mb-1">
                  <div className="flex items-center gap-2">
                    <MapPin size={14} className="text-text-tertiary" />
                    <span className="text-sm font-medium">{dir.name}</span>
                    {!dir.is_linked && (
                      <span className="text-xs px-1.5 py-0.5 rounded bg-accent-blue/10 text-accent-blue">
                        default
                      </span>
                    )}
                    <span className="text-xs text-text-tertiary">
                      {datasetsInDir.length} dataset{datasetsInDir.length !== 1 ? "s" : ""}
                    </span>
                  </div>
                  {dir.is_linked && (
                    <button
                      onClick={async () => {
                        const confirmed = await ask(`Unlink directory "${dir.name}"? Datasets inside will no longer be visible.`, { title: "Unlink Directory" });
                        if (!confirmed) return;
                        try {
                          await invoke("dataset_remove_dir", { path: dir.path });
                          loadDirs();
                          onRefresh();
                        } catch (e) {
                          onError(`Failed to unlink directory: ${e}`);
                        }
                      }}
                      className="p-1 rounded hover:bg-red-500/10 text-text-tertiary hover:text-red-500"
                      title="Remove linked directory"
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>
                <p className="text-xs text-text-tertiary font-mono">{dir.path}</p>
              </div>
            );
          })}
        </div>

        {/* Add linked directory */}
        {showAddDir ? (
          <div className="p-3 rounded-lg border border-border-default bg-bg-card space-y-2">
            <div>
              <label className="block text-xs font-medium mb-1">Display Name</label>
              <input
                type="text"
                value={newDirName}
                onChange={(e) => setNewDirName(e.target.value)}
                placeholder="e.g. External SSD"
                className="w-full px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
              />
            </div>
            <div>
              <label className="block text-xs font-medium mb-1">Directory Path</label>
              <div className="flex gap-1">
                <input
                  type="text"
                  value={newDirPath}
                  onChange={(e) => setNewDirPath(e.target.value)}
                  placeholder="e.g. /mnt/external/fms-datasets"
                  className="flex-1 px-2 py-1 rounded border border-border-default bg-bg-surface text-sm"
                />
                {!mobile && (
                  <button
                    onClick={async () => {
                      const picked = await open({ directory: true, multiple: false, title: "Select Directory" });
                      if (typeof picked === "string") {
                        setNewDirPath(picked);
                      }
                    }}
                    className="px-2 py-1 rounded border border-border-default hover:bg-mid-gray/20 text-sm"
                    title="Browse..."
                  >
                    <FolderOpen size={14} />
                  </button>
                )}
              </div>
            </div>
            <div className="flex gap-2">
              <button
                onClick={async () => {
                  if (!newDirName.trim() || !newDirPath.trim()) return;
                  try {
                    await invoke("dataset_add_dir", { name: newDirName.trim(), path: newDirPath.trim() });
                    setShowAddDir(false);
                    setNewDirName("");
                    setNewDirPath("");
                    loadDirs();
                    onRefresh();
                  } catch (e) {
                    onError(`Failed to add directory: ${e}`);
                  }
                }}
                className="px-3 py-1 rounded bg-accent-blue text-sm hover:opacity-90"
              >
                Add
              </button>
              <button
                onClick={() => {
                  setShowAddDir(false);
                  setNewDirName("");
                  setNewDirPath("");
                }}
                className="px-3 py-1 rounded border border-border-default text-sm"
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setShowAddDir(true)}
            className="px-3 py-1.5 rounded border border-border-default text-sm hover:bg-mid-gray/20"
          >
            <FolderPlus size={14} className="inline mr-1" />
            Add Linked Directory
          </button>
        )}
      </section>

      {/* Datasets - card list */}
      <section>
        <h2 className="text-lg font-semibold mb-3 flex items-center gap-2">
          <Settings size={18} />
          Datasets
        </h2>
        {allDatasets.length === 0 ? (
          <p className="text-sm text-text-secondary">No card datasets found.</p>
        ) : (
          <div className="space-y-3">
            {allDatasets.map((ds) => (
              <DatasetCard
                key={ds.info.uuid}
                dataset={ds}
                isSelected={ds.info.uuid === selectedDatasetUuid}
                onDelete={async () => {
                  const confirmed = await ask(`Delete dataset "${ds.info.name}"? This is irreversible.`, { title: "Delete Dataset" });
                  if (!confirmed) return;
                  await onDeleteDataset(ds.info.uuid);
                }}
                onError={onError}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

// ── Dataset Card (for Advanced tab) ──────────────────────────────────────────

function DatasetCard({
  dataset,
  isSelected,
  onDelete,
  onError,
}: {
  dataset: CardDatasetSummary;
  isSelected: boolean;
  onDelete: () => Promise<void>;
  onError: (msg: string) => void;
}) {
  const [syncing, setSyncing] = useState(false);
  const [syncStatus, setSyncStatus] = useState<{
    last_synced_at: string | null;
    pending_push: number;
  } | null>(null);
  const [deleting, setDeleting] = useState(false);

  // Load sync status on mount
  useEffect(() => {
    invoke<{ last_synced_at: string | null; pending_push: number } | null>(
      "card_sync_status",
      { datasetUuid: dataset.info.uuid }
    )
      .then((status) => setSyncStatus(status))
      .catch(() => {});
  }, [dataset.info.uuid]);

  async function handleSync() {
    setSyncing(true);
    try {
      const result = await invoke<{ pulled: number; pushed: number; conflicts: number; server_time: string }>(
        "card_sync_full",
        { datasetUuid: dataset.info.uuid }
      );
      alert(`Sync complete!\nPulled: ${result.pulled}\nPushed: ${result.pushed}\nConflicts: ${result.conflicts}`);
      // Refresh sync status
      const status = await invoke<{ last_synced_at: string | null; pending_push: number } | null>(
        "card_sync_status",
        { datasetUuid: dataset.info.uuid }
      );
      setSyncStatus(status);
    } catch (e) {
      onError(`Sync failed: ${e}`);
    } finally {
      setSyncing(false);
    }
  }

  async function handleDelete() {
    setDeleting(true);
    try {
      await onDelete();
    } finally {
      setDeleting(false);
    }
  }

  const dirName = dataset.location;

  return (
    <div
      className={`p-4 rounded-xl border bg-bg-card transition-colors ${
        isSelected
          ? "border-accent-blue/40 ring-1 ring-accent-blue/20"
          : "border-border-default hover:border-border-default/80"
      }`}
    >
      {/* Header: name + visibility badge */}
      <div className="flex items-start justify-between gap-3 mb-2">
        <div className="flex items-center gap-2 min-w-0">
          <h3 className="text-sm font-semibold truncate">{dataset.info.name}</h3>
          <span
            className={`text-[10px] px-1.5 py-0.5 rounded font-medium shrink-0 ${
              dataset.info.sharing.visibility === "public"
                ? "bg-green-500/10 text-green-600"
                : dataset.info.sharing.visibility === "shared"
                  ? "bg-amber-500/10 text-amber-600"
                  : "bg-mid-gray/20 text-text-tertiary"
            }`}
          >
            {dataset.info.sharing.visibility}
          </span>
          {isSelected && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-accent-blue/10 text-accent-blue shrink-0">
              active
            </span>
          )}
        </div>
        <span className="text-xs text-text-tertiary shrink-0">
          {dataset.card_count} card{dataset.card_count !== 1 ? "s" : ""}
        </span>
      </div>

      {/* Description */}
      {dataset.info.description && (
        <p className="text-xs text-text-secondary mb-3 line-clamp-2">
          {dataset.info.description}
        </p>
      )}

      {/* Meta row: location + UUID */}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-tertiary mb-3">
        <span className="flex items-center gap-1">
          <MapPin size={11} />
          {dirName}
        </span>
        <span className="font-mono text-[10px]">{dataset.info.uuid.slice(0, 8)}…</span>
      </div>

      {/* Sync section — the hub is the login origin (auth::BASE_URL), not a
          per-dataset URL, so there is nothing to configure per dataset here. */}
      <div className="flex items-center flex-wrap gap-x-4 gap-y-1 text-xs mb-3">
        <span className="text-text-secondary">
          Last synced:{" "}
          {syncStatus?.last_synced_at
            ? new Date(syncStatus.last_synced_at).toLocaleString()
            : "Never"}
        </span>
        {(syncStatus?.pending_push ?? 0) > 0 && (
          <span className="text-amber-500">
            {syncStatus!.pending_push} pending
          </span>
        )}
      </div>

      {/* Action buttons */}
      <div className="flex items-center gap-2">
        <button
          onClick={handleSync}
          disabled={syncing}
          className="px-3 py-1 rounded border border-border-default text-xs hover:bg-mid-gray/20 disabled:opacity-50 flex items-center gap-1"
        >
          <RefreshCcw size={12} className={syncing ? "animate-spin" : ""} />
          {syncing ? "Syncing..." : "Sync"}
        </button>
        <button
          onClick={handleDelete}
          disabled={deleting}
          className="ml-auto px-3 py-1 rounded bg-red-500/10 border border-red-500/20 text-red-500 text-xs hover:bg-red-500/20 disabled:opacity-50 flex items-center gap-1"
        >
          <Trash2 size={12} />
          {deleting ? "Deleting..." : "Remove"}
        </button>
      </div>
    </div>
  );
}

function plural(n: number, singular: string, count: string) {
  return n === 1 ? singular : count;
}

/**
 * Button caption that disappears below `sm`, so a phone sees the icon alone while the
 * wide layout keeps icon + text. The owning button carries `title`/`aria-label`, so the
 * accessible name and a hover hint do not depend on this span being visible.
 */
function IconLabel({ children }: { children: ReactNode }) {
  return <span className="hidden sm:inline">{children}</span>;
}

/**
 * Review queue summary: how many cards are in front of you, how many are waiting,
 * and why the waiting ones are not showing up yet.
 *
 * `card_test_get` serves overdue cards before never-reviewed ones, so a big "to learn"
 * count beside a small "due" count is not missing data — it is a queue that needs the
 * overdue cards answered first. Saying so here keeps that from looking like a bug.
 */
function ReviewQueueBar({ stats }: { stats: CardTestStats | null }) {
  if (!stats) return null;

  let hint: string | null = null;
  if (stats.due > 0 && stats.fresh > 0) {
    hint = `${stats.fresh} ${plural(stats.fresh, "card is", "cards are")} waiting to learn in this dataset, but the ${stats.due} overdue ${plural(stats.due, "card goes", "cards go")} first — rate this one to bring the next forward.`;
  } else if (stats.fresh > 0) {
    hint = `${stats.fresh} ${plural(stats.fresh, "card is", "cards are")} waiting to learn in this dataset — answer this one to get the next.`;
  } else if (stats.due > 0) {
    hint = `No new cards left here — only the ${stats.due} overdue ${plural(stats.due, "one remains", "ones remain")}.`;
  }

  return (
    <div className="my-3 text-center">
      <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-xs text-text-tertiary">
        <span>{stats.due} due</span>
        <span>{stats.fresh} to learn</span>
        <span>{stats.total} cards</span>
        {stats.mature > 0 && <span>{stats.mature} mastered</span>}
        {stats.incomplete > 0 && <span>{stats.incomplete} incomplete</span>}
      </div>
      {hint && <p className="mt-1 text-xs text-accent-blue">{hint}</p>}
    </div>
  );
}


