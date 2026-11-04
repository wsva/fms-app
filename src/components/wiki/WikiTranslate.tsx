"use client";

/**
 * WikiTranslate — the wiki's one AI road, in one popover.
 *
 * The books' Understand panel offers two roads (call the LLM provider, or ask an
 * agent) because a whole document translation is not something a single
 * completion can be trusted to do: the work is a *procedure* — read the file,
 * split it into blocks, decide what must stay verbatim, write a sibling file,
 * check it landed. So the wiki has one button, and it asks an agent.
 *
 * What this component does is file a request in the mailbox and hand the learner
 * the prompt for the case where no agent is attached. It translates nothing, and
 * — deliberately — the app verifies nothing: the answer is a markdown file the
 * learner opens, so the row records *who says it is done* rather than what the
 * app saw. That is why the filed line points at the dock instead of at a result.
 */

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Bot, Check, Copy, Languages } from "lucide-react";
import { ToolbarButton } from "@/components/ui/toolbar";
import { copyText } from "@/lib/clipboard";
import { isTauri } from "@/lib/platform";

/** Common targets, named in their own language so a learner recognises them.
 *  The `code` is what becomes part of the file name, so the list is closed: a
 *  free-text box would put a learner's typing into a path. */
const TARGETS: { code: string; name: string }[] = [
  { code: "en", name: "English" },
  { code: "de", name: "Deutsch" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "it", name: "Italiano" },
  { code: "zh", name: "中文" },
  { code: "ja", name: "日本語" },
  { code: "ru", name: "Русский" },
];

const LANG_KEY = "wiki-translate-lang";

/** What `wiki_translate_request` returns. `payload_json` carries the output name
 *  the *backend* computed, so this line never restates the naming rule. */
type Ask = {
  mail: { uuid: string; status: string; payload_json: string };
  prompt: string;
  replaced: string | null;
};

type Props = {
  datasetUuid: string;
  sourceRel: string;
};

/** The translated file's name, as the request stored it. */
function targetName(ask: Ask): string | null {
  try {
    const p = JSON.parse(ask.mail.payload_json) as { target_rel?: string };
    return p.target_rel?.split("/").pop() ?? null;
  } catch {
    return null;
  }
}

export default function WikiTranslate({ datasetUuid, sourceRel }: Props) {
  const [open, setOpen] = useState(false);
  const [lang, setLang] = useState<string>(() => {
    if (typeof window === "undefined") return "en";
    const stored = window.localStorage.getItem(LANG_KEY);
    return stored && TARGETS.some((t) => t.code === stored) ? stored : "en";
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [copied, setCopied] = useState(false);
  const [clipFailed, setClipFailed] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  // Another document is another ask: the popover must not open showing the
  // previous file's "asked" line.
  useEffect(() => {
    setAsk(null);
    setError(null);
    setCopied(false);
    setClipFailed(false);
  }, [datasetUuid, sourceRel]);

  // Click-outside closes it, as the model popover next to this one does.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!popoverRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const target = TARGETS.find((t) => t.code === lang) ?? TARGETS[0];

  const chooseLang = (code: string) => {
    setLang(code);
    window.localStorage.setItem(LANG_KEY, code);
    // A different language is a different request, so the old receipt stops
    // being the answer on screen.
    setAsk(null);
    setCopied(false);
    setError(null);
  };

  const askAgent = async () => {
    if (!isTauri()) {
      setError("The mailbox lives in the desktop app.");
      return;
    }
    setBusy(true);
    setError(null);
    setCopied(false);
    setClipFailed(false);
    try {
      const res = await invoke<Ask>("wiki_translate_request", {
        datasetUuid,
        sourceRel,
        langCode: target.code,
        langName: target.name,
      });
      setAsk(res);
    } catch (e) {
      // The refusal is a sentence naming the next move (another agent holds it,
      // the file is not local yet), so it is shown rather than summarised.
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const copyPrompt = async () => {
    if (!ask) return;
    const ok = await copyText(ask.prompt);
    setCopied(ok);
    setClipFailed(!ok);
  };

  return (
    <div className="relative shrink-0" ref={popoverRef}>
      <ToolbarButton
        variant="toggleSoft"
        active={open}
        icon={Languages}
        label="Translate"
        title="Ask an agent to translate this document into another language"
        onClick={() => setOpen((v) => !v)}
      />
      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 w-80 rounded-lg border border-border-default bg-bg-card shadow-2xl flex flex-col p-3 gap-2">
          <div className="flex items-center gap-2 text-xs text-text-secondary">
            <Bot size={13} className="shrink-0 text-text-tertiary" />
            <span className="min-w-0 flex-1">
              An agent reads the document and writes a new file beside it —
              every original paragraph followed by its translation.
            </span>
          </div>

          <select
            className="w-full px-2 py-1 rounded-md bg-bg-input border border-border-default text-text-primary text-xs focus:outline-none focus:border-accent"
            value={lang}
            onChange={(e) => chooseLang(e.target.value)}
          >
            {TARGETS.map((t) => (
              <option key={t.code} value={t.code}>
                {t.name}
              </option>
            ))}
          </select>

          <div className="flex items-center gap-2">
            <button
              type="button"
              className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium bg-accent text-white hover:opacity-90 cursor-pointer disabled:opacity-50 disabled:cursor-default shrink-0"
              onClick={() => void askAgent()}
              disabled={busy}
            >
              {busy ? "Asking…" : ask ? "Ask again" : "Ask agent"}
            </button>
            <button
              type="button"
              className="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-medium border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer disabled:opacity-40 disabled:cursor-default shrink-0"
              onClick={() => void copyPrompt()}
              disabled={!ask}
              title={ask ? "Copy the request to paste into an agent" : "Ask first — the request carries its own id"}
            >
              {copied ? <Check size={13} className="text-green-500" /> : <Copy size={13} />}
              {copied ? "Copied" : "Copy"}
            </button>
          </div>

          {ask && (
            <p className="text-[11px] text-text-secondary">
              {ask.replaced
                ? "Re-asked · the earlier request was replaced. "
                : "Asked. "}
              {targetName(ask) ? `Look for \`${targetName(ask)}\` when it lands. ` : ""}
              Track it in the Agent dock (Ctrl+Space).
            </p>
          )}
          {clipFailed && (
            <p className="text-[11px] text-amber-500">
              Could not reach the clipboard — copy from the Agent dock instead.
            </p>
          )}
          {error && <p className="text-[11px] text-red-500">{error}</p>}
        </div>
      )}
    </div>
  );
}
