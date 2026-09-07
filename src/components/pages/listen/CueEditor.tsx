"use client";

import React, { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button, Chip, Input, InputGroup, TextArea, Tooltip } from "@heroui/react";
import { formatVttTime, parseVttTime, validateVttTime } from "@/lib/listen/subtitle";
import { hideWord, playMediaPart, pureContent, splitContent } from "@/lib/listen/utils";
import {
  ArrowLeftToLine,
  ArrowRightToLine,
  MapPin,
  Mic,
  Pencil,
  Play,
  Trash2,
  X,
} from "lucide-react";
import { lcs } from "@/lib/listen/lcs";
import type { Cue } from "@/lib/types";
import { handleToggle, subscribe, getVoiceState, getVoiceError, type VoiceState } from "@/lib/voice-input";

// ── Mic Button ────────────────────────────────────────────────────────────────

function MicButton() {
  const voiceState = useSyncExternalStore(subscribe, getVoiceState, getVoiceState);
  const error = useSyncExternalStore(subscribe, getVoiceError, getVoiceError);

  return (
    <div className="flex items-center gap-1">
      {voiceState === "processing" && (
        <span className="text-xs text-text-tertiary animate-pulse">Transcribing…</span>
      )}
      {error && <span className="text-xs text-red-500">{error}</span>}
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
    </div>
  );
}

// ── Dictation ────────────────────────────────────────────────────────────────

type DictationProps = {
  cue: Cue;
  media: HTMLMediaElement | null;
  stateSuccess: boolean;
  setStateSuccess: React.Dispatch<React.SetStateAction<boolean>>;
  onSuccess?: (uuid: string, success: boolean) => void;
  onFocusInput?: () => void;
  mode: "compact" | "large";
};

function Dictation({
  cue,
  media,
  stateSuccess,
  setStateSuccess,
  onSuccess,
  onFocusInput,
  mode,
}: DictationProps) {
  const [stateInput, setStateInput] = useState<string>("");

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

  return (
    <div>
      {mode === "compact" ? (
        <div className="flex flex-col items-start justify-center w-full gap-1">
          <div className="flex flex-row items-center justify-start w-full gap-1">
            <Input
              aria-label="input answer"
              autoComplete="one-time-code"
              id={`d-s-i-${cue.uuid}`}
              className="text-xl font-bold border-b-2 border-b-border-light bg-bg-muted rounded-none p-0 my-1 w-full shadow-none focus:ring-0 focus:border-b-accent"
              value={stateInput}
              onFocus={onFocusInput}
              onChange={(e) => {
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
              }}
              onKeyDown={(e) => {
                if (!media) return;
                if (e.ctrlKey && "sS".includes(e.key)) {
                  if (media.paused) playMediaPart(cue, media, false);
                  else media.pause();
                  e.preventDefault();
                }
                if (e.ctrlKey && "dD".includes(e.key)) {
                  if (media.paused) playMediaPart(cue, media, false);
                  else media.pause();
                  e.preventDefault();
                }
              }}
            />
          </div>
          <div className="bg-bg-muted rounded-sm px-1 text-text-tertiary font-normal w-full">
            {getTip(stateInput, cue.content)}
          </div>
          {!!cue.reference && cue.reference !== cue.content && (
            <div className="bg-bg-muted rounded-sm px-1 mt-3 text-text-tertiary font-normal">
              {getTip(stateInput, cue.reference)}
            </div>
          )}
        </div>
      ) : (
        <div className="flex flex-col items-start justify-center w-full gap-1">
          <div className="flex flex-row items-center justify-start w-full gap-1">
            <TextArea
              aria-label="input answer"
              autoComplete="one-time-code"
              id={`d-s-i-${cue.uuid}`}
              className="text-4xl font-bold border-b-2 border-b-border-light bg-bg-muted rounded-lg p-2 my-1 w-full shadow-none focus:ring-0 focus:border-b-accent"
              value={stateInput}
              rows={5}
              onFocus={onFocusInput}
              onChange={(e) => {
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
              }}
              onKeyDown={(e) => {
                if (!media) return;
                if (e.ctrlKey && "sS".includes(e.key)) {
                  if (media.paused) playMediaPart(cue, media, false);
                  else media.pause();
                  e.preventDefault();
                }
                if (e.ctrlKey && "dD".includes(e.key)) {
                  if (media.paused) playMediaPart(cue, media, false);
                  else media.pause();
                  e.preventDefault();
                }
              }}
            />
          </div>
          <div className="bg-bg-muted rounded-sm px-1 text-text-tertiary text-2xl w-full">
            {getTip(stateInput, cue.content)}
          </div>
          {!!cue.reference && cue.reference !== cue.content && (
            <div className="bg-bg-muted rounded-sm px-1 mt-3 text-text-tertiary text-2xl">
              {getTip(stateInput, cue.reference)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── CueEditor ─────────────────────────────────────────────────────────────

export type CueEditorProps = {
  cue: Cue;
  media: HTMLMediaElement | null;

  allowEdit: boolean;
  mode: "dictation" | "edit" | "dictation_edit" | "dictation_focus";

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
      <InputGroup className="w-xs shadow-none data-focus-within:border-x-2 data-focus-within:ring-0">
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
          className={`text-center font-normal bg-bg-muted w-min ${
            !(!!(validateVttTime(stateStart) && !!validateVttTime(stateEnd))
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

  return (
    <div className={containerClass(cue)}>
      <div className="flex flex-row items-center justify-start w-full gap-1">
        <Tooltip isDisabled={mode !== "dictation"}>
          <Tooltip.Trigger>
            <Chip size="lg" variant="primary" color={stateSuccess ? "success" : undefined}>
              <span className="text-sm font-medium">{cue.order_num}</span>
            </Chip>
          </Tooltip.Trigger>
          <Tooltip.Content>turn green on success: punctuation does not matter</Tooltip.Content>
        </Tooltip>
        <div className="hidden lg:flex">{timeEditorEl()}</div>
        <Tooltip isDisabled={mode !== "dictation"}>
          <Tooltip.Trigger>
            <Button
              isIconOnly
              variant="ghost"
              size="sm"
              onPress={() => {
                if (!media) return;
                if (media.paused) playMediaPart(cue, media, false);
                else media.pause();
              }}
            >
              <Play size={16} />
            </Button>
          </Tooltip.Trigger>
          <Tooltip.Content>shortcut: Ctrl+S, Ctrl+D, or type two spaces at the end</Tooltip.Content>
        </Tooltip>
        {(mode === "dictation" || mode === "dictation_focus") && (
          <MicButton />
        )}
        {(mode === "edit" || mode === "dictation_edit") && (
          <MicButton />
        )}
        {(mode === "edit" || mode === "dictation_edit") && allowEdit && (
          <>
            <Tooltip>
              <Tooltip.Trigger>
                <Button
                  isIconOnly
                  variant="ghost"
                  size="sm"
                  isDisabled={isDisabled}
                  onPress={() => onInsert(cue.order_num)}
                >
                  <div className="text-lg">#1</div>
                </Button>
              </Tooltip.Trigger>
              <Tooltip.Content>insert before</Tooltip.Content>
            </Tooltip>
            <Tooltip>
              <Tooltip.Trigger>
                <Button
                  isIconOnly
                  variant="ghost"
                  size="sm"
                  isDisabled={isDisabled}
                  onPress={() => onInsert(cue.order_num + 1)}
                >
                  <div className="text-lg">#2</div>
                </Button>
              </Tooltip.Trigger>
              <Tooltip.Content>insert after</Tooltip.Content>
            </Tooltip>
            <Tooltip>
              <Tooltip.Trigger>
                <Button
                  isIconOnly
                  variant="ghost"
                  size="sm"
                  isDisabled={isDisabled}
                  onPress={onMergeNext}
                >
                  <div className="text-lg">#3</div>
                </Button>
              </Tooltip.Trigger>
              <Tooltip.Content>merge next</Tooltip.Content>
            </Tooltip>
            <Button isIconOnly variant="ghost" size="sm" isDisabled={isDisabled} onPress={onDelete}>
              <Trash2 size={16} color="red" />
            </Button>
          </>
        )}
        <div className="ml-auto flex gap-1.5">
          {mode === "dictation" && allowEdit && (
            <div>
              <Tooltip>
                <Tooltip.Trigger>
                  <Button isIconOnly variant="ghost" size="sm" onPress={onEdit}>
                    <Pencil size={16} />
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>edit subtitle</Tooltip.Content>
              </Tooltip>
            </div>
          )}
          {mode === "dictation_edit" && allowEdit && (
            <div>
              <Button isIconOnly variant="ghost" size="sm" onPress={onDone}>
                <X size={16} />
              </Button>
            </div>
          )}
        </div>
      </div>
      <div className="lg:hidden flex">{timeEditorEl()}</div>
      <div className={mode === "dictation" || mode === "dictation_focus" ? "" : "hidden"}>
        <Dictation
          cue={cue}
          media={media}
          stateSuccess={stateSuccess}
          setStateSuccess={setStateSuccess}
          onSuccess={onSuccess}
          onFocusInput={onFocusInput}
          mode={mode === "dictation_focus" ? "large" : "compact"}
        />
      </div>
      <div className={mode === "edit" || mode === "dictation_edit" ? "w-full" : "hidden"}>
        <div className="flex items-start gap-2">
          <TextArea
            aria-label="text"
            autoComplete="one-time-code"
            ref={editAreaRef}
            className="w-full text-xl font-bold border-2 border-border-light flex-1"
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
        </div>
        {!!cue.reference && <div>{cue.reference}</div>}
      </div>
    </div>
  );
}
