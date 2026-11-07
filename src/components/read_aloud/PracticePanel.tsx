"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Play, RotateCcw, Wand2 } from "lucide-react";
import type { ReadAloudSubmitResult, ReadAttempt, ReadStandardAudio, ReadText } from "@/lib/read_aloud/types";
import { GOOD_SCORE, PASS_SCORE, scoreBadgeClasses } from "@/lib/read_aloud/types";
import { useRecorder } from "@/components/books/useRecorder";
import { measureAttempt, measureAttemptOnHub, saveTake, type SegmentTiming, type WordTiming } from "@/lib/read_aloud/rerecognize";
import AsrDiff from "@/components/read_aloud/AsrDiff";
import AssessmentCards, { ModelScores } from "@/components/read_aloud/AssessmentCards";
import FeedbackPanel from "@/components/read_aloud/FeedbackPanel";
import ProgressOverlay from "@/components/read_aloud/ProgressOverlay";
import HubStatusOverlay, { type HubStage } from "@/components/read_aloud/HubStatusOverlay";
import { textReference } from "@/components/read_aloud/useAssessAgain";
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

/** The ended take, held between the save and the measurement. The attempt row is
 *  already on disk by the time this exists, so what is pending is only its
 *  numbers — never the recording. The WAV stays in memory so the hub route does
 *  not have to read back what was just written. */
type PendingTake = { attempt: ReadAttempt; wavBase64: string };

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
 * The take ends by being **saved**, not by being scored: the recording and its
 * row are written first, and the measurement is an update on top of them
 * (`saveTake` → `measureAttempt`, docs/design/hub-mcp-compute-offload.md §4.3).
 * Everything the measuring can go wrong with — a hub call that runs minutes and
 * gets abandoned, a model that will not load, an app that closes — therefore
 * leaves a saved attempt behind instead of losing the take.
 *
 * The recorder runs with the live preview on: the transcript being produced is
 * published upward with the rest of the recorder state, so the page can show it
 * while the reader is still speaking. The take still ends with the toolbar's
 * Stop, which transcribes the whole recording.
 */
export default function PracticePanel({ datasetUuid, text, standardAudio, showNormalized = false, fontCss = "", spaceCss = "", onScored, onRecorder }: Props) {
  const [result, setResult] = useState<ReadAloudSubmitResult | null>(null);
  // A measurement is being written onto the saved row.
  const [measuring, setMeasuring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // This workspace's role, read once: a hub scores locally with no choice (its
  // models are its own); a follower is asked where each take should be measured
  // — even with no hub configured, because the hub option's failure then names
  // the missing configuration, which is more useful than no dialog at all.
  const [role, setRole] = useState<string | null>(null);
  const [choiceOpen, setChoiceOpen] = useState(false);
  // The saved take, held while the choice or the hub call is in flight; one
  // measure path finishes it, whichever way it was scored.
  const [pending, setPending] = useState<PendingTake | null>(null);
  // A take that ended up saved without numbers — because the learner chose
  // "Not now", or because the measuring failed. The recording is safe either
  // way, so the panel says where its score can still come from.
  const [savedUnmeasured, setSavedUnmeasured] = useState(false);
  // The hub call's visible lifecycle: the status overlay stays open from the
  // choice tap through sending → waiting → complete (or the error with its
  // one-tap way back), and only then yields to the result card.
  const [hubStage, setHubStage] = useState<HubStage | null>(null);
  const [hubError, setHubError] = useState<string | null>(null);
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
    setPending(null);
    setSavedUnmeasured(false);
    setHubStage(null);
    setHubError(null);
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

  // The backend reports the one milestone the client cannot see from the
  // outside: the tool call leaving the handshake and going up to the hub.
  // Until that event lands, the overlay honestly says "sending".
  useEffect(() => {
    if (!isTauri() || hubStage !== "sending") return;
    const unlisten = listen<string>("hub-mcp-phase", (e) => {
      if (e.payload === "waiting") setHubStage((s) => (s === "sending" ? "waiting" : s));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [hubStage]);

  // One landing for both routes: the measurement has been written onto the row
  // the save wrote, so the panel shows its numbers and the page refreshes its
  // counts. XP belongs to this moment, not to the save — the number that earns it
  // is the one measured here.
  const applyResult = useCallback(
    (res: ReadAloudSubmitResult, via: string | null) => {
      const a = res.assessment;
      logInfo(
        `measured: score=${res.score.toFixed(1)} xp=+${res.xp_awarded} via=${via ?? "local"}` +
          ` recognized="${res.attempt.recognized}"` +
          (a
            ? ` fluency=${a.fluency ? `${Math.round(a.fluency.wcpm)} wcpm` : "not measured"}` +
              ` pron=${a.pronunciation.score != null ? Math.round(a.pronunciation.score * 100) : "not measured"}`
            : " no assessment"),
        LOG_MODULE
      );
      setResult(res);
      setHubScored(via === "hub");
      setSavedUnmeasured(false);
      setPending(null);
      onScored();
    },
    [onScored]
  );

  // The measure step every route ends in: write one transcript and the numbers
  // derived from it onto the saved attempt, in place. Errors travel to the
  // caller — the take is already saved, so a failed measurement only leaves it
  // waiting for "Assess again".
  const measureHere = useCallback(
    async (
      held: PendingTake,
      recognized: string,
      placement: { words?: WordTiming[] | null; segments?: SegmentTiming[] | null; model?: string | null }
    ) => {
      setMeasuring(true);
      setError(null);
      try {
        const res = await measureAttempt({
          datasetUuid,
          uuid: held.attempt.uuid,
          recognized,
          // The model's placement is what fluency is measured from — neither
          // list and that axis is reported as not measured.
          words: placement.words ?? null,
          segments: placement.segments ?? null,
          // Named, so this number is filed under the model that produced it and
          // an earlier, better measurement of the same take is not replaced.
          model: placement.model ?? null,
        });
        applyResult(res, null);
      } finally {
        setMeasuring(false);
      }
    },
    [datasetUuid, applyResult]
  );

  // The local route: recognize on this device (the hook deferred it), then
  // measure the saved row with what came back.
  const runLocal = useCallback(
    async (held: PendingTake) => {
      setChoiceOpen(false);
      setHubScored(false);
      setTranscribingLocal(true);
      try {
        const detailed = await recorderRef.current.transcribeTake(held.wavBase64);
        await measureHere(held, (detailed.text || "").trim(), {
          words: detailed.words ?? null,
          segments: detailed.segments ?? null,
          model: detailed.model ?? null,
        });
      } catch (e) {
        const msg = String(e);
        logError(`local scoring failed: ${msg}`, LOG_MODULE);
        setError(msg);
        setPending(null);
        setSavedUnmeasured(true);
      } finally {
        setTranscribingLocal(false);
      }
    },
    [measureHere]
  );

  // Bumped when a hub score is abandoned ("Not now") or fails: the reply of the
  // abandoned call must not land a measurement the learner walked away from.
  const hubCallToken = useRef(0);

  // The hub route (Variant B, docs/design/hub-mcp-compute-offload.md): the saved
  // take's WAV is posted to the hub's `read_aloud_assess_take`, and what comes
  // back is stored as measured — with `via` provenance journalled inside the
  // attempt's metrics. The hub persists nothing: this device stays the author,
  // and the take is already on disk, so a failed call means only that the
  // numbers wait for "Assess again".
  const runHub = useCallback(
    async (held: PendingTake) => {
      setChoiceOpen(false);
      setHubError(null);
      setHubStage("sending");
      setError(null);
      const token = ++hubCallToken.current;
      try {
        const res = await measureAttemptOnHub({
          datasetUuid,
          uuid: held.attempt.uuid,
          textUuid: text.uuid,
          wavBase64: held.wavBase64,
          // The phone's own reference, the same resolution order the hub applies
          // to its row — the fallback for a text not yet synced there.
          // `textReference` is that one rule, shared with "Assess again".
          content: textReference(text),
          milestones: {
            // The reply is in; the overlay says so while this device stores it.
            onReplied: () => setHubStage("done"),
            // The request outlives a "Not now", so the reply is dropped rather
            // than written over the choice the learner made instead.
            cancelled: () => token !== hubCallToken.current,
          },
        });
        if (!res) return; // walked away mid-call — nothing was stored
        applyResult(res, "hub");
        if (token === hubCallToken.current) setHubStage(null);
      } catch (e) {
        if (token !== hubCallToken.current) return;
        const msg = String(e);
        logError(`hub scoring failed: ${msg}`, LOG_MODULE);
        // The take stays as it is — saved and unmeasured. The overlay holds the
        // hub's error text and the one way out; nothing has to be rescued.
        setHubError(msg);
        setHubStage("error");
      }
    },
    [datasetUuid, text, applyResult]
  );

  // "Not now", from either dialog: stop waiting for numbers and leave the saved
  // attempt unmeasured. Bumping the token makes a hub reply that arrives
  // afterwards drop instead of overwriting the choice that was just made.
  const walkAway = useCallback(() => {
    hubCallToken.current += 1;
    setChoiceOpen(false);
    setHubStage(null);
    setHubError(null);
    setPending(null);
    setSavedUnmeasured(true);
  }, []);

  // The recorder's end-of-take. Saving comes first: from this line the take is on
  // disk and in the attempts list, so a cancelled hub call, a model that fails to
  // load or an app that closes mid-flight never loses a recording — only its
  // numbers, and those are one "Assess again" away. (Without `deferTranscribe`
  // the hook fills `text`/`words`/`segments` itself and never sets `captured` —
  // only the hub role could change mid-take, so handle both shapes.)
  const handleResult = useCallback(
    async ({
      wavBase64,
      text: recognized,
      words,
      segments,
      model,
      captured,
    }: {
      wavBase64: string;
      text: string;
      words?: WordTiming[];
      segments?: SegmentTiming[];
      model?: string | null;
      captured?: boolean;
    }) => {
      if (!isTauri()) return;
      setError(null);
      setSavedUnmeasured(false);
      // The transcript this device already has, if any: the measuring overwrites
      // it, so an empty one costs nothing but an empty row until then.
      const transcript = (recognized || "").trim();
      let saved: ReadAttempt;
      try {
        saved = await saveTake(datasetUuid, text.uuid, wavBase64, transcript);
      } catch (e) {
        const msg = String(e);
        logError(`save take failed: ${msg}`, LOG_MODULE);
        setError(msg);
        return;
      }
      const held = { attempt: saved, wavBase64 };
      setPending(held);
      // The attempt exists now, so best score and counts have already changed —
      // refreshed before anything measures it, since measuring may never return.
      onScored();
      if (role === "hub" || !captured) {
        // This device has the models and the transcript: measure it right away.
        try {
          await measureHere(held, transcript, {
            words: words ?? null,
            segments: segments ?? null,
            model: model ?? null,
          });
        } catch (e) {
          const msg = String(e);
          logError(`local scoring failed: ${msg}`, LOG_MODULE);
          setError(msg);
          setPending(null);
          setSavedUnmeasured(true);
        }
        return;
      }
      setChoiceOpen(true);
    },
    [datasetUuid, text.uuid, role, measureHere, onScored]
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
    measuring ||
    hubStage !== null ||
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
    for (const w of result?.assessment?.words ?? []) {
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

      {/* A take saved without numbers — walked away from, or a measurement that
          failed: the recording is safe, so say where its score can still come
          from instead of leaving a silent gap. */}
      {savedUnmeasured && !result && (
        <div className="p-3 rounded-lg bg-bg-muted text-text-secondary text-sm">
          Your recording is saved. Score it with “Assess again” on its attempt.
        </div>
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
          {/* Diff of transcript vs reference (rendered from the scoring
              alignment). Absent only when there was no reference to align
              against — the same case in which the attempt has no axes. */}
          {result.summary && (
            <div className="mt-1 p-2 rounded bg-bg-muted text-text-primary leading-relaxed whitespace-pre-wrap break-words">
              <AsrDiff summary={result.summary} pron={pronByRef} />
            </div>
          )}

          {/* The other two axes. Accuracy is the number above — unchanged in
              meaning and in what XP is awarded for. */}
          {result.assessment && (
            <div className="mt-1">
              <AssessmentCards assessment={result.assessment} />
            </div>
          )}

          {/* What this model measured next to what the others measured. The
              number above is this take's answer; the attempt's final score is
              the best of every model that has heard it, which is worth seeing
              when another engine heard something quite different. */}
          <ModelScores measurements={result.attempt.measurements} className="mt-1" />

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

      {/* After the take ends it is already saved, so what happens next only
          decides where its numbers come from. On a follower the ring yields to
          the scoring-choice dialog; once the hub is picked, the status overlay
          keeps that dialog's place for the whole call — sending, waiting,
          complete while the result is stored (a cold model load on the hub can
          run minutes behind "waiting"). Both dialogs have one exit, "Not now":
          the take is safe on disk either way and its row can be measured later. */}
      {choiceOpen && pending && (
        <ScoringChoiceDialog
          onHub={() => void runHub(pending)}
          onLocal={() => void runLocal(pending)}
          onLater={walkAway}
        />
      )}
      {(recorder.preparing || recorder.processing || transcribingLocal || (measuring && hubStage === null)) && (
        <ProgressOverlay
          label={
            recorder.preparing
              ? "Preparing…"
              : recorder.processing || transcribingLocal
              ? "Transcribing…"
              : "Scoring…"
          }
        />
      )}
      {hubStage !== null && (
        <HubStatusOverlay stage={hubStage} error={hubError} onLater={walkAway} />
      )}
    </div>
  );
}
