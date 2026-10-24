"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { Play, Trash2, History, ChevronRight, Loader2, Mic, RefreshCw, Award } from "lucide-react";
import type { ReadAttempt, ReadText } from "@/lib/read_aloud/types";
import { rerecognizeAttempt } from "@/lib/read_aloud/rerecognize";
import ConfirmDialog, { type ConfirmRequest } from "@/components/read_book/ConfirmDialog";
import AssessmentCards, { PauseRibbon, ScoreAxes } from "@/components/read_aloud/AssessmentCards";
import AssessmentLogPanel from "@/components/read_aloud/AssessmentLogPanel";
import { highlightDifferences } from "@/components/read_book/diff";
import { lcs } from "@/lib/listen/lcs";
import { isTauri } from "@/lib/tauri";
import { isMobileApp } from "@/lib/platform";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

type Props = {
  datasetUuid: string;
  text: ReadText;
  /** Called after a deletion so the caller can refresh best score / counts. */
  onChanged?: () => void;
  /** The attempt whose detail is shown, controlled by the page so the
   *  breadcrumb can reflect it as a level. null → show the list. */
  selectedUuid: string | null;
  onSelectUuid: (uuid: string | null) => void;
  /** Reports the resolved selected attempt (or null) upward so the page can
   *  label the attempt crumb. */
  onSelectAttempt?: (attempt: ReadAttempt | null) => void;
  /** Bumped by the page after it deletes the open attempt (the toolbar owns
   *  that action) so the list reloads. */
  refreshKey?: number;
  /** When true, the detail view compares the text and the recognized transcript
   *  sentence by sentence instead of showing the transcript alone. Toggled by
   *  the page toolbar's "Compare" button. */
  compare?: boolean;
};

function playUrl(url?: string | null) {
  if (!url) return;
  new Audio(convertFileSrc(url)).play().catch(() => {});
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

/** `m:ss` length of a take, from the duration its assessment recorded. */
function formatLength(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** Split text into sentences, keeping terminal punctuation with its sentence. */
function splitSentences(text: string): string[] {
  const cleaned = text.replace(/\s+/g, " ").trim();
  if (!cleaned) return [];
  const parts = cleaned.match(/[^.!?…]+[.!?…]+["'”)\]]*|[^.!?…]+$/g);
  return (parts ?? [cleaned]).map((s) => s.trim()).filter(Boolean);
}

// Leading/trailing punctuation stripped before comparing words, so `word` and
// `word,` tokenize alike. Only ASCII + common typographic marks are removed —
// letters (including accented ones, e.g. German umlauts) are preserved.
const LEAD_PUNCT = /^[.,!?;:"'“”„«»(\[…—–-]+/;
const TRAIL_PUNCT = /[.,!?;:"'“”„«»)\]…—–-]+$/;

/** Lowercased word tokens of a sentence, for the similarity metric. */
function wordsOf(sentence: string): string[] {
  return sentence
    .toLowerCase()
    .split(/\s+/)
    .map((w) => w.replace(LEAD_PUNCT, "").replace(TRAIL_PUNCT, ""))
    .filter((w) => w.length > 0);
}

/** Dice-style similarity in [0,1] from the longest common subsequence of words. */
function sentenceSim(a: string[], b: string[]): number {
  if (a.length === 0 && b.length === 0) return 1;
  if (a.length === 0 || b.length === 0) return 0;
  return (2 * lcs(a, b).length) / (a.length + b.length);
}

// Cost of leaving a sentence unmatched. With similarity in [0,1], a 0.5 penalty
// means a gap is only introduced when the counts differ (Needleman–Wunsch then
// places it to maximize total similarity); equal-count sequences pair up
// monotonically rather than gapping a merely-dissimilar pair.
const GAP_PENALTY = 0.5;

type AlignRow = { ref: string | null; rec: string | null };

/**
 * Align two sentence sequences with a Needleman–Wunsch global alignment scored by
 * sentence similarity. An inserted / dropped sentence becomes a gap instead of
 * shifting every following pair (the flaw of index pairing). The alignment is
 * monotonic — sentence order is preserved, no crossings.
 */
function alignSentences(refSents: string[], recSents: string[]): AlignRow[] {
  const n = refSents.length;
  const m = recSents.length;
  if (n === 0) return recSents.map((rec) => ({ ref: null, rec }));
  if (m === 0) return refSents.map((ref) => ({ ref, rec: null }));

  const ra = refSents.map(wordsOf);
  const rb = recSents.map(wordsOf);

  // F[i][j] = best score aligning the first i reference and first j recognized
  // sentences; P records the move (0 = diagonal, 1 = up → gap in recognized,
  // 2 = left → gap in reference).
  const F: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  const P: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = 1; i <= n; i++) {
    F[i][0] = F[i - 1][0] - GAP_PENALTY;
    P[i][0] = 1;
  }
  for (let j = 1; j <= m; j++) {
    F[0][j] = F[0][j - 1] - GAP_PENALTY;
    P[0][j] = 2;
  }
  for (let i = 1; i <= n; i++) {
    for (let j = 1; j <= m; j++) {
      const diag = F[i - 1][j - 1] + sentenceSim(ra[i - 1], rb[j - 1]);
      const up = F[i - 1][j] - GAP_PENALTY;
      const left = F[i][j - 1] - GAP_PENALTY;
      let best = diag;
      let move = 0;
      if (up > best) {
        best = up;
        move = 1;
      }
      if (left > best) {
        best = left;
        move = 2;
      }
      F[i][j] = best;
      P[i][j] = move;
    }
  }

  const rows: AlignRow[] = [];
  let i = n;
  let j = m;
  while (i > 0 || j > 0) {
    const move = P[i][j];
    if (i > 0 && j > 0 && move === 0) {
      rows.push({ ref: refSents[i - 1], rec: recSents[j - 1] });
      i--;
      j--;
    } else if (i > 0 && (move === 1 || j === 0)) {
      rows.push({ ref: refSents[i - 1], rec: null });
      i--;
    } else {
      rows.push({ ref: null, rec: recSents[j - 1] });
      j--;
    }
  }
  rows.reverse();
  return rows;
}

type SplitMethod = "punct" | "sat";

// The catalog's reference SaT model — the same one the Split Book stage and the
// Split Text page default to (textsplit::DEFAULT_MODEL_ID on the backend).
const SAT_MODEL_ID = "segment-any-text/sat-3l-sm";

const segActive = "px-2 py-0.5 text-xs rounded-md bg-accent text-white cursor-pointer";
const segIdle = "px-2 py-0.5 text-xs rounded-md text-text-secondary hover:bg-bg-hover cursor-pointer";
const segDisabled = "px-2 py-0.5 text-xs rounded-md text-text-tertiary opacity-50 cursor-not-allowed";

/**
 * Sentence-by-sentence comparison of the reference text against the recognized
 * transcript. The user picks how sentences are segmented: a fast punctuation
 * heuristic, or the SaT-3L-sm model (desktop only, needs the model downloaded).
 * The two sequences are then fuzzy-aligned (see alignSentences) rather than
 * paired by index; the recognized side carries the same red diff highlighting
 * the read-a-book sentence drawer uses. Side by side on a wide screen (PC),
 * stacked on narrow ones.
 */
function SentenceCompare({ reference, recognized }: { reference: string; recognized: string }) {
  const [method, setMethod] = useState<SplitMethod>("punct");
  // SaT splits are fetched lazily and cached for the current text pair.
  const [satRef, setSatRef] = useState<string[] | null>(null);
  const [satRec, setSatRec] = useState<string[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // `text_split` only exists in the desktop binary; hide the SaT choice elsewhere.
  const [mobile, setMobile] = useState(false);
  useEffect(() => setMobile(isMobileApp()), []);

  // A different attempt / text invalidates any cached SaT split.
  useEffect(() => {
    setSatRef(null);
    setSatRec(null);
    setError(null);
  }, [reference, recognized]);

  // Fetch the SaT split the first time that method is chosen for this text pair.
  useEffect(() => {
    if (method !== "sat" || (satRef && satRec)) return;
    if (!isTauri()) {
      setError("SaT splitting needs the desktop app.");
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const [r, s] = await Promise.all([
          invoke<string[]>("text_split", { text: reference, model: SAT_MODEL_ID }),
          invoke<string[]>("text_split", { text: recognized, model: SAT_MODEL_ID }),
        ]);
        if (cancelled) return;
        setSatRef(r.filter((x) => x.trim().length > 0));
        setSatRec(s.filter((x) => x.trim().length > 0));
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [method, reference, recognized, satRef, satRec]);

  // Prefer the SaT split once loaded; otherwise (punctuation method, still
  // loading, or errored) fall back to the heuristic so the view always renders.
  const useSat = method === "sat" && satRef != null && satRec != null;
  const refSentences = useSat ? satRef! : splitSentences(reference);
  const recSentences = useSat ? satRec! : splitSentences(recognized);
  const rows =
    refSentences.length === 0 && recSentences.length === 0
      ? []
      : alignSentences(refSentences, recSentences);

  return (
    <div className="flex flex-col gap-2 min-w-0">
      {/* Segmenter choice */}
      <div className="flex items-center gap-1.5 flex-wrap">
        <span className="text-xs text-text-tertiary">Split by</span>
        <button
          type="button"
          className={method === "punct" ? segActive : segIdle}
          onClick={() => setMethod("punct")}
          title="Fast punctuation heuristic"
        >
          Punctuation
        </button>
        <button
          type="button"
          className={mobile ? segDisabled : method === "sat" ? segActive : segIdle}
          onClick={() => !mobile && setMethod("sat")}
          disabled={mobile}
          title={mobile ? "Desktop only" : "SaT-3L-sm model (download it on the Models page)"}
        >
          SaT model
        </button>
        {loading && (
          <span className="flex items-center gap-1 text-xs text-text-tertiary">
            <Loader2 size={12} className="animate-spin" /> Splitting…
          </span>
        )}
      </div>

      {error && (
        <div className="p-2 rounded-lg bg-error-bg text-error-text text-xs break-words">{error}</div>
      )}

      {rows.length === 0 ? (
        <p className="text-xs text-text-tertiary">Nothing to compare.</p>
      ) : (
        <>
          <div className="hidden lg:grid lg:grid-cols-2 gap-2 min-w-0">
            <h3 className="text-xs font-medium text-text-secondary">Text</h3>
            <h3 className="text-xs font-medium text-text-secondary">Recognized</h3>
          </div>
          {rows.map((row, i) => (
            <div key={i} className="grid grid-cols-1 lg:grid-cols-2 gap-2 min-w-0">
              <div className="p-3 rounded-lg bg-bg-muted text-text-primary text-base leading-relaxed whitespace-pre-wrap break-words">
                {row.ref || <span className="text-text-tertiary">—</span>}
              </div>
              <div className="p-3 rounded-lg bg-bg-muted text-text-primary text-base leading-relaxed break-words">
                {row.rec != null ? (
                  highlightDifferences(row.ref ?? "", row.rec)
                ) : (
                  <span className="text-text-tertiary">—</span>
                )}
              </div>
            </div>
          ))}
        </>
      )}
    </div>
  );
}

/**
 * One recorded take, as a card.
 *
 * Its three scores are the card's subject — accuracy, fluency and pronunciation
 * side by side — so the history can be read down the list without opening a
 * take. The transcript is a two-line excerpt (the detail view is one click
 * away) and the recording's own actions sit in the header beside it.
 */
function AttemptCard({
  attempt,
  best,
  busy,
  blocked,
  onOpen,
  onPlay,
  onAssess,
  onDelete,
}: {
  attempt: ReadAttempt;
  /** Highest accuracy in this text's history — the row the others aim at. */
  best: boolean;
  /** This take's assessment is the one running. */
  busy: boolean;
  /** Any take's assessment is running, so this row's action stands aside. */
  blocked: boolean;
  onOpen: () => void;
  onPlay?: () => void;
  onAssess?: () => void;
  onDelete: () => void;
}) {
  const length = attempt.metrics?.fluency?.duration;

  return (
    <li
      onClick={onOpen}
      className="flex flex-col gap-2 p-2.5 rounded-xl border border-border-default bg-bg-card hover:bg-bg-hover cursor-pointer transition-colors min-w-0"
    >
      <div className="flex items-center gap-1.5 min-w-0">
        <span className="text-[11px] text-text-tertiary tabular-nums truncate">
          {formatWhen(attempt.created_at)}
        </span>
        {length != null && length > 0 && (
          <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-tertiary tabular-nums shrink-0">
            {formatLength(length)}
          </span>
        )}
        {best && (
          <span
            className="flex items-center gap-0.5 text-[10px] px-1.5 py-0.5 rounded-full bg-success-bg text-success-text font-semibold shrink-0"
            title="Highest accuracy among this text's takes"
          >
            <Award size={11} />
            best
          </span>
        )}
        <div className="flex-1 min-w-0" />
        {onPlay && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onPlay();
            }}
            title="Play recording"
            className="p-1.5 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-muted cursor-pointer shrink-0"
          >
            <Play size={15} />
          </button>
        )}
        {onAssess && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              onAssess();
            }}
            disabled={blocked}
            title="Assess again (re-transcribe this recording and score it again)"
            className="p-1.5 rounded-md text-text-tertiary hover:text-text-primary hover:bg-bg-muted cursor-pointer shrink-0 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {busy ? <Loader2 size={15} className="animate-spin" /> : <RefreshCw size={15} />}
          </button>
        )}
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          title="Delete attempt"
          className="p-1.5 rounded-md text-text-tertiary hover:text-error-text hover:bg-bg-muted cursor-pointer shrink-0"
        >
          <Trash2 size={15} />
        </button>
        <ChevronRight size={16} className="text-text-tertiary shrink-0" />
      </div>

      {/* The point of the card: all three scores, whatever was measurable. */}
      <ScoreAxes score={attempt.score} metrics={attempt.metrics} />

      <p
        className="text-xs text-text-secondary break-words line-clamp-2"
        title={attempt.recognized}
      >
        {attempt.recognized || "(no speech recognized)"}
      </p>
    </li>
  );
}

/**
 * A dedicated page listing every recorded attempt at one text. Each card shows
 * the take's three scores; clicking it opens the detail — the full recognized
 * text at reading size, plus the stored three-axis assessment in depth. Reached
 * from the practice view's toolbar ("Attempts").
 */
export default function AttemptsPanel({
  datasetUuid,
  text,
  onChanged,
  selectedUuid,
  onSelectUuid,
  onSelectAttempt,
  refreshKey,
  compare,
}: Props) {
  const [attempts, setAttempts] = useState<ReadAttempt[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  // The attempt currently being re-recognized (its uuid), so the row can show a
  // spinner and other rows disable while the model runs.
  const [rerecognizing, setRerecognizing] = useState<string | null>(null);

  const loadAttempts = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const rows = await invoke<ReadAttempt[]>("read_aloud_list_attempts", {
        datasetUuid,
        textUuid: text.uuid,
      });
      setAttempts(rows);
    } catch (e) {
      logError(`list attempts failed: ${String(e)}`, LOG_MODULE);
    }
  }, [datasetUuid, text.uuid]);

  useEffect(() => {
    setError(null);
    loadAttempts();
  }, [loadAttempts, refreshKey]);

  const deleteAttempt = (a: ReadAttempt) => {
    setConfirm({
      title: "Delete attempt",
      message: "Delete this recording and its score? This cannot be undone.",
      confirmLabel: "Delete",
      onConfirm: async () => {
        try {
          await invoke("read_aloud_delete_attempt", { datasetUuid, uuid: a.uuid });
          // Leaving the detail panel if the attempt being viewed was deleted.
          if (selectedUuid === a.uuid) onSelectUuid(null);
          await loadAttempts();
          onChanged?.();
        } catch (e) {
          setError(String(e));
        }
      },
    });
  };

  // Re-run STT over an existing recording and refresh its transcript + score in
  // place. The recording itself is untouched — only the recognized text changes.
  // The WAV is device-local, so this is only offered when a.audio_url resolves on
  // this machine. The work itself lives in `rerecognizeAttempt`, shared with the
  // attempt page's toolbar button; it also feeds the live assessment log.
  const assessAgain = async (a: ReadAttempt) => {
    if (!isTauri() || !a.audio_url) return;
    setRerecognizing(a.uuid);
    setError(null);
    try {
      await rerecognizeAttempt(datasetUuid, a);
      await loadAttempts();
      onChanged?.();
    } catch (e) {
      const msg = String(e);
      logError(`assess again failed: ${msg}`, LOG_MODULE);
      setError(msg);
    } finally {
      setRerecognizing(null);
    }
  };

  const selectedAttempt = attempts.find((a) => a.uuid === selectedUuid) ?? null;
  // Best accuracy in the list rather than `text.best_score`: the rows are
  // re-scored against the current reference on read, so the stored best can lag
  // behind the numbers the cards show.
  const bestScore = attempts.length > 0 ? Math.max(...attempts.map((a) => a.score)) : null;
  // Scoring — and the diff below — uses the normalized reference when set, else
  // the text to read. Mirror that so the highlighting matches the score.
  const reference = text.normalized_content?.trim() ? text.normalized_content : text.content;

  // Keep the page's breadcrumb crumb in sync with the resolved attempt.
  useEffect(() => {
    onSelectAttempt?.(selectedAttempt);
  }, [selectedAttempt, onSelectAttempt]);

  return (
    // The live log is docked under the scroll area rather than inside it, so a
    // long assessment stays visible while the attempt itself is read.
    <div className="flex-1 min-h-0 min-w-0 flex flex-col">
      {/* Same bottom room as the practice pane - the attempt detail and the compare
          view are read too, and end on a sentence rather than at the edge. */}
      <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 pb-[50vh] flex flex-col gap-3 min-w-0">
        {selectedAttempt ? (
          <>
            {/* Detail panel — the recognized text at normal reading size.
                Navigation back to the list is via the breadcrumb's "Attempts" crumb;
                Play / Delete for the open attempt live in the page toolbar. */}
            <div className="flex items-center gap-2 min-w-0">
              <span className="text-xs text-text-tertiary tabular-nums">
                {formatWhen(selectedAttempt.created_at)}
              </span>
              {selectedAttempt.metrics?.fluency?.duration != null && (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-bg-muted text-text-tertiary tabular-nums">
                  {formatLength(selectedAttempt.metrics.fluency.duration)}
                </span>
              )}
            </div>

            {/* The take's scores. With a stored assessment the cards carry the
                whole picture — accuracy in depth, then fluency and pronunciation —
                so nothing is repeated above them. A take recorded before
                assessment existed has no cards, and then the same three-score
                strip the list uses says what it has and what is missing. */}
            {selectedAttempt.metrics ? (
              <section className="flex flex-col gap-2 min-w-0">
                <AssessmentCards assessment={selectedAttempt.metrics} />
                <PauseRibbon
                  pauses={selectedAttempt.metrics.pauses}
                  duration={
                    selectedAttempt.metrics.fluency?.duration ??
                    selectedAttempt.metrics.pauses[selectedAttempt.metrics.pauses.length - 1]?.end ??
                    0
                  }
                />
              </section>
            ) : (
              <ScoreAxes score={selectedAttempt.score} metrics={null} size="md" />
            )}

            {/* Compare mode (toolbar "Compare") pairs the texts sentence by
                sentence; otherwise just show the recognized transcript. */}
            {compare ? (
              <SentenceCompare reference={reference} recognized={selectedAttempt.recognized} />
            ) : (
              <section className="flex flex-col gap-1 min-w-0">
                <h3 className="text-xs font-medium text-text-secondary">Recognized text</h3>
                <div className="p-4 rounded-xl bg-bg-muted text-text-primary text-base leading-relaxed whitespace-pre-wrap break-words">
                  {selectedAttempt.recognized || "(no speech recognized)"}
                </div>
              </section>
            )}
          </>
        ) : (
          <>
            <section className="flex flex-col gap-0.5 min-w-0">
              <h2 className="text-lg font-semibold text-text-primary break-words flex items-center gap-2">
                <History size={18} className="text-text-tertiary shrink-0" />
                Attempts
              </h2>
              <p className="text-xs text-text-tertiary break-words">
                {text.title || "Untitled"} · {attempts.length} attempt
                {attempts.length === 1 ? "" : "s"}
                {bestScore != null && ` · best ${Math.round(bestScore)}%`}
              </p>
            </section>

            {attempts.length === 0 ? (
              <div className="flex flex-col items-center gap-1.5 p-8 rounded-xl border border-dashed border-border-default text-center">
                <Mic size={20} className="text-text-tertiary" />
                <p className="text-sm text-text-secondary">No attempts yet</p>
                <p className="text-xs text-text-tertiary max-w-sm break-words">
                  Record yourself reading in the practice view. Every take is scored on
                  accuracy, fluency and pronunciation, and its three scores line up here.
                </p>
              </div>
            ) : (
              <ul className="flex flex-col gap-2">
                {attempts.map((a) => (
                  <AttemptCard
                    key={a.uuid}
                    attempt={a}
                    best={attempts.length > 1 && a.score === bestScore}
                    busy={rerecognizing === a.uuid}
                    blocked={rerecognizing !== null}
                    onOpen={() => onSelectUuid(a.uuid)}
                    onPlay={a.audio_url ? () => playUrl(a.audio_url) : undefined}
                    onAssess={a.audio_url ? () => void assessAgain(a) : undefined}
                    onDelete={() => deleteAttempt(a)}
                  />
                ))}
              </ul>
            )}
          </>
        )}

        {error && (
          <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words">{error}</div>
        )}

        <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
      </div>

      {/* Live log of the running assessment: appears with the first run, tails
          the backend model log while it works, and stays as the run's record. */}
      <AssessmentLogPanel />
    </div>
  );
}
