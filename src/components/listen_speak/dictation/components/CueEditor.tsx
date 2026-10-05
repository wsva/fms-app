"use client";

import React, { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button, Input, InputGroup, TextArea, Tooltip } from "@heroui/react";
import { formatVttTime, parseVttTime, validateVttTime } from "@/lib/listen/subtitle";
import { hideWord, playMediaPart, pureContent, splitContent } from "@/lib/listen/utils";
import {
  ArrowLeftToLine,
  ArrowRightToLine,
  Copy,
  Eraser,
  Eye,
  EyeOff,
  MapPin,
  Mic,
  Pencil,
  Play,
  Star,
  Trash2,
  X,
} from "lucide-react";
import { lcs } from "@/lib/listen/lcs";
import type { Cue } from "@/lib/types";
import { handleToggle, subscribe, getVoiceState, type VoiceState } from "@/lib/voice-input";

// ── Mic Button ────────────────────────────────────────────────────────────────

function MicButton() {
  const voiceState = useSyncExternalStore(subscribe, getVoiceState, getVoiceState);

  return (
    <>
      <Tooltip>
        <Tooltip.Trigger>
          <Button
            isIconOnly
            variant={voiceState === "recording" ? "danger" : "ghost"}
            size="sm"
            isDisabled={voiceState === "processing"}
            onPress={handleToggle}
            className={voiceState === "recording" ? "animate-pulse" : ""}
          >
            {voiceState === "recording" ? <X size={14} /> : <Mic size={14} />}
          </Button>
        </Tooltip.Trigger>
        <Tooltip.Content>Ctrl+C (no selection)</Tooltip.Content>
      </Tooltip>
    </>
  );
}

// ── Copy Button ──────────────────────────────────────────────────────────────

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Tooltip>
      <Tooltip.Trigger>
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          aria-label="copy"
          className="shrink-0"
          onPress={async () => {
            try {
              await navigator.clipboard.writeText(text);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            } catch {
              /* clipboard unavailable */
            }
          }}
        >
          {copied ? <X size={14} /> : <Copy size={14} />}
        </Button>
      </Tooltip.Trigger>
      <Tooltip.Content>{copied ? "copied" : "copy to clipboard"}</Tooltip.Content>
    </Tooltip>
  );
}

// ── Tip Toggle Component ─────────────────────────────────────────────────────

type TipToggleProps = {
  tip: string;
  content: string;
  showContent: boolean;
  onToggle: () => void;
  size?: "small" | "medium" | "large";
  className?: string;
};

function TipToggle({ tip, content, showContent, onToggle, size = "small", className = "" }: TipToggleProps) {
  const sizeClasses = size === "large" ? "text-2xl" : size === "medium" ? "text-xl" : "font-normal";

  return (
    <div
      data-no-focus
      className={`bg-bg-muted rounded-sm px-1 text-text-tertiary ${sizeClasses} w-full cursor-pointer select-text ${className}`}
      onDoubleClick={(e) => {
        // Suppress the default double-click word-selection so it doesn't fight
        // with the toggle; users can still click-drag to select the text.
        e.preventDefault();
        onToggle();
      }}
      title="Double-click to toggle content/tip"
    >
      {showContent ? content : tip}
    </div>
  );
}

// ── Dictation ────────────────────────────────────────────────────────────────

// Shared field styling so the dictation answer input and the edit-mode content
// input look identical; only the type scale differs per render size.
type FieldSize = "compact" | "medium" | "large";

function fieldClass(size: FieldSize) {
  return size === "large"
    ? "text-4xl font-bold border-b-2 border-b-border-light bg-bg-muted rounded-lg p-2 my-1 w-full shadow-none focus:ring-0 focus:border-b-accent"
    : size === "medium"
      ? "text-2xl font-bold border-b-2 border-b-border-light bg-bg-muted rounded-lg p-2 my-1 w-full shadow-none focus:ring-0 focus:border-b-accent"
      : "text-xl font-bold border-b-2 border-b-border-light bg-bg-muted rounded-none p-0 my-1 w-full shadow-none focus:ring-0 focus:border-b-accent";
}

type DictationProps = {
  cue: Cue;
  media: HTMLMediaElement | null;
  stateSuccess: boolean;
  setStateSuccess: React.Dispatch<React.SetStateAction<boolean>>;
  onSuccess?: (uuid: string, success: boolean) => void;
  onFocusInput?: () => void;
  mode: FieldSize;
  isActive: boolean;
};

function Dictation({
  cue,
  media,
  stateSuccess,
  setStateSuccess,
  onSuccess,
  onFocusInput,
  mode,
  isActive,
}: DictationProps) {
  const [stateInput, setStateInput] = useState<string>("");
  const [stateShowContent, setStateShowContent] = useState(false);
  const [stateShowReference, setStateShowReference] = useState(false);

  useEffect(() => {
    setStateInput("");
    setStateShowContent(false);
    setStateShowReference(false);
  }, [cue.uuid]);

  const isSuccess = (answer: string) => {
    return (
      answer === cue.content ||
      pureContent(answer) === pureContent(cue.content) ||
      (!!cue.reference && answer === cue.reference) ||
      (!!cue.reference && pureContent(answer) === pureContent(cue.reference))
    );
  };

  const getTip = (answer: string, content: string) => {
    const answerWords = splitContent(answer, true).map((v) => v.content);
    const tipParts = splitContent(content, false);
    const wordParts = tipParts.filter((v) => v.isWord);

    const matches = lcs(
      wordParts.map((v) => v.content),
      answerWords
    );
    const matchedIndexes = new Set(matches.map(([, contentIndex]) => contentIndex));

    wordParts.forEach((part, index) => {
      if (!matchedIndexes.has(index)) {
        part.content = hideWord(part.content);
      }
    });

    return tipParts.map((v) => v.content).join("");
  };

  const showTextArea = mode === "large" || mode === "medium";
  const inputClassName = fieldClass(mode);
  const tipSize = mode === "large" ? "large" : mode === "medium" ? "medium" : "small";

  const handleChange = (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    const content = e.target.value;
    if (content.endsWith("  ")) {
      if (!!media) {
        if (media.paused) playMediaPart(cue, media, false);
        else media.pause();
      }
    } else {
      setStateInput(content);
      if (!stateSuccess && isSuccess(content)) {
        setStateSuccess(true);
        onSuccess?.(cue.uuid, true);
      }
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    // Ctrl+←/→ toggles the content/reference tips of this focused cue.
    if (e.ctrlKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      setStateShowContent((v) => !v);
      setStateShowReference((v) => !v);
      e.preventDefault();
      return;
    }
    if (!media) return;
    if (e.ctrlKey && "sS".includes(e.key)) {
      if (media.paused) playMediaPart(cue, media, false);
      else media.pause();
      e.preventDefault();
    }
    if (e.ctrlKey && "dD".includes(e.key)) {
      setStateInput("");
      e.preventDefault();
    }
  };

  // Same effect as the Ctrl+←/→ shortcut, exposed as a tap target: on a phone there is
  // no keyboard and hover-only tooltips ("Double-click to toggle content/tip") are
  // unreachable, so clearing and revealing the answer need first-class controls.
  const answerShown = stateShowContent || stateShowReference;
  const toggleAnswer = () => {
    setStateShowContent(!stateShowContent);
    setStateShowReference(!stateShowReference);
  };

  return (
    <div className="flex flex-col items-start justify-center w-full gap-1">
      {showTextArea ? (
        <TextArea
          aria-label="input answer"
          autoComplete="one-time-code"
          id={`d-s-i-${cue.uuid}`}
          className={inputClassName}
          value={stateInput}
          rows={mode === "large" ? 5 : 3}
          onFocus={onFocusInput}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
        />
      ) : (
        <Input
          aria-label="input answer"
          autoComplete="one-time-code"
          id={`d-s-i-${cue.uuid}`}
          className={inputClassName}
          value={stateInput}
          onFocus={onFocusInput}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
        />
      )}
      {isActive && (
        <div className="flex items-center gap-1 flex-wrap">
          <Button variant="outline" size="sm" isDisabled={!stateInput} className="h-6 px-2 py-0 text-xs" onPress={() => setStateInput("")}>
            <Eraser size={12} /> Clear
          </Button>
          <Button variant={answerShown ? "primary" : "outline"} size="sm" className="h-6 px-2 py-0 text-xs" onPress={toggleAnswer}>
            {answerShown ? <EyeOff size={12} /> : <Eye size={12} />} {answerShown ? "Hide answer" : "Show answer"}
          </Button>
        </div>
      )}
      <TipToggle
        tip={getTip(stateInput, cue.content)}
        content={cue.content}
        showContent={stateShowContent}
        onToggle={() => setStateShowContent(!stateShowContent)}
        size={tipSize}
      />
      {!!cue.reference && cue.reference !== cue.content && (
        <TipToggle
          tip={getTip(stateInput, cue.reference)}
          content={cue.reference}
          showContent={stateShowReference}
          onToggle={() => setStateShowReference(!stateShowReference)}
          size={tipSize}
          className="mt-1"
        />
      )}
    </div>
  );
}

// ── CueEditor ─────────────────────────────────────────────────────────────

export type CueEditorProps = {
  cue: Cue;
  media: HTMLMediaElement | null;

  allowEdit: boolean;
  mode: "dictation" | "edit" | "dictation_edit" | "dictation_large";

  isDisabled: boolean;
  onUpdate: (updated: Cue) => void;
  onExpandStart: () => void;
  onExpandEnd: () => void;
  onDelete: () => void;
  onInsert: (index: number) => void;
  onMergeNext: () => void;
  onEdit: () => void;
  onDone: () => void;

  initialSuccess?: boolean;
  onSuccess?: (uuid: string, success: boolean) => void;
  onFocusInput?: () => void;
  /** When true, this is the active (focused) cue: the end button column is
   *  shown and the editor renders at a slightly larger size. */
  isActive?: boolean;
  onAddToFavorites?: () => void;
  /** When true, the "add to favorites" button is shown but disabled (e.g. the
   *  current dataset already IS the Favorites dataset). */
  favoritesDisabled?: boolean;
  /** When true, this cue is already in the Favorites dataset: the Star renders
   *  filled and disabled (one-way add — remove clips from the Favorites dataset). */
  isFavorited?: boolean;
};

export default function CueEditor({
  cue,
  media,
  allowEdit,
  mode,
  isDisabled,
  onUpdate,
  onExpandStart,
  onExpandEnd,
  onDelete,
  onInsert,
  onMergeNext,
  onEdit,
  onDone,
  initialSuccess,
  onSuccess,
  onFocusInput,
  isActive,
  onAddToFavorites,
  favoritesDisabled,
  isFavorited,
}: CueEditorProps) {
  const [stateStart, setStateStart] = useState(formatVttTime(cue.start_ms));
  const [stateEnd, setStateEnd] = useState(formatVttTime(cue.end_ms));
  const [stateSuccess, setStateSuccess] = useState<boolean>(initialSuccess ?? false);
  const editAreaRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => setStateStart(formatVttTime(cue.start_ms)), [cue.start_ms]);
  useEffect(() => setStateEnd(formatVttTime(cue.end_ms)), [cue.end_ms]);

  useEffect(() => {
    setStateSuccess(initialSuccess ?? false);
  }, [initialSuccess]);

  const timeEditorEl = () => {
    return (
      <InputGroup fullWidth className="max-w-sm shadow-none rounded-xl bg-bg-muted data-focus-within:border-x-2 data-focus-within:ring-0">
        <InputGroup.Prefix className="p-0 bg-bg-muted">
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="w-min px-2"
            isDisabled={isDisabled}
            onPress={onExpandStart}
          >
            <ArrowLeftToLine size={16} />
          </Button>
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="w-min px-2"
            isDisabled={isDisabled}
            onPress={() => {
              if (media) {
                const startMs = Math.round(media.currentTime * 1000);
                setStateStart(formatVttTime(startMs));
                onUpdate({ ...cue, start_ms: startMs, modified: true });
              }
            }}
          >
            <MapPin size={16} />
          </Button>
        </InputGroup.Prefix>
        <InputGroup.Input
          aria-label="start time"
          autoComplete="one-time-code"
          className={`min-w-0 text-center font-normal bg-bg-muted ${!(!!(validateVttTime(stateStart) && !!validateVttTime(stateEnd))
              ? true
              : false)
              ? "text-error-text"
              : ""
            }`}
          value={`${stateStart} ➔ ${stateEnd}`}
          disabled={isDisabled}
          onChange={(e) => {
            const parts = e.target.value.split(" ➔ ");
            setStateStart(parts[0]);
            setStateEnd(parts[1]);
          }}
          onBlur={() => {
            if (validateVttTime(stateStart))
              onUpdate({ ...cue, start_ms: parseVttTime(stateStart) });
            if (validateVttTime(stateEnd))
              onUpdate({ ...cue, end_ms: parseVttTime(stateEnd) });
          }}
        />
        <InputGroup.Suffix className="p-0 bg-bg-muted">
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="w-min px-2"
            isDisabled={isDisabled}
            onPress={() => {
              if (media) {
                const endMs = Math.round(media.currentTime * 1000);
                setStateEnd(formatVttTime(endMs));
                onUpdate({ ...cue, end_ms: endMs, modified: true });
              }
            }}
          >
            <MapPin size={16} />
          </Button>
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="w-min px-2"
            isDisabled={isDisabled}
            onPress={onExpandEnd}
          >
            <ArrowRightToLine size={16} />
          </Button>
        </InputGroup.Suffix>
      </InputGroup>
    );
  };

  const containerClass = (cue: Cue) => {
    if (cue.deleted) {
      return "flex flex-col gap-0.5 w-full border-solid border-error-text";
    }
    if (cue.modified) {
      return "flex flex-col gap-0.5 w-full border-solid border-accent";
    }
    return "flex flex-col gap-0.5 w-full";
  };

  const isDictationMode = mode === "dictation" || mode === "dictation_large";
  const inEditMode = mode === "edit" || mode === "dictation_edit";
  // End (right) column buttons: edit / done / add-favorites.
  const showEditButton = isDictationMode && allowEdit && isActive;
  const showDoneButton = mode === "dictation_edit" && allowEdit;
  const showDeleteButton = (mode === "edit" || mode === "dictation_edit") && allowEdit;
  const showFavoriteButton = isDictationMode && !!onAddToFavorites;

  return (
    <div className={containerClass(cue)}>
      {/* Narrow viewports (portrait phones) stack the action bar on top of the card via
          flex-col-reverse; from 640px up (landscape phones, tablets, desktop) the same bar
          becomes the right-hand rail. Width, not platform, is the real constraint. */}
      <div className="flex flex-col-reverse gap-1 sm:flex-row-reverse">
        {/* Left column: time row on top, then the cue content */}
        <div className="flex-1 flex flex-col gap-0.5 min-w-0">
          {/* Time row: the editable editor (with the capture / expand buttons) is a
              editing affordance, so it appears only in edit mode. An active cue in
              dictation mode shows the same compact time string as an inactive one —
              on a phone the editor ate a full row plus its button column. */}
          <div className="w-full">
            {inEditMode ? (
              timeEditorEl()
            ) : (
              <div className={`px-1 py-1 text-xs ${isActive ? "text-text-secondary" : "text-text-tertiary"}`}>{formatVttTime(cue.start_ms)} ➔ {formatVttTime(cue.end_ms)}</div>
            )}
          </div>
          {(mode === "edit" || mode === "dictation_edit") && allowEdit && (
            <div className="flex items-center gap-1 flex-wrap">
              <Button variant="outline" size="sm" isDisabled={isDisabled} className="h-6 px-2 py-0 text-xs" onPress={() => onInsert(cue.order_num)}>
                Insert Before
              </Button>
              <Button variant="outline" size="sm" isDisabled={isDisabled} className="h-6 px-2 py-0 text-xs" onPress={() => onInsert(cue.order_num + 1)}>
                Insert After
              </Button>
              <Button variant="outline" size="sm" isDisabled={isDisabled} className="h-6 px-2 py-0 text-xs" onPress={onMergeNext}>
                Merge Next
              </Button>
            </div>
          )}
          <div className={isDictationMode ? "" : "hidden"}>
            <Dictation
              cue={cue}
              media={media}
              stateSuccess={stateSuccess}
              setStateSuccess={setStateSuccess}
              onSuccess={onSuccess}
              onFocusInput={onFocusInput}
              mode={mode === "dictation_large" ? "large" : isActive ? "medium" : "compact"}
              isActive={!!isActive}
            />
          </div>
          <div className={mode === "edit" || mode === "dictation_edit" ? "w-full flex flex-col gap-1" : "hidden"}>
            <TextArea
              aria-label="text"
              autoComplete="one-time-code"
              ref={editAreaRef}
              rows={3}
              className={fieldClass(isActive ? "medium" : "compact")}
              disabled={isDisabled || cue.deleted}
              value={cue.content}
              onChange={(e) =>
                onUpdate({
                  ...cue,
                  content: e.target.value,
                  modified: e.target.value !== cue.content_original,
                })
              }
            />
            {/* Plain-text view of content and reference at the bottom */}
            <div className="flex flex-col gap-0.5">
              <div className="flex items-center gap-1">
                <span className="text-xs font-semibold uppercase text-text-tertiary">Content</span>
                <CopyButton text={cue.content} />
              </div>
              <div className="text-sm text-text-tertiary whitespace-pre-wrap break-words">{cue.content}</div>
            </div>
            {!!cue.reference && (
              <div className="flex flex-col gap-0.5">
                <div className="flex items-center gap-1">
                  <span className="text-xs font-semibold uppercase text-text-tertiary">Reference</span>
                  <CopyButton text={cue.reference} />
                </div>
                <div className="text-sm text-text-tertiary whitespace-pre-wrap break-words">{cue.reference}</div>
              </div>
            )}
          </div>
        </div>

        {/* Action bar — horizontal strip on top (narrow) or 36px vertical rail (≥640px).
            DOM order is [primary, mode, delete]: `ms-auto` pushes the mode/delete buttons into
            the top-right corner on a phone, while `sm:order-first` lifts them back to the top
            of the rail so the desktop column order (edit → star/play/mic → delete) is kept.
            On short viewports (landscape phones) the rail may outgrow the card's own content,
            so it wraps into extra columns instead of stretching the card. */}
        {(isActive || inEditMode) && (
          <div
            className={`flex flex-row items-center gap-1 w-full pb-1 pt-0 shrink-0 transition-colors rounded-lg sm:w-9 sm:flex-col md:pt-0.5 ${stateSuccess && isDictationMode ? "bg-success-bg" : "bg-transparent"} [@media(min-width:640px)_and_(max-height:500px)]:flex-wrap [@media(min-width:640px)_and_(max-height:500px)]:max-h-32`}
          >
            {isActive && (
              <div className="flex items-center gap-1 sm:flex-col">
                {showFavoriteButton && (
                  <Tooltip>
                    <Tooltip.Trigger>
                      <Button isIconOnly variant="ghost" size="sm" aria-label="Add to favorites" isDisabled={favoritesDisabled || isFavorited} onPress={() => onAddToFavorites?.()}>
                        <Star size={16} fill={isFavorited ? "currentColor" : "none"} />
                      </Button>
                    </Tooltip.Trigger>
                    <Tooltip.Content>{favoritesDisabled ? "already in the Favorites dataset" : isFavorited ? "already in Favorites" : "add to favorites"}</Tooltip.Content>
                  </Tooltip>
                )}
                <Tooltip isDisabled={!isDictationMode}>
                  <Tooltip.Trigger>
                    <Button
                      isIconOnly
                      variant="ghost"
                      size="sm"
                      aria-label="Play"
                      onPress={() => {
                        if (!media) return;
                        if (media.paused) playMediaPart(cue, media, false);
                        else media.pause();
                      }}
                    >
                      <Play size={16} />
                    </Button>
                  </Tooltip.Trigger>
                  <Tooltip.Content>shortcut: Ctrl+S or type two spaces at the end</Tooltip.Content>
                </Tooltip>
                {(isDictationMode || inEditMode) && <MicButton />}
              </div>
            )}
            <div className="flex items-center gap-1 ms-auto sm:ms-0 sm:order-first sm:flex-col">
              {showEditButton && (
                <Tooltip>
                  <Tooltip.Trigger>
                    <Button isIconOnly variant="ghost" size="sm" onPress={onEdit}>
                      <Pencil size={16} />
                    </Button>
                  </Tooltip.Trigger>
                  <Tooltip.Content>edit subtitle</Tooltip.Content>
                </Tooltip>
              )}
              {showDoneButton && (
                <Tooltip>
                  <Tooltip.Trigger>
                    <Button isIconOnly variant="ghost" size="sm" aria-label="Done editing" onPress={onDone}>
                      <X size={16} />
                    </Button>
                  </Tooltip.Trigger>
                  <Tooltip.Content>close editor</Tooltip.Content>
                </Tooltip>
              )}
            </div>
            {showDeleteButton && (
              <Tooltip>
                <Tooltip.Trigger>
                  <Button isIconOnly variant="ghost" size="sm" isDisabled={isDisabled} aria-label="Delete" onPress={onDelete}>
                    <Trash2 size={16} color="red" />
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>delete cue</Tooltip.Content>
              </Tooltip>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
