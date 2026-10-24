//! The reference timeline: what the learner was *supposed* to say, as phones.
//!
//! ## Why the phone target is decoded from a recording, not built from a dictionary
//!
//! GOP needs the expected phone sequence pinned to time. Three ways to get it:
//!
//! 1. A G2P engine (espeak-ng, CMUdict). Rejected: espeak-ng is a C library
//!    needing Android NDK work, and no G2P crate covers German.
//! 2. Teacher-forcing the STT model. Rejected: `models/` exposes no logits,
//!    logprobs or decoder access for any backend (see the capability table in
//!    `models::transcribe_file_word_level`), so it cannot be done at all.
//! 3. Decode a canonical recording of the text with the *same* phoneme model the
//!    learner's recording is decoded with. Adopted.
//!
//! Option 3 is what makes the rest cheap: the reference phone inventory is the
//! model's own inventory, so there is no lexicon, no language table and no
//! hand-written mapping to keep in sync. It also measures the learner rather
//! than the model's guess, because the canon is produced by a different voice
//! reading the text correctly.
//!
//! The canonical recording is the text's **standard audio** — a clip the dataset
//! owns (see `super::standard_audio`), either synthesised once with Edge TTS or
//! uploaded and cropped by a human. Nothing here fetches anything: scoring is a
//! local read over a file that already exists, so an offline device scores
//! exactly like an online one. A TTS mispronunciation becomes a target
//! mispronunciation, which is the one error mode this design cannot detect; it is
//! listed under known limitations in `docs/design/read-aloud-score.md`, and it is
//! the reason a human upload is a supported way to make a clip.

use crate::edge_tts::TtsWordBoundary;

use super::gop::PhoneSpan;

/// Seconds per frame of a posterior matrix, from its frame count and the audio
/// it was computed over. The model's own stride, not an assumed one.
pub(crate) fn frame_seconds(frames: usize, audio_secs: f64) -> f64 {
    if frames == 0 || audio_secs <= 0.0 {
        return 0.0;
    }
    audio_secs / frames as f64
}

/// Attribute the phones of the standard audio to reference words.
///
/// `spans` are the phone spans on the **clip's** posteriors, `word_count` the
/// number of compared reference words. When the boundary count and the word
/// count agree — the normal case for a synthesised clip — each phone joins the
/// word interval its midpoint falls in (nearest interval when it falls in a
/// gap). When they disagree (a punctuation-only event, a number split into two
/// words), or when there are no boundaries at all (an uploaded clip), the phones
/// are distributed by time proportional to the word count, which keeps their
/// order — the only thing forced alignment actually needs. `clip_seconds` is the
/// clip's own duration, which is what that fallback divides by.
pub(crate) fn group_phones(
    spans: &[PhoneSpan],
    frame_secs: f64,
    boundaries: &[TtsWordBoundary],
    word_count: usize,
    clip_seconds: f64,
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); word_count];
    if spans.is_empty() || word_count == 0 {
        return groups;
    }

    let midpoint = |span: &PhoneSpan| (span.start as f64 + span.end as f64) / 2.0 * frame_secs;

    if boundaries.len() == word_count {
        for span in spans {
            let at = midpoint(span);
            let mut best = 0usize;
            let mut best_distance = f64::INFINITY;
            for (index, boundary) in boundaries.iter().enumerate() {
                let start = boundary.start;
                let end = boundary.start + boundary.duration;
                let distance = if at < start {
                    start - at
                } else if at > end {
                    at - end
                } else {
                    0.0
                };
                if distance < best_distance {
                    best_distance = distance;
                    best = index;
                }
            }
            groups[best].push(span.phone);
        }
        return groups;
    }

    log::debug!(
        "read_aloud reference: {} boundaries vs {} words, distributing by time",
        boundaries.len(),
        word_count
    );
    // The span the frames cover. Word timings bound it when they exist; a clip
    // without them is bounded by its own duration. Either way it is a real
    // number, so the fraction below is never divided by `EPSILON` — which used
    // to clamp every phone into the last word.
    let total = boundaries
        .iter()
        .map(|b| b.start + b.duration)
        .fold(0.0_f64, f64::max)
        .max(clip_seconds)
        .max(f64::EPSILON);
    for span in spans {
        let fraction = (midpoint(span) / total).clamp(0.0, 0.999_999);
        let index = ((fraction * word_count as f64) as usize).min(word_count - 1);
        groups[index].push(span.phone);
    }
    groups
}

/// The phone target of the whole attempt, in reading order.
pub(crate) fn flatten(groups: &[Vec<usize>]) -> Vec<usize> {
    groups.iter().flatten().copied().collect()
}

/// Cut an aligned phone timeline back into the words it came from.
///
/// Returns one empty group per word when the alignment does not cover exactly
/// the phones the grouping asked for — a word with no alignment has no
/// pronunciation score, which is honest; inventing one is not.
pub(crate) fn split_by_word(spans: &[PhoneSpan], groups: &[Vec<usize>]) -> Vec<Vec<PhoneSpan>> {
    let total: usize = groups.iter().map(|g| g.len()).sum();
    if spans.len() != total {
        return vec![Vec::new(); groups.len()];
    }
    let mut out: Vec<Vec<PhoneSpan>> = Vec::with_capacity(groups.len());
    let mut offset = 0usize;
    for group in groups {
        out.push(spans[offset..offset + group.len()].to_vec());
        offset += group.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(text: &str, start: f64, duration: f64) -> TtsWordBoundary {
        TtsWordBoundary { text: text.to_string(), start, duration }
    }

    fn span(phone: usize, start: usize, end: usize) -> PhoneSpan {
        PhoneSpan { phone, start, end }
    }

    #[test]
    fn phones_are_grouped_by_the_word_they_fall_into() {
        // Two words: [0, 0.5) and [0.6, 1.1). Frames are 10 ms.
        let boundaries = vec![boundary("ab", 0.0, 0.5), boundary("cd", 0.6, 0.5)];
        let spans = vec![span(11, 0, 5), span(12, 5, 20), span(13, 60, 65), span(14, 65, 70)];

        let groups = group_phones(&spans, 0.01, &boundaries, 2, 1.1);
        assert_eq!(groups, vec![vec![11, 12], vec![13, 14]]);
    }

    #[test]
    fn a_phone_in_a_gap_joins_the_nearest_word() {
        let boundaries = vec![boundary("ab", 0.0, 0.2), boundary("cd", 0.9, 0.1)];
        let spans = vec![span(11, 0, 2), span(12, 40, 42), span(13, 91, 95)];
        // 0.41 s sits closer to the end of word one (0.2) than to word two
        // (0.9), so it stays with word one.
        let groups = group_phones(&spans, 0.01, &boundaries, 2, 1.0);
        assert_eq!(groups, vec![vec![11, 12], vec![13]]);
    }

    #[test]
    fn a_boundary_count_mismatch_falls_back_to_time_order() {
        let boundaries = vec![boundary("a", 0.0, 0.5), boundary("b", 0.5, 0.5), boundary("c", 0.9, 0.1)];
        let spans = vec![span(11, 0, 10), span(12, 50, 60), span(13, 95, 99)];
        let groups = group_phones(&spans, 0.01, &boundaries, 2, 1.0);
        // Order is preserved even though the counts disagree.
        let flat = flatten(&groups);
        assert_eq!(flat, vec![11, 12, 13]);
        assert_eq!(groups.len(), 2);
    }

    /// An uploaded clip has no word timings at all, so the proportional fallback
    /// is the only path it can take — and it must spread the phones over the
    /// clip rather than piling them into the last word.
    #[test]
    fn an_uploaded_clip_with_no_boundaries_spreads_phones_by_its_own_duration() {
        // Four words over a 2 s clip, frames of 10 ms: midpoints at 0.05, 0.55,
        // 1.05 and 1.55 s — one per quarter.
        let spans = vec![span(11, 0, 10), span(12, 50, 60), span(13, 100, 110), span(14, 150, 160)];
        let groups = group_phones(&spans, 0.01, &[], 4, 2.0);
        assert_eq!(groups, vec![vec![11], vec![12], vec![13], vec![14]]);
    }

    #[test]
    fn no_phones_means_empty_groups_not_a_panic() {
        let groups = group_phones(&[], 0.01, &[boundary("a", 0.0, 1.0)], 1, 1.0);
        assert_eq!(groups, vec![Vec::<usize>::new()]);
        assert!(group_phones(&[], 0.01, &[], 0, 1.0).is_empty());
        assert_eq!(frame_seconds(0, 1.0), 0.0);
    }

    #[test]
    fn alignment_spans_are_cut_back_into_words() {
        let groups = vec![vec![11, 12], vec![13]];
        let spans = vec![span(11, 0, 3), span(12, 3, 6), span(13, 6, 9)];
        let split = split_by_word(&spans, &groups);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].len(), 2);
        assert_eq!(split[1], vec![span(13, 6, 9)]);
    }

    #[test]
    fn a_short_alignment_leaves_words_unscored() {
        let groups = vec![vec![11, 12], vec![13]];
        let spans = vec![span(11, 0, 3)];
        let split = split_by_word(&spans, &groups);
        assert_eq!(split, vec![Vec::new(), Vec::new()]);
    }
}
