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
  Maximize2,
  Minimize2,
} from "lucide-react";
import type { DrawerState, BookSentenceWord } from "@/lib/read/types";
import { isTauri } from "@/lib/tauri";
import { BG_COLORS } from "./utils";
import { getUUID } from "./utils";
import { highlightDifferences } from "./diff";

const LS_KEY = "read_auto_replace_rules";
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
  { char: "„", hint: "opening quotation marks" },
  { char: "“", hint: "closing quotation marks" },
  { char: "‚", hint: "single open quote" },
  { char: "‘", hint: "single close quote" },
  { char: "»", hint: "chevrons »…«" },
  { char: "«", hint: "chevrons «…»" },
  { char: "›", hint: "single chevrons" },
  { char: "‹", hint: "single chevrons" },
  { char: "é", hint: "e acute" },
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
  onPlay: () => void;
  onToggleRecording: () => void;
  onSaveAdd: () => void;
  onSaveEdit: () => void;
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
  onPlay,
  onToggleRecording,
  onSaveAdd,
  onSaveEdit,
  onDelete,
  onInsertBefore,
  onInsertAfter,
  onParagraphBefore,
  onParagraphAfter,
}: Props) {
  const [expanded, setExpanded] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [rulesText, setRulesText] = useState(DEFAULT_RULES_TEXT);
  const [showRulesEditor, setShowRulesEditor] = useState(false);
  const [words, setWords] = useState<BookSentenceWord[]>([]);
  const [newWord, setNewWord] = useState("");
  const [newWordType, setNewWordType] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const editUUID = drawer?.mode === "edit" ? drawer.sentence.uuid : null;

  useEffect(() => {
    setRulesText(loadRulesText());
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

  const menuItem =
    "w-full text-left px-3 py-2 text-sm hover:bg-bg-hover cursor-pointer flex items-center gap-2";

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col justify-end bg-black/40"
      onClick={onClose}
    >
      <div
        className={`bg-bg-elevated rounded-t-2xl shadow-2xl w-full max-h-[75vh] overflow-y-auto border-t border-border-default transition-[min-height] duration-300 ${
          expanded ? "min-h-[50vh]" : ""
        }`}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex flex-row items-center justify-between px-4 pt-4 pb-2 border-b border-border-default">
          <span className="font-semibold text-base text-text-primary">
            {drawer.mode === "edit" ? "Edit Sentence" : "New Sentence"}
          </span>
          <div className="flex flex-row items-center gap-2">
            <button
              className="px-4 py-1.5 rounded-md bg-accent-bg text-white text-sm font-medium hover:opacity-90 disabled:opacity-50 cursor-pointer"
              disabled={saving}
              onClick={drawer.mode === "add" ? onSaveAdd : onSaveEdit}
            >
              {drawer.mode === "add" ? "Add" : "Save"}
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
              onClick={() => setExpanded((e) => !e)}
              title={expanded ? "Collapse" : "Expand"}
            >
              {expanded ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
            </button>
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
        <div className="flex flex-col gap-3 p-4">
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
              {recording ? "Stop" : processing ? "Processing…" : "Record"}
            </button>
          </div>

          {/* STT diff */}
          {hasAudio && recognized && (
            <div
              className={`bg-bg-muted rounded p-2 text-text-primary ${
                expanded ? "text-xl" : "text-sm"
              }`}
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
            className={`w-full px-3 py-2 rounded-lg bg-bg-input border border-border-default text-text-primary resize-y focus:outline-none focus:border-accent ${
              expanded ? "text-2xl font-bold" : "text-base"
            }`}
            rows={3}
            value={content}
            onChange={(e) => onContentChange(e.target.value)}
          />

          {/* Background color picker */}
          <div className="flex flex-row items-center gap-2">
            <span className="text-xs text-text-secondary shrink-0">Color:</span>
            <button
              title="None"
              onClick={() => onBgColorChange(null)}
              className={`w-6 h-6 rounded-full border-2 bg-bg-input cursor-pointer ${
                bgColor === null ? "border-accent" : "border-border-default"
              }`}
            />
            {BG_COLORS.map((c) => (
              <button
                key={c.key}
                title={c.label}
                onClick={() => onBgColorChange(bgColor === c.key ? null : c.key)}
                className={`w-6 h-6 rounded-full border-2 cursor-pointer ${c.swatch} ${
                  bgColor === c.key ? "border-accent" : "border-transparent"
                }`}
              />
            ))}
          </div>

          {/* Words */}
          {drawer.mode === "edit" && (
            <div className="flex flex-col gap-2 pt-2 border-t border-border-default">
              <div className="flex flex-row flex-wrap gap-1.5">
                {words.map((w) => (
                  <span
                    key={w.uuid}
                    title={w.note || undefined}
                    className="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-bg-muted border border-border-default text-xs"
                  >
                    <span className="font-medium text-text-primary">{w.word}</span>
                    {w.word_type && (
                      <span className="text-text-tertiary">{w.word_type}</span>
                    )}
                    <button
                      className="text-text-tertiary hover:text-red-500 cursor-pointer"
                      onClick={() => removeWord(w.uuid)}
                      title="Remove word"
                    >
                      <X size={12} />
                    </button>
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
                <input
                  className="w-28 px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
                  placeholder="type"
                  value={newWordType}
                  onChange={(e) => setNewWordType(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") addWord();
                  }}
                />
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
