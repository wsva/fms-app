"use client";

// Body of the wiki markdown editor. Header chrome (Save / View / Fullscreen /
// Spell-check / Help toggles) and the Symbols/German/Format/Lists Tools panel
// both live on the wiki page's sub-toolbars now, so this component is
// controlled for those flags and exposes only the textarea, the Shortcuts
// panel, and the word/char counter. The rendered markdown view is a page-level
// concern: the wiki page swaps this component for its shared view renderer.

import { forwardRef, useCallback, useEffect, useRef } from "react";
import { insertAround, insertAtLineStart, insertLink, replaceRange } from "./mdEditorInserts";

type Props = {
    value: string;
    onChange: (value: string) => void;
    fullscreen: boolean;
    spellCheck: boolean;
    shortcutsOpen: boolean;
};

const shortcutList = [
    ["Ctrl+B", "bold"],
    ["Ctrl+I", "italic"],
    ["Ctrl+`", "inline code"],
    ["Ctrl+K", "link"],
    ["Tab", "indent"],
    ["Shift+Tab", "dedent"],
    ["Enter", "continue list"],
];

/** Nearest ancestor that actually scrolls. The wiki page scrolls its own
 *  overflow-y-auto content pane, not the window, so preserving the window's
 *  scroll position across a resize would be a no-op. */
function scrollParent(el: HTMLElement): HTMLElement | null {
    let p = el.parentElement;
    while (p) {
        const oy = getComputedStyle(p).overflowY;
        if ((oy === "auto" || oy === "scroll") && p.scrollHeight > p.clientHeight) return p;
        p = p.parentElement;
    }
    return null;
}

const MdEditor = forwardRef<HTMLTextAreaElement, Props>(({ value, onChange, fullscreen, spellCheck, shortcutsOpen }, forwardedRef) => {
    const innerRef = useRef<HTMLTextAreaElement | null>(null);

    const wordCount = value.trim() ? value.trim().split(/\s+/).length : 0;
    const charCount = value.length;

    const autoResize = useCallback((el: HTMLTextAreaElement) => {
        if (fullscreen) {
            el.style.height = "";
            return;
        }
        // Collapsing to height="auto" shrinks the pane's content, and the
        // browser clamps its scrollTop away from the caret; re-expanding never
        // brings it back, so the caret line lands pinned to the viewport
        // bottom. Snapshot and restore the real scroll parent's offset.
        const scroller = scrollParent(el);
        const savedTop = scroller ? scroller.scrollTop : window.scrollY;
        el.style.height = "auto";
        el.style.height = el.scrollHeight + "px";
        if (scroller) scroller.scrollTop = savedTop;
        else window.scrollTo(0, savedTop);
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
        // Rich-text formatting is stripped (the point of this handler), but links
        // must survive as Markdown: a browser address-bar copy puts the page title
        // in the anchor text and the actual URL only in its href, so taking plain
        // textContent alone would drop the address entirely.
        for (const a of Array.from(div.querySelectorAll("a[href]"))) {
            const href = a.getAttribute("href") || "";
            const text = a.textContent?.trim() || "";
            a.replaceWith(text && text !== href ? `[${text}](${href})` : href);
        }
        replaceRange(e.currentTarget, div.textContent || div.innerText || "");
    };

    const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
        const isMac = navigator.userAgent.includes("Mac");
        const ctrl = isMac ? e.metaKey : e.ctrlKey;

        // While an IME is composing, Enter/Tab belong to the candidate
        // picker — intercepting them would hijack confirmation.
        if (e.nativeEvent.isComposing) return;

        if (e.key === "Enter") {
            const textarea = e.currentTarget;
            const start = textarea.selectionStart;
            const val = textarea.value;
            const lineStart = val.lastIndexOf("\n", start - 1) + 1;
            const line = val.slice(lineStart, start);
            // Indent-tolerant so nested list items keep their depth; the
            // actual bullet (`-` or `*`) is repeated, not forced to `-`.
            const unordered = line.match(/^(\s*)([-*]) (.*)/);
            const ordered = line.match(/^(\s*)(\d+)\. (.*)/);
            const quote = line.match(/^(>\s?)(.*)/);
            if (unordered) {
                e.preventDefault();
                // Empty item drops marker and indent, exiting the list.
                if (!unordered[3]) {
                    replaceRange(textarea, "", lineStart, start);
                } else {
                    replaceRange(textarea, `\n${unordered[1]}${unordered[2]} `, start, start);
                }
                return;
            }
            if (ordered) {
                e.preventDefault();
                if (!ordered[3]) {
                    replaceRange(textarea, "", lineStart, start);
                } else {
                    replaceRange(textarea, `\n${ordered[1]}${parseInt(ordered[2]) + 1}. `, start, start);
                }
                return;
            }
            if (quote) {
                e.preventDefault();
                if (!quote[2]) {
                    replaceRange(textarea, "", lineStart, start);
                } else {
                    replaceRange(textarea, "\n> ", start, start);
                }
                return;
            }
        }

        if (e.key === "Tab") {
            e.preventDefault();
            const textarea = e.currentTarget;
            const { selectionStart: start, selectionEnd: end, value } = textarea;
            if (!value.slice(start, end).includes("\n")) {
                if (e.shiftKey) {
                    const lineStart = value.lastIndexOf("\n", start - 1) + 1;
                    const spaces = value.slice(lineStart).match(/^ {1,2}/)?.[0] ?? "";
                    if (spaces) {
                        replaceRange(textarea, "", lineStart, lineStart + spaces.length);
                    }
                } else {
                    replaceRange(textarea, "  ");
                }
                return;
            }
            // Multi-line selection: indent/outdent every touched line instead
            // of overwriting the selection with spaces.
            const blockStart = value.lastIndexOf("\n", start - 1) + 1;
            let blockEnd = value.indexOf("\n", end);
            if (blockEnd === -1) blockEnd = value.length;
            // A selection ending exactly at a line start doesn't touch that line.
            if (end === value.lastIndexOf("\n", end - 1) + 1) blockEnd = end - 1;
            const block = value.slice(blockStart, blockEnd);
            const lines = block.split("\n");
            const newBlock = e.shiftKey
                ? lines.map((l) => l.replace(/^ {1,2}/, "")).join("\n")
                : lines.map((l) => "  " + l).join("\n");
            if (newBlock === block) return;
            replaceRange(textarea, newBlock, blockStart, blockEnd);
            // Reselect the block so Tab can be pressed repeatedly to go deeper.
            setTimeout(() => textarea.setSelectionRange(blockStart, blockStart + newBlock.length), 0);
            return;
        }

        if (ctrl) {
            const ta = innerRef.current;
            if (!ta) return;
            // With Shift held e.key arrives uppercase; normalize so Ctrl+B/I
            // keep working.
            switch (e.key.toLowerCase()) {
                case "b": e.preventDefault(); insertAround(ta, "**", "**"); break;
                case "i": e.preventDefault(); insertAround(ta, "*", "*"); break;
                case "`": e.preventDefault(); insertAround(ta, "`", "`"); break;
                case "k": e.preventDefault(); insertLink(ta); break;
            }
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

            {/* Word / char count */}
            <div className="text-xs text-text-tertiary text-right px-1">
                {wordCount} words · {charCount} chars
            </div>
        </div>
    );
});

MdEditor.displayName = "MdEditor";

export default MdEditor;
