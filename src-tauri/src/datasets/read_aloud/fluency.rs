//! Fluency metrics: speech rate, pauses, runs and restarts.
//!
//! Two evidence sources are combined, and neither alone is enough:
//!
//! * **STT timings** ([`Timings`]) say *what* was being read and when. They
//!   anchor a silence to the text; they do not find silences, and they come at
//!   two resolutions — per word, or per phrase for an engine that can only
//!   place phrases (whisper's timestamp tokens today).
//! * **VAD segments** ([`super::vad`]) say when the room was quiet. Every pause
//!   counted here is detected on the waveform, so the *number* of pauses, their
//!   lengths and the reading span do not depend on which STT model ran.
//!
//! What the timings' resolution decides is the *label*: a silence after a comma
//! is a sentence-boundary pause, a silence mid-phrase is a hesitation. Per-word
//! spans answer that from the word's own punctuation, which
//! `asr_metric::normalize_words` preserves next to each compared word instead of
//! throwing away. Per-phrase spans can only say whether a boundary of the model
//! fell inside the silence, so the label gets coarser and the word it followed
//! stays unreported — [`Pause::after_ref`] is `None`, not guessed.
//!
//! Both halves of the score are judged against the text's **standard audio**
//! ([`ReferenceProfile`]), not against a published reading norm. The pace is a
//! ratio to the reference's own words per minute, and a silence costs only when
//! the reference speaker did not take it — or took a materially shorter one at
//! that point. A published norm describes a different text, language and genre:
//! measured against the 36 reference clips of a German C1 dataset, a flawless
//! professional reading scored 46-57, because 100 wcpm worth of credit was
//! withheld (the norm sat at 180) and the pauses it was entitled to take were
//! charged as hesitations.

use serde::{Deserialize, Serialize};

use crate::datasets::asr_metric::{NormWord, Verdict, WordVerdict};

use super::vad::Segment;

/// One word the STT model placed in time. Deserialized from the frontend, which
/// gets it from `model_transcribe_detailed`, so it must mirror that command's
/// JSON exactly.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct TimedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// One *phrase* the STT model placed in time — the coarser twin of
/// [`TimedWord`], deserialized from the same command's `segments`. What it can
/// say is limited to what it knows: where a phrase began and ended, never where
/// one word did.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub(crate) struct TimedSegment {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// How a scored take was placed in time. Not the model's capability — that is
/// `models::catalog_stt::TimestampGranularity`, which answers for a *model* —
/// this answers for one stored measurement, so a number read back months later
/// still says what kind of evidence produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TimingEvidence {
    Words,
    Segments,
}

/// What the STT run said about *when* the take was read.
///
/// The two lists are alternatives, not additions, and the choice between them is
/// the ladder: word spans are strictly finer, so when the engine gave them, the
/// phrase spans are dropped rather than consulted. Keeping both in play would
/// make a pause's label depend on which list happened to be asked first.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Timings<'a> {
    words: &'a [TimedWord],
    segments: &'a [TimedSegment],
}

impl<'a> Timings<'a> {
    pub(crate) fn new(words: &'a [TimedWord], segments: &'a [TimedSegment]) -> Self {
        Self { words, segments: if words.is_empty() { segments } else { &[] } }
    }

    /// From what the scoring commands receive: either list may be absent.
    pub(crate) fn from_optional(
        words: Option<&'a [TimedWord]>,
        segments: Option<&'a [TimedSegment]>,
    ) -> Self {
        Self::new(words.unwrap_or_default(), segments.unwrap_or_default())
    }

    /// Whether anything placed this take in time at all. `false` is the one case
    /// fluency goes unmeasured; `true` with only segments means measured, coarser.
    pub(crate) fn is_empty(&self) -> bool {
        self.words.is_empty() && self.segments.is_empty()
    }

    /// The per-word spans, for the word list's own timeline. Empty whenever the
    /// engine could only place phrases, which is what makes those entries'
    /// `start`/`end` reportable as absent rather than as zero.
    pub(crate) fn words(&self) -> &'a [TimedWord] {
        self.words
    }

    /// Which evidence a stored assessment should record for itself.
    pub(crate) fn evidence(&self) -> Option<TimingEvidence> {
        if !self.words.is_empty() {
            Some(TimingEvidence::Words)
        } else if !self.segments.is_empty() {
            Some(TimingEvidence::Segments)
        } else {
            None
        }
    }

    /// Where the reading began and ended, from the placement available.
    ///
    /// A phrase's edges are the model's guess, not the audio's, so the span is
    /// slightly wider than the word span and every rate divided by it slightly
    /// *slower* — the conservative direction, since a slow reader is the case the
    /// band is there to notice.
    fn span(&self) -> Option<(f64, f64)> {
        match (self.words.first(), self.words.last()) {
            (Some(f), Some(l)) => return Some((f.start, l.end)),
            // Words asked for and none returned: fall through, the same answer
            // a segment-only engine would give.
            (None, None) => {}
            _ => return None,
        }
        match (self.segments.first(), self.segments.last()) {
            (Some(f), Some(l)) => Some((f.start, l.end)),
            _ => None,
        }
    }

    /// Label one VAD silence: which reference word it followed, and whether that
    /// was the end of a phrase.
    ///
    /// `mapping` is the alignment's hypothesis-to-reference index, which only
    /// per-word placement can enter.
    fn label_gap(
        &self,
        gap_start: f64,
        gap_end: f64,
        mapping: &[Option<usize>],
        ref_words: &[NormWord],
        cfg: &FluencyConfig,
    ) -> (Option<usize>, bool) {
        if !self.words.is_empty() {
            let Some(h) = word_before(gap_start, self.words, cfg.attribution_tolerance_secs) else {
                return (None, false);
            };
            let ref_index = mapping.get(h).copied().flatten();
            return (ref_index, ends_phrase(ref_index.and_then(|i| ref_words.get(i))));
        }
        // Positive evidence only, and the wider tolerance a phrase timestamp
        // deserves: a boundary the model placed inside the silence means the
        // silence is where it thought a phrase ended. Anywhere else counts as a
        // hesitation, which is the same conservative default the word path uses.
        // Nothing pretends to know which word came before.
        let at_boundary = self.segments.iter().any(|s| {
            s.end >= gap_start - cfg.phrase_boundary_tolerance_secs
                && s.end <= gap_end + cfg.phrase_boundary_tolerance_secs
        });
        (None, at_boundary)
    }
}

/// One silence the reference speaker took, in the form a learner's pause can be
/// matched against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RefPause {
    /// Reference word the silence followed, when the clip's evidence named one.
    /// A TTS clip's word boundaries do; an uploaded clip's do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_ref: Option<usize>,
    /// Where the silence sat in the reading, `0..1` of the clip's span. The
    /// position that survives a different speaker at a different pace, and the
    /// only way to match a pause when the evidence is phrase-only.
    pub at: f64,
    /// How long the reference held it.
    pub secs: f64,
}

/// The pace and pause shape of a text's **standard audio** — what a take is
/// judged against.
///
/// This is the measurement that makes the score mean something for *this* text:
/// a three-minute dialogue breathes between turns, a poem is read slowly, a
/// German audiobook runs at 105-150 wcpm. A norm table has to be right for all
/// of them; the clip only has to be right for itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ReferenceProfile {
    /// The reference reader's words per minute over its reading span.
    pub wcpm: f64,
    /// The silences it took, so the same pause from a learner is free.
    #[serde(default)]
    pub pauses: Vec<RefPause>,
}

impl ReferenceProfile {
    /// The reference pause a learner's pause corresponds to, if any.
    ///
    /// A word index is exact evidence and wins *when it agrees with where the
    /// silence actually fell*: a clip whose boundaries split one written word
    /// into two (a number, a hyphenated compound) shifts every later index by
    /// one, so an index that claims a pause far from the measured position is a
    /// shifted index, not a match. Position is the fallback and the tie-breaker,
    /// because that is all phrase-only placement (and an uploaded clip) offers.
    fn matching(
        &self,
        pause: &Pause,
        span_start: f64,
        reading_span: f64,
        cfg: &FluencyConfig,
    ) -> Option<&RefPause> {
        if reading_span <= 1e-6 {
            return None;
        }
        let at = ((pause.start - span_start) / reading_span).clamp(0.0, 1.0);
        if let Some(index) = pause.after_ref {
            if let Some(found) = self.pauses.iter().find(|r| r.after_ref == Some(index)) {
                if (found.at - at).abs() <= cfg.pause_index_trust_span {
                    return Some(found);
                }
            }
        }
        self.pauses
            .iter()
            .map(|r| ((r.at - at).abs(), r))
            .filter(|(delta, _)| *delta <= cfg.pause_match_tolerance)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, r)| r)
    }
}

/// The reference profile of one clip, from evidence the clip already carries:
/// its VAD segments and, for a synthesis, its per-word boundaries.
///
/// `ref_words` is how many words the reference text has; it is the numerator of
/// the pace, so a clip that does not say the text would be caught by accuracy,
/// not hidden here. `None` when the clip holds nothing measurable — no speech
/// segment, or a span too short to divide by.
pub(crate) fn reference_profile(
    speech_spans: &[Segment],
    words: &[TimedWord],
    ref_words: usize,
    cfg: &FluencyConfig,
) -> Option<ReferenceProfile> {
    let first = speech_spans.first()?;
    let last = speech_spans.last()?;
    if ref_words == 0 {
        return None;
    }
    // Word boundaries are the reader's own edges; the VAD's are padded by a few
    // frames on each side and would report every rate slightly slower. Where
    // there are none (an aligned or uploaded clip) the segments are all there is.
    let (span_start, span_end) = match (words.first(), words.last()) {
        (Some(f), Some(l)) if l.end > f.start => (f.start, l.end),
        _ => (first.start, last.end),
    };
    let reading_span = span_end - span_start;
    if reading_span <= 1e-6 {
        return None;
    }

    let pauses = speech_spans
        .windows(2)
        .filter_map(|pair| {
            let secs = pair[1].start - pair[0].end;
            (secs >= cfg.min_pause_secs).then(|| RefPause {
                after_ref: word_before(pair[0].end, words, cfg.attribution_tolerance_secs),
                at: ((pair[0].end - span_start) / reading_span).clamp(0.0, 1.0),
                secs,
            })
        })
        .collect();

    Some(ReferenceProfile { wcpm: per_minute(ref_words, reading_span), pauses })
}

/// Every threshold and band edge, in one place.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FluencyConfig {
    /// Silence shorter than this is a stop consonant, not a pause.
    pub min_pause_secs: f64,
    /// A pause this long is reported separately — it usually means the reader
    /// lost their place rather than breathed.
    pub long_pause_secs: f64,
    /// Read this much slower than the reference and the band says `slow`.
    pub slow_ratio: f64,
    /// Read this much faster than the reference and the band says `fast`.
    pub fast_ratio: f64,
    /// The pace ratio a take is scored against when the text has no usable
    /// standard clip. The median reference pace of the 36 clips measured in
    /// German C1 audiobook material (105-149 wcpm); a fallback describes the
    /// material this app is used with, not reading research, and is labelled as
    /// such wherever it surfaces.
    pub fallback_ref_wcpm: f64,
    /// How far past a word's `end` a pause may start and still be attributed
    /// to it (word timings are not sample-accurate).
    pub attribution_tolerance_secs: f64,
    /// How far outside a VAD silence a *phrase* boundary may fall and still be
    /// counted as the reason for it. Deliberately much wider than the word
    /// tolerance: a segment timestamp is the model's guess at a phrase edge, and
    /// whisper's own boundaries drift by a few hundred milliseconds.
    pub phrase_boundary_tolerance_secs: f64,
    /// How near, as a fraction of the reading span, a learner's pause must start
    /// to count as the same pause the reference speaker took. Two percent of a
    /// two-minute take is ~2.4 s, of a 20 s take ~0.4 s — the tolerance scales
    /// with the material because placement error does too.
    pub pause_match_tolerance: f64,
    /// How far from the position it claims a matched word index may sit before
    /// it is read as a shifted index rather than a match. Wider than
    /// `pause_match_tolerance` because the index is exact evidence *when* the
    /// clip's word sequence equals the text's, and it earns the benefit of the
    /// doubt up to here.
    pub pause_index_trust_span: f64,
    /// A matched pause costs only what it exceeds: this multiple of the
    /// reference pause, or `pause_extra_secs` if that is larger. Breathing is
    /// not the offence — holding the breath twice as long as the reader who
    /// showed you how is.
    pub pause_length_factor: f64,
    /// Slack on top of a matched pause, so a slightly longer breath is free.
    pub pause_extra_secs: f64,
    /// Without a reference profile, a mid-phrase silence this long is a
    /// hesitation.
    pub fallback_hesitation_secs: f64,
    /// Without a reference profile, a silence at punctuation is free up to this
    /// length — past it the reader most likely lost their place.
    pub fallback_punctuation_pause_secs: f64,
}

impl Default for FluencyConfig {
    fn default() -> Self {
        Self {
            min_pause_secs: 0.25,
            long_pause_secs: 1.0,
            slow_ratio: 0.75,
            fast_ratio: 1.15,
            fallback_ref_wcpm: 130.0,
            attribution_tolerance_secs: 0.08,
            phrase_boundary_tolerance_secs: 0.6,
            pause_match_tolerance: 0.02,
            pause_index_trust_span: 0.08,
            pause_length_factor: 1.8,
            pause_extra_secs: 0.4,
            fallback_hesitation_secs: 0.4,
            fallback_punctuation_pause_secs: 1.6,
        }
    }
}

/// Where a silence fell, relative to the reference text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PauseKind {
    /// Before the first word — recording start-up, not reading.
    Leading,
    /// After a comma, full stop, question or exclamation mark.
    AtPunctuation,
    /// Mid-phrase: a hesitation.
    WithinPhrase,
    /// After the last word — recording tail.
    Trailing,
}

impl PauseKind {
    fn is_reading_pause(self) -> bool {
        matches!(self, PauseKind::AtPunctuation | PauseKind::WithinPhrase)
    }
}

/// One detected silence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pause {
    pub start: f64,
    pub end: f64,
    pub kind: PauseKind,
    /// Index into the reference words of the word this pause followed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_ref: Option<usize>,
}

impl Pause {
    pub(crate) fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// Reading-speed band, from the attempt's words-per-minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RateBand {
    Slow,
    Target,
    Fast,
}

/// The aggregate fluency picture. No per-frame or per-word data lives here —
/// that would not fit the ~2 KB metrics budget, and it is recomputable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fluency {
    /// Length of the whole recording, in seconds.
    pub duration: f64,
    /// Time actually spent reading: the first placed span to the last one,
    /// minus the pauses in between. Leading/trailing silence is excluded, so
    /// fumbling for the record button does not look like a slow reader.
    pub speaking_time: f64,
    /// Correct words per minute — the classic reading-fluency figure. Counts
    /// only *correct* words, so misread words cost twice: fewer in the
    /// numerator, same denominator.
    pub wcpm: f64,
    /// The fluency axis's own 0-100 score: [`pace_score_ratio`] of the take's
    /// pace against the reference's, discounted by [`disfluency_penalty`].
    /// `None` on rows stored before the number existed — the reader falls back
    /// to showing the raw `wcpm` rather than inventing a score nobody measured.
    #[serde(default)]
    pub score: Option<f64>,
    /// The pace the score was judged against, when it came from the text's own
    /// standard clip. Absent means the fallback norm answered
    /// ([`FluencyConfig::fallback_ref_wcpm`]), so the number still says what
    /// produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_wcpm: Option<f64>,
    /// Spoken words per minute over the reading span (includes pauses).
    pub speech_rate: f64,
    /// Spoken words per minute excluding pauses.
    pub articulation_rate: f64,
    /// Pauses while reading (at punctuation or mid-phrase).
    pub pause_count: usize,
    /// Of those, the ones at or beyond [`FluencyConfig::long_pause_secs`].
    pub long_pause_count: usize,
    /// Share of reading pauses that fell mid-phrase — the hesitancy proxy.
    pub within_phrase_pause_ratio: f64,
    /// Mean length of run: words between pauses.
    pub mean_length_of_run: f64,
    /// Single-word repeats (stutters).
    pub repetition_count: usize,
    /// Multi-word repeats (restarts).
    pub restart_count: usize,
    pub band: RateBand,
}

/// Per-minute rate from a duration in seconds, guarding zero (no NaN, ever —
/// this value is serialized to the frontend and stored).
fn per_minute(count: usize, secs: f64) -> f64 {
    if secs <= 1e-6 {
        0.0
    } else {
        count as f64 / (secs / 60.0)
    }
}

/// The pace half of the fluency score, 0-100, as a curve over the take's pace
/// *ratio* to the reference's: `0.6 → 55`, `1.0 → 95`, saturating at 100 by
/// `1.25`.
///
/// Reading at the pace of the clip the learner was shown scores 95, not 100: a
/// take is being compared against one rendition, and matching it is not the same
/// as being unimprovable. The plateau above 1.25 is deliberate — racing earns no
/// extra credit, and a too-fast-to-be-intelligible take is punished by pauses
/// and repairs, not by the curve.
pub(crate) fn pace_score_ratio(ratio: f64) -> f64 {
    const FLOOR: f64 = 0.6;
    const FLOOR_SCORE: f64 = 55.0;
    const TARGET_SCORE: f64 = 95.0;
    const FULL: f64 = 1.25;
    if !(ratio > 0.0) || !ratio.is_finite() {
        0.0
    } else if ratio < FLOOR {
        ratio / FLOOR * FLOOR_SCORE
    } else if ratio <= 1.0 {
        FLOOR_SCORE + (ratio - FLOOR) * (TARGET_SCORE - FLOOR_SCORE) / (1.0 - FLOOR)
    } else {
        (TARGET_SCORE + (ratio - 1.0) * (100.0 - TARGET_SCORE) / (FULL - 1.0)).min(100.0)
    }
}

/// The smoothness discount on [`pace_score_ratio`], in [0, `PENALTY_CAP`].
///
/// The charges are **per hundred words**, not per take. `0.05` for a hesitation
/// reproduces the previous calibration at the ~100-word take the old formula was
/// tuned on, and a four-hundred-word text no longer collects four times the
/// penalty for the same delivery — with a cap, a fixed per-take count stopped
/// measuring anything at all past about a minute of audio, which is how every
/// reference clip in a C1 dataset scored the flat maximum.
///
/// The cap keeps a disastrous delivery from wiping out a genuinely good pace:
/// the worst possible score is 65% of it, which still says "your speed is real,
/// your fluency isn't".
fn disfluency_penalty(
    hesitation_count: usize,
    overrun_count: usize,
    restart_count: usize,
    repetition_count: usize,
    spoken_words: usize,
) -> f64 {
    const PENALTY_CAP: f64 = 0.35;
    let per_hundred = 100.0 / spoken_words.max(1) as f64;
    ((0.05 * hesitation_count as f64
        + 0.03 * overrun_count as f64
        + 0.05 * restart_count as f64
        + 0.02 * repetition_count as f64)
        * per_hundred)
        .min(PENALTY_CAP)
}

/// Split the reading pauses into the two kinds that cost anything.
///
/// *A hesitation* is a silence in a place the reference did not pause — the
/// reader stopped to think. *An overrun* is a pause in a place the reference did
/// pause, held materially longer (`pause_length_factor`, plus
/// `pause_extra_secs` of slack). Everything else is delivery: the reference
/// speaker took the same breath at the same point, and the learner is entitled
/// to it.
///
/// Without a reference profile the rule falls back to punctuation: a mid-phrase
/// silence is a hesitation once it passes `fallback_hesitation_secs`, and one at
/// punctuation is free until `fallback_punctuation_pause_secs`. Cruder, but it is
/// the same idea — a breath is not an error — and it never invents a comparison
/// it does not have.
fn pause_charges(
    pauses: &[&Pause],
    reference: Option<&ReferenceProfile>,
    span_start: f64,
    reading_span: f64,
    cfg: &FluencyConfig,
) -> (usize, usize) {
    let (mut hesitations, mut overruns) = (0usize, 0usize);
    for pause in pauses {
        let secs = pause.duration();
        match reference {
            Some(profile) => match profile.matching(pause, span_start, reading_span, cfg) {
                Some(ref_pause) => {
                    let allowed =
                        (cfg.pause_length_factor * ref_pause.secs).max(cfg.pause_extra_secs);
                    if secs > allowed {
                        overruns += 1;
                    }
                }
                None => hesitations += 1,
            },
            None => {
                let tolerated = if pause.kind == PauseKind::AtPunctuation {
                    cfg.fallback_punctuation_pause_secs
                } else {
                    cfg.fallback_hesitation_secs
                };
                if secs >= tolerated {
                    hesitations += 1;
                }
            }
        }
    }
    (hesitations, overruns)
}

/// `hyp_index → ref_index`, derived from the alignment.
fn hyp_to_ref(verdicts: &[WordVerdict]) -> Vec<Option<usize>> {
    let len = verdicts
        .iter()
        .filter_map(|v| v.hyp_index)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let mut map = vec![None; len];
    for v in verdicts {
        if let (Some(h), Some(r)) = (v.hyp_index, v.ref_index) {
            map[h] = Some(r);
        }
    }
    map
}

/// Count stutters (one-word repeats) and restarts (multi-word repeats).
fn count_repeats(verdicts: &[WordVerdict]) -> (usize, usize) {
    let (mut repetitions, mut restarts) = (0usize, 0usize);
    let mut run = 0usize;
    let mut flush = |run: &mut usize| {
        if *run == 1 {
            repetitions += 1;
        } else if *run >= 2 {
            restarts += 1;
        }
        *run = 0;
    };
    for v in verdicts {
        if v.verdict == Verdict::Repeated {
            run += 1;
        } else {
            flush(&mut run);
        }
    }
    flush(&mut run);
    (repetitions, restarts)
}

/// The word a silence followed: last word ending at or before the silence.
fn word_before(pause_start: f64, words: &[TimedWord], tolerance: f64) -> Option<usize> {
    let mut found = None;
    for (i, w) in words.iter().enumerate() {
        if w.end <= pause_start + tolerance {
            found = Some(i);
        } else {
            break;
        }
    }
    found
}

/// Does this reference word end a phrase?
fn ends_phrase(word: Option<&NormWord>) -> bool {
    match word {
        Some(w) => w.punct.sentence_end || !w.punct.trailing.is_empty(),
        None => false,
    }
}

/// Compute fluency plus the pause list, from waveform segments and whatever
/// placement the STT run had.
///
/// Pure: no I/O, no model, no clock. Every division is guarded, so an empty
/// recording, a one-word recording and a recording the STT returned nothing for
/// numbers instead of `NaN`.
///
/// `reference` is the text's own standard audio when the device has a fresh
/// clip for it; `None` falls back to the punctuation rule and the fallback norm.
pub(crate) fn compute(
    duration: f64,
    speech_spans: &[Segment],
    timings: &Timings,
    verdicts: &[WordVerdict],
    ref_words: &[NormWord],
    reference: Option<&ReferenceProfile>,
    correct_words: usize,
    cfg: &FluencyConfig,
) -> (Fluency, Vec<Pause>) {
    let mapping = hyp_to_ref(verdicts);
    let mut pauses: Vec<Pause> = Vec::new();

    if let Some(first) = speech_spans.first() {
        if first.start >= cfg.min_pause_secs {
            pauses.push(Pause { start: 0.0, end: first.start, kind: PauseKind::Leading, after_ref: None });
        }
    }
    for pair in speech_spans.windows(2) {
        let (gap_start, gap_end) = (pair[0].end, pair[1].start);
        let gap = gap_end - gap_start;
        if gap >= cfg.min_pause_secs {
            let (after_ref, phrase) =
                timings.label_gap(gap_start, gap_end, &mapping, ref_words, cfg);
            pauses.push(Pause {
                start: gap_start,
                end: gap_end,
                kind: if phrase { PauseKind::AtPunctuation } else { PauseKind::WithinPhrase },
                after_ref,
            });
        }
    }
    if let Some(last) = speech_spans.last() {
        let tail = duration - last.end;
        if tail >= cfg.min_pause_secs {
            pauses.push(Pause { start: last.end, end: duration, kind: PauseKind::Trailing, after_ref: None });
        }
    }

    let reading_pauses: Vec<&Pause> = pauses.iter().filter(|p| p.kind.is_reading_pause()).collect();
    let pause_count = reading_pauses.len();
    let long_pause_count = reading_pauses
        .iter()
        .filter(|p| p.duration() >= cfg.long_pause_secs)
        .count();
    let within_phrase = reading_pauses
        .iter()
        .filter(|p| p.kind == PauseKind::WithinPhrase)
        .count();
    let within_phrase_pause_ratio = if pause_count == 0 {
        0.0
    } else {
        within_phrase as f64 / pause_count as f64
    };
    let pause_total: f64 = reading_pauses.iter().map(|p| p.duration()).sum();

    // The reading span, not the recording length: the margins between pressing
    // record and starting to read are not part of the performance. Which list
    // the edges come from changes their precision, never what they mean.
    let (span_start, span_end) = timings.span().unwrap_or((0.0, 0.0));
    let reading_span = (span_end - span_start).max(0.0);
    let speaking_time = (reading_span - pause_total).max(0.0);

    // Counted from the alignment, never from the timing list: the two
    // granularities hold different numbers of entries, and a rate whose
    // numerator changed meaning with the model would stop being comparable
    // across models. One sentence of six words is six words however it was
    // placed.
    let spoken_words = mapping.len();
    let wcpm = per_minute(correct_words, reading_span);
    let speech_rate = per_minute(spoken_words, reading_span);
    let articulation_rate = per_minute(spoken_words, speaking_time);
    let mean_length_of_run = if spoken_words == 0 {
        0.0
    } else {
        spoken_words as f64 / (pause_count + 1) as f64
    };

    // The bar this take is measured against. A profile that cannot be divided by
    // (an empty clip) is not a bar, so it is dropped here rather than scored
    // against: the fallback then answers and `ref_wcpm` stays absent.
    let measured_ref = reference.map(|r| r.wcpm).filter(|w| *w > 1e-6);
    let ratio = match measured_ref {
        Some(ref_wcpm) => wcpm / ref_wcpm,
        None => wcpm / cfg.fallback_ref_wcpm,
    };

    let band = if reading_span <= 1e-6 {
        RateBand::Target
    } else if ratio < cfg.slow_ratio {
        RateBand::Slow
    } else if ratio > cfg.fast_ratio {
        RateBand::Fast
    } else {
        RateBand::Target
    };

    let (repetition_count, restart_count) = count_repeats(verdicts);

    // The axis's own number: pace against the reference, discounted by the
    // disfluencies the reference did not commit. Derived from measurements that
    // are all stored beside it, so a reader can always re-explain the score
    // from `wcpm`, `ref_wcpm` and the pause/repair counts.
    let score = if reading_span <= 1e-6 {
        None
    } else {
        let (hesitations, overruns) =
            pause_charges(&reading_pauses, reference, span_start, reading_span, cfg);
        Some(pace_score_ratio(ratio)
            * (1.0
                - disfluency_penalty(
                    hesitations,
                    overruns,
                    restart_count,
                    repetition_count,
                    spoken_words,
                )))
    };

    (
        Fluency {
            duration,
            speaking_time,
            wcpm,
            score,
            ref_wcpm: measured_ref,
            speech_rate,
            articulation_rate,
            pause_count,
            long_pause_count,
            within_phrase_pause_ratio,
            mean_length_of_run,
            repetition_count,
            restart_count,
            band,
        },
        pauses,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::asr_metric::normalize_words;

    fn seg(a: f64, b: f64) -> Segment {
        Segment { start: a, end: b }
    }

    fn word(text: &str, start: f64, end: f64) -> TimedWord {
        TimedWord { text: text.to_string(), start, end }
    }

    fn phrase(text: &str, start: f64, end: f64) -> TimedSegment {
        TimedSegment { text: text.to_string(), start, end }
    }

    /// Verdicts for a transcript read perfectly against `n` reference words.
    fn perfect_verdicts(n: usize) -> Vec<WordVerdict> {
        (0..n)
            .map(|i| WordVerdict {
                ref_index: Some(i),
                hyp_index: Some(i),
                verdict: Verdict::Correct,
            })
            .collect()
    }

    #[test]
    fn a_gap_covered_by_speech_is_not_a_pause() {
        // Two VAD segments but one continuous span of words: the words fill the
        // gap, so the "silence" the segments imply never happened.
        let words = vec![word("a", 0.0, 0.4), word("b", 0.45, 0.9)];
        let segments = vec![seg(0.0, 0.9)];
        let (f, pauses) = compute(
            0.9, &segments, &Timings::new(&words, &[]), &perfect_verdicts(2), &normalize_words("a b"), None, 2,
            &FluencyConfig::default(),
        );
        assert_eq!(f.pause_count, 0);
        assert!(pauses.is_empty());
        assert!(f.wcpm > 100.0);
    }

    #[test]
    fn a_gap_between_segments_is_a_pause() {
        let words = vec![word("hallo", 0.0, 0.4), word("welt", 1.0, 1.4)];
        let segments = vec![seg(0.0, 0.45), seg(0.95, 1.45)];
        let (f, pauses) = compute(
            1.45, &segments, &Timings::new(&words, &[]), &perfect_verdicts(2), &normalize_words("hallo welt"), None, 2,
            &FluencyConfig::default(),
        );
        assert_eq!(f.pause_count, 1);
        assert_eq!(f.long_pause_count, 0);
        assert_eq!(pauses[0].kind, PauseKind::WithinPhrase);
        assert!((pauses[0].start - 0.45).abs() < 1e-9);
        assert!((pauses[0].end - 0.95).abs() < 1e-9);
        // The reading span is 1.4 s, of which 0.5 s is pausing.
        assert!((f.speaking_time - 0.9).abs() < 1e-9, "speaking_time: {}", f.speaking_time);
        assert!(f.articulation_rate > f.speech_rate);
    }

    #[test]
    fn a_pause_after_a_comma_is_at_punctuation() {
        let words = vec![word("hallo", 0.0, 0.4), word("welt", 1.2, 1.6)];
        let segments = vec![seg(0.0, 0.45), seg(1.15, 1.65)];
        let (_, pauses) = compute(
            1.65, &segments, &Timings::new(&words, &[]), &perfect_verdicts(2),
            &normalize_words("hallo, welt"), None, 2, &FluencyConfig::default(),
        );
        assert_eq!(pauses[0].kind, PauseKind::AtPunctuation);
        assert_eq!(pauses[0].after_ref, Some(0));
    }

    #[test]
    fn a_phrase_boundary_inside_the_silence_labels_it_at_punctuation() {
        // No word rows at all: an engine that places phrases only. The model
        // ended a phrase inside the silence, so the silence is where a phrase
        // ended — and nothing is pretended about which word came before.
        let phrases = vec![phrase("hallo welt", 0.0, 0.5), phrase("und tschuss", 0.95, 1.45)];
        let segments = vec![seg(0.0, 0.45), seg(0.95, 1.45)];
        let (f, pauses) = compute(
            1.45, &segments, &Timings::new(&[], &phrases), &perfect_verdicts(4),
            &normalize_words("hallo welt und tschuss"), None, 4, &FluencyConfig::default(),
        );
        assert_eq!(pauses.len(), 1);
        assert_eq!(pauses[0].kind, PauseKind::AtPunctuation);
        assert_eq!(pauses[0].after_ref, None, "a phrase boundary cannot name a word");
        // Every number the words were never the source of is still measured:
        // four words over a 1.45 s span, 0.5 s of it paused.
        assert!((f.speaking_time - 0.95).abs() < 1e-9, "speaking_time: {}", f.speaking_time);
        assert!(f.wcpm > 100.0, "wcpm: {}", f.wcpm);
        assert_eq!(f.mean_length_of_run, 2.0);
    }

    #[test]
    fn a_silence_inside_a_phrase_is_still_a_hesitation() {
        // One phrase covers the whole take, so the model offered no boundary
        // where the room went quiet.
        let phrases = vec![phrase("hallo und tschuss welt", 0.0, 2.0)];
        let segments = vec![seg(0.0, 0.85), seg(1.35, 2.0)];
        let (_, pauses) = compute(
            2.0, &segments, &Timings::new(&[], &phrases), &perfect_verdicts(4),
            &normalize_words("hallo und tschuss welt"), None, 4, &FluencyConfig::default(),
        );
        assert_eq!(pauses[0].kind, PauseKind::WithinPhrase);
    }

    #[test]
    fn word_spans_win_when_both_kinds_arrive() {
        // The ladder, in one assertion: a phrase boundary saying
        // `AtPunctuation` must not outvote the word's own punctuation.
        let words = vec![word("hallo", 0.0, 0.4), word("welt", 1.0, 1.4)];
        let phrases = vec![phrase("hallo welt", 0.0, 1.4)];
        let segments = vec![seg(0.0, 0.45), seg(0.95, 1.45)];
        let (_, pauses) = compute(
            1.45, &segments, &Timings::new(&words, &phrases), &perfect_verdicts(2),
            &normalize_words("hallo welt"), None, 2, &FluencyConfig::default(),
        );
        assert_eq!(pauses[0].kind, PauseKind::WithinPhrase);
        assert_eq!(pauses[0].after_ref, Some(0));
    }

    #[test]
    fn leading_and_trailing_silence_are_labelled_but_not_counted() {
        let words = vec![word("hi", 0.5, 0.8)];
        let segments = vec![seg(0.45, 0.85)];
        let (f, pauses) = compute(
            1.5, &segments, &Timings::new(&words, &[]), &perfect_verdicts(1), &normalize_words("hi"), None, 1,
            &FluencyConfig::default(),
        );
        assert_eq!(f.pause_count, 0, "margins are not reading pauses");
        assert_eq!(pauses.len(), 2);
        assert_eq!(pauses[0].kind, PauseKind::Leading);
        assert_eq!(pauses[1].kind, PauseKind::Trailing);
        assert!((f.duration - 1.5).abs() < 1e-9);
        // Rates use the reading span (0.3 s), not the 1.5 s recording.
        assert!((f.speaking_time - 0.3).abs() < 1e-9);
    }

    #[test]
    fn bands_follow_the_configured_edges() {
        let cfg = FluencyConfig::default();
        // One word in 0.3 s of reading span = 200 wcpm → fast.
        let words = vec![word("schnell", 1.0, 1.3)];
        let (fast, _) = compute(1.3, &[seg(1.0, 1.3)], &Timings::new(&words, &[]), &perfect_verdicts(1),
            &normalize_words("schnell"), None, 1, &cfg);
        assert_eq!(fast.band, RateBand::Fast);

        // 10 words in 10 s = 60 wcpm → slow.
        let words: Vec<TimedWord> =
            (0..10).map(|i| word("wort", i as f64, i as f64 + 0.5)).collect();
        let (slow, _) = compute(10.0, &[seg(0.0, 10.0)], &Timings::new(&words, &[]), &perfect_verdicts(10),
            &normalize_words("wort wort wort wort wort wort wort wort wort wort"), None, 10, &cfg);
        assert_eq!(slow.band, RateBand::Slow);
    }

    #[test]
    fn repeats_are_split_into_stutters_and_restarts() {
        let mut verdicts = vec![
            WordVerdict { ref_index: Some(0), hyp_index: Some(0), verdict: Verdict::Correct },
            // one-word repeat
            WordVerdict { ref_index: None, hyp_index: Some(1), verdict: Verdict::Repeated },
            WordVerdict { ref_index: Some(1), hyp_index: Some(2), verdict: Verdict::Correct },
            // two-word restart
            WordVerdict { ref_index: None, hyp_index: Some(3), verdict: Verdict::Repeated },
            WordVerdict { ref_index: None, hyp_index: Some(4), verdict: Verdict::Repeated },
            WordVerdict { ref_index: Some(2), hyp_index: Some(5), verdict: Verdict::Correct },
        ];
        let words: Vec<TimedWord> = (0..6)
            .map(|i| word("w", i as f64 * 0.5, i as f64 * 0.5 + 0.4))
            .collect();
        let (f, _) = compute(3.0, &[seg(0.0, 3.0)], &Timings::new(&words, &[]), &verdicts, &normalize_words("w w w"), None, 3,
            &FluencyConfig::default());
        assert_eq!(f.repetition_count, 1);
        assert_eq!(f.restart_count, 1);
        verdicts.clear();
    }

    #[test]
    fn degenerate_inputs_produce_no_nan() {
        let cfg = FluencyConfig::default();
        for (duration, segments, words) in [
            (0.0, Vec::new(), Vec::new()),
            (2.0, Vec::new(), Vec::new()),
            (0.0, vec![seg(0.0, 0.0)], Vec::new()),
            (0.5, vec![seg(0.2, 0.2)], vec![word("a", 0.2, 0.2)]),
        ] {
            let (f, _) = compute(duration, &segments, &Timings::new(&words, &[]), &[], &[], None, 0, &cfg);
            for v in [
                f.duration, f.speaking_time, f.wcpm, f.speech_rate, f.articulation_rate,
                f.within_phrase_pause_ratio, f.mean_length_of_run,
                f.score.unwrap_or(0.0),
            ] {
                assert!(v.is_finite(), "non-finite metric: {v:?}");
            }
        }
    }

    #[test]
    fn pace_score_follows_the_anchors() {
        assert_eq!(pace_score_ratio(0.0), 0.0);
        assert!((pace_score_ratio(0.6) - 55.0).abs() < 1e-9);
        assert!((pace_score_ratio(1.0) - 95.0).abs() < 1e-9);
        assert!((pace_score_ratio(1.25) - 100.0).abs() < 1e-9);
        // Monotone through the middle: half-way between the floor and the
        // reference pace is half-way between their scores.
        assert!((pace_score_ratio(0.8) - 75.0).abs() < 1e-9);
        // The plateau: racing earns nothing extra, and never exceeds 100.
        assert!((pace_score_ratio(2.5) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn disfluency_penalty_costs_shares_and_caps() {
        assert_eq!(disfluency_penalty(0, 0, 0, 0, 100), 0.0);
        // 2 hesitations + 4 overruns + 1 restart = 0.10 + 0.12 + 0.05.
        assert!((disfluency_penalty(2, 4, 1, 0, 100) - 0.27).abs() < 1e-9);
        // The cap: 0.47 of charges at 40 words still only costs 0.35.
        assert_eq!(disfluency_penalty(4, 6, 2, 0, 40), 0.35);
    }

    #[test]
    fn a_long_take_is_not_penalised_for_being_long() {
        // The same delivery density: 1 hesitation per 100 words, twice the
        // text. Under the old per-take counts the second one collected double.
        let short = disfluency_penalty(2, 0, 0, 0, 200);
        let long = disfluency_penalty(8, 0, 0, 0, 800);
        assert!((short - long).abs() < 1e-9, "{short} vs {long}");
    }

    #[test]
    fn a_reference_scores_against_itself_without_a_penalty() {
        let cfg = FluencyConfig::default();
        // Ten words at half a second each, read straight through with one 1.2 s
        // breath after the fifth. The clip the profile describes is the take
        // being scored, so everything matches: the pace ratio is 1.0 and no
        // silence is a hesitation.
        let words: Vec<TimedWord> = (0..10)
            .map(|i| word("wort", i as f64 * 0.5, i as f64 * 0.5 + 0.5))
            .collect();
        let mut segments: Vec<Segment> =
            (0..5).map(|i| seg(i as f64 * 0.5, i as f64 * 0.5 + 0.5)).collect();
        segments.extend((0..5).map(|i| seg(3.7 + i as f64 * 0.5, 4.2 + i as f64 * 0.5)));

        let profile = reference_profile(&segments, &words, 10, &cfg).expect("profile");
        assert!((profile.wcpm - per_minute(10, 5.0)).abs() < 1e-6);
        assert_eq!(profile.pauses.len(), 1);
        assert_eq!(profile.pauses[0].after_ref, Some(4));

        let (f, _) = compute(
            6.2, &segments, &Timings::new(&words, &[]), &perfect_verdicts(10),
            &normalize_words("wort wort wort wort wort wort wort wort wort wort"),
            Some(&profile), 10, &cfg,
        );
        assert_eq!(f.ref_wcpm, Some(profile.wcpm));
        assert!(f.score.expect("score") > 94.0, "{}", f.score.unwrap());
    }

    #[test]
    fn a_pause_the_reference_took_is_free_and_an_inserted_one_is_not() {
        let cfg = FluencyConfig::default();
        let reference_words: Vec<TimedWord> = (0..10)
            .map(|i| word("wort", i as f64 * 0.6, i as f64 * 0.6 + 0.5))
            .collect();
        let mut ref_segments: Vec<Segment> =
            (0..5).map(|i| seg(i as f64 * 0.6, i as f64 * 0.6 + 0.5)).collect();
        ref_segments.extend((0..5).map(|i| seg(3.4 + i as f64 * 0.6, 3.9 + i as f64 * 0.6)));
        let profile = reference_profile(&ref_segments, &reference_words, 10, &cfg).expect("profile");
        let text = normalize_words("wort wort wort wort wort wort wort wort wort wort");

        // Same pause, same place: the learner took the breath the reference took.
        let (free, _) = compute(
            6.4, &ref_segments, &Timings::new(&reference_words, &[]), &perfect_verdicts(10),
            &text, Some(&profile), 10, &cfg,
        );

        // The reference's pause kept, plus one it never took: the reader stopped
        // for 0.7 s between 4.5 s and 5.2 s, where the reference ran on.
        let hesitating = vec![
            seg(0.0, 0.5), seg(0.6, 1.1), seg(1.2, 1.7), seg(1.8, 2.3), seg(2.4, 2.9),
            seg(3.4, 3.9), seg(4.0, 4.5),
            seg(5.2, 5.7), seg(5.8, 6.3), seg(6.4, 6.9), seg(7.0, 7.5),
        ];
        let (charged, pauses) = compute(
            7.5, &hesitating, &Timings::new(&reference_words, &[]), &perfect_verdicts(10),
            &text, Some(&profile), 10, &cfg,
        );

        let free_score = free.score.expect("score");
        let charged_score = charged.score.expect("score");
        assert_eq!(pauses.iter().filter(|p| p.kind.is_reading_pause()).count(), 2);
        // Ten words is a short text: one hesitation is half a point of penalty
        // per hundred words, so the cap is what it costs here.
        assert!(
            (charged_score - free_score * 0.65).abs() < 1e-6,
            "{charged_score} vs {free_score}"
        );
    }

    #[test]
    fn without_a_profile_the_punctuation_rule_still_tolerates_a_breath() {
        let cfg = FluencyConfig::default();
        let words = vec![word("hallo", 0.0, 0.4), word("welt", 1.6, 2.0)];
        // A 1.2 s silence after a comma: long, but punctuation entitles it.
        let segments = vec![seg(0.0, 0.45), seg(1.55, 2.0)];
        let (f, _) = compute(
            2.0, &segments, &Timings::new(&words, &[]), &perfect_verdicts(2),
            &normalize_words("hallo, welt"), None, 2, &cfg,
        );
        assert_eq!(f.ref_wcpm, None, "the fallback answered, so say so");
        // 2 words / 2 s = 60 wcpm → ratio 0.46 of the 130 fallback norm.
        assert!(f.score.expect("score") > 40.0, "{}", f.score.unwrap());
    }
}
