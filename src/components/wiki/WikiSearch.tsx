"use client";

import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Search, X, FileText, CircleHelp } from "lucide-react";
import type { WikiSource } from "@/lib/wiki/types";

interface WikiSearchResult {
  file_path: string;
  file_name: string;
  relative_path: string;
  snippet: string;
  rank: number;
}

type SearchMode =
  | { kind: "legacy" }
  | { kind: "dataset"; uuid: string }
  | { kind: "hub"; uuid: string };

interface WikiSearchProps {
  mode: SearchMode;
  onResultClick: (source: WikiSource) => void;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// Map one raw FTS row to a WikiSource for the current search mode. Legacy rows
// carry an absolute `file_path`; dataset/hub rows carry a dataset-relative one.
function toSource(mode: SearchMode, r: WikiSearchResult): WikiSource {
  switch (mode.kind) {
    case "legacy":
      return { kind: "legacy", path: r.file_path };
    case "dataset":
      return { kind: "dataset", uuid: mode.uuid, rel: r.file_path };
    case "hub":
      return { kind: "hub-dataset", uuid: mode.uuid, rel: r.file_path, name: "" };
  }
}

export default function WikiSearch({ mode, onResultClick }: WikiSearchProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<WikiSearchResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [showResults, setShowResults] = useState(false);
  const [showHelp, setShowHelp] = useState(false);

  const performSearch = useCallback(async (keyword: string) => {
    if (!isTauri() || !keyword.trim()) {
      setResults([]);
      setShowResults(false);
      return;
    }

    setIsSearching(true);
    try {
      let searchResults: WikiSearchResult[];
      if (mode.kind === "dataset") {
        searchResults = await invoke<WikiSearchResult[]>("wiki_dataset_search", {
          uuid: mode.uuid,
          keyword: keyword.trim(),
        });
      } else if (mode.kind === "hub") {
        searchResults = await invoke<WikiSearchResult[]>("wiki_hub_search", {
          uuid: mode.uuid,
          keyword: keyword.trim(),
        });
      } else {
        searchResults = await invoke<WikiSearchResult[]>("wiki_search", {
          keyword: keyword.trim(),
        });
      }
      setResults(searchResults);
      setShowResults(true);
    } catch (err) {
      console.error("Search failed:", err);
      setResults([]);
    } finally {
      setIsSearching(false);
    }
  }, [mode]);

  // Debounced search
  useEffect(() => {
    const timer = setTimeout(() => {
      if (query.trim().length >= 2) {
        performSearch(query);
      } else {
        setResults([]);
        setShowResults(false);
      }
    }, 300);

    return () => clearTimeout(timer);
  }, [query, performSearch]);

  const clearSearch = () => {
    setQuery("");
    setResults([]);
    setShowResults(false);
  };

  const placeholder =
    mode.kind === "dataset" ? "Search this wiki dataset..."
    : mode.kind === "hub" ? "Search hub wiki dataset..."
    : "Search wiki...";

  return (
    <div className="relative">
      <div className="flex items-center gap-2 px-3 py-2 bg-bg-input border border-border-default rounded-lg">
        <Search size={16} className="text-text-tertiary shrink-0" />
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={placeholder}
          className="flex-1 bg-transparent outline-none text-sm text-text-primary placeholder:text-text-tertiary"
        />
        {query && (
          <button
            onClick={clearSearch}
            className="text-text-tertiary hover:text-text-primary transition-colors"
          >
            <X size={14} />
          </button>
        )}
        {isSearching && (
          <span className="text-xs text-text-tertiary">Searching...</span>
        )}
        <div className="relative">
          <button
            onClick={() => setShowHelp(!showHelp)}
            className="text-text-tertiary hover:text-text-primary transition-colors"
            title="Search syntax help"
          >
            <CircleHelp size={14} />
          </button>
          {showHelp && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setShowHelp(false)} />
              <div className="absolute right-0 top-full mt-2 w-72 bg-bg-card border border-border-default rounded-lg shadow-lg p-3 z-50">
                <h4 className="text-sm font-semibold text-text-primary mb-2">Search Syntax</h4>
                <div className="space-y-1.5 text-xs text-text-secondary">
                  <div>
                    <span className="text-text-primary font-medium">hello world</span>
                    <span className="ml-2">AND search (both terms)</span>
                  </div>
                  <div>
                    <span className="text-text-primary font-medium">hello*</span>
                    <span className="ml-2">prefix matching</span>
                  </div>
                  <div>
                    <span className="text-text-primary font-medium">&quot;hello world&quot;</span>
                    <span className="ml-2">exact phrase</span>
                  </div>
                  <div>
                    <span className="text-text-primary font-medium">hello OR world</span>
                    <span className="ml-2">either term</span>
                  </div>
                  <div>
                    <span className="text-text-primary font-medium">hello NOT world</span>
                    <span className="ml-2">exclude term</span>
                  </div>
                  <div>
                    <span className="text-text-primary font-medium">hello NEAR world</span>
                    <span className="ml-2">terms near each other</span>
                  </div>
                </div>
              </div>
            </>
          )}
        </div>
      </div>

      {/* Search results dropdown */}
      {showResults && (
        <div className="absolute top-full left-0 right-0 mt-1 bg-bg-card border border-border-default rounded-lg shadow-lg max-h-80 overflow-y-auto z-50">
          {results.length === 0 ? (
            <div className="px-3 py-4 text-sm text-text-tertiary text-center">
              No results found
            </div>
          ) : (
            <div className="py-1">
              {results.map((result) => (
                <button
                  key={`${mode.kind}:${result.file_path}`}
                  className="w-full px-3 py-2 text-left hover:bg-bg-hover transition-colors border-b border-border-light last:border-b-0"
                  onClick={() => {
                    onResultClick(toSource(mode, result));
                    setShowResults(false);
                  }}
                >
                  <div className="flex items-center gap-2 mb-1">
                    <FileText size={12} className="text-text-tertiary shrink-0" />
                    <span className="text-sm font-medium text-text-primary truncate">
                      {result.file_name}
                    </span>
                  </div>
                  <div className="text-xs text-text-tertiary truncate mb-1">
                    {result.relative_path}
                  </div>
                  <div
                    className="text-xs text-text-secondary line-clamp-2"
                    dangerouslySetInnerHTML={{ __html: result.snippet }}
                  />
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
