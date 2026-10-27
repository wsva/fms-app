# Read-Aloud Feedback — Turning Measurements Into Advice

> Status: **implemented** (`read_aloud_feedback`, `read_aloud_feedback_context`, `read_aloud_save_feedback`, the `Feedback` card, and `datasets/read_aloud/feedback.rs`). This is the advice layer over the measurements specified in [read-aloud-score.md](./read-aloud-score.md); it changes none of them.

## The gap

Read Aloud reports three numbers — accuracy, fluency, pronunciation — and then stops. They answer *did you say the right words*, *at what pace*, and *how clearly*. They never answer what the learner actually wants after a 43%: **what should I do differently?**

The information needed to answer that is already in the app. The alignment says *which* words were misread, the fluency pass says *where* the pauses fell and *how long* they were, the GOP pass says *which sound* in *which word* came out wrong — and, with one more step, *what the model heard instead*. None of it is prose, and nobody reads a posterior matrix.

So this adds one explicit action, on a result or on any past attempt, that sends that evidence to a language model and stores a structured report: what went wrong, why, how to fix it, a short drill, and one line to hold on to for the next take.

## Terminology

| Term | Means | Lives in |
|---|---|---|
| **feedback** | The report attached to one attempt: summary, diagnosis, issues, focus words, drill, `next_try` | `feedback_json` on `read_attempt_event` |
| **evidence** | The rendered facts the report must explain — every number the scorer produced, in text | `feedback::build_evidence` |
| **prompt** | The instruction plus the evidence: the exact words a model is answering | `feedback::INSTRUCTION` + evidence |
| **context** | Evidence + prompt + the recording's whereabouts, for a caller doing its own reasoning | `read_aloud_feedback_context` |

It is called *feedback*, not *coach*: this is one-shot derived output about one attempt, not a persona with sessions and memory. It also keeps the file family plain (`assess.rs`, `normalize.rs`, `audit.rs`, `feedback.rs`) and matches the buttons that already exist ("Assess again", "Re-recognize").

## Two paths, one evidence pack

```
attempt row (recognized, score, metrics_json)  ─┐
reference text (effective reference)           ─┼─► build_evidence ──► INSTRUCTION + evidence
dataset info.json (language)                   ─┤                          │
device-local recording ─► phoneme model ───────┘                          │
   (weak phones + what was heard, on demand)                              │
                                                    ┌─────────────────────┴──────────────────┐
                                                    ▼                                        ▼
                                       read_aloud_feedback                      read_aloud_feedback_context
                                       llm_complete(...)                        (evidence, prompt, audio path)
                                                    │                                        │
                                             parse_report                             external agent
                                                    │                                        │
                                                    └──────────────► feedback_json ◄──────────┘
                                                                     (read_aloud_save_feedback)
```

The first path is in-app and offline-capable (Ollama, or any configured cloud provider). The second is the handoff: this app's LLM seam carries **text only** — `goose-sdk::MessageContent` has Text, Image and Document, no audio — so a recording can only reach a model that can hear it through an agent that reads the file itself. Both write the same shape, so the UI card is the same either way.

## The report

Stored verbatim as `feedback_json`:

```rust
struct FeedbackReport {
    schema_version: i64,                 // 1
    summary: String,                     // 3-4 sentences: what these numbers say
    diagnosis: Option<String>,           // the single biggest cause of the lost points
    issues: Vec<FeedbackIssue>,          // <= 5, worst first
    focus_words: Vec<FeedbackFocusWord>, // <= 8: word, ipa, hint, score
    drill: Option<String>,               // 2-5 minute plan (markdown allowed)
    next_try: String,                    // one actionable sentence
    source: String,                      // "llm" | "agent"
    model: Option<String>,
    created_at: String,
    format: String,                      // "report" | "text"
}

struct FeedbackIssue { area, evidence, detail, fix, drill: Option<String> }
struct FeedbackFocusWord { word, ipa: Option<String>, hint, score: Option<f64> }
```

`format: "text"` is the fallback: a model that answered in prose instead of JSON still said something useful, and losing it would be worse than storing it unstructured. Then `summary` carries the prose and nothing else is rendered.

## The evidence pack

`build_evidence(...)` is pure — every fact comes from its arguments — which is what makes the prompt unit-testable without a model, a recording or a network. Each item is a **number or a quote**, never a paraphrase:

- the dataset language and the effective reference the scorer used, quoted, with its word count
- the transcript the recognizer produced, quoted
- **accuracy**: score, lenient score, WER, reference/correct/near-miss/misread/omitted/inserted/repeated counts, then the alignment runs — `misread: "tag" heard as "tak"`, `never said: "laut"`, `said but not in the passage: "ja"` (capped at 12)
- **fluency**: `wcpm` with its band, articulation rate, pause count (long / mid-phrase share), words between pauses, repeats and restarts, the reading span inside the recording — or an explicit "not measured: the speech model gave no word timings for this take…"
- **pronunciation**: score, weak ratio, the stored weakest words with their GOP — or the stored `unavailable_reason`, rendered in words (`describe_reason`)
- **phone detail**, when it could be recomputed: `"hund" /h u n t/ 0.31 — weak /u/ 0.12 (heard /o/)`
- **pauses**: length, start, kind, and the reference word each followed
- the recording: duration and format, and whether this analysis had the audio (it never does, in-app)

Two subtleties worth stating, because both are failure modes that look like data:

1. **The stored aggregate and the recomputed runs can disagree.** Storage keeps only the twelve weakest words, so alignment runs are recomputed with `score_asr(passage, recognized)` on every call. If the passage or the metric changed since the take was scored, the aggregate is stale and the evidence says so explicitly (`stored 71.0, recomputed 68.0`) with a note that the word list is the current one.
2. **"Not measured" is a line in the evidence, not silence.** A model with no fluency numbers will invent fluent-sounding advice about pacing unless it is told the axis is blank. `INSTRUCTION` makes that a rule, and each unmeasured axis names itself and its reason.

## The prompt

One `INSTRUCTION` constant, `{{language}}` substituted, plus the evidence. The rules exist because each forbids an observed failure mode of small local models:

- the evidence is measured fact — never invent numbers, words, sounds or timings, and quote what you use
- when an axis says "not measured", say so plainly rather than guessing what the learner did
- judge pronunciation, rhythm and accuracy of the reading — **not** comprehension, grammar or the passage's content
- every issue quotes its evidence; every fix is doable in under five minutes with no app, tool or partner
- keep target-language words quoted, in the passage's language, while writing the whole answer in `{{language}}`
- reply with JSON only, in the given shape, at most 5 issues and 8 focus words

`build_prompt` is shared by the command, the copy-prompt route and the MCP tool, for the same reason `normalize.rs` holds its prompt: identical input has to produce comparable output however it was run.

Generation is deliberately cold: `temperature = 0.3`. The evidence is the content; the failure mode is elaboration, not blandness.

## Phone-level evidence

The advice that makes a report worth reading is concrete: *the /u/ in "hund" came out as /o/*, not *work on your vowels*. Reaching that needs three small additions to the pronunciation pipeline:

- `PhonemeModel::tokens()` — the vocabulary in id order, so a phone id can be named
- `LogPost::argmax_phone(span)` — the modal non-blank winner across the span's frames: what the model itself thought it heard there. Blank frames are skipped (they hold CTC transitions, not a sound); a span of nothing but blanks returns `None` rather than claiming silence was a phone
- `PronRun` carries `tokens`, filled in `pronunciation_evidence` from the model it already has open

`feedback::phone_detail` then re-runs `pronunciation_evidence` on the **device-local recording** against the text's default clip, and keeps the worst ≤ 6 words below 0.6, each with up to two weakest phones: `{ipa, score, heard}` plus the word's full target IPA. Every missing ingredient — the recording, the phoneme model, the standard clip — simply yields no phone detail; the report then carries none, which is honest.

This is on demand, behind the explicit click, so it never slows a submission, and it keeps `Assessment` and its ~2 KB stored-metrics budget untouched.

## Storage and sync

Additive, local, no protocol change:

```sql
ALTER TABLE read_attempt_event ADD COLUMN feedback_json TEXT
```

applied idempotently in `ensure_attempt_schema`, exactly like `metrics_json`. `ReadAttempt.feedback` is populated by `read_aloud_list_attempts` and `None` from `read_aloud_submit`.

`feedback_json` is deliberately **not** in `attempt_payload`, `rows_for_user` or `apply_remote_save`:

- it is derived from one device's model (or one agent's reading) and one device's recording, so two devices would legitimately produce two different reports for the same attempt
- it costs one click to regenerate
- it must not grow journal traffic with megabytes of prose

So it is not journaled and not synced, and a peer that knows nothing about the column simply never sees it.

`read_aloud_save_feedback` writes with a direct `UPDATE … WHERE uuid = ? AND user_id = ?`, so an agent's report lands on the user's own attempt and nowhere else.

## The commands and the MCP tools

Two Tauri commands, registered in **both** handlers (`llm_complete` compiles on Android):

- `read_aloud_feedback(dataset_uuid, attempt_uuid, text_uuid?, model?, temperature?, language?)` → the stored `FeedbackReport`
- `read_aloud_feedback_context(dataset_uuid, attempt_uuid, text_uuid?, language?)` → `FeedbackContext` (evidence, prompt, audio facts) — no audio, which is what the UI's copy route needs

Three MCP tools wrapping the same core functions:

| Tool | Tier | What it does |
|---|---|---|
| `read_aloud_feedback` | write (derived) | Run the in-app route for one attempt and store the report |
| `read_aloud_feedback_context` | **read** | Evidence + prompt + recording path; `include_audio_base64: true` inlines the bytes (refused above 8 MB — the message names `audio.path`) |
| `read_aloud_save_feedback` | write (derived) | Validate and store an agent's report, forcing `source: "agent"` |

Both write-tier tools are in the default `write` tier not because they destroy anything but because they change device-local state; only the context tool joins `READ_TOOLS` in `policy.rs`. `mcp/manual.rs` needs nothing — the `read_aloud_` prefix is already known.

## The UI

`FeedbackPanel` (`src/components/read_aloud/`, plus types and calls in `src/lib/read_aloud/feedback.ts`) appears in two places:

- **PracticePanel** result card, directly under `AssessmentCards` — the first thing a learner sees after a low score
- **AttemptsPanel** detail view — the stored report for that attempt, with "Ask again"

States: idle ("Get feedback" + a compact model select under the shared `llm_selected_model` key, `""` = the Settings default) → busy ("Analyzing your take… a local model can take a minute") → report → prose fallback → error. The report renders a small provenance line, `summary` through the shared `MarkdownViewer`, the diagnosis, issue cards (area chip, quoted evidence, detail, fix, optional drill), focus-word chips with IPA, the drill, and `next_try` highlighted as "On your next take".

Two behaviours are deliberate:

- **No metrics, no request.** An attempt whose recording is gone has nothing measured, so the panel says so and offers "Assess again" instead of asking a model to explain a blank.
- **"Copy prompt" is always there.** It is the same escape hatch the normalize card offers: the exact prompt, for a provider the app cannot reach. An agent's answer shows up in the same card after the attempts are reloaded.

## Tests

Pure, no model and no network:

| Area | Covers |
|---|---|
| `feedback` | the prompt quotes the real numbers; each unmeasured axis says so; misread and inserted runs are named; a measured axis renders from the stored numbers; phone detail names the sound and what was heard; JSON is found inside fences and prose; a truncated tail after the object is ignored; prose survives as one summary; an empty answer is an error; JSON without a `summary` is not a report; caps hold on a flooded report; the language defaults and is echoed; an issue with no content is dropped |
| `gop` | `argmax_phone` reports the modal non-blank sound and returns `None` for an all-blank span |
| `phoneme` | `tokens()` matches `vocab.json` order |

Checks: `cargo check --offline` (no features), `cargo check --offline --features desktop`, `cargo test --offline --features desktop --lib`, `npx tsc --noEmit`.

## Not in v1

Automatic feedback on submit (it is an explicit click: a model call is worth asking for, not spending); streaming the answer into the card; a feedback history per text; translating the report into every UI language (the tag is threaded through, `en` is what today's UI sends); a composite "coach score"; uploading audio to a cloud provider from the in-app path; and any change to `score`, `best_score`, XP, `metrics_json` or `PROTOCOL_VERSION`.

## Decision log

**F1 — The report is derived output, so it does not sync.** The tempting shape was a journalled artifact like a book's understanding layer. But a report is a function of *(attempt, evidence, model)*: two devices with different models would produce different answers, and the answer can be regenerated for free. Journaling it would add cost and conflict for no shared state, so it stays a local column. This is the same reasoning that keeps `metrics_json` compact — but the opposite conclusion about scope, because metrics travel and feedback does not.

**F2 — Evidence is recomputed for the word runs, stored for the aggregates.** `metrics_json` keeps only the twelve weakest words, so the alignment runs the report needs are not there. Re-scoring the stored transcript is cheap and exact, so the runs are recomputed on every call. When the two disagree by more than a point the evidence says which one is which rather than silently picking — a stale reference is a real state (a text's normalized form may have changed) and the model has to know.

**F3 — Phone detail is recomputed, never stored.** It needs the recording, the phoneme model and the standard clip, all device-local and all already loaded by the scoring pass. Storing it would blow the metrics budget; computing it during submit would slow every submission for a report most takes never ask for. So it runs behind the click, and its absence is not an error.

**F4 — `text_uuid` is optional and cross-checked, not trusted.** The plan required it. Three call sites already know the attempt uuid and the text uuid, so requiring it only adds a way to get the pairing wrong — and a mismatch would silently attach a report to the wrong passage. It is accepted when given, compared against the attempt's own `text_uuid`, and refused when they differ.

**F5 — Parsing is tolerant in one direction only.** Fences, prose around the object, a truncated tail and prose-instead-of-JSON all survive; JSON that parses but carries no `summary` does not, because a report without its one required field is not something a learner can read. The prose fallback exists for the model that ignored the instruction, not for the model that answered nonsense.

**F6 — `argmax_phone` is modal, not the single most confident frame.** One confident frame inside a span the model mostly read as something else is noise; the mode over the span's non-blank winners is what "the model heard /o/ there" means. It costs a few lines and makes the claim defensible.

**F7 — Re-recognizing an attempt clears its feedback.** "Assess again" replaces the transcript, which changes every number the report quotes — word runs, counts, the accuracy score. Keeping the report would show advice about a take that no longer exists, so `update_recognized_inner` sets `feedback_json = NULL` with the metrics it rewrites, and the card offers "Ask again".

**F8 — The card is a card, not a fourth axis.** Feedback has no score, is not folded into the assessment, and never appears in the XP path. A number for "how good is the advice" would be invented, and the learner already has the three numbers that are measured.
