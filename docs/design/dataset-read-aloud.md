# Read-Aloud Datasets

## The idea

Read-aloud is the fifth dataset kind: a collection of short texts that the user reads into a microphone, meant to train *producing* the language rather than decoding it. The loop is always the same: show a text, record the reading, run STT over the recording, score the transcript against the reference, and award XP for a faithful read. What makes this a dataset rather than a feature is that the material — texts, recordings, attempt history — lives in one self-contained folder obeying every rule of [dataset.md](./dataset.md): uuid identity via `info.json`, per-type root under `read_aloud/`, trash-instead-of-delete for the whole folder, and sync-by-directory.

The attempt record is the interesting object. It keeps the audio, the transcript *and* the score together, so history is replayable (you can listen to last week's attempt) and re-scorable (the transcript is stored, which is what makes a metric change non-destructive — see "Scoring"). Unlike dictation progress, attempts live **inside** the dataset's `data.sqlite3`, not in the app database: they are inseparable from the material (a score against *this* text) and they travel with the folder when it syncs.

## Storage

```
<datasets_dir>/read_aloud/<name>/
├── info.json          # shared DatasetInfo (spec 2), type "read_aloud", format "read_aloud-v1"
├── data.sqlite3
│   ├── read_text      # uuid PK, dataset_uuid, order_num, title, note, content,
│   │                  #   best_score, created_at, updated_at
│   └── read_attempt   # uuid PK, text_uuid FK, user_id, audio_path, recognized,
│                      #   score, created_at
└── media/             # one .wav per attempt, named <attempt uuid>.wav
```

`src-tauri/src/datasets/read_aloud.rs` is the module; it is always compiled (no feature gate), so the whole practice loop — record, transcribe with a loaded STT model, score, submit — works on the Android thin client, which is the primary device for speaking exercises. `audio_path` is dataset-directory-relative (`media/<uuid>.wav`), the book/read-aloud convention, not dictation's media-relative one. The schema is re-applied with `CREATE TABLE IF NOT EXISTS` on every open, so a folder copied from an older build heals itself.

`note` carries provenance (source, register, pronunciation hints); `best_score` is a stored roll-up of the text's attempts, and `attempt_count` is computed on read rather than persisted — the same "counts never live in info.json" rule the descriptor contract enforces, since attempts are content churn inside the database the journal owns.

## Scoring

An attempt is scored by `src-tauri/src/datasets/asr_metric.rs`, a word-level ASR metric adapted from [rwer](https://github.com/ryaningli/rwer) v0.2.2 (MIT, vendored down to ~400 lines: no transform pipeline, no CLI, no `rapidfuzz`). The design question this answers is "how faithfully was *this text* read", which is a reference-vs-hypothesis comparison — the same problem speech recognition evaluation solved decades ago, so the metric family is the standard one, not an invention:

1. **Normalize** both sides identically: NFC → lowercase → fold `ß → ss` → strip Unicode punctuation → split on whitespace into word tokens. Each step carries a German rationale: NFC because a composed and a decomposed umlaut must be the same word (and because upstream rwer normalized only the character path, letting the word path count `Möbel` ≠ `Mo\u{0308}bel` as an error); the `ß` fold because a Swiss or South-German reader who says "Strasse" against a reference "Straße" made no error; punctuation stripping because STT adds and drops commas at will, and `„“«»…` are not speakable.
2. **Align** the two token sequences with Levenshtein: a single-row DP computes the distance, then traceback runs a *banded* Wagner–Fischer restricted to a diagonal band of width `2·dist + 1` — O(M×D), which matters because a competent read of a 200-word text has a distance near zero while the sequences are long. The alignment yields H (hits), S (substitutions), D (deletions), I (insertions).
3. **Score** with WIP (Word Information Preserved): `score = (H/N) · (H/(H+S+D+I)) × 100`. The multiplicative form is deliberate — omissions and substitutions compound, so skipping 40% of the text lands around 36, not in the pass band. `wer` and `mer` ride along in the result for agents and future calibration.

The whole picture — score, rates, counts, and the **alignment chunks** (consecutive same-op runs carrying the words from both sides) — is one serializable `AsrScoreSummary`. Chunks exist because a bare percentage is not feedback: the UI and the MCP both consume the *same* alignment that produced the number, so the highlighted diff can never disagree with the score. (Upstream rwer's `EditOp::Equal` carried only a reference index, which breaks whenever the sequences shift — "a b c" vs "a c" matches `c` at ref 2, hyp 1. The port records both indices; a test caught this on the first run.)

Two historical notes. The metric started as Ratcliff-Obershelp similarity (`textsim::similarity_score`), which was designed for cue *alignment* anchoring and leaked two of its heuristics into read-aloud scoring: a flat-90 containment branch and an ×80 scale. That is why the modules are now split for good: `textsim.rs` stays the alignment similarity (its longest-common-block semantics is what the cue anchor DP needs — WER is not a drop-in there), and `asr_metric.rs` owns transcript scoring only. And because stored `score` values predate the metric switch, `list_texts` and `list_attempts` **recompute on read** from the persisted `recognized` text — old-scale numbers never surface, nothing was migrated, and the stored column remains a write-on-submit cache that converges as new attempts arrive.

## Submission, XP, and the record

`read_aloud_submit` is the single atomic path: the frontend records via `useRecorder`, transcribes via `model_transcribe`, and hands the backend base64 WAV + transcript; the backend scores, writes the WAV and the attempt row, rolls `best_score` forward, and awards XP in one call. XP follows the scaled pattern: +1 at score ≥ 60 (`<text_uuid>:pass`) and +1 more at ≥ 80 (`<text_uuid>:good`), deduped per text in the XP ledger — repeating a read you already passed never farms, but improving into the next tier earns the extra point. An `xp-earned` event fires with the combined total.

The thresholds are the ones inherited from the old metric. WIP is stricter — an honest near-perfect read scores ~92 and lands comfortably above them — but they are calibration candidates, not constants, and recomputing history makes recalibration a display-only change.

Texts are ordinary CRUD (`read_aloud_list_texts` / `_save_text` / `_delete_text`, with the dataset itself list/create/update/delete'd through the shared trash machinery). Deleting a text cascades its attempts and removes their WAVs directly — row-scoped deletes are *not* trashed, per dataset.md's rule (and its known rough edges).

## Sync and the thin client

Rows journal through the common `commit_change` choke point as `read_text_save` / `read_text_delete` / `read_attempt_save` (`save_text_row` / `save_attempt_row` are shared by the commands and both sync apply paths), so an attempt recorded on a phone converges on the hub exactly like one made there — including the `best_score` roll-up. The WAV is not part of the journal: it is an ordinary file in `media/`, moving by content hash and snapshot, which is why only `audio_path` is journalled and each device re-resolves `audio_url` on read. Attempts keep the submitting `user_id`; a follower apply rewrites it with the identity the hub bound the device to, like dictation progress does.

## The agent-facing surface

Every command has an MCP twin in `mcp/books.rs` (11 `read_aloud_*` tools). `read_aloud_score` is the pure preview — reference text and transcript in, structured `AsrScoreSummary` + `passed` out — so an agent can judge a transcript without touching storage, and `read_aloud_submit` returns the summary inside its result. The chunks make an agent's feedback loop as informative as the UI's: it can tell the learner *which* words were misread, skipped, or added.

The align plane has its own note, [read-aloud-align.md](./read-aloud-align.md): a folder of recordings in, one span per text out, and the rules that follow from that (a span is the truth, a clip is a cache of it, and a save never overwrites one).

## Frontend

`src/components/read_aloud/`: `ReadAloudPage` (dataset selector, text list with score badges, management), `PracticePanel` (record → transcribe → submit, result, attempt history with playback), `TextEditor` and `DatasetDialog` (modals). The diff under a result is `AsrDiff`, a pure renderer for backend chunks — substituted and inserted words go red with the reference word in the tooltip, skipped words render struck through. The older client-side LCS highlighter (`books/diff.tsx`, with its fake-space "missing" placeholder) now serves only the book `SentenceDrawer`; unifying it onto `asr_metric` chunks is an open follow-up.

## Known rough edges

- **Normalization has a loss edge.** Hyphen stripping joins "Eisen-Bahn" to "Eisenbahn" (desirable), but so does "don't"-style apostrophe loss for English datasets, and `ß→ss` is one-way — a transcript "Straße" and reference "Strasse" fold to the same token, which is intended, but so do genuinely distinct words in edge cases ("heißen" vs "heissen" — same word anyway; a real collision would need a typo like "ss" where "sß" was meant, which normalization cannot and need not catch).
- **Word order is not credited.** WER punishes a re-ordering of correct words as S+D+I, which is the standard ASR stance and arguably right for read-aloud; nothing yet measures fluency (rate, pauses) — the audio is kept precisely so that could be built later.
- **Numbers and abbreviations are surface-mismatched.** A reader who says "zwei Tausend" against a reference "2000" is counted wrong, because normalization folds form, not meaning. The fix is the normalized *field* on `read_text` (numbers spelled out, abbreviations expanded): scoring reads `COALESCE(normalized_content, content)`, and a candidate comes from one shared prompt pressed through a local model, the clipboard, or an agent — then measured against the user's own transcripts (`read_aloud_normalize`, `read_aloud_normalize_measure`) before it is kept.
- **Thresholds predate the metric.** See above; recalibrate against real attempt data now that history is re-scorable for free.
- **SentenceDrawer still diffs client-side.** Two diff renderers exist in the app while the book page keeps the LCS one; they can disagree with each other even though only the read-aloud one can disagree with a *score*.
