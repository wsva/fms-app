/**
 * Integration with the online card system (https://lusworkshop.site).
 *
 * Per the design decision, vocabulary review is delegated to the external card
 * website rather than a native flashcard system. Card routes are opened in the
 * OS default browser via the Tauri opener plugin (falling back to a new tab in
 * plain web preview), so the site's own cookie login is handled by the browser.
 */

import { isTauri } from "@/lib/tauri";

/** Origin of the online card system. Keep in sync with `BASE_URL` in src-tauri/src/auth.rs. */
export const CARD_BASE_URL = "https://lusworkshop.site";

export type CardLink = {
  key: string;
  label: string;
  description: string;
  /** Site-relative route passed to `open_card_window`. */
  path: string;
};

/** Entry points surfaced on the Cards tab. */
export const CARD_LINKS: CardLink[] = [
  { key: "my", label: "My Cards", description: "Browse and review your saved cards.", path: "/card/my" },
  { key: "table", label: "Card Table", description: "All your cards in a table view.", path: "/card/table" },
  { key: "add", label: "Add Card", description: "Create a card for a word, sentence, or note.", path: "/card/add?edit=y" },
  { key: "test", label: "Card Test", description: "Practice cards with spaced repetition.", path: "/card/test" },
  { key: "manage", label: "Manage Cards", description: "Organize cards by tag and user.", path: "/card/manage" },
  { key: "improve", label: "Improve Cards", description: "Review AI-suggested improvements.", path: "/card/improve" },
];

/**
 * Build a `/card/add` route with prefilled fields, for handing content off from
 * other parts of the app (e.g. a Read-a-Book sentence or a dictation cue).
 */
export function buildAddCardPath(params: {
  question?: string;
  answer?: string;
  note?: string;
  suggestion?: string;
  tags?: string[];
}): string {
  const sp = new URLSearchParams();
  sp.set("edit", "y");
  if (params.question) sp.set("question", params.question);
  if (params.answer) sp.set("answer", params.answer);
  if (params.note) sp.set("note", params.note);
  if (params.suggestion) sp.set("suggestion", params.suggestion);
  if (params.tags && params.tags.length > 0) sp.set("tags", params.tags.join(","));
  return `/card/add?${sp.toString()}`;
}

/**
 * Open a card-site route in the OS default browser (Tauri) or a new tab (web).
 * `path` is site-relative, e.g. `/card/my`.
 */
export async function openCardUrl(path: string): Promise<void> {
  const url = `${CARD_BASE_URL}${path}`;
  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } else {
    window.open(url, "_blank");
  }
}
