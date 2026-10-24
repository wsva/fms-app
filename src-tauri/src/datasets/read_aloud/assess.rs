//! The single assessment seam: turn one attempt into one scored picture.
//!
//! Three axes, deliberately independent, deliberately not combined into a
//! composite:
//!
//! | axis | source | changed by this work? |
//! |---|---|---|
//! | accuracy | `asr_metric::score_asr` (WIP × 100) | **no** — byte-identical |
//! | fluency | energy VAD + STT word timings | new |
//! | pronunciation | CTC phoneme GOP against the text's standard audio | new |
//!
//! The accuracy number keeps its exact meaning because XP thresholds (60/80),
//! `best_score`, sync payloads and every stored attempt depend on it. Everything
//! new is an added field — see the invariant note in `docs/design/read-aloud-score.md`.
//!
//! Nothing here performs I/O. Evidence gathering (STT, TTS, ONNX) happens in
//! [`super`] and is handed in, which is what makes the whole module testable
//! against hand-written tables instead of recorded audio.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::datasets::asr_metric::{
    normalize_tokens, normalize_words, AsrScoreSummary, Verdict,
};

use super::fluency::{self, Fluency, FluencyConfig, Pause, TimedWord};
use super::gop::{self, LogPost, PhoneSpan};
use super::vad::{self, VadConfig};

/// Bumped when the shape below changes incompatibly. Stored metrics from an
/// older schema are still readable (`#[serde(default)]` on the reader side)
/// because every field is additive.
pub(crate) const SCHEMA_VERSION: i64 = 1;

/// `tau` of the GOP → score mapping. Larger flattens the curve. A heuristic:
/// replacing it with a calibrated mapping needs teacher ratings this project
/// does not collect.
pub(crate) const GOP_TAU: f32 = 2.5;

/// Stored metrics are capped so one attempt row stays small. Word detail is
/// only kept for the words worth showing, and the pause ribbon is truncated.
///
/// The cap is measured, not guessed: one stored pause costs ~48 bytes
/// (`{"start":12.04,"end":12.31,"kind":"within_phrase"}`), the aggregates
/// another ~520, so 24 pauses lands just inside the 2 KB row budget. Raising it
/// needs a narrower pause shape, not just a bigger number.
pub(crate) const MAX_STORED_WEAK_WORDS: usize = 12;
pub(crate) const MAX_STORED_PAUSES: usize = 24;

/// What this attempt's pipeline could actually measure. Reported so a caller (and
/// an agent) can tell "pronunciation was fine" from "pronunciation was not
/// measured".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct Capabilities {
    /// The STT model returned per-word timings, so fluency could be computed.
    #[serde(default)]
    pub word_timings: bool,
    /// GOP ran.
    #[serde(default)]
    pub gop: bool,
    /// A dataset-owned reference clip supplied the phone timeline.
    #[serde(default)]
    pub tts_reference: bool,
}

/// Why an axis is missing. First-class states, never a silently substituted
/// proxy — a fake score is worse than an honest gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PronReason {
    /// No phoneme model is installed.
    ModelMissing,
    /// The phoneme model exists but could not be loaded or run.
    ModelError,
    /// The text has no standard audio, so there is nothing to compare against.
    NoReference,
    /// The standard audio was made from an earlier version of the text, so its
    /// phone target no longer matches the words being compared.
    StaleReference,
    /// Legacy: the reference timeline used to be fetched from Edge TTS while
    /// scoring. Never emitted now that the dataset owns the clip; kept so
    /// attempts scored before that change still deserialize.
    NoNetwork,
    /// Legacy, as above: the on-demand reference could not be synthesised.
    TtsFailed,
    /// Fewer frames than reference phones — the attempt is too short to align.
    TooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccuracyCounts {
    #[serde(rename = "ref")]
    pub reference: usize,
    pub correct: usize,
    pub minor: usize,
    pub substituted: usize,
    pub omitted: usize,
    pub inserted: usize,
    pub repeated: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Accuracy {
    /// The WIP score, unchanged by this work.
    pub score: f64,
    /// The same shape with homophone/near-miss substitutions counted as hits.
    pub lenient_score: f64,
    pub wer: f64,
    pub counts: AccuracyCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pronunciation {
    /// `None` when nothing was measured — see `unavailable_reason`.
    #[serde(default)]
    pub score: Option<f64>,
    /// `"ctc_gop"` when measured.
    #[serde(default)]
    pub method: Option<String>,
    /// Share of scored words below [`gop::WEAK_WORD_SCORE`].
    #[serde(default)]
    pub weak_ratio: Option<f64>,
    #[serde(default)]
    pub unavailable_reason: Option<PronReason>,
}

impl Pronunciation {
    fn unavailable(reason: PronReason) -> Self {
        Self { score: None, method: None, weak_ratio: None, unavailable_reason: Some(reason) }
    }
}

/// One reference word with everything known about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WordAssessment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_index: Option<usize>,
    /// The compared form of the reference word (lowercased, punctuation gone).
    #[serde(rename = "ref")]
    pub ref_word: String,
    /// The compared form of what was heard (empty for an omission).
    #[serde(default)]
    pub hyp: String,
    pub verdict: Verdict,
    /// Timings from the STT model, when it gave any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// GOP score of this word in `(0, 1]`, when pronunciation was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pron: Option<f64>,
}

/// The whole picture for one attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Assessment {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(default)]
    pub capabilities: Capabilities,
    pub accuracy: Accuracy,
    /// `None` when the STT model produced no word timings — fluency is not
    /// guessed from transcript gaps.
    #[serde(default)]
    pub fluency: Option<Fluency>,
    pub pronunciation: Pronunciation,
    #[serde(default)]
    pub words: Vec<WordAssessment>,
    #[serde(default)]
    pub pauses: Vec<Pause>,
}

/// Pronunciation evidence, either measured or explicitly absent.
pub(crate) enum PronEvidence<'a> {
    Unavailable(PronReason),
    Measured {
        /// Learner log-posteriors along the recording.
        post: &'a LogPost,
        /// Per reference word, the phone spans it was aligned to.
        spans: &'a [Vec<PhoneSpan>],
        tau: f32,
    },
}

/// Build the assessment. Pure: everything model-shaped is an argument.
pub(crate) fn assess(
    summary: &AsrScoreSummary,
    reference: &str,
    recognized: &str,
    wav: &[f32],
    words: Option<&[TimedWord]>,
    pron: PronEvidence<'_>,
) -> Assessment {
    let ref_words = normalize_words(reference);
    let hyp_tokens = normalize_tokens(recognized);

    let accuracy = Accuracy {
        score: summary.score,
        lenient_score: summary.lenient_score,
        wer: summary.wer,
        counts: AccuracyCounts {
            reference: summary.ref_len,
            correct: summary.hits,
            minor: summary.minor,
            substituted: summary.substitutions,
            omitted: summary.deletions,
            inserted: summary.insertions,
            repeated: summary.repeated,
        },
    };

    // --- pronunciation -----------------------------------------------------
    let mut word_pron: Vec<Option<f64>> = vec![None; ref_words.len()];
    let pronunciation = match pron {
        PronEvidence::Unavailable(reason) => Pronunciation::unavailable(reason),
        PronEvidence::Measured { post, spans, tau } => {
            let mut scored: Vec<(usize, f64)> = Vec::new();
            for (index, word_spans) in spans.iter().enumerate() {
                let Some(word) = ref_words.get(index) else { continue };
                if let Some(score) = gop::word_score(post, word_spans, tau) {
                    word_pron[index] = Some(score);
                    scored.push((word.norm.chars().count(), score));
                }
            }
            match gop::rollup(&scored) {
                Some((score, weak_ratio)) => Pronunciation {
                    score: Some(score),
                    method: Some("ctc_gop".to_string()),
                    weak_ratio: Some(weak_ratio),
                    unavailable_reason: None,
                },
                None => Pronunciation::unavailable(PronReason::TooShort),
            }
        }
    };

    // --- words -------------------------------------------------------------
    let timed = words.unwrap_or(&[]);
    let word_list: Vec<WordAssessment> = summary
        .words
        .iter()
        .map(|v| {
            let ref_word = v
                .ref_index
                .and_then(|i| ref_words.get(i))
                .map(|w| w.norm.clone())
                .unwrap_or_default();
            let hyp = v
                .hyp_index
                .and_then(|i| hyp_tokens.get(i))
                .cloned()
                .unwrap_or_default();
            let timing = v.hyp_index.and_then(|i| timed.get(i));
            WordAssessment {
                ref_index: v.ref_index,
                ref_word,
                hyp,
                verdict: v.verdict,
                start: timing.map(|t| t.start),
                end: timing.map(|t| t.end),
                pron: v.ref_index.and_then(|i| word_pron.get(i).copied().flatten()),
            }
        })
        .collect();

    // --- fluency -----------------------------------------------------------
    let duration = wav.len() as f64 / vad::SAMPLE_RATE as f64;
    let (fluency, pauses) = if words.is_some() {
        let segments = vad::detect(wav, &VadConfig::default());
        let (metrics, pauses) = fluency::compute(
            duration,
            &segments,
            timed,
            &summary.words,
            &ref_words,
            summary.hits,
            &FluencyConfig::default(),
        );
        (Some(metrics), pauses)
    } else {
        (None, Vec::new())
    };

    Assessment {
        schema_version: SCHEMA_VERSION,
        capabilities: Capabilities {
            word_timings: words.is_some(),
            gop: pronunciation.score.is_some(),
            tts_reference: matches!(pron, PronEvidence::Measured { .. }),
        },
        accuracy,
        fluency,
        pronunciation,
        words: word_list,
        pauses,
    }
}

impl Assessment {
    /// The compact form stored per attempt: aggregates, the twelve weakest
    /// words and at most forty pauses. Per-frame and per-phone data is never
    /// stored — it is re-derived from the device-local recording on demand.
    pub(crate) fn metrics_json(&self) -> String {
        let mut compact = self.clone();
        compact.words = self.weakest_words(MAX_STORED_WEAK_WORDS);
        compact.pauses = self.pauses.iter().take(MAX_STORED_PAUSES).cloned().collect();
        serde_json::to_string(&compact).unwrap_or_default()
    }

    /// The words worth showing, worst first. Empty when pronunciation was not
    /// measured — accuracy detail lives in the alignment chunks, not here.
    fn weakest_words(&self, limit: usize) -> Vec<WordAssessment> {
        let mut weak: Vec<WordAssessment> = self
            .words
            .iter()
            .filter(|w| w.pron.map(|p| p < gop::WEAK_WORD_SCORE).unwrap_or(false))
            .cloned()
            .collect();
        weak.sort_by(|a, b| {
            a.pron
                .partial_cmp(&b.pron)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        weak.truncate(limit);
        weak
    }

    /// A short human/agent-readable summary line, used by the MCP tools so an
    /// agent does not have to interpret the whole structure to act.
    pub(crate) fn headline(&self) -> String {
        let mut parts = vec![format!("accuracy {:.1}", self.accuracy.score)];
        if let Some(f) = &self.fluency {
            parts.push(format!("{} wpm", f.wcpm.round() as i64));
            parts.push(format!("{} pause(s)", f.pause_count));
        } else {
            parts.push("fluency not measured".to_string());
        }
        match self.pronunciation.score {
            Some(score) => parts.push(format!("pronunciation {:.0}", score * 100.0)),
            None => parts.push("pronunciation not measured".to_string()),
        }
        parts.join(", ")
    }

    /// The `Value` shape the MCP tools return.
    pub(crate) fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "error": "unserializable" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::asr_metric::score_asr;

    fn timed(n: usize, step: f64) -> Vec<TimedWord> {
        (0..n)
            .map(|i| TimedWord {
                text: "wort".into(),
                start: i as f64 * step,
                end: i as f64 * step + step * 0.7,
            })
            .collect()
    }

    /// 16 kHz samples: 200 ms tone, 400 ms silence, 200 ms tone.
    ///
    /// The bursts must outlast [`vad::VadConfig::min_speech_secs`] (150 ms) or
    /// the detector correctly rejects them as clicks, and the gap must outlast
    /// a pad (40 ms) plus [`fluency::FluencyConfig::min_pause_secs`] (250 ms).
    fn two_bursts() -> Vec<f32> {
        let mut s = Vec::new();
        for (n, amp) in [(3200, 0.3f32), (6400, 0.0), (3200, 0.3)] {
            for i in 0..n {
                let t = i as f32 / 16_000.0;
                s.push((2.0 * std::f32::consts::PI * 200.0 * t).sin() * amp);
            }
        }
        s
    }

    #[test]
    fn accuracy_is_the_untouched_wip_score() {
        // The invariant in one assertion: whatever else happens, this holds.
        for (r, h) in [
            ("the cat sat", "the cat sat on"),
            ("a b c", "a c"),
            ("guten tag", "guten tag"),
            ("Der Hund bellt laut im Garten", "Der Hund bellt laut im park"),
        ] {
            let summary = score_asr(r, h);
            let assessment = assess(
                &summary, r, h, &[], None, PronEvidence::Unavailable(PronReason::ModelMissing),
            );
            assert_eq!(assessment.accuracy.score, summary.score);
            assert_eq!(assessment.accuracy.lenient_score, summary.lenient_score);
            assert_eq!(assessment.schema_version, 1);
        }
    }

    #[test]
    fn no_word_timings_means_no_fluency_not_zero_fluency() {
        let summary = score_asr("hello world", "hello world");
        let a = assess(&summary, "hello world", "hello world", &two_bursts(), None,
            PronEvidence::Unavailable(PronReason::ModelMissing));
        assert!(a.fluency.is_none());
        assert!(a.pauses.is_empty());
        assert!(!a.capabilities.word_timings);
        assert_eq!(a.pronunciation.unavailable_reason, Some(PronReason::ModelMissing));
        assert!(a.pronunciation.score.is_none());
        assert!(a.headline().contains("fluency not measured"));
    }

    #[test]
    fn word_timings_produce_fluency_and_pauses() {
        let wav = two_bursts();
        let words = vec![
            TimedWord { text: "hallo".into(), start: 0.0, end: 0.2 },
            TimedWord { text: "welt".into(), start: 0.6, end: 0.8 },
        ];
        let summary = score_asr("hallo welt", "hallo welt");
        let a = assess(&summary, "hallo welt", "hallo welt", &wav, Some(&words),
            PronEvidence::Unavailable(PronReason::ModelMissing));
        let f = a.fluency.expect("timings present");
        assert!(a.capabilities.word_timings);
        assert_eq!(f.pause_count, 1);
        assert!(f.wcpm > 0.0);
        assert_eq!(a.pauses.len(), 1);
        assert_eq!(a.words.len(), 2);
        assert!((a.words[0].start.unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn weakest_words_are_ranked_and_capped() {
        let summary = score_asr("a b c d e f g h i j k l m n", "a b c d e f g h i j k l m n");
        let mut a = assess(&summary, "a b c d e f g h i j k l m n", "a b c d e f g h i j k l m n",
            &[], Some(&timed(14, 0.5)),
            PronEvidence::Unavailable(PronReason::ModelMissing));
        // Hand-assign descending scores so the ranking is unambiguous.
        for (i, w) in a.words.iter_mut().enumerate() {
            w.pron = Some(0.1 + 0.05 * i as f64);
        }
        let weak = a.weakest_words(3);
        assert_eq!(weak.len(), 3);
        assert!(weak[0].pron.unwrap() < weak[1].pron.unwrap());
        assert!(weak[2].pron.unwrap() < gop::WEAK_WORD_SCORE);
    }

    #[test]
    fn stored_metrics_stay_small_and_round_trip() {
        let summary = score_asr("hallo welt", "hallo welt");
        let mut a = assess(&summary, "hallo welt", "hallo welt", &[], None,
            PronEvidence::Unavailable(PronReason::ModelMissing));
        a.pauses = (0..100)
            .map(|i| Pause {
                start: i as f64,
                end: i as f64 + 0.3,
                kind: fluency::PauseKind::WithinPhrase,
                after_ref: None,
            })
            .collect();
        let json = a.metrics_json();
        assert!(json.len() < 2048, "stored metrics must stay under 2 KB: {} bytes", json.len());
        let back: Assessment = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back.pauses.len(), MAX_STORED_PAUSES);
        assert_eq!(back.accuracy.score, a.accuracy.score);
    }
}
