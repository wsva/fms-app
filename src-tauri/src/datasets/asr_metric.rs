//! Word-level ASR metrics: align a reference text against an STT transcript
//! and score how faithfully it was read. Used by the read-aloud feature
//! (`score = WIP × 100`, plus the alignment chunks the UI highlights).
//!
//! The alignment algorithm and a subset of the tests are adapted from
//! github.com/ryaningli/rwer v0.2.2 (MIT). Deliberate deviations:
//! - No `rapidfuzz` (the crate uses it only in auxiliary distance helpers;
//!   the core path is the same single-row DP + banded Wagner–Fischer).
//! - No pluggable transform pipeline: one fixed German-aware normalization
//!   (`normalize_tokens`) instead.
//! - NFC normalization is applied to the *word* path too — upstream only does
//!   it for CER, so a decomposed umlaut in a transcript would count as a
//!   substitution there.
//! - `ß` is folded to `ss` so Swiss/South-German "Strasse" matches a
//!   reference "Straße".
//! - Visualization/error-collection APIs are replaced by serializable chunks
//!   (`AsrChunk`) that the frontend renders directly.
//!
//! `textsim` (Ratcliff-Obershelp) remains the metric for cue alignment; this
//! module is for transcript-vs-reference scoring only.

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

// ============================================================
// Alignment (adapted from rwer::alignment)
// ============================================================

/// A single edit operation in the reference↔hypothesis alignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditOp {
    /// Reference and hypothesis tokens match (rwer keeps a single `index` here;
    /// both sides are tracked so chunk building can look up either word list).
    Equal {
        ref_index: usize,
        hyp_index: usize,
    },
    /// Reference token was substituted by a hypothesis token.
    Substitute { ref_index: usize, hyp_index: usize },
    /// Hypothesis token was inserted (not in reference).
    Insert { hyp_index: usize },
    /// Reference token was deleted (missing from hypothesis).
    Delete { ref_index: usize },
}

/// Counts of each edit operation type from an alignment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct OperationCounts {
    hits: usize,
    substitutions: usize,
    deletions: usize,
    insertions: usize,
}

fn count_operations(ops: &[EditOp]) -> OperationCounts {
    let mut counts = OperationCounts::default();
    for op in ops {
        match op {
            EditOp::Equal { .. } => counts.hits += 1,
            EditOp::Substitute { .. } => counts.substitutions += 1,
            EditOp::Insert { .. } => counts.insertions += 1,
            EditOp::Delete { .. } => counts.deletions += 1,
        }
    }
    counts
}

/// Levenshtein edit distance between two token sequences, single-row DP
/// (O(min(M,N)) space).
fn edit_distance(reference: &[String], hypothesis: &[String]) -> usize {
    let m = reference.len();
    let n = hypothesis.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev_row: Vec<usize> = (0..=n).collect();
    let mut curr_row = vec![0; n + 1];

    for i in 1..=m {
        curr_row[0] = i;
        for j in 1..=n {
            let cost = usize::from(reference[i - 1] != hypothesis[j - 1]);
            curr_row[j] = (prev_row[j] + 1)
                .min(curr_row[j - 1] + 1)
                .min(prev_row[j - 1] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[n]
}

/// Levenshtein alignment between two token sequences.
///
/// Two-phase for performance: the total edit distance is computed with the
/// single-row DP, then traceback runs a banded Wagner–Fischer restricted to a
/// diagonal band of width `2 * dist + 1` — O(M×D) instead of O(M×N) when the
/// distance is small relative to the sequence lengths (the common case for a
/// competent read-aloud take).
fn align(reference: &[String], hypothesis: &[String]) -> Vec<EditOp> {
    let m = reference.len();
    let n = hypothesis.len();

    if m == 0 {
        return (0..n).map(|i| EditOp::Insert { hyp_index: i }).collect();
    }
    if n == 0 {
        return (0..m).map(|i| EditOp::Delete { ref_index: i }).collect();
    }

    let dist = edit_distance(reference, hypothesis);
    if dist == 0 {
        return (0..m)
            .map(|i| EditOp::Equal { ref_index: i, hyp_index: i })
            .collect();
    }

    align_banded(reference, hypothesis, dist)
}

/// Banded Wagner–Fischer: build the DP rows within the band, then backtrack.
fn align_banded(reference: &[String], hypothesis: &[String], dist: usize) -> Vec<EditOp> {
    let ref_len = reference.len();
    let hyp_len = hypothesis.len();
    let band = dist;
    let lo = |row: usize| row.saturating_sub(band);
    let hi = |row: usize| hyp_len.min(row + band);

    // Row 0
    let mut rows: Vec<Vec<usize>> = Vec::with_capacity(ref_len + 1);
    {
        let (lo_val, hi_val) = (lo(0), hi(0));
        let row: Vec<usize> = (lo_val..=hi_val).collect();
        rows.push(row);
    }

    // Rows 1..=ref_len
    for ref_idx in 1..=ref_len {
        let (lo_val, hi_val) = (lo(ref_idx), hi(ref_idx));
        let prev_lo = lo(ref_idx - 1);
        let prev_hi = hi(ref_idx - 1);
        let mut row = vec![0; hi_val - lo_val + 1];

        for hyp_idx in lo_val..=hi_val {
            let local_j = hyp_idx - lo_val;
            if hyp_idx == 0 {
                row[local_j] = ref_idx;
                continue;
            }

            // The diagonal neighbor is always inside the band when band = dist.
            let diag = rows[ref_idx - 1][hyp_idx - 1 - prev_lo];
            let up = if hyp_idx >= prev_lo && hyp_idx <= prev_hi {
                Some(rows[ref_idx - 1][hyp_idx - prev_lo] + 1)
            } else {
                None
            };
            let left = if hyp_idx > lo_val { Some(row[local_j - 1] + 1) } else { None };
            let cost = usize::from(reference[ref_idx - 1] != hypothesis[hyp_idx - 1]);

            row[local_j] =
                up.into_iter().chain(left).chain(Some(diag + cost)).min().unwrap_or(0);
        }
        rows.push(row);
    }

    // Backtrack from (ref_len, hyp_len).
    let mut ops = Vec::with_capacity(ref_len + hyp_len);
    let (mut ref_pos, mut hyp_pos) = (ref_len, hyp_len);
    while ref_pos > 0 || hyp_pos > 0 {
        let lo_val = lo(ref_pos);
        let cv = rows[ref_pos][hyp_pos - lo_val];

        if ref_pos > 0 && hyp_pos > 0 && reference[ref_pos - 1] == hypothesis[hyp_pos - 1] {
            ops.push(EditOp::Equal { ref_index: ref_pos - 1, hyp_index: hyp_pos - 1 });
            ref_pos -= 1;
            hyp_pos -= 1;
        } else {
            let prev_lo = lo(ref_pos.saturating_sub(1));
            let prev_hi = if ref_pos > 0 { hi(ref_pos - 1) } else { 0 };

            let diag_ok = ref_pos > 0
                && hyp_pos > 0
                && hyp_pos > prev_lo
                && hyp_pos - 1 <= prev_hi
                && cv == rows[ref_pos - 1][hyp_pos - 1 - prev_lo] + 1;
            let left_ok = hyp_pos > lo_val && cv == rows[ref_pos][hyp_pos - 1 - lo_val] + 1;

            if diag_ok {
                ops.push(EditOp::Substitute { ref_index: ref_pos - 1, hyp_index: hyp_pos - 1 });
                ref_pos -= 1;
                hyp_pos -= 1;
            } else if left_ok {
                ops.push(EditOp::Insert { hyp_index: hyp_pos - 1 });
                hyp_pos -= 1;
            } else {
                ops.push(EditOp::Delete { ref_index: ref_pos - 1 });
                ref_pos -= 1;
            }
        }
    }
    ops.reverse();
    ops
}

// ============================================================
// Normalization
// ============================================================

/// The single preprocessing seam shared by scoring and chunk building:
/// NFC-normalize, lowercase (`ẞ → ß`), fold `ß → ss`, strip Unicode
/// punctuation (keeps umlauts; removes `„“«»…`), split into word tokens.
pub(crate) fn normalize_tokens(text: &str) -> Vec<String> {
    let lowered: String = text.nfc().collect::<String>().to_lowercase();
    let folded = lowered.replace('ß', "ss");
    let cleaned: String = folded
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    cleaned.split_whitespace().map(|w| w.to_string()).collect()
}

// ============================================================
// Metrics + summary
// ============================================================

/// The kind of an alignment chunk, serialized lowercase for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AsrOp {
    Equal,
    Substitute,
    Insert,
    Delete,
}

/// A readable run of consecutive same-op alignment cells, with the words on
/// each side joined by spaces. Indices are the run starts (`null` where the
/// side has no words — insertions have no reference, deletions no hypothesis).
#[derive(Debug, Clone, Serialize)]
pub struct AsrChunk {
    pub op: AsrOp,
    pub ref_word: String,
    pub hyp_word: String,
    pub ref_index: Option<usize>,
    pub hyp_index: Option<usize>,
}

/// The whole read-aloud scoring picture: WIP-based `score` (0-100), the raw
/// WER/MER rates, H/S/D/I counts, and the alignment chunks to highlight.
#[derive(Debug, Clone, Serialize)]
pub struct AsrScoreSummary {
    /// `wip × 100` — the number shown to the user and stored per attempt.
    pub score: f64,
    /// Word Error Rate `(S + D + I) / N`.
    pub wer: f64,
    /// Match Error Rate `(S + D + I) / (H + S + D + I)`.
    pub mer: f64,
    pub hits: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    pub ref_len: usize,
    pub hyp_len: usize,
    pub chunks: Vec<AsrChunk>,
}

impl AsrScoreSummary {
    fn new(
        ref_len: usize,
        hyp_len: usize,
        counts: &OperationCounts,
        chunks: Vec<AsrChunk>,
    ) -> Self {
        let errors = counts.substitutions + counts.deletions + counts.insertions;
        let denom = counts.hits + errors;
        let wer = if ref_len == 0 {
            if hyp_len == 0 {
                0.0
            } else {
                1.0
            }
        } else {
            errors as f64 / ref_len as f64
        };
        let mer = if denom == 0 { 0.0 } else { errors as f64 / denom as f64 };
        // WIP = (H/N) · (H/(H+S+D+I)) — multiplicative, so omissions and
        // substitutions both compound; an empty side preserves nothing (0).
        let wip = if ref_len == 0 || denom == 0 {
            0.0
        } else {
            (counts.hits as f64 / ref_len as f64) * (counts.hits as f64 / denom as f64)
        };
        Self {
            score: wip * 100.0,
            wer,
            mer,
            hits: counts.hits,
            substitutions: counts.substitutions,
            deletions: counts.deletions,
            insertions: counts.insertions,
            ref_len,
            hyp_len,
            chunks,
        }
    }
}

fn align_raw(reference: &str, hypothesis: &str) -> (Vec<String>, Vec<String>, Vec<EditOp>) {
    let ref_words = normalize_tokens(reference);
    let hyp_words = normalize_tokens(hypothesis);
    let ops = align(&ref_words, &hyp_words);
    (ref_words, hyp_words, ops)
}

/// Full scoring picture including the alignment chunks (what the commands and
/// the MCP tool return).
pub(crate) fn score_asr(reference: &str, hypothesis: &str) -> AsrScoreSummary {
    let (ref_words, hyp_words, ops) = align_raw(reference, hypothesis);
    let counts = count_operations(&ops);
    let chunks = build_chunks(&ops, &ref_words, &hyp_words);
    AsrScoreSummary::new(ref_words.len(), hyp_words.len(), &counts, chunks)
}

/// Score only (no chunk building) — the cheap path for re-scoring stored
/// attempts on read.
pub(crate) fn score_value(reference: &str, hypothesis: &str) -> f64 {
    let (ref_words, hyp_words, ops) = align_raw(reference, hypothesis);
    let counts = count_operations(&ops);
    AsrScoreSummary::new(ref_words.len(), hyp_words.len(), &counts, Vec::new()).score
}

/// Merge consecutive same-op cells into readable chunks.
fn build_chunks(
    ops: &[EditOp],
    ref_words: &[String],
    hyp_words: &[String],
) -> Vec<AsrChunk> {
    let mut chunks: Vec<AsrChunk> = Vec::new();
    for op in ops {
        let (kind, ri, hi) = match op {
            EditOp::Equal { ref_index, hyp_index } => (AsrOp::Equal, Some(*ref_index), Some(*hyp_index)),
            EditOp::Substitute { ref_index, hyp_index } => {
                (AsrOp::Substitute, Some(*ref_index), Some(*hyp_index))
            }
            EditOp::Insert { hyp_index } => (AsrOp::Insert, None, Some(*hyp_index)),
            EditOp::Delete { ref_index } => (AsrOp::Delete, Some(*ref_index), None),
        };
        let rw = ri.map(|i| ref_words[i].as_str()).unwrap_or("");
        let hw = hi.map(|i| hyp_words[i].as_str()).unwrap_or("");

        match chunks.last_mut() {
            Some(last) if last.op == kind => {
                if !rw.is_empty() {
                    last.ref_word.push(' ');
                    last.ref_word.push_str(rw);
                }
                if !hw.is_empty() {
                    last.hyp_word.push(' ');
                    last.hyp_word.push_str(hw);
                }
            }
            _ => chunks.push(AsrChunk {
                op: kind,
                ref_word: rw.to_string(),
                hyp_word: hw.to_string(),
                ref_index: ri,
                hyp_index: hi,
            }),
        }
    }
    chunks
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(|w| w.to_string()).collect()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    // --- alignment (ported from rwer) ---

    #[test]
    fn align_identical_sequences() {
        let r = words("hello world");
        let ops = align(&r, &r);
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().all(|op| matches!(op, EditOp::Equal { .. })));
        assert_eq!(ops[1], EditOp::Equal { ref_index: 1, hyp_index: 1 });
    }

    #[test]
    fn align_empty_inputs() {
        assert!(align(&[], &[]).is_empty());
        let ops = align(&[], &words("hello world"));
        assert!(ops.iter().all(|op| matches!(op, EditOp::Insert { .. })));
        let ops = align(&words("hello world"), &[]);
        assert!(ops.iter().all(|op| matches!(op, EditOp::Delete { .. })));
    }

    #[test]
    fn align_substitution_deletion_insertion() {
        let ops = align(&words("hello world"), &words("hello earth"));
        assert_eq!(ops[1], EditOp::Substitute { ref_index: 1, hyp_index: 1 });

        let ops = align(&words("hello world foo"), &words("hello foo"));
        assert!(matches!(ops[1], EditOp::Delete { .. }));

        let ops = align(&words("hello foo"), &words("hello world foo"));
        assert!(matches!(ops[1], EditOp::Insert { .. }));
    }

    #[test]
    fn align_mixed_operations() {
        let r = words("a b c d");
        let h = words("a x c d e");
        let ops = align(&r, &h);
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[0], EditOp::Equal { .. }));
        assert!(matches!(ops[1], EditOp::Substitute { .. }));
        assert!(matches!(ops[2], EditOp::Equal { .. }));
        assert!(matches!(ops[3], EditOp::Equal { .. }));
        assert!(matches!(ops[4], EditOp::Insert { .. }));
    }

    #[test]
    fn alignment_counts() {
        let counts = count_operations(&align(&words("a b c"), &words("a x c d")));
        assert_eq!((counts.hits, counts.substitutions, counts.deletions, counts.insertions), (2, 1, 0, 1));
    }

    #[test]
    fn banded_alignment_error_count_matches_edit_distance() {
        // S+D+I of the traceback must equal the independently computed distance.
        for (r, h) in [
            ("a b c d e f g h i j", "a b x d e f g h i j"),
            ("one two three four five", "one tree five four five"),
            ("completely different here", "nothing matches at all"),
        ] {
            let rw = words(r);
            let hw = words(h);
            let dist = edit_distance(&rw, &hw);
            let counts = count_operations(&align(&rw, &hw));
            assert_eq!(
                dist,
                counts.substitutions + counts.deletions + counts.insertions,
                "case: {} vs {}",
                r,
                h
            );
        }
    }

    // --- metrics ---

    #[test]
    fn metrics_match_rwer_process_words_expectations() {
        // rwer: process_words("the cat sat", "the cat sat on") →
        // wer 1/3, mer 0.25, wip 0.75.
        let s = score_asr("the cat sat", "the cat sat on");
        assert!(approx(s.wer, 1.0 / 3.0));
        assert!(approx(s.mer, 0.25));
        assert!(approx(s.score, 75.0));

        // rwer: process_words("a b c", "a c") → wer 1/3 (one deletion).
        let s = score_asr("a b c", "a c");
        assert_eq!((s.hits, s.deletions), (2, 1));
        assert!(approx(s.wer, 1.0 / 3.0));

        // Perfect read.
        let s = score_asr("guten tag", "guten tag");
        assert!(approx(s.score, 100.0));
        assert!(approx(s.wer, 0.0));
    }

    #[test]
    fn empty_reference_scores_zero() {
        assert!(approx(score_value("", "ein wort"), 0.0));
        assert!(approx(score_value("ein wort", ""), 0.0));
        assert!(approx(score_value("", ""), 0.0));
    }

    // --- German-aware normalization ---

    #[test]
    fn nfc_decomposed_umlaut_matches_composed() {
        // "Möchten" composed vs decomposed must align with zero errors.
        let composed = "Ich möchte das Möbelschrank";
        let decomposed = "Ich mo\u{0308}chte das Mo\u{0308}belschrank";
        let s = score_asr(composed, decomposed);
        assert!(approx(s.score, 100.0), "summary: {:?}", s);
    }

    #[test]
    fn sharp_s_folds_to_ss() {
        assert!(approx(score_value("die Straße", "die Strasse"), 100.0));
        assert!(approx(score_value("Großartig", "grossartig"), 100.0));
    }

    #[test]
    fn german_quotes_and_ellipsis_are_stripped() {
        let s = score_asr("„Hallo, Welt!“ …", "hallo welt");
        assert!(approx(s.score, 100.0), "summary: {:?}", s);
    }

    #[test]
    fn compound_split_is_visible_in_chunks() {
        // "Eisen bahn" vs "Eisenbahn" is a real error for WER, but the chunks
        // must expose it instead of hiding it behind a blob.
        let s = score_asr("der Eisenbahn", "der Eisen bahn");
        assert!(s.score < 100.0);
        assert!(s.chunks.iter().any(|c| c.op != AsrOp::Equal));
        assert!(s.insertions + s.substitutions >= 1);
    }

    #[test]
    fn score_value_matches_score_asr() {
        let r = "Der Hund bellt laut im Garten";
        let h = "Der Hund bellt laut im park";
        let full = score_asr(r, h);
        assert!(approx(full.score, score_value(r, h)));
    }

    // --- chunks ---

    #[test]
    fn chunks_merge_runs_and_carry_words() {
        let s = score_asr("a b c d", "a x y d");
        let kinds: Vec<_> = s.chunks.iter().map(|c| (c.op, c.ref_word.as_str(), c.hyp_word.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (AsrOp::Equal, "a", "a"),
                (AsrOp::Substitute, "b c", "x y"),
                (AsrOp::Equal, "d", "d"),
            ]
        );
    }
}
