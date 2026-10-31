"use client";

// ---------------------------------------------------------------------------
// One field of a card, rendered or edited, in the same place on screen.
//
// The editor used to be two layouts switched by `viewMode`: a reading flow with
// its own typography and a form of labelled boxes with another. Same card, two
// geometries, so every mode switch moved the text under the reader's eye and
// re-taught them where the answer was. This component is the merge: the rendered
// half and the `<textarea>` half carry the same type classes and the same
// padding, so clicking a field to edit it does not move a line.
//
// It is presentational on purpose. Field values, dirtiness, the draft, and which
// fields are open all belong to the editor provider — a field that owned its own
// state could not tell the guard "unsaved", and per-field edit and whole-card
// edit are the same thing seen from two ends.
//
// Commit rules, because a card is read far more often than it is edited:
//   - a single-line field commits on Enter;
//   - a textarea commits on Ctrl/Cmd+Enter and on blur, so clicking anywhere
//     else in the card leaves no half-open field behind;
//   - Escape closes *this field only* and swallows the event, so it never reaches
//     the overlay's close guard. That was the old editor's worst habit: Escape ate
//     an in-progress answer and closed the card with it.
// ---------------------------------------------------------------------------

import { useCallback, useLayoutEffect, useRef, type CSSProperties } from "react";
import {
  Pencil, Paperclip, Image as ImageIcon, Music2, Video, FileQuestion,
} from "lucide-react";
import type { CardMedia, CardMediaField } from "@/lib/types";
import { resolveSrc } from "@/lib/media/anchors";
import { ToolbarMenuItem, ToolbarSelect } from "@/components/ui/toolbar";
import MediaField, { type MediaFieldTools } from "./MediaField";

const KIND_ICON = { image: ImageIcon, audio: Music2, video: Video } as const;

type Props = {
  field: CardMediaField;
  /** Shown in both modes, so the label does not appear and disappear on edit. */
  label: string;
  value: string;
  /** How the rendered half draws prose: markdown (answer/note) or plain. */
  mode: "plain" | "markdown";
  editing: boolean;
  /** This field's text differs from the last value read or saved. */
  dirty: boolean;
  media: CardMedia[];
  datasetPath: string;
  /** Where a new attachment or a Library "Place" lands. */
  attachTarget: boolean;
  /** A file is being dragged over the window, and would land here. */
  dropTarget?: boolean;
  /** One line: Enter commits instead of inserting a newline (suggestion). */
  singleLine?: boolean;
  placeholder?: string;
  /** Typography shared by both halves. */
  textClassName?: string;
  /** Font scale, applied to the value alone — never to this field's chrome. */
  textStyle?: CSSProperties;
  /** Desktop-only: the file pickers need the Tauri dialog plugin. */
  canAttach: boolean;
  /** Ignore a blur while a media dialog is up, or the field would close itself. */
  keepEditing?: boolean;
  onChange: (value: string) => void;
  onStartEdit: () => void;
  onEndEdit: (reason: "commit" | "cancel") => void;
  onAttach: (kind: "image" | "audio" | "video") => void;
  /** Upload / Copy / Remove on an attachment's own preview. Absent in hosts that
   *  only read a card, where a click on the field already means something. */
  mediaTools?: MediaFieldTools;
  /** Lets the provider insert a token at this field's caret. */
  registerElement: (el: HTMLTextAreaElement | HTMLInputElement | null) => void;
};

export default function EditableField({
  field,
  label,
  value,
  mode,
  editing,
  dirty,
  media,
  datasetPath,
  attachTarget,
  dropTarget = false,
  singleLine = false,
  placeholder,
  textClassName,
  textStyle,
  canAttach,
  keepEditing = false,
  onChange,
  onStartEdit,
  onEndEdit,
  onAttach,
  mediaTools,
  registerElement,
}: Props) {
  const elRef = useRef<HTMLTextAreaElement | HTMLInputElement | null>(null);

  // Auto-grow every multi-line field, not just the answer. The old editor gave
  // the answer `min-h-[15rem]` and the others `rows={1}`/`resize-y`, so three of
  // four fields showed a fraction of what they held.
  const grow = useCallback(() => {
    const el = elRef.current;
    if (!el || el.tagName !== "TEXTAREA") return;
    el.style.height = "0px";
    el.style.height = `${el.scrollHeight}px`;
  }, []);

  useLayoutEffect(grow, [value, editing, field, grow]);

  const attachRef = useCallback(
    (el: HTMLTextAreaElement | HTMLInputElement | null) => {
      elRef.current = el;
      registerElement(el);
      if (el && el.tagName === "TEXTAREA") {
        el.style.height = "0px";
        el.style.height = `${el.scrollHeight}px`;
      }
    },
    [registerElement]
  );

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      // Field-first: the overlay's guard must not see this keystroke, or Escape
      // would close the card on top of the text being typed.
      e.stopPropagation();
      e.preventDefault();
      onEndEdit("cancel");
      return;
    }
    if (singleLine) {
      if (e.key === "Enter") {
        e.preventDefault();
        onEndEdit("commit");
      }
      return;
    }
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      onEndEdit("commit");
    }
  };

  // Shared type between halves is the whole point; `inputCls` only adds the box.
  const typeCls = textClassName ?? "";
  // Every field is a panel on the page's own card surface — the same border and
  // background the reading level gives it. Editing should change what the page
  // does, not what it is made of.
  const surfaceCls = "rounded-xl border border-border-default bg-bg-card";
  const padCls = "px-3 py-2";

  // One border per field: the value element *is* the box, so an open field draws
  // no ring around the box it already has — that double outline was the old
  // box-inside-a-box. Only a drag rings, because a drag is transient and has to
  // win over whatever the field looks like. The attach target is marked on the
  // label instead: a permanent accent ring reads as focus, and the question wore
  // one whenever it happened to be the target.
  const stateCls = dropTarget
    ? "ring-2 ring-accent"
    : editing
      ? ""
      : "hover:ring-1 hover:ring-accent/40";

  // This field's own attachments, in table order — the strip that makes a
  // `{{handle}}` token legible without decoding it in the text.
  const rows = media.filter((m) => m.field === field);

  return (
    <section className={`group/field relative transition-shadow ${stateCls}`}>
      <header className="flex items-center gap-2 min-h-7 mb-1">
        {/* The label is a label, not a heading: it rides at word scale and steps
            back while you read. Hovering or focusing the field brings it up; on a
            phone there is no hover, so it is simply always there. */}
        <span
          className={`select-none text-[11px] uppercase tracking-[0.08em] text-text-tertiary transition-opacity ${
            editing
              ? "opacity-70"
              : "opacity-0 group-hover/field:opacity-100 group-focus-within/field:opacity-100 max-sm:opacity-100"
          }`}
        >
          {label}
        </span>
        {/* Where the toolbar's Attach will land. A glyph, not a ring: it has to
            be legible without claiming the field is focused. */}
        {attachTarget && editing && (
          <span
            className="flex shrink-0 items-center text-accent"
            title={`Attachments land in ${label}`}
          >
            <Paperclip size={10} aria-hidden />
          </span>
        )}
        {dirty && (
          <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-secondary select-none">
            unsaved
          </span>
        )}
        <div className="flex-1 min-w-0" />
        {/* Quiet until the field is addressed, and icon-only when it appears: one
            labelled Attach row per field made five attach controls on a card, and
            the toolbar's copy already targets this field. `onMouseDown`
            preventDefault: a menu that steals focus would blur the textarea into
            a commit and unmount its own menu mid-click. */}
        <div
          className="flex items-center gap-1 opacity-0 transition-opacity group-hover/field:opacity-100 group-focus-within/field:opacity-100 max-sm:opacity-100"
          onMouseDown={(e) => e.preventDefault()}
        >
          {editing && canAttach && (
            <ToolbarSelect
              iconOnly
              icon={Paperclip}
              label={`Attach to ${label}`}
              value=""
              title={`Attach an image, audio, or video to ${label}`}
            >
              <ToolbarMenuItem icon={ImageIcon} onClick={() => onAttach("image")}>
                Image
              </ToolbarMenuItem>
              <ToolbarMenuItem icon={Music2} onClick={() => onAttach("audio")}>
                Audio
              </ToolbarMenuItem>
              <ToolbarMenuItem icon={Video} onClick={() => onAttach("video")}>
                Video
              </ToolbarMenuItem>
            </ToolbarSelect>
          )}
          {!editing && (
            <button
              type="button"
              onClick={onStartEdit}
              title={`Edit ${label.toLowerCase()}`}
              aria-label={`Edit ${label.toLowerCase()}`}
              className="flex h-7 w-7 items-center justify-center rounded-md text-text-tertiary hover:bg-bg-hover hover:text-text-primary"
            >
              <Pencil size={13} />
            </button>
          )}
        </div>
      </header>

      {editing ? (
        <div className="space-y-2">
          {rows.length > 0 && (
            <ul className="flex flex-wrap items-center gap-1.5">
              {rows.map((m) => {
                const Icon = KIND_ICON[m.kind] ?? FileQuestion;
                const src = m.kind === "image" ? resolveSrc(datasetPath, m.rel_path) : "";
                const gone = m.present === false;
                return (
                  <li
                    key={m.uuid}
                    title={gone ? `${m.rel_path} — not on this device` : m.rel_path}
                    // The strip is the staging tray, not a second copy of the
                    // text: an attachment its token is already in the field for is
                    // dimmed, and one that is attached but unplaced keeps full
                    // weight and a dot, because that is the one to act on.
                    className={`flex items-center gap-1 rounded border border-border-default bg-bg-surface px-1.5 py-1 text-[11px] text-text-secondary max-w-[13rem] ${
                      value.includes(`{{${m.handle}}}`) ? "opacity-50 hover:opacity-100" : ""
                    }`}
                  >
                    {src && !gone ? (
                      <img src={src} alt="" className="h-5 w-5 rounded-sm object-cover" draggable={false} />
                    ) : (
                      <Icon size={13} className={gone ? "text-error-text" : "shrink-0"} />
                    )}
                    <span className="truncate">
                      {m.title || m.rel_path.split(/[/\\]/).pop() || m.handle}
                    </span>
                    {!value.includes(`{{${m.handle}}}`) && (
                      <span
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-warning-text"
                        title="Attached, not placed in the text"
                      />
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {/* The value is the only thing the font step scales. Zooming an ancestor
              inflated the label, the chips and the attach control with it, which
              is why a 160% answer read like a poster. */}
          <div style={textStyle}>
            {singleLine ? (
              <input
                ref={attachRef as (el: HTMLInputElement | null) => void}
                type="text"
                value={value}
                placeholder={placeholder}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={onKeyDown}
                onBlur={() => {
                  if (!keepEditing) onEndEdit("commit");
                }}
                className={`w-full ${surfaceCls} ${padCls} ${typeCls} text-text-primary placeholder:text-text-tertiary outline-none focus:border-accent`}
              />
            ) : (
              <textarea
                ref={attachRef as (el: HTMLTextAreaElement | null) => void}
                value={value}
                placeholder={placeholder}
                rows={1}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={onKeyDown}
                onBlur={() => {
                  if (!keepEditing) onEndEdit("commit");
                }}
                className={`w-full block resize-none overflow-hidden ${surfaceCls} ${padCls} ${typeCls} text-text-primary placeholder:text-text-tertiary outline-none focus:border-accent`}
              />
            )}
          </div>
        </div>
      ) : (
        // Reading half. Clicking anywhere in it opens this one field, so "edit"
        // is a place in the card rather than a mode the toolbar switches you into.
        <div
          role="button"
          tabIndex={0}
          aria-label={`Edit ${label.toLowerCase()}`}
          onClick={onStartEdit}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onStartEdit();
            }
          }}
          style={textStyle}
          className={`cursor-text ${surfaceCls} ${padCls} outline-none focus-visible:ring-1 ${
            value.trim() ? "" : "text-text-tertiary italic"
          }`}
        >
          {value.trim() ? (
            <MediaField
              text={value}
              media={media.filter((m) => m.field === field)}
              datasetPath={datasetPath}
              mode={mode}
              textClassName={typeCls}
              tools={mediaTools}
            />
          ) : (
            <span className={`whitespace-pre-wrap break-words ${typeCls}`}>
              {placeholder || `No ${label.toLowerCase()}`}
            </span>
          )}
        </div>
      )}

      {/* Said only while a file is actually over the field: as a permanent caption
          it was a promise about a gesture nobody was making. */}
      {dropTarget && (
        <p className="mt-1 text-right text-[11px] text-accent select-none">
          attachments land here
        </p>
      )}
    </section>
  );
}
