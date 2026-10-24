# Read-Aloud Scoring — Accuracy, Fluency, Pronunciation

> Status: **implemented** (accuracy · fluency · pronunciation, with the pronunciation axis inert until a phoneme model is installed — see [Decision log](#decision-log)). Supersedes [read-aloud-score-claude.md](./read-aloud-score-claude.md), which specified a standalone CLI workspace; the scoring design survived, the delivery vehicle did not. The v1 practice loop this extends is described in [dataset-read-aloud.md](./dataset-read-aloud.md).

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

**D3 — Pronunciation is a CTC phoneme model scored against a TTS-derived phone timeline.** GOP needs the expected phone sequence, and three ways to get one were considered. *Teacher forcing* the STT model is impossible (above). *espeak-ng G2P* is a C library with an NDK build, plus a lexicon per language, plus a phoneme-inventory mapping — a large amount of native build surface for one scoring axis. *Per-word DTW against TTS audio* measures similarity to a specific voice rather than correctness, and does not survive a change of speaker. The chosen route removes both problems: synthesize the reference with Edge TTS, decode it with the same phoneme model, and take the CTC-collapsed decode as the target phone sequence, grouped per word by the TTS boundary times. The target is then independent of the learner (it is what a canonical speaker actually said), needs no G2P, and works for any language Edge TTS speaks. The cost is one network fetch per text, cached — see "Storage and sync".

**D4 — Derived and capped, in the app database.** An attempt's metrics are recomputed from the recording on demand; only a compact aggregate (~2 KB: rates, counts, pause count, the twelve weakest words, plus the capability flags that produced them) is stored on `read_attempt_event`. Per-frame posteriors, per-phone GOP and full word timing arrays are never persisted: they are large, device-local, and exactly reconstructible from the WAV that is already kept.

**D5 — Unavailable is a state, not a proxy.** If the phoneme model is not installed, the app is offline, or the take is too short to align, `pronunciation.unavailable_reason` says so and the card says "not measured". Substituting ASR word confidence would produce a number that looks like pronunciation and is not, and a plausible fake is worse than an honest blank in a learning tool.

**D6 — `datasets/read_aloud/` becomes a module directory.** The file is 1569 lines and this work adds five concerns to it (VAD, fluency, GOP, phoneme session, TTS reference). Splitting keeps the pure logic testable without audio or models, which is the property that makes the thresholds safe to change.

**D7 — No composite score, no XP change, no protocol bump.** The XP gate stays on accuracy so the ledger's meaning is stable. `metrics_json` is an additive optional column on an already-journalled row, so old peers ignore it and no `PROTOCOL_VERSION` change is needed.

## Data flow

```
WAV (16 kHz mono, straight from the recorder)
  ├── energy VAD ─────────────► speech segments
  ├── model_transcribe_detailed ─► text + TimedWord[]   (Parakeet: per word; others: text only)
  └── phoneme model ──────────► per-frame log-posteriors
reference text
  ├── normalize_words ────────► norm words + punctuation per word
  └── Edge TTS reference ─────► canonical MP3 + word boundaries
        └── phoneme model ───► reference phone sequence, grouped per reference word
                                    │
   CTC forced alignment of that sequence onto the learner's posteriors
                                    ▼
                     per-phone GOP ─► per-word pronunciation score
accuracy alignment (reference ↔ transcript) ─► verdicts, WIP (unchanged)
fluency (word times clipped into VAD segments) ─► pauses, rates
                                    ▼
                              Assessment (serde)
```

The pure half — normalization, alignment, classification, VAD, pause detection, rate arithmetic, forced alignment and GOP arithmetic — never touches a model or a file. Only `phoneme.rs` (session) and `tts_ref.rs` (network + cache) are effectful, and `assess.rs` is the one function that composes them, so every threshold remains testable against hand-built numbers.

## Storage and sync

`read_attempt_event` gains one nullable column, applied idempotently in `ensure_attempt_schema` the way `normalized_content` was added to `read_text`:

```sql
ALTER TABLE read_attempt_event ADD COLUMN metrics_json TEXT
```

`attempt_payload` adds the field to the journal payload and `apply_remote_save` reads it with the same tolerant `get()` helper it uses for the other keys, so a peer on an older build simply ignores it. Word-level and phone-level arrays stay out of the payload on purpose: the journal is sync traffic, and a per-frame matrix would multiply it for data that is device-local anyway.

The TTS reference is cached in `<workspace>/read_aloud_tts/<voice>-<hash8>.mp3|.json`, beside `read_aloud_audio/`, never inside the dataset directory. Practising must not write into a dataset (that is what keeps an adopted base fork-free and a published dataset clean), and the cache is genuinely device-local: the key is `sha256(effective_reference \x1f voice)`, so it is per text, per voice, and invalidated by editing the text because the hash covers it.

One measured addition to D4: a stored pause costs ~48 bytes (`{"start":12.04,"end":12.31,"kind":"within_phrase"}`) and the aggregates ~520, so `MAX_STORED_PAUSES` is **24**, not 40 — 40 would put a verbose take at ~3 KB and break the row budget. Raising it again needs a narrower pause shape, not a bigger number.

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
| VAD | 20 ms frames; noise floor = 10th-percentile frame RMS, ceilinged at half the 90th percentile so a speech-only take is not read as silence; threshold `max(min(3 × floor, 0.5 × loud), 0.004)`; 2-frame hysteresis; ≥ 150 ms speech; ≥ 120 ms gap to split; 40 ms pad | as stated |
| `long_pause` | Long enough to be a break rather than a breath | 1.0 s |
| `within_phrase_pause_ratio` | Pauses not aligned with reference punctuation ÷ all pauses | — |
| `mean_length_of_run` | Words between pauses | — |
| `gop(phone)` | Mean over the phone's aligned frames of `log p(target) − log max_q p(q)` | — |
| `pron_score(word)` | `exp(gop / τ)` clamped to `[0,1]`, duration-weighted across the word's phones | τ = 2.5 |
| `weak_ratio` | Share of scored words below 0.4 | 0.4 |
| band | `wcpm` against a configured slow/target/fast table | heuristic |

`exp(gop/τ)` was chosen over a fitted sigmoid deliberately: it is monotone, bounded, needs no calibration data to be *meaningful*, and one constant is easy to revisit once attempt history exists. The band table is labelled a heuristic wherever it surfaces — it is not a published reading norm, and pretending otherwise in a language-learning app would be the more damaging mistake.

## Configuration

`FluencyConfig` (`fluency.rs`) holds every fluency threshold in one struct with documented defaults; it is *not* serialized — the assessment stores the numbers it produced, and a tuning change applies to the next take rather than rewriting history. `VadConfig` (`vad.rs`) does the same for the detector. `GOP_TAU` and the band edges are constants in `assess.rs` / `FluencyConfig`, deliberately not settings: they describe the metric, not the material, and a per-user τ would make two learners' scores incomparable.

One setting is user-facing: `read_aloud_tts_voice` (workspace-scoped, empty = `read_aloud::DEFAULT_TTS_VOICE`, `en-US-EmmaMultilingualNeural`), because the reference voice should speak the language you are learning. Nothing here is per dataset — a dataset is shareable content and must not carry its owner's scoring preferences.

## Commands and MCP surface

- `read_aloud_assess(content, recognized, wav_base64, words?)` — preview: `Assessment` out, nothing persisted. The twin of `read_aloud_score` for the audio side, so an agent can evaluate a take it recorded.
- `read_aloud_submit(..., words?)` — unchanged contract, plus the `Assessment` in its result and `metrics_json` on the stored attempt. XP and thresholds are untouched.
- `read_aloud_rerecognize(..., words?)` — recomputes the assessment from the device-local recording, since a new transcript changes accuracy and may change which words line up with which pauses.
- `model_transcribe_detailed(wav_base64)` — the recorder's final STT call: `{ text, words[], capabilities: { word_timings } }`, so the page knows before rendering whether a fluency card is possible. `model_transcribe` is untouched for the live preview, voice input and every other caller.
- `read_aloud_list_attempts` — attempts carry their stored `metrics`, so the history view needs no second call.

`words` is the one piece of evidence that is *not* gathered on the backend: it is the STT output the frontend already produces, passed in the same way `recognized` is. That keeps `read_aloud` free of the `stt`-gated `models` module, and it means a peer replaying the journal needs nothing extra.

Per the agent-friendly rule, all of them return structured JSON and state what is missing rather than failing opaquely.

## Frontend

`PracticePanel` renders one result as three blocks, side by side on a wide screen and stacked on a phone: Accuracy (the existing badge and XP copy, unchanged), Fluency (correct words/min with a band chip, articulation rate, pause counts, words between pauses), and Pronunciation (score, the three weakest words, or "not measured" plus the reason). `AsrDiff` gains an optional per-reference-word score map so a word that was heard correctly but pronounced weakly gets an amber underline — `ref_index + offset` inside an equal run, so the diff logic is untouched. `AttemptsPanel` shows the same three blocks from `metrics_json` for any stored attempt, plus a pause ribbon drawn from the capped pause list, and its "recognize again" now re-runs `model_transcribe_detailed` so a re-scored attempt keeps its fluency.

## Degradation matrix

| Condition | Accuracy | Fluency | Pronunciation |
|---|---|---|---|
| Parakeet + phoneme model + cached reference | full | full | full |
| Non-Parakeet STT model | full | hidden (`word_timings: false`) | full (does not need word times) |
| `phoneme` feature compiled out (`--no-default-features`) | full | full | "not measured — model missing" |
| Phoneme model not installed | full | full | "not measured — no phoneme model" |
| Offline, no cached reference | full | full | "not measured — no network" |
| Take shorter than the alignable minimum | full | pauses only | "not measured — too short" |
| Silence or no speech detected | score 0, as today | hidden | "not measured" |

The one asymmetry worth stating: pronunciation does not depend on `word_timings`, because the forced alignment produces its own word spans from the reference phone sequence. That is why a Whisper user can still get pronunciation and cannot get fluency.

## Known limitations

- A TTS mispronunciation becomes a target error, and the learner is marked down for saying it correctly. Rare, but real, for proper nouns and loanwords.
- GOP on accented speech is unvalidated here. Foreign-accented deviations are precisely what GOP is worst at separating from errors without a large labelled corpus.
- `τ` and the band table are heuristics. Nothing in this project has teacher ratings, so nothing is calibrated; both are one constant and one table to revise later.
- Fluency is Parakeet-only, which on a dataset read with a Whisper model means the fluency card never appears.
- Pronunciation is reported per word; phone-level detail exists in the scoring path but is not surfaced in the UI.
- The phoneme model is a second download (≈95–100 MB int8). It is not on the critical path — every other feature works without it — and there is **no catalog entry yet**: `phoneme::find_model_dir` looks for `<model_root>/phoneme/**/model.onnx` + `vocab.json`, so an install is a directory drop until a verified export is published. Nothing is fetched automatically, and nothing pretends to have measured pronunciation in the meantime.
- No recorded fixtures are in the repo. The Phase-0 gate (four takes: clean, error-laden, restarting, deliberately mispronounced) needs a microphone and stays an acceptance step for a real device; the automated surface is the synthetic and hand-built ones below.

## Test surface

Pure functions carry the tests, because they are the parts whose thresholds will be tuned. `cargo test --offline --features desktop --lib` is **286 passed, 0 failed, 1 ignored**; nothing in that number needs a model file, a network connection or a microphone.

| Area | What is asserted |
|---|---|
| `asr_metric` | every verdict class, punctuation/case, empty sides, and a WIP-invariance regression across the pre-existing cases |
| `vad` | silence → no segments; a lone tone burst → one segment; tone–gap–tone → two; a stop-consonant gap stays inside one segment (the loud-level ceiling) |
| `fluency` | hand-built timing tables for pause counts, classification and every rate; a gap covered by VAD speech is not a pause; the stored-pause cap; single-word / no-pause / all-omitted takes produce no `NaN` and no division by zero |
| `gop` | the forced aligner on a hand-built posterior matrix; `exp(gop/τ)` monotone and bounded in `[0,1]` |
| `assess` | the whole pipeline on a synthetic take with word timings; round-trip through `metrics_json` **and** the size assertion that keeps a stored card under 2 KB |

Integration was meant to run on four recorded fixtures — clean, word errors, restart plus a long mid-sentence pause, deliberate mispronunciations the STT still transcribes correctly — the last being the only honest check that GOP discriminates. They need a microphone and are not in the repo, so that check remains open (see the decision log).

## Not in v1

A calibrated composite reading score; a lexicon/G2P fallback so the reference phone sequence works offline; phoneme-level feedback in the UI; published reading-rate norms; streaming or real-time feedback during the take; a catalog entry and one-click download for the phoneme model; and any change to the XP gate.

## Decision log

Decisions taken while implementing, in the order they became real.

**D8 — `words` travels from the frontend, not from the backend.** `model_transcribe_detailed` is called by the recorder because that is where the WAV and the STT already are; passing the resulting timings back into `read_aloud_submit` / `read_aloud_rerecognize` / `read_aloud_assess` as `words?` mirrors how `recognized` is already passed. The alternative — having `read_aloud` call `models::transcribe` itself — would have made the read-aloud module depend on the `stt` gate for nothing.

**D9 — `minor` counts as an error for `score`.** It would have been easy to fold near-misses into WIP and make everyone's accuracy rise. That would silently change `best_score`, the XP gate and every stored attempt's comparison basis. The lenient view therefore lives in a *second* number (`lenient_score`), and the plan's WIP-invariance test pins the first one.

**D10 — The VAD needs a loud-level ceiling.** A take with no silence in it has a 10th-percentile frame RMS equal to its speech level, so `floor × 3` lands above every frame and the detector reports nothing. The threshold is therefore `max(min(3 × floor, 0.5 × p90), 0.004)`: above the estimated noise, below half the loud level. Two failing unit tests — not a listening session — produced this.

**D11 — Fluency rates are computed over the reading span, not the recording.** The recorder keeps running while the learner fumbles for the button, and `duration` includes that. `wcpm` and `speech_rate` use first-word-start → last-word-end; `duration` is still reported, because the pause ribbon needs the coordinate system the recording has.

**D12 — `MAX_STORED_PAUSES` is 24.** The plan said 40. A round-trip test measured a 3003-byte row at 40 pauses against the 2 KB budget, so the cap and the pause shape (an omitted `after_ref` is not serialized) were both tightened until the test passed. The budget is the requirement; the number is derived from it.

**D13 — No catalog entry for the phoneme model yet.** Publishing one requires a verified int8 CTC export with a matching `vocab.json`; none was confirmed. Rather than register a guess, `phoneme::find_model_dir` resolves `<model_root>/phoneme/**/model.onnx` by convention, so an install is a directory drop and the page says exactly that. When a real export is chosen, the catalog entry is additive.

**D14 — The pronunciation axis ships inert.** The Phase-0 gate (a usable ONNX phoneme export) was not cleared, and the plan's own instruction for that case is to stop rather than substitute. So the whole path — TTS reference, forced alignment, GOP, the card, the `read_aloud_assess` tool — is built, compiled and tested, and every take reports `unavailable_reason: "model_missing"` until a model is installed. Nothing is faked, and the two axes that can be measured are unaffected.

Still open, to be settled on a real device:

- **Does the TTS target hold for German compound nouns?** Edge TTS handles them, but the phone sequence for a compound read as two words versus one will differ; the alignment tolerates it, the word grouping may not.
- **Does GOP separate accent from error?** Unanswerable without the fourth fixture; the score is presented as informational until it is.
- **Is `wcpm` the right fluency headline for adults?** It is the standard oral-reading-fluency measure and the right shape here, but its bands are drawn from school-age reading research; adult self-study may need its own table.
