"use client";

import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { usePageRefresh } from "@/lib/refresh/page-refresh";
import { ask } from "@tauri-apps/plugin-dialog";
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
  Layers,
  GraduationCap,
  FolderCog,
  type LucideIcon,
} from "lucide-react";
import type {
  CardDatasetSummary,
  Card,
  CardReview,
  CardTag,
  CardTestStats,
  CrossDatasetCard,
} from "@/lib/types";
import { isMobileApp } from "@/lib/platform";
import { LocationsManager } from "@/components/ui/LocationsManager";
import PendingWritebackBanner from "@/components/ui/PendingWritebackBanner";
import {
  Toolbar,
  Breadcrumb,
  ToolbarButton,
  ToolbarOverflow,
  ToolbarMenuItem,
} from "@/components/ui/toolbar";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import { useCardEditor } from "./CardEditorContext";
import { oneOf, reqStr, optStr, useAgentActions } from "@/lib/agent/ui-actions";

type TabId = "cards" | "review" | "tags";

// Detail-view toolbar modes, rendered as toggle buttons on the toolbar right
// (active = solid accent), mirroring ReadBook's Manage/Advanced mode group.
// The former "Search" tab is gone: searching is unified into the Cards view's
// search box, whose This-dataset/All-datasets scope toggle switches engines in place.
// "Cards" itself is not rendered as a toggle either: it is the dataset's default
// view, and the breadcrumb's Cards crumb is the way back to it from Review/Tags.
const TABS: { id: TabId; label: string; icon: LucideIcon }[] = [
  { id: "cards", label: "Cards", icon: Layers },
  { id: "review", label: "Review", icon: GraduationCap },
  { id: "tags", label: "Tags", icon: Tag },
];

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

  // Toolbar "Locations" mode: shows the location management view as an overlay,
  // reached from the overflow menu on the dataset list.
  const [showManage, setShowManage] = useState(false);
  // Bumped by the shared page refresh so an open Locations overlay re-reads its
  // own directory list instead of keeping the one it mounted with.
  const [locRefresh, setLocRefresh] = useState(0);

  // Overflow (three-dot) menu visibility — Locations currently lives in there.
  const [menuOpen, setMenuOpen] = useState(false);

  // Use global card editor
  const { openCardEditor, isOpen: editorOpen } = useCardEditor();

  // Review state
  const [reviewCard, setReviewCard] = useState<Card | null>(null);
  const [reviewFlipped, setReviewFlipped] = useState(false);
  const [reviewResult, setReviewResult] = useState<CardReview | null>(null);
  const [reviewStats, setReviewStats] = useState<CardTestStats | null>(null);
  // All-datasets review: which dataset the drawn card came from, so the panel can name
  // it and submitReview can rate it in the right place. Empty in a dataset-scoped
  // session, where reviewScope is the source and the submit targets it directly.
  const [reviewDatasetUuid, setReviewDatasetUuid] = useState<string>("");
  const [reviewDatasetName, setReviewDatasetName] = useState<string>("");
  // Which dataset the review session draws from: "" = all datasets (the default),
  // otherwise a dataset uuid picked in the review page's own selector. This is
  // a filter on the session, not navigation — selecting a dataset here does not
  // open it, so the review page stays a page of its own.
  const [reviewScope, setReviewScope] = useState<string>("");

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

  // Unified search scope within the Cards view: "dataset" filters the open dataset
  // live via card_list; "all" runs a cross-dataset FTS5 query via card_search.
  const [searchScope, setSearchScope] = useState<"dataset" | "all">("dataset");

  // Search sub-toolbar visibility: toggled by the toolbar's search button and
  // hidden by default (the wiki page's idiom). Hiding also resets the view to
  // the unfiltered dataset list, so a live filter can't stay applied invisibly.
  const [searchOpen, setSearchOpen] = useState(false);

  function toggleSearch() {
    if (searchOpen) {
      setSearchOpen(false);
      setSearchScope("dataset");
      setKeyword("");
      setPage(0);
    } else {
      // The search row lives in the Cards view, so opening it from Review/Tags
      // jumps there first. At the dataset-list root there is no current dataset, so
      // search is inherently cross-dataset — force the "all datasets" scope.
      setActiveTab("cards");
      if (!selectedDatasetUuid) setSearchScope("all");
      setSearchOpen(true);
    }
  }

  // The search row lives only in the Cards view, so leaving that view closes
  // it — keeping the toolbar button's active state honest.
  useEffect(() => {
    if (activeTab !== "cards") setSearchOpen(false);
  }, [activeTab]);

  // Error display
  const [error, setError] = useState<string | null>(null);

  // Deferred platform flag (SSR-safe) for collapsing toolbar button labels on mobile.
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    setMobile(isMobileApp());
  }, []);

  // Load datasets on mount
  const loadDatasets = useCallback(async () => {
    try {
      setLoading(true);
      const result = await invoke<CardDatasetSummary[]>("card_dataset_list");
      setDatasets(result);
    } catch (e) {
      setError(`Failed to load datasets: ${e}`);
    } finally {
      setLoading(false);
    }
  }, []);

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

  // Re-read everything this page shows — used by the shared page refresh.
  const handleRefresh = useCallback(() => {
    void loadDatasets();
    if (selectedDatasetUuid) void loadCards();
    setLocRefresh((n) => n + 1);
  }, [loadDatasets, loadCards, selectedDatasetUuid]);

  // Keep the dataset/card lists current without a toolbar button: the shared
  // hook re-reads on activation, on app resume and on `dataset-list-changed`.
  // Dialog drafts are component-local state, so a re-read cannot destroy one.
  usePageRefresh("cards", {
    refresh: handleRefresh,
    events: ["dataset-list-changed"],
  });

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
  // describe the same pool the draw just picked from: one dataset's queue when a
  // dataset is selected, the aggregate over every dataset in an all-datasets session.
  const loadReviewStats = useCallback(async () => {
    try {
      setReviewStats(
        reviewScope
          ? await invoke<CardTestStats>("card_test_stats", {
              datasetUuid: reviewScope,
            })
          : await invoke<CardTestStats>("card_test_stats_all")
      );
    } catch {
      // Informational only, so a failure just hides the bar.
      setReviewStats(null);
    }
  }, [reviewScope]);

  // Get next review card
  const loadNextReview = useCallback(async () => {
    try {
      if (reviewScope) {
        // Dataset-scoped session: the source dataset is the one selected above.
        const result = await invoke<[Card, CardReview] | null>("card_test_get", {
          datasetUuid: reviewScope,
        });
        setReviewCard(result ? result[0] : null);
        setReviewDatasetUuid("");
        setReviewDatasetName("");
      } else {
        // All-datasets session: the drawn card names its own source dataset, which
        // submitReview must target.
        const result = await invoke<CrossDatasetCard | null>("card_test_get_all");
        setReviewCard(result ? result.card : null);
        setReviewDatasetUuid(result ? result.dataset_uuid : "");
        setReviewDatasetName(result ? result.dataset_name : "");
      }
      setReviewFlipped(false);
      setReviewResult(null);
    } catch (e) {
      setError(`Failed to load review card: ${e}`);
    } finally {
      void loadReviewStats();
    }
  }, [reviewScope, loadReviewStats]);

  useEffect(() => {
    // Review works at the dataset-list root too (all datasets), so it never required
    // an open dataset — just the review mode. Changing the scope rebuilds
    // loadNextReview, which is what re-draws from the new pool.
    if (activeTab === "review") {
      loadNextReview();
    }
  }, [activeTab, loadNextReview]);

  // Agent control: `app_open_review` puts this page into review/quiz mode,
  // optionally preselecting a card dataset. The review-load effect above picks
  // up the tab/dataset change and draws the first due card.
  useEffect(() => {
    const unlisten = listen<{ dataset_uuid: string | null }>("agent-open-review", (event) => {
      const uuid = event.payload?.dataset_uuid;
      // Scope the session to the requested dataset, or to every dataset when none was
      // named. Also open that dataset, so the breadcrumb shows where the review is.
      setReviewScope(uuid ?? "");
      if (uuid) setSelectedDatasetUuid(uuid);
      setActiveTab("review");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // ── Agent-drivable actions ──
  // The same three things the toolbar offers, reached generically: open a
  // dataset, start a review session, search across all of them. `start-review`
  // is the registry twin of the `agent-open-review` handler above — one body
  // would be nicer, but that handler is an event listener for a legacy tool and
  // this one has to return a result, so they stay side by side.
  useAgentActions("cards", [
    {
      id: "cards.select-dataset",
      label: "Open a card dataset and show its cards.",
      kind: "read",
      params_schema: {
        dataset_uuid: {
          type: "string",
          description: "The card dataset uuid, from dataset_list.",
          required: true,
        },
      },
      run: (params) => {
        const uuid = reqStr("cards.select-dataset", params, "dataset_uuid");
        const found = datasets.find((x) => x.info.uuid === uuid);
        if (!found) {
          throw new Error(
            `no card dataset with uuid '${uuid}' is loaded on this page. Loaded: ${
              datasets.map((x) => `${x.info.name} (${x.info.uuid})`).join(", ") ||
              "(none yet — the list may still be loading; retry once)"
            }. dataset_list has the full set across every kind.`
          );
        }
        setSearchOpen(false);
        setSelectedDatasetUuid(uuid);
        setActiveTab("cards");
        return { opened: uuid, name: found.info.name };
      },
    },
    {
      id: "cards.start-review",
      label:
        "Put this page into the interactive review/quiz panel. Grading still happens in the UI, so this only starts the session.",
      kind: "read",
      params_schema: {
        dataset_uuid: {
          type: "string",
          description:
            "Draw only from this card dataset. Omit to draw from every dataset that has cards due.",
        },
      },
      run: (params) => {
        const uuid = optStr("cards.start-review", params, "dataset_uuid");
        if (uuid && !datasets.some((x) => x.info.uuid === uuid)) {
          throw new Error(
            `no card dataset with uuid '${uuid}'. Loaded: ${
              datasets.map((x) => `${x.info.name} (${x.info.uuid})`).join(", ") || "(none)"
            }`
          );
        }
        setReviewScope(uuid ?? "");
        if (uuid) setSelectedDatasetUuid(uuid);
        setActiveTab("review");
        return {
          started: true,
          scope: uuid ?? "all-datasets",
          note: "The review-load effect draws the first due card. Read the queue with card_test_next (or card_test_next_all for an unscoped session); grading stays in the UI.",
        };
      },
    },
    {
      id: "cards.search",
      label: "Search every card dataset (full text, or questions only) and show the hits in the search panel.",
      kind: "read",
      params_schema: {
        query: { type: "string", description: "The search text.", required: true },
        mode: {
          type: "string",
          description: "fulltext (default) matches question, answer and note; question matches questions only.",
          enum: ["fulltext", "question"],
        },
      },
      run: async (params) => {
        const id = "cards.search";
        const query = reqStr(id, params, "query");
        const mode = oneOf(id, params, "mode", ["fulltext", "question"] as const, "fulltext");
        setSearchOpen(true);
        setSearchScope("all");
        setActiveTab("cards");
        setGlobalSearchQuery(query);
        setSearchMode(mode);
        const hits = await handleGlobalSearch(query, mode);
        const shown = 10;
        return {
          query,
          mode,
          count: hits.length,
          truncated: hits.length > shown,
          hits: hits.slice(0, shown).map((h) => ({
            dataset_uuid: h.dataset_uuid,
            dataset_name: h.dataset_name,
            card_uuid: h.card_uuid,
            question: h.question,
            answer: h.answer,
          })),
        };
      },
    },
  ]);

  // Submit review
  const submitReview = useCallback(
    async (quality: number) => {
      // The dataset to rate against: the scoped one, or the source dataset of a
      // draw that came out of the all-datasets pool.
      const targetDatasetUuid = reviewScope || reviewDatasetUuid;
      if (!reviewCard || !targetDatasetUuid) return;
      try {
        const result = await invoke<CardReview>("card_test_submit", {
          datasetUuid: targetDatasetUuid,
          cardUuid: reviewCard.uuid,
          quality,
        });
        setReviewResult(result);
        // Load next card after a short delay
        setTimeout(() => loadNextReview(), 800);
      } catch (e) {
        setError(`Failed to submit review: ${e}`);
      }
    },
    [reviewCard, reviewScope, reviewDatasetUuid, loadNextReview]
  );

  // Keyboard shortcuts for the review flow (Anki-style): Space/Enter flips the
  // card, 1–5 grade it, S skips. Live only while the review panel owns the
  // content, a card is drawn, and the card editor overlay is closed. `e.key`
  // (not `e.code`) so the digit row and numpad map to the same grade, and any
  // keystroke while typing in a field is left alone.
  useEffect(() => {
    if (activeTab !== "review" || !reviewCard || editorOpen) return;
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.tagName === "SELECT" ||
          t.isContentEditable)
      ) {
        return;
      }
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        setReviewFlipped((v) => !v);
      } else if (e.key === "s" || e.key === "S") {
        e.preventDefault();
        loadNextReview();
      } else if (e.key >= "1" && e.key <= "5") {
        e.preventDefault();
        submitReview(Number(e.key));
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeTab, reviewCard, editorOpen, loadNextReview, submitReview]);

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

  // Global FTS search across all datasets. The overrides exist for the
  // agent-driven action, which cannot wait a render for its own `setState` to
  // land; callers that pass nothing read the inputs exactly as before. Returns
  // the hits so a caller can report them instead of reading state back.
  async function handleGlobalSearch(
    queryOverride?: string,
    modeOverride?: "question" | "fulltext"
  ) {
    const query = (queryOverride ?? globalSearchQuery).trim();
    const mode = modeOverride ?? searchMode;
    if (!query) {
      setGlobalSearchResults([]);
      return [] as typeof globalSearchResults;
    }
    setSearching(true);
    try {
      const results = await invoke<
        { dataset_uuid: string; dataset_name: string; card_uuid: string; question: string; answer: string; note: string; location: string }[]
      >("card_search", { query, mode });
      setGlobalSearchResults(results);
      return results;
    } catch (e) {
      setError(`Search failed: ${e}`);
      return [] as typeof globalSearchResults;
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

  // Search is a page-level concern (works from the dataset-list root and from a
  // dataset's Cards view): the row shows while toggled on and not in Manage; the
  // cross-dataset results replace the normal content when scope is "all".
  const datasetOpen = !!selectedDatasetUuid;
  // With no dataset open there is nothing to filter in-place, so search is
  // inherently cross-dataset — effScope pins to "all" regardless of the toggle.
  const effScope = datasetOpen ? searchScope : "all";
  const searchActive = searchOpen && !showManage;
  const showAllResults = searchActive && effScope === "all";

  // Root-level sub-views that own the content area, named in the breadcrumb so
  // the trail reads Cards › Locations / Cards › Review instead of a bare "Cards".
  // Inside a dataset the trail stays Cards › <dataset>.
  const rootSubView = datasetOpen
    ? null
    : showManage
      ? "Locations"
      : activeTab === "review"
        ? "Review"
        : null;
  const crumbTail = selectedDataset ? selectedDataset.info.name : rootSubView;

  // Familiarity color — theme-aware ramp (see --fam-* in globals.css), so the
  // row tint tracks the active Light/Dark/Solarized/Gruvbox theme instead of a
  // fixed light pastel. Literal class names keep Tailwind's scanner happy.
  function familiarityColor(f: number) {
    const colors = [
      "bg-fam-0",    // 0 - new
      "bg-fam-1",    // 1 - strange
      "bg-fam-2",    // 2 - known
      "bg-fam-3",    // 3 - familiar
      "bg-fam-4",    // 4 - skilled
      "bg-fam-5",    // 5 - practiced
      "bg-fam-6",    // 6 - easy
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
    <main className="flex-1 flex flex-col min-h-0 min-w-0 bg-bg-body">
      {/* Thin-client reminder: card edits and reviews sit in the writeback
          queue until a sync round flushes them to the hub. */}
      <PendingWritebackBanner mobile={mobile} />
      {/* Breadcrumb navigation — wiki/dictation-style full-width bar above the
          toolbar: the Cards › <dataset> trail, or Cards › Locations / Cards › Review
          for the root-level sub-views. The root "Cards" crumb is the dataset list;
          it becomes a link back to it whenever a dataset or a sub-view owns the page. */}
      <Breadcrumb>
        {crumbTail ? (
          <Breadcrumb.Link
            onClick={() => {
              // Inside a dataset, "back to Cards" means the dataset's card list — stay in
              // the dataset and only leave the Review/Tags mode (also unseeding the
              // dataset-scoped session). From any other level it goes to the dataset grid.
              if (selectedDatasetUuid && activeTab !== "cards") {
                setReviewScope("");
                setActiveTab("cards");
                return;
              }
              setSelectedDatasetUuid(""); setShowManage(false); setMenuOpen(false); setSearchScope("dataset"); setReviewScope(""); setActiveTab("cards");
            }}
          >
            Cards
          </Breadcrumb.Link>
        ) : (
          <Breadcrumb.Current grow={false}>Cards</Breadcrumb.Current>
        )}
        {crumbTail && (
          <>
            <Breadcrumb.Sep />
            <Breadcrumb.Current title={crumbTail}>{crumbTail}</Breadcrumb.Current>
          </>
        )}
      </Breadcrumb>

      {/* Toolbar — dataset-level info on the left, actions on the right (Wiki-style
          full-width bar under the breadcrumb): the mode toggles (in a dataset) or the
          dataset-list actions, then search, then the overflow menu. */}
      <Toolbar mobile={mobile}>
        <div className="flex-1 min-w-0" />
        {!showManage && (activeTab === "review" ? (
          <>
            {/* The review page's card-level actions live here. Its own mode toggles
                and dataset-list actions are hidden while it owns the screen, so the
                grade scale in the panel below is the only decision left in view. */}
            <ToolbarButton
              icon={FlipHorizontal}
              label={reviewFlipped ? "Question" : "Answer"}
              title={reviewFlipped ? "Show question (Space)" : "Show answer (Space)"}
              ariaLabel={reviewFlipped ? "Show question" : "Show answer"}
              disabled={!reviewCard}
              onClick={() => setReviewFlipped((v) => !v)}
            />
            <ToolbarButton
              icon={SkipForward}
              label="Skip"
              title="Skip this card (S)"
              ariaLabel="Skip this card"
              onClick={loadNextReview}
            />
          </>
        ) : selectedDatasetUuid ? (
          <>
            <div className="flex items-center gap-1 shrink-0">
              {/* Review / Tags only — the card list is the dataset's home view, not a
                  mode you toggle to; the breadcrumb's Cards crumb returns to it. */}
              {TABS.map((t) =>
                t.id === "cards" ? null : (
                  <ToolbarButton
                    key={t.id}
                    variant="toggle"
                    active={activeTab === t.id}
                    icon={t.icon}
                    label={t.label}
                    onClick={() => {
                      // Entering Review from inside a dataset keeps that dataset as the
                      // session scope; the selector can still widen it to all datasets.
                      // Exiting Review back to the card list is the breadcrumb's
                      // Cards crumb — the list has no toolbar toggle any more.
                      if (t.id === "review") setReviewScope(selectedDatasetUuid);
                      setActiveTab(t.id);
                    }}
                  />
                ),
              )}
            </div>
            {/* Add Card lives on the dataset toolbar (desktop) so it is always
                reachable — it used to hide inside the search row, which meant
                opening search just to create a card. On a phone it tucks into the
                overflow, since the stacked bar has no room for a one-shot action. */}
            {!mobile && (
              <ToolbarButton
                icon={Plus}
                label="Add Card"
                title="Add a new card"
                ariaLabel="Add card"
                onClick={() => openCardEditor(selectedDatasetUuid, null, () => loadCards())}
              />
            )}
          </>
        ) : (
          <>
            {/* Cross-dataset review — reachable from the dataset list with no dataset open.
                Shown only while the review page does NOT own the screen (the
                review toolbar carries Answer/Skip alone); the breadcrumb's Cards
                crumb is the way back. Opening it closes search so the review
                panel (not the search results) owns the content area. */}
            <ToolbarButton
              icon={GraduationCap}
              label="Review"
              title="Review cards across all datasets"
              ariaLabel="Review across all datasets"
              onClick={() => {
                setSearchOpen(false);
                setReviewScope("");
                setActiveTab("review");
              }}
            />
            {!mobile && (
              <ToolbarButton icon={Plus} label="New Dataset" title="Create new dataset" onClick={createDataset} />
            )}
          </>
        ))}
        {/* Search button — present at every level but Review, on the far right
            (just before the overflow menu): inside a dataset it toggles the
            This-dataset / All-datasets row, from the dataset list it opens a cross-dataset
            search directly. Hidden while the Locations overlay or the review page
            owns the content area — the review toolbar shows only Answer/Skip — so
            its active state always matches a visible row. */}
        {!showManage && activeTab !== "review" && (
          <ToolbarButton
            icon={Search}
            variant="toggleSoft"
            active={searchOpen}
            label="Search"
            title={searchOpen ? "Hide search" : "Show search"}
            ariaLabel="Toggle search"
            onClick={toggleSearch}
          />
        )}
        {/* Overflow menu — the three-dot home for occasional actions. On desktop it
            shows only at the dataset list and carries Locations (the former Manage
            toggle), a root-level concern. On a phone it also swallows Add Card /
            New Dataset, so the visible stacked row keeps just the contextual
            actions. Rendered only where it has an entry: the Refresh item that
            used to keep it alive at every level is gone, and an empty panel would
            be a dead control. Active state marks the overlay owning. */}
        {(!selectedDatasetUuid && activeTab !== "review") ||
        (mobile && selectedDatasetUuid && activeTab !== "review") ||
        (mobile && !selectedDatasetUuid) ? (
          <ToolbarOverflow open={menuOpen} onOpenChange={setMenuOpen}>
            {!selectedDatasetUuid && activeTab !== "review" && (
              <ToolbarMenuItem
                icon={FolderCog}
                checked={showManage}
                onClick={() => setShowManage((v) => !v)}
              >
                Locations
              </ToolbarMenuItem>
            )}
            {mobile && selectedDatasetUuid && activeTab !== "review" && (
              <ToolbarMenuItem
                icon={Plus}
                onClick={() => openCardEditor(selectedDatasetUuid, null, () => loadCards())}
              >
                Add Card
              </ToolbarMenuItem>
            )}
            {mobile && !selectedDatasetUuid && (
              <ToolbarMenuItem icon={Plus} onClick={createDataset}>
                New Dataset
              </ToolbarMenuItem>
            )}
          </ToolbarOverflow>
        ) : null}
      </Toolbar>

      {error && (
        <div className="mx-2 mt-2 p-2 rounded bg-red-500/10 text-red-500 text-sm sm:mx-6">
          {error}
          <button onClick={() => setError(null)} className="ml-2 underline">
            dismiss
          </button>
        </div>
      )}

      {/* Search row — page-level, toggled by the toolbar Search button, so it
          works both from the dataset-list root (always "All datasets") and from a
          dataset's Cards view ("This dataset" live filter vs cross-dataset FTS5). */}
      {searchActive && (
        <div className="shrink-0 px-2 py-2 flex flex-wrap items-center gap-2 border-b border-border-default sm:px-6">
          {datasetOpen && (
            <div className="flex items-center gap-1 shrink-0">
              <ToolbarButton
                variant="toggle"
                active={searchScope === "dataset"}
                label="This dataset"
                onClick={() => setSearchScope("dataset")}
              />
              <ToolbarButton
                variant="toggle"
                active={searchScope === "all"}
                label="All datasets"
                onClick={() => setSearchScope("all")}
              />
            </div>
          )}
          {effScope === "dataset" ? (
            <>
              <div className="relative flex-1 max-w-xs">
                <Search
                  size={14}
                  className="absolute left-2 top-1/2 -translate-y-1/2 text-text-tertiary"
                />
                <input
                  type="text"
                  placeholder="Search this dataset..."
                  value={keyword}
                  onChange={(e) => {
                    setKeyword(e.target.value);
                    setPage(0);
                  }}
                  className="w-full pl-7 pr-2 py-1 rounded border border-border-default bg-bg-card text-sm"
                  autoFocus
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
            </>
          ) : (
            <>
              <div className="relative flex-1 max-w-md">
                <Search
                  size={16}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary"
                />
                <input
                  type="text"
                  placeholder="Search all datasets... (FTS5)"
                  value={globalSearchQuery}
                  onChange={(e) => setGlobalSearchQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleGlobalSearch();
                  }}
                  className="w-full pl-9 pr-3 py-2 rounded-lg border border-border-default bg-bg-card text-sm"
                  autoFocus
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
                onClick={() => handleGlobalSearch()}
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
            </>
          )}
        </div>
      )}

      {/* Manage overlay (location management) when the toolbar Manage toggle is
          on; otherwise the dataset list (no dataset selected) or the dataset detail. */}
      {showManage ? (
        <div className="flex-1 min-h-0 overflow-hidden">
          <LocationsManager
            datasetType="card"
            description="Card datasets are stored across dataset directories. Add linked directories to store datasets on external drives or other locations."
            onError={setError}
            onChanged={loadDatasets}
            countFor={(path) => datasets.filter((d) => d.location === path).length}
            refreshToken={locRefresh}
          />
        </div>
      ) : showAllResults ? (
        <div className="flex-1 min-h-0 overflow-y-auto px-2 py-3 sm:px-6">
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
                    // Jump into the dataset (back to This-dataset scope) and open the card
                    setSelectedDatasetUuid(result.dataset_uuid);
                    setSearchScope("dataset");
                    invoke<Card>("card_get", {
                      datasetUuid: result.dataset_uuid,
                      cardUuid: result.card_uuid,
                    })
                      .then((card) => {
                        openCardEditor(result.dataset_uuid, card, () => {
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
      ) : (!selectedDatasetUuid && activeTab !== "review") ? (
        <div className="flex-1 min-h-0 overflow-y-auto px-4 py-4">
          {datasets.length === 0 ? (
            <p className="text-text-secondary">No card datasets found. Use "New Dataset" to create one.</p>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
              {datasets.map((ds) => (
                <button
                  key={ds.info.uuid}
                  onClick={() => {
                    setSelectedDatasetUuid(ds.info.uuid);
                    setActiveTab("cards");
                    setPage(0);
                    setShowManage(false);
                    setKeyword("");
                    setSearchScope("dataset");
                  }}
                  className="text-left p-4 rounded-xl border border-border-default bg-bg-card hover:border-accent-blue/40 transition-colors cursor-pointer flex flex-col gap-1"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-semibold truncate">{ds.info.name}</span>
                    <span
                      className={`text-[10px] px-1.5 py-0.5 rounded font-medium shrink-0 ${
                        ds.info.sharing.visibility === "public"
                          ? "bg-green-500/10 text-green-600"
                          : ds.info.sharing.visibility === "shared"
                            ? "bg-amber-500/10 text-amber-600"
                            : "bg-mid-gray/20 text-text-tertiary"
                      }`}
                    >
                      {ds.info.sharing.visibility}
                    </span>
                  </div>
                  {ds.info.description && (
                    <p className="text-xs text-text-secondary line-clamp-2">{ds.info.description}</p>
                  )}
                  <span className="text-xs text-text-tertiary">
                    {ds.card_count} card{ds.card_count !== 1 ? "s" : ""}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      ) : (
        <>
          {/* Tab content */}
          <div className="flex-1 min-h-0 overflow-hidden">
        {/* Cards tab */}
        {activeTab === "cards" && (
          <div className="flex flex-col h-full">
            {/* Card list — the dataset's own cards. The page-level search row above
                drives a live keyword filter over these (This dataset) or swaps the
                content for cross-dataset results (All datasets). */}
            <div className="flex-1 overflow-y-auto px-2 py-2 sm:px-6">
              {cards.length === 0 ? (
                <p className="text-text-secondary text-center py-8">
                  {keyword || filter !== "all"
                    ? "No cards match your search."
                    : 'No cards yet. Use "Add Card" to create one.'}
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
                        <p className="text-lg font-medium whitespace-normal break-words sm:text-2xl">
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

        {/* Review tab. This panel is the only scroller: the card grows with its content
            instead of scrolling internally, so a long answer uses the full tab height.
            Content is top-aligned on purpose: centring it would move the toolbar up and
            down every time the answer is revealed or a longer card is drawn. */}
        {activeTab === "review" && (
          <div className="h-full overflow-y-auto p-3 sm:p-6">
            {/* One sticky block tops the review page: the scope selector, then the
                grade scale while a card is drawn. Sticky so the grade buttons stay
                reachable while a long answer scrolls (the same `sticky top-0 z-10
                bg-bg-body` idiom the dictation toolbar uses). The negative margins
                pull the block over the panel's own padding and its padding puts that
                space back, so the opaque background covers the gutters and the top
                edge too and no text peeks around it. */}
            <div className="sticky top-0 z-10 -mx-3 -mt-3 bg-bg-body px-3 pb-4 pt-3 sm:-mx-6 sm:-mt-6 sm:px-6 sm:pt-6">
              {/* Scope selector — the dataset this session draws from, "All datasets" by
                  default. A filter on the session rather than navigation, so picking
                  a dataset here does not open it. */}
              <div className="mb-3 flex items-center gap-2">
                <Layers size={14} className="shrink-0 text-text-tertiary" />
                <label htmlFor="review-scope" className="sr-only">
                  Review dataset
                </label>
                <select
                  id="review-scope"
                  value={reviewScope}
                  onChange={(e) => setReviewScope(e.target.value)}
                  className="min-w-0 max-w-xs rounded-lg border border-border-default bg-bg-card px-2 py-1.5 text-sm hover:bg-mid-gray/20"
                >
                  <option value="">All datasets</option>
                  {datasets.map((ds) => (
                    <option key={ds.info.uuid} value={ds.info.uuid}>
                      {ds.info.name}
                    </option>
                  ))}
                </select>
              </div>
              {/* The grade scale alone, dead-centre on its row. Ratings are always on
                  screen, so a card can be graded from its question and nothing shifts
                  when the answer appears. Answer and Skip moved up into the page
                  toolbar, so nothing else shares this row. */}
              {reviewCard && (
                <div className="flex flex-wrap justify-center gap-2">
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
                      title={`${btn.label} (${btn.q})`}
                      aria-label={btn.label}
                      className={`flex items-center gap-1.5 px-4 py-2 min-h-11 sm:min-h-0 rounded-lg border border-border-default ${btn.color} text-sm hover:opacity-90`}
                    >
                      <btn.Icon size={14} />
                      <IconLabel>{btn.label}</IconLabel>
                    </button>
                  ))}
                </div>
              )}
            </div>
            {reviewCard ? (
              <div className="w-full">

                {/* In an all-datasets session (no single dataset scoped) name the source
                    dataset, so it is clear where this card came from and where the
                    rating is recorded. */}
                {!reviewScope && reviewDatasetName && (
                  <div className="mb-2 flex items-center gap-1.5 text-xs text-text-tertiary">
                    <Layers size={12} className="shrink-0" />
                    <span className="truncate">{reviewDatasetName}</span>
                  </div>
                )}

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

                <ReviewQueueBar stats={reviewStats} allDatasets={!reviewScope} />
              </div>
            ) : (
              <div className="text-center">
                <ReviewQueueBar stats={reviewStats} allDatasets={!reviewScope} />
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

          </div>
        </>
      )}
    </main>
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
function ReviewQueueBar({ stats, allDatasets }: { stats: CardTestStats | null; allDatasets?: boolean }) {
  if (!stats) return null;

  // The counts describe one dataset's queue when a dataset is scoped, or the aggregate
  // across every dataset in an all-datasets session — the hint wording follows.
  const where = allDatasets ? "across all your datasets" : "in this dataset";
  let hint: string | null = null;
  if (stats.due > 0 && stats.fresh > 0) {
    hint = `${stats.fresh} ${plural(stats.fresh, "card is", "cards are")} waiting to learn ${where}, but the ${stats.due} overdue ${plural(stats.due, "card goes", "cards go")} first — rate this one to bring the next forward.`;
  } else if (stats.fresh > 0) {
    hint = `${stats.fresh} ${plural(stats.fresh, "card is", "cards are")} waiting to learn ${where} — answer this one to get the next.`;
  } else if (stats.due > 0) {
    hint = `No new cards left ${allDatasets ? "anywhere" : "here"} — only the ${stats.due} overdue ${plural(stats.due, "one remains", "ones remain")}.`;
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


