"use client";

/**
 * UnderstandingPanel — the tab body of the book "Understanding" mode: the
 * right pane (desktop split) / slide-in drawer (mobile). Its scope — the whole
 * chapter or the focused paragraph — is set by the Chapter toggle on the
 * understanding sub-toolbar, which also owns the tab strip and the model
 * picker; this component renders the active tab's content for `b.scope`.
 *
 * All data and mutations come from `useBookData` (passed as `b`); this component
 * only renders and keeps transient UI state (quiz answers, pickers).
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Volume2,
  Wand2,
  RefreshCw,
  Plus,
  Trash2,
  Check,
  X,
  Loader2,
  Bot,
  Copy,
  Languages,
} from "lucide-react";
import MarkdownViewer from "@/components/wiki/markdown/markdown";
import ArtifactGenerateButton from "./ArtifactGenerateButton";
import SpeakToolbar from "./SpeakToolbar";
import type { useBookData } from "@/hooks/useBookData";
import type {
  ArtifactJob,
  ArtifactKind,
  ArtifactScopeType,
  BookArtifactView,
  BookTerm,
  QuizQuestion,
} from "@/lib/read/types";
import {
  findArtifact,
  notesForScope,
  assessmentForNote,
  parseQuiz,
  emptyAnswers,
  scoreQuiz,
  isMcqCorrect,
  sourceLabel,
  type QuizAnswer,
} from "@/lib/read/artifacts";import { buildArtifactJobPrompt } from "@/lib/read/agent-prompt";
import type { CardDatasetSummary } from "@/lib/types";

type Props = { b: ReturnType<typeof useBookData> };

/** Shared read-aloud language + its setter, passed down so every Listen button
 *  in the panel stays in sync (one language for the whole chapter). */
type SpeakCtx = { language: string; onChange: (lang: string) => void };

/** "paragraph" / "chapter" — the noun the current scope reads as, for copy. */
function scopeNoun(scopeType: ArtifactScopeType): string {
  return scopeType === "chapter" ? "chapter" : "paragraph";
}

export default function UnderstandingPanel({ b }: Props) {
  const {
    focusParagraph,
    paragraphs,
    artifactsByScope,
    saving,
    handleParagraphListen,
    understandingTab,
    scope,
    chapterMode,
    currentChapter,
  } = b;

  // Read-aloud language for the Listen buttons. "" means "use the book's own
  // language"; persisted so the choice survives across chapters and books.
  const [speakLang, setSpeakLangState] = useState<string>(() => {
    if (typeof window === "undefined") return "";
    return window.localStorage.getItem("book-understanding-tts-lang") || "";
  });
  const setSpeakLang = useCallback((lang: string) => {
    setSpeakLangState(lang);
    if (typeof window !== "undefined") window.localStorage.setItem("book-understanding-tts-lang", lang);
  }, []);
  const speak: SpeakCtx = {
    language: speakLang || b.selectedBook?.language || "",
    onChange: setSpeakLang,
  };

  const focusedPara = useMemo(
    () => paragraphs.find((p) => p.breakSentence?.uuid === focusParagraph) ?? null,
    [paragraphs, focusParagraph]
  );
  const scopeArtifacts = scope.uuid ? artifactsByScope.get(scope.uuid) : undefined;

  // The header names what the Chapter toggle currently points the tabs at:
  // the chapter title, or the focused paragraph's opening line.
  const paraPreview = focusedPara?.sentences.map((s) => s.content).join(" ").trim() ?? "";
  const hasScope = !!scope.uuid;
  const scopeLabel = chapterMode
    ? `Chapter · ${currentChapter?.title ?? ""}`
    : "Understanding";
  const scopePreview = chapterMode
    ? currentChapter?.title ?? ""
    : paraPreview || "(empty paragraph)";

  return (
    <div className="flex flex-col h-full min-h-0 min-w-0 bg-bg-body">
      {/* Header: what the scope currently points at + Listen (paragraph only —
          the command behind it is addressed by a paragraph's break-row uuid). */}
      <div className="shrink-0 px-3 py-2 border-b border-border-default flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <div className="text-[11px] font-semibold text-text-tertiary tracking-wider uppercase">
            {scopeLabel}
          </div>
          <p
            className="text-xs text-text-secondary line-clamp-2 mt-0.5"
            title={scopePreview}
          >
            {hasScope ? scopePreview : "Select a paragraph"}
          </p>
        </div>
        <button
          title="Listen to this paragraph (Edge TTS)"
          disabled={!hasScope || chapterMode || saving}
          className="shrink-0 p-2 rounded-md text-text-secondary hover:bg-bg-hover disabled:opacity-40 cursor-pointer"
          onClick={() => focusParagraph && handleParagraphListen(focusParagraph)}
        >
          {saving ? <Loader2 size={16} className="animate-spin" /> : <Volume2 size={16} />}
        </button>
      </div>

      {/* Body — the tab strip and model picker live on the understanding
          sub-toolbar above the split; this renders the active tab. */}
      <div className="flex-1 min-h-0 overflow-y-auto p-3">
        {understandingTab === "glossary" ? (
          <GlossaryTab b={b} />
        ) : !hasScope ? (
          <div className="text-sm text-text-tertiary text-center py-8">
            Click a paragraph on the left to understand it.
          </div>
        ) : understandingTab === "digest" ? (
          <GeneratedSection
            b={b}
            speak={speak}
            kind="digest"
            scopeType={scope.type}
            scopeUuid={scope.uuid}
            artifact={findArtifact(scopeArtifacts, "digest")}
            emptyHint="No premise outline yet."
            cta={chapterMode ? "Digest chapter" : "Digest this paragraph"}
          />
        ) : understandingTab === "explain" ? (
          <GeneratedSection
            b={b}
            speak={speak}
            kind="explanation"
            scopeType={scope.type}
            scopeUuid={scope.uuid}
            artifact={findArtifact(scopeArtifacts, "explanation")}
            emptyHint="No explanation yet."
            cta={`Explain this ${scopeNoun(scope.type)}`}
          />
        ) : understandingTab === "simplify" ? (
          <GeneratedSection
            b={b}
            speak={speak}
            kind="simplification"
            scopeType={scope.type}
            scopeUuid={scope.uuid}
            artifact={findArtifact(scopeArtifacts, "simplification")}
            emptyHint="No simplified rewrite yet."
            cta={`Simplify this ${scopeNoun(scope.type)}`}
          />
        ) : understandingTab === "translate" ? (
          <TranslationTab b={b} speak={speak} scopeArtifacts={scopeArtifacts} />
        ) : understandingTab === "argument" ? (
          <GeneratedSection
            b={b}
            speak={speak}
            kind="argument_map"
            scopeType={scope.type}
            scopeUuid={scope.uuid}
            artifact={findArtifact(scopeArtifacts, "argument_map")}
            emptyHint="No argument map yet."
            cta="Map the argument"
          />
        ) : understandingTab === "quiz" ? (
          <QuizTab
            b={b}
            artifact={findArtifact(scopeArtifacts, "questions")}
            scopeType={scope.type}
            scopeUuid={scope.uuid}
          />
        ) : (
          <NotesTab
            b={b}
            speak={speak}
            scopeArtifacts={scopeArtifacts}
            scopeType={scope.type}
            scopeUuid={scope.uuid}
          />
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// What an open request is doing
// ---------------------------------------------------------------------------

/** "7 min left on the lease", or the honest alternative when there is no
 *  deadline to count. Pure, so the ticker only has to supply `now`. */
function leaseText(expiresAt: number | null | undefined, now: number): string {
  if (!expiresAt) return "on a live lease";
  const left = Math.ceil((expiresAt - now) / 60_000);
  if (left <= 0) return "the lease is about to lapse";
  return `${left} min left on the lease`;
}

/** The one sentence the panel shows where it used to print a prompt.
 *
 *  With the mailbox, an attached agent claims a request by itself, so the
 *  question the learner actually has is "who has it, and is anything
 *  happening" — not "what do I paste where". The prompt survives a disclosure
 *  below, for the one case this sentence cannot help: no agent attached yet. */
function jobStateLine(job: ArtifactJob, now: number): string {
  const state = (() => {
    if (job.status === "waiting_user") {
      return "The agent asked a question — answer it in the Mailbox (Ctrl+Space).";
    }
    if (job.status === "claimed" || job.status === "delivering") {
      const who = job.claimed_by?.trim() || "Your agent";
      return `${who} is working on this · ${leaseText(job.expires_at, now)}.`;
    }
    if (job.attempts > 0) {
      const n = job.attempts === 1 ? "one attempt" : `${job.attempts} attempts`;
      // The ledger's own sentence, when it kept one: an agent that released
      // the item said why, and that is the part the learner cannot infer from
      // a count.
      const why = job.reason?.trim() ? ` Last one: ${job.reason.trim()}` : "";
      return `The last attempt came back unfulfilled (${n} so far).${why} An attached agent can claim it again.`;
    }
    // The one case the sentence has to answer for: nobody is attached, and the
    // request will sit here until someone is. The page that fixes it is named.
    return "Nobody has picked this up yet. An attached agent claims it by itself — Tools → Agent if none is attached.";
  })();
  // The header's amber wand is the icon; this is the sentence that says what it
  // means, because the passage changing under a live request is the one thing a
  // learner should not have to infer from a colour.
  return job.stale ? `${state} The passage changed since you asked.` : state;
}

// ---------------------------------------------------------------------------
// Generated markdown artifact (explain / simplify / argument / digest)
// ---------------------------------------------------------------------------

function GeneratedSection({
  b,
  speak,
  kind,
  scopeType,
  scopeUuid,
  artifact,
  emptyHint,
  cta,
  getInstructions,
}: {
  b: ReturnType<typeof useBookData>;
  speak: SpeakCtx;
  kind: ArtifactKind;
  scopeType: ArtifactScopeType;
  scopeUuid: string;
  artifact?: BookArtifactView;
  emptyHint: string;
  cta: string;
  /** One-off guidance appended to the backend prompt at click time (the
   *  Translate tab names its target language here). Agent requests capture it
   *  into the job, so the copy-prompt fallback carries it too. */
  getInstructions?: () => string | undefined;
}) {
  const { generating, handleGenerate, handleRequestAgent, handleCancelJob, handleDeleteArtifact, jobs, mobile, bookUUID, understandingModel } = b;
  // The outstanding agent request for this exact scope + kind, if any.
  const job = jobs.find((j) => j.scope_uuid === scopeUuid && j.kind === kind);
  // The fallback, one disclosure down: for an agent that cannot attach on its
  // own. Built from the job row itself (its instructions are the truth), naming
  // the job so the agent claims this one, and pre-filling the existing uuid
  // only when regenerating in place.
  const agentPrompt = job
    ? buildArtifactJobPrompt({
        bookUuid: bookUUID,
        jobUuid: job.uuid,
        scopeType: job.scope_type,
        scopeUuid: job.scope_uuid,
        kind: job.kind,
        instructions: job.instructions,
        existingUuid: artifact?.uuid,
      })
    : "";
  // The lease line names minutes, so a claimed card has to re-render. 30 s is
  // enough for a countdown read at a glance, and nothing else here ticks. The
  // effect keys on the status, not the row: the poll replaces the object every
  // 12 s, and keying on it would restart the interval before it ever fired.
  const [now, setNow] = useState(() => Date.now());
  const jobStatus = job?.status;
  useEffect(() => {
    if (jobStatus !== "claimed" && jobStatus !== "delivering") return;
    // First tick is the claim itself, so the countdown starts from when the row
    // was read rather than from when this panel mounted.
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, [jobStatus]);

  const [copied, setCopied] = useState(false);
  const [clipFailed, setClipFailed] = useState(false);
  // A new job (or its disappearance) resets the copy feedback.
  useEffect(() => {
    setCopied(false);
    setClipFailed(false);
  }, [job?.uuid]);
  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(agentPrompt);
      setCopied(true);
      setClipFailed(false);
    } catch {
      setClipFailed(true);
    }
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 text-xs text-text-tertiary min-w-0">
          {job ? (
            <span className="flex items-center gap-1 text-accent truncate">
              <Loader2 size={12} className="animate-spin shrink-0" />{" "}
              {job.status === "waiting_user"
                ? "Needs your answer"
                : job.status === "claimed" || job.status === "delivering"
                  ? "Agent working…"
                  : "Waiting for agent…"}
              {job.stale && <Wand2 size={12} className="text-amber-500 shrink-0" />}
            </span>
          ) : artifact ? (
            <>
              <span>{sourceLabel(artifact.source)}</span>
              {artifact.model && <span className="truncate">· {artifact.model}</span>}
              {artifact.stale && (
                <span className="flex items-center gap-1 text-amber-500">
                  <Wand2 size={12} /> outdated
                </span>
              )}
            </>
          ) : (
            <span>{emptyHint}</span>
          )}
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {job && (
            <button
              title="Cancel agent request"
              className="p-1.5 rounded text-text-tertiary hover:bg-bg-hover hover:text-red-500 cursor-pointer"
              onClick={() => handleCancelJob(job.uuid)}
            >
              <X size={14} />
            </button>
          )}
          {artifact && !job && (
            <button
              title="Delete"
              disabled={generating}
              className="p-1.5 rounded text-text-tertiary hover:bg-bg-hover hover:text-red-500 disabled:opacity-40 cursor-pointer"
              onClick={() => handleDeleteArtifact(artifact.uuid)}
            >
              <Trash2 size={14} />
            </button>
          )}
          {/* One button, two roads: the label generates with the configured
              LLM now, the menu also offers the agent mailbox. On a phone the
              agent road cannot exist (agents attach to the desktop MCP
              server), so the menu is not shown. Hidden while a request is
              already outstanding — the job card owns the affordance then. */}
          {!job && (
            <ArtifactGenerateButton
              label={artifact ? "Regenerate" : cta}
              busy={generating}
              disabled={generating}
              showAgent={!mobile}
              modelName={understandingModel}
              onCallLlm={() => handleGenerate(scopeType, scopeUuid, kind, getInstructions?.())}
              onAskAgent={() => handleRequestAgent(scopeType, scopeUuid, kind, getInstructions?.())}
            />
          )}
        </div>
      </div>
      {job && (
        <div className="flex flex-col gap-2 rounded-lg border border-border-default bg-bg-muted px-2.5 py-2">
          {/* State, not instructions. An attached agent claims this on its own,
              so what is worth showing is who has it and how long is left; the
              prompt is the fallback for when nobody is attached at all. */}
          <div className="flex items-start gap-1.5 text-xs text-text-secondary">
            <span className="mt-px shrink-0 text-text-tertiary">
              {job.status === "claimed" || job.status === "delivering" ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Bot size={12} />
              )}
            </span>
            <span className="min-w-0">{jobStateLine(job, now)}</span>
          </div>
          <details>
            <summary className="cursor-pointer text-[11px] text-text-tertiary hover:text-text-secondary">
              No agent attached? Copy the request for one.
            </summary>
            <div className="mt-2 flex flex-col gap-1.5">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] text-text-tertiary">
                  Paste it into an agent that cannot attach on its own.
                </span>
                <button
                  type="button"
                  className="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-medium border border-border-default text-text-secondary hover:bg-bg-hover cursor-pointer shrink-0"
                  onClick={() => void copyPrompt()}
                >
                  {copied ? <Check size={13} className="text-green-500" /> : <Copy size={13} />}
                  {copied ? "Copied" : "Copy prompt"}
                </button>
              </div>
              <textarea
                readOnly
                value={agentPrompt}
                onFocus={(e) => e.currentTarget.select()}
                className="w-full h-32 p-2 rounded-md bg-bg-base border border-border-default text-[11px] text-text-primary font-mono resize-y"
              />
              {clipFailed && (
                <p className="text-[11px] text-amber-500">
                  Could not reach the clipboard — select the text above and copy it.
                </p>
              )}
            </div>
          </details>
        </div>
      )}
      {artifact ? (
        <>
          <div className="flex items-center justify-end">
            <SpeakToolbar
              text={artifact.content}
              language={speak.language}
              onLanguageChange={speak.onChange}
            />
          </div>
          <div className="rounded-lg border border-border-default bg-bg-card p-3">
            <MarkdownViewer content={artifact.content} />
          </div>
        </>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Quiz tab
// ---------------------------------------------------------------------------

function QuizTab({
  b,
  artifact,
  scopeType,
  scopeUuid,
}: {
  b: ReturnType<typeof useBookData>;
  artifact?: BookArtifactView;
  scopeType: ArtifactScopeType;
  scopeUuid: string;
}) {
  const { generating, handleGenerate, handleQuizFinish, listCardDatasets, handlePromoteToCard, mobile, understandingModel } = b;
  const questions = useMemo<QuizQuestion[]>(
    () => (artifact ? parseQuiz(artifact.content) : []),
    [artifact]
  );

  const [answers, setAnswers] = useState<QuizAnswer[]>([]);
  const [finished, setFinished] = useState(false);
  const [score, setScore] = useState<{ total: number; correct: number } | null>(null);
  const [xpMsg, setXpMsg] = useState("");
  const [promoteQ, setPromoteQ] = useState<QuizQuestion | null>(null);
  const [datasets, setDatasets] = useState<CardDatasetSummary[]>([]);

  useEffect(() => {
    setAnswers(emptyAnswers(questions));
    setFinished(false);
    setScore(null);
    setXpMsg("");
    setPromoteQ(null);
  }, [artifact?.uuid, questions.length]);

  if (!artifact) {
    return (
      <div className="flex flex-col items-center gap-3 py-8 text-center">
        <p className="text-sm text-text-tertiary">No quiz for this {scopeNoun(scopeType)} yet.</p>
        <ArtifactGenerateButton
          label="Generate quiz"
          busy={generating}
          disabled={generating}
          showAgent={!mobile}
          modelName={understandingModel}
          onCallLlm={() => handleGenerate(scopeType, scopeUuid, "questions")}
          onAskAgent={() => b.handleRequestAgent(scopeType, scopeUuid, "questions")}
        />
      </div>
    );
  }

  if (questions.length === 0) {
    return (
      <div className="flex flex-col items-center gap-3 py-8 text-center">
        <p className="text-sm text-text-tertiary">This quiz has no valid questions.</p>
        <button
          className="px-3 py-1.5 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
          disabled={generating}
          onClick={() => handleGenerate(scopeType, scopeUuid, "questions")}
        >
          Regenerate
        </button>
      </div>
    );
  }

  const allAnswered = questions.every((q, i) => {
    const a = answers[i];
    if (!a) return false;
    return q.type === "mcq" ? a.type === "mcq" && a.selected !== null : a.type === "open" && a.graded !== null;
  });

  const finish = async () => {
    const s = scoreQuiz(questions, answers);
    setScore(s);
    setFinished(true);
    const res = await handleQuizFinish(artifact.uuid, s.total, s.correct);
    if (res?.xp?.xp_awarded && res.xp.xp_awarded > 0) {
      setXpMsg(`+${res.xp.xp_awarded} XP · level ${res.xp.level}`);
    } else if (res) {
      setXpMsg("Best score recorded");
    }
  };

  const openPromote = async (q: QuizQuestion) => {
    const list = await listCardDatasets();
    if (list.length === 0) {
      setXpMsg("No card datasets — create one first.");
      return;
    }
    setDatasets(list);
    setPromoteQ(q);
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <span className="text-xs text-text-tertiary">
          {questions.length} question{questions.length === 1 ? "" : "s"}
          {artifact.stale && <span className="ml-1 text-amber-500">· outdated</span>}
        </span>
        <button
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-text-secondary hover:bg-bg-hover cursor-pointer"
          disabled={generating}
          onClick={() => handleGenerate(scopeType, scopeUuid, "questions")}
        >
          <RefreshCw size={12} /> Regenerate
        </button>
      </div>

      {questions.map((q, i) => {
        const a = answers[i];
        return (
          <QuestionCard
            key={i}
            index={i}
            question={q}
            answer={a}
            finished={finished}
            onAnswer={(next) =>
              setAnswers((prev) => {
                const copy = [...prev];
                copy[i] = next;
                return copy;
              })
            }
            onPromote={() => openPromote(q)}
          />
        );
      })}

      {!finished ? (
        <button
          className="px-3 py-2 rounded-md text-sm font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
          disabled={!allAnswered}
          onClick={finish}
        >
          Finish quiz
        </button>
      ) : (
        <div className="rounded-lg border border-border-default bg-bg-card p-3 text-sm">
          <div className="font-semibold text-text-primary">
            Score: {score?.correct}/{score?.total}
          </div>
          {xpMsg && <div className="text-xs text-green-500 mt-1">{xpMsg}</div>}
        </div>
      )}

      {/* Card-dataset picker for promotion */}
      {promoteQ && (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40"
          onClick={() => setPromoteQ(null)}
        >
          <div
            className="bg-bg-card rounded-xl shadow-2xl p-4 w-72 border border-border-default flex flex-col gap-2"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between">
              <span className="text-sm font-semibold text-text-primary">Promote to card</span>
              <button className="cursor-pointer text-text-tertiary" onClick={() => setPromoteQ(null)}>
                <X size={16} />
              </button>
            </div>
            <p className="text-xs text-text-tertiary line-clamp-2">{promoteQ.stem}</p>
            <div className="flex flex-col gap-1 max-h-60 overflow-y-auto">
              {datasets.map((d) => (
                <button
                  key={d.info.uuid}
                  className="text-left px-3 py-2 rounded-md text-sm bg-bg-muted hover:bg-bg-hover cursor-pointer truncate"
                  onClick={async () => {
                    await handlePromoteToCard(d.info.uuid, promoteQ);
                    setPromoteQ(null);
                    setXpMsg(`Added to “${d.info.name}”`);
                  }}
                >
                  {d.info.name}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function QuestionCard({
  index,
  question,
  answer,
  finished,
  onAnswer,
  onPromote,
}: {
  index: number;
  question: QuizQuestion;
  answer?: QuizAnswer;
  finished: boolean;
  onAnswer: (a: QuizAnswer) => void;
  onPromote: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  useEffect(() => setRevealed(false), [question.stem]);

  const showResult = finished || revealed;

  if (question.type === "mcq") {
    const selected = answer?.type === "mcq" ? answer.selected : null;
    const correct = finished && selected !== null && isMcqCorrect(question, selected);
    const missed = finished && !correct;
    return (
      <div className="rounded-lg border border-border-default bg-bg-card p-3 flex flex-col gap-2">
        <div className="text-sm text-text-primary font-medium">
          {index + 1}. {question.stem}
        </div>
        <div className="flex flex-col gap-1">
          {question.choices.map((c, ci) => {
            let cls = "border-border-default bg-bg-muted hover:bg-bg-hover";
            if (showResult) {
              if (ci === question.correct) cls = "border-green-500 bg-green-500/15";
              else if (ci === selected) cls = "border-red-500 bg-red-500/15";
              else cls = "border-border-default bg-bg-muted opacity-70";
            } else if (ci === selected) {
              cls = "border-accent bg-accent-bg/20";
            }
            return (
              <button
                key={ci}
                disabled={finished}
                className={`text-left px-3 py-1.5 rounded-md text-sm border cursor-pointer transition-colors disabled:cursor-default ${cls}`}
                onClick={() => onAnswer({ type: "mcq", selected: ci })}
              >
                {c}
              </button>
            );
          })}
        </div>
        {!finished && selected !== null && !revealed && (
          <button
            className="self-start px-2.5 py-1 rounded text-xs bg-bg-hover text-text-secondary cursor-pointer"
            onClick={() => setRevealed(true)}
          >
            Check
          </button>
        )}
        {showResult && question.explanation && (
          <p className="text-xs text-text-secondary border-t border-border-default pt-2">
            {question.explanation}
          </p>
        )}
        {missed && (
          <button
            className="self-start flex items-center gap-1 px-2.5 py-1 rounded text-xs text-text-secondary hover:bg-bg-hover cursor-pointer"
            onClick={onPromote}
          >
            <Plus size={12} /> Promote to card
          </button>
        )}
      </div>
    );
  }

  // open question
  const graded = answer?.type === "open" ? answer.graded : null;
  const missed = finished && graded !== true;
  return (
    <div className="rounded-lg border border-border-default bg-bg-card p-3 flex flex-col gap-2">
      <div className="text-sm text-text-primary font-medium">
        {index + 1}. {question.stem}
      </div>
      {!revealed && !finished ? (
        <button
          className="self-start px-2.5 py-1 rounded text-xs bg-bg-hover text-text-secondary cursor-pointer"
          onClick={() => setRevealed(true)}
        >
          Show model answer
        </button>
      ) : (
        <div className="rounded-md bg-bg-muted p-2 text-sm text-text-secondary">
          <div className="text-[11px] uppercase tracking-wider text-text-tertiary mb-1">
            Model answer
          </div>
          <MarkdownViewer content={question.model_answer} />
          {question.explanation && (
            <p className="text-xs text-text-tertiary mt-2 border-t border-border-default pt-2">
              {question.explanation}
            </p>
          )}
        </div>
      )}
      {(revealed || finished) && graded === null && !finished && (
        <div className="flex gap-1">
          <button
            className="flex items-center gap-1 px-2.5 py-1 rounded text-xs bg-green-500/15 text-green-600 cursor-pointer"
            onClick={() => onAnswer({ type: "open", graded: true })}
          >
            <Check size={12} /> I got it
          </button>
          <button
            className="flex items-center gap-1 px-2.5 py-1 rounded text-xs bg-red-500/15 text-red-500 cursor-pointer"
            onClick={() => onAnswer({ type: "open", graded: false })}
          >
            <X size={12} /> Missed
          </button>
        </div>
      )}
      {finished && (
        <div className="text-xs">
          {graded === true ? (
            <span className="text-green-600">Marked correct</span>
          ) : (
            <span className="text-red-500">Missed</span>
          )}
        </div>
      )}
      {missed && (
        <button
          className="self-start flex items-center gap-1 px-2.5 py-1 rounded text-xs text-text-secondary hover:bg-bg-hover cursor-pointer"
          onClick={onPromote}
        >
          <Plus size={12} /> Promote to card
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Notes tab
// ---------------------------------------------------------------------------

function NotesTab({
  b,
  speak,
  scopeArtifacts,
  scopeType,
  scopeUuid,
}: {
  b: ReturnType<typeof useBookData>;
  speak: SpeakCtx;
  scopeArtifacts?: BookArtifactView[];
  scopeType: ArtifactScopeType;
  scopeUuid: string;
}) {
  const { saving, generating, handleSaveNote, handleDeleteArtifact, handleAssessNote } = b;
  const [draft, setDraft] = useState("");
  const notes = useMemo(() => notesForScope(scopeArtifacts), [scopeArtifacts]);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[11px] font-semibold text-text-tertiary tracking-wider uppercase">New note</span>
          <SpeakToolbar text={draft} language={speak.language} onLanguageChange={speak.onChange} disabled={saving} />
        </div>
        <textarea
          className="w-full px-3 py-2 rounded-md bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent min-h-[80px]"
          placeholder={`Write a note about this ${scopeNoun(scopeType)}…`}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
        />
        <div className="flex justify-end">
          <button
            className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
            disabled={saving || !draft.trim()}
            onClick={async () => {
              await handleSaveNote(scopeType, scopeUuid, draft);
              setDraft("");
            }}
          >
            <Plus size={13} /> Save note
          </button>
        </div>
      </div>

      {notes.length === 0 ? (
        <p className="text-sm text-text-tertiary text-center py-4">No notes yet.</p>
      ) : (
        <div className="flex flex-col gap-2">
          {notes.map((n) => {
            const assessment = assessmentForNote(scopeArtifacts, n.uuid);
            return (
              <div key={n.uuid} className="rounded-lg border border-border-default bg-bg-card p-3 flex flex-col gap-2">
                <div className="flex items-start justify-between gap-2">
                  <div className="text-sm text-text-primary whitespace-pre-wrap flex-1 min-w-0">
                    {n.content}
                  </div>
                  <button
                    title="Delete note"
                    disabled={saving}
                    className="shrink-0 p-1 rounded text-text-tertiary hover:text-red-500 disabled:opacity-40 cursor-pointer"
                    onClick={() => handleDeleteArtifact(n.uuid)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    className="flex items-center gap-1 px-2 py-1 rounded text-xs text-text-secondary bg-bg-muted hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
                    disabled={generating}
                    onClick={() => handleAssessNote(n.uuid)}
                  >
                    {generating ? <Loader2 size={12} className="animate-spin" /> : <Wand2 size={12} />}
                    {assessment ? "Re-assess" : "Assess"}
                  </button>
                  <span className="text-[11px] text-text-tertiary">{n.created_at.slice(0, 10)}</span>
                  <div className="ml-auto">
                    <SpeakToolbar text={n.content} language={speak.language} onLanguageChange={speak.onChange} disabled={saving} />
                  </div>
                </div>
                {assessment && (
                  <div className="rounded-md bg-bg-muted p-2 border-l-2 border-accent">
                    <div className="text-[11px] uppercase tracking-wider text-text-tertiary mb-1">
                      Assessment
                    </div>
                    <MarkdownViewer content={assessment.content} />
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Translate tab
// ---------------------------------------------------------------------------

/** Common targets, named in their own language so a learner recognises them. */
const TRANSLATE_TARGETS: { value: string; label: string }[] = [
  { value: "English", label: "English" },
  { value: "中文（简体）", label: "中文（简体）" },
  { value: "Deutsch", label: "Deutsch" },
  { value: "Français", label: "Français" },
  { value: "Español", label: "Español" },
  { value: "日本語", label: "日本語" },
  { value: "한국어", label: "한국어" },
  { value: "Русский", label: "Русский" },
];

/** Seed the target from a book `language` code ("de", "zh-CN", …). Unknown
 *  codes return null — the picker then keeps the generic default. */
function seedTranslateTarget(language: string | undefined): string | null {
  const base = language?.trim().toLowerCase().split(/[-_]/)[0];
  switch (base) {
    case "en": return "English";
    case "zh": return "中文（简体）";
    case "de": return "Deutsch";
    case "fr": return "Français";
    case "es": return "Español";
    case "ja": return "日本語";
    case "ko": return "한국어";
    case "ru": return "Русский";
    default: return null;
  }
}

/** The translation tab is a generated section plus the one decision the
 *  backend prompt cannot make for itself: which language to translate into.
 *  The choice rides the `instructions` channel both the built-in generator and
 *  an agent request already carry, and persists like the other panel prefs. */
function TranslationTab({
  b,
  speak,
  scopeArtifacts,
}: {
  b: ReturnType<typeof useBookData>;
  speak: SpeakCtx;
  scopeArtifacts?: BookArtifactView[];
}) {
  const { scope, selectedBook } = b;
  const known = (v: string) => TRANSLATE_TARGETS.some((t) => t.value === v);
  const [targetState, setTargetState] = useState<string>(() => {
    if (typeof window === "undefined") return "English";
    return window.localStorage.getItem("book-understanding-translate-lang") || "English";
  });
  // A stored value outside the common list is the free-text case.
  const [custom, setCustom] = useState(() => !known(targetState));
  // First ever use, still on the generic default: follow the book's own
  // language when it names one of the targets (the plan's seed), once only —
  // after that the persisted choice is the learner's.
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (window.localStorage.getItem("book-understanding-translate-lang")) return;
    const seed = seedTranslateTarget(selectedBook?.language);
    if (seed) setTargetState(seed);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const setTarget = (v: string) => {
    setTargetState(v);
    if (typeof window !== "undefined") window.localStorage.setItem("book-understanding-translate-lang", v);
  };
  const target = custom ? targetState.trim() : targetState;
  const getInstructions = () =>
    target ? `Translate the passage into ${target}.` : undefined;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2 text-xs text-text-tertiary">
        <Languages size={13} className="shrink-0" />
        <span className="shrink-0">Target language</span>
        <select
          className="min-w-0 max-w-48 px-2 py-1 rounded-md bg-bg-input border border-border-default text-text-primary focus:outline-none focus:border-accent cursor-pointer"
          value={custom ? "__custom" : targetState}
          onChange={(e) => {
            if (e.target.value === "__custom") {
              setCustom(true);
              setTarget("");
            } else {
              setCustom(false);
              setTarget(e.target.value);
            }
          }}
        >
          {TRANSLATE_TARGETS.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
          <option value="__custom">Other…</option>
        </select>
        {custom && (
          <input
            className="w-36 px-2 py-1 rounded-md bg-bg-input border border-border-default text-text-primary focus:outline-none focus:border-accent"
            placeholder="e.g. Português"
            value={targetState}
            onChange={(e) => setTarget(e.target.value)}
          />
        )}
      </div>
      <GeneratedSection
        b={b}
        speak={speak}
        kind="translation"
        scopeType={scope.type}
        scopeUuid={scope.uuid}
        artifact={findArtifact(scopeArtifacts, "translation")}
        emptyHint="No translation yet."
        cta={`Translate this ${scopeNoun(scope.type)}`}
        getInstructions={getInstructions}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Glossary tab: the book's recurring concepts
// ---------------------------------------------------------------------------

function GlossaryTab({ b }: { b: ReturnType<typeof useBookData> }) {
  const {
    chapterUUID,
    focusParagraph,
    terms,
    generating,
    saving,
    handleExtractTerms,
    handleSaveTerm,
    handleDeleteTerm,
  } = b;

  const [editing, setEditing] = useState<string | null>(null);
  const [editDef, setEditDef] = useState("");
  const [newTerm, setNewTerm] = useState("");
  const [newDef, setNewDef] = useState("");

  return (
    <div className="flex flex-col gap-5">
      {/* Glossary */}
      <section className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-2">
          <div className="text-[11px] font-semibold text-text-tertiary tracking-wider uppercase">
            Glossary · {terms.length}
          </div>
          <div className="flex items-center gap-1">
            {focusParagraph && (
              <button
                className="px-2 py-1 rounded text-xs text-text-secondary bg-bg-muted hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
                disabled={generating}
                title="Extract terms from the focused paragraph"
                onClick={() => handleExtractTerms("paragraph", focusParagraph)}
              >
                Extract ¶
              </button>
            )}
            <button
              className="flex items-center gap-1 px-2 py-1 rounded text-xs text-text-secondary bg-bg-muted hover:bg-bg-hover disabled:opacity-50 cursor-pointer"
              disabled={generating}
              title="Extract terms from the whole chapter"
              onClick={() => handleExtractTerms("chapter", chapterUUID)}
            >
              {generating ? <Loader2 size={12} className="animate-spin" /> : <Wand2 size={12} />}
              Extract chapter
            </button>
          </div>
        </div>

        {/* Add term */}
        <div className="flex flex-col gap-1.5 rounded-lg border border-border-default bg-bg-card p-2">
          <input
            className="px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent"
            placeholder="Term"
            value={newTerm}
            onChange={(e) => setNewTerm(e.target.value)}
          />
          <textarea
            className="px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent min-h-[48px]"
            placeholder="Definition"
            value={newDef}
            onChange={(e) => setNewDef(e.target.value)}
          />
          <div className="flex justify-end">
            <button
              className="flex items-center gap-1 px-2 py-1 rounded text-xs font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
              disabled={saving || !newTerm.trim()}
              onClick={async () => {
                const now = new Date().toISOString();
                const term: BookTerm = {
                  uuid: "",
                  book_uuid: "",
                  term: newTerm.trim(),
                  definition: newDef.trim(),
                  source: "user",
                  model: null,
                  created_at: now,
                  updated_at: now,
                };
                await handleSaveTerm(term);
                setNewTerm("");
                setNewDef("");
              }}
            >
              <Plus size={12} /> Add term
            </button>
          </div>
        </div>

        {/* Term list */}
        {terms.length === 0 ? (
          <p className="text-sm text-text-tertiary text-center py-4">
            No glossary terms yet. Use “Extract”.
          </p>
        ) : (
          <div className="flex flex-col gap-1.5">
            {terms.map((t) => (
              <div key={t.uuid} className="rounded-lg border border-border-default bg-bg-card p-2.5">
                <div className="flex items-start justify-between gap-2">
                  <button
                    className="text-sm font-medium text-text-primary cursor-pointer hover:text-accent flex-1 min-w-0 text-left"
                    onClick={() => {
                      setEditing(editing === t.uuid ? null : t.uuid);
                      setEditDef(t.definition);
                    }}
                  >
                    {t.term}
                  </button>
                  <div className="flex items-center gap-1 shrink-0">
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-bg-muted text-text-tertiary">
                      {sourceLabel(t.source)}
                    </span>
                    <button
                      title="Delete term"
                      disabled={saving}
                      className="p-1 rounded text-text-tertiary hover:text-red-500 disabled:opacity-40 cursor-pointer"
                      onClick={() => handleDeleteTerm(t.uuid)}
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
                {editing === t.uuid ? (
                  <div className="flex flex-col gap-1.5 mt-2">
                    <textarea
                      className="px-2 py-1 rounded bg-bg-input border border-border-default text-sm text-text-primary focus:outline-none focus:border-accent min-h-[60px]"
                      value={editDef}
                      onChange={(e) => setEditDef(e.target.value)}
                    />
                    <div className="flex justify-end gap-1">
                      <button
                        className="px-2 py-1 rounded text-xs text-text-secondary hover:bg-bg-hover cursor-pointer"
                        onClick={() => setEditing(null)}
                      >
                        Cancel
                      </button>
                      <button
                        className="px-2 py-1 rounded text-xs font-medium bg-accent-bg text-white hover:opacity-90 disabled:opacity-50 cursor-pointer"
                        disabled={saving}
                        onClick={async () => {
                          await handleSaveTerm({ ...t, definition: editDef, source: "user", updated_at: new Date().toISOString() });
                          setEditing(null);
                        }}
                      >
                        Save
                      </button>
                    </div>
                  </div>
                ) : (
                  t.definition && (
                    <div className="mt-1 text-sm text-text-secondary">
                      <MarkdownViewer content={t.definition} />
                    </div>
                  )
                )}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
