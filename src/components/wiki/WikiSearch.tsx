"use client";

import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Search, X, FileText } from "lucide-react";

interface WikiSearchResult {
  file_path: string;
  file_name: string;
  relative_path: string;
  snippet: string;
  rank: number;
}

interface WikiSearchProps {
  wikiDir: string;
  onResultClick: (path: string) => void;
}

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export default function WikiSearch({ wikiDir, onResultClick }: WikiSearchProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<WikiSearchResult[]>([]);
  const [isSearching, setIsSearching] = useState(false);
  const [showResults, setShowResults] = useState(false);

  const performSearch = useCallback(async (keyword: string) => {
    if (!isTauri() || !keyword.trim()) {
      setResults([]);
      setShowResults(false);
      return;
    }

    setIsSearching(true);
    try {
      const searchResults = await invoke<WikiSearchResult[]>("wiki_search", {
        keyword: keyword.trim(),
      });
      setResults(searchResults);
      setShowResults(true);
    } catch (err) {
      console.error("Search failed:", err);
      setResults([]);
    } finally {
      setIsSearching(false);
    }
  }, []);

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

  return (
    <div className="relative">
      <div className="flex items-center gap-2 px-3 py-2 bg-bg-input border border-border-default rounded-lg">
        <Search size={16} className="text-text-tertiary shrink-0" />
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search wiki..."
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
                  key={result.file_path}
                  className="w-full px-3 py-2 text-left hover:bg-bg-hover transition-colors border-b border-border-light last:border-b-0"
                  onClick={() => {
                    onResultClick(result.file_path);
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
