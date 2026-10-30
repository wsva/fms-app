"use client";

/**
 * What FmS is, which build you are running, and whether there is a newer one.
 *
 * One card, two doors: Settings → About and Help → About FmS (the dialog). The
 * shape is deliberately not the key/value table it replaces. A table gives every
 * row the same weight, so "Installed version" sat level with a progress bar and
 * the one button could only be indented into the value column — which is why the
 * old block needed placeholder spans to fake alignment. Here the identity is a
 * header, the update is one sentence with one action on its own row, and the
 * links are a footer. Nothing is a peer of anything it is not.
 *
 * The card never states an update fact twice: the version on the right answers
 * "what am I running", the sentence answers "is there something newer", and the
 * changelog is folded away until asked for.
 */

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDown, ChevronRight } from "lucide-react";
import { CARD_BASE_URL } from "@/lib/cards";
import { isTauri } from "@/lib/platform";
import {
  checkForUpdates,
  describeUpdate,
  installUpdate,
  ISSUES_URL,
  RELEASES_URL,
  UPDATE_TONE_CLASS,
  useUpdate,
} from "@/lib/ui/update";

/** From the README's own description of the project, trimmed to one line. */
const TAGLINE = "A local language lab — dictation, reading, speaking and vocabulary, offline.";

const LICENSE_URL = "https://github.com/wsva/fms-app/blob/main/LICENSE";

/** `{git_describe, package_version}` from `app_build_version` in lib.rs. */
interface BuildVersion {
  git_describe: string;
  package_version: string;
}

function openExternal(url: string) {
  if (isTauri()) void openUrl(url).catch(() => {});
  else window.open(url, "_blank", "noopener,noreferrer");
}

/**
 * Put a string on the clipboard, and say whether it worked.
 *
 * `navigator.clipboard` needs a secure context, and a Tauri webview's origin is
 * not one on every platform — so the hidden-textarea fallback is not legacy
 * politeness, it is the path that works where the modern API refuses. Returns
 * false rather than throwing: the button reports "Copy failed" instead of the
 * version silently going nowhere.
 */
async function copyText(text: string): Promise<boolean> {
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

export default function AboutCard({
  active = false,
  onOpenLogs,
  className = "",
}: {
  /** Whether the card is on screen. Settings keeps every section mounted and
   *  hides them with a class, so a mount effect there would ask the manifest
   *  before anyone looked at About; the dialog is mounted when opened and passes
   *  this unconditionally. */
  active?: boolean;
  /** Jumps the shell to the Logs page. Omitted where the host cannot navigate
   *  (there is then no Logs link, rather than a dead one). */
  onOpenLogs?: () => void;
  /** Extra classes for the card's frame — the dialog centres it on a shadow. */
  className?: string;
}) {
  const update = useUpdate();
  const summary = describeUpdate(update);

  const [build, setBuild] = useState<BuildVersion | null>(null);
  const [copied, setCopied] = useState<"idle" | "ok" | "fail">("idle");
  const [notesOpen, setNotesOpen] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    invoke<BuildVersion>("app_build_version")
      .then(setBuild)
      .catch(() => {
        getVersion()
          .then((v) => setBuild({ git_describe: "", package_version: v }))
          .catch(() => {});
      });
  }, []);

  // Ask when the card comes into view. The question is "is there a new release
  // *now*", and the answer used to arrive whenever Settings happened to mount —
  // so a session that opened Settings at launch read a stale one hours later.
  // A minute of grace keeps Check for updates → About from asking twice.
  useEffect(() => {
    if (active) void checkForUpdates({ maxAgeMs: 60_000 });
  }, [active]);

  const version = build ? build.git_describe || build.package_version : "";
  const versionDetail =
    build && build.git_describe && build.package_version && build.git_describe !== build.package_version
      ? `Installed ${build.git_describe} (package ${build.package_version})`
      : version
        ? `Installed ${version}`
        : "Reading the build version…";

  async function copyVersion() {
    if (!version) return;
    const ok = await copyText(version);
    setCopied(ok ? "ok" : "fail");
    setTimeout(() => setCopied("idle"), 1600);
  }

  function runAction() {
    if (!summary.action) return;
    if (summary.action.kind === "install") void installUpdate();
    else if (summary.action.kind === "download") openExternal(update.status?.release_url || RELEASES_URL);
    else void checkForUpdates();
  }

  const notes = update.status?.notes?.trim() ?? "";
  const showNotes = Boolean(update.status?.available && notes);
  const percent = update.progress
    ? Math.min(
        100,
        Math.round(
          (update.progress.downloaded / Math.max(1, update.progress.total ?? update.progress.downloaded)) *
            100
        )
      )
    : 0;

  return (
    <div
      className={`w-full max-w-xl rounded-xl border border-border-default bg-bg-card p-4 ${className}`}
    >
      {/* Identity: what this is, and the build it is. */}
      <div className="flex items-start gap-3">
        <img src="/logo.png" alt="" draggable={false} className="h-9 w-9 shrink-0 rounded-lg" />
        <div className="min-w-0 flex-1">
          <p className="text-[1.05em] font-semibold leading-tight text-text-primary">FmS</p>
          <p className="mt-0.5 text-xs text-text-tertiary">{TAGLINE}</p>
        </div>
        <button
          type="button"
          onClick={() => void copyVersion()}
          disabled={!version}
          title={`${versionDetail} — click to copy`}
          className="shrink-0 cursor-pointer rounded-md border border-border-light px-2 py-1 font-mono text-xs text-text-secondary hover:bg-bg-hover disabled:cursor-default"
        >
          {copied === "ok" ? "Copied" : copied === "fail" ? "Copy failed" : version || "unknown"}
        </button>
      </div>

      {/* The whole update story, in one line, with the one thing to do about it. */}
      <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-border-light pt-3">
        <span className={`min-w-0 flex-1 text-sm ${UPDATE_TONE_CLASS[summary.tone]}`}>
          {summary.sentence}
        </span>
        {summary.action && (
          <button
            type="button"
            onClick={runAction}
            disabled={
              update.installing || (summary.action.kind === "check" && update.phase === "checking")
            }
            className={`shrink-0 rounded-md px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
              summary.action.kind === "check"
                ? "border border-border-light text-text-secondary hover:bg-bg-hover"
                : "bg-accent-bg text-white hover:bg-accent-bg-hover"
            }`}
          >
            {summary.action.label}
          </button>
        )}
      </div>

      {summary.detail && (
        <p className="mt-1.5 text-[11px] text-text-tertiary">{summary.detail}</p>
      )}

      {update.progress && update.installing && (
        <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-border-light">
          <div
            className="h-full rounded-full bg-accent-bg transition-[width] duration-200"
            style={{ width: `${percent}%` }}
          />
        </div>
      )}

      {update.installError && (
        <p className="mt-2 break-words text-xs text-red-500">{update.installError}</p>
      )}

      {/* Already fetched by the check, and already in the payload: a release
          notes link that leaves the app is a worse answer than the notes. */}
      {showNotes && (
        <div className="mt-2">
          <button
            type="button"
            onClick={() => setNotesOpen((v) => !v)}
            aria-expanded={notesOpen}
            className="flex cursor-pointer items-center gap-1 text-xs text-text-secondary hover:text-text-primary"
          >
            {notesOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            {`What's new in v${update.status?.latest ?? ""}`}
          </button>
          {notesOpen && (
            <div className="mt-1.5 max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md bg-bg-body p-2 text-xs leading-relaxed text-text-secondary">
              {notes}
            </div>
          )}
        </div>
      )}

      {/* The five things an About page is actually opened for. */}
      <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border-light pt-3 text-[11px]">
        <FooterLink onClick={() => openExternal(CARD_BASE_URL)}>Website & manual</FooterLink>
        <FooterDot />
        <FooterLink onClick={() => openExternal(RELEASES_URL)}>Releases</FooterLink>
        <FooterDot />
        <FooterLink onClick={() => openExternal(ISSUES_URL)}>Report an issue</FooterLink>
        {onOpenLogs && (
          <>
            <FooterDot />
            <FooterLink onClick={onOpenLogs}>Logs</FooterLink>
          </>
        )}
        <FooterDot />
        <FooterLink onClick={() => openExternal(LICENSE_URL)}>MIT license</FooterLink>
      </div>
    </div>
  );
}

function FooterLink({ onClick, children }: { onClick: () => void; children: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="cursor-pointer text-text-tertiary hover:text-text-primary hover:underline"
    >
      {children}
    </button>
  );
}

function FooterDot() {
  return <span className="text-text-tertiary opacity-40">·</span>;
}
