// Textarea mutation helpers shared by MdEditor (keyboard shortcuts) and the
// wiki page's Tools sub-toolbar (symbol / German / format / list buttons). They
// operate directly on the HTMLTextAreaElement so both callers can drive the
// controlled textarea without going through a component method.

export function replaceRange(
    textarea: HTMLTextAreaElement,
    text: string,
    from?: number,
    to?: number,
) {
    const s = from ?? textarea.selectionStart;
    const e = to ?? textarea.selectionEnd;
    if (!text && s === e) return;
    // execCommand only applies to the focused element.
    if (document.activeElement !== textarea) textarea.focus();
    textarea.setSelectionRange(s, e);
    // Prefer execCommand: unlike setRangeText it keeps the native undo stack
    // intact (one Ctrl+Z undoes just the insert, not everything before it),
    // and it fires `input` itself so React's controlled value updates through
    // onChange. Chromium supports it on textareas (our Tauri webviews); other
    // engines return false and we fall back to the manual path below.
    let ok = false;
    try {
        ok = text
            ? document.execCommand("insertText", false, text)
            : document.execCommand("delete");
    } catch {
        ok = false;
    }
    if (!ok) {
        textarea.setRangeText(text, s, e, "end");
        // setRangeText does not fire `input`; dispatch it so React's controlled
        // value updates through onChange instead of being clobbered on rerender.
        textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }
}

export function insertAround(
    textarea: HTMLTextAreaElement,
    startText: string,
    endText: string,
) {
    textarea.focus();
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const selected = textarea.value.slice(start, end);
    // Toggle off: markers already sit inside the selection.
    if (
        selected.length >= startText.length + endText.length &&
        selected.startsWith(startText) &&
        selected.endsWith(endText)
    ) {
        const inner = selected.slice(startText.length, selected.length - endText.length);
        replaceRange(textarea, inner, start, end);
        setTimeout(() => textarea.setSelectionRange(start, start + inner.length), 0);
        return;
    }
    // Toggle off: markers sit just outside the selection — covers the caret
    // parked inside an empty pair too (collapsed selection matches here).
    if (
        textarea.value.slice(start - startText.length, start) === startText &&
        textarea.value.slice(end, end + endText.length) === endText
    ) {
        replaceRange(textarea, selected, start - startText.length, end + endText.length);
        const innerStart = start - startText.length;
        setTimeout(() => textarea.setSelectionRange(innerStart, innerStart + selected.length), 0);
        return;
    }
    replaceRange(textarea, startText + selected + endText, start, end);
    setTimeout(() => {
        if (start === end) {
            textarea.setSelectionRange(start + startText.length, start + startText.length);
        } else {
            textarea.setSelectionRange(start, start + startText.length + selected.length + endText.length);
        }
    }, 0);
}

export function insertAtLineStart(textarea: HTMLTextAreaElement, prefix: string) {
    textarea.focus();
    const start = textarea.selectionStart;
    const lineStart = textarea.value.lastIndexOf("\n", start - 1) + 1;
    replaceRange(textarea, prefix, lineStart, lineStart);
}

// Ctrl+K: wrap the selection as a Markdown link and park the caret inside the
// empty parens so the URL can be typed immediately — mirrors the [title](url)
// shape the paste handler produces.
export function insertLink(textarea: HTMLTextAreaElement) {
    textarea.focus();
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const selected = textarea.value.slice(start, end);
    replaceRange(textarea, `[${selected}]()`, start, end);
    const urlPos = start + selected.length + 3;
    setTimeout(() => textarea.setSelectionRange(urlPos, urlPos), 0);
}
