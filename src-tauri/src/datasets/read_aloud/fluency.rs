//! Fluency metrics: speech rate, pauses, runs and restarts.
//!
//! Two evidence sources are combined, and neither alone is enough:
//!
//! * **Word timings** (from an STT model that emits them) say *when* something
//!   was said, but they are anchored to words — a gap between two words is
//!   equally consistent with a pause and with a slow speaker.
//! * **VAD segments** ([`super::vad`]) say when the room was quiet, but know
//!   nothing about what was being read.
//!
//! So pauses are detected on the waveform and then *labelled* using the word
//! timings: a silence after a comma is a sentence-boundary pause, a silence
//! mid-phrase is a hesitation. That distinction needs the reference text's
//! punctuation, which `asr_metric::normalize_words` preserves next to each
//! compared word instead of throwing away.

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

/// Every threshold and band edge, in one place.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FluencyConfig {
    /// Silence shorter than this is a stop consonant, not a pause.
    pub min_pause_secs: f64,
    /// A pause this long is reported separately — it usually means the reader
    /// lost their place rather than breathed.
    pub long_pause_secs: f64,
    /// Below this words-per-minute the reading is `slow`.
    pub slow_wcpm: f64,
    /// Above this words-per-minute it is `fast`.
    pub target_wcpm: f64,
    /// How far past a word's `end` a pause may start and still be attributed
    /// to it (word timings are not sample-accurate).
    pub attribution_tolerance_secs: f64,
}

impl Default for FluencyConfig {
    fn default() -> Self {
        Self {
            min_pause_secs: 0.25,
            long_pause_secs: 1.0,
            slow_wcpm: 80.0,
            target_wcpm: 140.0,
            attribution_tolerance_secs: 0.08,
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
    /// Time actually spent reading: first word to last word, minus the pauses
    /// in between. Leading/trailing silence is excluded, so fumbling for the
    /// record button does not look like a slow reader.
    pub speaking_time: f64,
    /// Correct words per minute — the classic reading-fluency figure. Counts
    /// only *correct* words, so misread words cost twice: fewer in the
    /// numerator, same denominator.
    pub wcpm: f64,
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

/// Compute fluency plus the pause list, from waveform segments and word timings.
///
/// Pure: no I/O, no model, no clock. Every division is guarded, so an empty
/// recording, a one-word recording and a recording the STT returned nothing for
/// numbers instead of `NaN`.
pub(crate) fn compute(
    duration: f64,
    segments: &[Segment],
    words: &[TimedWord],
    verdicts: &[WordVerdict],
    ref_words: &[NormWord],
    correct_words: usize,
    cfg: &FluencyConfig,
) -> (Fluency, Vec<Pause>) {
    let mapping = hyp_to_ref(verdicts);
    let mut pauses: Vec<Pause> = Vec::new();

    let at_word = |pause_start: f64, words: &[TimedWord]| -> (Option<usize>, bool) {
        match word_before(pause_start, words, cfg.attribution_tolerance_secs) {
            Some(h) => {
                let ref_index = mapping.get(h).copied().flatten();
                let phrase = ends_phrase(ref_index.and_then(|r| ref_words.get(r)));
                (ref_index, phrase)
            }
            None => (None, false),
        }
    };

    if let Some(first) = segments.first() {
        if first.start >= cfg.min_pause_secs {
            pauses.push(Pause { start: 0.0, end: first.start, kind: PauseKind::Leading, after_ref: None });
        }
    }
    for pair in segments.windows(2) {
        let gap = pair[1].start - pair[0].end;
        if gap >= cfg.min_pause_secs {
            let (after_ref, phrase) = at_word(pair[0].end, words);
            pauses.push(Pause {
                start: pair[0].end,
                end: pair[1].start,
                kind: if phrase { PauseKind::AtPunctuation } else { PauseKind::WithinPhrase },
                after_ref,
            });
        }
    }
    if let Some(last) = segments.last() {
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
    // record and starting to read are not part of the performance.
    let (span_start, span_end) = match (words.first(), words.last()) {
        (Some(f), Some(l)) => (f.start, l.end),
        _ => (0.0, 0.0),
    };
    let reading_span = (span_end - span_start).max(0.0);
    let speaking_time = (reading_span - pause_total).max(0.0);

    let spoken_words = words.len();
    let wcpm = per_minute(correct_words, reading_span);
    let speech_rate = per_minute(spoken_words, reading_span);
    let articulation_rate = per_minute(spoken_words, speaking_time);
    let mean_length_of_run = if spoken_words == 0 {
        0.0
    } else {
        spoken_words as f64 / (pause_count + 1) as f64
    };

    let band = if reading_span <= 1e-6 {
        RateBand::Target
    } else if wcpm < cfg.slow_wcpm {
        RateBand::Slow
    } else if wcpm > cfg.target_wcpm {
        RateBand::Fast
    } else {
        RateBand::Target
    };

    let (repetition_count, restart_count) = count_repeats(verdicts);

    (
        Fluency {
            duration,
            speaking_time,
            wcpm,
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
            0.9, &segments, &words, &perfect_verdicts(2), &normalize_words("a b"), 2,
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
            1.45, &segments, &words, &perfect_verdicts(2), &normalize_words("hallo welt"), 2,
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
            1.65, &segments, &words, &perfect_verdicts(2),
            &normalize_words("hallo, welt"), 2, &FluencyConfig::default(),
        );
        assert_eq!(pauses[0].kind, PauseKind::AtPunctuation);
        assert_eq!(pauses[0].after_ref, Some(0));
    }

    #[test]
    fn leading_and_trailing_silence_are_labelled_but_not_counted() {
        let words = vec![word("hi", 0.5, 0.8)];
        let segments = vec![seg(0.45, 0.85)];
        let (f, pauses) = compute(
            1.5, &segments, &words, &perfect_verdicts(1), &normalize_words("hi"), 1,
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
        let (fast, _) = compute(1.3, &[seg(1.0, 1.3)], &words, &perfect_verdicts(1),
            &normalize_words("schnell"), 1, &cfg);
        assert_eq!(fast.band, RateBand::Fast);

        // 10 words in 10 s = 60 wcpm → slow.
        let words: Vec<TimedWord> =
            (0..10).map(|i| word("wort", i as f64, i as f64 + 0.5)).collect();
        let (slow, _) = compute(10.0, &[seg(0.0, 10.0)], &words, &perfect_verdicts(10),
            &normalize_words("wort wort wort wort wort wort wort wort wort wort"), 10, &cfg);
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
        let (f, _) = compute(3.0, &[seg(0.0, 3.0)], &words, &verdicts, &normalize_words("w w w"), 3,
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
            let (f, _) = compute(duration, &segments, &words, &[], &[], 0, &cfg);
            for v in [
                f.duration, f.speaking_time, f.wcpm, f.speech_rate, f.articulation_rate,
                f.within_phrase_pause_ratio, f.mean_length_of_run,
            ] {
                assert!(v.is_finite(), "non-finite metric: {v:?}");
            }
        }
    }
}
