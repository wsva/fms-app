# Read-Aloud Fluency — one number, its bar, and its evidence

> Status: **implemented**. Companion to [read-aloud-score.md](./read-aloud-score.md), which owns the three-axis model, the accuracy metric and the pronunciation axis; this document owns the fluency axis alone and is written to be read without the code in front of you.
>
> Code: `src-tauri/src/datasets/read_aloud/fluency.rs` (all of the arithmetic), `vad.rs` (where silence comes from), `mod.rs::clip_profile` (where the bar comes from), `assess.rs` (the seam), `src/lib/read_aloud/types.ts` (the shape the UI reads).

## 1. The defect, measured rather than argued

A learner reported that even a dataset's own **standard audio** — the professional reference clips the dataset ships, the rendition the learner is asked to imitate — scored only 50–70% on fluency. Measured on `de_c1` (German C1 audiobook material, 38 texts, 36 default clips):

| Measurement | Value |
|---|---|
| Reference pace, 36 clips | **105.2 – 149.0 wcpm**, median 131.6 |
| `pace_score` of those same clips under the old curve | median **79.0**, max 89 |
| Silences ≥ 0.25 s in the 7 clips with decoded audio | 12–51 per clip, 185 total (median 24) |
| Of those, ≥ 1.0 s | 21 |
| Old penalty on those 7 clips | **saturated at the 0.35 cap on every one** |
| Old fluency score, flawless reference audio | **46.0 – 55.7, median 51.5** |

Two independent defects, each sufficient on its own:

1. **The bar was a published norm, not this text.** The pace curve granted full credit at 180 correct words per minute and 90 at 150. A German audiobook narrator reads at 105–150 wcpm — deliberately, because that is what the material is — so the *best achievable* score on a perfect take was 89, and the median was 79. The norm was written for a different language, a different genre and a different age band.
2. **The pause charge was an absolute count with a cap.** `0.05 · long_pauses + 0.04 · mid_phrase_pauses`, capped at 0.35. On clips of 60–180 s that is 12–51 charges against 7 allowed, so (a) the penalty measured *how long the text is*, (b) it charged pauses the reference speaker herself took — breathing at a paragraph break is how the text is meant to be read — and (c) on studio-quiet audio the VAD threshold falls to its `absolute_floor` (−48 dBFS), so every natural inter-phrase silence counts as a defect.

Fixed correctly, the reference audio scored against itself must be near the top of the scale by construction: it *is* the target. Anything else means the score is measuring the comparison, not the reading.

## 2. What the score is now

```
fluency = pace_score(r) · (1 − disfluency_penalty)
r       = wcpm_take / wcpm_reference          // pace ratio, not an absolute rate
```

`wcpm` (correct words per minute) is unchanged: correct words ÷ reading span in minutes, where the reading span is first placement edge → last placement edge, so the margins between pressing record and reading are not scored (read-aloud-score.md, D11).

Both halves are now relative to the text's own standard audio:

**Pace** — piecewise linear over the ratio `r`:

| `r` | score | meaning |
|---|---|---|
| ≤ 0.6 | `r / 0.6 · 55` | less than six tenths of the reference pace |
| 0.6 | 55 | the floor: at a 130 wcpm reference this is 78 wcpm |
| 0.8 | 75 | noticeably slow but clearly reading |
| 1.0 | **95** | the reference's own pace |
| 1.25 | 100 | a quarter faster than the reference |
| > 1.25 | 100 | plateau — racing earns nothing |

**Disfluency** — charged per **100 spoken words**, not per take:

| charge | cost | when |
|---|---|---|
| hesitation | 0.05 | a pause where the reference did not pause |
| overrun | 0.03 | a pause where it did, held materially longer |
| restart | 0.05 | the reader went back and re-read a run |
| repetition | 0.02 | a stuttered word |
| **cap** | **0.35** | the worst possible score is 65% of the pace score |

Restarts and repetitions are still counted from the accuracy alignment (unchanged); the pause classes are new and are defined in §4.

The composite (`total`) is untouched: `accuracy · (0.5 + 0.5 · min(fluency, pronunciation))`, derived on read. XP still rides on accuracy alone, so nothing in the ledger changes meaning.

## 3. The reference profile — where the bar comes from

```rust
struct ReferenceProfile { wcpm: f64, pauses: Vec<RefPause> }
struct RefPause { after_ref: Option<usize>, at: f64, secs: f64 }
```

- **`wcpm`** — the clip's own correct-words/minute over *its* reading span.
- **`at`** — where the silence sat, as a fraction `0..1` of the clip's span. This is the key that survives a different speaker at a different pace.
- **`after_ref`** — the reference word index the silence followed, when the clip has word boundaries (a synthesis does; an aligned or uploaded clip does not).
- **`secs`** — how long the reference held it.

**Derivation** (`fluency::reference_profile`, called from `mod.rs::clip_profile`): run the same energy VAD over the clip's PCM that the attempt is measured with, take the span from the TTS word boundaries when they exist and from the VAD segments otherwise, and emit one `RefPause` per inter-segment gap ≥ 0.25 s. Nothing is stored: the profile is derived per score, because the only expensive artefact here (the phoneme model) is cached elsewhere and a VAD pass over a clip is milliseconds of RMS. A cache would buy nothing and would owe an invalidation rule of its own.

**It is always measured the way the take is.** Same detector, same thresholds, same span definition — so a reference scored against its own profile reads exactly `r = 1.0`, every silence matches, `penalty = 0`, and the score is **95.0** (asserted by a unit test).

**When there is no usable clip** (`DefaultClip::Missing` — no clip on this device; or `Stale` — the clip predates the current text), the fallback answers and `ref_wcpm` is absent from the stored metrics, so the number still says which bar produced it:

- pace against a fixed **130 wcpm** — the median of the 36 measured reference clips, i.e. a statement about the material, not about reading research;
- pauses by punctuation: a mid-phrase silence costs from **0.4 s**, one at punctuation from **1.6 s**.

Cruder, but the same idea: a breath is not an error, and no comparison is invented that the device cannot make.

## 4. Pause charging, precisely

Only *reading* pauses are considered — leading silence (before the first word) and trailing (after the last) are reported but never charged; they are the recorder, not the reader.

For each reading pause of length `s`:

1. **Match it to a reference pause.** If the pause carries a reference word index (`after_ref`, available with word-level placement) and the profile has a pause after that same word *and that reference pause sits within 8% of the span of where the take's silence actually fell*, that is the match. The position check is there because an index is only exact while the clip's word sequence equals the text's: a synthesis that splits one written number into two words shifts every later index by one. Otherwise — and for phrase-only placement — compare normalized positions: the take's pause at `(start − span_start) / reading_span` matches the nearest reference pause within **2%** of the span (≈2.4 s on a two-minute text, ≈0.4 s on a 20-second one — placement error scales with the material, so the tolerance does).
2. **Matched** → the allowance is `max(1.8 · ref.secs, 0.4 s)`. `s` within it is free; beyond it, one **overrun**.
3. **Unmatched** → one **hesitation**. The reader stopped where the text did not.

Without a profile, step 3 becomes: a mid-phrase silence ≥ 0.4 s, or an at-punctuation silence ≥ 1.6 s, is a hesitation.

Two properties this buys, both of which the old formula lacked:

- **Length invariance.** The same delivery density scores the same on a 100-word text and a 400-word one (a unit test asserts it).
- **The bar is the text, not a table.** A three-minute dialogue that breathes between turns, a poem read slowly, a fast news item: each is judged against the rendition that was actually offered.

## 5. Why these numbers

Every constant is a judgement, and each is chosen so the *shape* is defensible even where the constant could be argued:

- **1.0 → 95, not 100.** The take is compared against one rendition. Matching it is a good score, not an unimprovable one; 100 stays reachable (a slightly brisker take), which keeps the scale honest at the top instead of pinning good readers to a ceiling they cannot pass.
- **0.6 → 55 as the floor.** Below that the reading is laboured — at the measured corpus median that is 79 wcpm, roughly half a narrator's pace. It is a floor rather than a cliff so a very slow but otherwise clean reading is not scored as if it were incoherent.
- **Plateau above 1.25.** Fast reading is not better reading; a take too fast to be intelligible is punished by the repairs and misreads it produces, which is where that defect belongs.
- **0.05 per hesitation, per 100 words.** Reproduces the previous calibration at the ~100-word take the old formula was tuned on, so the change is not a silent re-scaling of how harsh the axis is — only of what it counts and how it grows.
- **Overrun cheaper than hesitation (0.03 vs 0.05).** Holding a breath the reference also took is a smaller error than stopping where the text never stops.
- **Cap 0.35.** A disastrous delivery cannot wipe out a genuinely correct pace; the worst score still says "your speed is real, your fluency isn't".
- **2% position tolerance, 1.8× length factor, 0.4 s slack.** Loose enough that a different speaker's timing is not punished, tight enough that an extra stop in the middle of a phrase cannot hide inside a neighbouring one.

## 6. Measured after the change (same 36 clips)

| | median | range |
|---|---|---|
| Old algorithm, reference audio as a take | 51.5 | 46.0 – 55.7 |
| **New, reference scored against its own profile** | **95.0** | 95.0 – 95.0 |
| New, the same delivery at 90% of the reference pace | 85.0 | — |
| New, reference pace + one inserted hesitation per 100 words | 90.2 | 89.8 – 91.4 |
| New, no clip on the device (fallback norm 130) | 95.2 | — |

The separation the axis exists for survives: reading the text the way it was read scores in the nineties, reading it a little slower lands in the mid-eighties, and stopping where the text does not stop costs about five points per hundred words. What is gone is the ~40 points of credit withheld from everyone by a norm no clip in the dataset met.

Reproduce with `.qoder/probe_fluency_new.py` (both algorithms over the same decoded audio) after `.qoder/probe_fluency_c1.py` has materialised the wav cache.

## 7. What this does not measure

Stated plainly, because a number invites more belief than it can carry:

- **Intonation, stress, expressiveness.** Nothing here looks at pitch.
- **Comprehension.** Reading fluently without understanding scores fluently.
- **Word-level latency.** A uniformly slow reader is banded `slow` and loses pace credit, but is not "disfluent" — the axis has no per-word reaction time, because the alignment does not measure one.
- **A reference that is itself wrong.** If the clip mispronounces, the clip is the bar; accuracy catches a text/clip mismatch (§ the `Stale` state refuses rather than re-synthesising), not a mispronounced word inside a correct clip.
- **Very short texts.** On a 25-word text, one hesitation is 4% of the words; per-100 normalization is noisy there and the cap does most of the work. Below ~30 words, treat the score as a hint.
- **A genuinely unusual rendition.** If the dataset's default clip is a dramatic reading with long theatrical pauses, the bar is that reading. This is the design's one deliberate subjectivity.

## 8. Compatibility, stored data, and the UI

- `Fluency::score` keeps its meaning and its optionality: rows stored before it existed still read `None`, and the UI still falls back to the raw `wcpm` rather than inventing a number.
- `Fluency::ref_wcpm` is **additive** — `Option<f64>`, `#[serde(default)]`, omitted when the fallback answered, `skip_serializing_if` so the ~2 KB row budget is unaffected. No `SCHEMA_VERSION` bump, no protocol change: an old peer ignores the field, a new reader of an old row sees `None`.
- `band` (`slow` / `target` / `fast`) now means *relative to this text* (`r < 0.75`, `r > 1.15`), not absolute wcpm. It is a display chip only; nothing stores a decision on it.
- The UI shows the bar beside the number (`84 correct wpm against 131`), and the tooltip says whether the reference or the fallback norm answered.

## 9. Open questions for an outside reader

These are the places the design is weakest, in the order they would matter:

1. **Is a single-rendition reference the right bar at all?** It makes the score reproducible and offline, but it inherits one speaker's habits. A dataset with several clips per text could average their profiles — is that better than "match this teacher", or does it make the bar unattainable in a different way?
2. **Is a normalized position (`at`) the right pause key when word indices exist?** Word index is exact but brittle: one omitted word shifts every later index, which is why it is now gated by `pause_index_trust_span`. The remaining assumption is that pauses are spread proportionally through the text, which a long first sentence violates.
3. **Should pace above 1.0 earn any credit before 1.25?** The current 95→100 slope over 0.25 is a compromise: it rewards a confident take slightly without making speed the goal.
4. **Is 0.05 : 0.03 the right ratio between stopping and over-holding?** It is asserted, not measured; there are no human ratings in this project to fit it against.
5. **Is `pace_score · (1 − penalty)` the right composition?** A `min(pace, smoothness)` would punish a fast-but-clean take differently; a sum would let a very fast take buy its way out of hesitations.
6. **Are 0.4 s / 1.6 s the right fallback tolerances across languages?** They are one German corpus's cadence.
7. **Should the axis band a reader whose pace is right but whose phrasing is entirely different?** Today, phrasing is judged only where it produces silence.

## 10. Tuning knobs

All of it lives in `FluencyConfig` (`fluency.rs`), one `Default` impl:

| field | default | role |
|---|---|---|
| `min_pause_secs` | 0.25 | a gap shorter than this is inside a word, not between them |
| `long_pause_secs` | 1.0 | reported separately (`long_pause_count`), not scored |
| `slow_ratio` / `fast_ratio` | 0.75 / 1.15 | the `band` chip's edges |
| `fallback_ref_wcpm` | 130.0 | the bar when no clip is available |
| `attribution_tolerance_secs` | 0.08 | how far past a word's `end` a silence may start and still belong to it |
| `phrase_boundary_tolerance_secs` | 0.6 | how far outside a silence a *phrase* edge may fall and still explain it |
| `pause_match_tolerance` | 0.02 | position match window, as a fraction of the span |
| `pause_index_trust_span` | 0.08 | how far from its claimed position a matched word index may sit and still be trusted |
| `pause_length_factor` | 1.8 | a matched pause may be this many times the reference's |
| `pause_extra_secs` | 0.4 | …or this much longer, whichever is larger |
| `fallback_hesitation_secs` | 0.4 | mid-phrase silence that costs, without a profile |
| `fallback_punctuation_pause_secs` | 1.6 | at-punctuation silence that costs, without a profile |

The curve anchors (0.6 / 1.0 / 1.25 and 55 / 95 / 100) are constants in `pace_score_ratio`, named in its doc comment; the charge weights and the 0.35 cap are constants in `disfluency_penalty`.

## 11. Test surface

Pure functions carry the tests, so threshold tuning stays safe:

| test | asserts |
|---|---|
| `pace_score_follows_the_anchors` | 0.6 → 55, 0.8 → 75, 1.0 → 95, 1.25 → 100, plateau beyond |
| `disfluency_penalty_costs_shares_and_caps` | the weights at 100 words, and the cap |
| `a_long_take_is_not_penalised_for_being_long` | the same density scores the same at 200 and 800 words |
| `a_reference_scores_against_itself_without_a_penalty` | a clip scored against its own profile: `ref_wcpm` set, penalty 0, score > 94 |
| `a_pause_the_reference_took_is_free_and_an_inserted_one_is_not` | the matched pause is free; a silence inserted mid-phrase costs ≥ 3 points |
| `without_a_profile_the_punctuation_rule_still_tolerates_a_breath` | the fallback path leaves `ref_wcpm` absent and does not collapse a comma pause |
| the existing pause/rate tests (`a_gap_covered_by_speech_is_not_a_pause`, `leading_and_trailing_silence_are_labelled_but_not_counted`, `degenerate_inputs_produce_no_nan`, …) | nothing about silence detection or rate arithmetic regressed |

Gate commands: `cargo check --offline`, `cargo check --offline --features desktop`, `cargo test --offline --features desktop --lib`, `npx tsc --noEmit`.
