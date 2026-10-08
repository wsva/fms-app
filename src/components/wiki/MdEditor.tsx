"use client";

// Body of the wiki markdown editor. Header chrome (Save / Preview / Fullscreen /
// Spell-check / Help toggles) and the Symbols/German/Format/Lists Tools panel
// both live on the wiki page's sub-toolbars now, so this component is
// controlled for those flags and exposes only the textarea, the Shortcuts
// panel, the Markdown preview, and the word/char counter.

import { forwardRef, useCallback, useEffect, useRef } from "react";
import MarkdownViewer from "./markdown/markdown";
import { insertAround, insertAtLineStart, replaceRange } from "./mdEditorInserts";

type Props = {
    value: string;
    onChange: (value: string) => void;
    preview: boolean;
    fullscreen: boolean;
    spellCheck: boolean;
    shortcutsOpen: boolean;
};

const shortcutList = [
    ["Ctrl+B", "bold"],
    ["Ctrl+I", "italic"],
    ["Ctrl+`", "inline code"],
    ["Tab", "indent"],
    ["Shift+Tab", "dedent"],
    ["Enter", "continue list"],
];

const MdEditor = forwardRef<HTMLTextAreaElement, Props>(({ value, onChange, preview, fullscreen, spellCheck, shortcutsOpen }, forwardedRef) => {
    const innerRef = useRef<HTMLTextAreaElement | null>(null);

    const wordCount = value.trim() ? value.trim().split(/\s+/).length : 0;
    const charCount = value.length;

    const autoResize = useCallback((el: HTMLTextAreaElement) => {
        if (fullscreen) {
            el.style.height = "";
            return;
        }
        const scrollY = window.scrollY;
        el.style.height = "auto";
        el.style.height = el.scrollHeight + "px";
        window.scrollTo(0, scrollY);
    }, [fullscreen]);

    useEffect(() => {
        if (innerRef.current) {
            autoResize(innerRef.current);
        }
    }, [value, autoResize]);

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
            const ta = innerRef.current;
            if (!ta) return;
            if (e.key === "b") { e.preventDefault(); insertAround(ta, "**", "**"); }
            else if (e.key === "i") { e.preventDefault(); insertAround(ta, "*", "*"); }
            else if (e.key === "`") { e.preventDefault(); insertAround(ta, "`", "`"); }
        }
    };
    
    return (
        <div className={fullscreen
            ? "flex flex-col min-h-0 flex-1 w-full bg-bg-body"
            : "flex flex-col py-0.5 bg-bg-card w-full"
        }>
            {/* Shortcuts panel — toggled by the wiki page's Help button. */}
            {shortcutsOpen && (
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
                    "w-full text-lg leading-relaxed p-1.5 outline-none resize-none overflow-hidden bg-bg-input border border-border-default rounded-md text-text-primary",
                    fullscreen ? "flex-1 min-h-0" : "min-h-40",
                    preview ? "hidden" : "",
                ].join(" ")}
                value={value}
                rows={1}
                autoComplete="off"
                autoCorrect="off"
                spellCheck={spellCheck}
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
            {preview && (
                <div className={[
                    "bg-bg-input border border-border-default rounded-md p-2",
                    fullscreen ? "flex-1 overflow-y-auto min-h-0" : "min-h-40",
                ].join(" ")}>
                    <MarkdownViewer content={value} withTOC />
                </div>
            )}

            {/* Word / char count */}
            {!preview && (
                <div className="text-xs text-text-tertiary text-right px-1">
                    {wordCount} words · {charCount} chars
                </div>
            )}
        </div>
    );
});

MdEditor.displayName = "MdEditor";

export default MdEditor;
