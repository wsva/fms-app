"use client";

// Port of the web-version editor (learning/fms/src/components/MdEditor.tsx):
// textarea + formatting toolbar + shortcuts + preview toggle, restyled with this
// app's Tailwind theme vars and HeroUI v3 sub-components, lucide icons, and the
// preview rendering the app's MarkdownViewer. Controlled via value/onChange;
// persistence is explicit through the Save button (onSave).

import { forwardRef, useCallback, useEffect, useRef, useState } from "react";
import { Button, Tooltip } from "@heroui/react";
import { CircleHelp, Maximize2, Minimize2, Save, X } from "lucide-react";
import MarkdownViewer from "./markdown/markdown";

type Props = {
    value: string;
    onChange: (value: string) => void;
    onSave?: () => void;
    onCancel?: () => void;
    saving?: boolean;
    dirty?: boolean;
    label?: string;
};

const char1 = ["#", "⬌", "■", "=", "≈", "➤", "🡆"];
const char1Tips = ["heading", "left-right arrow", "square", "equals", "approx.", "right arrow", "right arrow"];

const char2 = ["ä", "Ä", "ö", "Ö", "ü", "Ü", "ß", "é", "€"];

const char3 = [
    { label: "B", start: "**", end: "**", tip: "bold (Ctrl+B)" },
    { label: "„“", start: "„", end: "“", tip: "German double quotes" },
    { label: "‚‘", start: "‚", end: "‘", tip: "German single quotes" },
    { label: "`c`", start: "`", end: "`", tip: "inline code (Ctrl+`)" },
    { label: "C", start: "`````\n", end: "\n`````", tip: "code block" },
];

const char4 = [
    { label: "I", start: "*", end: "*", tip: "italic (Ctrl+I)" },
    { label: "~~", start: "~~", end: "~~", tip: "strikethrough" },
    { label: "🔗", start: "[", end: "](url)", tip: "link" },
    { label: "---", start: "\n---\n", end: "", tip: "horizontal rule" },
];

const shortcutList = [
    ["Ctrl+B", "bold"],
    ["Ctrl+I", "italic"],
    ["Ctrl+`", "inline code"],
    ["Tab", "indent"],
    ["Shift+Tab", "dedent"],
    ["Enter", "continue list"],
    ["Escape", "close toolbar"],
];

const MdEditor = forwardRef<HTMLTextAreaElement, Props>(({ value, onChange, onSave, onCancel, saving, dirty, label = "content" }, forwardedRef) => {
    const innerRef = useRef<HTMLTextAreaElement | null>(null);
    const [statePreview, setStatePreview] = useState(false);
    const [insertOpen, setInsertOpen] = useState(false);
    const [stateFullscreen, setStateFullscreen] = useState(false);
    const [stateSpellCheck, setStateSpellCheck] = useState(false);
    const [stateShowShortcuts, setStateShowShortcuts] = useState(false);

    const wordCount = value.trim() ? value.trim().split(/\s+/).length : 0;
    const charCount = value.length;

    const autoResize = useCallback((el: HTMLTextAreaElement) => {
        if (stateFullscreen) {
            el.style.height = "";
            return;
        }
        const scrollY = window.scrollY;
        el.style.height = "auto";
        el.style.height = el.scrollHeight + "px";
        window.scrollTo(0, scrollY);
    }, [stateFullscreen]);

    useEffect(() => {
        if (innerRef.current) {
            autoResize(innerRef.current);
        }
    }, [value, autoResize]);

    const toggleFullscreen = () => {
        setStateFullscreen((prev) => {
            const next = !prev;
            setTimeout(() => {
                if (innerRef.current) {
                    innerRef.current.style.height = "";
                    if (!next) autoResize(innerRef.current);
                }
            }, 0);
            return next;
        });
    };

    const replaceRange = (textarea: HTMLTextAreaElement, text: string, from?: number, to?: number) => {
        const s = from ?? textarea.selectionStart;
        const e = to ?? textarea.selectionEnd;
        textarea.setRangeText(text, s, e, "end");
        // setRangeText does not fire `input`; dispatch it so React's controlled
        // value updates through onChange instead of being clobbered on rerender.
        textarea.dispatchEvent(new Event("input", { bubbles: true }));
    };

    const insertToAnswer = (startText: string, endText: string) => {
        const textarea = innerRef.current;
        if (!textarea) return;
        textarea.focus();
        const start = textarea.selectionStart;
        const end = textarea.selectionEnd;
        const selected = textarea.value.slice(start, end);
        replaceRange(textarea, startText + selected + endText, start, end);
        setTimeout(() => {
            if (start === end) {
                textarea.setSelectionRange(start + startText.length, start + startText.length);
            } else {
                textarea.setSelectionRange(start, start + startText.length + selected.length + endText.length);
            }
        }, 0);
    };

    const insertAtLineStart = (prefix: string) => {
        const textarea = innerRef.current;
        if (!textarea) return;
        textarea.focus();
        const start = textarea.selectionStart;
        const lineStart = textarea.value.lastIndexOf("\n", start - 1) + 1;
        replaceRange(textarea, prefix, lineStart, lineStart);
    };

    const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
        const html = e.clipboardData.getData("text/html");
        if (!html) return;
        e.preventDefault();
        const div = document.createElement("div");
        div.innerHTML = html;
        replaceRange(e.currentTarget, div.textContent || div.innerText || "");
    };

    const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
        const isMac = navigator.userAgent.includes("Mac");
        const ctrl = isMac ? e.metaKey : e.ctrlKey;

        if (e.key === "Escape") {
            setInsertOpen(false);
            return;
        }

        // Ctrl+S saves (explicit save action, same as the Save button).
        if (ctrl && e.key === "s") {
            e.preventDefault();
            onSave?.();
            return;
        }

        if (e.key === "Enter") {
            const textarea = innerRef.current!;
            const start = textarea.selectionStart;
            const val = textarea.value;
            const lineStart = val.lastIndexOf("\n", start - 1) + 1;
            const line = val.slice(lineStart, start);
            const unordered = line.match(/^- (.*)/);
            const ordered = line.match(/^(\d+)\. (.*)/);
            if (unordered) {
                e.preventDefault();
                if (!unordered[1]) {
                    replaceRange(textarea, "", lineStart, start);
                } else {
                    replaceRange(textarea, "\n- ", start, start);
                }
                return;
            }
            if (ordered) {
                e.preventDefault();
                if (!ordered[2]) {
                    replaceRange(textarea, "", lineStart, start);
                } else {
                    replaceRange(textarea, `\n${parseInt(ordered[1]) + 1}. `, start, start);
                }
                return;
            }
        }

        if (e.key === "Tab") {
            e.preventDefault();
            if (e.shiftKey) {
                const textarea = innerRef.current!;
                const start = textarea.selectionStart;
                const lineStart = textarea.value.lastIndexOf("\n", start - 1) + 1;
                const spaces = textarea.value.slice(lineStart).match(/^ {1,2}/)?.[0] ?? "";
                if (spaces) {
                    replaceRange(textarea, "", lineStart, lineStart + spaces.length);
                }
            } else {
                replaceRange(innerRef.current!, "  ");
            }
            return;
        }

        if (ctrl) {
            if (e.key === "b") { e.preventDefault(); insertToAnswer("**", "**"); }
            else if (e.key === "i") { e.preventDefault(); insertToAnswer("*", "*"); }
            else if (e.key === "`") { e.preventDefault(); insertToAnswer("`", "`"); }
        }
    };

    const toolbar = (
        <div className="flex flex-wrap gap-x-6 gap-y-3 px-1 py-2 border-b border-border-default">
            <div>
                <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Symbols</div>
                <div className="flex flex-wrap gap-1">
                    {char1.map((v, i) => (
                        <Tooltip key={`c1-${i}`}>
                            <Tooltip.Trigger>
                                <Button size="sm" variant="ghost" isIconOnly className="text-xl" onPress={() => insertToAnswer(v, "")}>{v}</Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content>{char1Tips[i]}</Tooltip.Content>
                        </Tooltip>
                    ))}
                </div>
            </div>
            <div>
                <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">German</div>
                <div className="flex flex-wrap gap-1">
                    {char2.map((v, i) => (
                        <Tooltip key={`c2-${i}`}>
                            <Tooltip.Trigger>
                                <Button size="sm" variant="ghost" isIconOnly className="text-xl" onPress={() => insertToAnswer(v, "")}>{v}</Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content>{v}</Tooltip.Content>
                        </Tooltip>
                    ))}
                </div>
            </div>
            <div>
                <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Format</div>
                <div className="flex flex-wrap gap-1">
                    {[...char3, ...char4].map((v, i) => (
                        <Tooltip key={`c34-${i}`}>
                            <Tooltip.Trigger>
                                <Button size="sm" variant="ghost" isIconOnly className="text-xl" onPress={() => insertToAnswer(v.start, v.end)}>{v.label}</Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content>{v.tip}</Tooltip.Content>
                        </Tooltip>
                    ))}
                </div>
            </div>
            <div>
                <div className="text-xs text-text-tertiary uppercase tracking-wide mb-1">Lists</div>
                <div className="flex flex-wrap gap-1">
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button size="sm" variant="ghost" isIconOnly className="text-base font-mono" onPress={() => insertAtLineStart("- ")}>-</Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>unordered list</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button size="sm" variant="ghost" isIconOnly className="text-base font-mono" onPress={() => insertAtLineStart("1. ")}>1.</Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>ordered list</Tooltip.Content>
                    </Tooltip>
                </div>
            </div>
        </div>
    );

    return (
        <div className={stateFullscreen
            ? "fixed inset-0 z-50 flex flex-col bg-bg-body p-4 w-full"
            : "flex flex-col py-0.5 bg-bg-card w-full"
        }>
            {/* Header */}
            <div className="flex items-center justify-between bg-bg-card border border-border-default rounded-t-lg px-2">
                <div className="flex gap-1 items-center">
                    <label className="text-text-secondary text-sm">{label}</label>
                    <Button
                        size="sm"
                        variant={insertOpen ? "tertiary" : "ghost"}
                        onPress={() => setInsertOpen((v) => !v)}
                    >
                        Tools
                    </Button>
                </div>
                <div className="flex gap-1 items-center">
                    {onSave && (
                        <Button size="sm" variant={dirty ? "primary" : "ghost"} onPress={onSave} isDisabled={saving}>
                            <span className="flex items-center gap-1"><Save size={14} />{saving ? "Saving..." : "Save"}</span>
                        </Button>
                    )}
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button size="sm" variant="ghost" isIconOnly onPress={() => setStateShowShortcuts((v) => !v)}>
                                <CircleHelp size={16} />
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>keyboard shortcuts</Tooltip.Content>
                    </Tooltip>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button size="sm" variant={stateSpellCheck ? "tertiary" : "ghost"} onPress={() => setStateSpellCheck((v) => !v)}>
                                ABC
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>{stateSpellCheck ? "disable spell check" : "enable spell check"}</Tooltip.Content>
                    </Tooltip>
                    <Button size="sm" variant="ghost" onPress={() => setStatePreview((v) => !v)}>
                        {statePreview ? "editor" : "preview"}
                    </Button>
                    <Tooltip>
                        <Tooltip.Trigger>
                            <Button size="sm" variant="ghost" isIconOnly onPress={toggleFullscreen}>
                                {stateFullscreen ? <Minimize2 size={16} /> : <Maximize2 size={16} />}
                            </Button>
                        </Tooltip.Trigger>
                        <Tooltip.Content>{stateFullscreen ? "exit fullscreen" : "fullscreen"}</Tooltip.Content>
                    </Tooltip>
                    {onCancel && (
                        <Tooltip>
                            <Tooltip.Trigger>
                                <Button size="sm" variant="ghost" isIconOnly onPress={onCancel}>
                                    <X size={16} />
                                </Button>
                            </Tooltip.Trigger>
                            <Tooltip.Content>close editor</Tooltip.Content>
                        </Tooltip>
                    )}
                </div>
            </div>

            {/* Toolbar */}
            {insertOpen && toolbar}

            {/* Shortcuts panel */}
            {stateShowShortcuts && (
                <div className="flex flex-wrap gap-x-6 gap-y-1 px-2 py-2 text-sm text-text-secondary border-b border-border-default">
                    {shortcutList.map(([key, desc]) => (
                        <span key={key}>
                            <kbd className="font-mono text-xs bg-bg-hover px-1 rounded">{key}</kbd> {desc}
                        </span>
                    ))}
                </div>
            )}

            {/* TextArea */}
            <textarea
                className={[
                    "w-full text-lg leading-relaxed p-1.5 outline-none resize-none overflow-hidden bg-bg-input border border-t-0 border-border-default rounded-b-lg text-text-primary",
                    stateFullscreen ? "flex-1 min-h-0" : "min-h-40",
                    statePreview ? "hidden" : "",
                ].join(" ")}
                value={value}
                rows={1}
                autoComplete="off"
                autoCorrect="off"
                spellCheck={stateSpellCheck}
                onPaste={handlePaste}
                onInput={(e) => onChange(e.currentTarget.value)}
                onKeyDown={handleKeyDown}
                ref={(e) => {
                    innerRef.current = e;
                    if (typeof forwardedRef === "function") forwardedRef(e);
                    else if (forwardedRef) forwardedRef.current = e;
                }}
            />

            {/* Preview */}
            {statePreview && (
                <div className={[
                    "bg-bg-input border border-border-default rounded-md p-2",
                    stateFullscreen ? "flex-1 overflow-y-auto min-h-0" : "min-h-40",
                ].join(" ")}>
                    <MarkdownViewer content={value} withTOC />
                </div>
            )}

            {/* Word / char count */}
            {!statePreview && (
                <div className="text-xs text-text-tertiary text-right px-1">
                    {wordCount} words · {charCount} chars
                </div>
            )}
        </div>
    );
});

MdEditor.displayName = "MdEditor";

export default MdEditor;
