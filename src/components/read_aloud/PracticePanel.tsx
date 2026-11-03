"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Play, RotateCcw, Wand2 } from "lucide-react";
import type { HubAssessResult, ReadAloudSubmitResult, ReadAttempt, ReadStandardAudio, ReadText } from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE, scoreBadgeClasses } from "@/lib/read_aloud/types";
import { useRecorder } from "@/components/books/useRecorder";
import AsrDiff from "@/components/read_aloud/AsrDiff";
import AssessmentCards from "@/components/read_aloud/AssessmentCards";
import FeedbackPanel from "@/components/read_aloud/FeedbackPanel";
import ProgressOverlay from "@/components/read_aloud/ProgressOverlay";
import ScoringChoiceDialog from "@/components/read_aloud/ScoringChoiceDialog";
import StandardAudioPlayer from "@/components/read_aloud/StandardAudioPlayer";
import { isTauri } from "@/lib/tauri";
import { acquireAudioSrc, releaseAudioSrc } from "@/lib/media/audioSrc";
import { logError, logInfo } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

/** Recorder control surfaced to the page so the Record button and the live
 *  transcript dock can live in the toolbar / page frame while the recording and
 *  submit logic stay here. */
export type RecorderCtl = {
  toggle: () => void;
  recording: boolean;
  busy: boolean;
  /** Record was pressed; the microphone (and, on a hub, the model) is opening. */
  preparing: boolean;
  /** The transcript recognized so far, while `recording` is true. */
  liveText: string;
  /** A stretch of the take is being recognized right now. */
  liveBusy: boolean;
  /** Seconds recorded so far. */
  liveSeconds: number;
};

type Props = {
  datasetUuid: string;
  text: ReadText;
  /** Show the LLM-normalized reference instead of the raw text (view toggle
   *  driven from the page toolbar's overflow menu). */
  showNormalized?: boolean;
  /** Every clip this text owns; the panel plays the default one above the
   *  text. Loaded by the page for its list badges, so it is handed down
   *  rather than fetched again. */
  standardAudio?: ReadStandardAudio[];
  /** `font-family` for the text to read - one of the Playwrite DE school
   *  scripts, or empty for the interface font. See src/lib/read_aloud/fonts.ts. */
  fontCss?: string;
  /** `word-spacing` added between its words - one or two of the font's own
   *  spaces, or empty for the spacing the font itself has. */
  spaceCss?: string;
  /** Called after a submission so the caller can refresh best score / counts. */
  onScored: () => void;
  /** Publishes recorder status/toggle upward so the toolbar can render the
   *  Record button; called with null on unmount. */
  onRecorder?: (ctl: RecorderCtl | null) => void;
};

async function playUrl(url?: string | null) {
  if (!url) return;
  const src = await acquireAudioSrc(url);
  const el = new Audio(src);
  // The fire-and-forget player keeps no ref, so release the blob as soon as
  // the take is done (or the play fails) rather than leaking it.
  el.onended = () => releaseAudioSrc(src);
  el.play().catch(() => releaseAudioSrc(src));
}

/**
 * Practice panel for a single text: record yourself reading it, transcribe via
 * STT, score against the reference, and review past attempts.
 *
 * The recorder runs with the live preview on: the transcript being produced is
 * published upward with the rest of the recorder state, so the page can show it
 * while the reader is still speaking. The take still ends with the toolbar's
 * Stop, which transcribes the whole recording.
 */
export default function PracticePanel({ datasetUuid, text, standardAudio, showNormalized = false, fontCss = "", spaceCss = "", onScored, onRecorder }: Props) {
  const [result, setResult] = useState<ReadAloudSubmitResult | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // This workspace's role, read once: a hub scores locally with no choice (its
  // models are its own); a follower is asked where each take should be measured
  // — even with no hub configured, because the hub option's failure then names
  // the missing configuration, which is more useful than no dialog at all.
  const [role, setRole] = useState<string | null>(null);
  const [choiceOpen, setChoiceOpen] = useState(false);
  // The ended take, held while the choice or the hub call is in flight; one
  // submit path finishes it, whichever way it was scored.
  const [pendingWav, setPendingWav] = useState<string | null>(null);
  const [scoringRemote, setScoringRemote] = useState(false);
  // The local route runs `transcribeTake` outside the hook (the hook deferred
  // it), so the "Transcribing…" phase belongs to the panel here.
  const [transcribingLocal, setTranscribingLocal] = useState(false);
  // Provenance of the on-screen result: the hub line is shown only for a take
  // the hub actually measured (the attempt's stored metrics carry the same
  // `provenance.via` key for every device that reads the row later).
  const [hubScored, setHubScored] = useState(false);

  // Reset transient state whenever the selected text changes.
  useEffect(() => {
    setResult(null);
    setError(null);
    setChoiceOpen(false);
    setPendingWav(null);
    setScoringRemote(false);
    setTranscribingLocal(false);
    setHubScored(false);
  }, [text.uuid]);

  useEffect(() => {
    if (!isTauri()) {
      setRole("hub"); // browser dev shell: everything is local here
      return;
    }
    invoke<{ role: string }>("settings_get_workspace")
      .then((w) => setRole(w.role))
      .catch((e) => {
        logError(`role lookup failed: ${String(e)} — treating this device as a hub`, LOG_MODULE);
        setRole("hub");
      });
  }, []);

  // A delegated feedback report is written back out of band by an attached
  // agent (via `read_aloud_save_feedback`, which emits this event). When it
  // names the take on screen, refetch that attempt so its stored report feeds
  // the card — the just-finished `result` would otherwise never gain a report.
  useEffect(() => {
    if (!isTauri()) return;
    const uuid = result?.attempt.uuid;
    if (!uuid) return;
    const unlisten = listen<{ attempt_uuid?: string }>("read-aloud-feedback-saved", async (e) => {
      if (e.payload?.attempt_uuid && e.payload.attempt_uuid !== uuid) return;
      try {
        const rows = await invoke<ReadAttempt[]>("read_aloud_list_attempts", {
          datasetUuid,
          textUuid: text.uuid,
        });
        const fresh = rows.find((r) => r.uuid === uuid);
        if (fresh) setResult((prev) => (prev ? { ...prev, attempt: fresh } : prev));
      } catch (err) {
        logError(`feedback reload failed: ${String(err)}`, LOG_MODULE);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [result?.attempt.uuid, datasetUuid, text.uuid]);

  // One finish for both routes: persist, score-land, XP. A passed assessment
  // was measured on the hub and is stored as measured (with `via` provenance
  // journalled inside the attempt's metrics); absent, this device measures it.
  const finishSubmit = useCallback(
    async (payload: {
      wavBase64: string;
      recognized: string;
      words?: { text: string; start: number; end: number }[] | null;
      assessment?: unknown;
      via?: string;
    }) => {
      setSubmitting(true);
      setError(null);
      try {
        const res = await invoke<ReadAloudSubmitResult>("read_aloud_submit", {
          datasetUuid,
          textUuid: text.uuid,
          wavBase64: payload.wavBase64,
          recognized: payload.recognized,
          // Word timings (when a model reported them) are what fluency is
          // measured from — without them that axis is reported as not measured.
          words: payload.words ?? null,
          assessment: payload.assessment ?? null,
          via: payload.via ?? null,
        });
        const a = res.assessment;
        logInfo(
          `submit: score=${res.score.toFixed(1)} xp=+${res.xp_awarded} via=${payload.via ?? "local"}` +
            ` recognized="${payload.recognized}"` +
            ` fluency=${a.fluency ? `${Math.round(a.fluency.wcpm)} wcpm` : "not measured"}` +
            ` pron=${a.pronunciation.score != null ? Math.round(a.pronunciation.score * 100) : "not measured"}`,
          LOG_MODULE
        );
        setResult(res);
        setHubScored(payload.via === "hub");
        onScored();
      } catch (e) {
        const msg = String(e);
        logError(`submit failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setSubmitting(false);
        setPendingWav(null);
      }
    },
    [datasetUuid, text.uuid, onScored]
  );

  // The local route: recognize on this device (the hook deferred it), then the
  // shared submit — today's exact path, one command after the model call.
  const runLocal = useCallback(
    async (wavBase64: string) => {
      setChoiceOpen(false);
      setHubScored(false);
      setTranscribingLocal(true);
      try {
        const detailed = await recorderRef.current.transcribeTake(wavBase64);
        await finishSubmit({
          wavBase64,
          recognized: (detailed.text || "").trim(),
          words: detailed.words ?? null,
        });
      } catch (e) {
        setChoiceOpen(false);
        setPendingWav(null);
        const msg = String(e);
        logError(`local scoring failed: ${msg}`, LOG_MODULE);
        setError(msg);
      } finally {
        setTranscribingLocal(false);
      }
    },
    [finishSubmit]
  );

  // Bumped when a hub score is abandoned (cancel) or fails: the reply of the
  // abandoned call must not land an attempt the learner walked away from.
  const hubCallToken = useRef(0);

  // The hub route (Variant B, docs/design/hub-mcp-compute-offload.md): post the
  // WAV to the hub's MCP, get the measured take back, persist it here. The hub
  // persists nothing — this device stays the author. A failure names the next
  // step and re-opens the choice, so "This device" is one tap away.
  const runHub = useCallback(
    async (wavBase64: string) => {
      setChoiceOpen(false);
      setScoringRemote(true);
      setError(null);
      const token = ++hubCallToken.current;
      try {
        const reply = await invoke<HubAssessResult>("hub_mcp_call", {
          tool: "read_aloud_assess_take",
          args: {
            dataset_uuid: datasetUuid,
            text_uuid: text.uuid,
            wav_base64: wavBase64,
            // The phone's own reference, the same resolution order the hub
            // applies to its row — the fallback for a text not yet synced here.
            content: text.normalized_content ?? text.content,
          },
        });
        if (token !== hubCallToken.current) return; // cancelled while in flight
        await finishSubmit({
          wavBase64,
          recognized: reply.recognized,
          words: reply.words ?? null,
          assessment: reply.assessment,
          via: "hub",
        });
      } catch (e) {
        if (token !== hubCallToken.current) return;
        const msg = String(e);
        logError(`hub scoring failed: ${msg}`, LOG_MODULE);
        setError(msg);
        setPendingWav(null);
        setChoiceOpen(true);
      } finally {
        if (token === hubCallToken.current) setScoringRemote(false);
      }
    },
    [datasetUuid, text, finishSubmit]
  );

  // The recorder's deferred end-of-take: a hub role just scores locally; a
  // follower is asked. (Without `deferTranscribe` the hook fills `text`/`words`
  // itself and never sets `captured` — only the hub role could change mid-take,
  // so handle both shapes.)
  const handleResult = useCallback(
    async ({
      wavBase64,
      text: recognized,
      words,
      captured,
    }: {
      wavBase64: string;
      text: string;
      words?: { text: string; start: number; end: number }[];
      captured?: boolean;
    }) => {
      if (!isTauri()) return;
      setError(null);
      if (role === "hub" || !captured) {
        await finishSubmit({ wavBase64, recognized, words: words ?? null });
        return;
      }
      setPendingWav(wavBase64);
      setChoiceOpen(true);
    },
    [role, finishSubmit]
  );

  // The role decides the recorder's shape before the first render that could
  // press Record: a follower sheds the on-device model (`preloadModel: false`
  // also closes the live preview, which would auto-load it) and ends its take
  // at the WAV; the hub pre-loads and transcribes as it always has.
  const recorder = useRecorder(handleResult, (msg) => setError(msg), {
    live: true,
    deferTranscribe: role !== "hub",
    preloadModel: role === "hub",
  });
  // `runLocal` needs the hook's `transcribeTake` but the hook is built after
  // the routes; this ref closes the cycle without rebuilding either.
  const recorderRef = useRef(recorder);
  recorderRef.current = recorder;

  const busy =
    recorder.preparing ||
    recorder.processing ||
    transcribingLocal ||
    submitting ||
    scoringRemote ||
    choiceOpen;
  const score = result?.score ?? null;

  // The clip scoring would read: the default one, or — if its wav is not on
  // this device — any other clip that can at least be listened to.
  const defaultClip = useMemo(
    () =>
      standardAudio?.find((c) => c.is_default && c.audio_url) ??
      standardAudio?.find((c) => c.audio_url) ??
      null,
    [standardAudio]
  );

  // Reference-word index → GOP score, so the diff can underline words that were
  // heard correctly but pronounced weakly. Only words that were measured appear.
  const pronByRef = useMemo(() => {
    const map: Record<number, number> = {};
    for (const w of result?.assessment.words ?? []) {
      if (w.ref_index != null && w.pron != null) map[w.ref_index] = w.pron;
    }
    return Object.keys(map).length > 0 ? map : undefined;
  }, [result]);

  // Surface the recorder to the page frame: publish the latest toggle/status and
  // live preview whenever they change, and clear it on unmount so the Record
  // button disables and the dock closes. Nothing is published until the role
  // resolves: a fast first click would otherwise pick the wrong `preloadModel`.
  useEffect(() => {
    if (role === null) return;
    onRecorder?.({
      toggle: recorder.toggle,
      recording: recorder.recording,
      busy,
      preparing: recorder.preparing,
      liveText: recorder.liveText,
      liveBusy: recorder.liveBusy,
      liveSeconds: recorder.liveSeconds,
    });
  }, [
    onRecorder,
    role,
    recorder.toggle,
    recorder.recording,
    recorder.preparing,
    recorder.liveText,
    recorder.liveBusy,
    recorder.liveSeconds,
    busy,
  ]);

  useEffect(() => () => onRecorder?.(null), [onRecorder]);

  // The pb-[50vh] on the scroll root is what lets the last line be scrolled up to
  // the middle of the screen: this is the text you read aloud, so it should sit in
  // front of you while you speak instead of stopping at the bottom edge. The same
  // room the wiki reader and ReadingView give their prose. The live transcript is
  // published upward with the rest of the recorder state, so the page can dock it
  // under this pane where it stays put while the reader scrolls.
  return (
    <div className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden p-4 pb-[50vh] flex flex-col gap-4 min-w-0">
      {/* Reference text */}
      <section className="flex flex-col gap-1 min-w-0">
        <h2 className="text-lg font-semibold text-text-primary break-words">{text.title || "Untitled"}</h2>
        {text.note && (
          <p className="text-xs text-text-tertiary break-words">{text.note}</p>
        )}
        {showNormalized && text.normalized_content && (
          <p className="text-[11px] text-text-tertiary flex items-center gap-1">
            <Wand2 size={12} className="text-accent-bg" /> Showing the normalized reference used for
            scoring
          </p>
        )}
        {/* The reference heard before it is read: playing it is learning the
            pacing, so it sits above the text rather than behind Manage. */}
        {defaultClip && (
          <StandardAudioPlayer key={defaultClip.uuid} clip={defaultClip} disabled={recorder.recording} />
        )}
        {/* The one line the script and the spacing apply to: this is the text
            being read aloud, and nothing else on the page is meant to look
            like handwriting. */}
        <div
          className="ra-text mt-2 p-4 rounded-xl bg-bg-muted text-text-primary text-lg leading-relaxed whitespace-pre-wrap break-words"
          style={{ "--ra-font": fontCss || "inherit", "--ra-space": spaceCss || "normal" } as CSSProperties}
        >
          {showNormalized && text.normalized_content ? text.normalized_content : text.content}
        </div>
      </section>

      {/* Recorder status — the Record button itself lives in the page toolbar.
          Nothing here while recording (the live dock already says what is
          happening); once the take ends the overlay below takes over. */}

      {error && (
        <div className="p-3 rounded-lg bg-error-bg text-error-text text-sm break-words">{error}</div>
      )}

      {/* Latest result */}
      {result && (
        <section className="flex flex-col gap-2 p-4 rounded-xl border border-border-default bg-bg-card min-w-0">
          <div className="flex items-center gap-3">
            <span className={`text-2xl font-bold tabular-nums ${scoreBadgeClasses(score)}`}>
              {score != null ? `${Math.round(score)}%` : "—"}
            </span>
            <span className="text-sm text-text-secondary">
              {score != null && score >= GOOD_SCORE
                ? "Excellent read!"
                : score != null && score >= PASS_SCORE
                ? "Passed — nice work."
                : "Below 60% — give it another try."}
            </span>
            {hubScored && (
              <span className="text-xs text-text-tertiary">Scored on the Hub</span>
            )}
            {result.xp_awarded > 0 && (
              <span className="ml-auto text-sm font-semibold text-yellow-500">
                +{result.xp_awarded} XP
              </span>
            )}
          </div>
          <div className="text-xs text-text-tertiary">
            XP: +1 at {PASS_SCORE}% or more, +1 more at {GOOD_SCORE}% or more (once per text).
          </div>
          {/* Diff of transcript vs reference (rendered from the scoring alignment) */}
          <div className="mt-1 p-2 rounded bg-bg-muted text-text-primary leading-relaxed whitespace-pre-wrap break-words">
            <AsrDiff summary={result.summary} pron={pronByRef} />
          </div>

          {/* The other two axes. Accuracy is the number above — unchanged in
              meaning and in what XP is awarded for. */}
          <div className="mt-1">
            <AssessmentCards assessment={result.assessment} />
          </div>

          {/* The numbers above say *which* words were wrong; this says what to
              do about them. Asked for explicitly — a low score is exactly when
              the explanation is worth a model call, and nothing is generated
              for a take nobody asked about. */}
          <div className="mt-1">
            <FeedbackPanel
              datasetUuid={datasetUuid}
              attemptUuid={result.attempt.uuid}
              textUuid={text.uuid}
              stored={result.attempt.feedback ?? null}
              measured={result.attempt.metrics != null}
              audioPath={result.attempt.audio_url ?? null}
            />
          </div>
          {result.attempt.audio_url && (
            <button
              onClick={() => playUrl(result.attempt.audio_url)}
              className="mt-1 flex items-center gap-1.5 self-start text-xs text-text-secondary hover:text-text-primary cursor-pointer"
            >
              <Play size={14} /> Play your recording
            </button>
          )}
          <button
            onClick={() => setResult(null)}
            className="mt-1 flex items-center gap-1.5 self-start text-xs text-text-tertiary hover:text-text-primary cursor-pointer"
          >
            <RotateCcw size={13} /> Dismiss result
          </button>
        </section>
      )}

      {/* Attempt history lives on its own page now — reached from the toolbar's
          overflow menu ("Show Attempts"). */}

      {/* After the take ends, transcribing and scoring happen behind a modal
          ring — dimming the window is deliberate: there is nothing else to look
          at until the score is there. On a follower the ring yields first to
          the scoring-choice dialog; while the hub measures the take, the ring
          offers the way back to a local score (a cold model load there can run
          minutes). */}
      {choiceOpen && pendingWav !== null && (
        <ScoringChoiceDialog
          onHub={() => void runHub(pendingWav)}
          onLocal={() => void runLocal(pendingWav)}
          onDiscard={() => {
            setChoiceOpen(false);
            setPendingWav(null);
          }}
        />
      )}
      {(recorder.preparing || recorder.processing || transcribingLocal || submitting || scoringRemote) && (
        <ProgressOverlay
          label={
            recorder.preparing
              ? "Preparing…"
              : recorder.processing || transcribingLocal
              ? "Transcribing…"
              : scoringRemote
              ? "Scoring on the hub…"
              : "Scoring…"
          }
          onCancel={scoringRemote ? () => {
            // The abandoned call persists nothing on the hub, so walking away
            // mid-flight cannot strand anything: the dialog simply re-asks.
            // Bumping the token makes the late reply drop on the floor.
            hubCallToken.current += 1;
            setScoringRemote(false);
            setChoiceOpen(true);
          } : undefined}
          cancelLabel="On this device instead"
        />
      )}
    </div>
  );
}
