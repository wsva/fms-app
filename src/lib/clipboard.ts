// ---------------------------------------------------------------------------
// One way to put a string on the clipboard.
//
// `navigator.clipboard` needs a secure context, and a Tauri webview's origin is
// not one on every platform — so the hidden-textarea fallback is not legacy
// politeness, it is the path that works where the modern API refuses. Both
// callers (the About card's build stamp, the card editor's `{{handle}}` copy)
// report the boolean rather than catching: a copy that went nowhere must say so.
// ---------------------------------------------------------------------------

/** Put `text` on the clipboard. False means nothing was copied. */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the textarea */
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  } catch {
    return false;
  }
}
