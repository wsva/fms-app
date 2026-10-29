# Implementation Plan: Reading Assessment System (Rust)

> **Superseded by [read-aloud-score.md](./read-aloud-score.md)** — that document records what was built in this app and why; this one remains the rationale for the core insight (pronunciation needs a model whose posteriors can be read, not the STT's output text).

Measures **reading accuracy**, **fluency**, and **pronunciation** from a recording plus its reference transcript.

## 0. Instructions for the implementing agent

- Work phase by phase, in order. Do not start a phase until the previous phase's acceptance criteria pass (`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`).
- Commit at the end of each phase with a message of the form `phase N: <title>`.
- Do **not** reimplement ASR. Wrap the existing Whisper-large / Parakeet-v3 code behind the `AsrBackend` trait (Phase 3). Read the existing ASR code first and adapt the trait to what it can actually expose; record any deviation in `docs/decisions.md`.
- Pure logic (text alignment, fluency, scoring) must be testable **without** audio or model files. Keep model-dependent code behind traits.
- Prefer small, well-tested functions. No `unwrap()` outside tests; use `thiserror` for library errors and `anyhow` only in the CLI.
- If something in this plan conflicts with what the existing codebase makes possible, choose the simplest working option and document it, rather than stalling.

## 1. Goals and non-goals

**Goals**
1. Given `(audio file, reference transcript)`, produce a JSON assessment with per-word results and aggregate scores for accuracy, fluency, pronunciation.
2. Per-word feedback: which reference words were correct, substituted, omitted, repeated, or weakly pronounced.
3. Swappable ASR backend (Whisper-large or Parakeet-v3).
4. Deterministic output for the same input.

**Non-goals (for v1)**
- Training models. Streaming/real-time operation. A GUI or web service. Multi-language support beyond what the ASR backend and text normalizer handle (design for it, implement for one language first).

## 2. Key design insight (read before coding)

ASR models have strong language-model priors and tend to "correct" mispronounced words to the expected word. Therefore:

- **Reading accuracy** may use the free-decoded ASR text, but treat it as a noisy hypothesis.
- **Pronunciation** must NOT rely only on the free-decoded text. Use acoustic evidence: score the **reference** tokens against the audio (forced scoring), and compare against the free-decode result.
- **Fluency** uses timestamps, refined by a VAD, because ASR word timestamps are imprecise in gaps.

## 3. Architecture

```
audio file ─► decode ─► mono 16 kHz f32 ─┬─► VAD ─► speech segments
                                         ├─► ASR free decode ─► TimedWords (+ confidences)
                                         └─► ASR forced scoring of reference ─► TokenScores
reference text ─► normalize ─► tokens ─┐
                                       ▼
                    word-level alignment (ref ↔ hyp) ─► WordResults
                                       │
              ┌────────────────────────┼─────────────────────────┐
              ▼                        ▼                         ▼
       reading accuracy            fluency                 pronunciation
              └────────────────────────┴─────────────────────────┘
                                       ▼
                               Assessment (JSON)
```

### Workspace layout

```
reading-assess/
├── Cargo.toml                 # workspace
├── docs/decisions.md          # deviations / design notes
├── crates/
│   ├── assess-core/           # types, text normalize, alignment, fluency, scoring (no model deps)
│   ├── assess-audio/          # decode, resample, VAD
│   ├── assess-asr/            # AsrBackend trait + Whisper / Parakeet adapters (wrap existing code)
│   ├── assess-pron/           # forced scoring aggregation, optional phoneme GOP
│   └── assess-cli/            # binary: `assess --audio a.wav --text ref.txt --backend parakeet`
├── tests/fixtures/            # small wavs + reference texts + expected JSON
└── tools/                     # python scripts for calibration / model export (optional)
```

### Suggested dependencies

| Purpose | Crate |
|---|---|
| Audio decode | `symphonia` (features: wav, mp3, flac, ogg, aac) |
| Resample | `rubato` |
| ONNX inference (VAD, phoneme model) | `ort` |
| Arrays | `ndarray` |
| Text | `unicode-normalization`, `regex`, `unicode-segmentation` |
| Serialization | `serde`, `serde_json` |
| Errors | `thiserror`, `anyhow` (CLI only) |
| CLI | `clap` |
| Tests | `proptest`, `insta` (snapshot), `approx` |
| Logging | `tracing`, `tracing-subscriber` |

## 4. Core data types (`assess-core`)

```rust
pub struct TimedWord {
    pub text: String,          // as emitted by ASR
    pub start: f32,            // seconds
    pub end: f32,
    pub confidence: Option<f32>, // 0..1, from free decode
}

pub struct TokenScore {         // forced scoring of the REFERENCE
    pub word_index: usize,      // index into normalized reference words
    pub logprob: f32,           // sum or mean logprob of the word's tokens
    pub n_tokens: usize,
    pub start: Option<f32>,     // if the backend can supply alignment times
    pub end: Option<f32>,
}

pub enum ErrorKind { Correct, Minor, Substituted, Omitted, Inserted, Repeated }

pub struct WordResult {
    pub ref_index: Option<usize>,
    pub ref_word: Option<String>,
    pub hyp_word: Option<String>,
    pub kind: ErrorKind,
    pub start: Option<f32>,
    pub end: Option<f32>,
    pub asr_confidence: Option<f32>,
    pub pron_score: Option<f32>,    // 0..1, filled by assess-pron
}

pub struct Pause { pub start: f32, pub end: f32, pub after_ref_word: Option<usize>, pub kind: PauseKind }
pub enum PauseKind { AtPunctuation, WithinPhrase, Leading, Trailing }

pub struct FluencyMetrics { /* see Phase 4 */ }
pub struct Assessment { /* see section 8 */ }
```

```rust
pub trait AsrBackend {
    /// Free decoding. Must return word-level timestamps.
    fn transcribe(&self, pcm16k: &[f32]) -> Result<Vec<TimedWord>, AsrError>;

    /// Score the reference text against the audio (teacher-forced / forced alignment).
    /// Return None for words the backend cannot score.
    fn score_reference(&self, pcm16k: &[f32], reference_words: &[String])
        -> Result<Vec<TokenScore>, AsrError>;

    fn capabilities(&self) -> AsrCapabilities; // word_timestamps, forced_scoring, token_logprobs
}
```

Provide a `MockAsr` in `assess-asr` (feature `mock`) that returns canned results, so everything downstream can be tested without models.

---

## 5. Phases

### Phase 1: Workspace, text normalization, word alignment (reading accuracy)

**Tasks**
1. Create the workspace and crates with the layout above; CI-style script `scripts/check.sh` running fmt, clippy, test.
2. `assess-core::text`: `normalize(&str) -> Vec<NormWord>` where `NormWord { norm: String, original: String, trailing_punct: Option<Punct>, sentence_end: bool }`.
   - Lowercase, Unicode NFKC, strip punctuation but **remember** trailing punctuation (`, . ; : ! ? —`) because fluency needs it.
   - Normalize apostrophes/quotes/dashes; split hyphenated compounds consistently with how the ASR output is normalized (apply the same function to both sides).
   - Numbers: configurable `NumberPolicy` (`Words` | `Digits`); v1 can convert digits to words for English with a small built-in function, or leave as-is and document it.
   - Contractions: keep as single tokens (`don't`), and add an equivalence table for common variants (`cannot` ≈ `can not`).
3. `assess-core::align`: word-level edit-distance alignment between reference words and hypothesis words.
   - DP with costs: match 0, substitution 1 (reduced to 0.5 for near-matches, see below), insertion 1, deletion 1. Backtrace yields a sequence of ops.
   - Tie-breaking must be deterministic (prefer match > substitution > deletion > insertion).
   - Output `Vec<AlignOp>` where `AlignOp = Match | Sub | Del | Ins` with indices into both sequences.
4. Classification into `ErrorKind`:
   - `Correct`: exact normalized match.
   - `Minor`: substitution where the words are known homophones (small embedded list), or Levenshtein character distance ≤ 1 for words of length ≥ 5, or a number/format variant. Minor errors are reported but weighted at 0 in the accuracy score by default (configurable).
   - `Substituted`, `Omitted` (reference word with no hypothesis), `Inserted` (hypothesis word with no reference).
   - `Repeated`: an inserted word equal to an adjacent reference/hypothesis word, or an inserted run that repeats the preceding 1–3 words (restart: "the cat the cat sat").
5. Reading accuracy metrics: `correct_rate = (correct + minor) / n_ref`, plus counts for each error kind, plus `wcpm`-ready counters (correct words).

**Acceptance criteria**
- Unit tests covering: identical text, one substitution, omission at start/middle/end, insertion, repetition of 1 and 2 words, restart, homophone, punctuation/case differences, empty hypothesis, empty reference (returns an error, not a panic).
- Property tests (`proptest`): alignment of a sequence with itself is all matches; edit distance is symmetric in count of non-matches; ops reconstruct both sequences exactly.
- Snapshot tests (`insta`) for 5+ hand-written cases.

### Phase 2: Audio pipeline (decode, resample, VAD)

**Tasks**
1. `assess-audio::load(path) -> Result<Audio { samples: Vec<f32>, sample_rate: u32 }>` using `symphonia`; downmix to mono.
2. `resample_to_16k` using `rubato` (sinc, high quality). Skip if already 16 kHz.
3. Optional `normalize_loudness` (peak or RMS), off by default.
4. VAD: run Silero VAD (ONNX) via `ort`. Implement a `Vad` trait with `fn segments(&self, pcm16k: &[f32]) -> Vec<Segment { start, end }>`.
   - Frame size 512 samples at 16 kHz (32 ms); keep the recurrent state between frames per the model's I/O spec.
   - Post-processing: threshold 0.5 (configurable), min speech 200 ms, min silence 100 ms, speech pad 30 ms; merge segments separated by less than min silence.
   - Provide an `EnergyVad` fallback (RMS threshold with hysteresis) so tests and quick runs need no model file.
5. Document where to put the model file (`models/silero_vad.onnx`) and make the path configurable.

**Acceptance criteria**
- Loads wav/mp3/flac of different sample rates and channel counts; output is mono 16 kHz with a duration within 1 ms of the source.
- VAD returns no segments for silence and one segment for a synthetic tone burst surrounded by silence (test with generated signals).
- Integration test against one real fixture recording with hand-verified speech boundaries (±100 ms).

### Phase 3: ASR backend adapters

**Tasks**
1. **Read the existing Whisper/Parakeet implementation first.** Write a short note in `docs/decisions.md` listing what it exposes: word timestamps? token logprobs? CTC logits? decoder access for teacher forcing?
2. Implement `AsrBackend` for the existing code in `assess-asr`:
   - `transcribe`: return `TimedWord`s. Merge sub-word tokens into words using the tokenizer's word-boundary marker. Compute word confidence as the geometric mean of its token probabilities (or minimum, configurable).
     - Parakeet-v3 (TDT): derive word start/end from token frame indices and durations (frame stride is model-specific; read it from the model config, do not hardcode).
     - Whisper: use word-level timestamps via cross-attention DTW if already implemented; otherwise segment-level timestamps with a documented precision caveat.
   - `score_reference`: choose the strategy by what is available:
     - **CTC head available**: CTC forced alignment (Viterbi over the reference token sequence) giving per-token log posteriors and times.
     - **Whisper**: teacher-force the reference tokens through the decoder with the audio encoder output; collect per-token logprobs. Because Whisper is segment-based (30 s windows), chunk long audio by VAD segments and map the reference words to chunks using the free-decode alignment from Phase 1.
     - **Parakeet TDT/RNNT only (no CTC head)**: compute the forward score of the reference sequence under the transducer (or, if too costly, mark `forced_scoring = false` in capabilities and rely on the fallback below).
   - Fallback when forced scoring is impossible: use the free-decode word confidences and clearly flag in the output that pronunciation is a coarse proxy (`pronunciation.method = "asr_confidence_proxy"`).
3. Long audio: chunk by VAD segments (≤ 25 s for Whisper, model limit for Parakeet) with ~0.5 s overlap, and stitch timestamps back to global time. Deduplicate words in the overlap.
4. Implement `MockAsr` that reads canned `TimedWord`/`TokenScore` JSON from disk.

**Acceptance criteria**
- Both backends (or whichever exist) produce `TimedWord`s with monotonically non-decreasing timestamps within the audio duration.
- Chunked and unchunked transcription of a 60 s fixture agree on at least 95% of words.
- `capabilities()` reports honestly; a test asserts the mock and each real backend satisfy the trait contract (non-empty output on a speech fixture, empty output on silence).

### Phase 4: Fluency metrics

All inputs: aligned `WordResult`s with timestamps, VAD segments, and punctuation info from `NormWord`.

**Tasks**
1. **Timestamp refinement**: for each word, clip start/end into the nearest VAD speech segment; where an inter-word gap in ASR timestamps lies entirely inside a VAD speech segment, treat it as not a pause (ASR timestamp noise); where VAD shows silence ≥ 250 ms between words, treat it as a pause.
2. **Pause detection**: pauses are silences ≥ `min_pause` (default 0.25 s) between consecutive spoken words; also record leading and trailing silence (excluded from rate metrics).
3. **Pause classification** using reference punctuation: `AtPunctuation` if the preceding reference word has trailing punctuation (comma, period, etc.) or sits at a sentence end; `WithinPhrase` otherwise. Long pause threshold `long_pause` = 1.0 s (configurable).
4. Metrics (`FluencyMetrics`):
   - `total_duration` (first word start → last word end)
   - `speaking_time` (total duration minus pauses)
   - `speech_rate_wpm` = correct and incorrect words read ÷ total duration (min)
   - `wcpm` = words **correct** per minute (standard oral reading fluency measure)
   - `articulation_rate_wpm` = words ÷ speaking time
   - `pause_count`, `pause_total`, `pause_mean`, `long_pause_count`
   - `within_phrase_pause_ratio` = within-phrase pauses ÷ all pauses (higher = worse)
   - `mean_length_of_run` = mean words between pauses
   - `repetition_count`, `restart_count` (from Phase 1), `filler_count` (detect from a small configurable list: "um", "uh", "er", "hmm"; only meaningful if the ASR keeps fillers, note this in output)
   - `rhythm_cv` = coefficient of variation of (word duration ÷ expected duration proxy, e.g. character count), excluding words adjacent to pauses
5. Provide a `FluencyConfig` struct with all thresholds and defaults.

**Acceptance criteria**
- Synthetic tests: a hand-built list of words with timestamps yields exact expected pause counts, classifications, and rates (use `approx`).
- Test that a gap fully covered by VAD speech is not counted as a pause, and that a punctuation-aligned pause is classified correctly.
- Edge cases: single word, no pauses, all words omitted (no division by zero; return `None`/0 with documented semantics).

### Phase 5: Pronunciation scoring (forced scoring, "Option B")

**Tasks**
1. For each reference word `w`, obtain `forced_lp(w)` from `score_reference` (mean log-probability per token, plus token count).
2. Obtain a competing score `free_lp(w)` from the free decode for the aligned hypothesis region (the log-prob of the best path in the same time span). Compute the **log-likelihood ratio**:
   `llr(w) = forced_lp(w) - free_lp(w)` (≤ 0 typically; closer to 0 is better).
3. Map to a per-word score in `[0, 1]` with a monotonic squashing function with configurable parameters:
   `pron_score(w) = sigmoid(a * (llr(w) - b))`, with initial defaults chosen so llr ≈ 0 → ~0.95 and llr ≈ −3 → ~0.3. These constants will be re-fit in Phase 7.
4. Handling special cases:
   - Omitted words: `pron_score = None` (do not double penalize; accuracy already covers it).
   - Very short words (≤ 2 chars) and function words: down-weight in the aggregate (configurable list/weights), since their scores are noisy.
   - Words flagged `Minor` still get a pron score.
5. Aggregate: duration-weighted or length-weighted mean of word scores → `pronunciation.score` (0–100), plus percent of words below a `weak_threshold` (default 0.5), plus a list of the worst N words.
6. Report `pronunciation.method` as one of `forced_llr`, `forced_only`, `asr_confidence_proxy`, depending on backend capabilities.

**Acceptance criteria**
- Unit tests on the llr→score mapping (monotonic, bounded, deterministic).
- Test with `MockAsr` where one word has deliberately low forced logprob: that word is in the worst-words list and the aggregate decreases versus a clean case.
- Document known limitations in the README (see Phase 8).

### Phase 6: Pipeline orchestration, CLI, JSON output

**Tasks**
1. `assess-core::pipeline::Assessor` with builder: `Assessor::builder().backend(...).vad(...).configs(...).build()`; `assess(&self, audio: &Audio, reference: &str) -> Result<Assessment>`.
2. Order of operations: load/resample → VAD → free decode → align with reference (Phase 1) → forced scoring → word-level pron scores → fluency → aggregate.
3. `assess-cli`:
   `assess --audio <path> --text <path|-> --backend <whisper|parakeet|mock> [--model-dir ...] [--vad-model ...] [--config config.toml] [--out result.json] [--pretty]`
4. Config via TOML (serde): thresholds for alignment, fluency, pronunciation mapping.
5. Output JSON schema (stable, versioned with `"schema_version": 1`):

```json
{
  "schema_version": 1,
  "audio": { "duration": 42.1, "sample_rate_original": 44100 },
  "backend": { "name": "parakeet-v3", "capabilities": ["word_timestamps", "forced_scoring"] },
  "reading_accuracy": {
    "score": 93.5,
    "counts": { "reference": 120, "correct": 109, "minor": 3, "substituted": 5, "omitted": 3, "inserted": 4, "repeated": 2 }
  },
  "fluency": {
    "score": null,
    "wcpm": 98.2, "speech_rate_wpm": 104.0, "articulation_rate_wpm": 131.0,
    "pause_count": 14, "long_pause_count": 2, "within_phrase_pause_ratio": 0.36,
    "mean_length_of_run": 7.9, "repetition_count": 2, "restart_count": 1, "rhythm_cv": 0.41
  },
  "pronunciation": {
    "score": 81.0, "method": "forced_llr", "weak_word_ratio": 0.08,
    "worst_words": [{ "ref_index": 33, "word": "thorough", "score": 0.21 }]
  },
  "words": [
    { "ref_index": 0, "ref": "the", "hyp": "the", "kind": "correct", "start": 0.52, "end": 0.61, "asr_confidence": 0.98, "pron_score": 0.96 }
  ],
  "pauses": [ { "start": 3.2, "end": 3.9, "kind": "within_phrase", "after_ref_index": 7 } ],
  "warnings": ["Pronunciation is a coarse proxy: backend lacks forced scoring"]
}
```

   Fluency and overall composite scores are `null` until calibrated (Phase 7); until then report raw metrics only, not an invented 0–100.
6. Add `--format text` for a human-readable summary with colored word-level markup (correct / error words).

**Acceptance criteria**
- End-to-end test with `MockAsr` and a fixture wav produces JSON matching an `insta` snapshot.
- End-to-end run with a real backend on at least 3 fixture recordings (clean read, read with errors, read with long pauses) produces sensible results; record outputs in `tests/fixtures/expected/` for regression (with tolerances, not exact floats).
- Errors (missing file, unsupported codec, empty reference, no speech detected) produce clear messages and non-zero exit codes.

### Phase 7: Calibration and evaluation

**Tasks**
1. Define a small evaluation dataset format: `dataset/<id>/{audio.wav, reference.txt, labels.json}` where `labels.json` has teacher ratings (accuracy 1–5, fluency 1–5, pronunciation 1–5; optionally per-word mispronunciation flags).
2. `assess-cli eval --dataset dir --out metrics.csv`: runs the pipeline on every item and writes raw metrics next to labels.
3. `tools/calibrate.py` (Python is fine here, offline): fit
   - fluency score from {wcpm, within_phrase_pause_ratio, mean_length_of_run, long_pause_count, rhythm_cv} via ridge or isotonic regression;
   - pronunciation `a`, `b` constants (and the weak-word threshold) against teacher labels / per-word flags;
   - export the fitted parameters to `calibration.toml`, loaded by the CLI.
4. Report: Pearson/Spearman correlation and MAE vs teacher ratings per dimension; per-word mispronunciation detection precision/recall if word flags exist.
5. Targets (aim, not hard gates): Spearman ≥ 0.7 for each dimension on held-out items; if not reached, record findings and candidate fixes in `docs/decisions.md`.

**Acceptance criteria**
- `eval` command runs on a dataset of ≥ 20 items and writes a CSV.
- Calibration script runs, writes `calibration.toml`, and the CLI picks it up and fills in the previously-`null` score fields.

### Phase 8 (optional): Phoneme-level GOP ("Option C")

Only start if per-phoneme feedback is required (for example, L2 learners).

**Tasks**
1. Export a phoneme-recognition CTC model (e.g. `facebook/wav2vec2-lv-60-espeak-cv-ft` or a multilingual phoneme model) to ONNX; place in `models/`; run with `ort` in `assess-pron`.
2. Grapheme-to-phoneme for the reference: call `espeak-ng` (via FFI or subprocess, configurable) or use a CMUdict lookup for English, with an espeak fallback for out-of-vocabulary words. Map the phoneme inventory to the model vocabulary.
3. CTC forced alignment of the reference phoneme sequence → per-phone time spans and posteriors.
4. GOP per phone: `gop = mean over frames of [log P(expected phone) − max_q log P(q)]`; also report the top competing phone (e.g. "expected /iː/, heard /ɪ/").
5. Roll up to words (min or mean of phone GOPs) and add `phones` arrays to each word in the output JSON, behind `schema_version` bump or an optional field.

**Acceptance criteria**
- Unit test for the CTC Viterbi forced aligner on a hand-built posterior matrix.
- On fixtures with known mispronunciations, the expected phone is flagged with a lower GOP than correctly pronounced phones.

### Phase 9: Documentation and hardening

1. `README.md`: install, model download locations, CLI usage, JSON schema, configuration reference, how scores are computed.
2. Document limitations: ASR bias hiding mispronunciations; timestamp precision per backend; accented speech; scores are uncalibrated until Phase 7 is run; children's speech accuracy.
3. Performance: report real-time factor for each backend on the fixtures; avoid reloading models per request (the `Assessor` holds the loaded models); make inference threads configurable.
4. Robustness: fuzz/proptest the text normalizer and aligner; test very long audio (≥ 10 min); test clipped, noisy, and silent audio.

---

## 6. Cross-cutting requirements

- **Determinism**: no randomness; fixed tie-breaking; floats serialized with limited precision (e.g. 3 decimals).
- **Units**: all times in seconds as `f32`/`f64` consistently (choose `f64` for timestamps in the core to avoid drift on long files); document it.
- **Configuration over constants**: every threshold named in this plan lives in a config struct with documented defaults.
- **Logging**: `tracing` spans per stage with durations; `--verbose` flag.
- **Privacy**: never log audio content or transcripts above `debug` level.
- **Feature flags**: `whisper`, `parakeet`, `mock`, `phoneme-gop` so users build only what they need.

## 7. Test fixtures to prepare (ask the user if missing)

1. A clean ~20 s read of a known text (reference text provided).
2. The same text with 3–5 deliberate word errors (substitutions/omissions).
3. A read with a repetition, a restart, and a long mid-sentence pause.
4. A read with 2–3 deliberate mispronunciations that the ASR still decodes as the correct word (to validate Phase 5).
5. Silence and noise-only clips.

## 8. Open questions for the user (agent: proceed with the stated default if no answer)

| Question | Default if unanswered |
|---|---|
| Reading language(s)? | English |
| Native or L2 learners? | Mixed; skip Phase 8 |
| Does the existing ASR expose a CTC head, token logprobs, or decoder access? | Check the code; fall back to confidence proxy and flag it |
| Real-time or batch? | Batch |
| Target score scale | Raw metrics + 0–100 after calibration |
| Are teacher ratings available for calibration? | No; Phase 7 deferred, scores reported as raw metrics |

## 9. Definition of done (v1)

- Phases 1–6 complete with passing tests and clippy clean.
- CLI produces the JSON schema in Phase 6 for real recordings with the existing ASR backend(s).
- Per-word feedback (error kind, timing, pronunciation score) present for every reference word.
- README documents usage, scores, and limitations.
- Phases 7–8 either completed or explicitly documented as pending in `docs/decisions.md`.