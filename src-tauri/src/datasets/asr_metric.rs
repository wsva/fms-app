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

use serde::{Deserialize, Serialize};
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

/// Punctuation recorded *next to* a normalized word instead of being compared.
///
/// Scoring never looks at it — punctuation is stripped from both sides before
/// alignment — but fluency does: a pause after a comma or a full stop is a
/// different event from a pause mid-phrase, and that distinction is only
/// available from the reference text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct WordPunct {
    /// The trailing run of non-alphanumeric characters (`","`, `"!?"`, `"…"`).
    pub trailing: String,
    /// That run contains a sentence terminator (`. ! ? …`).
    pub sentence_end: bool,
}

/// One reference word as the scorer sees it: the compared form, the surface
/// form, and the punctuation that trails it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormWord {
    /// The compared form (lowercased, `ß → ss`, punctuation removed).
    pub norm: String,
    /// The token as it appeared in the source, case-folded but otherwise raw.
    pub original: String,
    pub punct: WordPunct,
}

/// The single preprocessing seam shared by scoring, chunk building and
/// fluency: NFC-normalize, lowercase (`ẞ → ß`), fold `ß → ss`, drop Unicode
/// punctuation (keeps umlauts; removes `„“«»…`) and split on whitespace.
///
/// Word boundaries come from whitespace only, exactly as before, so
/// `Eisen-Bahn` still folds to one token `eisenbahn`: punctuation is deleted,
/// never a split point.
pub(crate) fn normalize_words(text: &str) -> Vec<NormWord> {
    let lowered: String = text.nfc().collect::<String>().to_lowercase();
    let folded = lowered.replace('ß', "ss");
    let mut words: Vec<NormWord> = Vec::new();

    for raw in folded.split_whitespace() {
        let norm: String = raw.chars().filter(|c| c.is_alphanumeric()).collect();
        let trailing: String = raw
            .chars()
            .rev()
            .take_while(|c| !c.is_alphanumeric())
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let sentence_end = trailing.chars().any(|c| matches!(c, '.' | '!' | '?' | '…'));

        if norm.is_empty() {
            // A token that is nothing but punctuation (`—`, `…`) carries no
            // word of its own; its mark belongs to the word it follows, which
            // is where the pause it signals actually fell.
            if let Some(prev) = words.last_mut() {
                prev.punct.trailing.push_str(&trailing);
                prev.punct.sentence_end |= sentence_end;
            }
            continue;
        }

        words.push(NormWord {
            norm,
            original: raw.to_string(),
            punct: WordPunct { trailing, sentence_end },
        });
    }

    words
}

/// The compared tokens of a text — the form `align` and `build_chunks` use.
pub(crate) fn normalize_tokens(text: &str) -> Vec<String> {
    normalize_words(text).into_iter().map(|w| w.norm).collect()
}

/// The `(reference_index, hypothesis_index)` pairs the two token lists share,
/// in order.
///
/// Used by the standard-audio span guess: the text was read *somewhere* in a
/// longer recording, and the first and last of these pairs say where. The
/// hypothesis side is passed in rather than derived from a text, because the
/// caller needs a token-to-spoken-word mapping and only it has one.
pub(crate) fn aligned_pairs(reference: &[String], hypothesis: &[String]) -> Vec<(usize, usize)> {
    align(reference, hypothesis)
        .into_iter()
        .filter_map(|op| match op {
            EditOp::Equal { ref_index, hyp_index } => Some((ref_index, hyp_index)),
            _ => None,
        })
        .collect()
}

/// Direction codes of the traceback matrix [`aligned_pairs_glocal`] builds.
const STEP_DIAGONAL: u8 = 0;
const STEP_REFERENCE: u8 = 1;
const STEP_HYPOTHESIS: u8 = 2;

/// The `(reference_index, hypothesis_index)` pairs of a **semi-global**
/// alignment: the whole reference is consumed, the hypothesis may begin and end
/// anywhere.
///
/// This is the right comparison for "where in this recording is this text" and
/// [`aligned_pairs`] is not. The reference is one passage and the hypothesis is
/// a window of the recording, usually several times longer, holding that
/// passage plus whatever was read before and after it. A global alignment has to
/// account for every hypothesis token too, so it pays for the surplus with
/// deletions — and the deletions are cheapest *inside* the passage, which is
/// exactly where the matched words are. A text that was heard perfectly scores
/// well under 100 %, and the shorter the text relative to the window the worse
/// the score: a fifteen-word mail against a four-minute window came back with 7
/// of 15 matches instead of 11.
///
/// Free ends cost one thing a global alignment gives for free: the caller must
/// bound the window itself, because this will happily align ten words against a
/// window that starts three minutes earlier. The window's centre is the caller's
/// business (see the position prior in `read_aloud::source_align`).
///
/// The DP is O(m×n) in time and O(m×n) bytes for the traceback, which is why the
/// caller keeps the window tight; ties prefer the diagonal, then the reference
/// side, so a run of matched words stays one piece instead of being cut into
/// alternating substitutions.
pub(crate) fn aligned_pairs_glocal(reference: &[String], hypothesis: &[String]) -> Vec<(usize, usize)> {
    let ref_len = reference.len();
    let hyp_len = hypothesis.len();
    if ref_len == 0 || hyp_len == 0 {
        return Vec::new();
    }

    let stride = hyp_len + 1;
    let mut steps: Vec<u8> = vec![STEP_DIAGONAL; ref_len * stride];
    // Row 0 is all zeros: beginning anywhere in the hypothesis is free.
    let mut prev_row: Vec<usize> = vec![0; stride];
    let mut curr_row: Vec<usize> = vec![0; stride];

    for ref_idx in 1..=ref_len {
        curr_row[0] = ref_idx;
        let row = &mut steps[(ref_idx - 1) * stride..ref_idx * stride];
        row[0] = STEP_REFERENCE;
        for hyp_idx in 1..=hyp_len {
            let diagonal = prev_row[hyp_idx - 1]
                + usize::from(reference[ref_idx - 1] != hypothesis[hyp_idx - 1]);
            let up = prev_row[hyp_idx] + 1;
            let left = curr_row[hyp_idx - 1] + 1;
            let (cost, step) = if diagonal <= up && diagonal <= left {
                (diagonal, STEP_DIAGONAL)
            } else if up <= left {
                (up, STEP_REFERENCE)
            } else {
                (left, STEP_HYPOTHESIS)
            };
            curr_row[hyp_idx] = cost;
            row[hyp_idx] = step;
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    // Ending anywhere is free too: leave the matrix at the cheapest cell of the
    // last row, the earliest one on a tie so the span stays as short as the
    // evidence allows.
    let mut end = 0usize;
    for hyp_idx in 1..=hyp_len {
        if prev_row[hyp_idx] < prev_row[end] {
            end = hyp_idx;
        }
    }

    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let (mut ref_pos, mut hyp_pos) = (ref_len, end);
    while ref_pos > 0 && hyp_pos > 0 {
        match steps[(ref_pos - 1) * stride + hyp_pos] {
            STEP_DIAGONAL => {
                if reference[ref_pos - 1] == hypothesis[hyp_pos - 1] {
                    pairs.push((ref_pos - 1, hyp_pos - 1));
                }
                ref_pos -= 1;
                hyp_pos -= 1;
            }
            STEP_REFERENCE => ref_pos -= 1,
            _ => hyp_pos -= 1,
        }
    }
    pairs.reverse();
    pairs
}

// ============================================================
// Numbers
// ============================================================

/// The language a spelled-out number is rendered in.
///
/// `De` is deliberately kept without a production caller: scoring runs the
/// [`NumberPolicy::Digits`] default, and the sanctioned fix for a digit that the
/// reader said as a word is the text's LLM-normalized reference, not a second
/// speller. It stays as the hook for a caller that *does* know the reference
/// language — nothing on a `read_text` row records one — and the spelling
/// itself is covered by the tests at the bottom of this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumLang {
    En,
    #[allow(dead_code)]
    De,
}

/// What to do with digit tokens in the *reference* text.
///
/// `Digits` (the default) leaves `2000` alone, which is the historical
/// behaviour and the one read-aloud actually uses: the reference language is
/// not recorded on a `read_text` row, and the sanctioned fix for surface
/// mismatches is the LLM-normalized content field. `Words` exists for callers
/// that do know the language and would rather score spoken forms directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NumberPolicy {
    #[default]
    Digits,
    /// No production caller, same reason as [`NumLang::De`]: kept as the policy
    /// hook so a language-aware caller is a one-line change, not a rewrite.
    #[allow(dead_code)]
    Words(NumLang),
}

/// Everything the classification step needs to be tuned, kept in one struct so
/// no threshold hides in a function body.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScoringConfig {
    pub number_policy: NumberPolicy,
    pub number_lang: NumLang,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self { number_policy: NumberPolicy::Digits, number_lang: NumLang::En }
    }
}

const UNITS_EN: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen",
    "eighteen", "nineteen",
];
const TENS_EN: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

const UNITS_DE: [&str; 20] = [
    "null", "eins", "zwei", "drei", "vier", "fünf", "sechs", "sieben", "acht", "neun",
    "zehn", "elf", "zwölf", "dreizehn", "vierzehn", "fünfzehn", "sechzehn", "siebzehn",
    "achtzehn", "neunzehn",
];
const TENS_DE: [&str; 8] = [
    "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig", "achtzig", "neunzig",
];

fn below_hundred(n: u64, lang: NumLang) -> String {
    match lang {
        NumLang::En => {
            if n < 20 {
                UNITS_EN[n as usize].to_string()
            } else {
                let tens = TENS_EN[(n / 10 - 2) as usize];
                let unit = n % 10;
                if unit == 0 {
                    tens.to_string()
                } else {
                    format!("{} {}", tens, UNITS_EN[unit as usize])
                }
            }
        }
        NumLang::De => {
            if n < 20 {
                UNITS_DE[n as usize].to_string()
            } else {
                let tens = TENS_DE[(n / 10 - 2) as usize];
                let unit = n % 10;
                if unit == 0 {
                    tens.to_string()
                } else {
                    // German inverts and fuses: 42 → zweiundvierzig.
                    format!("{}und{}", UNITS_DE[unit as usize], tens)
                }
            }
        }
    }
}

fn below_thousand(n: u64, lang: NumLang) -> String {
    let hundreds = n / 100;
    let rest = n % 100;
    match lang {
        NumLang::En => {
            let mut out = String::new();
            if hundreds > 0 {
                out.push_str(UNITS_EN[hundreds as usize]);
                out.push_str(" hundred");
            }
            if rest > 0 {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&below_hundred(rest, lang));
            }
            out
        }
        NumLang::De => {
            if hundreds == 0 {
                return below_hundred(rest, lang);
            }
            let mut out = if hundreds == 1 {
                "ein".to_string()
            } else {
                UNITS_DE[hundreds as usize].to_string()
            };
            out.push_str("hundert");
            if rest > 0 {
                out.push_str(&below_hundred(rest, lang));
            }
            out
        }
    }
}

/// Spell an integer (`0..=999_999`) out in words. Larger values fall back to
/// their digits, because a number nobody reads aloud is not worth spelling.
pub(crate) fn spell_number(value: u64, lang: NumLang) -> String {
    if value > 999_999 {
        return value.to_string();
    }
    let thousands = value / 1000;
    let rest = value % 1000;
    match lang {
        NumLang::En => {
            let mut out = String::new();
            if thousands > 0 {
                out.push_str(&below_thousand(thousands, lang));
                out.push_str(" thousand");
            }
            if rest > 0 {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&below_thousand(rest, lang));
            }
            if out.is_empty() {
                out.push_str(UNITS_EN[0]);
            }
            out
        }
        NumLang::De => {
            let mut out = String::new();
            if thousands > 0 {
                if thousands == 1 {
                    out.push_str("ein");
                } else {
                    out.push_str(&below_thousand(thousands, lang));
                }
                out.push_str("tausend");
            }
            if rest > 0 {
                out.push_str(&below_thousand(rest, lang));
            }
            if out.is_empty() {
                out.push_str(UNITS_DE[0]);
            }
            out
        }
    }
}

/// Rewrite digit runs in `text` as spoken words when the policy asks for it.
/// The rewrite happens *before* tokenization so a spelled multi-word number
/// (`two thousand`) lines up with what STT emits as two tokens.
pub(crate) fn apply_number_policy(text: &str, policy: NumberPolicy) -> String {
    let lang = match policy {
        NumberPolicy::Digits => return text.to_string(),
        NumberPolicy::Words(lang) => lang,
    };

    let mut out = String::with_capacity(text.len());
    let mut digits = String::new();
    let flush = |digits: &mut String, out: &mut String| {
        if let Ok(value) = digits.parse::<u64>() {
            out.push_str(&spell_number(value, lang));
        } else {
            out.push_str(digits);
        }
        digits.clear();
    };

    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            if !digits.is_empty() {
                flush(&mut digits, &mut out);
            }
            out.push(ch);
        }
    }
    if !digits.is_empty() {
        flush(&mut digits, &mut out);
    }
    out
}

/// Is `a` the same number as `b` written differently?
///
/// Two digit forms compare by value (`2000` vs `2.000`, once punctuation is
/// gone); a digit form compares against a single-token spelled form (`20` vs
/// `zwanzig`). Multi-word English spellings (`twenty one`) cannot match a
/// single token and are left to the word-level alignment, where they show up
/// as an insertion plus a substitution — see the rounding note in the design
/// doc.
fn number_variant(a: &str, b: &str, lang: NumLang) -> bool {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => return x == y,
        (Ok(x), Err(_)) => {
            let joined: String = spell_number(x, lang).split_whitespace().collect();
            return joined == b;
        }
        (Err(_), Ok(y)) => {
            let joined: String = spell_number(y, lang).split_whitespace().collect();
            return joined == a;
        }
        (Err(_), Err(_)) => {}
    }
    false
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

/// Per-word outcome of the alignment. `AsrOp` says what the *aligner* did;
/// a verdict says what the *reader* did, which is the same thing with two
/// refinements: a near-miss substitution is `Minor`, and an insertion that
/// repeats the words just spoken is `Repeated` (a restart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    Correct,
    Minor,
    Substituted,
    Omitted,
    Inserted,
    Repeated,
}

/// One cell of the alignment, keyed by the indices it touches (`ref_index` is
/// `None` for insertions, `hyp_index` `None` for omissions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordVerdict {
    pub ref_index: Option<usize>,
    pub hyp_index: Option<usize>,
    pub verdict: Verdict,
}

/// The whole read-aloud scoring picture: WIP-based `score` (0-100), the raw
/// WER/MER rates, H/S/D/I counts, the alignment chunks to highlight and the
/// per-word verdicts the assessment and the UI read.
#[derive(Debug, Clone, Serialize)]
pub struct AsrScoreSummary {
    /// `wip × 100` — the number shown to the user and stored per attempt.
    /// Counts exact matches only, so it is unchanged by the verdict classes.
    pub score: f64,
    /// The same shape with `Minor` verdicts counted as hits.
    pub lenient_score: f64,
    /// Word Error Rate `(S + D + I) / N`.
    pub wer: f64,
    /// Match Error Rate `(S + D + I) / (H + S + D + I)`.
    pub mer: f64,
    pub hits: usize,
    /// Substitutions that are homophones or one character off. Counted as an
    /// error by `score`, as a hit by `lenient_score`.
    pub minor: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    /// Insertions that repeat the preceding 1-3 words (restarts). A subset of
    /// `insertions`, reported separately because fluency cares and accuracy
    /// already charges them.
    pub repeated: usize,
    pub ref_len: usize,
    pub hyp_len: usize,
    pub chunks: Vec<AsrChunk>,
    pub words: Vec<WordVerdict>,
}

/// `(H/N) · (H/(H+S+D+I))` — multiplicative, so omissions and substitutions
/// both compound; an empty side preserves nothing (0).
fn wip(hits: usize, ref_len: usize, errors: usize) -> f64 {
    let denom = hits + errors;
    if ref_len == 0 || denom == 0 {
        0.0
    } else {
        (hits as f64 / ref_len as f64) * (hits as f64 / denom as f64)
    }
}

impl AsrScoreSummary {
    fn new(
        ref_len: usize,
        hyp_len: usize,
        counts: &OperationCounts,
        minor: usize,
        repeated: usize,
        chunks: Vec<AsrChunk>,
        words: Vec<WordVerdict>,
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
        let lenient_hits = counts.hits + minor;
        let lenient_errors = errors.saturating_sub(minor);
        Self {
            score: wip(counts.hits, ref_len, errors) * 100.0,
            lenient_score: wip(lenient_hits, ref_len, lenient_errors) * 100.0,
            wer,
            mer,
            hits: counts.hits,
            minor,
            substitutions: counts.substitutions,
            deletions: counts.deletions,
            insertions: counts.insertions,
            repeated,
            ref_len,
            hyp_len,
            chunks,
            words,
        }
    }
}

fn align_raw(reference: &str, hypothesis: &str) -> (Vec<String>, Vec<String>, Vec<EditOp>) {
    let ref_words = normalize_tokens(reference);
    let hyp_words = normalize_tokens(hypothesis);
    let ops = align(&ref_words, &hyp_words);
    (ref_words, hyp_words, ops)
}

/// Full scoring picture including the alignment chunks and per-word verdicts
/// (what the commands and the MCP tool return).
pub(crate) fn score_asr(reference: &str, hypothesis: &str) -> AsrScoreSummary {
    score_asr_with(reference, hypothesis, &ScoringConfig::default())
}

/// [`score_asr`] with explicit number handling. `score` is identical either
/// way for a given normalized reference — the policy changes what the reference
/// normalizes to, never how the number is computed.
pub(crate) fn score_asr_with(
    reference: &str,
    hypothesis: &str,
    cfg: &ScoringConfig,
) -> AsrScoreSummary {
    let reference = apply_number_policy(reference, cfg.number_policy);
    let ref_words = normalize_words(&reference);
    let hyp_words = normalize_tokens(hypothesis);
    let ref_tokens: Vec<String> = ref_words.iter().map(|w| w.norm.clone()).collect();
    let ops = align(&ref_tokens, &hyp_words);
    let counts = count_operations(&ops);
    let verdicts = classify(&ops, &ref_tokens, &hyp_words, cfg);
    let minor = verdicts.iter().filter(|v| v.verdict == Verdict::Minor).count();
    let repeated = verdicts.iter().filter(|v| v.verdict == Verdict::Repeated).count();
    let chunks = build_chunks(&ops, &ref_tokens, &hyp_words);
    AsrScoreSummary::new(ref_tokens.len(), hyp_words.len(), &counts, minor, repeated, chunks, verdicts)
}

/// Score only (no chunk or verdict building) — the cheap path for re-scoring
/// stored attempts on read.
pub(crate) fn score_value(reference: &str, hypothesis: &str) -> f64 {
    let (ref_words, _hyp_words, ops) = align_raw(reference, hypothesis);
    let counts = count_operations(&ops);
    let errors = counts.substitutions + counts.deletions + counts.insertions;
    wip(counts.hits, ref_words.len(), errors) * 100.0
}

/// Classify every alignment cell into a verdict. Counts stay op-based —
/// `minor` is a refinement of a substitution and `repeated` of an insertion,
/// so the WIP inputs never move.
fn classify(
    ops: &[EditOp],
    ref_words: &[String],
    hyp_words: &[String],
    cfg: &ScoringConfig,
) -> Vec<WordVerdict> {
    ops.iter()
        .map(|op| match op {
            EditOp::Equal { ref_index, hyp_index } => WordVerdict {
                ref_index: Some(*ref_index),
                hyp_index: Some(*hyp_index),
                verdict: Verdict::Correct,
            },
            EditOp::Substitute { ref_index, hyp_index } => {
                let verdict = if is_minor(&ref_words[*ref_index], &hyp_words[*hyp_index], cfg) {
                    Verdict::Minor
                } else {
                    Verdict::Substituted
                };
                WordVerdict {
                    ref_index: Some(*ref_index),
                    hyp_index: Some(*hyp_index),
                    verdict,
                }
            }
            EditOp::Delete { ref_index } => WordVerdict {
                ref_index: Some(*ref_index),
                hyp_index: None,
                verdict: Verdict::Omitted,
            },
            EditOp::Insert { hyp_index } => WordVerdict {
                ref_index: None,
                hyp_index: Some(*hyp_index),
                verdict: if repeats_preceding(hyp_words, *hyp_index) {
                    Verdict::Repeated
                } else {
                    Verdict::Inserted
                },
            },
        })
        .collect()
}

/// Homophone pairs, one list for both languages the app ships STT models for.
/// Deliberately small: a pair earns its place only when the two words sound
/// the same in ordinary speech and STT plausibly picks the other one.
const HOMOPHONES: &[(&str, &str)] = &[
    // German
    ("das", "dass"),
    ("dann", "denn"),
    ("wenn", "wen"),
    ("seit", "seid"),
    ("war", "wahr"),
    ("mehr", "meer"),
    ("leere", "lehre"),
    ("fiel", "viel"),
    ("stadt", "statt"),
    ("wieder", "wider"),
    ("mann", "man"),
    // English
    ("there", "their"),
    ("your", "youre"),
    ("to", "too"),
    ("then", "than"),
    ("weather", "whether"),
    ("write", "right"),
    ("break", "brake"),
    ("hear", "here"),
    ("threw", "through"),
    ("one", "won"),
    ("knew", "new"),
    ("allowed", "aloud"),
    ("passed", "past"),
];

fn is_homophone(a: &str, b: &str) -> bool {
    HOMOPHONES
        .iter()
        .any(|(x, y)| (a == *x && b == *y) || (a == *y && b == *x))
}

/// Character-level Levenshtein distance, capped at `cap` (the walk stops as
/// soon as every cell in a row exceeds it).
fn char_distance(a: &str, b: &str, cap: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        if row_min > cap {
            return cap + 1;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// A substitution the reader plausibly did not make: a homophone, a one-character
/// slip in a long word, or the same number in another form.
fn is_minor(ref_word: &str, hyp_word: &str, cfg: &ScoringConfig) -> bool {
    if is_homophone(ref_word, hyp_word) {
        return true;
    }
    if number_variant(ref_word, hyp_word, cfg.number_lang) {
        return true;
    }
    let long_enough = ref_word.chars().count() >= 5 && hyp_word.chars().count() >= 5;
    long_enough && char_distance(ref_word, hyp_word, 1) <= 1
}

/// Does the hypothesis word at `index` belong to a duplicated run?
///
/// Catches both the stutter (`the the`) and the restart (`the cat the cat`).
/// The duplicate is searched for on **both** sides: the banded aligner may
/// place the extra words before the run they repeat or after it (`the cat |
/// the cat sat` backtracks to the leftmost placement), so looking only
/// backwards would silently miss every restart.
///
/// Bounded on purpose: a long verbatim repeat is a re-read of the passage, and
/// marking all of it `Repeated` would hide the insertion from the reader.
fn repeats_preceding(words: &[String], index: usize) -> bool {
    (1..=3).any(|k| {
        let backwards = index >= k
            && index + k <= words.len()
            && words[index - k..index] == words[index..index + k];
        let forwards = index + 2 * k <= words.len()
            && words[index..index + k] == words[index + k..index + 2 * k];
        backwards || forwards
    })
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
    fn glocal_ignores_the_words_around_the_passage() {
        // A window is the passage plus a minute of other reading either side.
        // A global alignment would be charged for every one of those tokens —
        // which is why handing it a wider window used to buy nothing — while
        // this one starts and ends where the passage does and keeps every word.
        let reference = words("kein betreff emmi du bist die schoenste frau der welt");
        let mut window = vec!["und".to_string(); 200];
        window.extend(reference.iter().cloned());
        window.extend(vec!["dann".to_string(); 200]);

        let pairs = aligned_pairs_glocal(&reference, &window);
        assert_eq!(pairs.len(), reference.len());
        assert_eq!(pairs[0], (0, 200));
        assert_eq!(pairs[reference.len() - 1], (reference.len() - 1, 200 + reference.len() - 1));
    }

    #[test]
    fn glocal_starts_and_ends_anywhere() {
        let reference = words("das ist der anfang vom ende");
        let window = words("lorem ipsum das ist der anfang vom ende dolor sit");
        let pairs = aligned_pairs_glocal(&reference, &window);
        assert_eq!(pairs.len(), reference.len());
        assert_eq!(pairs[0], (0, 2));
        assert_eq!(pairs[pairs.len() - 1], (reference.len() - 1, 7));
    }

    #[test]
    fn glocal_prefers_the_cheapest_end_on_a_tie() {
        // "ende" occurs twice; the first one leaves the shortest span.
        let reference = words("vom ende");
        let window = words("vom ende und vom ende");
        let pairs = aligned_pairs_glocal(&reference, &window);
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
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

    // --- verdicts (what the *reader* did, not what the aligner did) ---

    fn verdicts(s: &AsrScoreSummary) -> Vec<Verdict> {
        s.words.iter().map(|w| w.verdict).collect()
    }

    #[test]
    fn a_homophone_substitution_is_minor() {
        let s = score_asr("das wetter ist gut", "dass wetter ist gut");
        assert_eq!(s.substitutions, 1, "the aligner still sees a substitution");
        assert_eq!(s.minor, 1);
        assert_eq!(verdicts(&s)[0], Verdict::Minor);
    }

    #[test]
    fn a_one_character_slip_in_a_long_word_is_minor() {
        let s = score_asr("the garden is big", "the gardan is big");
        assert_eq!(s.minor, 1);
        // Below the length floor a one-character difference is a real error:
        let short = score_asr("the cat sat", "the vat sat");
        assert_eq!(short.minor, 0, "3-letter words are not eligible");
    }

    #[test]
    fn a_whole_word_change_is_a_substitution() {
        let s = score_asr("the cat sat", "the dog sat");
        assert_eq!(s.minor, 0);
        assert_eq!(verdicts(&s)[1], Verdict::Substituted);
    }

    #[test]
    fn omissions_and_insertions_keep_their_verdicts() {
        let omitted = score_asr("a b c", "a c");
        assert!(omitted.words.iter().any(|w| w.verdict == Verdict::Omitted && w.hyp_index.is_none()));

        let inserted = score_asr("a c", "a b c");
        assert!(inserted.words.iter().any(|w| w.verdict == Verdict::Inserted && w.ref_index.is_none()));
    }

    #[test]
    fn a_restart_marks_the_repeated_words() {
        let s = score_asr("the cat sat", "the cat the cat sat");
        assert_eq!(s.repeated, 1, "summary: {:?}", verdicts(&s));
        let repeated: Vec<&WordVerdict> =
            s.words.iter().filter(|w| w.verdict == Verdict::Repeated).collect();
        assert_eq!(repeated.len(), 1);
        assert!(repeated[0].ref_index.is_none(), "a repeat is an insertion");
        // Accuracy still charges it in full.
        assert_eq!(s.insertions, 2);
    }

    #[test]
    fn minor_counts_as_an_error_for_score_and_as_a_hit_for_lenient_score() {
        let s = score_asr("das wetter", "dass wetter");
        // (H/N) · (H/(H+S+D+I)) with H=1, N=2, errors=1 → 0.25.
        assert!(approx(s.score, 25.0), "score: {}", s.score);
        assert!(approx(s.lenient_score, 100.0), "lenient: {}", s.lenient_score);
    }

    #[test]
    fn every_existing_score_is_unchanged_by_the_verdict_classes() {
        // The hard invariant, as a table: these are the pre-existing cases.
        for (r, h, expected) in [
            ("the cat sat", "the cat sat on", 75.0),
            ("a b c", "a c", (2.0 / 3.0) * (2.0 / 3.0) * 100.0),
            ("guten tag", "guten tag", 100.0),
            ("Der Hund bellt laut im Garten", "Der Hund bellt laut im park", (5.0 / 6.0) * (5.0 / 6.0) * 100.0),
        ] {
            let s = score_asr(r, h);
            assert!(approx(s.score, expected), "{r} vs {h}: {} != {expected}", s.score);
        }
    }

    #[test]
    fn empty_sides_produce_no_verdicts() {
        assert!(score_asr("", "").words.is_empty());
        assert!(score_asr("", "eins zwei").words.is_empty() || score_asr("", "eins zwei").hits == 0);
        assert!(score_asr("eins zwei", "").words.iter().all(|w| w.verdict == Verdict::Omitted));
    }

    // --- punctuation memory (for fluency, never for scoring) ---

    #[test]
    fn punctuation_is_remembered_next_to_the_word() {
        let words = normalize_words("\u{201e}Hallo,\u{201c} sagte er.");
        assert_eq!(words.len(), 3, "words: {:?}", words.iter().map(|w| &w.norm).collect::<Vec<_>>());
        assert_eq!(words[0].norm, "hallo");
        assert!(words[0].punct.trailing.contains(','), "trailing: {:?}", words[0].punct.trailing);
        assert!(!words[0].punct.sentence_end);
        assert!(words[2].punct.sentence_end, "a full stop ends the sentence");
        // And none of it reaches the compared form.
        assert!(approx(score_value("Hallo, sagte er.", "hallo sagte er"), 100.0));
    }

    #[test]
    fn a_punctuation_only_token_attaches_to_the_word_before_it() {
        let words = normalize_words("Ende \u{2014} gut");
        assert_eq!(words.len(), 2);
        assert!(!words[0].punct.trailing.is_empty(), "the dash belongs to 'Ende'");
        assert!(words[1].punct.trailing.is_empty());
    }

    #[test]
    fn word_boundaries_still_come_from_whitespace_only() {
        // A hyphenated compound stays one token, exactly as before.
        let words = normalize_words("die Eisen-Bahn");
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].norm, "eisenbahn");
    }

    // --- numbers ---

    #[test]
    fn the_default_number_policy_leaves_digits_alone() {
        assert_eq!(apply_number_policy("im Jahr 2000", NumberPolicy::Digits), "im Jahr 2000");
    }

    #[test]
    fn the_words_policy_spells_numbers_out() {
        assert_eq!(spell_number(42, NumLang::En), "forty two");
        assert_eq!(spell_number(42, NumLang::De), "zweiundvierzig");
        assert_eq!(spell_number(2000, NumLang::De), "zweitausend");
        assert_eq!(spell_number(0, NumLang::En), "zero");
        assert_eq!(
            apply_number_policy("im Jahr 2000", NumberPolicy::Words(NumLang::De)),
            "im Jahr zweitausend"
        );
    }

    #[test]
    fn the_numbers_above_the_spelling_limit_keep_their_digits() {
        assert_eq!(spell_number(1_000_000, NumLang::En), "1000000");
    }

    #[test]
    fn a_spoken_number_matches_its_digits() {
        assert!(number_variant("20", "twenty", NumLang::En));
        assert!(number_variant("zwanzig", "20", NumLang::De));
        assert!(!number_variant("twenty", "thirty", NumLang::En));
        // Punctuation never reaches the comparison: normalize strips it first.
        assert!(approx(score_value("im Jahr 2.000 Euro", "im Jahr 2000 Euro"), 100.0));
    }
}
