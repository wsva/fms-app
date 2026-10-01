"use client";

import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Sparkles,
  Loader2,
  Settings2,
  X,
  Zap,
  CheckCircle2,
  AlertCircle,
} from "lucide-react";
import type {
  BookMeta,
  BookChapter,
  BookSentence,
  BookSentenceWord,
} from "@/lib/read/types";
import type { LlmChatResponse } from "@/lib/llm/types";
import { isTauri } from "@/lib/tauri";
import { getUUID } from "./utils";
import WordGenSettings from "./WordGenSettings";
import { parseWordGenResponse, loadWordGenSettings } from "./wordGen";
import type { WordGenSettings as WordGenSettingsType } from "./wordGen";

// ---------------------------------------------------------------------------
// Log entry types
// ---------------------------------------------------------------------------

type LogEntry = {
  type: "info" | "success" | "error" | "progress";
  text: string;
};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

type Props = { books: BookMeta[] };

export default function BookAdvanced({ books }: Props) {
  const [bookUUID, setBookUUID] = useState("");

  // Word generation settings (synced from WordGenSettings component)
  const [wordGenSettings, setWordGenSettings] = useState<WordGenSettingsType>(loadWordGenSettings());

  // Execution state
  const [running, setRunning] = useState(false);
  const [cancelRequested, setCancelRequested] = useState(false);
  const [progress, setProgress] = useState({ current: 0, total: 0 });
  const [logs, setLogs] = useState<LogEntry[]>([]);

  const addLog = useCallback((type: LogEntry["type"], text: string) => {
    setLogs((prev) => [...prev, { type, text }]);
  }, []);

  const handleGenerate = async () => {
    if (!bookUUID || !isTauri() || running) return;
    setRunning(true);
    setCancelRequested(false);
    setLogs([]);
    setProgress({ current: 0, total: 0 });

    // Use current settings from WordGenSettings
    const { model, prompt, temperature } = wordGenSettings;

    try {
      // 1. Load all chapters
      addLog("info", "Loading chapters...");
      const chapters = await invoke<BookChapter[]>("book_list_chapters", {
        bookUuid: bookUUID,
      });
      if (chapters.length === 0) {
        addLog("error", "No chapters found in this book.");
        return;
      }
      addLog("info", `Found ${chapters.length} chapter(s).`);

      // 2. Collect all sentences across all chapters
      const allSentences: { chapter: BookChapter; sentence: BookSentence }[] = [];
      for (const ch of chapters) {
        const sentences = await invoke<BookSentence[]>("book_list_sentences", {
          bookUuid: bookUUID,
          chapterUuid: ch.uuid,
        });
        for (const s of sentences) {
          if (s.sentence_type === "text" && s.content.trim()) {
            allSentences.push({ chapter: ch, sentence: s });
          }
        }
      }
      addLog("info", `Found ${allSentences.length} text sentence(s) across all chapters.`);

      // 3. Filter to sentences without words
      addLog("info", "Checking which sentences have no words...");
      const sentencesWithoutWords: { chapter: BookChapter; sentence: BookSentence }[] = [];
      for (const item of allSentences) {
        const words = await invoke<BookSentenceWord[]>("book_list_words", {
          bookUuid: bookUUID,
          sentenceUuid: item.sentence.uuid,
        });
        if (words.length === 0) {
          sentencesWithoutWords.push(item);
        }
      }
      addLog(
        "info",
        `${sentencesWithoutWords.length} sentence(s) without words (out of ${allSentences.length}).`
      );

      if (sentencesWithoutWords.length === 0) {
        addLog("success", "All sentences already have words. Nothing to do!");
        return;
      }

      setProgress({ current: 0, total: sentencesWithoutWords.length });

      // 4. Process each sentence
      let totalWordsAdded = 0;
      let processedCount = 0;
      let errorCount = 0;

      for (const item of sentencesWithoutWords) {
        if (cancelRequested) {
          addLog("info", "Cancelled by user.");
          break;
        }

        const { chapter, sentence } = item;
        processedCount++;
        setProgress({ current: processedCount, total: sentencesWithoutWords.length });

        const sentencePreview =
          sentence.content.length > 60
            ? sentence.content.slice(0, 60) + "…"
            : sentence.content;
        addLog("progress", `[${processedCount}/${sentencesWithoutWords.length}] "${sentencePreview}"`);

        try {
          const res = await invoke<LlmChatResponse>("llm_chat", {
            model,
            messages: [
              { role: "system", content: prompt },
              { role: "user", content: sentence.content.trim() },
            ],
            temperature,
          });

          const parsed = parseWordGenResponse(res.content);
          if (!parsed) {
            addLog("error", `  ↳ Could not parse LLM response for sentence.`);
            errorCount++;
            continue;
          }

          let added = 0;
          for (const item of parsed) {
            if (!item.keep || !item.lemma) continue;
            const w: BookSentenceWord = {
              uuid: getUUID(),
              sentence_uuid: sentence.uuid,
              word: item.lemma,
              word_type: item.pos || "",
              note: item.surface !== item.lemma ? `from: ${item.surface}` : "",
            };
            await invoke("book_save_word", { bookUuid: bookUUID, word: w });
            added++;
          }

          totalWordsAdded += added;
          addLog("success", `  ↳ Added ${added} word(s).`);
        } catch (e) {
          addLog("error", `  ↳ Error: ${e instanceof Error ? e.message : String(e)}`);
          errorCount++;
        }
      }

      // 5. Summary
      addLog(
        "success",
        `Done! Added ${totalWordsAdded} word(s) to ${processedCount - errorCount} sentence(s).` +
          (errorCount > 0 ? ` ${errorCount} sentence(s) had errors.` : "")
      );
    } catch (e) {
      addLog("error", `Fatal error: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setRunning(false);
      setCancelRequested(false);
    }
  };

  const selectedBook = books.find((b) => b.uuid === bookUUID);

  return (
    <div className="flex flex-col gap-4 h-full w-full">
      {/* Book selector */}
      <div className="flex flex-col gap-1">
        <label className="text-sm font-medium text-text-secondary">Book</label>
        <select
          className="px-3 py-2 rounded-lg bg-bg-input border border-border-default text-text-primary focus:outline-none focus:border-accent cursor-pointer"
          value={bookUUID}
          onChange={(e) => setBookUUID(e.target.value)}
          disabled={running}
        >
          <option value="">Select a book...</option>
          {books.map((b) => (
            <option key={b.uuid} value={b.uuid}>
              {b.title}
            </option>
          ))}
        </select>
      </div>

      {/* Generate Words section */}
      {selectedBook && (
        <div className="flex flex-col gap-3 p-4 rounded-lg border border-border-default bg-bg-card">
          <div className="flex items-center gap-2">
            <Zap size={18} className="text-accent" />
            <h2 className="text-base font-semibold text-text-primary">
              Bulk Generate Words
            </h2>
          </div>
          <p className="text-sm text-text-secondary">
            Walk through all sentences in <strong>{selectedBook.title}</strong> that have no
            vocabulary words yet, and generate words for each one using the LLM.
          </p>

          {/* Action buttons + Settings */}
          <WordGenSettings disabled={running}>
            {({ settings, showSettings, setShowSettings }) => (
              <>
                <div className="flex flex-row items-center gap-2">
                  <button
                    className="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-sm font-medium bg-accent-bg/15 text-accent hover:bg-accent-bg/25 disabled:opacity-50 cursor-pointer transition-colors"
                    disabled={running || !bookUUID}
                    onClick={() => {
                      setWordGenSettings(settings);
                      handleGenerate();
                    }}
                  >
                    {running ? (
                      <Loader2 size={16} className="animate-spin" />
                    ) : (
                      <Sparkles size={16} />
                    )}
                    {running ? "Generating..." : "Generate Words"}
                  </button>

                  {running && (
                    <button
                      className="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-sm font-medium bg-red-500/10 border border-red-500/20 text-red-500 hover:bg-red-500/20 cursor-pointer transition-colors"
                      onClick={() => setCancelRequested(true)}
                    >
                      <X size={14} /> Cancel
                    </button>
                  )}

                  <button
                    className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-xs font-medium text-text-tertiary hover:bg-bg-hover cursor-pointer transition-colors"
                    onClick={() => setShowSettings(!showSettings)}
                  >
                    <Settings2 size={14} /> Settings
                  </button>
                </div>

                {/* Progress bar */}
                {running && progress.total > 0 && (
                  <div className="flex flex-col gap-1">
                    <div className="flex items-center justify-between text-xs text-text-secondary">
                      <span>
                        {progress.current} / {progress.total} sentences
                      </span>
                      <span>{Math.round((progress.current / progress.total) * 100)}%</span>
                    </div>
                    <div className="w-full h-2 rounded-full bg-bg-muted overflow-hidden">
                      <div
                        className="h-full bg-accent rounded-full transition-all duration-300"
                        style={{ width: `${(progress.current / progress.total) * 100}%` }}
                      />
                    </div>
                  </div>
                )}
              </>
            )}
          </WordGenSettings>
        </div>
      )}

      {/* Log output */}
      {logs.length > 0 && (
        <div className="flex-1 min-h-0 flex flex-col rounded-lg border border-border-default bg-bg-card overflow-hidden">
          <div className="px-3 py-2 border-b border-border-default flex items-center justify-between">
            <span className="text-sm font-medium text-text-primary">Log</span>
            <button
              className="text-xs text-text-tertiary hover:text-text-secondary cursor-pointer"
              onClick={() => setLogs([])}
            >
              Clear
            </button>
          </div>
          <div className="flex-1 overflow-y-auto p-3 flex flex-col gap-1 font-mono text-xs">
            {logs.map((entry, i) => (
              <div
                key={i}
                className={
                  entry.type === "error"
                    ? "text-red-500 flex items-start gap-1"
                    : entry.type === "success"
                    ? "text-green-500 flex items-start gap-1"
                    : entry.type === "progress"
                    ? "text-text-secondary"
                    : "text-text-tertiary"
                }
              >
                {entry.type === "success" && <CheckCircle2 size={12} className="mt-0.5 shrink-0" />}
                {entry.type === "error" && <AlertCircle size={12} className="mt-0.5 shrink-0" />}
                <span className="whitespace-pre-wrap break-all">{entry.text}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Empty state */}
      {!selectedBook && (
        <div className="flex-1 flex items-center justify-center text-text-tertiary text-sm">
          Select a book to see advanced operations.
        </div>
      )}
    </div>
  );
}
