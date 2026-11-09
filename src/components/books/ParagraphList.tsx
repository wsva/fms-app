"use client";

import { useEffect, useRef, useState } from "react";
import { Play, Square, Copy, Upload, Trash2, Plus, FileDown, Brain } from "lucide-react";
import type { Paragraph, SentenceClient } from "@/lib/read/types";
import { useMediaPlay } from "@/hooks/useMediaPlay";
import { sentenceBgClass } from "./utils";

type Props = {
  paragraphs: Paragraph[];
  viewMode: "line" | "inline";
  saving: boolean;
  mobile?: boolean;
  /** Understanding split mode is on: show the focus affordance + ring. */
  understanding?: boolean;
  /** The focused paragraph's break-row uuid, if any. */
  focusParagraph?: string | null;
  /** The focused sentence's uuid, if any — it gets the ring and, on desktop,
   *  becomes the inline-edit target. */
  focusSentence?: string | null;
  /** The sentence currently rendered as contentEditable (desktop inline edit). */
  editingSentence?: string | null;
  /** Commit an inline edit (Enter / blur-with-changes) to the book DB. */
  onCommitEdit?: (uuid: string, content: string) => void;
  onFocusParagraph?: (uuid: string | null) => void;
  /** One click road for both platforms: the hook opens the drawer on a phone
   *  and focuses (desktop) otherwise. */
  onSelectSentence: (s: SentenceClient) => void;
  /** Mobile keeps the drawer's add-sentence road; desktop uses the Edit tab. */
  onAddSentence?: (para: Paragraph) => void;
  onDeleteParagraph: (para: Paragraph) => void;
  onParagraphAudio: (para: Paragraph) => void;
  onImport: (para: Paragraph, text: string) => void;
};

/** The paragraph's audio control, which is the same clip's **Stop** button while
 *  that clip sounds. Playing through `useMediaPlay` means one clip at a time
 *  app-wide, and the button whose clip got interrupted reverts on its own. */
function ParagraphAudioButton({
  url,
  playing,
  onToggle,
  pad,
}: {
  url?: string | null;
  playing: unknown;
  onToggle: (url: string | null | undefined, token: unknown) => void;
  pad: string;
}) {
  const isPlaying = !!url && playing === url;
  return (
    <button
      title={isPlaying ? "Stop paragraph audio" : "Play paragraph audio"}
      className={`${pad} rounded hover:bg-bg-hover ${
        isPlaying ? "text-accent" : "text-text-secondary"
      } cursor-pointer`}
      // The clip is its own token: a second press stops it rather than starting
      // an overlapping copy of it.
      onClick={() => onToggle(url, url)}
    >
      {isPlaying ? <Square size={16} /> : <Play size={16} />}
    </button>
  );
}

/**
 * One sentence in the reading pane. Plain text to click everywhere; when it is
 * the inline-edit target (desktop) the same element becomes contentEditable:
 * Enter (or blur with changes) commits, Escape reverts. This replaced the
 * drawer as the PC edit surface.
 */
function SentenceAnchor({
  s,
  block,
  editable,
  focused,
  understanding,
  onSelect,
  onCommit,
}: {
  s: SentenceClient;
  /** Line view lays sentences out one per line; inline flows them. */
  block: boolean;
  editable: boolean;
  focused: boolean;
  understanding: boolean;
  onSelect: () => void;
  onCommit: (text: string) => void;
}) {
  const ref = useRef<HTMLElement | null>(null);

  const classes = [
    block ? "block w-full text-left py-0.5 px-2" : "inline px-0.5",
    "cursor-pointer rounded transition-colors text-text-primary",
    s.modified
      ? "bg-bg-hover"
      : sentenceBgClass(s.bg_color) || "hover:bg-bg-hover",
    block
      ? s.audio_path || s.hasLocalAudio
        ? "border-l-2 border-accent"
        : ""
      : s.audio_path || s.hasLocalAudio
        ? "underline decoration-accent decoration-2"
        : "",
    understanding && focused ? "ring-2 ring-accent/50" : "",
  ].join(" ");

  // Entering edit mode (a click on another sentence) moves the caret here.
  useEffect(() => {
    if (!editable) return;
    const id = requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.focus();
      const range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
    });
    return () => cancelAnimationFrame(id);
  }, [editable]);

  const commitText = () => {
    const el = ref.current;
    if (!el) return;
    const text = (el.textContent ?? "").trim();
    if (text !== s.content.trim()) onCommit(text);
  };

  if (!editable) {
    return (
      <a
        ref={(el) => {
          ref.current = el;
        }}
        data-sentence-uuid={s.uuid}
        onClick={onSelect}
        className={classes}
      >
        {s.content}
        {block ? "" : " "}
      </a>
    );
  }

  return (
    <span
      ref={(el) => {
        ref.current = el;
      }}
      data-sentence-uuid={s.uuid}
      role="textbox"
      className={classes}
      contentEditable
      suppressContentEditableWarning
      // The click that lands here also selects the sentence through the
      // parent's delegation; suppress the extra fire so focus churn stays
      // to one road.
      onClick={(e) => {
        e.stopPropagation();
        onSelect();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          // Enter commits and moves on; Shift+Enter keeps a manual line break
          // inside the sentence (rare, but free to keep).
          if (e.shiftKey) return;
          e.preventDefault();
          commitText();
          ref.current?.blur();
        } else if (e.key === "Escape") {
          e.preventDefault();
          if (ref.current) ref.current.textContent = s.content;
          ref.current?.blur();
        }
      }}
      onBlur={commitText}
    >
      {s.content}
    </span>
  );
}

export default function ParagraphList({
  paragraphs,
  viewMode,
  saving,
  mobile = false,
  understanding = false,
  focusParagraph = null,
  focusSentence = null,
  editingSentence = null,
  onCommitEdit,
  onFocusParagraph,
  onSelectSentence,
  onAddSentence,
  onDeleteParagraph,
  onParagraphAudio,
  onImport,
}: Props) {
  // One clip plays at a time across the app, and the paragraph whose clip is
  // sounding gets the Stop button (see useMediaPlay).
  const { playing, toggle } = useMediaPlay();
  const [importIdx, setImportIdx] = useState<number | null>(null);
  const [importText, setImportText] = useState("");
  // Comfortable tap targets on narrow touch screens; compact on desktop.
  const iconPad = mobile ? "p-2" : "p-1";
  const textBtnPad = mobile ? "px-2 py-2" : "px-2 py-1";
  return (
    <div className="flex flex-col gap-4">
      {paragraphs.map((para, pi) => (
        <div
          key={pi}
          data-para-uuid={para.breakSentence?.uuid}
          onClick={(e) => {
            // Blank space (anything that is not a sentence or a control)
            // selects this paragraph, so the reader never has to hunt for
            // the Brain button. Sentence picks stop their own click here.
            if ((e.target as HTMLElement).closest("a,button,[contenteditable]")) return;
            if (para.breakSentence) onFocusParagraph?.(para.breakSentence.uuid);
          }}
          className={`flex flex-col gap-2 p-3 rounded-lg border border-border-default ${
            pi % 2 === 0 ? "bg-bg-card" : "bg-bg-muted"
          } ${
            understanding && focusParagraph && para.breakSentence?.uuid === focusParagraph
              ? "ring-2 ring-accent/50"
              : ""
          }`}
        >
          {/* Paragraph badge + actions */}
          <div className="flex flex-row items-center justify-between">
            <div className="text-xs font-semibold text-text-tertiary select-none">
              ¶ {pi + 1}
            </div>
            <div className="flex flex-row items-center gap-1">
              {understanding && para.breakSentence && (
                <button
                  title="Understand this paragraph"
                  className={`${iconPad} rounded hover:bg-bg-hover cursor-pointer ${
                    focusParagraph === para.breakSentence.uuid
                      ? "text-accent"
                      : "text-text-secondary"
                  }`}
                  onClick={() => onFocusParagraph?.(para.breakSentence!.uuid)}
                >
                  <Brain size={16} />
                </button>
              )}
              {para.sentences.length > 0 && (
                <button
                  title="Copy paragraph text"
                  className={`${iconPad} rounded hover:bg-bg-hover text-text-secondary cursor-pointer`}
                  onClick={() =>
                    navigator.clipboard.writeText(
                      para.sentences.map((s) => s.content.trim()).join(" ")
                    )
                  }
                >
                  <Copy size={16} />
                </button>
              )}
              {para.breakSentence?.audio_path && (
                <ParagraphAudioButton
                  url={para.breakSentence?.audio_url}
                  playing={playing}
                  onToggle={toggle}
                  pad={iconPad}
                />
              )}
              <button
                title={para.breakSentence?.audio_path ? "Replace audio" : "Upload audio"}
                disabled={saving}
                className={`${iconPad} rounded hover:bg-bg-hover disabled:opacity-40 cursor-pointer`}
                style={{ color: para.breakSentence?.audio_path ? "#e5484d" : undefined }}
                onClick={() => onParagraphAudio(para)}
              >
                <Upload size={16} />
              </button>
              {(para.sentences.length > 0 || para.breakSentence) && (
                <button
                  title="Delete paragraph"
                  disabled={saving}
                  className={`${iconPad} rounded hover:bg-bg-hover text-red-500 disabled:opacity-40 cursor-pointer`}
                  onClick={() => onDeleteParagraph(para)}
                >
                  <Trash2 size={16} />
                </button>
              )}
            </div>
          </div>

          {/* Sentences */}
          {para.sentences.length === 0 ? (
            <p className="text-sm text-text-tertiary italic">empty paragraph</p>
          ) : (
            <div className={viewMode === "line" ? "flex flex-col gap-1" : "text-xl leading-relaxed"}>
              {para.sentences.map((s) => (
                <SentenceAnchor
                  key={s.uuid}
                  s={s}
                  block={viewMode === "line"}
                  editable={!mobile && editingSentence === s.uuid}
                  focused={focusSentence === s.uuid}
                  understanding={understanding}
                  onSelect={() => onSelectSentence(s)}
                  onCommit={(text) => onCommitEdit?.(s.uuid, text)}
                />
              ))}
            </div>
          )}

          {/* Import textarea */}
          {importIdx === pi && (
            <div className="flex flex-col gap-2 p-2 rounded-md border border-border-default bg-bg-muted">
              <textarea
                autoFocus
                className="w-full px-3 py-2 rounded-md bg-bg-input border border-border-default text-sm text-text-primary font-mono focus:outline-none focus:border-accent min-h-[120px]"
                placeholder={"Line 1 → sentence\nLine 2 → sentence\n\nEmpty line → paragraph break"}
                value={importText}
                onChange={(e) => setImportText(e.target.value)}
              />
              <div className="flex gap-2 justify-end">
                <button
                  className="flex items-center gap-1 px-2 py-1 rounded text-sm font-medium bg-accent-bg text-white hover:opacity-90 cursor-pointer disabled:opacity-50"
                  disabled={saving || !importText.trim()}
                  onClick={() => {
                    onImport(para, importText);
                    setImportIdx(null);
                    setImportText("");
                  }}
                >
                  <FileDown size={14} />
                  Import
                </button>
                <button
                  className="px-2 py-1 rounded text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
                  onClick={() => { setImportIdx(null); setImportText(""); }}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}

          {/* Import stays; Add Sentence retired to mobile — on desktop the
              panel's Edit tab reflows the paragraph as a whole. */}
          <div className="flex justify-end gap-1">
            <button
              className={`flex items-center gap-1 ${textBtnPad} rounded text-sm text-text-secondary hover:bg-bg-hover cursor-pointer`}
              onClick={() => {
                if (importIdx === pi) { setImportIdx(null); setImportText(""); }
                else { setImportIdx(pi); setImportText(""); }
              }}
            >
              <FileDown size={14} />
              {importIdx === pi ? "Cancel" : "Import"}
            </button>
            {mobile && (
              <button
                className={`flex items-center gap-1 ${textBtnPad} rounded text-sm text-text-secondary hover:bg-bg-hover cursor-pointer`}
                onClick={() => onAddSentence?.(para)}
              >
                <Plus size={14} />
                Add Sentence
              </button>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}
