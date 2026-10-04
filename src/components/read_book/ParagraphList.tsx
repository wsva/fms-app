"use client";

import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Play, Copy, Upload, Trash2, Plus, FileDown } from "lucide-react";
import type { Paragraph, SentenceClient } from "@/lib/read/types";
import { sentenceBgClass } from "./utils";

type Props = {
  paragraphs: Paragraph[];
  viewMode: "line" | "inline";
  saving: boolean;
  mobile?: boolean;
  onEditSentence: (s: SentenceClient) => void;
  onAddSentence: (para: Paragraph) => void;
  onDeleteParagraph: (para: Paragraph) => void;
  onParagraphAudio: (para: Paragraph) => void;
  onImport: (para: Paragraph, text: string) => void;
};

function playUrl(url?: string | null) {
  if (!url) return;
  new Audio(convertFileSrc(url)).play();
}

export default function ParagraphList({
  paragraphs,
  viewMode,
  saving,
  mobile = false,
  onEditSentence,
  onAddSentence,
  onDeleteParagraph,
  onParagraphAudio,
  onImport,
}: Props) {
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
          className={`flex flex-col gap-2 p-3 rounded-lg border border-border-default ${
            pi % 2 === 0 ? "bg-bg-card" : "bg-bg-muted"
          }`}
        >
          {/* Paragraph badge + actions */}
          <div className="flex flex-row items-center justify-between">
            <div className="text-xs font-semibold text-text-tertiary select-none">
              ¶ {pi + 1}
            </div>
            <div className="flex flex-row items-center gap-1">
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
                <button
                  title="Play paragraph audio"
                  className={`${iconPad} rounded hover:bg-bg-hover text-text-secondary cursor-pointer`}
                  onClick={() => playUrl(para.breakSentence?.audio_url)}
                >
                  <Play size={16} />
                </button>
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
          ) : viewMode === "inline" ? (
            <p className="text-xl leading-relaxed text-text-primary">
              {para.sentences.map((s) => (
                <a
                  key={s.uuid}
                  onClick={() => onEditSentence(s)}
                  className={[
                    "inline cursor-pointer rounded px-0.5 transition-colors",
                    s.modified
                      ? "bg-bg-hover"
                      : sentenceBgClass(s.bg_color) || "hover:bg-bg-hover",
                    s.audio_path || s.hasLocalAudio
                      ? "underline decoration-accent decoration-2"
                      : "",
                  ].join(" ")}
                >
                  {s.content}{" "}
                </a>
              ))}
            </p>
          ) : (
            <div className="flex flex-col gap-1">
              {para.sentences.map((s) => (
                <a
                  key={s.uuid}
                  onClick={() => onEditSentence(s)}
                  className={[
                    "text-left text-xl cursor-pointer rounded px-2 py-0.5 transition-colors text-text-primary",
                    s.modified
                      ? "bg-bg-hover"
                      : sentenceBgClass(s.bg_color) || "hover:bg-bg-hover",
                    s.audio_path || s.hasLocalAudio ? "border-l-2 border-accent" : "",
                  ].join(" ")}
                >
                  {s.content}
                </a>
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

          {/* Add Sentence + Import buttons */}
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
            <button
              className={`flex items-center gap-1 ${textBtnPad} rounded text-sm text-text-secondary hover:bg-bg-hover cursor-pointer`}
              onClick={() => onAddSentence(para)}
            >
              <Plus size={14} />
              Add Sentence
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
