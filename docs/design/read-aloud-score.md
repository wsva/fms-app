# Read-Aloud Scoring — Accuracy, Fluency, Pronunciation

> Status: **implemented** (accuracy · fluency · pronunciation, with the pronunciation axis live once its model is downloaded from the Models page — see [Decision log](#decision-log)). Supersedes [read-aloud-score-claude.md](./read-aloud-score-claude.md), which specified a standalone CLI workspace; the scoring design survived, the delivery vehicle did not. The v1 practice loop this extends is described in [dataset-read-aloud.md](./dataset-read-aloud.md).

## Terminology

Three words, one meaning each, used identically in the code, the UI and these docs:

| Term | Means | Lives in |
|---|---|---|
| **attempt** | One submission: the recording plus its transcript plus the three-axis assessment | `ReadAttempt`, `read_attempt_event` (app DB) |
| **recording** | The WAV bytes of an attempt, device-local | `<workspace>/read_aloud_audio/<attempt_uuid>.wav` |
| **standard audio** | A reference clip the *dataset* owns — what pronunciation is measured against | `read_standard_audio` (dataset DB) + `media/<text_uuid>/<audio_uuid>.wav` |

"The take" was film jargon used only in prose; every identifier already said attempt. It is retired, because in a read-aloud dataset "audio" could plausibly mean either the reference or the reader's own recording — the distinction the three terms above exist to keep.

## What exists today

Read-aloud scores one thing: how faithfully a transcript reproduces the reference text. `asr_metric::score_asr` normalizes both sides through a single `normalize_tokens` seam (NFC → lowercase → `ß → ss` → strip punctuation → split on whitespace), aligns them with a banded Wagner–Fischer traceback, and reports `score = WIP × 100` plus `wer`, `mer`, H/S/D/I counts and the `AsrChunk` runs the UI highlights. `read_aloud_submit` stores that number on a `read_attempt_event` row and awards XP at 60 and 80 (`XP_PASS_SCORE` / `XP_GOOD_SCORE`), and `list_attempts` recomputes it from the persisted transcript on every read, so the metric can change without a migration.

That number answers "did you say the right words". It cannot answer "did it sound right" or "did you read it well", because the input is text only: `model_transcribe` returns a `String`, the recording is handed to the backend as base64 and filed away, and nothing in the pipeline ever looks at time or at the signal. A learner who reads haltingly, restarts, or mangles a vowel gets the same 100% as one who reads it cleanly.

## What this stack can actually measure

`models/mod.rs` documents the capability table for the vendored `transcribe-rs` 0.3.11 — it is the constraint everything below is built around:

- **Word timestamps: Parakeet only**, via `ParakeetParams { timestamp_granularity: Some(TimestampGranularity::Word) }`. Whisper (whisper.cpp/GGML), Moonshine, SenseVoice, Canary, GigaAM and Cohere expose segment-level timestamps only, and the segment boundary is a chunk of audio, not a word.
- **No logits, no logprobs, no decoder handle, for any backend.** Teacher forcing the reference through the decoder (what the Claude document's `AsrBackend::score_reference` assumed) is not reachable from this API, and neither is CTC Viterbi over the ASR model's own posteriors. Any pronunciation score that depends on the ASR model's internal probabilities would require forking or replacing `transcribe-rs`.
- **Edge TTS already returns per-word boundaries and the app throws them away.** `edge_tts_rust::SynthesisResult` carries `boundaries: Vec<BoundaryEvent>` with `offset_ticks`, `duration_ticks` and the word text, because every call site asks for `Boundary::Word`; `synthesize_to_bytes` returned only `result.audio`. It now delegates to `edge_tts::synthesize_with_boundaries`, which returns both — the TTS page and the web service are unchanged.

Two consequences shape the whole design. Fluency is buildable today for Parakeet users and must degrade honestly for everyone else. Pronunciation needs a model whose posteriors we *can* read, which means a second, purpose-built model — not the STT model.

## Decisions

**D1 — Three axes, one of them already shipped.** Accuracy is the existing WIP score, untouched: same code path, same number, same XP gate, same stored column. Fluency and pronunciation are added *beside* it, never folded into it. The alternative (a blended 0–100) was rejected because the weights would be invented: there are no teacher ratings in this project to fit them against, and a blended number would silently redefine what "passed" means for every historical attempt.

**D2 — Fluency comes from the audio, not from ASR gaps.** Word timestamps are imprecise exactly where pauses live, so a confidence-free gap measurement is worse than useless. Pauses are detected on an energy VAD over the raw PCM, and ASR word times are only used to say *which* words surround each pause, clipped into the VAD segments. A gap that sits inside a VAD speech segment is ASR noise, not a pause.

**D3 — Pronunciation is a CTC phoneme model scored against a TTS-derived phone timeline.** GOP needs the expected phone sequence, and three ways to get one were considered. *Teacher forcing* the STT model is impossible (above). *espeak-ng G2P* is a C library with an NDK build, plus a lexicon per language, plus a phoneme-inventory mapping — a large amount of native build surface for one scoring axis. *Per-word DTW against TTS audio* measures similarity to a specific voice rather than correctness, and does not survive a change of speaker. The chosen route removes both problems: synthesize the reference with Edge TTS, decode it with the same phoneme model, and take the CTC-collapsed decode as the target phone sequence, grouped per word by the TTS boundary times. The target is then independent of the learner (it is what a canonical speaker actually said), needs no G2P, and works for any language Edge TTS speaks. The cost is one synthesis per text, stored in the dataset and reused until the text changes — see [Standard audio](#standard-audio-the-reference-the-scorer-reads).

**D4 — Derived and capped, in the app database.** An attempt's metrics are recomputed from the recording on demand; only a compact aggregate (~2 KB: rates, counts, pause count, the twelve weakest words, plus the capability flags that produced them) is stored on `read_attempt_event`. Per-frame posteriors, per-phone GOP and full word timing arrays are never persisted: they are large, device-local, and exactly reconstructible from the WAV that is already kept.

**D5 — Unavailable is a state, not a proxy.** If the phoneme model is not installed, the text has no standard audio (or the clip predates the current text), or the recording is too short to align, `pronunciation.unavailable_reason` says so and the card says "not measured". Substituting ASR word confidence would produce a number that looks like pronunciation and is not, and a plausible fake is worse than an honest blank in a learning tool.

**D6 — `datasets/read_aloud/` becomes a module directory.** The file is 1569 lines and this work adds five concerns to it (VAD, fluency, GOP, phoneme session, TTS reference). Splitting keeps the pure logic testable without audio or models, which is the property that makes the thresholds safe to change. The directory later gained `standard_audio.rs` beside those six.

**D7 — No composite score, no XP change, no protocol bump.** The XP gate stays on accuracy so the ledger's meaning is stable. `metrics_json` is an additive optional column on an already-journalled row, so old peers ignore it and no `PROTOCOL_VERSION` change is needed.

## Data flow

```
WAV (16 kHz mono, straight from the recorder)
  ├── energy VAD ─────────────► speech segments
  ├── model_transcribe_detailed ─► text + TimedWord[]   (Parakeet: per word; others: text only)
  └── phoneme model ──────────► per-frame log-posteriors
reference text
  └── normalize_words ────────► norm words + punctuation per word
standard audio (the text's default clip)
  └── phoneme model ──────────► reference phone sequence, grouped per reference word
        (grouped phones cached in <workspace>/read_aloud_phones/)
                                    │
   CTC forced alignment of that sequence onto the learner's posteriors
                                    ▼
                     per-phone GOP ─► per-word pronunciation score
accuracy alignment (reference ↔ transcript) ─► verdicts, WIP (unchanged)
fluency (word times clipped into VAD segments) ─► pauses, rates
                                    ▼
                              Assessment (serde)
```

The pure half — normalization, alignment, classification, VAD, pause detection, rate arithmetic, forced alignment and GOP arithmetic — never touches a model or a file. Only `phoneme.rs` (session), `standard_audio.rs` (the dataset's clips) and the reference-phone cache are effectful, and `assess.rs` is the one function that composes them, so every threshold remains testable against hand-built numbers.

## Storage and sync

`read_attempt_event` gains one nullable column, applied idempotently in `ensure_attempt_schema` the way `normalized_content` was added to `read_text`:

```sql
ALTER TABLE read_attempt_event ADD COLUMN metrics_json TEXT
```

`attempt_payload` adds the field to the journal payload and `apply_remote_save` reads it with the same tolerant `get()` helper it uses for the other keys, so a peer on an older build simply ignores it. Word-level and phone-level arrays stay out of the payload on purpose: the journal is sync traffic, and a per-frame matrix would multiply it for data that is device-local anyway.

### Standard audio: the reference the scorer reads

The reference is no longer synthesised during scoring. It is an asset the **dataset owns**: any number of clips per text, exactly one of them the default, described by a `read_standard_audio` row and stored as `media/<text_uuid>/<audio_uuid>.wav` (16 kHz mono — the phoneme model's input rate). The scorer reads only the default clip, and only from disk.

```sql
CREATE TABLE IF NOT EXISTS read_standard_audio (
    uuid        TEXT PRIMARY KEY,
    text_uuid   TEXT NOT NULL,
    note        TEXT NOT NULL DEFAULT '',   -- free-form: origin, voice, how it was cropped
    boundaries  TEXT,                       -- TTS word timings; NULL ⇒ not a synthesis
    text_sha    TEXT NOT NULL DEFAULT '',   -- sha256 of the reference text it was made from
    is_default  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    origin      TEXT NOT NULL DEFAULT '',   -- tts | upload | align ('' on a pre-column row)
    source_uuid TEXT,                       -- align only: the recording it was cut from
    start_secs  REAL,
    end_secs    REAL
);
```

The table carries only what cannot be derived. The row's identity *is* its path (so row and file cannot disagree); the duration is decoded from the wav when a text's clips are listed; and `note` is the single free-form field for provenance, never parsed. `text_sha` covers the *effective* reference (`normalized_content` when set, else `content`), which turns staleness into a comparison instead of a migration. `origin` names what made the clip; a row written before the column existed leaves it empty and is read by the old rule — only a TTS clip carries word boundaries — so a migrated dataset needs no row rewritten.

### Span first, file second

An aligned clip ("this text is read from 31.51 s to 44.58 s of *that* recording") stores the **span**, not the audio: `source_uuid` + `start_secs` + `end_secs`, with `media/<text_uuid>/<uuid>.wav` as a **cache** of those seconds. Applying a whole book is therefore a few hundred row writes instead of hours of decoding, a text nobody practises costs no disk, and an edge can be corrected numerically without touching audio.

The cache is cut in exactly three places, each a moment where the audio is about to be needed:

| where | why there |
|---|---|
| a **text-scoped** listing (`read_aloud_standard_audio_list` with a `text_uuid`) | the page opened the text to play or score it — this is the normal path |
| `pack_archive` / the REST `/snapshot` handler | the recordings stay where the user keeps them and `align/` is a derived index that never ships, so a follower could not cut the clip itself |
| never in the **dataset-wide** listing, and never in the scorer | badging a list is not worth decoding a book, and scoring is a read path that must not write into a dataset |

A read-only base under the download root is never cut into (that would copy the dataset just to draw a player); its own package was materialised when it was published. The `/snapshot` handler materialises *before* it hashes, and the follower records the hash that response carries rather than the earlier `/manifest` one — otherwise the first sync after an apply would re-download the dataset once to discover it already had it.

One default is enforced on write (the first insert for a text becomes default; `set_default` clears the others in one transaction; deleting the default promotes the earliest remaining) and tolerated on read (`ORDER BY is_default DESC, created_at, rowid`, then the first row that can be read) — a sync replay must never fail on a partial unique index. `usable` is "the wav is here, or the recording it names is", so a span clip is scorable before its cache exists and a clip whose recording has gone is reported as not usable rather than as missing audio.

Why not keep synthesising on demand into a cache: a score must not depend on an HTTP request, and a clipped or mispronounced TTS rendering must be replaceable by a human reading. The old `<workspace>/read_aloud_tts/` cache is retired; its files are left on disk as derived data.

### Reference-phone cache

Decoding the clip through the phoneme model is the expensive half of the reference pass, and it produces the same answer for every attempt at the same clip with the same model. So the *grouped target* — `Vec<Vec<usize>>`, the phone ids of each word, a few KB — is cached in `<workspace>/read_aloud_phones/<digest>.json`. Nothing downstream touches reference posteriors after grouping, so this is the whole reusable result, and a repeat score runs the model **once** (learner only) instead of twice.

The key is `sha256(model_identity ‖ sha256(clip PCM))`, with `model_identity` the phoneme directory's name plus `model.onnx`'s length and mtime — a re-crop, a regenerate or a different model each self-invalidate, so there is no invalidation logic to get wrong. Workspace, not dataset: scoring is a read path and must never write into a dataset.

### Inference budget

The phoneme model is a transformer: its self-attention is quadratic in frames (~50 per second at 16 kHz), so a single call over a five-minute recording asks ONNX Runtime for ~20 GB and the assessment dies with `Failed to allocate memory` — which to the learner looks like a hang. `PhonemeModel::posteriors` therefore normalises the utterance once and runs it in 20 s windows with 2 s of overlap, dropping the repeated frames after each window, so memory is bounded by the window rather than by the recording. On Windows the session is offered to DirectML first (the DirectX 12 provider the shipped ONNX Runtime already contains), falling back to the CPU — with a log line saying so — when the driver cannot host the graph: a GPU may cost nothing but speed. The provider, every window and the frame count are logged, and the attempt page's assessment log shows them live.

### Standard audio and sync

The clips ride the existing dataset channels for free. `read_standard_audio_save` / `_delete` join the dataset-scoped change kinds beside `read_text_save` / `_delete` (object id = uuid, delete → tombstone); the hub's snapshot is a whole `data.sqlite3` copy and `sanitize_snapshot_db` drops only `card_review` + `read_attempt`; and the wavs travel as ordinary files, hashed into `compute_manifest` and fetched by hash. Deleting a text cascades: its rows go with it, and `media/<text_uuid>/` is removed.

One measured addition to D4: a stored pause costs ~48 bytes (`{"start":12.04,"end":12.31,"kind":"within_phrase"}`) and the aggregates ~520, so `MAX_STORED_PAUSES` is **24**, not 40 — 40 would put a verbose attempt at ~3 KB and break the row budget. Raising it again needs a narrower pause shape, not a bigger number.

A span clip's wav is *derived*, so a follower may legitimately hold the row and not the file: it materialises the first time the text is opened. That is why the pack and snapshot paths must cut every missing span before the file walk — the source recording is not in the package, so nothing on the receiving side could cut it later.

## Scoring definitions

| Quantity | Definition | Default |
|---|---|---|
| `accuracy.score` | WIP × 100, unchanged: `(H/N) · (H/(H+S+D+I))` where H counts exact matches only | — |
| `lenient_score` | Same shape, with `minor` verdicts counted as hits | — |
| `minor` | A substitution that is a known homophone, a ≤1-character difference in a word of ≥5 characters, or the same number in another form | equivalence list |
| `repeated` | An insertion that repeats the preceding 1–3 words (a restart) | — |
| `wcpm` | Correct words ÷ reading span in minutes | — |
| reading span | First word start → last word end, so fumbling for the record button is not reading time | — |
| `speech_rate` | Spoken words ÷ reading span | — |
| `articulation_rate` | Spoken words ÷ speaking time (reading span minus pauses) | — |
| pause | Silence ≥ `min_pause` between two VAD segments, or a gap the VAD does not call speech; leading/trailing silence is reported separately and excluded from rates | 0.25 s |
| VAD | 20 ms frames; noise floor = 10th-percentile frame RMS, ceilinged at half the 90th percentile so a speech-only recording is not read as silence; threshold `max(min(3 × floor, 0.5 × loud), 0.004)`; 2-frame hysteresis; ≥ 150 ms speech; ≥ 120 ms gap to split; 40 ms pad | as stated |
| `long_pause` | Long enough to be a break rather than a breath | 1.0 s |
| `within_phrase_pause_ratio` | Pauses not aligned with reference punctuation ÷ all pauses | — |
| `mean_length_of_run` | Words between pauses | — |
| `gop(phone)` | Mean over the phone's aligned frames of `log p(target) − log max_q p(q)` | — |
| `pron_score(word)` | `exp(gop / τ)` clamped to `[0,1]`, duration-weighted across the word's phones | τ = 2.5 |
| `weak_ratio` | Share of scored words below 0.4 | 0.4 |
| band | `wcpm` against a configured slow/target/fast table | heuristic |

`exp(gop/τ)` was chosen over a fitted sigmoid deliberately: it is monotone, bounded, needs no calibration data to be *meaningful*, and one constant is easy to revisit once attempt history exists. The band table is labelled a heuristic wherever it surfaces — it is not a published reading norm, and pretending otherwise in a language-learning app would be the more damaging mistake.

## Configuration

`FluencyConfig` (`fluency.rs`) holds every fluency threshold in one struct with documented defaults; it is *not* serialized — the assessment stores the numbers it produced, and a tuning change applies to the next attempt rather than rewriting history. `VadConfig` (`vad.rs`) does the same for the detector. `GOP_TAU` and the band edges are constants in `assess.rs` / `FluencyConfig`, deliberately not settings: they describe the metric, not the material, and a per-user τ would make two learners' scores incomparable.

**Which voice a clip is made in** is decided in one place — `standard_audio::resolve_voice` — and every generation passes through it, so the four layers cannot disagree. First match wins:

| # | Source | `voice_source` |
|---|---|---|
| 1 | the `voice` parameter on this one call | `param` |
| 2 | `tts_voice_by_language[<dataset language>]`, exact tag first (`de-DE`), then the language prefix (`de`) | `preference` |
| 3 | `read_aloud_tts_voice` (workspace-scoped, empty = `read_aloud::DEFAULT_TTS_VOICE`, `en-US-EmmaMultilingualNeural`) | `setting` |
| 4 | `DEFAULT_TTS_VOICE` | `default` |

Step 2 is what ties a voice to a *language* instead of the alphabetical order of `tts_list_voices`, which is how a German book once got an Austrian narrator. The map is workspace-scoped for the same reason `read_aloud_tts_voice` is: it is a reading preference that travels with the workspace, not with a dataset. It is edited on the **Edge TTS** page, not on the read-aloud page, because `tts_synthesize` speaks too and that page is where voices are already listed and auditioned.

What *is* per dataset is `DatasetInfo.language` — content metadata, set at `read_aloud_create` / `read_aloud_update`, sync-visible, and the only thing step 2 can key on. A dataset with no `language` reports that fact instead of silently taking the workspace default: `resolve_voice` returns a `hint` saying so, because a missing language is the one failure a preference can never fix.

A resolution also reports one `warning`: when an explicit voice's locale speaks a different language than the dataset. A German book with an Austrian narrator is a legitimate choice and an accidental one is not, so the difference is surfaced rather than refused. Every pair written into `tts_voice_by_language` is validated at the two settings write seams (`SettingsState::save` / `save_workspace`), so `settings_set {"de": "en-US-AriaNeural"}` is rejected with the voice list that would have been valid — the same trap one layer up.

A clip carries the voice it was made with in its `note`, and changing a preference never re-voices an existing clip: the note is the record of what the file says, and the only way to change it is to regenerate.

## Commands and MCP surface

- `read_aloud_assess(dataset_uuid, text_uuid, content, recognized, wav_base64, words?)` — preview: `Assessment` out, nothing persisted. The twin of `read_aloud_score` for the audio side, so an agent can evaluate an attempt it recorded. The two uuids are required because pronunciation now needs the text's standard clip.
- `read_aloud_standard_audio_assess(dataset_uuid, text_uuid)` — the reference's own three-axis self-test: the text's default clip is transcribed with the loaded STT model, scored against the reference text (accuracy), paced from its own waveform (fluency, when the model gives word timings) and aligned against phones derived from itself (pronunciation). Nothing is persisted and no XP is awarded; it exists because a clip that does not say what the text says would mark down every learner who reads it correctly, and that is cheaper to catch on the clip than on the history. Surfaced as the **Standard audio check** card above the attempts list.
- `read_aloud_submit(..., words?)` — unchanged contract, plus the `Assessment` in its result and `metrics_json` on the stored attempt. XP and thresholds are untouched. It loads the text's default clip beside the content.
- `read_aloud_rerecognize(..., words?)` — recomputes the assessment from the device-local recording, since a new transcript changes accuracy and may change which words line up with which pauses.
- `model_transcribe_detailed(wav_base64)` — the recorder's final STT call: `{ text, words[], capabilities: { word_timings } }`, so the page knows before rendering whether a fluency card is possible. `model_transcribe` is untouched for the live preview, voice input and every other caller. It resolves the user's *default* model when none is loaded (the idle timeout unloads one), so an attempt is never unassessable just because the model was cold — and it never swaps out a model that is already loaded. A Parakeet take longer than the model's window is transcribed in the same 240 s / 15 s-overlap chunks the word-level file path uses, with the word times offset per chunk and the overlap de-duplicated; the text is rebuilt from the surviving words so it stays token-for-token aligned with the timings.
- `read_aloud_list_attempts` — attempts carry their stored `metrics`, so the history view needs no second call.
- `read_aloud_normalize(content, model?, temperature?)` — the one prompt that spells a text out into its spoken form, plus the candidate when `model` is named. Called without a model it returns the prompt alone, which *is* the clipboard route: the same words go to a provider the app cannot reach. Nothing is persisted — `read_aloud_save_text` is still the only writer.
- `read_aloud_normalize_measure(dataset_uuid, text_uuid, reference, candidate)` — the gate: the best score the reference in use today achieves versus the candidate's, over the user's own transcripts of that text, with `attempts` saying how many were compared. `reference` is passed in rather than read, so a draft can be measured before it is stored.
- `read_aloud_standard_audio_list(dataset_uuid, text_uuid?)` / `_generate` / `_import` / `_probe` / `_set_default` / `_delete` — the clips themselves. `_list` without a `text_uuid` returns the whole dataset's clips in one call, which is what badges the texts list; `_generate` synthesises the effective reference through Edge TTS and keeps the word boundaries; `_import` takes a picked file (a path, or base64 with a name on Android where the picker yields a `content://` URI) and stores only `[start, end)`; `_probe` decodes a candidate without storing it so the crop track has peaks. Each clip carries `stale` and `usable`, so a caller can decide without guessing.

`words` is the one piece of evidence that is *not* gathered on the backend: it is the STT output the frontend already produces, passed in the same way `recognized` is. That keeps `read_aloud` free of the `stt`-gated `models` module, and it means a peer replaying the journal needs nothing extra.

Per the agent-friendly rule, all of them return structured JSON and state what is missing rather than failing opaquely.

## Frontend

`PracticePanel` renders one result as three blocks, side by side on a wide screen and stacked on a phone: Accuracy (the existing badge and XP copy, unchanged), Fluency (correct words/min with a band chip, articulation rate, pause counts, words between pauses), and Pronunciation (score, the three weakest words, or "not measured" plus the reason). `AsrDiff` gains an optional per-reference-word score map so a word that was heard correctly but pronounced weakly gets an amber underline — `ref_index + offset` inside an equal run, so the diff logic is untouched. `AttemptsPanel` shows the same three blocks from `metrics_json` for any stored attempt, plus a pause ribbon drawn from the capped pause list, and its "recognize again" now re-runs `model_transcribe_detailed` so a re-scored attempt keeps its fluency. The same action sits on the attempt detail page's toolbar beside Play/Delete, and both entry points go through `lib/read_aloud/rerecognize.ts`, so the rule "re-recognition never swaps the active model" has one home rather than two.

The practice toolbar's **Manage** button opens the text page — one page, not a dialog, and the only editor a text has. It is four cards rather than one column of boxes, so the page reads as a list: **Details** (title, note), **Text to read**, **Scoring reference** (the normalized text and, under it, a *Scoring will read* line that names which of the two boxes the scorer will use and previews it — the one question the two boxes cannot answer on their own), and **Standard audio**. Every card is always open, and a **Sections** list beside them — the leftmost control in the sub-toolbar — scrolls to one and highlights the card being read as the page scrolls. On a desktop the list is a split column that starts open and is drag-resizable; on a phone it is the shared drawer, closed until asked for. The header names the text and carries its storage state: `unsaved changes` while the draft runs ahead of the stored text, a green `Saved` for two seconds after a save — which is what makes staying on the page legible now that Save no longer closes it.

The list is a table of contents, not Settings' filter — a deliberate difference. The four cards are one draft, and the audio card's "paused until saved" note only reads beside the fields it is about, so hiding the chosen card's siblings would answer a question nobody asked and would leave Save acting on fields out of sight. Folding cards away was the other alternative and went with this change: a collapsed card would make its own entry in the list a dead click. The column also carries blank space below the last card, measured as that card's shortfall from the viewport height and re-measured when the column is resized or the audio card grows with its clips: a jump that pins a card to the top needs the page to be able to scroll past it, and only the last card can be short of that.

The first three cards are one draft, saved or discarded together by the sub-toolbar's Save/Close (mirroring the wiki page); the clips are immediate. A clip describes the *saved* text, so while the draft is dirty the audio card stays on the page but disables every control that would add, choose or delete one — playing and reading stay live, and the card says why. Hiding it was the alternative and was rejected: a section that vanishes makes the page change shape under the cursor, and it hides the `Stale` chip that is often the reason to look. A brand-new text has no row to hang a clip on and says so instead.

The **Scoring reference** card offers two ways to fill its box, as buttons: *Generate with Ollama* (the app's chat seam, so the configured provider wins over the label) and *Copy prompt*, plus a *Measure* action for a candidate that came back by hand. The chosen model is remembered per device (`read_aloud_normalize_model`); when Ollama is unreachable generation is disabled and copying carries on, because the prompt needs nothing installed. The prompt itself lives in `normalize.rs`, not beside the buttons, for the reason the tools list implies: the button, the clipboard route and the MCP tool all have to send the same words or their numbers are not comparable. It is deliberately a spelling transform — numerals, ordinals, dates, currency, percentages, units, math symbols and abbreviations to spoken form — and explicitly forbids rewording, because punctuation and case are already ignored by the metric while a paraphrase scores worse. Every candidate is then measured against the reference in force and the text's own recordings, and the note says which of three things happened: `61.0 → 88.0 (+27.0). Worth keeping`, `no change — the mismatch was not spelling`, or `worse than what scoring reads now`, with a fourth answer when there is nothing to measure. The gate advises; the box stays an ordinary draft and Save is still the only writer, so a wrong candidate cannot become a stored reference by accident.

Clips are radio rows: the circled one is the clip in use, so the same control is both the state (`in use`) and the action (choose it) — no Default badge duplicating a *Set default* button. Each row carries its kind, duration, a `Stale` chip and an alert when the file is not on this device, with Play and Delete at the right. The empty state holds the two CTAs rather than sitting above them; *Generate with Edge TTS* is the only filled button, since it is the one action that produces a scorable reference, and a progress line names the voice while it runs instead of leaving an 11 px spinner. The voice is two pickers, language then voice, because one list of every locale's voices is a scroll, not a choice: they open on the configured voice's own language, the empty language means *the configured voice* itself, and every control in the row — selects included — shares one height, since a select a few pixels taller than the button beside it reads as a mistake. Upload shows a trim track: the picked file's peak envelope with two drag handles, the selection shaded, a minimum of 0.3 s, *Use whole clip* and Play-selection. The track also carries a playhead — placed by clicking or dragging the waveform, following playback — because *Set start* and *Set end* need a timestamp to take: each edge has its own group, `[◀][Set start][▶]`, and the arrows move that edge 100 ms at a time. A handle under a pointer is a tenth of a second at best, and the groups are the only precise way to crop on a phone, where the bytes have no playable path and so no playback to follow. Both edges are also fields: the selection line reads *Selection `0:12.3` – `0:45.6` (33.3 s of 1:02.0)*, the two times can be typed (`1:02.5`, `62.5`, or `62,5` for a German keyboard), Enter or blur commits, Escape gives up, and a draft that does not parse turns red and is reverted when left — the value is the only way to say "this edge, exactly", short of dragging at 2× zoom. Guessing the span is the one thing the waveform cannot do: a recording that holds more than the text has no edge in its shape to find, so *Guess from speech* transcribes the whole file with the loaded STT model, aligns the transcript to the text through the same preprocessing scoring uses, and keeps the longest run of matched words — alignment alone is not a span, because a common word heard elsewhere would stretch it across most of the file. It refuses rather than guesses when too little of the text was heard, or when the text's beginning or end was not heard at all, since a fragment offered as the whole reading is worse than no answer: it looks like one. Nothing is written — the edges move, and every other way of setting them still works — and the note reports how much of the text was heard, because an eleven-of-twenty match is a suggestion, not a result. The texts list marks a text that has a usable clip with an `AudioLines` badge, sourced from one dataset-wide `read_aloud_standard_audio_list` call — nothing is added to `ReadText`, so the sync payload is untouched.

## Degradation matrix

| Condition | Accuracy | Fluency | Pronunciation |
|---|---|---|---|
| Parakeet + phoneme model + a standard clip | full | full | full |
| Non-Parakeet STT model | full | hidden (`word_timings: false`) | full (does not need word times) |
| `phoneme` feature compiled out (`--no-default-features`) | full | full | "not measured — model missing" |
| Phoneme model not installed | full | full | "not measured — no phoneme model" |
| Text has no standard audio | full | full | "not measured — generate a standard clip for this text first" (`no_reference`) |
| The clip was made from an earlier version of the text | full | full | "not measured — the clip is stale, regenerate it" (`stale_reference`) |
| Recording shorter than the alignable minimum | full | pauses only | "not measured — too short" |
| Silence or no speech detected | score 0, as today | hidden | "not measured" |

The one asymmetry worth stating: pronunciation does not depend on `word_timings`, because the forced alignment produces its own word spans from the reference phone sequence. That is why a Whisper user can still get pronunciation and cannot get fluency. It does depend on the dataset: the clip must exist on this device, which sync delivers with the rest of the dataset's files. An already-synced clip whose wav has not arrived yet reads as "no standard audio", never as a wrong score.

The model that answers is whichever one is *loaded*, which is a live choice on the Models page rather than the persisted default. Loading `whisper-small` and then re-recognizing therefore produces an honest "not measured" for fluency, and the card names the fix (load a Parakeet model) instead of leaving the learner to guess.

## Known limitations

- A TTS mispronunciation becomes a target error, and the learner is marked down for saying it correctly. Rare, but real, for proper nouns and loanwords — which is why a clip is now a dataset asset a human reading can replace (D19), rather than something synthesised unseen.
- GOP on accented speech is unvalidated here. Foreign-accented deviations are precisely what GOP is worst at separating from errors without a large labelled corpus.
- `τ` and the band table are heuristics. Nothing in this project has teacher ratings, so nothing is calibrated; both are one constant and one table to revise later.
- Fluency is Parakeet-only, which on a dataset read with a Whisper model means the fluency card never appears.
- Long Parakeet takes are cut into overlapping chunks, so the language is chosen per chunk rather than for the take: a chunk that starts on a pause can still land on the wrong language. Before this, audio past the model's window was fed to the encoder in one pass and collapsed wholesale — a whole German take came back in English — so per-chunk choice is the smaller failure, not a new one.
- Pronunciation is reported per word; the per-phone detail the scorer computes is surfaced only inside a feedback report ([read-aloud-feedback.md](./read-aloud-feedback.md)), never on the assessment card itself.
- The phoneme model is a second, optional download (**603 MB**, fp16). It is not on the critical path — every other feature works without it — and it is installed from the Models page's **Pronunciation** section (catalog id `Hochien/wav2vec2-alignment`), which writes into `<model_root>/phoneme/<owner>/<repo>/` beside the hand-dropped layout `phoneme::find_model_dir` scans. The full 392-entry phone inventory is fetched from the upstream PyTorch repo, because the ONNX repo's own `vocab.json` files are pruned per language. Nothing is fetched automatically, and nothing pretends to have measured pronunciation in the meantime. The size is the honest cost of an fp16 wav2vec2-large export; there is no int8 build of this model to point at. The downloaded file was load-tested on Windows: it opens through `ort`, reports 392 classes and one posterior row per 20 ms of audio, and free-decodes a 38 s German recording into 92 plausible IPA phones rather than one saturated id — which is what the normalisation step is there to guarantee.
- No recorded fixtures are in the repo. The Phase-0 gate (four attempts: clean, error-laden, restarting, deliberately mispronounced) needs a microphone and stays an acceptance step for a real device; the automated surface is the synthetic and hand-built ones below.
- An uploaded clip has no word timings, so per-word attribution for it is time-proportional across the reference words — approximate, though the aggregate score is unaffected. Generating with Edge TTS is the exact-per-word route.
- A text with no standard clip cannot be scored for pronunciation at all; the fix is one click in **Manage**, and the card says so.

## Test surface

Pure functions carry the tests, because they are the parts whose thresholds will be tuned. `cargo test --offline --features desktop --lib` is **299 passed, 0 failed, 2 ignored**; nothing in that number needs a model file, a network connection or a microphone. The two ignored tests are the ones that reach a real dependency — the `ocr` Hunspell dictionary download, and the pronunciation chain below (Edge TTS plus the installed model) — because neither can be faked convincingly.

| Area | What is asserted |
|---|---|
| `asr_metric` | every verdict class, punctuation/case, empty sides, and a WIP-invariance regression across the pre-existing cases |
| `vad` | silence → no segments; a lone tone burst → one segment; tone–gap–tone → two; a stop-consonant gap stays inside one segment (the loud-level ceiling) |
| `fluency` | hand-built timing tables for pause counts, classification and every rate; a gap covered by VAD speech is not a pause; the stored-pause cap; single-word / no-pause / all-omitted attempts produce no `NaN` and no division by zero |
| `gop` | the forced aligner on a hand-built posterior matrix; `exp(gop/τ)` monotone and bounded in `[0,1]` |
| `assess` | the whole pipeline on a synthetic attempt with word timings; round-trip through `metrics_json` **and** the size assertion that keeps a stored card under 2 KB; `no_reference` and `stale_reference` end to end |
| `standard_audio` | the first insert for a text is the default and a second is not; `set_default` moves it; deleting the default promotes the earliest remaining; two defaults resolve to the earliest; a row whose wav is gone reads as nothing; a stale `text_sha` is reported stale; the clip lands at `media/<text_uuid>/<uuid>.wav` and delete (and a text delete) removes row and file |
| `tts_ref` | `group_phones` distributes by clip duration when there are no boundaries, instead of collapsing every phone into the last word |
| `phoneme` | CTC collapse with blanks as separators; model-directory resolution under both layouts; a `.part` leftover is not a model; the audio tensor is built in the element type the graph declares |
| `read_aloud` (live, ignored) | the whole pronunciation chain against the real service and a real model: a standard clip is generated with Edge TTS (word boundaries included), the model decodes it, forced alignment groups it per word, and the clip's own samples fed back as the recording score near the top of the band (worst word 1.00 over six words) |

Integration was meant to run on four recorded fixtures — clean, word errors, restart plus a long mid-sentence pause, deliberate mispronunciations the STT still transcribes correctly — the last being the only honest check that GOP discriminates. They need a microphone and are not in the repo, so that check remains open (see the decision log).

## Not in v1

A calibrated composite reading score; a lexicon/G2P fallback so the reference phone sequence works offline; a per-phone table on the assessment card (phone detail appears only in a requested feedback report — see [read-aloud-feedback.md](./read-aloud-feedback.md)); published reading-rate norms; streaming or real-time feedback during the attempt; a quantized (int8) phoneme export; automatic clip regeneration when a text changes (it is reported stale, one click to fix); per-clip usage statistics, waveform caches for clips, and re-trimming a clip after the fact; and any change to the XP gate.

## Decision log

Decisions taken while implementing, in the order they became real.

**D8 — `words` travels from the frontend, not from the backend.** `model_transcribe_detailed` is called by the recorder because that is where the WAV and the STT already are; passing the resulting timings back into `read_aloud_submit` / `read_aloud_rerecognize` / `read_aloud_assess` as `words?` mirrors how `recognized` is already passed. The alternative — having `read_aloud` call `models::transcribe` itself — would have made the read-aloud module depend on the `stt` gate for nothing.

**D9 — `minor` counts as an error for `score`.** It would have been easy to fold near-misses into WIP and make everyone's accuracy rise. That would silently change `best_score`, the XP gate and every stored attempt's comparison basis. The lenient view therefore lives in a *second* number (`lenient_score`), and the plan's WIP-invariance test pins the first one.

**D10 — The VAD needs a loud-level ceiling.** A recording with no silence in it has a 10th-percentile frame RMS equal to its speech level, so `floor × 3` lands above every frame and the detector reports nothing. The threshold is therefore `max(min(3 × floor, 0.5 × p90), 0.004)`: above the estimated noise, below half the loud level. Two failing unit tests — not a listening session — produced this.

**D11 — Fluency rates are computed over the reading span, not the recording.** The recorder keeps running while the learner fumbles for the button, and `duration` includes that. `wcpm` and `speech_rate` use first-word-start → last-word-end; `duration` is still reported, because the pause ribbon needs the coordinate system the recording has.

**D12 — `MAX_STORED_PAUSES` is 24.** The plan said 40. A round-trip test measured a 3003-byte row at 40 pauses against the 2 KB budget, so the cap and the pause shape (an omitted `after_ref` is not serialized) were both tightened until the test passed. The budget is the requirement; the number is derived from it.

**D13 — No catalog entry for the phoneme model yet.** Publishing one requires a verified int8 CTC export with a matching `vocab.json`; none was confirmed. Rather than register a guess, `phoneme::find_model_dir` resolves `<model_root>/phoneme/**/model.onnx` by convention, so an install is a directory drop and the page says exactly that. When a real export is chosen, the catalog entry is additive.

**D14 — The pronunciation axis ships inert.** The Phase-0 gate (a usable ONNX phoneme export) was not cleared, and the plan's own instruction for that case is to stop rather than substitute. So the whole path — TTS reference, forced alignment, GOP, the card, the `read_aloud_assess` tool — is built, compiled and tested, and every attempt reports `unavailable_reason: "model_missing"` until a model is installed. Nothing is faked, and the two axes that can be measured are unaffected.

**D15 — The pronunciation model became a catalog entry, and the loader grew two things it needed.** D13/D14 held only while no export existed. `Hochien/wav2vec2-alignment` is one: an Apache-2.0 ONNX export of `facebook/wav2vec2-lv-60-espeak-cv-ft`, `[batch, samples]` → `[batch, frames, 392]` IPA logits, fp16, with per-language model files. Registering it forced three changes that the directory-drop design had deferred. (1) The catalog stores under `<model_root>/candle/`, but the scorer scans `<model_root>/phoneme/`, so `hf_models_root` special-cases a `phoneme/`-prefixed folder — a downloaded model and a drifted-in one are then indistinguishable, which is the point. (2) The loader required the file to be named `model.onnx`; the export calls it `wav2vec2-lv60-espeak-fp16.onnx`, and exports will keep disagreeing, so a model is now "a directory with an ONNX file and a vocabulary" (`.part` downloads are still never picked up). (3) **`posteriors` now normalises the waveform.** The `Wav2Vec2FeatureExtractor` applies per-utterance zero-mean/unit-variance before the network; an ONNX export starts at the network, so without it every frame's argmax collapses onto one id and the axis would report a confident wrong score rather than a missing step. That is the one preprocessing step whose absence looks like a scoring result.

**D16 — The pronunciation card's `model_missing` copy points at the Models page.** It used to instruct the reader to drop `model.onnx` + `vocab.json` into a folder that nothing in the UI mentioned. The instruction was correct and useless; it now names the section and the model.

**D17 — The audio tensor's element type is read from the session, not guessed from the file name.** `wav2vec2-lv60-espeak-fp16.onnx` is an fp16 *conversion*, and a conversion normally takes the input down with it: handing such a graph an f32 tensor is rejected by ONNX Runtime before any inference, which would have surfaced as `model_error` on a model that is perfectly fine. This export happens to keep its `audio` input at f32 (checked against the real file: 392 classes, 49 posterior rows per second), but the name promises nothing about the graph, so `audio_input_dtype` asks `session.inputs()[0].dtype()` once at load and `posteriors` builds the tensor to match, defaulting to f32 for a non-tensor or undefined input. Both branches are unit-tested: the check earns its place on the day a differently-converted export arrives, and by then the failure would read as a broken model rather than as a wrong dtype.

**D18 — The Edge TTS client is pinned to a version that sends the v2 DRM token, and the output format is set explicitly.** Microsoft tightened `Sec-MS-GEC`: the v1 token (a plain SHA-256 of the 5-minute-rounded Windows tick count, version string `1-…`) is now refused, and `edge-tts-rust` 0.1.3 sends exactly that, so every synthesis — reference timeline, TTS preview, `edge_tts_synthesize` — answered `403 Forbidden`. It reached the UI as "the reference voice clip could not be synthesized or decoded", which was accurate and pointed at nothing the reader could fix. 0.1.7 sends the v2 `CS64-SHA256` digest with a `2-` version string, and the same request then upgrades (`101 Switching Protocols`); the handshake was reproduced by hand before the dependency was touched, because a 403 has three plausible owners (token, clock, egress address) and only the token was ours. The upgrade also flipped `SpeakOptions::default()` to WebM/Opus, so `edge_tts::speak_options` now states the mp3 format for every caller: the synthesised output decodes through Symphonia and the preview names it in an `<audio>` element. That is the general hazard with this dependency — its defaults are the service's, and they move.

**D19 — The reference is dataset-owned standard audio, not a network call during scoring.** D3 put the TTS reference in a device-local cache keyed by text and voice. That made the score depend on an HTTP request the first time, made the reference invisible (and unshareable) to everyone else, and left no way to fix a clipped or mispronounced rendering. It is now an asset of the dataset: `read_standard_audio` rows plus `media/<text_uuid>/<audio_uuid>.wav`, several clips per text with exactly one default, generated by Edge TTS or uploaded and cropped. The network is touched only when a human asks for a clip. A clip made from an earlier version of the text **refuses** rather than re-synthesising: its phone target no longer matches the compared words, and a silent network call behind a score is exactly what this decision removes. The old `read_aloud_tts/` cache is retired; its files are left on disk, being derived data with no destructive path worth writing.

**D20 — The phone cache stores the grouped target, keyed by model identity plus the clip's PCM digest.** The saving the wav2vec2 pass needs is not the posterior matrix (~392 floats × 50/s) but its only downstream use: `groups: Vec<Vec<usize>>`, a few KB, keyed by `sha256(model_identity ‖ sha256(clip PCM))` in `<workspace>/read_aloud_phones/`. A re-crop, a regenerate or a different model each change the key, so invalidation is a property of the name rather than a routine someone must remember to call. Because it lives in the workspace and not the dataset, scoring still never writes into a dataset. Measured effect: a second attempt at the same text runs the phoneme model once, not twice.

Still open, to be settled on a real device:

- **Does the TTS target hold for German compound nouns?** Edge TTS handles them, but the phone sequence for a compound read as two words versus one will differ; the alignment tolerates it, the word grouping may not.
- **Does GOP separate accent from error?** Unanswerable without the fourth fixture; the score is presented as informational until it is.
- **Is `wcpm` the right fluency headline for adults?** It is the standard oral-reading-fluency measure and the right shape here, but its bands are drawn from school-age reading research; adult self-study may need its own table.

## See also

- [read-aloud-feedback.md](./read-aloud-feedback.md) — what the three axes are turned into: an explicit "Get feedback" action that sends this page's measurements (and, through MCP, the recording itself) to a model and stores a structured report on the attempt.
- [dataset-read-aloud.md](./dataset-read-aloud.md) — the v1 practice loop these scores extend.
- [read-aloud-score-claude.md](./read-aloud-score-claude.md) — the superseded standalone-CLI specification.
