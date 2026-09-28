"use client";

import { useEffect, useRef, useState } from "react";
import { Plus } from "lucide-react";
import { buildAddCardPath, openCardUrl } from "@/lib/cards";

/**
 * Global text-selection context menu for cards.
 *
 * Ported from the website's CardContextMenu: whenever the user selects text
 * anywhere in the app (including inside <input>/<textarea>), a small floating
 * menu appears offering to create a card from the selection. Per the current
 * integration, "Add to Card" opens the prefilled /card/add page on the card
 * website in the OS default browser (no API key or login setup required).
 *
 * Mounted once in the app shell. Renders nothing until a selection is made, so
 * the initial server/client trees match (no hydration mismatch).
 */

type MenuAnchor = { x: number; top: number; bottom: number };

// Flip the menu below the selection when it would clip the top edge.
const TOP_MARGIN = 56;
// Rough half-width used to keep the menu inside the viewport.
const EDGE_MARGIN = 90;

export default function CardContextMenu() {
  const [anchor, setAnchor] = useState<MenuAnchor | null>(null);
  const [selectedText, setSelectedText] = useState("");
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      // Ignore interactions originating inside the menu itself.
      if (menuRef.current?.contains(e.target as Node)) return;

      const sel = window.getSelection();
      const selText = sel?.toString().trim();
      if (selText && sel?.rangeCount) {
        const rect = sel.getRangeAt(0).getBoundingClientRect();
        if (rect.width || rect.height) {
          setSelectedText(selText);
          setAnchor({ x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom });
          return;
        }
      }

      // window.getSelection() is empty inside <input>/<textarea>; read the
      // element's own selection instead.
      const active = document.activeElement as HTMLInputElement | HTMLTextAreaElement | null;
      if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) {
        const start = active.selectionStart ?? 0;
        const end = active.selectionEnd ?? 0;
        if (start < end) {
          const text = active.value.slice(start, end).trim();
          if (text) {
            const rect = active.getBoundingClientRect();
            setSelectedText(text);
            setAnchor({ x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom });
            return;
          }
        }
      }

      // No usable selection — dismiss.
      setAnchor(null);
    };

    const onPointerDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setAnchor(null);
      }
    };

    document.addEventListener("mouseup", onMouseUp);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("mouseup", onMouseUp);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, []);

  async function handleAddCard() {
    const text = selectedText;
    setAnchor(null);
    if (!text) return;
    try {
      await openCardUrl(buildAddCardPath({ question: text }));
    } catch (err) {
      console.error("Failed to open card page:", err);
    }
  }

  if (!anchor) return null;

  const flip = anchor.top < TOP_MARGIN;
  const style: React.CSSProperties = {
    position: "fixed",
    top: flip ? anchor.bottom : anchor.top,
    left: Math.max(8, Math.min(anchor.x, window.innerWidth - EDGE_MARGIN)),
    transform: flip ? "translate(-50%, 8px)" : "translate(-50%, calc(-100% - 8px))",
    zIndex: 9999,
  };

  return (
    <div
      ref={menuRef}
      style={style}
      className="bg-bg-card border border-border-default rounded-md shadow-lg py-1 min-w-40"
    >
      <div
        className="px-3 py-1 text-xs text-text-tertiary truncate max-w-[220px]"
        title={selectedText}
      >
        {selectedText}
      </div>
      <button
        onClick={handleAddCard}
        className="w-full text-left px-3 py-1.5 text-sm text-text-primary hover:bg-mid-gray/20 transition-colors cursor-pointer flex items-center gap-2"
      >
        <Plus size={14} className="shrink-0" />
        Add to Card
      </button>
    </div>
  );
}
