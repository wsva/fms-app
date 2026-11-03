"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ReadAttempt, ReadText } from "@/lib/read_aloud/types";
import { rerecognizeAttempt, rerecognizeAttemptOnHub } from "@/lib/read_aloud/rerecognize";
import HubStatusOverlay, { type HubStage } from "@/components/read_aloud/HubStatusOverlay";
import ScoringChoiceDialog from "@/components/read_aloud/ScoringChoiceDialog";
import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";

const LOG_MODULE = "read-aloud";

/** The reference a re-assessment is scored against: the normalized spoken form
 *  when the text has one, the text itself otherwise — the same resolution the
 *  backend applies, so the fallback sent to the hub and the diff shown here
 *  cannot disagree. */
export function textReference(text: ReadText): string {
  return text.normalized_content?.trim() ? text.normalized_content : text.content;
}

/** The re-assessment waiting on a decision or on the hub's reply: the recording
 *  itself is never at stake, only where its new numbers come from. */
type Pending = { attempt: ReadAttempt; content: string };

export type AssessAgain = {
  /** This device's role, read once. A hub has the models; a follower is asked. */
  role: string | null;
  /** The attempt in flight — its row shows the spinner while it runs. */
  activeUuid: string | null;
  /** A re-assessment is open anywhere, so every entry point stands aside. */
  busy: boolean;
  /** Press of "Assess again": a hub re-scores here at once, a follower is asked
   *  where. `content` is the text's reference, the hub's fallback for a text it
   *  has not received yet. */
  ask: (attempt: ReadAttempt, content: string) => void;
  /** The dialogs this route opens. Render once, wherever the attempts live. */
  ui: ReactNode;
};

/**
 * The "Assess again" route, shared by every entry point onto it (the attempts
 * list's per-row button, the attempt detail's ask, the page toolbar's). Which
 * machine measures a re-assessment is the learner's call, exactly as it is for
 * a fresh take — so this is the same dialog pair the practice panel shows,
 * driven by the same `hub-mcp-phase` milestones, and the same one owner of the
 * busy state: two entry points asking independently would let a hub call and a
 * local run overwrite the same attempt row.
 *
 * Both routes leave the recording untouched and change only the transcript and
 * what is derived from it; see `rerecognizeAttempt` / `rerecognizeAttemptOnHub`.
 * Neither can lose anything: an attempt is only ever measured in place, so
 * walking away mid-call — or a hub that fails — leaves the row as it was.
 */
export function useAssessAgain({
  datasetUuid,
  onDone,
  onError,
}: {
  datasetUuid: string;
  /** Refetch after a landed re-assessment: transcript, score and axes all changed. */
  onDone: () => void | Promise<void>;
  /** Report a failure where the host reports failures. */
  onError: (msg: string) => void;
}): AssessAgain {
  const [role, setRole] = useState<string | null>(null);
  // The attempt in flight, from the choice tap through the whole hub call: its
  // row shows the spinner, and the other entry points stand aside for it.
  const [pending, setPending] = useState<Pending | null>(null);
  const [choiceOpen, setChoiceOpen] = useState(false);
  const [hubStage, setHubStage] = useState<HubStage | null>(null);
  const [hubError, setHubError] = useState<string | null>(null);

  // The host passes these inline, so they change every render; the routes below
  // stay stable by reading them through a ref.
  const handlers = useRef({ onDone, onError });
  handlers.current = { onDone, onError };

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

  const runLocal = useCallback(async ({ attempt }: Pending) => {
    setChoiceOpen(false);
    setHubStage(null);
    setHubError(null);
    try {
      await rerecognizeAttempt(datasetUuid, attempt);
      await handlers.current.onDone();
    } catch (e) {
      const msg = String(e);
      logError(`assess again failed: ${msg}`, LOG_MODULE);
      handlers.current.onError(msg);
    } finally {
      setPending(null);
    }
  }, [datasetUuid]);

  // Bumped when a hub call is abandoned or fails: the reply of a call nobody is
  // waiting for must not re-write the attempt the learner walked away from.
  const hubToken = useRef(0);

  const runHub = useCallback(async ({ attempt, content }: Pending) => {
    setChoiceOpen(false);
    setHubError(null);
    setHubStage("sending");
    const token = ++hubToken.current;
    try {
      await rerecognizeAttemptOnHub(datasetUuid, attempt, content, {
        // The reply is in; the overlay says so while this device stores it.
        onReplied: () => setHubStage("done"),
        // The request outlives a cancel, so the reply is dropped here rather
        // than written over whatever the learner chose to do instead.
        cancelled: () => token !== hubToken.current,
      });
      if (token !== hubToken.current) return; // cancelled while in flight
      await handlers.current.onDone();
      setHubStage(null);
      setPending(null);
    } catch (e) {
      if (token !== hubToken.current) return;
      const msg = String(e);
      logError(`hub assess again failed: ${msg}`, LOG_MODULE);
      // The attempt stays held and unchanged: the overlay names the failure and
      // leaves the way out at "Not now" — pressing "Assess again" afterwards
      // re-asks where it should be measured.
      setHubError(msg);
      setHubStage("error");
    }
  }, [datasetUuid]);

  const ask = useCallback(
    (attempt: ReadAttempt, content: string) => {
      if (!isTauri() || !attempt.audio_url) return;
      // A hub's hub would be itself: nothing to choose, and the local route is
      // what it would pick anyway. The attempt is still held, so its row shows
      // the spinner and the other entry points stand aside for the whole run.
      if (role === "hub") {
        const next = { attempt, content };
        setHubError(null);
        setHubStage(null);
        setPending(next);
        void runLocal(next);
        return;
      }
      setHubError(null);
      setHubStage(null);
      setPending({ attempt, content });
      setChoiceOpen(true);
    },
    [role, runLocal]
  );

  // "Not now", from either dialog: stop watching this call and leave the attempt
  // exactly as it was. Only its numbers are pending — the recording is on disk —
  // so nothing is stranded by walking away, and the row's own "Assess again" can
  // be pressed whenever the learner wants. Bumping the token makes a hub reply
  // that arrives afterwards drop instead of overwriting this choice.
  const walkAway = useCallback(() => {
    hubToken.current += 1;
    setChoiceOpen(false);
    setHubStage(null);
    setHubError(null);
    setPending(null);
  }, []);

  const ui = (
    <>
      {choiceOpen && pending && (
        <ScoringChoiceDialog
          onHub={() => void runHub(pending)}
          onLocal={() => void runLocal(pending)}
          onLater={walkAway}
        />
      )}
      {hubStage !== null && (
        <HubStatusOverlay stage={hubStage} error={hubError} onLater={walkAway} />
      )}
    </>
  );

  return {
    role,
    activeUuid: pending?.attempt.uuid ?? null,
    busy: pending !== null,
    ask,
    ui,
  };
}
