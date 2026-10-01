"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ask, open } from "@tauri-apps/plugin-dialog";
import {
  Plus,
  RefreshCw,
  Search,
  ChevronLeft,
  ChevronRight,
  FlipHorizontal,
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
} from "@/lib/types";
import { CARD_BASE_URL, CARD_LINKS, openCardUrl } from "@/lib/cards";

type TabId = "cards" | "review" | "tags" | "online" | "advanced";

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

  // Card detail/edit
  const [editingCard, setEditingCard] = useState<Card | null>(null);
  const [showAddCard, setShowAddCard] = useState(false);

  // Review state
  const [reviewCard, setReviewCard] = useState<Card | null>(null);
  const [reviewFlipped, setReviewFlipped] = useState(false);
  const [reviewResult, setReviewResult] = useState<CardReview | null>(null);

  // Tags
  const [tags, setTags] = useState<CardTag[]>([]);

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
    }
  }, [selectedDatasetUuid]);

  useEffect(() => {
    if (selectedDatasetUuid && activeTab === "review") {
      loadNextReview();
    }
  }, [selectedDatasetUuid, activeTab, loadNextReview]);

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

  // Save card
  async function saveCard(card: Partial<Card>) {
    if (!selectedDatasetUuid) return;
    try {
      const fullCard: Card = {
        uuid: card.uuid || "",
        question: card.question || "",
        suggestion: card.suggestion || "",
        answer: card.answer || "",
        note: card.note || "",
        familiarity: card.familiarity || 0,
        question_hash: null,
        source_card_uuid: card.source_card_uuid || null,
        source_dataset_uuid: card.source_dataset_uuid || null,
        deleted_at: null,
        created_at: "",
        updated_at: "",
      };
      await invoke<Card>("card_save", {
        datasetUuid: selectedDatasetUuid,
        card: fullCard,
      });
      setEditingCard(null);
      setShowAddCard(false);
      loadCards();
    } catch (e) {
      setError(`Failed to save card: ${e}`);
    }
  }

  // Delete card
  async function deleteCard(cardUuid: string) {
    if (!selectedDatasetUuid) return;
    try {
      await invoke("card_delete", {
        datasetUuid: selectedDatasetUuid,
        cardUuid,
      });
      loadCards();
    } catch (e) {
      setError(`Failed to delete card: ${e}`);
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
    <main className="flex-1 flex flex-col min-h-0">
      {/* Header */}
      <div className="px-6 pt-4 pb-2 border-b border-border-default">
        <div className="flex items-center justify-between gap-4">
          <h1 className="text-[1.5em] font-bold">Cards</h1>
          <div className="flex items-center gap-2">
            <select
              value={selectedDatasetUuid}
              onChange={(e) => {
                setSelectedDatasetUuid(e.target.value);
                setPage(0);
              }}
              className="px-3 py-1.5 rounded-lg border border-border-default bg-bg-card text-sm"
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
          <div className="flex items-center gap-4 mt-1 text-xs text-text-secondary">
            <span>{selectedDataset.info.visibility}</span>
            <span>{selectedDataset.card_count} cards</span>
            {selectedDataset.info.sync_url && (
              <span className="text-text-tertiary">
                Sync: {selectedDataset.info.sync_url}
              </span>
            )}
          </div>
        )}

        {/* Tabs */}
        <div className="flex gap-1 mt-3">
          {(["cards", "review", "tags", "online", "advanced"] as TabId[]).map((tab) => (
            <button
              key={tab}
              onClick={() => setActiveTab(tab)}
              className={`px-4 py-1.5 rounded-t-lg text-sm transition-colors ${
                activeTab === tab
                  ? "bg-bg-card text-text-primary border-b-2 border-accent-blue"
                  : "text-text-secondary hover:text-text-primary"
              }`}
            >
              {tab === "cards" && "Cards"}
              {tab === "review" && "Review"}
              {tab === "tags" && "Tags"}
              {tab === "online" && "Online"}
              {tab === "advanced" && "Advanced"}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="mx-6 mt-2 p-2 rounded bg-red-500/10 text-red-500 text-sm">
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
            <div className="px-6 py-2 flex items-center gap-2 border-b border-border-default">
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
                  setShowAddCard(true);
                }}
                className="ml-auto px-3 py-1 rounded bg-accent-blue text-white text-sm hover:opacity-90"
              >
                <Plus size={14} className="inline mr-1" />
                Add Card
              </button>
            </div>

            {/* Card list */}
            <div className="flex-1 overflow-y-auto px-6 py-2">
              {cards.length === 0 ? (
                <p className="text-text-secondary text-center py-8">
                  No cards found. Click "Add Card" to create one.
                </p>
              ) : (
                <div className="space-y-1">
                  {cards.map((card) => (
                    <div
                      key={card.uuid}
                      className={`flex items-center gap-3 p-2 rounded-lg hover:bg-mid-gray/10 cursor-pointer ${familiarityColor(card.familiarity)}`}
                      onClick={() => setEditingCard(card)}
                    >
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-medium truncate">
                          {card.question || "(empty question)"}
                        </p>
                        <p className="text-xs text-text-secondary truncate">
                          {card.answer || "(empty answer)"}
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
              <div className="px-6 py-2 flex items-center justify-between border-t border-border-default">
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

        {/* Review tab */}
        {activeTab === "review" && (
          <div className="flex flex-col items-center justify-center h-full p-6">
            {reviewCard ? (
              <div className="w-full max-w-lg">
                {/* Card display */}
                <div
                  className="relative bg-bg-card rounded-xl border border-border-default p-8 min-h-[300px] flex items-center justify-center cursor-pointer"
                  onClick={() => setReviewFlipped(!reviewFlipped)}
                >
                  <div className="text-center">
                    {!reviewFlipped ? (
                      <>
                        <p className="text-2xl font-medium mb-4">
                          {reviewCard.question}
                        </p>
                        <p className="text-sm text-text-tertiary">
                          Click to reveal answer
                        </p>
                      </>
                    ) : (
                      <>
                        <p className="text-lg text-text-secondary mb-2">
                          {reviewCard.question}
                        </p>
                        <div className="border-t border-border-default my-4" />
                        <p className="text-xl">{reviewCard.answer}</p>
                        {reviewCard.note && (
                          <p className="text-sm text-text-tertiary mt-4">
                            {reviewCard.note}
                          </p>
                        )}
                      </>
                    )}
                  </div>
                  <div className="absolute top-2 right-2">
                    <FlipHorizontal size={16} className="text-text-tertiary" />
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

                {/* Rating buttons (only when flipped) */}
                {reviewFlipped && (
                  <div className="mt-6">
                    <p className="text-center text-sm text-text-secondary mb-3">
                      How well did you recall this?
                    </p>
                    <div className="flex justify-center gap-2">
                      {[
                        { q: 1, label: "Forgot", color: "bg-red-500" },
                        { q: 2, label: "Hard", color: "bg-orange-500" },
                        { q: 3, label: "Okay", color: "bg-amber-500" },
                        { q: 4, label: "Good", color: "bg-cyan-500" },
                        { q: 5, label: "Easy", color: "bg-green-500" },
                      ].map((btn) => (
                        <button
                          key={btn.q}
                          onClick={() => submitReview(btn.q)}
                          className={`px-4 py-2 rounded-lg ${btn.color} text-white text-sm hover:opacity-90`}
                        >
                          {btn.label}
                        </button>
                      ))}
                    </div>
                    <p className="text-center text-xs text-text-tertiary mt-2">
                      1 = complete failure, 5 = perfect recall
                    </p>
                  </div>
                )}

                {/* Skip button */}
                <div className="mt-4 text-center">
                  <button
                    onClick={loadNextReview}
                    className="text-sm text-text-secondary hover:text-text-primary"
                  >
                    Skip →
                  </button>
                </div>
              </div>
            ) : (
              <div className="text-center">
                <p className="text-text-secondary text-lg mb-4">
                  No cards to review!
                </p>
                <p className="text-text-tertiary text-sm">
                  All cards have been reviewed or no cards match the criteria.
                </p>
                <button
                  onClick={loadNextReview}
                  className="mt-4 px-4 py-2 rounded-lg bg-accent-blue text-white text-sm"
                >
                  Check again
                </button>
              </div>
            )}
          </div>
        )}

        {/* Tags tab */}
        {activeTab === "tags" && (
          <div className="p-6 overflow-y-auto h-full">
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
                className="px-3 py-1 rounded bg-accent-blue text-white text-sm"
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
          <div className="p-6 overflow-y-auto h-full">
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

      {/* Card edit modal */}
      {(editingCard || showAddCard) && (
        <CardEditModal
          card={editingCard}
          onSave={saveCard}
          onDelete={editingCard ? () => { deleteCard(editingCard.uuid); setEditingCard(null); } : undefined}
          onClose={() => {
            setEditingCard(null);
            setShowAddCard(false);
          }}
        />
      )}
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

  // Load dataset directories (locations)
  const loadDirs = useCallback(async () => {
    try {
      const result = await invoke<{ name: string; path: string; is_linked: boolean }[]>("dataset_list_dirs");
      setDirs(result);
    } catch {
      // Ignore
    }
  }, []);

  useEffect(() => {
    loadDirs();
  }, [loadDirs]);

  return (
    <div className="p-6 overflow-y-auto h-full space-y-6">
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
                className="px-3 py-1 rounded bg-accent-blue text-white text-sm hover:opacity-90"
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
    if (!dataset.info.sync_url) return;
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
              dataset.info.visibility === "public"
                ? "bg-green-500/10 text-green-600"
                : dataset.info.visibility === "shared"
                  ? "bg-amber-500/10 text-amber-600"
                  : "bg-mid-gray/20 text-text-tertiary"
            }`}
          >
            {dataset.info.visibility}
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

      {/* Sync section */}
      <div className="flex items-center flex-wrap gap-x-4 gap-y-1 text-xs mb-3">
        {dataset.info.sync_url ? (
          <>
            <span className="text-text-secondary">
              Sync: <span className="font-mono text-text-tertiary">{dataset.info.sync_url}</span>
            </span>
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
          </>
        ) : (
          <span className="text-text-tertiary italic">No sync URL configured</span>
        )}
      </div>

      {/* Action buttons */}
      <div className="flex items-center gap-2">
        {dataset.info.sync_url && (
          <button
            onClick={handleSync}
            disabled={syncing}
            className="px-3 py-1 rounded bg-accent-blue text-white text-xs hover:opacity-90 disabled:opacity-50 flex items-center gap-1"
          >
            <RefreshCcw size={12} className={syncing ? "animate-spin" : ""} />
            {syncing ? "Syncing..." : "Sync"}
          </button>
        )}
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

// ── Card Edit Modal ─────────────────────────────────────────────────────────

function CardEditModal({
  card,
  onSave,
  onDelete,
  onClose,
}: {
  card: Card | null;
  onSave: (card: Partial<Card>) => void;
  onDelete?: () => void;
  onClose: () => void;
}) {
  const [question, setQuestion] = useState(card?.question || "");
  const [answer, setAnswer] = useState(card?.answer || "");
  const [note, setNote] = useState(card?.note || "");
  const [suggestion, setSuggestion] = useState(card?.suggestion || "");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-bg-card rounded-xl border border-border-default p-6 w-full max-w-lg max-h-[90vh] overflow-y-auto">
        <h2 className="text-lg font-semibold mb-4">
          {card ? "Edit Card" : "New Card"}
        </h2>

        <div className="space-y-3">
          <div>
            <label className="block text-sm font-medium mb-1">Question</label>
            <textarea
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              rows={3}
              className="w-full px-3 py-2 rounded border border-border-default bg-bg-surface text-sm"
              placeholder="What do you want to learn?"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">Answer</label>
            <textarea
              value={answer}
              onChange={(e) => setAnswer(e.target.value)}
              rows={3}
              className="w-full px-3 py-2 rounded border border-border-default bg-bg-surface text-sm"
              placeholder="The answer or explanation"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">
              Suggestion (optional)
            </label>
            <input
              type="text"
              value={suggestion}
              onChange={(e) => setSuggestion(e.target.value)}
              className="w-full px-3 py-2 rounded border border-border-default bg-bg-surface text-sm"
              placeholder="AI suggestion or hint"
            />
          </div>
          <div>
            <label className="block text-sm font-medium mb-1">
              Note (optional)
            </label>
            <textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              rows={2}
              className="w-full px-3 py-2 rounded border border-border-default bg-bg-surface text-sm"
              placeholder="Additional notes"
            />
          </div>
        </div>

        <div className="flex justify-between mt-6">
          <div>
            {onDelete && (
              <button
                onClick={onDelete}
                className="px-4 py-2 rounded bg-red-500/10 text-red-500 text-sm hover:bg-red-500/20"
              >
                Delete
              </button>
            )}
          </div>
          <div className="flex gap-2">
            <button
              onClick={onClose}
              className="px-4 py-2 rounded border border-border-default text-sm"
            >
              Cancel
            </button>
            <button
              onClick={() =>
                onSave({
                  uuid: card?.uuid || "",
                  question,
                  answer,
                  note,
                  suggestion,
                  familiarity: card?.familiarity || 0,
                })
              }
              disabled={!question.trim()}
              className="px-4 py-2 rounded bg-accent-blue text-white text-sm hover:opacity-90 disabled:opacity-50"
            >
              Save
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
