"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Play,
  X,
  MoreVertical,
  Trash2,
  Copy,
  RotateCcw,
  Sparkles,
  Loader2,
  Settings2,
} from "lucide-react";
import { MicGlyph } from "@/components/voice/MicGlyph";
import type { DrawerState, BookSentenceWord } from "@/lib/read/types";
import type { LlmChatResponse } from "@/lib/llm/types";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logInfo } from "@/lib/logger";
import { BG_COLORS } from "./utils";
import { getUUID } from "./utils";
import { highlightDifferences } from "./diff";
import WordGenSettings from "./WordGenSettings";
import {
  parseWordGenResponse,
  loadWordGenSettings,
  saveWordGenSetting,
} from "./wordGen";
import type { WordGenSettings as WordGenSettingsType } from "./wordGen";

const LS_KEY = "read_auto_replace_rules";
const LS_DRAWER_FONT_SCALE = "read_drawer_font_scale";

const DRAWER_FONT_SCALES = [
  { label: "Small", size: "0.875rem", weight: "normal" },
  { label: "Normal", size: "1rem", weight: "normal" },
  { label: "Large", size: "1.125rem", weight: "bold" },
  { label: "Large x2", size: "1.5rem", weight: "bold" },
  { label: "Large x3", size: "1.875rem", weight: "bold" },
];

const WORD_TYPE_OPTIONS = [
  "Noun", "Verb", "Adjective", "Adverb", "Other",
];
const DEFAULT_RULES_TEXT = `\
// add rules in the following format
// saved in local app storage; back it up yourself
{"from":" vor Christus ", "to": " v. Chr. "}
{"from":" zum Beispiel ", "to": " z.B. "}
{"from":" sogenannte ",   "to": " sog. "}
`;

function textToRules(text: string): [string, string][] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => !line.startsWith("//") && line.startsWith("{"))
    .flatMap((line) => {
      try {
        const { from, to } = JSON.parse(line);
        return from && to ? [[from, to] as [string, string]] : [];
      } catch {
        return [];
      }
    });
}

function loadRulesText(): string {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (raw !== null) return raw;
  } catch {
    /* ignore */
  }
  return DEFAULT_RULES_TEXT;
}

const SPECIAL_CHARS: { char: string; hint: string }[] = [
  { char: "–", hint: "en dash" },
  { char: "é", hint: "e acute" },
];

const PAIRED_CHARS: { left: string; right: string; hint: string }[] = [
  { left: "„", right: "“", hint: "German quotes „…“" },
  { left: "‚", right: "‘", hint: "Single German quotes ‚…‘" },
  { left: "»", right: "«", hint: "Chevrons in German" },
  { left: "«", right: "»", hint: "Chevrons in French" },
  { left: "›", right: "‹", hint: "Single chevrons ›…‹" },
];

type Props = {
  drawer: DrawerState;
  recognized: string;
  content: string;
  onContentChange: (v: string) => void;
  hasAudio: boolean;
  saving: boolean;
  recording: boolean;
  processing: boolean;
  bgColor: string | null;
  onBgColorChange: (v: string | null) => void;
  bookUUID: string;
  onClose: () => void;
  onClear: () => void;
  onPlay: () => void;
  onToggleRecording: () => void;
  onSaveAdd: () => void;
  onSaveEdit: () => void;
  onDiscard: () => void;
  onDelete: () => void;
  onInsertBefore: () => void;
  onInsertAfter: () => void;
  onParagraphBefore: () => void;
  onParagraphAfter: () => void;
};

export default function SentenceDrawer({
  drawer,
  recognized,
  content,
  onContentChange,
  hasAudio,
  saving,
  recording,
  processing,
  bgColor,
  onBgColorChange,
  bookUUID,
  onClose,
  onClear,
  onPlay,
  onToggleRecording,
  onSaveAdd,
  onSaveEdit,
  onDiscard,
  onDelete,
  onInsertBefore,
  onInsertAfter,
  onParagraphBefore,
  onParagraphAfter,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [savingWords, setSavingWords] = useState(false);
  const [colorPickerOpen, setColorPickerOpen] = useState(false);
  const [rulesText, setRulesText] = useState(DEFAULT_RULES_TEXT);
  const [showRulesEditor, setShowRulesEditor] = useState(false);
  const [words, setWords] = useState<BookSentenceWord[]>([]);
  const [newWord, setNewWord] = useState("");
  const [newWordType, setNewWordType] = useState("");
  const [generatingWords, setGeneratingWords] = useState(false);
  const [wordGenError, setWordGenError] = useState("");
  const [wordGenSettings, setWordGenSettings] = useState<WordGenSettingsType>(loadWordGenSettings());
  const [wordGenResponse, setWordGenResponse] = useState("");
  const [drawerFontScale, setDrawerFontScale] = useState(1);
  const [pendingIds, setPendingIds] = useState<string[]>([]);
  // Deferred platform flag (SSR-safe): on mobile the header splits into two
  // rows so the controls never clip on a narrow screen.
  const [mobile, setMobile] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // Generated or manually typed words not yet written to the database;
  // flushed on header Save.
  const pendingWordsRef = useRef<BookSentenceWord[]>([]);

  const syncPendingIds = () => setPendingIds(pendingWordsRef.current.map((w) => w.uuid));

  const editUUID = drawer?.mode === "edit" ? drawer.sentence.uuid : null;

  useEffect(() => {
    setRulesText(loadRulesText());
    setWordGenSettings(loadWordGenSettings());
    setMobile(isMobileApp());
    const fs = localStorage.getItem(LS_DRAWER_FONT_SCALE);
    if (fs !== null) setDrawerFontScale(parseInt(fs) || 1);
  }, []);

  // Load vocabulary words when editing a sentence.
  useEffect(() => {
    setMenuOpen(false);
    if (!editUUID || !isTauri()) {
      setWords([]);
      return;
    }
    invoke<BookSentenceWord[]>("book_list_words", {
      bookUuid: bookUUID,
      sentenceUuid: editUUID,
    })
      .then(setWords)
      .catch(() => setWords([]));
  }, [editUUID, bookUUID]);

  // Reset unsaved words when switching sentences.
  useEffect(() => {
    pendingWordsRef.current = [];
    setPendingIds([]);
  }, [editUUID]);

  if (!drawer) return null;

  const applyRules = () => {
    let result = content;
    for (const [from, to] of textToRules(rulesText)) {
      result = result.split(from).join(to);
    }
    onContentChange(result);
  };

  const applySmartQuotes = () => {
    const parts = content.split('"');
    const result = parts.reduce(
      (acc, part, i) => (i === 0 ? part : acc + (i % 2 === 1 ? "„" : "“") + part),
      ""
    );
    onContentChange(result);
  };

  const insertAtCursor = (char: string) => {
    const el = textareaRef.current;
    if (!el) {
      onContentChange(content + char);
      return;
    }
    const start = el.selectionStart ?? content.length;
    const end = el.selectionEnd ?? content.length;
    onContentChange(content.slice(0, start) + char + content.slice(end));
    requestAnimationFrame(() => {
      el.selectionStart = start + char.length;
      el.selectionEnd = start + char.length;
      el.focus();
    });
  };

  const wrapSelection = (left: string, right: string) => {
    const el = textareaRef.current;
    if (!el) return;
    const start = el.selectionStart ?? content.length;
    const end = el.selectionEnd ?? content.length;
    const selected = content.slice(start, end);
    const newText = content.slice(0, start) + left + selected + right + content.slice(end);
    onContentChange(newText);
    requestAnimationFrame(() => {
      el.selectionStart = start + left.length;
      el.selectionEnd = start + left.length + selected.length;
      el.focus();
    });
  };

  const flushPendingWords = async () => {
    while (pendingWordsRef.current.length > 0) {
      const w = pendingWordsRef.current[0];
      await invoke("book_save_word", { bookUuid: bookUUID, word: w });
      pendingWordsRef.current = pendingWordsRef.current.slice(1);
      syncPendingIds();
    }
  };

  const handleHeaderSave = async () => {
    // Write queued words first; keep the drawer open if that fails, so the
    // unsaved batch is not lost.
    if (pendingWordsRef.current.length > 0) {
      setSavingWords(true);
      try {
        await flushPendingWords();
      } catch (e) {
        setWordGenError(`Failed to save words: ${e instanceof Error ? e.message : String(e)}`);
        setSavingWords(false);
        return;
      }
      setSavingWords(false);
    }
    if (drawer.mode === "add") onSaveAdd();
    else onSaveEdit();
  };

  const handleDiscard = () => {
    // Throw away words that were never written to the database.
    pendingWordsRef.current = [];
    setPendingIds([]);
    onDiscard();
  };

  const addWord = () => {
    if (!editUUID || !newWord.trim() || !isTauri()) return;
    const w: BookSentenceWord = {
      uuid: getUUID(),
      sentence_uuid: editUUID,
      word: newWord.trim(),
      word_type: newWordType.trim(),
      note: "",
    };
    // Queue it like generated words; written on header Save.
    setWords((prev) => [...prev, w]);
    pendingWordsRef.current.push(w);
    syncPendingIds();
    setNewWord("");
    setNewWordType("");
  };

  const removeWord = async (uuid: string) => {
    // Drop from the unsaved queue first; a pending word never reached the DB.
    if (pendingWordsRef.current.some((w) => w.uuid === uuid)) {
      pendingWordsRef.current = pendingWordsRef.current.filter((w) => w.uuid !== uuid);
      syncPendingIds();
      setWords((prev) => prev.filter((w) => w.uuid !== uuid));
      return;
    }
    if (!isTauri()) return;
    try {
      await invoke("book_delete_word", { bookUuid: bookUUID, uuid });
      setWords((prev) => prev.filter((w) => w.uuid !== uuid));
    } catch {
      /* ignore */
    }
  };

  const generateWords = async () => {
    if (!editUUID || !content.trim() || !isTauri()) return;
    setGeneratingWords(true);
    setWordGenError("");
    setWordGenResponse("");
    const messages = [
      {
        role: "system",
        content: wordGenSettings.prompt,
      },
      {
        role: "user",
        content: content.trim(),
      },
    ];
    logInfo(
      `[word_gen] llm_chat request: ${JSON.stringify({
        model: wordGenSettings.model,
        temperature: wordGenSettings.temperature,
        messages,
      })}`,
      "word_gen",
    );
    try {
      const res = await invoke<LlmChatResponse>("llm_chat", {
        model: wordGenSettings.model,
        messages,
        temperature: wordGenSettings.temperature,
      });
      logInfo(
        `[word_gen] llm_chat response: ${JSON.stringify(res)}`,
        "word_gen",
      );

      // Extract JSON array from the response (LLM may wrap it in markdown code blocks)
      const raw = res.content;
      setWordGenResponse(raw);
      const parsed = parseWordGenResponse(raw);
      if (!parsed) {
        setWordGenError("Could not parse LLM response");
        return;
      }

      // Get existing word lemmas to avoid duplicates
      const existingLemmas = new Set(words.map((w) => w.word.toLowerCase()));

      let added = 0;
      for (const item of parsed) {
        if (!item.keep || !item.lemma) continue;
        const lemmaLower = item.lemma.toLowerCase();
        if (existingLemmas.has(lemmaLower)) continue;
        existingLemmas.add(lemmaLower);

        const w: BookSentenceWord = {
          uuid: getUUID(),
          sentence_uuid: editUUID,
          word: item.lemma,
          word_type: item.pos || "",
          note: item.surface !== item.lemma ? `from: ${item.surface}` : "",
        };
        // Hold the word in local state; it is written to the database when
        // the header Save button is clicked.
        setWords((prev) => [...prev, w]);
        pendingWordsRef.current.push(w);
        added++;
      }

      if (added === 0) {
        setWordGenError("No new words to add");
      } else {
        syncPendingIds();
      }
    } catch (e) {
      setWordGenError(`Failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setGeneratingWords(false);
    }
  };

  const menuItem =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2";

  const closeBtn = (
    <button
      className="p-1.5 rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
      onClick={onClose}
      title="Close"
      aria-label="Close"
    >
      <X size={18} />
    </button>
  );

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col justify-end bg-black/40"
      onClick={onClose}
    >
      <div
        className="bg-bg-elevated rounded-t-2xl shadow-2xl w-full h-[80vh] flex flex-col border-t border-border-default [&_*]:![font-size:inherit] [&_*]:![font-weight:inherit]"
        style={{ fontSize: DRAWER_FONT_SCALES[drawerFontScale].size, fontWeight: DRAWER_FONT_SCALES[drawerFontScale].weight }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className={`flex ${mobile ? "flex-col gap-2" : "flex-row items-center justify-between"} px-4 pt-4 pb-2 border-b border-border-default`}>
          <div className={`flex items-center ${mobile ? "justify-between w-full gap-2" : ""}`}>
            <span className="font-semibold text-base text-text-primary">
              {drawer.mode === "edit" ? "Edit Sentence" : "New Sentence"}
            </span>
            {mobile && closeBtn}
          </div>
          <div className={`flex flex-row items-center gap-2 ${mobile ? "flex-wrap justify-end" : ""}`}>
            <select
              className="px-2 py-1 rounded-md bg-bg-muted border border-border-default text-text-primary cursor-pointer"
              value={drawerFontScale}
              onChange={(e) => {
                const v = parseInt(e.target.value);
                setDrawerFontScale(v);
                localStorage.setItem(LS_DRAWER_FONT_SCALE, String(v));
              }}
            >
              {DRAWER_FONT_SCALES.map((s, i) => (
                <option key={i} value={i}>{s.label}</option>
              ))}
            </select>
            {pendingIds.length > 0 && (
              <span className="text-xs text-amber-500">
                {pendingIds.length} unsaved word{pendingIds.length > 1 ? "s" : ""}
              </span>
            )}
            <button
              className="px-4 py-1.5 rounded-md bg-accent-bg text-white text-sm font-medium hover:opacity-90 disabled:opacity-50 cursor-pointer"
              disabled={saving || savingWords}
              onClick={handleHeaderSave}
            >
              {drawer.mode === "add" ? "Add" : "Save"}
            </button>
            <button
              className="px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
              disabled={saving || savingWords}
              onClick={handleDiscard}
            >
              Discard
            </button>

            {drawer.mode === "edit" && (
              <div className="relative">
                <button
                  className="p-1.5 rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
                  disabled={saving}
                  onClick={() => setMenuOpen((v) => !v)}
                >
                  <MoreVertical size={18} />
                </button>
                {menuOpen && (
                  <div
                    className="absolute right-0 top-full mt-1 w-56 rounded-lg border border-border-default bg-bg-card shadow-xl z-10 py-1"
                    onMouseLeave={() => setMenuOpen(false)}
                  >
                    <button
                      className={menuItem}
                      onClick={() => {
                        navigator.clipboard.writeText(content);
                        setMenuOpen(false);
                      }}
                    >
                      <Copy size={15} /> Copy
                    </button>
                    <button
                      className={menuItem}
                      onClick={() => {
                        onInsertBefore();
                        setMenuOpen(false);
                      }}
                    >
                      Insert Sentence Before
                    </button>
                    <button
                      className={menuItem}
                      onClick={() => {
                        onInsertAfter();
                        setMenuOpen(false);
                      }}
                    >
                      Insert Sentence After
                    </button>
                    <button
                      className={menuItem}
                      onClick={() => {
                        onParagraphBefore();
                        setMenuOpen(false);
                      }}
                    >
                      Insert Paragraph Before
                    </button>
                    <button
                      className={menuItem}
                      onClick={() => {
                        onParagraphAfter();
                        setMenuOpen(false);
                      }}
                    >
                      Insert Paragraph After
                    </button>
                    <button
                      className={`${menuItem} text-red-500`}
                      onClick={() => {
                        onDelete();
                        setMenuOpen(false);
                      }}
                    >
                      <Trash2 size={15} /> Delete
                    </button>
                  </div>
                )}
              </div>
            )}

            {!mobile && closeBtn}
          </div>
        </div>

        {/* Body */}
        <div className="flex flex-col gap-3 p-4 flex-1 min-h-0 overflow-y-auto">
          {/* Audio + Recording */}
          <div className="flex flex-row flex-wrap items-center gap-2">
            {hasAudio && (
              <button
                className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-bg-muted border border-border-default text-sm hover:bg-bg-hover cursor-pointer"
                onClick={onPlay}
              >
                <Play size={16} /> Play
              </button>
            )}
            <button
              className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-bg-muted border border-border-default text-sm hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
              disabled={!recording && processing}
              onClick={onToggleRecording}
            >
              <MicGlyph size={16} />
              {recording ? "Stop" : processing ? "Processing\u2026" : "Record"}
            </button>
            {drawer.mode === "add" && (
              <button
                className="flex items-center gap-2 px-3 py-1.5 rounded-md bg-red-500/10 border border-red-500/20 text-sm text-red-500 hover:bg-red-500/20 cursor-pointer"
                onClick={onClear}
              >
                <RotateCcw size={14} /> Clear
              </button>
            )}
          </div>

          {/* STT diff */}
          {hasAudio && recognized && (
            <div
              className="bg-bg-muted rounded p-2 text-text-primary text-xl"
            >
              {highlightDifferences(content, recognized)}
            </div>
          )}

          {/* Special characters + auto-replace */}
          <div className="flex flex-row flex-wrap items-center gap-1">
            {SPECIAL_CHARS.map(({ char, hint }) => (
              <button
                key={char}
                title={hint}
                className="px-2 py-0.5 text-sm rounded bg-bg-muted hover:bg-bg-hover border border-border-default font-mono cursor-pointer"
                onClick={() => insertAtCursor(char)}
              >
                {char}
              </button>
            ))}
            {PAIRED_CHARS.map(({ left, right, hint }) => (
              <button
                key={left + right}
                title={hint}
                className="px-2 py-0.5 text-sm rounded bg-bg-muted hover:bg-bg-hover border border-border-default font-mono cursor-pointer"
                onClick={() => wrapSelection(left, right)}
              >
                {left}{right}
              </button>
            ))}
            <div className="ml-auto flex flex-row gap-1">
              <button
                className="px-2 py-0.5 text-sm rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
                title={'Replace "..." with „...“'}
                onClick={applySmartQuotes}
              >
                „“
              </button>
              <button
                className="px-2 py-0.5 text-sm rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
                title="Apply auto-replace rules"
                onClick={applyRules}
              >
                Apply
              </button>
              <button
                className="px-2 py-0.5 text-sm rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
                title="Edit auto-replace rules"
                onClick={() => setShowRulesEditor(true)}
              >
                Rules
              </button>
            </div>
          </div>

          {/* Content editor */}
          <textarea
            data-no-voice
            ref={textareaRef}
            className="w-full px-3 py-2 rounded-lg bg-bg-input border border-border-default text-text-primary resize-y focus:outline-none focus:border-accent text-2xl font-bold"
            rows={3}
            value={content}
            onChange={(e) => onContentChange(e.target.value)}
          />

          {/* Background color selector */}
          <div className="flex flex-row items-center gap-2">
            <span className="text-xs text-text-secondary shrink-0">Color:</span>
            <div className="relative flex-1 min-w-0">
              <button
                className="flex items-center gap-2 px-2 py-1 rounded-md border border-border-default bg-bg-input cursor-pointer hover:bg-bg-hover w-full"
                onClick={() => setColorPickerOpen((v) => !v)}
              >
                {bgColor ? (
                  <>
                    <span className={`w-4 h-4 rounded-full ${BG_COLORS.find((c) => c.key === bgColor)?.swatch ?? "bg-bg-input"}`} />
                    <span className="text-xs">
                      {BG_COLORS.find((c) => c.key === bgColor)?.label}{" "}
                      <span className="text-text-tertiary">— {BG_COLORS.find((c) => c.key === bgColor)?.description}</span>
                    </span>
                  </>
                ) : (
                  <span className="text-xs text-text-tertiary">None</span>
                )}
              </button>
              {colorPickerOpen && (
                <div
                  className="absolute left-0 top-full mt-1 w-full rounded-lg border border-border-default bg-bg-card shadow-xl z-10 py-1"
                  onMouseLeave={() => setColorPickerOpen(false)}
                >
                  <button
                    className="w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2"
                    onClick={() => { onBgColorChange(null); setColorPickerOpen(false); }}
                  >
                    <span className="w-4 h-4 rounded-full bg-bg-input border border-border-default" />
                    None
                  </button>
                  {BG_COLORS.map((c) => (
                    <button
                      key={c.key}
                      className="w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2"
                      onClick={() => { onBgColorChange(bgColor === c.key ? null : c.key); setColorPickerOpen(false); }}
                    >
                      <span className={`w-4 h-4 rounded-full ${c.swatch}`} />
                      <span>{c.label}</span>
                      <span className="text-text-tertiary">— {c.description}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>

          {/* AI Generate Words - always visible */}
          <WordGenSettings disabled={generatingWords}>
            {({ settings, showSettings, setShowSettings }) => (
              <>
                <div className="flex flex-row items-center justify-end gap-2">
                  <button
                    className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium text-text-tertiary hover:bg-bg-hover cursor-pointer transition-colors"
                    onClick={() => setShowSettings(!showSettings)}
                  >
                    <Settings2 size={14} /> Settings
                  </button>
                  <button
                    className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium bg-accent-bg/15 text-accent hover:bg-accent-bg/25 disabled:opacity-50 cursor-pointer transition-colors"
                    disabled={generatingWords || !content.trim() || drawer.mode !== "edit"}
                    onClick={() => {
                      setWordGenSettings(settings);
                      generateWords();
                    }}
                    title={drawer.mode !== "edit" ? "Save the sentence first" : undefined}
                  >
                    {generatingWords ? (
                      <Loader2 size={14} className="animate-spin" />
                    ) : (
                      <Sparkles size={14} />
                    )}
                    {generatingWords ? "Generating\u2026" : "Generate Words"}
                  </button>
                  {wordGenError && (
                    <span className="text-xs text-red-500 flex items-center gap-1">
                      {wordGenError}
                      <button className="hover:text-red-400 cursor-pointer" onClick={() => setWordGenError("")}>
                        <X size={12} />
                      </button>
                    </span>
                  )}
                </div>

                {/* Response preview (inside settings) */}
                {showSettings && wordGenResponse && (
                  <div className="flex flex-col gap-1 p-3 rounded-lg bg-bg-muted border border-border-default text-xs">
                    <label className="text-text-secondary font-medium">Response</label>
                    <pre className="w-full max-h-48 overflow-auto px-2 py-1 rounded bg-bg-input border border-border-default text-2xl font-bold text-text-primary whitespace-pre-wrap break-all">
                      {wordGenResponse}
                    </pre>
                  </div>
                )}
              </>
            )}
          </WordGenSettings>

          {/* Words - edit mode only */}
          {drawer.mode === "edit" && (
            <div className="flex flex-col gap-2 pt-2 border-t border-border-default">
              <div className="flex flex-row flex-wrap gap-1.5">
                {words.map((w) => (
                  <span
                    key={w.uuid}
                    title={w.note || undefined}
                    className={`flex flex-col items-center justify-center gap-1 px-2 py-1 m-2 border rounded-md ${
                      pendingIds.includes(w.uuid)
                        ? "border-amber-500/50 border-dashed"
                        : "border-border-default"
                    }`}
                  >
                    <span className="font-semibold text-text-primary">{w.word}</span>
                    <div className="flex flex-row items-center justify-between w-full [&>.word-type]:![font-size:0.5em]">
                      {w.word_type && (
                        <span className="word-type text-accent bg-accent-bg/15 px-1.5 py-0.5 rounded-sm">{w.word_type}</span>
                      )}
                      <button
                        className="text-text-tertiary hover:text-red-500 cursor-pointer p-0.5 rounded-full bg-red-500/10"
                        onClick={() => removeWord(w.uuid)}
                        title="Remove word"
                      >
                        <X size={18} />
                      </button>
                    </div>
                  </span>
                ))}
                {words.length === 0 && (
                  <span className="text-xs text-text-tertiary">No vocabulary yet.</span>
                )}
              </div>
              <div className="flex flex-row gap-2">
                <input
                  className="flex-1 px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
                  placeholder="Add word…"
                  value={newWord}
                  onChange={(e) => setNewWord(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") addWord();
                  }}
                />
                <select
                  className="w-28 px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent cursor-pointer"
                  value={newWordType}
                  onChange={(e) => setNewWordType(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") addWord();
                  }}
                >
                  <option value="">type…</option>
                  {WORD_TYPE_OPTIONS.map((t) => (
                    <option key={t} value={t}>{t}</option>
                  ))}
                </select>
                <button
                  className="px-3 py-1 rounded bg-bg-muted border border-border-default text-sm hover:bg-bg-hover cursor-pointer"
                  onClick={addWord}
                  disabled={!newWord.trim()}
                >
                  Add
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Rules editor modal */}
      {showRulesEditor && (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40"
          onClick={() => setShowRulesEditor(false)}
        >
          <div
            className="bg-bg-card rounded-2xl shadow-2xl p-5 flex flex-col gap-3 w-96 max-w-[90vw] border border-border-default"
            onClick={(e) => e.stopPropagation()}
          >
            <p className="font-semibold text-base text-text-primary">Auto-replace Rules</p>
            <textarea
              className="w-full h-48 text-sm font-mono border border-border-default rounded p-2 resize-none focus:outline-none focus:border-accent overflow-x-auto whitespace-pre bg-bg-input text-text-primary"
              value={rulesText}
              onChange={(e) => setRulesText(e.target.value)}
              spellCheck={false}
            />
            <div className="flex flex-row gap-2 justify-end">
              <button
                className="px-3 py-1.5 rounded-md text-sm hover:bg-bg-hover text-text-secondary cursor-pointer"
                onClick={() => setShowRulesEditor(false)}
              >
                Cancel
              </button>
              <button
                className="px-4 py-1.5 rounded-md bg-accent-bg text-white text-sm font-medium hover:opacity-90 cursor-pointer"
                onClick={() => {
                  localStorage.setItem(LS_KEY, rulesText);
                  setShowRulesEditor(false);
                }}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
