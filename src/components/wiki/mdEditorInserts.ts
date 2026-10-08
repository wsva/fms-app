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
    textarea.setRangeText(text, s, e, "end");
    // setRangeText does not fire `input`; dispatch it so React's controlled
    // value updates through onChange instead of being clobbered on rerender.
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
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
