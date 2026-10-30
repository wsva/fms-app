"use client";

// Body of the wiki markdown editor. Header chrome (Save / View / Spell-check /
// Help toggles) and the Symbols/German/Format/Lists Tools panel both live on
// the wiki page's sub-toolbars now, so this component is controlled for those
// flags and exposes only the textarea, the Shortcuts panel, and the word/char
// counter. The rendered markdown view is a page-level concern: the wiki page
// swaps this component for its shared view renderer.

import { forwardRef, useCallback, useEffect, useRef } from "react";
import { insertAround, insertAtLineStart, insertLink, replaceRange } from "./mdEditorInserts";

type Props = {
    value: string;
    onChange: (value: string) => void;
    // Fullscreen stretch mode is kept for future embedders; the wiki page no
    // longer offers the toggle, so it stays at its default there.
    fullscreen?: boolean;
    spellCheck: boolean;
    shortcutsOpen: boolean;
    // Background utility for the textarea, so an embedder can match sibling
    // inputs (e.g. the read-aloud form's `bg-bg-muted` title field). Defaults to
    // the wiki editor's own `bg-bg-input`.
    textareaBgClass?: string;
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

// Elements whose content starts and ends on its own line. `<br>` is handled
// apart from these: it ends a line without opening a block around anything.
const blockTags = new Set([
    "address", "article", "aside", "blockquote", "dd", "div", "dl", "dt",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5",
    "h6", "header", "hr", "li", "main", "nav", "ol", "p", "pre", "section",
    "table", "tbody", "td", "th", "thead", "tr", "ul",
]);

/** Clipboard HTML → the plain text a Markdown textarea wants.
 *
 * Reading `textContent` straight off the fragment cannot work. Clipboard
 * producers pretty-print their markup, and the newlines *between* tags are
 * text nodes that survive there — so a pasted block arrived wrapped in blank
 * lines — while the breaks a reader actually sees (a block boundary, a `<br>`)
 * survive as nothing at all, which collapses a copied page onto one line. So
 * the tree is walked instead: the newlines come from the tags, whitespace that
 * only separates markup is worth at most one space, and `<pre>` content is
 * left exactly as it was copied.
 */
function htmlToPlainText(root: Node): string {
    const lines: string[] = [];
    let line = "";
    let space = false; // whitespace seen at the end of this line, not yet written
    let written = 0; // content written so far, to tell an empty block apart
    let pre = false;
    let preStart = false; // a <pre> whose first newline is markup, not content

    const flush = (blank = false) => {
        const text = line.replace(/\s+$/, "");
        if (text || blank) lines.push(text);
        line = "";
        space = false;
    };

    const walk = (node: Node) => {
        if (node.nodeType === Node.TEXT_NODE) {
            const raw = node.nodeValue ?? "";
            if (pre) {
                const text = preStart && raw.startsWith("\n") ? raw.slice(1) : raw;
                preStart = false;
                const [head, ...rest] = text.split("\n");
                line += head;
                for (const part of rest) {
                    flush();
                    line = part;
                }
                if (text) written++;
                return;
            }
            if (!raw.trim()) {
                // Markup indentation: between two inline tags it renders as a
                // space, at the start of a line it renders as nothing.
                if (line) space = true;
                return;
            }
            const text = raw.replace(/\s+/g, " ");
            line += line ? (space ? " " : "") + text : text.trimStart();
            space = false;
            written++;
            return;
        }
        if (node.nodeType !== Node.ELEMENT_NODE) return;
        const el = node as HTMLElement;
        const tag = el.tagName.toLowerCase();
        if (tag === "br") {
            flush(true);
            return;
        }
        const block = blockTags.has(tag);
        if (block) flush();
        const writtenBefore = written;
        const linesBefore = lines.length;
        if (tag === "pre") {
            pre = true;
            preStart = true;
        }
        for (const child of Array.from(el.childNodes)) walk(child);
        if (tag === "pre") pre = false;
        // A block that produced nothing was an empty paragraph, which reads as
        // a blank line in the source — keep it, or paragraphs run together.
        if (block) flush(written === writtenBefore && lines.length === linesBefore);
    };

    for (const child of Array.from(root.childNodes)) walk(child);
    flush();
    // Blank lines at either edge come from the markup, never from the copy.
    while (lines.length && !lines[0]) lines.shift();
    while (lines.length && !lines[lines.length - 1]) lines.pop();
    return lines.join("\n");
}

const MdEditor = forwardRef<HTMLTextAreaElement, Props>(({ value, onChange, fullscreen = false, spellCheck, shortcutsOpen, textareaBgClass = "bg-bg-input" }, forwardedRef) => {
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
        // An inert document, so no script or handler in the pasted markup runs.
        const doc = new DOMParser().parseFromString(html, "text/html");
        // Rich-text formatting is stripped (the point of this handler), but links
        // must survive as Markdown: a browser address-bar copy puts the page title
        // in the anchor text and the actual URL only in its href, so taking plain
        // text alone would drop the address entirely.
        for (const a of Array.from(doc.querySelectorAll("a[href]"))) {
            const href = a.getAttribute("href") || "";
            const text = (a.textContent ?? "").replace(/\s+/g, " ").trim();
            a.replaceWith(text && text !== href ? `[${text}](${href})` : href);
        }
        // If the rich flavour held no text at all, let the plain one through
        // rather than eating the paste.
        const text = htmlToPlainText(doc.body);
        replaceRange(e.currentTarget, text || e.clipboardData.getData("text/plain"));
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
                    `w-full text-lg leading-relaxed p-1.5 outline-none resize-none overflow-hidden ${textareaBgClass} border border-border-default rounded-md text-text-primary`,
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
