"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Play,
  Mic,
  MicOff,
  X,
  MoreVertical,
  Trash2,
  Copy,
  RotateCcw,
  Sparkles,
  Loader2,
  Settings2,
} from "lucide-react";
import type { DrawerState, BookSentenceWord } from "@/lib/read/types";
import type { LlmChatResponse, OllamaModelInfo } from "@/lib/llm/types";
import { isTauri } from "@/lib/tauri";
import { BG_COLORS } from "./utils";
import { getUUID } from "./utils";
import { highlightDifferences } from "./diff";

const LS_KEY = "read_auto_replace_rules";
const LS_WORD_GEN_MODEL = "read_word_gen_model";
const LS_WORD_GEN_PROMPT = "read_word_gen_prompt";
const LS_WORD_GEN_TEMP = "read_word_gen_temp";
const LS_DRAWER_FONT_SCALE = "read_drawer_font_scale";

const DRAWER_FONT_SCALES = [
  { label: "Small", size: "0.875rem", weight: "normal" },
  { label: "Normal", size: "1rem", weight: "normal" },
  { label: "Large", size: "1.125rem", weight: "bold" },
  { label: "Large x2", size: "1.5rem", weight: "bold" },
  { label: "Large x3", size: "1.875rem", weight: "bold" },
];

const DEFAULT_WORD_GEN_PROMPT = `You are a German language NLP assistant. Analyze the given German sentence and extract all meaningful words. For each word provide: the lemma (base/dictionary form), the part of speech, and whether it carries useful semantic meaning.
Rules:
- Restore separable verb parts to their base infinitive (e.g. 'steht auf' → 'aufstehen').
- Restore perfect/pluperfect forms to the base infinitive (e.g. 'ist gegangen' → 'gehen').
- Restore adjective declensions to the masculine nominative base form (e.g. 'guten' → 'gut').
- For nouns (Nomen), always include the definite article (der/die/das) in the lemma, e.g. 'Hunde' → 'der Hund', 'Katze' → 'die Katze', 'Haus' → 'das Haus'.
- Set keep=true for nouns, verbs, adjectives, adverbs with real meaning.
- Set keep=false for articles, prepositions, conjunctions, pronouns, auxiliary verbs, and other function words.
- Set keep=false for proper names of people (especially historical figures), places, and organizations — these are not vocabulary words to learn.
Respond ONLY with a JSON array, no other text. Each element: {"surface":"...","lemma":"...","pos":"...","keep":true/false}`;
const DEFAULT_WORD_GEN_MODEL = "gemma4:e4b";

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
  const [colorPickerOpen, setColorPickerOpen] = useState(false);
  const [rulesText, setRulesText] = useState(DEFAULT_RULES_TEXT);
  const [showRulesEditor, setShowRulesEditor] = useState(false);
  const [words, setWords] = useState<BookSentenceWord[]>([]);
  const [newWord, setNewWord] = useState("");
  const [newWordType, setNewWordType] = useState("");
  const [generatingWords, setGeneratingWords] = useState(false);
  const [wordGenError, setWordGenError] = useState("");
  const [wordGenModel, setWordGenModel] = useState(DEFAULT_WORD_GEN_MODEL);
  const [wordGenPrompt, setWordGenPrompt] = useState(DEFAULT_WORD_GEN_PROMPT);
  const [wordGenTemp, setWordGenTemp] = useState(0);
  const [showWordGenSettings, setShowWordGenSettings] = useState(false);
  const [wordGenResponse, setWordGenResponse] = useState("");
  const [drawerFontScale, setDrawerFontScale] = useState(1);
  const [availableModels, setAvailableModels] = useState<OllamaModelInfo[]>([]);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const editUUID = drawer?.mode === "edit" ? drawer.sentence.uuid : null;

  useEffect(() => {
    setRulesText(loadRulesText());
    const m = localStorage.getItem(LS_WORD_GEN_MODEL);
    if (m) setWordGenModel(m);
    const p = localStorage.getItem(LS_WORD_GEN_PROMPT);
    if (p) setWordGenPrompt(p);
    const t = localStorage.getItem(LS_WORD_GEN_TEMP);
    if (t !== null) setWordGenTemp(parseFloat(t) || 0);
    const fs = localStorage.getItem(LS_DRAWER_FONT_SCALE);
    if (fs !== null) setDrawerFontScale(parseInt(fs) || 1);
    // Fetch installed Ollama models
    invoke<{ installed: OllamaModelInfo[] }>("llm_list_models")
      .then((res) => setAvailableModels(res.installed))
      .catch(() => {/* Ollama not running */ });
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

  const addWord = async () => {
    if (!editUUID || !newWord.trim() || !isTauri()) return;
    const w: BookSentenceWord = {
      uuid: getUUID(),
      sentence_uuid: editUUID,
      word: newWord.trim(),
      word_type: newWordType.trim(),
      note: "",
    };
    try {
      await invoke("book_save_word", { bookUuid: bookUUID, word: w });
      setWords((prev) => [...prev, w]);
      setNewWord("");
      setNewWordType("");
    } catch {
      /* ignore */
    }
  };

  const removeWord = async (uuid: string) => {
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
    try {
      const res = await invoke<LlmChatResponse>("llm_chat", {
        model: wordGenModel,
        messages: [
          {
            role: "system",
            content: wordGenPrompt,
          },
          {
            role: "user",
            content: content.trim(),
          },
        ],
        temperature: wordGenTemp,
      });

      // Extract JSON array from the response (LLM may wrap it in markdown code blocks)
      const raw = res.content;
      setWordGenResponse(raw);
      const jsonMatch = raw.match(/\[[\s\S]*\]/);
      if (!jsonMatch) {
        setWordGenError("Could not parse LLM response");
        return;
      }

      const parsed: { surface: string; lemma: string; pos: string; keep: boolean }[] =
        JSON.parse(jsonMatch[0]);

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
        await invoke("book_save_word", { bookUuid: bookUUID, word: w });
        setWords((prev) => [...prev, w]);
        added++;
      }

      if (added === 0) {
        setWordGenError("No new words to add");
      }
    } catch (e) {
      setWordGenError(`Failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setGeneratingWords(false);
    }
  };

  const menuItem =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2";

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
        <div className="flex flex-row items-center justify-between px-4 pt-4 pb-2 border-b border-border-default">
          <span className="font-semibold text-base text-text-primary">
            {drawer.mode === "edit" ? "Edit Sentence" : "New Sentence"}
          </span>
          <div className="flex flex-row items-center gap-2">
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
            <button
              className="px-4 py-1.5 rounded-md bg-accent-bg text-white text-sm font-medium hover:opacity-90 disabled:opacity-50 cursor-pointer"
              disabled={saving}
              onClick={drawer.mode === "add" ? onSaveAdd : onSaveEdit}
            >
              {drawer.mode === "add" ? "Add" : "Save"}
            </button>
            <button
              className="px-3 py-1.5 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
              disabled={saving}
              onClick={onDiscard}
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

            <button
              className="p-1.5 rounded hover:bg-bg-hover text-text-secondary cursor-pointer"
              onClick={onClose}
              title="Close"
            >
              <X size={18} />
            </button>
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
              {recording ? <MicOff size={16} /> : <Mic size={16} />}
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
          <div className="flex flex-row items-center justify-end gap-2">
            <button
              className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium text-text-tertiary hover:bg-bg-hover cursor-pointer transition-colors"
              onClick={() => setShowWordGenSettings((v) => !v)}
            >
              <Settings2 size={14} /> Settings
            </button>
            <button
              className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium bg-accent-bg/15 text-accent hover:bg-accent-bg/25 disabled:opacity-50 cursor-pointer transition-colors"
              disabled={generatingWords || !content.trim() || drawer.mode !== "edit"}
              onClick={generateWords}
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

          {/* Collapsible settings */}
          {showWordGenSettings && (
            <div className="flex flex-col gap-2 p-2 rounded-lg bg-bg-muted border border-border-default text-xs">
              <div className="flex flex-row gap-2">
                <div className="flex flex-col gap-1 max-w-48">
                  <label className="text-text-secondary font-medium">Model</label>
                  <select
                    className="px-2 py-1 rounded bg-bg-input border border-border-default text-xs text-text-primary focus:outline-none focus:border-accent cursor-pointer"
                    value={wordGenModel}
                    onChange={(e) => {
                      setWordGenModel(e.target.value);
                      localStorage.setItem(LS_WORD_GEN_MODEL, e.target.value);
                    }}
                  >
                    {availableModels.length === 0 && (
                      <option value="">No models found</option>
                    )}
                    {availableModels.map((m) => (
                      <option key={m.name} value={m.name}>{m.name}</option>
                    ))}
                  </select>
                </div>
                <div className="flex flex-col gap-1 w-20">
                  <label className="text-text-secondary font-medium">Temp</label>
                  <select
                    value={wordGenTemp}
                    onChange={(e) => {
                      const v = parseFloat(e.target.value);
                      setWordGenTemp(v);
                      localStorage.setItem(LS_WORD_GEN_TEMP, String(v));
                    }}
                    className="px-2 py-1 rounded-md bg-bg-muted border border-border-default text-text-primary text-xs cursor-pointer"
                  >
                    {[0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0].map((v) => (
                      <option key={v} value={v}>{v.toFixed(1)}</option>
                    ))}
                  </select>
                </div>
              </div>
              <div className="flex flex-col gap-1">
                <div className="flex items-center justify-between">
                  <label className="text-text-secondary font-medium">System Prompt</label>
                  <button
                    className="text-text-tertiary hover:text-accent cursor-pointer"
                    onClick={() => {
                      setWordGenPrompt(DEFAULT_WORD_GEN_PROMPT);
                      localStorage.setItem(LS_WORD_GEN_PROMPT, DEFAULT_WORD_GEN_PROMPT);
                    }}
                  >
                    Reset
                  </button>
                </div>
                <textarea
                  className="w-full h-32 px-2 py-1 rounded bg-bg-input border border-border-default text-2xl font-bold text-text-primary resize-y focus:outline-none focus:border-accent"
                  value={wordGenPrompt}
                  onChange={(e) => {
                    setWordGenPrompt(e.target.value);
                    localStorage.setItem(LS_WORD_GEN_PROMPT, e.target.value);
                  }}
                />
              </div>
              {wordGenResponse && (
                <div className="flex flex-col gap-1">
                  <label className="text-text-secondary font-medium">Response</label>
                  <pre className="w-full max-h-48 overflow-auto px-2 py-1 rounded bg-bg-input border border-border-default text-2xl font-bold text-text-primary whitespace-pre-wrap break-all">
                    {wordGenResponse}
                  </pre>
                </div>
              )}
            </div>
          )}

          {/* Words - edit mode only */}
          {drawer.mode === "edit" && (
            <div className="flex flex-col gap-2 pt-2 border-t border-border-default">
              <div className="flex flex-row flex-wrap gap-1.5">
                {words.map((w) => (
                  <span
                    key={w.uuid}
                    title={w.note || undefined}
                    className="flex flex-col items-center justify-center gap-1 px-2 py-1 m-2 border border-border-default rounded-md"
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
