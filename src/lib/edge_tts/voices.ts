/**
 * Human-facing helpers for Edge TTS voices.
 *
 * A voice is identified by its short name (`de-DE-ConradNeural`) and grouped by
 * its locale (`de-DE`), which is a BCP 47 tag. Both the TTS page and the
 * read-aloud card group voices by locale, and a picker that lists `de-DE` reads
 * as a debug value — so the label lives here once instead of twice.
 */

const languageNames = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" });
  } catch {
    return null;
  }
})();

/** `"de-DE"` → `"German (Germany)"`. The runtime falls back to the tag itself. */
export function localeLabel(locale: string): string {
  try {
    return languageNames?.of(locale) || locale;
  } catch {
    return locale;
  }
}

/**
 * `de-DE-ConradNeural` → `Conrad`, for a list whose row already says which
 * language it is. Anything that does not have the `xx-YY-Name` shape is
 * returned without its suffix at least.
 */
export function voiceName(shortName: string): string {
  return shortName
    .replace(/^[^-]+-[^-]+-/, "")
    .replace(/Neural$/, "");
}

/**
 * `de-DE-ConradNeural` → `DE`: the region the voice actually speaks.
 *
 * A picker grouped by language lists every region at once, so `Conrad (DE)` and
 * `Conrad (AT)` have to be told apart by something more than their name.
 */
export function voiceRegion(shortName: string): string {
  return (shortName.split("-")[1] ?? "").toUpperCase();
}
