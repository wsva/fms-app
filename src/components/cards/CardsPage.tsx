"use client";

import { useEffect, useState } from "react";
import { ExternalLink } from "lucide-react";
import { isTauri } from "@/lib/tauri";
import { CARD_BASE_URL, CARD_LINKS, openCardUrl } from "@/lib/cards";

/**
 * Cards tab — entry points into the online card system.
 *
 * Each action opens the website in the OS default browser (or a new tab when
 * running outside Tauri). The site uses its own cookie login, handled entirely
 * by the browser.
 */
export default function CardsPage() {
  const [error, setError] = useState<string | null>(null);
  // Resolved after mount to avoid a hydration mismatch: `isTauri()` is false
  // during SSR/prerender but true inside the Tauri webview, so it must not be
  // read during the initial render. `null` = not yet determined.
  const [inTauri, setInTauri] = useState<boolean | null>(null);

  useEffect(() => {
    setInTauri(isTauri());
  }, []);

  async function handleOpen(path: string) {
    setError(null);
    try {
      await openCardUrl(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <main className="flex-1 p-8 overflow-y-auto">
      <h1 className="text-[1.8em] font-bold mb-2">Cards</h1>
      <p className="text-text-secondary text-sm mb-6">
        Vocabulary and sentence cards are powered by the online card system at{" "}
        <span className="font-medium">{CARD_BASE_URL}</span>. Each action opens in your default
        browser.
        {inTauri === false && " (Running outside the desktop app: links open in a new browser tab.)"}
      </p>

      {error && (
        <p className="text-sm text-red-500 mb-4">Failed to open the card site: {error}</p>
      )}

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
        {CARD_LINKS.map((link) => (
          <button
            key={link.key}
            onClick={() => handleOpen(link.path)}
            className="text-left p-4 rounded-xl border border-border-default bg-bg-card hover:bg-mid-gray/20 transition-colors cursor-pointer flex flex-col gap-1"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="font-semibold text-text-primary">{link.label}</span>
              <ExternalLink size={16} className="text-text-tertiary shrink-0" />
            </div>
            <span className="text-sm text-text-secondary">{link.description}</span>
          </button>
        ))}
      </div>

      <p className="text-xs text-text-tertiary mt-6">
        Cards open in your default browser, where you sign in to the card website as usual.
      </p>
    </main>
  );
}
